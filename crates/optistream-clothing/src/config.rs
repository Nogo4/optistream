//! Parameters, presets and their typed view.

use optistream_core::{ParamKind, ParamSpec, Preset, Settings, Value};
use optistream_texture::{TexturePolicy, TextureRole};

pub const OPT_ONLY_3D: &str = "opt.only_3d";
pub const OPT_MIN_PHYSICAL: &str = "opt.min_physical_mib";
pub const TEX_HALVE: &str = "tex.halve";
pub const TEX_CROP: &str = "tex.crop_uv";
pub const TEX_CONVERT: &str = "tex.convert_formats";
pub const TEX_FIX_MIPS: &str = "tex.fix_mips";
pub const TEX_FORCE_POT: &str = "tex.force_pot";
pub const MESH_ENABLED: &str = "mesh.enabled";
pub const MESH_REORDER: &str = "mesh.reorder";
pub const MESH_BUDGET_LARGE: &str = "mesh.budget_large";
pub const MESH_BUDGET_SMALL: &str = "mesh.budget_small";
pub const MESH_MAX_ERROR: &str = "mesh.max_error_pct";
pub const MESH_LOWER_LODS: &str = "mesh.lower_lods";

pub fn params() -> Vec<ParamSpec> {
    let flag = |key, label, group, help| ParamSpec { key, label, group, help, kind: ParamKind::Bool };
    vec![
        flag(TEX_CONVERT, "Convertir BC7 / DXT3 / non compressé", "Textures", "Normal maps → DXT5 ; autres → DXT1, ou DXT5 si l'alpha est réellement utilisé. Les DXT1 / DXT5 gardent leur format."),
        flag(TEX_FORCE_POT, "Forcer les puissances de 2", "Textures", "Arrondi vers le bas : 3000×3000 → 2048×2048, etc. (sauf diffuses de peau _r)."),
        flag(TEX_FIX_MIPS, "Compléter les mipmaps", "Textures", "Chaîne complète jusqu'à 4 px, comme le jeu de base."),
        flag(TEX_HALVE, "Diviser par 2", "Textures", "Toutes les textures des fichiers traités sont réduites de moitié (sauf diffuses de peau _r)."),
        flag(TEX_CROP, "Recadrer sur la zone utilisée", "Textures", "Les textures sont coupées à la partie de la feuille que le vêtement utilise vraiment (sans perte), et ses UV réécrites. S'applique au .ydd, à son _1.ydd et à toutes ses variations ensemble ; jamais aux pièces _r, cloth ou à textures répétées."),
        flag(OPT_ONLY_3D, "Fichiers 3D uniquement (.ydd)", "Général", "Optimise seulement les .ydd ; les .ytd sont laissés tels quels."),
        ParamSpec {
            key: OPT_MIN_PHYSICAL,
            label: "Mémoire physique minimale",
            group: "Général",
            help: "Seuls les fichiers dont la mémoire physique déclarée dépasse cette valeur sont optimisés ; 16 Mio est le seuil d'avertissement de FiveM. 0 : tous les fichiers.",
            kind: ParamKind::Int { min: 0, max: 128, step: 4, unit: "Mio" },
        },
        flag(MESH_ENABLED, "Réduire les polygones", "Maillages", "Décimation du LOD High au-dessus du budget (jamais sur le cloth)."),
        flag(MESH_REORDER, "Optimiser l'ordre des triangles (sans perte)", "Maillages", "Cache de sommets et overdraw, triangles dégénérés ou en double retirés. Sommets inchangés ; shaders à transparence exclus."),
        ParamSpec {
            key: MESH_BUDGET_LARGE,
            label: "Budget grosses pièces",
            group: "Maillages",
            help: "Triangles max du LOD High : hauts, bas, vestes, accessoires, sacs, cheveux, chaussures, masques.",
            kind: ParamKind::Int { min: 2_000, max: 200_000, step: 1_000, unit: "tris" },
        },
        ParamSpec {
            key: MESH_BUDGET_SMALL,
            label: "Budget petites pièces",
            group: "Maillages",
            help: "Triangles max du LOD High : chaînes, mains, decals, props (montres, chapeaux, lunettes…).",
            kind: ParamKind::Int { min: 500, max: 100_000, step: 500, unit: "tris" },
        },
        ParamSpec {
            key: MESH_MAX_ERROR,
            label: "Tolérance de déformation",
            group: "Maillages",
            help: "Écart de forme maximal, en % de la taille de la pièce. La réduction s'arrête avant, même si le budget n'est pas atteint.",
            kind: ParamKind::Float { min: 0.05, max: 5.0, step: 0.05, unit: "%" },
        },
        flag(MESH_LOWER_LODS, "Plafonner les LOD inférieurs", "Maillages", "Un LOD Med/Low plus lourd que le nouveau High est réduit à sa taille."),
    ]
}

fn preset(id: &'static str, label: &'static str, description: &'static str, budgets: [i64; 2], err: f64, halve: bool, crop: bool) -> Preset {
    let values = Settings::default()
        .with(OPT_ONLY_3D, Value::Bool(false))
        .with(OPT_MIN_PHYSICAL, Value::Int(16))
        .with(TEX_CONVERT, Value::Bool(true))
        .with(TEX_FIX_MIPS, Value::Bool(true))
        .with(TEX_FORCE_POT, Value::Bool(true))
        .with(TEX_HALVE, Value::Bool(halve))
        .with(TEX_CROP, Value::Bool(crop))
        .with(MESH_ENABLED, Value::Bool(true))
        .with(MESH_REORDER, Value::Bool(true))
        .with(MESH_BUDGET_LARGE, Value::Int(budgets[0]))
        .with(MESH_BUDGET_SMALL, Value::Int(budgets[1]))
        .with(MESH_MAX_ERROR, Value::Float(err))
        .with(MESH_LOWER_LODS, Value::Bool(true));
    Preset { id, label, description, values }
}

pub fn presets() -> Vec<Preset> {
    vec![
        preset("leger", "Léger", "Textures : conversion, puissance de 2, mipmaps. ≤ 80k tris.", [80_000, 20_000], 0.2, false, false),
        preset("equilibre", "Équilibré", "Textures : conversion, puissance de 2, mipmaps, recadrage sur la zone utilisée. ≤ 40k tris.", [40_000, 10_000], 0.5, false, true),
        preset("fort", "Fort", "Textures : comme Équilibré, puis divisées par 2. ≤ 20k tris.", [20_000, 5_000], 1.0, true, true),
    ]
}

/// Typed view of the settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub only_3d: bool,
    pub min_physical: u64,
    pub halve: bool,
    pub crop_uv: bool,
    pub convert_formats: bool,
    pub fix_mips: bool,
    pub force_pot: bool,
    pub mesh_enabled: bool,
    pub reorder: bool,
    pub budget_large: u64,
    pub budget_small: u64,
    pub max_error: f32,
    pub lower_lods: bool,
}

impl Config {
    pub fn from_settings(s: &Settings) -> Self {
        Config {
            only_3d: s.bool(OPT_ONLY_3D, false),
            min_physical: s.int(OPT_MIN_PHYSICAL, 16).max(0) as u64 * 1024 * 1024,
            halve: s.bool(TEX_HALVE, false),
            crop_uv: s.bool(TEX_CROP, true),
            convert_formats: s.bool(TEX_CONVERT, true),
            fix_mips: s.bool(TEX_FIX_MIPS, true),
            force_pot: s.bool(TEX_FORCE_POT, true),
            mesh_enabled: s.bool(MESH_ENABLED, true),
            reorder: s.bool(MESH_REORDER, true),
            budget_large: s.int(MESH_BUDGET_LARGE, 40_000).max(1) as u64,
            budget_small: s.int(MESH_BUDGET_SMALL, 10_000).max(1) as u64,
            max_error: (s.float(MESH_MAX_ERROR, 0.5) / 100.0) as f32,
            lower_lods: s.bool(MESH_LOWER_LODS, true),
        }
    }

    /// Policy of one texture. `skin` marks the diffuse variations of skin-dependent (`_r`)
    /// drawables, which must keep the dimensions of the original MP skin. `alpha_mask`: the
    /// texture is only drawn by shaders that use its alpha as coverage.
    pub fn texture_policy(&self, role: TextureRole, skin: bool, alpha_mask: bool) -> TexturePolicy {
        let keep_dims = Self::keeps_dims(role, skin);
        TexturePolicy {
            halve: self.halve && !keep_dims,
            halvings: 0,
            convert_formats: self.convert_formats,
            fix_mips: self.fix_mips,
            force_pot: self.force_pot && !keep_dims,
            alpha_mask: alpha_mask && !keep_dims,
        }
    }

    /// Whether the dimensions of this texture are protected (diffuse of a skin `_r` drawable).
    pub fn keeps_dims(role: TextureRole, skin: bool) -> bool {
        skin && matches!(role, TextureRole::Diffuse | TextureRole::Other)
    }

    /// Whether the declared physical memory of an RSC7 file (header bytes) does not exceed the threshold.
    pub fn below_min_physical(&self, bytes: &[u8]) -> bool {
        self.min_physical > 0
            && bytes.len() >= 16
            && optistream_rage::rsc7::PageFlags(u32::from_le_bytes(bytes[12..16].try_into().unwrap())).size() <= self.min_physical
    }

    pub fn budget_for_slot(&self, slot: &str) -> u64 {
        if crate::naming::is_large_slot(slot) {
            self.budget_large
        } else {
            self.budget_small
        }
    }
}
