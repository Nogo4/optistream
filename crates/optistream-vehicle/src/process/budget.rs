//! Reduction plan of one vehicle file: the shared budget engine with the vehicle rules.

use anyhow::Result;
use optistream_mapping::roles::Usage;
use optistream_texture::budget::{self, Access, BlockPlan, BudgetTex, FilePlan, Hold, Rule, HOLD_PROTECTED};
use optistream_texture::resource as texres;
use optistream_texture::TextureRole;

use crate::config::{Config, LIVERY_FLOOR};
use crate::process::{Class, Parsed, Tex};
use crate::roles;

const HOLD_LIVERY: Hold = Hold { label: "livrées à 1024 px (plancher)", rank: 5 };
const HOLD_LIGHTS: Hold = Hold { label: "feux / gyrophares / vitres (nom)", rank: 10 };
const HOLD_SMALL: Hold = Hold { label: "petites textures conservées (préréglage)", rank: 15 };

pub(crate) fn rule(t: &Tex, cfg: &Config) -> Rule {
    let (access, tier) = match t.class {
        Class::HiCopy | Class::Livery | Class::Big => (Access::Normal, 0),
        Class::Small if cfg.small => (Access::Normal, 1),
        Class::Small => (Access::Never(HOLD_SMALL), 1),
        Class::Last if t.usage == Usage::Role(TextureRole::Other) && !roles::light_or_glass_by_name(&t.entry.name) => {
            (Access::ForceOnly(HOLD_PROTECTED), 2)
        }
        Class::Last => (Access::ForceOnly(HOLD_LIGHTS), 2),
    };
    let (hard_floor, hard_floor_hold) = if t.livery { (LIVERY_FLOOR, Some(HOLD_LIVERY)) } else { (0, None) };
    // A livery may always go down to its 1024 px floor: the user set that size as its quality bar.
    Rule { access, tier, min_size: cfg.min_size, max_steps: cfg.max_steps, hard_floor, hard_floor_hold, ignore_loss: t.livery }
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
            BudgetTex { entry: &t.entry, data_off: t.data_off, data_len: t.data_len, role, rule: rule(t, cfg) }
        })
        .collect();
    let raw = texres::raw_ranges(&p.res, &p.drawables);
    budget::plan_file(&p.res, &raw, &textures, &cfg.limits())
}

pub(super) fn describe(p: &Parsed, b: &BlockPlan) -> Option<String> {
    let enc = b.enc.as_ref()?;
    let t = &p.textures[b.users[0]];
    let mut s = format!("texture {} ({}) : {}", t.entry.name, t.label(), enc.changes.join(", "));
    if b.steps > 0 {
        s += &format!(" — réduction ciblée ÷{} (perte {:.1} %){}", 1u32 << b.steps, b.loss, if b.forced { ", forcée" } else { "" });
    }
    Some(s)
}
