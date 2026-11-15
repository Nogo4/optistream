//! What each texture of a vehicle is used for, and how it may be reduced.
//!
//! Roles come from the shaders of the readable `.yft` / `.ydr` / `.ydd` of the resource
//! (vehicle samplers added to the mapping table), else from the name. Vehicle packs are
//! often encrypted: the name is then the only hint.

use std::collections::HashSet;
use std::sync::OnceLock;

use optistream_mapping::roles::{is_emissive_shader, sampler_usage, RoleIndex, Usage};
use optistream_rage::drawable::ShaderInfo;
use optistream_rage::joaat::joaat;

/// Sampler of the livery sheet in the vehicle paint shaders.
pub fn livery_sampler() -> u32 {
    static H: OnceLock<u32> = OnceLock::new();
    *H.get_or_init(|| joaat("DiffuseSampler2"))
}

fn vehicle_protected_samplers() -> &'static HashSet<u32> {
    static SET: OnceLock<HashSet<u32>> = OnceLock::new();
    // License plate font atlas: glyphs read by coordinate.
    SET.get_or_init(|| ["FontSampler", "FontNormalSampler"].iter().map(|n| joaat(n)).collect())
}

/// Vehicle shaders whose textures glow (lights, dashboards): never reduced.
const EMISSIVE_SHADERS: &[&str] = &[
    "vehicle_lightsemissive", "vehicle_lightsemissive_siren", "vehicle_emissive_opaque", "vehicle_emissive_alpha",
    "vehicle_dash_emissive", "vehicle_dash_emissive_opaque", "vehicle_track_emissive", "vehicle_track2_emissive",
];

fn emissive_hashes() -> &'static HashSet<u32> {
    static SET: OnceLock<HashSet<u32>> = OnceLock::new();
    SET.get_or_init(|| EMISSIVE_SHADERS.iter().flat_map(|n| [joaat(n), joaat(&format!("{n}.sps"))]).collect())
}

fn is_emissive(s: &ShaderInfo) -> bool {
    let set = emissive_hashes();
    is_emissive_shader(s) || set.contains(&s.name_hash) || set.contains(&s.file_hash)
}

pub fn sampler_usage_vehicle(s: &ShaderInfo, param: u32) -> Usage {
    if is_emissive(s) || vehicle_protected_samplers().contains(&param) {
        Usage::Protected
    } else {
        sampler_usage(param)
    }
}

/// Roles and liveries of one resource.
#[derive(Clone, Debug, Default)]
pub struct VehicleIndex {
    pub roles: RoleIndex,
    /// Lowercased names sampled as a livery.
    pub liveries: HashSet<String>,
}

impl VehicleIndex {
    pub fn add_shaders<'a>(&mut self, shaders: impl IntoIterator<Item = &'a ShaderInfo> + Clone) {
        self.roles.add_shaders_with(shaders.clone(), sampler_usage_vehicle);
        for s in shaders {
            for (param, name) in &s.textures {
                if *param == livery_sampler() && !name.is_empty() {
                    self.liveries.insert(name.to_ascii_lowercase());
                }
            }
        }
    }

    pub fn merge(&mut self, other: &VehicleIndex) {
        self.roles.merge(&other.roles);
        self.liveries.extend(other.liveries.iter().cloned());
    }
}

/// Whether a texture name is a livery sheet (`*_sign_N`, `*livery*`, `*_liv_*`).
pub fn livery_by_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    has_livery_word(&n) || n.contains("_sign_") || n.ends_with("_sign") || words(&n).any(|w| w == "liv")
}

/// Whether a `.yft` file is a livery mod part (its embedded textures are liveries).
pub fn livery_file(file_name_lower: &str) -> bool {
    let stem = file_name_lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name_lower);
    has_livery_word(stem) || words(stem).any(|w| w == "liv")
}

/// `livery` anywhere in the name, except inside `delivery`.
fn has_livery_word(n: &str) -> bool {
    n.match_indices("livery").any(|(i, _)| !n[..i].ends_with("de"))
}

/// Lights, sirens and glass (by name): reduced last, only when forcing.
pub fn light_or_glass_by_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["light", "glass", "siren", "lamp", "emis"].iter().any(|w| n.contains(w))
}

/// Role from the name: the mapping rules, plus names written with spaces by vehicle
/// modders (`SEAT NM`, `body n`) whose last word marks a normal map.
pub fn usage_from_name(name: &str) -> Usage {
    let n = name.to_ascii_lowercase();
    if let Some(last) = n.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).last() {
        if n.contains(' ') && matches!(last, "n" | "nm" | "nrm" | "norm" | "normal") {
            return Usage::Role(optistream_texture::TextureRole::Normal);
        }
    }
    optistream_mapping::roles::usage_from_name(name)
}

/// Words of a lowercased name, trailing digits removed (`onx_liv_lsfd1` → `onx`, `liv`, `lsfd`).
fn words(n: &str) -> impl Iterator<Item = &str> {
    n.split(|c: char| !c.is_ascii_alphanumeric()).map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn livery_names() {
        for n in ["tfpdstanier_sign_1", "scout_sign_5", "reflectivelivery", "onx_ambjog_liv_lsfd1", "car_livery2"] {
            assert!(livery_by_name(n), "{n}");
        }
        for n in ["tfstanier_lights", "hustler_belt_d", "signal_lamp", "olive_paint", "delivery_van_d"] {
            assert!(!livery_by_name(n), "{n}");
        }
    }

    #[test]
    fn livery_files() {
        assert!(livery_file("onx_ambjog_liv_lsfd1.yft"));
        assert!(livery_file("carter_livery3.yft"));
        assert!(!livery_file("carter_hi.yft"));
        assert!(!livery_file("olivier.yft"));
    }

    #[test]
    fn spaced_normal_names() {
        use optistream_texture::TextureRole;
        assert_eq!(usage_from_name("SEAT NM"), Usage::Role(TextureRole::Normal));
        assert_eq!(usage_from_name("chrome norm"), Usage::Role(TextureRole::Normal));
        assert_eq!(usage_from_name("SEAT D"), Usage::Role(TextureRole::Diffuse));
        assert_eq!(usage_from_name("BM PLATE NM"), Usage::Role(TextureRole::Normal));
    }

    #[test]
    fn lights_and_glass() {
        for n in ["tfstanier_lights", "generic_lightbar_rbwa", "vehicle_generic_glasswindows_custom", "siren_d"] {
            assert!(light_or_glass_by_name(n), "{n}");
        }
        assert!(!light_or_glass_by_name("hustler_belt_d"));
    }

    fn shader(name: &str, textures: &[(&str, &str)]) -> ShaderInfo {
        ShaderInfo { name_hash: joaat(name), file_hash: 0, textures: textures.iter().map(|(s, t)| (joaat(s), (*t).into())).collect() }
    }

    #[test]
    fn emissive_shaders_and_plate_fonts_are_protected() {
        use optistream_texture::TextureRole;
        let lights = shader("vehicle_lightsemissive", &[]);
        assert_eq!(sampler_usage_vehicle(&lights, joaat("DiffuseSampler")), Usage::Protected);
        let by_file = ShaderInfo { name_hash: 0, file_hash: joaat("vehicle_dash_emissive.sps"), textures: Vec::new() };
        assert_eq!(sampler_usage_vehicle(&by_file, joaat("DiffuseSampler")), Usage::Protected);
        let paint = shader("vehicle_paint1", &[]);
        assert_eq!(sampler_usage_vehicle(&paint, joaat("FontSampler")), Usage::Protected);
        assert_eq!(sampler_usage_vehicle(&paint, joaat("BumpSampler")), Usage::Role(TextureRole::Normal));
        assert_eq!(sampler_usage_vehicle(&paint, joaat("DirtSampler")), Usage::Role(TextureRole::Other));
    }

    #[test]
    fn index_collects_liveries_and_merges() {
        let mut a = VehicleIndex::default();
        a.add_shaders(&[shader("vehicle_paint3", &[("DiffuseSampler2", "Carter_Sign_1"), ("DiffuseSampler", "carter_body"), ("DiffuseSampler2", "")])]);
        assert_eq!(a.liveries, ["carter_sign_1".to_owned()].into_iter().collect());
        let mut b = VehicleIndex::default();
        b.add_shaders(&[shader("vehicle_lightsemissive", &[("DiffuseSampler", "carter_body"), ("DiffuseSampler2", "carter_sign_2")])]);
        a.merge(&b);
        assert_eq!(a.liveries.len(), 2);
        // The protected usage of the emissive shader wins over the plain diffuse one.
        assert_eq!(a.roles.get("CARTER_BODY"), Some(Usage::Protected));
    }

    #[test]
    fn known_vehicle_samplers() {
        // Hashes observed in carter.yft and onx_ambjog_liv_lsfd1.yft.
        assert_eq!(livery_sampler(), 181_641_832);
        assert_eq!(joaat("DirtSampler"), 2_124_031_998);
        assert!(vehicle_protected_samplers().contains(&2_048_139_100));
    }
}
