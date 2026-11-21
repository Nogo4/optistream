//! The reduction plan of one file.

use anyhow::Result;
use optistream_i18n::{t, tf};
use optistream_rage::gfx;
use optistream_rage::Resource;
use rayon::prelude::*;

use super::{mib, Access, BlockPlan, BudgetTex, Candidate, FilePlan, Hold, Limits, PAST};
use crate::format::{self as fmt};
use crate::{ops, plan, quality, resource as texres, Encoded, Rgba8, TextureRole};

/// Why textures could not be reduced further (for the warning of a file left above budget).
#[derive(Default)]
struct Blocked {
    loss: u64,
    min_size: u64,
    max_steps: u64,
    holds: Vec<(Hold, u64)>,
}

impl Blocked {
    fn hold(&mut self, h: Hold, bytes: u64) {
        match self.holds.iter_mut().find(|(o, _)| *o == h) {
            Some((_, b)) => *b += bytes,
            None => self.holds.push((h, bytes)),
        }
    }
}

/// Limits of one candidate, from its rule and the run.
struct Bounds {
    /// Floor of the limits or of the forced reduction, without the hard floor.
    floor: u32,
    /// `floor` raised to the hard floor.
    hard: u32,
    step_cap: u32,
}

fn same_header(textures: &[BudgetTex], users: &[usize]) -> bool {
    let e = textures[users[0]].entry;
    users.iter().all(|&u| {
        let o = textures[u].entry;
        (o.width, o.height, o.format, o.levels) == (e.width, e.height, e.format, e.levels)
    })
}

/// Original level 0 at the corrected dimensions (power-of-two floor), for loss measures.
fn reference(t: &BudgetTex, data: &[u8], width: u32, height: u32, role: TextureRole) -> Option<Rgba8> {
    let top = plan::decode_top(&t.meta(), data).ok()?;
    if (top.width, top.height) == (width, height) {
        Some(top)
    } else {
        Some(ops::resize(&top, width, height, role.color_space()))
    }
}

/// Plans a file. `raw` lists the system ranges holding raw vertex / index data (skipped by
/// the unknown pointer guard). Textures stored without mips (maps) cost about a third more
/// once completed, which must never make a file heavier than it was: when it would, the
/// file is planned again with the mips left as they are.
pub fn plan_file(res: &Resource, raw: &[(usize, usize)], textures: &[BudgetTex], limits: &Limits) -> Result<FilePlan> {
    let before = res.graphics_flags.size();
    let fp = plan_with(res, raw, textures, limits)?;
    if fp.physical_after <= before || !limits.policy.fix_mips {
        return Ok(fp);
    }
    let mut no_mips = limits.clone();
    no_mips.policy.fix_mips = false;
    let mut alt = plan_with(res, raw, textures, &no_mips)?;
    if alt.physical_after < fp.physical_after {
        alt.warnings.insert(
            0,
            tf!("texture.budget.mips_not_completed", before = mib(before), after = mib(fp.physical_after)),
        );
        return Ok(alt);
    }
    Ok(fp)
}

fn plan_with(res: &Resource, raw: &[(usize, usize)], textures: &[BudgetTex], limits: &Limits) -> Result<FilePlan> {
    let mut warnings = Vec::new();
    let users: Vec<texres::BlockUser> = textures
        .iter()
        .map(|t| texres::BlockUser {
            name: &t.entry.name,
            data_off: t.data_off,
            ptr_field: t.entry.data_ptr_field(),
            name_field: t.entry.name_ptr_field(),
        })
        .collect();
    let physical_before = res.graphics_flags.size();
    let blocks = match texres::texture_blocks(res, &users, raw) {
        Ok(b) => b,
        Err(why) => {
            warnings.push(why);
            return Ok(FilePlan { blocks: Vec::new(), warnings, physical_after: physical_before });
        }
    };
    let blocks: Vec<(usize, Vec<usize>)> = blocks.into_iter().collect();
    let original = |i: usize| -> &[u8] {
        let (off, users) = &blocks[i];
        &res.graphics[*off..*off + textures[users[0]].data_len]
    };
    let role_of = |i: usize| -> Option<TextureRole> {
        let users = &blocks[i].1;
        if !same_header(textures, users) {
            return None;
        }
        textures[users[0]].role
    };
    let encode = |i: usize, steps: u32| -> Result<Option<Encoded>> {
        let Some(role) = role_of(i) else { return Ok(None) };
        let t = &textures[blocks[i].1[0]];
        plan::optimize(&t.meta(), original(i), role, &limits.texture_policy(steps))
            .map_err(|e| anyhow::anyhow!(tf!("texture.budget.texture_error", name = t.entry.name, error = format!("{e:#}"))))
    };
    let fix_mips = limits.policy.fix_mips;

    // Stage 1: corrections only.
    let mut encoded: Vec<Option<Encoded>> = (0..blocks.len()).into_par_iter().map(|i| encode(i, 0)).collect::<Result<_>>()?;
    let mut steps = vec![0u32; blocks.len()];
    let mut losses = vec![0f64; blocks.len()];
    let mut forced = vec![false; blocks.len()];
    let size_of = |enc: &Option<Encoded>, i: usize| enc.as_ref().map(|e| e.data.len()).unwrap_or(original(i).len()) as u64;
    let mut sizes: Vec<u64> = (0..blocks.len()).map(|i| size_of(&encoded[i], i)).collect();
    let mut physical = gfx::packed_size(res, &sizes)?;

    // Stage 2: targeted reduction.
    if limits.enabled && physical > limits.budget {
        let mut blocked = Blocked::default();
        let mut candidates: Vec<(Candidate, Bounds)> = Vec::new();
        for i in 0..blocks.len() {
            let t = &textures[blocks[i].1[0]];
            if role_of(i).is_none() {
                blocked.hold(super::HOLD_PROTECTED, sizes[i]);
                continue;
            }
            let rule = t.rule;
            match rule.access {
                Access::Never(h) => {
                    blocked.hold(h, sizes[i]);
                    continue;
                }
                Access::ForceOnly(h) if !limits.force => {
                    blocked.hold(h, sizes[i]);
                    continue;
                }
                _ => {}
            }
            let floor = if limits.force { limits.force_min_size.min(rule.min_size) } else { rule.min_size };
            let bounds = Bounds {
                floor,
                hard: floor.max(rule.hard_floor),
                step_cap: if limits.force { limits.force_max_steps.max(rule.max_steps) } else { rule.max_steps },
            };
            let (format, width, height, levels) = match &encoded[i] {
                Some(e) => (e.format, e.width, e.height, e.levels),
                None => (t.entry.format, t.entry.width as u32, t.entry.height as u32, t.entry.levels as u32),
            };
            if fmt::layout(format).is_none() || width.min(height) / 2 < bounds.floor {
                blocked.min_size += sizes[i];
                continue;
            }
            if width.min(height) / 2 < bounds.hard {
                blocked.hold(rule.hard_floor_hold.unwrap_or(super::HOLD_PROTECTED), sizes[i]);
                continue;
            }
            let c = Candidate { block: i, format, width, height, levels, loss: vec![0.0], key: vec![rule.tier] };
            candidates.push((c, bounds));
        }
        // Loss of each admissible step, measured in parallel (the costly part).
        candidates.par_iter_mut().for_each(|(c, b)| {
            let t = &textures[blocks[c.block].1[0]];
            let rule = t.rule;
            let force_only = matches!(rule.access, Access::ForceOnly(_));
            let role = role_of(c.block).unwrap_or(TextureRole::Other);
            let Some(top) = reference(t, original(c.block), c.width, c.height, role) else { return };
            let alpha = top.alpha_used();
            for k in 1..=b.step_cap {
                let side = c.width.min(c.height) >> k;
                if side < b.hard {
                    break;
                }
                let l = quality::reduction_loss(&top, k, role, alpha);
                let past = force_only || k > rule.max_steps || side < rule.min_size || (!rule.ignore_loss && l > limits.max_loss);
                if past && !limits.force {
                    break;
                }
                let key = c.key[k as usize - 1].max(rule.tier + if past { PAST } else { 0 });
                c.loss.push(l);
                c.key.push(key);
            }
        });

        // Greedy choice; the estimated sizes are checked against real encodings after
        // each round, and the loop goes on while the file is still above the budget.
        for _round in 0..6 {
            loop {
                if physical <= limits.budget {
                    break;
                }
                let best = candidates
                    .iter()
                    .enumerate()
                    .filter_map(|(ci, (c, _))| {
                        let k = steps[c.block] + 1;
                        let next = *c.loss.get(k as usize)?;
                        let gain = sizes[c.block].saturating_sub(c.size(k, fix_mips)) as f64;
                        let score = gain / (next - c.loss[k as usize - 1] + 0.05);
                        (gain > 0.0).then(|| (ci, c.key[k as usize], score))
                    })
                    .max_by(|a, b| b.1.cmp(&a.1).then(a.2.total_cmp(&b.2)));
                let Some((ci, _, _)) = best else { break };
                let c = &candidates[ci].0;
                steps[c.block] += 1;
                losses[c.block] = c.loss[steps[c.block] as usize];
                forced[c.block] = c.key[steps[c.block] as usize] >= PAST;
                sizes[c.block] = c.size(steps[c.block], fix_mips);
                physical = gfx::packed_size(res, &sizes)?;
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
            physical = gfx::packed_size(res, &sizes)?;
            if physical <= limits.budget {
                break;
            }
        }

        if physical > limits.budget {
            for (c, b) in &candidates {
                let k = steps[c.block];
                if (k as usize) + 1 < c.loss.len() {
                    continue;
                }
                let next_side = c.width.min(c.height) >> (k + 1);
                if k >= b.step_cap {
                    blocked.max_steps += sizes[c.block];
                } else if next_side < b.floor {
                    blocked.min_size += sizes[c.block];
                } else if next_side < b.hard {
                    let rule = textures[blocks[c.block].1[0]].rule;
                    blocked.hold(rule.hard_floor_hold.unwrap_or(super::HOLD_PROTECTED), sizes[c.block]);
                } else {
                    blocked.loss += sizes[c.block];
                }
            }
            let mut why = Vec::new();
            for (bytes, label) in
                [(blocked.loss, "texture.budget.blocked_loss"), (blocked.min_size, "texture.budget.blocked_min_size"), (blocked.max_steps, "texture.budget.blocked_max_steps")]
            {
                if bytes > 0 {
                    why.push(tf!("texture.budget.reason", label = t(label), size = mib(bytes)));
                }
            }
            blocked.holds.sort_by_key(|(h, _)| h.rank);
            for (h, bytes) in &blocked.holds {
                if *bytes > 0 {
                    why.push(tf!("texture.budget.reason", label = t(h.label), size = mib(*bytes)));
                }
            }
            warnings.push(tf!(
                "texture.budget.over_budget",
                size = mib(physical),
                budget = mib(limits.budget),
                reasons = why.join(", ")
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
fn candidate_width(candidates: &[(Candidate, Bounds)], i: usize, steps: u32) -> u32 {
    candidates.iter().find(|(c, _)| c.block == i).map(|(c, _)| c.width >> steps).unwrap_or(0)
}
