use optistream_mapping::roles::Usage;
use optistream_rage::texture::{format as tf, TextureEntry};
use optistream_texture::budget::Access;
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
