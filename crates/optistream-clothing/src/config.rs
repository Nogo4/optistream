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
pub const GEO_ENABLED: &str = "geo.enabled";
pub const GEO_HIGH_FLOOR: &str = "geo.high_floor_pct";
pub const GEO_HIGH_ERROR: &str = "geo.high_error_pct";

/// Virtual memory the geometry reduction gets under (FiveM warning threshold).
pub const GEO_BUDGET: u64 = 16 * 1024 * 1024;

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
            help: "Seuls les fichiers dont la mémoire physique déclarée dépasse cette valeur sont optimisés (ou la mémoire virtuelle, si « Réduire sous 16 Mio de mémoire virtuelle » est actif) ; 16 Mio est le seuil d'avertissement de FiveM. 0 : tous les fichiers.",
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
        flag(GEO_ENABLED, "Réduire sous 16 Mio de mémoire virtuelle", "Mémoire virtuelle", "Si un .ydd dépasse encore 16 Mio de mémoire virtuelle (avertissement FiveM) après le budget de triangles, ses polygones sont réduits jusqu'à passer dessous : LODs lointains d'abord (VLow, Low, Med), le LOD High seulement si ça ne suffit pas. Bords, coutures et skinning préservés ; jamais sur le cloth. Le segment système est toujours recompacté (sans perte)."),
        ParamSpec {
            key: GEO_HIGH_FLOOR,
            label: "Plancher du LOD High",
            group: "Mémoire virtuelle",
            help: "Part minimale des triangles que le LOD High garde pour passer sous 16 Mio (après le budget de triangles).",
            kind: ParamKind::Int { min: 50, max: 100, step: 5, unit: "%" },
        },
        ParamSpec {
            key: GEO_HIGH_ERROR,
            label: "Tolérance du LOD High",
            group: "Mémoire virtuelle",
            help: "Écart de forme maximal du LOD High pendant cette réduction, en % de la taille de la pièce (les LODs lointains sont plus permissifs).",
            kind: ParamKind::Float { min: 0.0, max: 10.0, step: 0.1, unit: "%" },
        },
    ]
}

#[allow(clippy::too_many_arguments)]
fn preset(id: &'static str, label: &'static str, description: &'static str, budgets: [i64; 2], err: f64, halve: bool, crop: bool, geo: Option<(i64, f64)>) -> Preset {
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
        .with(MESH_LOWER_LODS, Value::Bool(true))
        .with(GEO_ENABLED, Value::Bool(geo.is_some()))
        .with(GEO_HIGH_FLOOR, Value::Int(geo.map_or(80, |g| g.0)))
        .with(GEO_HIGH_ERROR, Value::Float(geo.map_or(0.5, |g| g.1)));
    Preset { id, label, description, values }
}

pub fn presets() -> Vec<Preset> {
    vec![
        preset("leger", "Léger", "Textures : conversion, puissance de 2, mipmaps. ≤ 80k tris.", [80_000, 20_000], 0.2, false, false, None),
        preset("equilibre", "Équilibré", "Textures : conversion, puissance de 2, mipmaps, recadrage sur la zone utilisée. ≤ 40k tris, puis sous 16 Mio de mémoire virtuelle (High ≥ 80 %).", [40_000, 10_000], 0.5, false, true, Some((80, 0.5))),
        preset("fort", "Fort", "Textures : comme Équilibré, puis divisées par 2. ≤ 20k tris, puis sous 16 Mio de mémoire virtuelle (High ≥ 60 %).", [20_000, 5_000], 1.0, true, true, Some((60, 1.0))),
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
    pub geo_enabled: bool,
    /// Smallest share of its triangles the High LOD keeps in the virtual memory reduction.
    pub geo_high_floor: f64,
    /// Error tolerance of the High LOD in that reduction (fraction of the geometry extent).
    pub geo_high_error: f32,
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
            geo_enabled: s.bool(GEO_ENABLED, true),
            geo_high_floor: s.int(GEO_HIGH_FLOOR, 80).clamp(50, 100) as f64 / 100.0,
            geo_high_error: (s.float(GEO_HIGH_ERROR, 0.5).clamp(0.0, 10.0) / 100.0) as f32,
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

    /// Whether the declared physical memory of an RSC7 file (header bytes) does not exceed
    /// the threshold, nor its virtual memory when the virtual memory reduction is on.
    pub fn below_min_physical(&self, bytes: &[u8]) -> bool {
        use optistream_rage::rsc7::PageFlags;
        if self.min_physical == 0 || bytes.len() < 16 {
            return false;
        }
        let flags = |at: usize| PageFlags(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())).size();
        flags(12) <= self.min_physical && (!self.geo_enabled || flags(8) <= self.min_physical)
    }

    /// Limits of the virtual memory stage; with the reduction off, only the lossless
    /// re-layout of the system segment is done.
    pub fn geo_limits(&self) -> optistream_mesh::budget::Limits {
        optistream_mesh::budget::Limits {
            budget: if self.geo_enabled && self.mesh_enabled { GEO_BUDGET } else { u64::MAX },
            high_floor: self.geo_high_floor,
            high_error: self.geo_high_error,
        }
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
        header2(PageFlags(0), flags)
    }

    /// RSC7 header with the given system (virtual) and graphics (physical) flags.
    fn header2(system: PageFlags, graphics: PageFlags) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        b[8..12].copy_from_slice(&system.0.to_le_bytes());
        b[12..16].copy_from_slice(&graphics.0.to_le_bytes());
        b
    }

    #[test]
    fn empty_settings_give_the_documented_defaults() {
        let c = Config::from_settings(&Settings::default());
        assert!(!c.only_3d && !c.halve);
        assert!(c.crop_uv && c.convert_formats && c.fix_mips && c.force_pot);
        assert!(c.mesh_enabled && c.reorder && c.lower_lods && c.geo_enabled);
        assert!((c.geo_high_floor - 0.8).abs() < 1e-12 && (c.geo_high_error - 0.005).abs() < 1e-7);
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
        // Léger never reduces for the virtual memory; Fort goes further than Équilibré.
        assert!(!l.geo_enabled && e.geo_enabled && f.geo_enabled);
        assert!(f.geo_high_floor < e.geo_high_floor && f.geo_high_error > e.geo_high_error);
        assert_eq!(l.geo_limits().budget, u64::MAX);
        assert_eq!(e.geo_limits().budget, GEO_BUDGET);
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
    fn min_physical_threshold_checks_the_virtual_memory_with_the_reduction() {
        // 2 MiB physical, 32 MiB virtual (the warned hair / jbib files).
        let bytes = header2(PageFlags::from_counts(8, [16, 0, 0, 0, 0], 0), PageFlags::from_counts(8, [1, 0, 0, 0, 0], 0));
        assert!(!preset_config("equilibre").below_min_physical(&bytes));
        assert!(!preset_config("fort").below_min_physical(&bytes));
        // Reduction off (Léger): the physical memory alone decides.
        assert!(preset_config("leger").below_min_physical(&bytes));
        // Virtual memory at the threshold: still skipped.
        let at = header2(PageFlags::from_counts(8, [8, 0, 0, 0, 0], 0), PageFlags::from_counts(8, [1, 0, 0, 0, 0], 0));
        assert!(preset_config("equilibre").below_min_physical(&at));
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
