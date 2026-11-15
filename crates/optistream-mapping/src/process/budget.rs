//! Reduction plan of one map file: the shared budget engine with the mapping rules.
//!
//! Every texture is a normal candidate, except texts / logos (by name) and textures of
//! unknown role (detail, height, dirt maps): those are only reduced when forcing, last.

use anyhow::Result;
use optistream_texture::budget::{self, Access, BudgetTex, Hold, Limits, Rule, HOLD_PROTECTED};
use optistream_texture::resource as texres;
use optistream_texture::TextureRole;

pub use optistream_texture::budget::{BlockPlan, FilePlan};

use crate::config::{Config, FORCE_MAX_STEPS, FORCE_MIN_SIZE};
use crate::process::Parsed;
use crate::roles::Usage;

const HOLD_READABLE: Hold = Hold { label: "textes / logos / enseignes (nom)", rank: 10 };

pub(crate) fn limits(cfg: &Config) -> Limits {
    Limits {
        enabled: cfg.budget_enabled,
        budget: cfg.budget,
        max_loss: cfg.max_loss,
        force: cfg.force,
        force_min_size: FORCE_MIN_SIZE,
        force_max_steps: FORCE_MAX_STEPS,
        policy: cfg.texture_policy(0),
    }
}

fn rule(readable: bool, role: Option<TextureRole>, cfg: &Config) -> Rule {
    let access = if readable {
        Access::ForceOnly(HOLD_READABLE)
    } else if role == Some(TextureRole::Other) {
        Access::ForceOnly(HOLD_PROTECTED)
    } else {
        Access::Normal
    };
    let tier = if access == Access::Normal { 0 } else { 2 };
    Rule { access, tier, min_size: cfg.min_size, max_steps: cfg.max_steps, hard_floor: 0, hard_floor_hold: None, ignore_loss: false }
}

/// Plans a file (see [`optistream_texture::budget`]).
pub fn plan_file(p: &Parsed, cfg: &Config) -> Result<FilePlan> {
    let textures: Vec<BudgetTex> = p
        .textures
        .iter()
        .map(|t| {
            let role = match t.usage {
                Usage::Role(r) => Some(r),
                Usage::Protected => None,
            };
            BudgetTex { entry: &t.entry, data_off: t.data_off, data_len: t.data_len, role, rule: rule(t.readable, role, cfg) }
        })
        .collect();
    let raw = texres::raw_ranges(&p.res, &p.drawables);
    budget::plan_file(&p.res, &raw, &textures, &limits(cfg))
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
