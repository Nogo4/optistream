use optistream_mapping::roles::Usage;
use optistream_rage::texture::{format as tf, TextureEntry};
use optistream_texture::budget::{Access, HOLD_PROTECTED};
use optistream_texture::TextureRole;

use super::budget::rule;
use super::*;
use crate::config::{presets, Config, LIVERY_FLOOR};

fn entry(name: &str, side: u16) -> TextureEntry {
    TextureEntry { struct_offset: 0, name: name.into(), width: side, height: side, depth: 1, stride: side, format: tf::DXT5, levels: 1, data_ptr: 0 }
}

fn tex(name: &str, side: u16, livery: bool, class: Class) -> Tex {
    Tex { entry: entry(name, side), usage: Usage::Role(TextureRole::Diffuse), livery, class, data_off: None, data_len: 0 }
}

fn cfg(preset: &str) -> Config {
    Config::from_settings(&presets().into_iter().find(|p| p.id == preset).unwrap().values)
}

#[test]
fn kinds_from_names() {
    assert_eq!(Kind::of("a.ytd"), Some(Kind::Ytd));
    assert_eq!(Kind::of("b.yft"), Some(Kind::Yft));
    assert_eq!(Kind::of("c.ydr"), Some(Kind::Ydr));
    assert_eq!(Kind::of("d.ymt"), None);
}

#[test]
fn liveries_never_under_1024() {
    let r = rule(&tex("x_sign_1", 2048, true, Class::Livery), &cfg("fort"));
    assert_eq!(r.hard_floor, LIVERY_FLOOR);
    assert_eq!(r.access, Access::Normal);
    assert_eq!(r.tier, 0);
}

#[test]
fn small_textures_only_in_fort() {
    let t = tex("hustler_belt_d", 512, false, Class::Small);
    assert!(matches!(rule(&t, &cfg("equilibre")).access, Access::Never(_)));
    assert_eq!(rule(&t, &cfg("fort")).access, Access::Normal);
    assert_eq!(rule(&t, &cfg("fort")).tier, 1);
}

#[test]
fn lights_only_when_forcing() {
    let r = rule(&tex("tfpdstorm_lights", 512, false, Class::Last), &cfg("fort"));
    assert!(matches!(r.access, Access::ForceOnly(_)));
    assert_eq!(r.tier, 2);
}

fn with_usage(mut t: Tex, usage: Usage) -> Tex {
    t.usage = usage;
    t
}

/// How much an access lets the engine reduce: never < only when forcing < normal.
fn openness(a: Access) -> u8 {
    match a {
        Access::Never(_) => 0,
        Access::ForceOnly(_) => 1,
        Access::Normal => 2,
    }
}

/// One texture of every class, livery or not.
fn every_class() -> Vec<Tex> {
    let mut out = Vec::new();
    for class in [Class::HiCopy, Class::Last, Class::Livery, Class::Big, Class::Small] {
        for livery in [false, true] {
            out.push(tex("body_d", 2048, livery, class));
        }
    }
    out.push(with_usage(tex("dirt_x", 512, false, Class::Last), Usage::Role(TextureRole::Other)));
    out.push(with_usage(tex("lights_glass", 512, false, Class::Last), Usage::Role(TextureRole::Other)));
    out
}

#[test]
fn big_classes_are_normal_candidates_of_the_first_tier() {
    for preset in ["leger", "equilibre", "fort"] {
        for class in [Class::HiCopy, Class::Big, Class::Livery] {
            let r = rule(&tex("body_d", 2048, false, class), &cfg(preset));
            assert_eq!((r.access, r.tier), (Access::Normal, 0), "{preset} {class:?}");
        }
    }
}

#[test]
fn livery_flag_sets_the_hard_floor_whatever_the_class() {
    for t in every_class() {
        for preset in ["equilibre", "fort"] {
            let r = rule(&t, &cfg(preset));
            if t.livery {
                assert_eq!(r.hard_floor, LIVERY_FLOOR, "{:?}", t.class);
                assert!(r.hard_floor_hold.is_some(), "{:?}", t.class);
                assert!(r.ignore_loss, "{:?}", t.class);
            } else {
                assert_eq!((r.hard_floor, r.hard_floor_hold, r.ignore_loss), (0, None, false), "{:?}", t.class);
            }
        }
    }
}

#[test]
fn unknown_role_textures_wait_for_forcing_as_protected() {
    let c = cfg("fort");
    let unknown = rule(&with_usage(tex("dirt_x", 512, false, Class::Last), Usage::Role(TextureRole::Other)), &c);
    assert_eq!(unknown.access, Access::ForceOnly(HOLD_PROTECTED));
    assert_eq!(unknown.tier, 2);
    // A light / glass name keeps its own reason, whatever its role.
    for usage in [Usage::Role(TextureRole::Other), Usage::Role(TextureRole::Diffuse)] {
        let r = rule(&with_usage(tex("tfstanier_lights", 512, false, Class::Last), usage), &c);
        assert!(matches!(r.access, Access::ForceOnly(h) if h != HOLD_PROTECTED), "{usage:?}");
        assert_eq!(r.tier, 2);
    }
}

#[test]
fn tiers_follow_the_class_priority() {
    let c = cfg("fort");
    let tier = |class| rule(&tex("x_d", 1024, false, class), &c).tier;
    assert!(tier(Class::Big) < tier(Class::Small));
    assert!(tier(Class::Small) < tier(Class::Last));
    assert_eq!(tier(Class::HiCopy), tier(Class::Big));
}

#[test]
fn rule_takes_its_limits_from_the_config() {
    use crate::config::{BUDGET_MAX_STEPS, BUDGET_MIN_SIZE};
    use optistream_core::{Settings, Value};
    let c = Config::from_settings(&Settings::default().with(BUDGET_MAX_STEPS, Value::Int(3)).with(BUDGET_MIN_SIZE, Value::Int(512)));
    for t in every_class() {
        let r = rule(&t, &c);
        assert_eq!((r.min_size, r.max_steps), (512, 3), "{:?}", t.class);
    }
}

#[test]
fn stronger_preset_never_reduces_less() {
    for t in every_class() {
        for (weak, strong) in [("leger", "equilibre"), ("equilibre", "fort")] {
            let (w, s) = (rule(&t, &cfg(weak)), rule(&t, &cfg(strong)));
            let what = format!("{weak} → {strong}, {:?} livery {}", t.class, t.livery);
            assert!(openness(s.access) >= openness(w.access), "{what}");
            assert!(s.max_steps >= w.max_steps, "{what}");
            assert!(s.min_size <= w.min_size, "{what}");
            // The livery floor is a hard bar: no preset lowers it.
            assert_eq!(s.hard_floor, w.hard_floor, "{what}");
        }
    }
}

fn parsed(textures: Vec<Tex>) -> Parsed {
    use optistream_rage::rsc7::PageFlags;
    let res = Resource { version: 13, system_flags: PageFlags(0), graphics_flags: PageFlags(0), system: Vec::new(), graphics: Vec::new() };
    Parsed { res, kind: Kind::Ytd, drawables: Vec::new(), textures }
}

fn block(enc: bool, steps: u32, forced: bool) -> optistream_texture::budget::BlockPlan {
    let enc = enc.then(|| optistream_texture::Encoded {
        width: 1024,
        height: 1024,
        format: tf::DXT1,
        levels: 9,
        stride: 512,
        data: Vec::new(),
        changes: vec!["DXT5 → DXT1".into(), "mipmaps complétés".into()],
    });
    optistream_texture::budget::BlockPlan { off: 0, users: vec![0], enc, steps, loss: 2.44, forced }
}

#[test]
fn describe_reports_changes_and_targeted_reduction() {
    use super::budget::describe;
    let p = parsed(vec![tex("carter_sign_1", 2048, true, Class::Livery)]);
    // Nothing re-encoded: nothing to say.
    assert_eq!(describe(&p, &block(false, 1, false)), None);
    let plain = describe(&p, &block(true, 0, false)).unwrap();
    assert!(plain.starts_with("texture carter_sign_1 ("), "{plain}");
    assert!(plain.contains("DXT5 → DXT1, mipmaps complétés"), "{plain}");
    assert!(!plain.contains("réduction ciblée"), "{plain}");
    let reduced = describe(&p, &block(true, 2, false)).unwrap();
    assert!(reduced.contains("÷4 (perte 2.4 %)") && !reduced.contains("forcée"), "{reduced}");
    let forced = describe(&p, &block(true, 3, true)).unwrap();
    assert!(forced.contains("÷8") && forced.ends_with(", forcée"), "{forced}");
}
