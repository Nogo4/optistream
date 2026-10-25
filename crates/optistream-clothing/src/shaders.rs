//! Shader and sampler knowledge used to classify textures and protect special cases.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use optistream_rage::drawable::ShaderInfo;
use optistream_rage::joaat::joaat;
use optistream_texture::TextureRole;

const PED_SHADERS: &[&str] = &[
    "ped", "ped_alpha", "ped_cloth", "ped_cloth_enveff", "ped_decal", "ped_decal_decoration",
    "ped_decal_exp", "ped_decal_expensive", "ped_decal_nodiff", "ped_default", "ped_default_cloth",
    "ped_default_cutout", "ped_default_enveff", "ped_default_mp", "ped_default_palette", "ped_emissive",
    "ped_enveff", "ped_fur", "ped_hair_cutout_alpha", "ped_hair_cutout_alpha_cloth", "ped_hair_spiked",
    "ped_nopeddamagedecals", "ped_palette", "ped_wrinkle", "ped_wrinkle_cloth", "ped_wrinkle_cloth_enveff",
    "ped_wrinkle_cs", "ped_wrinkle_enveff", "default",
];

/// Shader hash (name or `.sps` file name) to shader name.
fn shader_names() -> &'static HashMap<u32, &'static str> {
    static MAP: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for &n in PED_SHADERS {
            m.insert(joaat(n), n);
            m.insert(joaat(&format!("{n}.sps")), n);
        }
        m
    })
}

pub fn shader_name(s: &ShaderInfo) -> Option<&'static str> {
    let names = shader_names();
    names.get(&s.name_hash).or_else(|| names.get(&s.file_hash)).copied()
}

pub fn is_cloth_shader(s: &ShaderInfo) -> bool {
    shader_name(s).is_some_and(|n| n.contains("cloth"))
}

pub fn is_emissive_shader(s: &ShaderInfo) -> bool {
    shader_name(s).is_some_and(|n| n.contains("emissive"))
}

/// Whether the shader reads the diffuse alpha as coverage (cutout, blend, decal, hair, fur).
/// Unknown shaders are treated as opaque: their alpha may be anything, even where visible.
pub fn uses_diffuse_alpha(s: &ShaderInfo) -> bool {
    shader_name(s).is_some_and(|n| {
        n.contains("alpha") || n.contains("cutout") || n.contains("decal") || n.contains("hair_spiked") || n.contains("fur")
    })
}

fn diffuse_textures(s: &ShaderInfo) -> impl Iterator<Item = &str> {
    s.textures
        .iter()
        .filter(|(param, name)| !name.is_empty() && sampler_usage(*param) == Some(Usage::Role(TextureRole::Diffuse)))
        .map(|(_, name)| name.as_str())
}

/// Diffuse textures (lowercased names) drawn only by shaders that use their alpha.
pub fn alpha_masked_diffuses(shaders: &[ShaderInfo]) -> HashSet<String> {
    let mut masked = HashSet::new();
    let mut opaque = HashSet::new();
    for s in shaders {
        let set = if uses_diffuse_alpha(s) { &mut masked } else { &mut opaque };
        set.extend(diffuse_textures(s).map(str::to_ascii_lowercase));
    }
    masked.retain(|n| !opaque.contains(n));
    masked
}

/// Whether every shader that samples a diffuse uses its alpha (and at least one does). A
/// variation `.ytd` replaces the diffuses of its drawable, whatever their names.
pub fn all_diffuse_shaders_use_alpha<'a>(shaders: impl IntoIterator<Item = &'a ShaderInfo>) -> bool {
    let mut any = false;
    for s in shaders.into_iter().filter(|s| diffuse_textures(s).next().is_some()) {
        if !uses_diffuse_alpha(s) {
            return false;
        }
        any = true;
    }
    any
}

/// Whether the draw order of the triangles matters (alpha blending: hair layers, stacked
/// decals). Unknown shaders are treated as order sensitive.
pub fn is_order_sensitive(s: Option<&ShaderInfo>) -> bool {
    let Some(name) = s.and_then(shader_name) else {
        return true;
    };
    if name == "ped_nopeddamagedecals" {
        return false;
    }
    name.contains("decal")
        || name.contains("hair_spiked")
        || name.contains("fur")
        || name.contains("emissive")
        || (name.contains("alpha") && !name.contains("cutout"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Role(TextureRole),
    /// Must never be modified (palettes, render targets, emissive, unknown special samplers).
    Protected,
}

pub fn sampler_usage(hash: u32) -> Option<Usage> {
    static MAP: OnceLock<HashMap<u32, Usage>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for n in ["DiffuseSampler", "DiffuseSampler2"] {
            m.insert(joaat(n), Usage::Role(TextureRole::Diffuse));
        }
        m.insert(joaat("BumpSampler"), Usage::Role(TextureRole::Normal));
        m.insert(joaat("SpecSampler"), Usage::Role(TextureRole::Specular));
        for n in [
            "TextureSamplerDiffPal", "TintPaletteSampler", "WrinkleSampler_A", "WrinkleSampler_B",
            "WrinkleMaskSampler_0", "WrinkleMaskSampler_1", "WrinkleMaskSampler_2", "WrinkleMaskSampler_3",
        ] {
            m.insert(joaat(n), Usage::Protected);
        }
        m
    })
    .get(&hash)
    .copied()
}

/// Role guessed from a texture name (used for YTDs and unreferenced embedded textures).
pub fn usage_from_name(name: &str) -> Usage {
    let n = name.to_ascii_lowercase();
    if n.contains("script_rt") || n.contains("palette") || n.ends_with("_pal") {
        return Usage::Protected;
    }
    if n.ends_with("_n") || n.contains("normal") || n.contains("bump") || n.contains("_nrm") {
        return Usage::Role(TextureRole::Normal);
    }
    if n.ends_with("_s") || n.contains("spec") {
        return Usage::Role(TextureRole::Specular);
    }
    Usage::Role(TextureRole::Diffuse)
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

/// Usage of every texture name referenced by the shaders (lowercased names).
pub fn usages_from_shaders(shaders: &[ShaderInfo]) -> HashMap<String, Usage> {
    let mut out: HashMap<String, Usage> = HashMap::new();
    for s in shaders {
        let emissive = is_emissive_shader(s);
        for (param, name) in &s.textures {
            if name.is_empty() {
                continue;
            }
            let usage = if emissive {
                Usage::Protected
            } else {
                sampler_usage(*param).unwrap_or(Usage::Role(TextureRole::Other))
            };
            let key = name.to_ascii_lowercase();
            let keep = out.get(&key).is_some_and(|&old| rank(old) >= rank(usage));
            if !keep {
                out.insert(key, usage);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ped_default_hash_is_known() {
        // Hash read from a real oren_eupp ydd shader.
        let s = ShaderInfo { name_hash: 0x34D9_0761, file_hash: 0x203B_2307, textures: vec![] };
        assert!(shader_name(&s).is_some());
    }

    #[test]
    fn order_sensitive_shaders() {
        let named = |n: &str| ShaderInfo { name_hash: joaat(n), file_hash: 0, textures: vec![] };
        for n in ["ped_alpha", "ped_decal", "ped_decal_decoration", "ped_hair_spiked", "ped_fur", "ped_emissive"] {
            assert!(is_order_sensitive(Some(&named(n))), "{n}");
        }
        for n in ["ped", "ped_default", "ped_default_cutout", "ped_nopeddamagedecals", "ped_hair_cutout_alpha", "ped_wrinkle", "default"] {
            assert!(!is_order_sensitive(Some(&named(n))), "{n}");
        }
        assert!(is_order_sensitive(Some(&named("some_unknown_shader"))));
        assert!(is_order_sensitive(None));
    }

    #[test]
    fn sampler_hashes_from_real_file() {
        // DiffuseSampler / BumpSampler / SpecSampler hashes observed in task_031_u.ydd.
        assert_eq!(sampler_usage(4_059_966_321), Some(Usage::Role(TextureRole::Diffuse)));
        assert_eq!(sampler_usage(1_186_448_975), Some(Usage::Role(TextureRole::Normal)));
        assert_eq!(sampler_usage(1_619_499_462), Some(Usage::Role(TextureRole::Specular)));
        assert_eq!(sampler_usage(2_878_898_974), Some(Usage::Protected));
    }
}
