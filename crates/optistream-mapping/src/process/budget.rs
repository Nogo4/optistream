//! Reduction plan of one map file: the shared budget engine with the mapping rules.
//!
//! Every texture is a normal candidate, except texts / logos (by name) and textures of
//! unknown role (detail, height, dirt maps): those are only reduced when forcing, last.

use anyhow::Result;
use optistream_i18n::{t, tf};
use optistream_texture::budget::{self, Access, BudgetTex, Hold, Limits, Rule, HOLD_PROTECTED};
use optistream_texture::resource as texres;
use optistream_texture::TextureRole;

pub use optistream_texture::budget::{BlockPlan, FilePlan};

use crate::config::{Config, FORCE_MAX_STEPS, FORCE_MIN_SIZE};
use crate::process::Parsed;
use crate::roles::Usage;

/// Hold of texts / logos, labelled in the current language.
fn hold_readable() -> Hold {
    Hold { label: t("mapping.hold.readable"), rank: 10 }
}

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
        Access::ForceOnly(hold_readable())
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
    let tex = &p.textures[b.users[0]];
    let mut s = tf!("mapping.change.texture", name = tex.entry.name, role = tex.usage.label(), changes = enc.changes.join(", "));
    if b.steps > 0 {
        s += &format!(" — {}", reduction(b));
    }
    Some(s)
}

/// "targeted reduction ÷N (loss x %)", followed by ", forced" when the steps went past the limits.
pub(super) fn reduction(b: &BlockPlan) -> String {
    let forced = if b.forced { t("mapping.reduction.forced") } else { "" };
    tf!("mapping.reduction.text", factor = 1u32 << b.steps, loss = format!("{:.1}", b.loss)) + forced
}

#[cfg(test)]
mod tests {
    use optistream_rage::rsc7::PageFlags;
    use optistream_rage::texture::{format as tf, TextureEntry};
    use optistream_rage::Resource;
    use optistream_texture::Encoded;

    use super::*;
    use crate::config::presets;
    use crate::process::{Kind, Tex};

    const ROLES: [Option<TextureRole>; 5] =
        [Some(TextureRole::Diffuse), Some(TextureRole::Normal), Some(TextureRole::Specular), Some(TextureRole::Other), None];

    fn cfg(id: &str) -> Config {
        Config::from_settings(&presets().into_iter().find(|p| p.id == id).unwrap().values)
    }

    /// How much an access lets the engine reduce: never < only when forcing < normal.
    fn openness(a: Access) -> u8 {
        match a {
            Access::Never(_) => 0,
            Access::ForceOnly(_) => 1,
            Access::Normal => 2,
        }
    }

    #[test]
    fn readable_textures_wait_for_forcing_whatever_their_role() {
        for role in ROLES {
            let r = rule(true, role, &cfg("fort"));
            assert_eq!((r.access, r.tier), (Access::ForceOnly(hold_readable()), 2), "{role:?}");
        }
    }

    #[test]
    fn unknown_role_waits_for_forcing_as_protected() {
        let r = rule(false, Some(TextureRole::Other), &cfg("fort"));
        assert_eq!((r.access, r.tier), (Access::ForceOnly(HOLD_PROTECTED), 2));
    }

    #[test]
    fn known_roles_are_normal_candidates_of_the_first_tier() {
        // `None` (protected) is left to the engine, which never touches a texture without a role.
        for role in [Some(TextureRole::Diffuse), Some(TextureRole::Normal), Some(TextureRole::Specular), None] {
            let r = rule(false, role, &cfg("equilibre"));
            assert_eq!((r.access, r.tier), (Access::Normal, 0), "{role:?}");
        }
    }

    #[test]
    fn rules_have_no_hard_floor_and_take_the_config_limits() {
        for id in ["leger", "equilibre", "fort"] {
            let c = cfg(id);
            for readable in [false, true] {
                for role in ROLES {
                    let r = rule(readable, role, &c);
                    assert_eq!((r.min_size, r.max_steps), (c.min_size, c.max_steps), "{id}");
                    assert_eq!((r.hard_floor, r.hard_floor_hold, r.ignore_loss), (0, None, false), "{id}");
                    // Forced steps always go at least as far as the limits.
                    assert!(FORCE_MIN_SIZE <= r.min_size && FORCE_MAX_STEPS >= r.max_steps, "{id}");
                }
            }
        }
    }

    #[test]
    fn stronger_preset_never_reduces_less() {
        for readable in [false, true] {
            for role in ROLES {
                for (weak, strong) in [("leger", "equilibre"), ("equilibre", "fort")] {
                    let (w, s) = (rule(readable, role, &cfg(weak)), rule(readable, role, &cfg(strong)));
                    assert!(openness(s.access) >= openness(w.access), "{weak} → {strong}");
                    assert!(s.max_steps >= w.max_steps && s.min_size <= w.min_size, "{weak} → {strong}");
                }
            }
        }
        let (lw, ls) = (limits(&cfg("equilibre")), limits(&cfg("fort")));
        assert!(ls.max_loss >= lw.max_loss && ls.force >= lw.force);
    }

    #[test]
    fn limits_mirror_the_config_with_mapping_force_floors() {
        for p in presets() {
            let c = Config::from_settings(&p.values);
            let l = limits(&c);
            assert_eq!((l.enabled, l.budget, l.max_loss, l.force), (c.budget_enabled, c.budget, c.max_loss, c.force), "{}", p.id);
            assert_eq!((l.force_min_size, l.force_max_steps), (FORCE_MIN_SIZE, FORCE_MAX_STEPS));
            assert_eq!(l.policy.halvings, 0);
            assert_eq!(l.texture_policy(2).halvings, 2);
            assert_eq!(l.policy.convert_formats, c.convert_formats);
        }
        assert!(!limits(&cfg("leger")).enabled);
    }

    fn parsed() -> Parsed {
        let res = Resource { version: 13, system_flags: PageFlags(0), graphics_flags: PageFlags(0), system: Vec::new(), graphics: Vec::new() };
        let entry = TextureEntry { struct_offset: 0, name: "wood_floor".into(), width: 2048, height: 2048, depth: 1, stride: 2048, format: tf::DXT5, levels: 1, data_ptr: 0 };
        let tex = Tex { entry, usage: Usage::Role(TextureRole::Diffuse), readable: false, data_off: None, data_len: 0 };
        Parsed { res, kind: Kind::Ytd, drawables: Vec::new(), textures: vec![tex] }
    }

    fn block(enc: bool, steps: u32, forced: bool) -> BlockPlan {
        let enc = enc.then(|| Encoded { width: 1024, height: 1024, format: tf::DXT1, levels: 9, stride: 512, data: Vec::new(), changes: vec!["DXT5 → DXT1".into()] });
        BlockPlan { off: 0, users: vec![0], enc, steps, loss: 1.46, forced }
    }

    #[test]
    fn describe_reports_changes_and_targeted_reduction() {
        let p = parsed();
        assert_eq!(describe(&p, &block(false, 2, true)), None);
        let plain = describe(&p, &block(true, 0, false)).unwrap();
        assert!(plain.starts_with("texture wood_floor (") && plain.ends_with(": DXT5 → DXT1"), "{plain}");
        let reduced = describe(&p, &block(true, 1, false)).unwrap();
        assert!(reduced.contains("÷2 (loss 1.5 %)") && !reduced.contains("forced"), "{reduced}");
        let forced = describe(&p, &block(true, 4, true)).unwrap();
        assert!(forced.contains("÷16") && forced.ends_with(", forced"), "{forced}");
    }
}
