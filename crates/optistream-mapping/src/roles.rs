//! What each texture of a map is used for, from the shaders that sample it.
//!
//! A map `.ytd` holds the textures of the drawables (`.ydr`, `.ydd`) of its resource: the
//! textures have no role of their own, the samplers that reference them by name give it.
//! The shaders of every readable drawable of the resource are indexed once
//! ([`RoleIndex`]); encrypted drawables tell nothing, the texture name is then the only hint.

use std::collections::HashMap;
use std::sync::OnceLock;

use optistream_rage::drawable::ShaderInfo;
use optistream_rage::joaat::joaat;
use optistream_texture::TextureRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Role(TextureRole),
    /// Never modified: emissive, render targets, palettes, lookup tables, flow maps.
    Protected,
}

impl Usage {
    pub fn label(self) -> &'static str {
        match self {
            Usage::Role(r) => r.label(),
            Usage::Protected => "protégée",
        }
    }
}

fn sampler_table() -> &'static HashMap<u32, Usage> {
    static MAP: OnceLock<HashMap<u32, Usage>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        let mut add = |names: &[&str], u: Usage| {
            for n in names {
                m.insert(joaat(n), u);
            }
        };
        add(
            &[
                "DiffuseSampler", "DiffuseSampler2", "DiffuseSampler3", "DiffuseExtraSampler", "DiffuseTexSampler",
                "TextureSampler_layer0", "TextureSampler_layer1", "TextureSampler_layer2", "TextureSampler_layer3",
            ],
            Usage::Role(TextureRole::Diffuse),
        );
        add(
            &["BumpSampler", "BumpSampler2", "BumpSampler_layer0", "BumpSampler_layer1", "BumpSampler_layer2", "BumpSampler_layer3", "NormalMapSampler"],
            Usage::Role(TextureRole::Normal),
        );
        add(&["SpecSampler", "SpecularSampler"], Usage::Role(TextureRole::Specular));
        add(
            &[
                "lookupSampler", "TintPaletteSampler", "TextureSamplerDiffPal", "distanceMapSampler", "FlowSampler",
                "FogSampler", "noiseSampler", "PlateBgSampler", "EnvironmentSampler",
            ],
            Usage::Protected,
        );
        m
    })
}

/// Role given by a sampler; unknown samplers (detail, height, dirt, ...) give `Other`.
pub fn sampler_usage(hash: u32) -> Usage {
    sampler_table().get(&hash).copied().unwrap_or(Usage::Role(TextureRole::Other))
}

/// Emissive shaders of the map, vehicle and decal families: their textures glow, a
/// reduction there shows at once (signs, screens, neons).
const EMISSIVE_SHADERS: &[&str] = &[
    "emissive", "emissive_alpha", "emissive_alpha_tnt", "emissive_additive_alpha", "emissive_additive_uv_alpha",
    "emissive_clip", "emissive_speclum", "emissive_tnt", "emissivenight", "emissivenight_alpha",
    "emissivenight_geomnightonly", "emissivestrong", "emissivestrong_alpha", "decal_emissive_only",
    "decal_emissivenight_only", "glass_emissive", "glass_emissive_alpha", "glass_emissivenight",
    "glass_emissivenight_alpha", "ped_emissive", "vehicle_emissive_opaque", "vehicle_emissive_alpha",
];

fn emissive_hashes() -> &'static std::collections::HashSet<u32> {
    static SET: OnceLock<std::collections::HashSet<u32>> = OnceLock::new();
    SET.get_or_init(|| {
        EMISSIVE_SHADERS.iter().flat_map(|n| [joaat(n), joaat(&format!("{n}.sps"))]).collect()
    })
}

pub fn is_emissive_shader(s: &ShaderInfo) -> bool {
    let set = emissive_hashes();
    set.contains(&s.name_hash) || set.contains(&s.file_hash)
}

fn rank(u: Usage) -> u8 {
    match u {
        Usage::Protected => 4,
        Usage::Role(TextureRole::Normal) => 3,
        Usage::Role(TextureRole::Specular) => 2,
        Usage::Role(TextureRole::Diffuse) => 1,
        Usage::Role(TextureRole::Other) => 0,
    }
}

/// Usages of texture names (lowercased): when several samplers reference one name, the
/// most demanding one wins (protected > normal > specular > diffuse > other).
#[derive(Clone, Debug, Default)]
pub struct RoleIndex {
    map: HashMap<String, Usage>,
}

impl RoleIndex {
    pub fn add_shaders<'a>(&mut self, shaders: impl IntoIterator<Item = &'a ShaderInfo>) {
        for s in shaders {
            let emissive = is_emissive_shader(s);
            for (param, name) in &s.textures {
                if name.is_empty() {
                    continue;
                }
                let usage = if emissive { Usage::Protected } else { sampler_usage(*param) };
                self.insert(name.to_ascii_lowercase(), usage);
            }
        }
    }

    fn insert(&mut self, key: String, usage: Usage) {
        if !self.map.get(&key).is_some_and(|&old| rank(old) >= rank(usage)) {
            self.map.insert(key, usage);
        }
    }

    pub fn merge(&mut self, other: &RoleIndex) {
        for (k, &u) in &other.map {
            self.insert(k.clone(), u);
        }
    }

    pub fn get(&self, name: &str) -> Option<Usage> {
        self.map.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Role guessed from a texture name, when no readable shader references it.
pub fn usage_from_name(name: &str) -> Usage {
    let n = name.to_ascii_lowercase();
    let protected_word = words(&n).any(|w| matches!(w, "pal" | "palette" | "lut" | "lookup" | "tint"));
    if n.contains("script_rt") || n.contains("palette") || n.contains("lookup") || protected_word {
        return Usage::Protected;
    }
    if n.ends_with("_n") || n.ends_with("_nm") || n.contains("normal") || n.contains("bump") || n.contains("_nrm") {
        return Usage::Role(TextureRole::Normal);
    }
    if n.ends_with("_s") || n.contains("spec") {
        return Usage::Role(TextureRole::Specular);
    }
    Usage::Role(TextureRole::Diffuse)
}

/// Words of a texture name that mark text or graphics read up close: such a texture keeps
/// its size in the targeted reduction (format and mip fixes still apply).
const READABLE_WORDS: &[&str] = &[
    "logo", "logos", "sign", "signs", "signage", "text", "texts", "txt", "poster", "posters", "decal", "decals",
    "label", "labels", "menu", "menus", "font", "fonts", "neon", "neons", "screen", "screens", "tv", "banner",
    "banners", "sticker", "stickers", "flag", "flags", "graffiti", "graffitis", "letter", "letters", "billboard",
];

/// Why a texture keeps its size in the targeted reduction because of its name, if it does.
pub fn readable_by_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("logo") || n.contains("poster") || n.contains("billboard") || n.contains("graffiti") {
        return true;
    }
    let readable = words(&n).any(|w| READABLE_WORDS.contains(&w));
    readable
}

/// Words of a lowercased name, trailing digits removed (`t_mrpd_pal1` → `t`, `mrpd`, `pal`).
fn words(n: &str) -> impl Iterator<Item = &str> {
    n.split(|c: char| !c.is_ascii_alphanumeric()).map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit()))
}

/// Uncompressed strips a few texels high or wide are palettes or lookup tables read by
/// coordinate: any resampling or compression changes what the shader reads.
pub fn is_lookup_shaped(width: u16, height: u16, format: u32) -> bool {
    use optistream_rage::texture::format as tf;
    let uncompressed = matches!(format, tf::A8R8G8B8 | tf::X8R8G8B8 | tf::A8B8G8R8 | tf::A1R5G5B5 | tf::A16B16G16R16 | tf::A8 | tf::L8);
    uncompressed && width.min(height) <= 16 && width != height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names() {
        for n in ["gabz_logo_01", "burgershotlogo", "prompt_sign2", "menu_board", "wall_poster_03", "bar_neon", "tv_screen"] {
            assert!(readable_by_name(n), "{n}");
        }
        for n in ["design_wall", "signal_box", "concrete_floor", "wood_planks_n", "textile_sofa"] {
            assert!(!readable_by_name(n), "{n}");
        }
    }

    #[test]
    fn palettes_by_name() {
        for n in ["T_mrpd_pal1", "car_palette", "wall_tint", "grade_lut", "x_lookup"] {
            assert_eq!(usage_from_name(n), Usage::Protected, "{n}");
        }
        assert_eq!(usage_from_name("pale_wall"), Usage::Role(TextureRole::Diffuse));
    }

    #[test]
    fn strongest_usage_wins() {
        let mut idx = RoleIndex::default();
        idx.insert("a".into(), Usage::Role(TextureRole::Diffuse));
        idx.insert("a".into(), Usage::Role(TextureRole::Normal));
        idx.insert("a".into(), Usage::Role(TextureRole::Other));
        assert_eq!(idx.get("A"), Some(Usage::Role(TextureRole::Normal)));
    }

    #[test]
    fn known_sampler_hashes() {
        // Same hashes as observed in the clothing files (DiffuseSampler / BumpSampler / SpecSampler).
        assert_eq!(sampler_usage(4_059_966_321), Usage::Role(TextureRole::Diffuse));
        assert_eq!(sampler_usage(1_186_448_975), Usage::Role(TextureRole::Normal));
        assert_eq!(sampler_usage(1_619_499_462), Usage::Role(TextureRole::Specular));
    }
}
