//! Reduction plan of one file: corrections, then the cheapest halvings until it fits the budget.

use anyhow::Result;
use optistream_core::mib;
use optistream_rage::gfx;
use optistream_texture::format::{self as fmt, chain_size, full_levels};
use optistream_texture::{ops, plan, quality, resource as texres, Encoded, Rgba8, TextureRole};
use rayon::prelude::*;

use crate::config::{Config, FORCE_MAX_STEPS, FORCE_MIN_SIZE};
use crate::process::{Parsed, Tex};
use crate::roles::Usage;

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
pub(super) struct Candidate {
    pub(super) block: usize,
    /// Format, dimensions and levels after the corrections (stage 1).
    pub(super) format: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) levels: u32,
    /// `loss[k]` = loss of `k` halvings (index 0 = 0.0); only admissible steps are listed.
    pub(super) loss: Vec<f64>,
    /// `tier[k]` of step `k`: 0 within the limits, 1 past them (forced), 2 forced on a text /
    /// logo or a texture of unknown role. Lower tiers are always spent first.
    pub(super) tier: Vec<u8>,
}

impl Candidate {
    pub(super) fn size(&self, steps: u32, fix_mips: bool) -> u64 {
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
pub(crate) fn reference(t: &Tex, data: &[u8], width: u32, height: u32, role: TextureRole) -> Option<Rgba8> {
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

pub(super) fn describe(p: &Parsed, b: &BlockPlan) -> Option<String> {
    let enc = b.enc.as_ref()?;
    let t = &p.textures[b.users[0]];
    let mut s = format!("texture {} ({}) : {}", t.entry.name, t.usage.label(), enc.changes.join(", "));
    if b.steps > 0 {
        s += &format!(" — réduction ciblée ÷{} (perte {:.1} %){}", 1u32 << b.steps, b.loss, if b.forced { ", forcée" } else { "" });
    }
    Some(s)
}

// ---------------------------------------------------------------- optimisation
