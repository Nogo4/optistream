//! Parsing, analysis and optimisation of one map file (`.ytd`, `.ydr`, `.ydd`).
//!
//! Two stages per file:
//! 1. corrections (format conversion, power of two, complete mips), as for clothing;
//! 2. when the declared physical memory is still above the budget, targeted reduction:
//!    the textures losing the least detail per byte saved are halved one step at a time
//!    until the file fits, within the limits (steps per texture, minimum size, maximum
//!    loss against the original). A file that cannot fit stays above and says why.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use optistream_core::{mib, FileAnalysis, Issue, PreviewContent, PreviewImage, PreviewItem};
use optistream_rage::drawable::{self, DrawableInfo};
use optistream_rage::gfx::{self, GfxBlock};
use optistream_rage::texture::{self as rtex, format as tf, TextureEntry};
use optistream_rage::Resource;
use optistream_texture::format::{self as fmt, chain_size, full_levels, level_dims};
use optistream_texture::resource as texres;
use optistream_texture::{ops, plan, quality, Encoded, Rgba8, TextureMeta, TextureRole};
use rayon::prelude::*;

use crate::config::{Config, FORCE_MAX_STEPS, FORCE_MIN_SIZE};
use crate::roles::{self, RoleIndex, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ytd,
    Ydr,
    Ydd,
}

impl Kind {
    pub fn of(file_name_lower: &str) -> Option<Kind> {
        match file_name_lower.rsplit('.').next()? {
            "ytd" => Some(Kind::Ytd),
            "ydr" => Some(Kind::Ydr),
            "ydd" => Some(Kind::Ydd),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ytd => "ytd",
            Kind::Ydr => "ydr",
            Kind::Ydd => "ydd",
        }
    }
}

pub struct Tex {
    pub entry: TextureEntry,
    pub usage: Usage,
    /// Text or graphics read up close (name): kept at its size by the targeted reduction.
    pub readable: bool,
    pub data_off: Option<usize>,
    pub data_len: usize,
}

impl Tex {
    pub fn meta(&self) -> TextureMeta {
        TextureMeta {
            width: self.entry.width as u32,
            height: self.entry.height as u32,
            format: self.entry.format,
            levels: self.entry.levels as u32,
        }
    }
}

pub struct Parsed {
    pub res: Resource,
    pub kind: Kind,
    pub drawables: Vec<DrawableInfo>,
    pub textures: Vec<Tex>,
}

/// Drawables of a `.ydr` (one, at the root) or a `.ydd` (dictionary at the root).
pub fn read_drawables(res: &Resource, kind: Kind) -> Result<Vec<DrawableInfo>> {
    match kind {
        Kind::Ytd => Ok(Vec::new()),
        Kind::Ydr => Ok(vec![drawable::read_drawable(res, 0, 0)?]),
        Kind::Ydd => drawable::read_dictionary(res, 0),
    }
}

fn tex(res: &Resource, entry: TextureEntry, usage: Usage) -> Tex {
    let (data_off, data_len) = match texres::locate(res, &entry) {
        Some((off, len)) => (Some(off), len),
        None => (None, 0),
    };
    let readable = roles::readable_by_name(&entry.name);
    Tex { entry, usage, readable, data_off, data_len }
}

/// Role of a texture: its own drawable's shaders, then the resource's, then its name.
fn usage(entry: &TextureEntry, own: Option<&RoleIndex>, resource: &RoleIndex) -> Usage {
    let name = &entry.name;
    own.and_then(|i| i.get(name)).or_else(|| resource.get(name)).unwrap_or_else(|| {
        if roles::is_lookup_shaped(entry.width, entry.height, entry.format) {
            Usage::Protected
        } else {
            roles::usage_from_name(name)
        }
    })
}

pub fn parse(kind: Kind, bytes: &[u8], index: &RoleIndex) -> Result<Parsed> {
    let res = Resource::parse(bytes)?;
    let drawables = read_drawables(&res, kind)?;
    let mut textures = Vec::new();
    if kind == Kind::Ytd {
        for entry in rtex::read_dictionary(&res, 0)? {
            let u = usage(&entry, None, index);
            textures.push(tex(&res, entry, u));
        }
    } else {
        let mut seen = std::collections::HashSet::new();
        for d in &drawables {
            let Some(td) = d.texture_dict else { continue };
            if !seen.insert(td) {
                continue;
            }
            let mut own = RoleIndex::default();
            own.add_shaders(&d.shaders);
            for entry in rtex::read_dictionary(&res, td)? {
                let u = usage(&entry, Some(&own), index);
                textures.push(tex(&res, entry, u));
            }
        }
    }
    Ok(Parsed { res, kind, drawables, textures })
}

fn tex_data<'a>(res: &'a Resource, t: &Tex) -> Option<&'a [u8]> {
    t.data_off.map(|off| &res.graphics[off..off + t.data_len])
}

// ---------------------------------------------------------------- analysis

fn tex_issues(t: &Tex, data: Option<&[u8]>, cfg: &Config, out: &mut Vec<Issue>) {
    let e = &t.entry;
    let Usage::Role(role) = t.usage else { return };
    let policy = cfg.texture_policy(0);
    let (w, h) = (e.width as u32, e.height as u32);
    let mut problems = Vec::new();
    if policy.force_pot && (!fmt::is_pot(w) || !fmt::is_pot(h)) {
        problems.push("pas une puissance de 2".to_owned());
    }
    if policy.convert_formats {
        match e.format {
            tf::BC7 | tf::DXT3 => problems.push(format!("format {}", fmt::name(e.format))),
            tf::A8R8G8B8 | tf::A8B8G8R8 | tf::X8R8G8B8 | tf::A1R5G5B5 | tf::A16B16G16R16 => {
                problems.push(format!("non compressée ({})", fmt::name(e.format)))
            }
            _ => {}
        }
    }
    if fmt::layout(e.format).is_none() {
        out.push(Issue::info(format!("texture {} ({}) : format {} non géré, laissée telle quelle", e.name, role.label(), fmt::name(e.format))));
    }
    let full = full_levels(w, h);
    if policy.fix_mips && (e.levels as u32) < full {
        let (tw, th) = plan::target_dims(&t.meta(), &policy);
        let kept_as_stored = (0..(e.levels as u32).max(1)).any(|l| level_dims(w, h, l) == (tw, th));
        if !(kept_as_stored && data.is_some_and(|d| plan::punch_through_blocks_mips(&t.meta(), d))) {
            problems.push(if e.levels <= 1 { "sans mipmaps".to_owned() } else { format!("mipmaps incomplets ({}/{full})", e.levels) });
        }
    }
    if !problems.is_empty() {
        out.push(Issue::warning(format!("texture {} ({}) : {}", e.name, role.label(), problems.join(", "))));
    }
}

pub fn analyze(p: &Parsed, cfg: &Config) -> FileAnalysis {
    let mut a = FileAnalysis { kind: p.kind.label().into(), texture_count: p.textures.len(), ..Default::default() };
    for t in &p.textures {
        tex_issues(t, tex_data(&p.res, t), cfg, &mut a.issues);
    }
    let mut parts = Vec::new();
    if !p.textures.is_empty() {
        let max = p.textures.iter().map(|t| t.entry.width.max(t.entry.height)).max().unwrap_or(0);
        parts.push(format!("{} texture(s), max {max} px", p.textures.len()));
    } else {
        parts.push("aucune texture".to_owned());
    }
    let physical = p.res.graphics_flags.size();
    if cfg.budget_enabled && physical > cfg.budget && !p.textures.is_empty() {
        a.issues.push(Issue::info(format!(
            "au-dessus du budget de {} : réduction ciblée des textures qui perdent le moins",
            mib(cfg.budget)
        )));
    }
    let protected = p.textures.iter().filter(|t| t.usage == Usage::Protected).count();
    if protected > 0 {
        a.issues.push(Issue::info(format!("{protected} texture(s) protégée(s) (émissive, palette, lookup…) : non modifiées")));
    }
    a.summary = parts.join(" · ");
    a
}

// ---------------------------------------------------------------- planning

/// One unique pixel block and what the plan does with it.
pub struct BlockPlan {
    pub off: usize,
    /// Indices into `Parsed::textures` of the headers sharing this block.
    pub users: Vec<usize>,
    /// New content, `None` to keep the stored bytes.
    pub enc: Option<Encoded>,
    /// Halvings applied by the targeted reduction.
    pub steps: u32,
    /// Measured loss of those halvings (percent, against the original).
    pub loss: f64,
    /// Whether the reduction went past the configured limits ("Forcer sous le budget").
    pub forced: bool,
}

pub struct FilePlan {
    pub blocks: Vec<BlockPlan>,
    pub warnings: Vec<String>,
    /// Declared physical memory after the plan.
    pub physical_after: u64,
}

/// Targeted reduction candidate.
struct Candidate {
    block: usize,
    /// Format, dimensions and levels after the corrections (stage 1).
    format: u32,
    width: u32,
    height: u32,
    levels: u32,
    /// `loss[k]` = loss of `k` halvings (index 0 = 0.0); only admissible steps are listed.
    loss: Vec<f64>,
    /// `tier[k]` of step `k`: 0 within the limits, 1 past them (forced), 2 forced on a text /
    /// logo or a texture of unknown role. Lower tiers are always spent first.
    tier: Vec<u8>,
}

impl Candidate {
    fn size(&self, steps: u32, fix_mips: bool) -> u64 {
        let (w, h) = (self.width >> steps, self.height >> steps);
        let levels = if fix_mips { full_levels(w, h) } else { self.levels.saturating_sub(steps).max(1).min(full_levels(w, h)) };
        chain_size(self.format, w, h, levels) as u64
    }
}

/// Why textures could not be reduced further (for the warning of a file left above budget).
#[derive(Default)]
struct Blocked {
    protected: u64,
    readable: u64,
    min_size: u64,
    loss: u64,
    max_steps: u64,
}

fn same_header(p: &Parsed, users: &[usize]) -> bool {
    let e = &p.textures[users[0]].entry;
    users.iter().all(|&u| {
        let o = &p.textures[u].entry;
        (o.width, o.height, o.format, o.levels) == (e.width, e.height, e.format, e.levels)
    })
}

/// Original level 0 at the corrected dimensions (power-of-two floor), for loss measures.
fn reference(t: &Tex, data: &[u8], width: u32, height: u32, role: TextureRole) -> Option<Rgba8> {
    let top = plan::decode_top(&t.meta(), data).ok()?;
    if (top.width, top.height) == (width, height) {
        Some(top)
    } else {
        Some(ops::resize(&top, width, height, role.color_space()))
    }
}

/// Plans a file. Map textures are often stored without mips: completing them costs about a
/// third more memory, which must never make a file heavier than it was. When it would,
/// the file is planned again with the mips left as they are.
pub fn plan_file(p: &Parsed, cfg: &Config) -> Result<FilePlan> {
    let before = p.res.graphics_flags.size();
    let fp = plan_with(p, cfg)?;
    if fp.physical_after <= before || !cfg.fix_mips {
        return Ok(fp);
    }
    let no_mips = Config { fix_mips: false, ..cfg.clone() };
    let mut alt = plan_with(p, &no_mips)?;
    if alt.physical_after < fp.physical_after {
        alt.warnings.insert(
            0,
            format!(
                "mipmaps manquants non complétés : la mémoire serait passée de {} à {}",
                mib(before),
                mib(fp.physical_after)
            ),
        );
        return Ok(alt);
    }
    Ok(fp)
}

fn plan_with(p: &Parsed, cfg: &Config) -> Result<FilePlan> {
    let mut warnings = Vec::new();
    let users: Vec<texres::BlockUser> = p
        .textures
        .iter()
        .map(|t| texres::BlockUser { name: &t.entry.name, data_off: t.data_off, ptr_field: t.entry.data_ptr_field(), name_field: t.entry.name_ptr_field() })
        .collect();
    let physical_before = p.res.graphics_flags.size();
    let blocks = match texres::texture_blocks(&p.res, &users, &texres::raw_ranges(&p.res, &p.drawables)) {
        Ok(b) => b,
        Err(why) => {
            warnings.push(why);
            return Ok(FilePlan { blocks: Vec::new(), warnings, physical_after: physical_before });
        }
    };
    let blocks: Vec<(usize, Vec<usize>)> = blocks.into_iter().collect();
    let original = |i: usize| -> &[u8] {
        let (off, users) = &blocks[i];
        &p.res.graphics[*off..*off + p.textures[users[0]].data_len]
    };
    let role_of = |i: usize| -> Option<TextureRole> {
        let users = &blocks[i].1;
        if !same_header(p, users) {
            return None;
        }
        match p.textures[users[0]].usage {
            Usage::Role(r) => Some(r),
            Usage::Protected => None,
        }
    };
    let encode = |i: usize, steps: u32| -> Result<Option<Encoded>> {
        let Some(role) = role_of(i) else { return Ok(None) };
        let t = &p.textures[blocks[i].1[0]];
        plan::optimize(&t.meta(), original(i), role, &cfg.texture_policy(steps))
            .map_err(|e| anyhow::anyhow!("texture {} : {e:#}", t.entry.name))
    };

    // Stage 1: corrections only.
    let mut encoded: Vec<Option<Encoded>> = (0..blocks.len()).into_par_iter().map(|i| encode(i, 0)).collect::<Result<_>>()?;
    let mut steps = vec![0u32; blocks.len()];
    let mut losses = vec![0f64; blocks.len()];
    let mut forced = vec![false; blocks.len()];
    let size_of = |enc: &Option<Encoded>, i: usize| enc.as_ref().map(|e| e.data.len()).unwrap_or(original(i).len()) as u64;
    let mut sizes: Vec<u64> = (0..blocks.len()).map(|i| size_of(&encoded[i], i)).collect();
    let mut physical = gfx::packed_size(&p.res, &sizes)?;

    // Stage 2: targeted reduction.
    if cfg.budget_enabled && physical > cfg.budget {
        // Effective limits: the forced reduction goes past the configured ones, down to floors.
        let floor = if cfg.force { FORCE_MIN_SIZE.min(cfg.min_size) } else { cfg.min_size };
        let step_cap = if cfg.force { FORCE_MAX_STEPS.max(cfg.max_steps) } else { cfg.max_steps };
        let mut blocked = Blocked::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        for i in 0..blocks.len() {
            let t = &p.textures[blocks[i].1[0]];
            let Some(role) = role_of(i) else {
                blocked.protected += sizes[i];
                continue;
            };
            // Detail, height, dirt maps (unknown sampling) and texts / logos: only when forced, last.
            let base_tier = if role == TextureRole::Other || t.readable {
                if !cfg.force {
                    if t.readable {
                        blocked.readable += sizes[i];
                    } else {
                        blocked.protected += sizes[i];
                    }
                    continue;
                }
                2
            } else {
                0
            };
            let (format, width, height, levels) = match &encoded[i] {
                Some(e) => (e.format, e.width, e.height, e.levels),
                None => (t.entry.format, t.entry.width as u32, t.entry.height as u32, t.entry.levels as u32),
            };
            if fmt::layout(format).is_none() || width.min(height) / 2 < floor {
                blocked.min_size += sizes[i];
                continue;
            }
            candidates.push(Candidate { block: i, format, width, height, levels, loss: vec![0.0], tier: vec![base_tier] });
        }
        // Loss of each admissible step, measured in parallel (the costly part).
        candidates.par_iter_mut().for_each(|c| {
            let t = &p.textures[blocks[c.block].1[0]];
            let role = role_of(c.block).unwrap_or(TextureRole::Other);
            let Some(top) = reference(t, original(c.block), c.width, c.height, role) else { return };
            let alpha = top.alpha_used();
            for k in 1..=step_cap {
                let side = c.width.min(c.height) >> k;
                if side < floor {
                    break;
                }
                let l = quality::reduction_loss(&top, k, role, alpha);
                let past = k > cfg.max_steps || side < cfg.min_size || l > cfg.max_loss;
                if past && !cfg.force {
                    break;
                }
                let tier = c.tier[k as usize - 1].max(past as u8);
                c.loss.push(l);
                c.tier.push(tier);
            }
        });

        // Greedy choice; the estimated sizes are checked against real encodings after
        // each round, and the loop goes on while the file is still above the budget.
        for _round in 0..6 {
            loop {
                if physical <= cfg.budget {
                    break;
                }
                let best = candidates
                    .iter()
                    .enumerate()
                    .filter_map(|(ci, c)| {
                        let k = steps[c.block] + 1;
                        let next = *c.loss.get(k as usize)?;
                        let gain = sizes[c.block].saturating_sub(c.size(k, cfg.fix_mips)) as f64;
                        let score = gain / (next - c.loss[k as usize - 1] + 0.05);
                        (gain > 0.0).then(|| (ci, c.tier[k as usize], score))
                    })
                    .max_by(|a, b| b.1.cmp(&a.1).then(a.2.total_cmp(&b.2)));
                let Some((ci, _, _)) = best else { break };
                let c = &candidates[ci];
                steps[c.block] += 1;
                losses[c.block] = c.loss[steps[c.block] as usize];
                forced[c.block] = c.tier[steps[c.block] as usize] > 0;
                sizes[c.block] = c.size(steps[c.block], cfg.fix_mips);
                physical = gfx::packed_size(&p.res, &sizes)?;
            }
            // Real encodings of the reduced textures.
            let todo: Vec<usize> = (0..blocks.len())
                .filter(|&i| steps[i] > 0 && encoded[i].as_ref().map(|e| e.width) != Some(candidate_width(&candidates, i, steps[i])))
                .collect();
            let fresh: Vec<(usize, Option<Encoded>)> =
                todo.par_iter().map(|&i| encode(i, steps[i]).map(|e| (i, e))).collect::<Result<_>>()?;
            for (i, e) in fresh {
                sizes[i] = size_of(&e, i);
                encoded[i] = e;
            }
            physical = gfx::packed_size(&p.res, &sizes)?;
            if physical <= cfg.budget {
                break;
            }
        }

        if physical > cfg.budget {
            for c in &candidates {
                let k = steps[c.block];
                if (k as usize) + 1 < c.loss.len() {
                    continue;
                }
                if k >= step_cap {
                    blocked.max_steps += sizes[c.block];
                } else if c.width.min(c.height) >> (k + 1) < floor {
                    blocked.min_size += sizes[c.block];
                } else {
                    blocked.loss += sizes[c.block];
                }
            }
            let mut why = Vec::new();
            for (bytes, label) in [
                (blocked.loss, "perte maximale atteinte"),
                (blocked.min_size, "à la taille minimale"),
                (blocked.max_steps, "divisions max atteintes"),
                (blocked.readable, "textes / logos / enseignes (nom)"),
                (blocked.protected, "protégées ou rôle inconnu"),
            ] {
                if bytes > 0 {
                    why.push(format!("{label} : {}", mib(bytes)));
                }
            }
            warnings.push(format!(
                "reste à {} (budget {}) : limites atteintes, textures restantes — {}",
                mib(physical),
                mib(cfg.budget),
                why.join(", ")
            ));
        }
    }

    let blocks = blocks
        .into_iter()
        .zip(encoded)
        .enumerate()
        .map(|(i, ((off, users), enc))| BlockPlan { off, users, enc, steps: steps[i], loss: losses[i], forced: forced[i] })
        .collect();
    Ok(FilePlan { blocks, warnings, physical_after: physical })
}

/// Expected width of block `i` after `steps` halvings (to know whether its encoding is current).
fn candidate_width(candidates: &[Candidate], i: usize, steps: u32) -> u32 {
    candidates.iter().find(|c| c.block == i).map(|c| c.width >> steps).unwrap_or(0)
}

fn describe(p: &Parsed, b: &BlockPlan) -> Option<String> {
    let enc = b.enc.as_ref()?;
    let t = &p.textures[b.users[0]];
    let mut s = format!("texture {} ({}) : {}", t.entry.name, t.usage.label(), enc.changes.join(", "));
    if b.steps > 0 {
        s += &format!(" — réduction ciblée ÷{} (perte {:.1} %){}", 1u32 << b.steps, b.loss, if b.forced { ", forcée" } else { "" });
    }
    Some(s)
}

// ---------------------------------------------------------------- optimisation

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(kind: Kind, bytes: &[u8], cfg: &Config, index: &RoleIndex) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, index)?;
    if p.textures.is_empty() {
        return Ok((None, Vec::new(), Vec::new()));
    }
    let fp = plan_file(&p, cfg)?;
    let mut changes: Vec<String> = fp.blocks.iter().filter_map(|b| describe(&p, b)).collect();
    let mut warnings = fp.warnings;
    if fp.blocks.is_empty() {
        return Ok((None, changes, warnings));
    }
    let before = p.res.graphics_flags.size();
    if fp.physical_after > before {
        // Format fixes alone would make the file heavier (BC7 → DXT5 is the same size,
        // uncompressed alpha textures...): keep the original.
        warnings.push(format!("non modifié : le résultat ({}) serait plus lourd que l'original ({})", mib(fp.physical_after), mib(before)));
        return Ok((None, Vec::new(), warnings));
    }
    let any_change = !changes.is_empty();
    if !any_change && fp.physical_after >= p.res.graphics_flags.size() {
        return Ok((None, changes, warnings));
    }
    let mut new_blocks = Vec::with_capacity(fp.blocks.len());
    for b in fp.blocks {
        let refs: Vec<usize> = b.users.iter().map(|&u| p.textures[u].entry.data_ptr_field()).collect();
        match b.enc {
            Some(enc) => {
                for &u in &b.users {
                    rtex::write_texture_header(
                        &mut p.res,
                        p.textures[u].entry.struct_offset,
                        enc.width as u16,
                        enc.height as u16,
                        enc.stride,
                        enc.format,
                        enc.levels as u8,
                    );
                }
                new_blocks.push(GfxBlock { data: enc.data, refs });
            }
            None => {
                let len = p.textures[b.users[0]].data_len;
                new_blocks.push(GfxBlock { data: p.res.graphics[b.off..b.off + len].to_vec(), refs });
            }
        }
    }
    if !any_change {
        changes.push("pages graphiques recompactées".to_owned());
    }
    gfx::rebuild(&mut p.res, new_blocks)?;
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, index)?;
    if check.textures.len() != p.textures.len() || check.drawables.len() != p.drawables.len() {
        bail!("vérification après écriture échouée");
    }
    Ok((Some(out), changes, warnings))
}

// ---------------------------------------------------------------- previews

fn preview_image(img: &Rgba8) -> PreviewImage {
    let small = ops::preview(img, 1024);
    PreviewImage { width: small.width, height: small.height, rgba: small.data }
}

fn tex_desc(w: u32, h: u32, format: u32, levels: u32, bytes: usize) -> String {
    format!("{w}×{h} {} · {levels} mips · {:.2} Mio", fmt::name(format), bytes as f64 / 1048576.0)
}

pub fn preview(kind: Kind, bytes: &[u8], cfg: &Config, index: &RoleIndex) -> Result<Vec<PreviewItem>> {
    let p = parse(kind, bytes, index)?;
    let fp = plan_file(&p, cfg)?;
    let by_texture: BTreeMap<usize, &BlockPlan> = fp.blocks.iter().flat_map(|b| b.users.iter().map(move |&u| (u, b))).collect();
    let mut items = Vec::new();
    // Biggest textures first: they are the ones the reduction is about.
    let mut order: Vec<usize> = (0..p.textures.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(p.textures[i].data_len));
    for i in order {
        let t = &p.textures[i];
        let Some(data) = tex_data(&p.res, t) else { continue };
        let meta = t.meta();
        let Ok(before) = plan::decode_top(&meta, data) else { continue };
        let before_desc = tex_desc(meta.width, meta.height, meta.format, meta.levels, t.data_len);
        let (after, after_desc) = match by_texture.get(&i) {
            _ if t.usage == Usage::Protected => (None, "protégée : non modifiée".to_owned()),
            Some(b) if b.enc.is_some() => {
                let enc = b.enc.as_ref().unwrap();
                let m = TextureMeta { width: enc.width, height: enc.height, format: enc.format, levels: enc.levels };
                let img = plan::decode_top(&m, &enc.data)?;
                let mut desc = tex_desc(enc.width, enc.height, enc.format, enc.levels, enc.data.len());
                if b.steps > 0 {
                    desc += &format!(" · réduction ciblée ÷{} (perte {:.1} %){}", 1u32 << b.steps, b.loss, if b.forced { ", forcée" } else { "" });
                }
                (Some(preview_image(&img)), desc)
            }
            _ => (None, if t.readable { "texte / logo : taille conservée".to_owned() } else { "non modifiée".to_owned() }),
        };
        items.push(PreviewItem {
            title: format!("Texture {} ({})", t.entry.name, t.usage.label()),
            before_desc,
            after_desc,
            content: PreviewContent::Texture { before: preview_image(&before), after },
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_sizes_follow_halvings() {
        let c = Candidate { block: 0, format: tf::DXT1, width: 2048, height: 2048, levels: 12, loss: vec![0.0], tier: vec![0] };
        let full = c.size(0, true);
        let half = c.size(1, true);
        // DXT1 2048² full chain ≈ 2.67 Mio; one halving keeps about a quarter.
        assert!(full > 2_700_000 && full < 2_900_000, "{full}");
        assert!(half * 4 >= full - 64 && half * 4 <= full + 64, "{half}");
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(Kind::of("a.ytd"), Some(Kind::Ytd));
        assert_eq!(Kind::of("b.ydr"), Some(Kind::Ydr));
        assert_eq!(Kind::of("c.ydd"), Some(Kind::Ydd));
        assert_eq!(Kind::of("d.yft"), None);
    }
}
