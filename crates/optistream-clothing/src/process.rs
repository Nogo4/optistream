//! Parsing, analysis and optimisation of one clothing YDD/YTD.

use std::collections::{BTreeMap, HashSet};

use anyhow::{bail, ensure, Result};
use rayon::prelude::*;
use optistream_core::{FileAnalysis, Issue, PreviewContent, PreviewImage, PreviewItem, PreviewMesh};
use optistream_mesh::{decimate, reorder_indices, Decimated, GeometryData};
use optistream_rage::drawable::{self, geom_field, ib_field, vb_field, DrawableInfo, GeometryInfo, ShaderInfo};
use optistream_rage::gfx::{self, GfxBlock};
use optistream_rage::texture::{self as rtex, format as tf, TextureEntry};
use optistream_rage::{pages, Resource, Segment};
use optistream_texture::format::{self as fmt, chain_size, game_chain_size};
use optistream_texture::{ops, plan, Rgba8, TextureMeta, TextureRole};

use crate::config::Config;
use crate::crop::{self, Window};
use crate::shaders::{self, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ytd,
    Ydd,
}

/// What the file name and its siblings tell about a file.
#[derive(Clone, Debug, Default)]
pub struct FileInfo {
    /// Component or prop slot (`jbib`, `p_head`, ...).
    pub slot: String,
    /// Cloth piece (`.yld` next to the drawable): meshes are never touched.
    pub cloth: bool,
    /// Diffuse variation of a skin-dependent (`_r`) drawable: dimensions are kept.
    pub skin: bool,
    /// Diffuse variation (`*_diff_*.ytd`): every texture is a diffuse, whatever its name.
    pub variation: bool,
    /// Variation whose drawable only draws its diffuses with alpha shaders (see
    /// [`shaders::all_diffuse_shaders_use_alpha`]).
    pub alpha_mask: bool,
    /// UV crop window of the file's group (see [`crop`]): textures are cut to it and, in a
    /// `.ydd`, TEXCOORD0 is rewritten into it.
    pub crop: Option<Window>,
}

pub struct TexItem {
    pub entry: TextureEntry,
    pub usage: Usage,
    /// Diffuse alpha is a coverage mask (only alpha shaders draw it).
    pub alpha_mask: bool,
    /// Pixel data location in the graphics segment.
    pub data_off: Option<usize>,
    pub data_len: usize,
}

impl TexItem {
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
    pub textures: Vec<TexItem>,
}

pub fn parse(kind: Kind, bytes: &[u8], info: &FileInfo) -> Result<Parsed> {
    let res = Resource::parse(bytes)?;
    let mut drawables = Vec::new();
    let mut textures = Vec::new();
    match kind {
        Kind::Ytd => {
            for entry in rtex::read_dictionary(&res, 0)? {
                // `_n` / `_s` are also variation letters (`p_head_diff_000_n`, `bagdad_n`): a
                // variation diffuse taken for a normal map was filtered as one and lost its alpha.
                let usage = match shaders::usage_from_name(&entry.name) {
                    Usage::Role(_) if info.variation => Usage::Role(TextureRole::Diffuse),
                    other => other,
                };
                textures.push(tex_item(&res, entry, usage, info.alpha_mask));
            }
        }
        Kind::Ydd => {
            drawables = drawable::read_dictionary(&res, 0)?;
            let mut seen = HashSet::new();
            for d in &drawables {
                let Some(td) = d.texture_dict else { continue };
                if !seen.insert(td) {
                    continue;
                }
                let usages = shaders::usages_from_shaders(&d.shaders);
                let masked = shaders::alpha_masked_diffuses(&d.shaders);
                for entry in rtex::read_dictionary(&res, td)? {
                    let usage = usages
                        .get(&entry.name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or_else(|| shaders::usage_from_name(&entry.name));
                    let alpha_mask = masked.contains(&entry.name.to_ascii_lowercase());
                    textures.push(tex_item(&res, entry, usage, alpha_mask));
                }
            }
        }
    }
    Ok(Parsed { res, kind, drawables, textures })
}

/// Whether every diffuse of a YDD is drawn by alpha shaders only (what its variations inherit).
pub fn drawable_alpha_mask(bytes: &[u8]) -> Result<bool> {
    let res = Resource::parse(bytes)?;
    let drawables = drawable::read_dictionary(&res, 0)?;
    Ok(shaders::all_diffuse_shaders_use_alpha(drawables.iter().flat_map(|d| &d.shaders)))
}

fn tex_item(res: &Resource, entry: TextureEntry, usage: Usage, alpha_mask: bool) -> TexItem {
    let (data_off, data_len) = match res.resolve(entry.data_ptr) {
        Some((Segment::Graphics, off)) => {
            let exact = chain_size(entry.format, entry.width as u32, entry.height as u32, entry.levels.max(1) as u32);
            let game = game_chain_size(entry.stride as u32, entry.height as u32, entry.levels as u32);
            (Some(off), exact.max(game).min(res.graphics.len() - off))
        }
        _ => (None, 0),
    };
    TexItem { entry, usage, alpha_mask, data_off, data_len }
}

fn role_of(usage: Usage) -> Option<TextureRole> {
    match usage {
        Usage::Role(r) => Some(r),
        Usage::Protected => None,
    }
}

/// Pixel data of a texture (all stored levels), if it lives in the graphics segment.
fn tex_data<'a>(res: &'a Resource, t: &TexItem) -> Option<&'a [u8]> {
    t.data_off.map(|off| &res.graphics[off..off + t.data_len])
}

fn tex_issues(t: &TexItem, data: Option<&[u8]>, cfg: &Config, skin: bool, out: &mut Vec<Issue>) {
    let e = &t.entry;
    let name = &e.name;
    let Some(role) = role_of(t.usage) else {
        return;
    };
    let policy = cfg.texture_policy(role, skin, t.alpha_mask);
    let (w, h) = (e.width as u32, e.height as u32);
    let mut problems = Vec::new();
    if !fmt::is_pot(w) || !fmt::is_pot(h) {
        if Config::keeps_dims(role, skin) {
            out.push(Issue::info(format!("texture {name} ({}) : {w}×{h}, dimensions conservées (diffuse de peau _r)", role.label())));
        } else if policy.force_pot {
            problems.push("pas une puissance de 2".to_owned());
        }
    }
    if policy.convert_formats {
        match e.format {
            tf::BC7 | tf::DXT3 => problems.push(format!("format {}", fmt::name(e.format))),
            tf::A8R8G8B8 | tf::A8B8G8R8 | tf::X8R8G8B8 | tf::A1R5G5B5 => {
                problems.push(format!("non compressée ({})", fmt::name(e.format)))
            }
            _ => {}
        }
    }
    let full = fmt::full_levels(w, h);
    if policy.fix_mips && (e.levels as u32) < full {
        // Kept at a stored size (no re-encoding), a punch-through DXT1 keeps its short chain:
        // nothing the optimiser can do without a 1-bit alpha DXT1 encoder, so not a warning.
        let (tw, th) = plan::target_dims(&t.meta(), &policy);
        let kept_as_stored = (0..(e.levels as u32).max(1)).any(|l| fmt::level_dims(w, h, l) == (tw, th));
        if kept_as_stored && data.is_some_and(|d| plan::punch_through_blocks_mips(&t.meta(), d)) {
            out.push(Issue::info(format!(
                "texture {name} ({}) : mipmaps incomplets ({}/{full}), non complétables : alpha 1 bit DXT1",
                role.label(),
                e.levels
            )));
        } else {
            problems.push(if e.levels <= 1 { "sans mipmaps".to_owned() } else { format!("mipmaps incomplets ({}/{full})", e.levels) });
        }
    }
    if !problems.is_empty() {
        out.push(Issue::warning(format!("texture {} ({}) : {}", name, role.label(), problems.join(", "))));
    }
}

/// Heaviest High LOD among the drawables the optimiser may decimate (the polygon budget
/// applies per drawable), with its index.
fn heaviest_high(drawables: &[DrawableInfo]) -> Option<(usize, u64)> {
    drawables
        .iter()
        .enumerate()
        .filter(|(_, d)| !has_cloth(d))
        .map(|(i, d)| (i, d.lod_triangles(0)))
        .max_by_key(|&(i, t)| (t, std::cmp::Reverse(i)))
}

pub fn analyze(p: &Parsed, cfg: &Config, info: &FileInfo) -> FileAnalysis {
    let (slot, cloth_file) = (info.slot.as_str(), info.cloth);
    let mut a = FileAnalysis {
        kind: match p.kind {
            Kind::Ytd => "ytd".into(),
            Kind::Ydd => "ydd".into(),
        },
        texture_count: p.textures.len(),
        ..Default::default()
    };
    for t in &p.textures {
        tex_issues(t, tex_data(&p.res, t), cfg, info.skin, &mut a.issues);
    }
    let mut parts = Vec::new();
    if !p.textures.is_empty() {
        let max = p.textures.iter().map(|t| t.entry.width.max(t.entry.height)).max().unwrap_or(0);
        parts.push(format!("{} texture(s), max {max} px", p.textures.len()));
    }
    if p.kind == Kind::Ydd {
        let tris: u64 = p.drawables.iter().map(|d| d.lod_triangles(0)).sum();
        a.triangles = Some(tris);
        parts.push(format!("High {} tris", group(tris)));
        let budget = cfg.budget_for_slot(slot);
        if cloth_file || p.drawables.iter().any(has_cloth) {
            a.issues.push(Issue::info("cloth : maillage protégé (non décimé)"));
        }
        if !cloth_file {
            if let Some((i, worst)) = heaviest_high(&p.drawables).filter(|&(_, t)| t > budget) {
                let which = if p.drawables.len() > 1 { format!(", drawable #{i}") } else { String::new() };
                a.issues.push(Issue::warning(format!(
                    "LOD High {} triangles > budget {} ({slot}{which})",
                    group(worst),
                    group(budget)
                )));
            }
        }
    }
    a.summary = parts.join(" · ");
    a
}

fn has_cloth(d: &DrawableInfo) -> bool {
    d.shaders.iter().any(shaders::is_cloth_shader) || d.geometries().any(|g| g.decl.is_cloth())
}

pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- meshes

fn vertex_ptr(g: &GeometryInfo) -> u64 {
    if g.vertex_ptr != 0 {
        g.vertex_ptr
    } else {
        g.vertex_ptr2
    }
}

struct GeomData {
    vertices: Vec<u8>,
    indices: Vec<u16>,
}

fn read_geometry(res: &Resource, g: &GeometryInfo) -> Result<GeomData> {
    let vlen = g.vertex_count as usize * g.vertex_stride as usize;
    let ilen = g.index_count as usize * 2;
    for ptr in [vertex_ptr(g), g.index_ptr] {
        if !matches!(res.resolve(ptr), Some((Segment::System, _))) {
            bail!("vertex/index data outside the system segment");
        }
    }
    let vertices = res.slice(vertex_ptr(g), vlen)?.to_vec();
    let indices = res.slice(g.index_ptr, ilen)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok(GeomData { vertices, indices })
}

/// System-segment ranges holding raw vertex/index data (not structures).
fn raw_ranges(p: &Parsed) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for d in &p.drawables {
        for g in d.geometries() {
            let vlen = g.vertex_count as usize * g.vertex_stride as usize;
            for ptr in [g.vertex_ptr, g.vertex_ptr2] {
                if let Some((Segment::System, off)) = p.res.resolve(ptr) {
                    out.push((off, vlen));
                }
            }
            if let Some((Segment::System, off)) = p.res.resolve(g.index_ptr) {
                out.push((off, g.index_count as usize * 2));
            }
        }
    }
    out.sort_unstable();
    out
}

/// Rewrites the index buffer of a geometry (never longer than the original one).
fn write_indices(res: &mut Resource, g: &GeometryInfo, indices: &[u16]) -> Result<()> {
    ensure!(indices.len() <= g.index_count as usize, "index buffer cannot grow");
    let old_ilen = g.index_count as usize * 2;
    let ioff = res.sys_offset(g.index_ptr)?;
    let ibuf = &mut res.system[ioff..ioff + old_ilen];
    for (dst, &i) in ibuf.chunks_exact_mut(2).zip(indices) {
        dst.copy_from_slice(&i.to_le_bytes());
    }
    ibuf[indices.len() * 2..].fill(0);
    res.set_u32(g.ib_offset + ib_field::INDICES_COUNT, indices.len() as u32);
    res.set_u32(g.offset + geom_field::INDICES_COUNT, indices.len() as u32);
    res.set_u32(g.offset + geom_field::TRIANGLES_COUNT, (indices.len() / 3) as u32);
    Ok(())
}

fn write_geometry(res: &mut Resource, g: &GeometryInfo, d: &Decimated) -> Result<()> {
    let stride = g.vertex_stride as usize;
    let old_vlen = g.vertex_count as usize * stride;
    let mut targets: Vec<u64> = vec![g.vertex_ptr, g.vertex_ptr2];
    targets.retain(|&p| p != 0);
    targets.dedup();
    for ptr in targets {
        let off = res.sys_offset(ptr)?;
        let buf = &mut res.system[off..off + old_vlen];
        buf[..d.vertices.len()].copy_from_slice(&d.vertices);
        buf[d.vertices.len()..].fill(0);
    }
    let vcount = d.vertex_count(stride);
    res.set_u32(g.vb_offset + vb_field::VERTEX_COUNT, vcount as u32);
    res.set_u16(g.offset + geom_field::VERTICES_COUNT, vcount as u16);
    write_indices(res, g, &d.indices)
}

/// Shader drawing a geometry.
fn geometry_shader<'a>(d: &'a DrawableInfo, g: &GeometryInfo) -> Option<&'a ShaderInfo> {
    g.shader_index.and_then(|i| d.shaders.get(i as usize))
}

/// Geometries the decimation may touch (the others keep their triangle count).
fn is_decimable(g: &GeometryInfo, shared_vbs: &HashSet<usize>) -> bool {
    !g.decl.is_cloth()
        && !shared_vbs.contains(&g.vb_offset)
        && g.triangles() >= 64
        && g.decl.component_type(drawable::semantic::POSITION) == drawable::component::FLOAT3
}

/// Ratio applied to every decimable geometry so that the whole LOD reaches `target`
/// although the other geometries (`fixed` triangles) are left as they are.
fn lod_ratio(target: u64, decimable: u64, fixed: u64) -> f64 {
    if decimable == 0 {
        return 1.0;
    }
    (target.saturating_sub(fixed) as f64 / decimable as f64).clamp(0.0, 1.0)
}

/// Decimation plan of one LOD: per geometry, the decimated data (or `None` if kept).
fn decimate_lod(
    res: &Resource,
    d: &DrawableInfo,
    geoms: &[GeometryInfo],
    shared_vbs: &HashSet<usize>,
    target_total: u64,
    max_error: f32,
) -> Result<Vec<Option<Decimated>>> {
    let total: u64 = geoms.iter().map(|g| g.triangles() as u64).sum();
    if total == 0 || target_total >= total {
        return Ok(geoms.iter().map(|_| None).collect());
    }
    let decimable: u64 =
        geoms.iter().filter(|g| is_decimable(g, shared_vbs)).map(|g| g.triangles() as u64).sum();
    let ratio = lod_ratio(target_total, decimable, total - decimable);
    // Geometries are independent: decimate them in parallel.
    geoms
        .par_iter()
        .map(|g| {
            if !is_decimable(g, shared_vbs) {
                return Ok(None);
            }
            let data = read_geometry(res, g)?;
            let geom = GeometryData {
                decl: g.decl,
                stride: g.vertex_stride as usize,
                vertices: &data.vertices,
                indices: &data.indices,
            };
            let target = ((g.triangles() as f64) * ratio).ceil() as usize;
            decimate(&geom, target, max_error, shaders::is_order_sensitive(geometry_shader(d, g)))
        })
        .collect()
}

/// Lossless index reordering planned for the geometries left untouched by the decimation.
struct ReorderPlan {
    writes: Vec<(GeometryInfo, Vec<u16>)>,
    removed_triangles: usize,
    /// Vertex shader invocations (ACMR x triangles) of every candidate geometry.
    cost_before: f64,
    cost_after: f64,
    triangles_before: u64,
    triangles_after: u64,
}

impl ReorderPlan {
    /// A reordering alone is only worth rewriting the file for a clear gain.
    fn worth_alone(&self) -> bool {
        self.removed_triangles > 0 || self.cost_after <= self.cost_before * 0.9
    }

    fn describe(&self) -> String {
        let acmr = |cost: f64, tris: u64| if tris == 0 { 0.0 } else { cost / tris as f64 };
        let mut s = format!(
            "ordre des triangles optimisé : {} géométrie(s), ACMR {:.2} → {:.2}",
            self.writes.len(),
            acmr(self.cost_before, self.triangles_before),
            acmr(self.cost_after, self.triangles_after)
        );
        if self.removed_triangles > 0 {
            s += &format!(", {} triangle(s) dégénéré(s) ou en double retiré(s)", group(self.removed_triangles as u64));
        }
        s
    }
}

fn plan_reorder(p: &Parsed, decimated: &HashSet<usize>) -> Result<ReorderPlan> {
    // Index buffers referenced by more than one geometry are left alone.
    let mut ib_users: BTreeMap<u64, usize> = BTreeMap::new();
    for d in &p.drawables {
        for g in d.geometries() {
            *ib_users.entry(g.index_ptr).or_default() += 1;
        }
    }
    let mut candidates: Vec<(GeometryInfo, bool)> = Vec::new();
    for d in p.drawables.iter().filter(|d| !has_cloth(d)) {
        for g in d.geometries() {
            if g.decl.is_cloth() || decimated.contains(&g.offset) || ib_users[&g.index_ptr] > 1 {
                continue;
            }
            candidates.push((g.clone(), shaders::is_order_sensitive(geometry_shader(d, g))));
        }
    }
    let results: Vec<(GeometryInfo, Option<optistream_mesh::Reordered>, f32)> = candidates
        .into_par_iter()
        .map(|(g, order_sensitive)| {
            let data = read_geometry(&p.res, &g)?;
            let geom = GeometryData {
                decl: g.decl,
                stride: g.vertex_stride as usize,
                vertices: &data.vertices,
                indices: &data.indices,
            };
            let r = reorder_indices(&geom, order_sensitive)?;
            let acmr = if r.is_none() { optistream_mesh::index_acmr(&geom) } else { 0.0 };
            Ok((g, r, acmr))
        })
        .collect::<Result<_>>()?;

    let mut plan = ReorderPlan {
        writes: Vec::new(),
        removed_triangles: 0,
        cost_before: 0.0,
        cost_after: 0.0,
        triangles_before: 0,
        triangles_after: 0,
    };
    for (g, r, acmr) in results {
        let tris = g.triangles() as u64;
        plan.triangles_before += tris;
        match r {
            Some(r) => {
                let after = (r.indices.len() / 3) as u64;
                plan.triangles_after += after;
                plan.cost_before += r.acmr_before as f64 * tris as f64;
                plan.cost_after += r.acmr_after as f64 * after as f64;
                plan.removed_triangles += r.removed_triangles;
                plan.writes.push((g, r.indices));
            }
            None => {
                plan.triangles_after += tris;
                plan.cost_before += acmr as f64 * tris as f64;
                plan.cost_after += acmr as f64 * tris as f64;
            }
        }
    }
    Ok(plan)
}

fn shared_vertex_buffers(drawables: &[DrawableInfo]) -> HashSet<usize> {
    let mut count: BTreeMap<usize, usize> = BTreeMap::new();
    for d in drawables {
        for g in d.geometries() {
            *count.entry(g.vb_offset).or_default() += 1;
        }
    }
    count.into_iter().filter(|&(_, c)| c > 1).map(|(o, _)| o).collect()
}

/// Decimates the drawables in place. Returns the change descriptions; the offsets of the
/// rewritten geometries are added to `decimated`.
fn optimize_meshes(
    p: &mut Parsed,
    cfg: &Config,
    slot: &str,
    warnings: &mut Vec<String>,
    decimated: &mut HashSet<usize>,
) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    let budget = cfg.budget_for_slot(slot);
    let shared = shared_vertex_buffers(&p.drawables);
    let drawables = p.drawables.clone();
    for d in &drawables {
        if has_cloth(d) {
            continue;
        }
        let high_before = d.lod_triangles(0);
        let mut high_after = high_before;
        for lod in 0..4 {
            let before = d.lod_triangles(lod);
            let target = if lod == 0 {
                budget
            } else if cfg.lower_lods {
                high_after
            } else {
                continue;
            };
            if before <= target {
                continue;
            }
            let plan = decimate_lod(&p.res, d, &d.lods[lod], &shared, target, cfg.max_error)?;
            let mut after = 0u64;
            let mut max_err = 0f32;
            for (g, dec) in d.lods[lod].iter().zip(&plan) {
                match dec {
                    Some(dec) => {
                        write_geometry(&mut p.res, g, dec)?;
                        decimated.insert(g.offset);
                        after += (dec.indices.len() / 3) as u64;
                        max_err = max_err.max(dec.error);
                    }
                    None => after += g.triangles() as u64,
                }
            }
            if lod == 0 {
                high_after = after;
            }
            if after < before {
                let mut c = format!(
                    "LOD {} : {} → {} triangles (écart {:.2} %)",
                    drawable::LOD_NAMES[lod],
                    group(before),
                    group(after),
                    max_err * 100.0
                );
                if after > target {
                    c += " — limité par la tolérance";
                }
                changes.push(c);
            } else if lod == 0 {
                warnings.push(format!(
                    "LOD High {} triangles : aucune réduction possible sans dépasser la tolérance",
                    group(before)
                ));
            }
        }
    }
    Ok(changes)
}

// ---------------------------------------------------------------- textures

/// Optimises textures and re-lays out the graphics segment. Returns change descriptions.
/// Optimises one texture, after cutting it to the crop window if there is one. A cropped
/// texture is always rewritten (its dimensions changed) and always gets its mips back.
fn optimize_texture(t: &TexItem, original: &[u8], cfg: &Config, skin: bool, window: Option<&Window>) -> Result<Option<plan::Encoded>> {
    let Some(role) = role_of(t.usage) else {
        ensure!(window.is_none(), "texture protégée dans un groupe recadré");
        return Ok(None);
    };
    let mut policy = cfg.texture_policy(role, skin, t.alpha_mask);
    let cropped = match window {
        Some(w) => crop::crop_texture(&t.meta(), original, w)?,
        None => None,
    };
    let Some((meta, data)) = cropped else {
        return plan::optimize(&t.meta(), original, role, &policy);
    };
    policy.fix_mips = true;
    let mut enc = match plan::optimize(&meta, &data, role, &policy)? {
        Some(enc) => enc,
        None => plan::Encoded {
            width: meta.width,
            height: meta.height,
            format: meta.format,
            levels: 1,
            stride: fmt::stride(meta.format, meta.width, meta.height),
            data,
            changes: Vec::new(),
        },
    };
    // Changes are listed against the original texture, not the cropped intermediate.
    enc.changes.retain(|c| !c.starts_with("mips "));
    enc.changes.insert(0, format!("recadrée {}×{} → {}×{}", t.entry.width, t.entry.height, meta.width, meta.height));
    if enc.levels != t.entry.levels as u32 {
        enc.changes.push(format!("mips {} → {}", t.entry.levels, enc.levels));
    }
    Ok(Some(enc))
}

/// Unique pixel blocks (graphics offset → textures using them), or why the textures of the
/// file cannot be rewritten.
fn texture_blocks(p: &Parsed) -> Result<BTreeMap<usize, Vec<usize>>, String> {
    let mut blocks: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, t) in p.textures.iter().enumerate() {
        match t.data_off {
            Some(off) => blocks.entry(off).or_default().push(i),
            None => return Err(format!("texture {} : données hors du segment graphique, ignorée", t.entry.name)),
        }
    }
    // Graphics data is only reachable through pointers: once every pointer into the
    // segment is a known texture data field, unreferenced bytes (extra mip levels beyond
    // the declared count, tool padding) are dead and can be dropped.
    let fields: HashSet<usize> = p.textures.iter().map(|t| t.entry.data_ptr_field()).collect();
    let unknown = gfx::find_unknown_graphics_refs(&p.res, &fields, &raw_ranges(p));
    if !unknown.is_empty() {
        let list: Vec<String> = unknown
            .iter()
            .take(3)
            .map(|&o| format!("0x{o:X}→0x{:X}", p.res.u64_at(o).unwrap_or(0)))
            .collect();
        return Err(format!(
            "textures non modifiées : {} référence(s) graphique(s) inconnue(s) ({})",
            unknown.len(),
            list.join(", ")
        ));
    }
    Ok(blocks)
}

/// Whether every texture of the file can be cropped: a group is only planned when all of
/// its members can be rewritten, otherwise UVs and textures would disagree.
/// Returns the parsed file, for the planning that follows.
pub fn crop_ready(kind: Kind, bytes: &[u8]) -> Result<Parsed> {
    let p = parse(kind, bytes, &FileInfo::default())?;
    let blocks = texture_blocks(&p).map_err(anyhow::Error::msg)?;
    for users in blocks.values() {
        let e = &p.textures[users[0]].entry;
        ensure!(
            users.iter().all(|&u| {
                let o = &p.textures[u].entry;
                (o.width, o.height, o.format, o.levels) == (e.width, e.height, e.format, e.levels)
            }),
            "texture {} : données partagées par des en-têtes différents",
            e.name
        );
    }
    Ok(p)
}

fn optimize_textures(p: &mut Parsed, cfg: &Config, info: &FileInfo, warnings: &mut Vec<String>) -> Result<Vec<String>> {
    let skin = info.skin;
    if p.textures.is_empty() && p.res.graphics.is_empty() {
        return Ok(Vec::new());
    }
    let blocks = match texture_blocks(p) {
        Ok(b) => b,
        // The UVs of the group are rewritten: leaving these textures whole would misplace them.
        Err(why) if info.crop.is_some() => bail!("recadrage impossible : {why}"),
        Err(why) => {
            warnings.push(why);
            return Ok(Vec::new());
        }
    };

    // Pixel blocks are independent: encode them in parallel, then apply in order.
    let blocks: Vec<(usize, Vec<usize>)> = blocks.into_iter().collect();
    let encoded: Vec<Option<plan::Encoded>> = blocks
        .par_iter()
        .map(|(off, users)| {
            let first = &p.textures[users[0]];
            let same_header = users.iter().all(|&u| {
                let e = &p.textures[u].entry;
                (e.width, e.height, e.format, e.levels) == (first.entry.width, first.entry.height, first.entry.format, first.entry.levels)
            });
            if !same_header {
                ensure!(info.crop.is_none(), "texture {} : données partagées par des en-têtes différents", first.entry.name);
                return Ok(None);
            }
            let original = &p.res.graphics[*off..*off + first.data_len];
            optimize_texture(first, original, cfg, skin, info.crop.as_ref())
                .map_err(|e| anyhow::anyhow!("texture {} : {e:#}", first.entry.name))
        })
        .collect::<Result<_>>()?;

    let mut changes = Vec::new();
    let mut new_blocks = Vec::with_capacity(blocks.len());
    let mut any_change = false;
    for ((off, users), encoded) in blocks.iter().zip(encoded) {
        let (off, users) = (*off, users);
        let first = &p.textures[users[0]];
        let refs: Vec<usize> = users.iter().map(|&u| p.textures[u].entry.data_ptr_field()).collect();
        match encoded {
            Some(enc) => {
                any_change = true;
                for &u in users {
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
                changes.push(format!("texture {} : {}", first.entry.name, enc.changes.join(", ")));
                new_blocks.push(GfxBlock { data: enc.data, refs });
            }
            None => new_blocks.push(GfxBlock { data: p.res.graphics[off..off + first.data_len].to_vec(), refs }),
        }
    }

    if new_blocks.is_empty() {
        return Ok(Vec::new());
    }
    if !any_change {
        // A pure repack is still worth it when the page layout shrinks.
        let sizes: Vec<u64> = new_blocks.iter().map(|b| b.data.len() as u64).collect();
        let layout = pages::pack(&sizes, 128, false)?;
        if layout.size() >= p.res.graphics_flags.size() {
            return Ok(Vec::new());
        }
        changes.push("pages graphiques recompactées".to_owned());
    }
    gfx::rebuild(&mut p.res, new_blocks)?;
    Ok(changes)
}

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(
    kind: Kind,
    bytes: &[u8],
    cfg: &Config,
    info: &FileInfo,
) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, info)?;
    let mut warnings = Vec::new();
    let mut changes = Vec::new();
    let mut decimated = HashSet::new();
    if kind == Kind::Ydd && cfg.mesh_enabled && !info.cloth {
        changes.extend(optimize_meshes(&mut p, cfg, &info.slot, &mut warnings, &mut decimated)?);
    }
    // Planned before the textures (system segment offsets do not move), applied after:
    // on its own, a reordering only rewrites the file for a clear gain.
    let reorder = if kind == Kind::Ydd && cfg.reorder && !info.cloth {
        Some(plan_reorder(&p, &decimated)?)
    } else {
        None
    };
    if let (Kind::Ydd, Some(w)) = (kind, &info.crop) {
        let n = crop::remap_uvs(&mut p.res, &p.drawables, w)?;
        changes.push(format!("UV recadrées sur {} ({n} sommets)", w.describe()));
    }
    changes.extend(optimize_textures(&mut p, cfg, info, &mut warnings)?);
    if let Some(plan) = reorder.filter(|r| !r.writes.is_empty() && (!changes.is_empty() || r.worth_alone())) {
        for (g, indices) in &plan.writes {
            write_indices(&mut p.res, g, indices)?;
        }
        changes.push(plan.describe());
    }
    if changes.is_empty() {
        return Ok((None, changes, warnings));
    }
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, info)?;
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

fn lod_mesh(res: &Resource, geoms: &[GeometryInfo], replaced: Option<&[Option<Decimated>]>) -> Result<PreviewMesh> {
    let mut m = PreviewMesh::default();
    for (i, g) in geoms.iter().enumerate() {
        if g.decl.component_type(drawable::semantic::POSITION) != drawable::component::FLOAT3 {
            continue;
        }
        let data = read_geometry(res, g)?;
        let stride = g.vertex_stride as usize;
        let (verts, idx): (&[u8], Vec<u16>) = match replaced.and_then(|r| r[i].as_ref()) {
            Some(d) => (&d.vertices, d.indices.clone()),
            None => (&data.vertices, data.indices.clone()),
        };
        let po = g.decl.component_offset(drawable::semantic::POSITION);
        let base = m.positions.len() as u32;
        for v in verts.chunks_exact(stride) {
            let f = |o: usize| f32::from_le_bytes(v[po + o..po + o + 4].try_into().unwrap());
            m.positions.push([f(0), f(4), f(8)]);
        }
        m.indices.extend(idx.iter().map(|&i| base + i as u32));
    }
    Ok(m)
}

pub fn preview(kind: Kind, bytes: &[u8], cfg: &Config, info: &FileInfo) -> Result<Vec<PreviewItem>> {
    let (slot, cloth_file) = (info.slot.as_str(), info.cloth);
    let p = parse(kind, bytes, info)?;
    let mut items = Vec::new();

    if kind == Kind::Ydd {
        let shared = shared_vertex_buffers(&p.drawables);
        for (di, d) in p.drawables.iter().enumerate() {
            let geoms = &d.lods[0];
            if geoms.is_empty() {
                continue;
            }
            let before = lod_mesh(&p.res, geoms, None)?;
            let cloth = cloth_file || has_cloth(d);
            let budget = cfg.budget_for_slot(slot);
            let (after, after_desc) = if cloth {
                (None, "cloth : non modifié".to_owned())
            } else if !cfg.mesh_enabled || d.lod_triangles(0) <= budget {
                (None, "dans le budget : non modifié".to_owned())
            } else {
                let plan = decimate_lod(&p.res, d, geoms, &shared, budget, cfg.max_error)?;
                let m = lod_mesh(&p.res, geoms, Some(&plan))?;
                let desc = format!("{} triangles", group(m.triangles() as u64));
                (Some(m), desc)
            };
            items.push(PreviewItem {
                title: if p.drawables.len() > 1 {
                    format!("Maillage #{di} — LOD High")
                } else {
                    "Maillage — LOD High".to_owned()
                },
                before_desc: format!("{} triangles · {} vertices", group(before.triangles() as u64), group(d.lod_vertices(0))),
                after_desc,
                content: PreviewContent::Mesh { before, after },
            });
        }
    }

    for t in &p.textures {
        let Some(off) = t.data_off else { continue };
        let data = &p.res.graphics[off..off + t.data_len];
        let meta = t.meta();
        let before = match plan::decode_top(&meta, data) {
            Ok(img) => img,
            Err(_) => continue,
        };
        let before_desc = tex_desc(meta.width, meta.height, meta.format, meta.levels, t.data_len);
        let (after, after_desc) = match role_of(t.usage) {
            None => (None, "protégée : non modifiée".to_owned()),
            Some(_) => match optimize_texture(t, data, cfg, info.skin, info.crop.as_ref())? {
                Some(enc) => {
                    let m = TextureMeta { width: enc.width, height: enc.height, format: enc.format, levels: enc.levels };
                    let img = plan::decode_top(&m, &enc.data)?;
                    let mut desc = tex_desc(enc.width, enc.height, enc.format, enc.levels, enc.data.len());
                    if let Some(c) = enc.changes.first().filter(|c| c.starts_with("recadrée")) {
                        desc = format!("{desc} · {c}");
                    }
                    (Some(preview_image(&img)), desc)
                }
                None => (None, "déjà conforme : non modifiée".to_owned()),
            },
        };
        let role = role_of(t.usage).map(|r| r.label()).unwrap_or("protégée");
        items.push(PreviewItem {
            title: format!("Texture {} ({role})", t.entry.name),
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
    use optistream_rage::drawable::VertexDecl;

    fn geometry(offset: usize, triangles: u32) -> GeometryInfo {
        GeometryInfo {
            offset,
            vb_offset: offset + 1,
            ib_offset: offset + 2,
            decl: VertexDecl { flags: 1, stride: 12, count: 1, types: drawable::component::FLOAT3 as u64 },
            shader_index: Some(0),
            vertex_stride: 12,
            vertex_count: triangles * 3,
            vertex_ptr: 0,
            vertex_ptr2: 0,
            index_ptr: 0,
            index_count: triangles * 3,
        }
    }

    fn drawable_with_high(offset: usize, triangles: u32) -> DrawableInfo {
        DrawableInfo {
            offset,
            hash: 0,
            has_skeleton: true,
            shaders: Vec::new(),
            texture_dict: None,
            lods: [vec![geometry(offset + 16, triangles)], Vec::new(), Vec::new(), Vec::new()],
        }
    }

    #[test]
    fn high_budget_is_checked_per_drawable() {
        // Two drawables at 0.6 x budget each: the optimiser leaves both alone, so must the analysis.
        let budget = 40_000u64;
        let ds = [drawable_with_high(0, 24_000), drawable_with_high(1000, 24_000)];
        let (_, worst) = heaviest_high(&ds).unwrap();
        assert!(worst <= budget);
        let ds = [drawable_with_high(0, 24_000), drawable_with_high(1000, 45_000)];
        assert_eq!(heaviest_high(&ds), Some((1, 45_000)));
    }

    #[test]
    fn ratio_compensates_for_fixed_geometries() {
        // 50k tris, 35k of them not decimable, target 40k: the 15k left must give 10k.
        let r = lod_ratio(40_000, 15_000, 35_000);
        assert!((r - 5_000.0 / 15_000.0).abs() < 1e-9);
        assert_eq!(lod_ratio(40_000, 10_000, 45_000), 0.0);
        assert_eq!(lod_ratio(40_000, 0, 50_000), 1.0);
        assert_eq!(lod_ratio(60_000, 50_000, 0), 1.0);
    }

    #[test]
    fn small_and_shared_geometries_are_not_decimable() {
        let shared: HashSet<usize> = [101].into_iter().collect();
        assert!(is_decimable(&geometry(0, 64), &shared));
        assert!(!is_decimable(&geometry(0, 63), &shared));
        assert!(!is_decimable(&geometry(100, 1000), &shared));
    }
}
