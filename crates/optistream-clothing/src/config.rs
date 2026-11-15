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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use optistream_rage::rsc7::PageFlags;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn preset_config(id: &str) -> Config {
        let p = presets().into_iter().find(|p| p.id == id).unwrap();
        Config::from_settings(&p.values)
    }

    /// RSC7 header whose graphics flags (physical memory) declare `flags`.
    fn header(flags: PageFlags) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        b[12..16].copy_from_slice(&flags.0.to_le_bytes());
        b
    }

    #[test]
    fn empty_settings_give_the_documented_defaults() {
        let c = Config::from_settings(&Settings::default());
        assert!(!c.only_3d && !c.halve);
        assert!(c.crop_uv && c.convert_formats && c.fix_mips && c.force_pot);
        assert!(c.mesh_enabled && c.reorder && c.lower_lods);
        assert_eq!(c.min_physical, 16 * MIB);
        assert_eq!((c.budget_large, c.budget_small), (40_000, 10_000));
        assert!((c.max_error - 0.005).abs() < 1e-9);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_types_coerced() {
        let s = Settings::default()
            .with(OPT_MIN_PHYSICAL, Value::Int(-5))
            .with(MESH_BUDGET_LARGE, Value::Int(0))
            .with(MESH_BUDGET_SMALL, Value::Int(-100))
            .with(MESH_MAX_ERROR, Value::Int(2));
        let c = Config::from_settings(&s);
        assert_eq!(c.min_physical, 0);
        // A zero budget would ask for an empty mesh: at least one triangle.
        assert_eq!((c.budget_large, c.budget_small), (1, 1));
        // Integer percentage read as a float, then turned into a fraction.
        assert!((c.max_error - 0.02).abs() < 1e-9);
        // Float given for an integer parameter: truncated.
        let c = Config::from_settings(&Settings::default().with(OPT_MIN_PHYSICAL, Value::Float(20.9)));
        assert_eq!(c.min_physical, 20 * MIB);
        // Wrong type for a flag: the default is kept.
        let c = Config::from_settings(&Settings::default().with(TEX_CROP, Value::Int(0)));
        assert!(c.crop_uv);
    }

    #[test]
    fn presets_set_every_parameter_within_its_range() {
        let keys: BTreeSet<&str> = params().iter().map(|p| p.key).collect();
        for p in presets() {
            let set: BTreeSet<&str> = p.values.0.keys().map(String::as_str).collect();
            assert_eq!(set, keys, "{}", p.id);
            for spec in params() {
                let v = p.values.get(spec.key).unwrap();
                match (spec.kind, v) {
                    (ParamKind::Bool, Value::Bool(_)) => {}
                    (ParamKind::Int { min, max, .. }, Value::Int(x)) => assert!((min..=max).contains(&x), "{} {}", p.id, spec.key),
                    (ParamKind::Float { min, max, .. }, Value::Float(x)) => assert!(x >= min && x <= max, "{} {}", p.id, spec.key),
                    (k, v) => panic!("{} {}: {v:?} for {k:?}", p.id, spec.key),
                }
            }
        }
    }

    #[test]
    fn presets_get_stronger_in_order() {
        let ids: Vec<&str> = presets().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["leger", "equilibre", "fort"]);
        let [l, e, f] = ["leger", "equilibre", "fort"].map(preset_config);
        assert!(l.budget_large > e.budget_large && e.budget_large > f.budget_large);
        assert!(l.budget_small > e.budget_small && e.budget_small > f.budget_small);
        assert!(l.max_error < e.max_error && e.max_error < f.max_error);
        assert!(!l.crop_uv && e.crop_uv && f.crop_uv);
        assert!(!l.halve && !e.halve && f.halve);
        // Small pieces always get a smaller budget than large ones.
        for c in [&l, &e, &f] {
            assert!(c.budget_small < c.budget_large);
        }
    }

    #[test]
    fn skin_diffuses_keep_their_dimensions() {
        let c = Config { halve: true, force_pot: true, ..Config::from_settings(&Settings::default()) };
        for role in [TextureRole::Diffuse, TextureRole::Other] {
            assert!(Config::keeps_dims(role, true));
            assert!(!Config::keeps_dims(role, false));
            let p = c.texture_policy(role, true, true);
            assert!(!p.halve && !p.force_pot && !p.alpha_mask);
            // Format conversion and mips never change the dimensions: still applied.
            assert!(p.convert_formats && p.fix_mips);
        }
        // Normal and specular maps of a skin drawable are not tied to the MP skin.
        for role in [TextureRole::Normal, TextureRole::Specular] {
            assert!(!Config::keeps_dims(role, true));
            let p = c.texture_policy(role, true, false);
            assert!(p.halve && p.force_pot);
        }
        let p = c.texture_policy(TextureRole::Diffuse, false, true);
        assert!(p.halve && p.force_pot && p.alpha_mask);
        assert_eq!(p.halvings, 0);
    }

    #[test]
    fn min_physical_threshold_is_inclusive() {
        let c = Config::from_settings(&Settings::default());
        // Base page 2 MiB (0x2000 << 8).
        assert!(c.below_min_physical(&header(PageFlags::from_counts(8, [8, 0, 0, 0, 0], 0))));
        assert!(c.below_min_physical(&header(PageFlags::from_counts(8, [1, 0, 0, 0, 0], 0))));
        assert!(!c.below_min_physical(&header(PageFlags::from_counts(8, [9, 0, 0, 0, 0], 0))));
        // A header too short to read is never skipped.
        assert!(!c.below_min_physical(&[0u8; 15]));
        // Threshold 0: every file is processed.
        let all = Config::from_settings(&Settings::default().with(OPT_MIN_PHYSICAL, Value::Int(0)));
        assert!(!all.below_min_physical(&header(PageFlags(0))));
    }

    #[test]
    fn slot_budgets() {
        let c = Config::from_settings(&Settings::default());
        for slot in ["uppr", "lowr", "jbib", "accs", "task", "hair", "feet", "berd"] {
            assert_eq!(c.budget_for_slot(slot), 40_000, "{slot}");
        }
        for slot in ["hand", "teef", "decl", "p_head", "p_eyes", ""] {
            assert_eq!(c.budget_for_slot(slot), 10_000, "{slot}");
        }
    }
}
