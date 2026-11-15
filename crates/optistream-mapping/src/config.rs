//! Parameters, presets and their typed view.

use optistream_core::{ParamKind, ParamSpec, Preset, Settings, Value};
use optistream_texture::TexturePolicy;

pub const OPT_MIN_PHYSICAL: &str = "opt.min_physical_mib";
pub const TEX_CONVERT: &str = "tex.convert_formats";
pub const TEX_FIX_MIPS: &str = "tex.fix_mips";
pub const TEX_FORCE_POT: &str = "tex.force_pot";
pub const BUDGET_ENABLED: &str = "budget.enabled";
pub const BUDGET_MIB: &str = "budget.mib";
pub const BUDGET_MAX_STEPS: &str = "budget.max_steps";
pub const BUDGET_MIN_SIZE: &str = "budget.min_size";
pub const BUDGET_MAX_LOSS: &str = "budget.max_loss_pct";
pub const BUDGET_FORCE: &str = "budget.force";

/// Floors of the forced reduction (used when the limits above do not reach the budget).
pub const FORCE_MIN_SIZE: u32 = 64;
pub const FORCE_MAX_STEPS: u32 = 6;

/// Default largest loss accepted for a texture (Équilibré), see [`optistream_texture::quality`]:
/// a wood floor ÷2 passes (1.5 %), a slatted wall (3.0 %) or printed text (14 %) does not.
pub const DEFAULT_MAX_LOSS: f64 = 1.6;

pub fn params() -> Vec<ParamSpec> {
    let flag = |key, label, group, help| ParamSpec { key, label, group, help, kind: ParamKind::Bool };
    vec![
        ParamSpec {
            key: OPT_MIN_PHYSICAL,
            label: "Mémoire physique minimale",
            group: "Général",
            help: "Seuls les fichiers dont la mémoire physique déclarée dépasse cette valeur sont optimisés ; 16 Mio est le seuil d'avertissement de FiveM. 0 : tous les fichiers.",
            kind: ParamKind::Int { min: 0, max: 128, step: 4, unit: "Mio" },
        },
        flag(TEX_CONVERT, "Convertir BC7 / DXT3 / non compressé", "Textures", "Normal maps → DXT5 ; autres → DXT1, ou DXT5 si l'alpha est réellement utilisé. Les DXT1 / DXT5 gardent leur format."),
        flag(TEX_FORCE_POT, "Forcer les puissances de 2", "Textures", "Arrondi vers le bas : 3000×3000 → 2048×2048, etc."),
        flag(TEX_FIX_MIPS, "Compléter les mipmaps", "Textures", "Chaîne complète jusqu'à 4 px, comme le jeu de base."),
        flag(BUDGET_ENABLED, "Réduire jusqu'au budget", "Réduction ciblée", "Si le fichier dépasse encore le budget après les corrections, les textures qui perdent le moins à être divisées par 2 le sont, une à la fois, jusqu'à passer sous le budget ou atteindre les limites ci-dessous."),
        ParamSpec {
            key: BUDGET_MIB,
            label: "Budget",
            group: "Réduction ciblée",
            help: "Mémoire physique visée par fichier. 16 Mio : plus d'avertissement FiveM.",
            kind: ParamKind::Int { min: 4, max: 64, step: 2, unit: "Mio" },
        },
        ParamSpec {
            key: BUDGET_MAX_STEPS,
            label: "Divisions max par texture",
            group: "Réduction ciblée",
            help: "1 division = largeur et hauteur ÷ 2 (mémoire ÷ 4).",
            kind: ParamKind::Int { min: 1, max: 4, step: 1, unit: "" },
        },
        ParamSpec {
            key: BUDGET_MIN_SIZE,
            label: "Taille minimale",
            group: "Réduction ciblée",
            help: "Une texture n'est jamais réduite sous cette taille (petit côté).",
            kind: ParamKind::Choice(vec![(128, "128 px"), (256, "256 px"), (512, "512 px"), (1024, "1024 px")]),
        },
        ParamSpec {
            key: BUDGET_MAX_LOSS,
            label: "Perte maximale",
            group: "Réduction ciblée",
            help: "Écart moyen toléré entre la texture réduite et l'originale, en % de la plage noir → blanc (0 % : identique). Repères : mur de plâtre ÷2 ≈ 0,9 %, parquet ÷2 ≈ 1,5 %, lames de bois ÷2 ≈ 3 %, texte imprimé ≈ 14 %. Une texture qui perdrait plus n'est pas réduite.",
            kind: ParamKind::Float { min: 0.1, max: 10.0, step: 0.1, unit: "%" },
        },
        flag(BUDGET_FORCE, "Forcer sous le budget", "Réduction ciblée", "Si les limites ci-dessus ne suffisent pas, la réduction continue au-delà (perte, divisions, taille minimale jusqu'à 64 px), toujours en commençant par les textures qui perdent le moins ; les textes / logos et les rôles inconnus passent en dernier. Les palettes, émissives et script_rt ne sont jamais réduites."),
    ]
}

fn preset(id: &'static str, label: &'static str, description: &'static str, budget: bool, steps: i64, min_size: i64, loss: f64, force: bool) -> Preset {
    let values = Settings::default()
        .with(OPT_MIN_PHYSICAL, Value::Int(16))
        .with(TEX_CONVERT, Value::Bool(true))
        .with(TEX_FIX_MIPS, Value::Bool(true))
        .with(TEX_FORCE_POT, Value::Bool(true))
        .with(BUDGET_ENABLED, Value::Bool(budget))
        .with(BUDGET_MIB, Value::Int(16))
        .with(BUDGET_MAX_STEPS, Value::Int(steps))
        .with(BUDGET_MIN_SIZE, Value::Int(min_size))
        .with(BUDGET_MAX_LOSS, Value::Float(loss))
        .with(BUDGET_FORCE, Value::Bool(force));
    Preset { id, label, description, values }
}

pub fn presets() -> Vec<Preset> {
    vec![
        preset("leger", "Léger", "Textures : conversion, puissance de 2, mipmaps. Aucune réduction de taille.", false, 2, 512, DEFAULT_MAX_LOSS, false),
        preset("equilibre", "Équilibré", "Comme Léger, puis réduction ciblée jusqu'à 16 Mio : 2 divisions max, jamais sous 512 px, perte faible.", true, 2, 512, DEFAULT_MAX_LOSS, false),
        preset("fort", "Fort", "Réduction ciblée plus poussée (3 divisions max, 256 px, perte moyenne) ; si ça ne suffit pas, réduction forcée jusqu'à passer sous 16 Mio.", true, 3, 256, DEFAULT_MAX_LOSS * 2.0, true),
    ]
}

/// Typed view of the settings.
#[derive(Clone, Debug)]
pub struct Config {
    pub min_physical: u64,
    pub convert_formats: bool,
    pub fix_mips: bool,
    pub force_pot: bool,
    pub budget_enabled: bool,
    pub budget: u64,
    pub max_steps: u32,
    pub min_size: u32,
    pub max_loss: f64,
    /// Go past the limits above (never below [`FORCE_MIN_SIZE`]) when they do not reach the budget.
    pub force: bool,
}

impl Config {
    pub fn from_settings(s: &Settings) -> Self {
        Config {
            min_physical: s.int(OPT_MIN_PHYSICAL, 16).max(0) as u64 * 1024 * 1024,
            convert_formats: s.bool(TEX_CONVERT, true),
            fix_mips: s.bool(TEX_FIX_MIPS, true),
            force_pot: s.bool(TEX_FORCE_POT, true),
            budget_enabled: s.bool(BUDGET_ENABLED, true),
            budget: s.int(BUDGET_MIB, 16).max(1) as u64 * 1024 * 1024,
            max_steps: s.int(BUDGET_MAX_STEPS, 2).clamp(1, 4) as u32,
            min_size: s.int(BUDGET_MIN_SIZE, 512).clamp(4, 4096) as u32,
            max_loss: s.float(BUDGET_MAX_LOSS, DEFAULT_MAX_LOSS).max(0.0),
            force: s.bool(BUDGET_FORCE, false),
        }
    }

    /// Policy of one texture, `halvings` times divided by 2 on top of the corrections.
    pub fn texture_policy(&self, halvings: u32) -> TexturePolicy {
        TexturePolicy {
            halve: false,
            halvings,
            convert_formats: self.convert_formats,
            fix_mips: self.fix_mips,
            force_pot: self.force_pot,
            alpha_mask: false,
        }
    }

    /// Whether the declared physical memory of an RSC7 file (header bytes) does not exceed the threshold.
    pub fn below_min_physical(&self, bytes: &[u8]) -> bool {
        self.min_physical > 0
            && bytes.len() >= 16
            && optistream_rage::rsc7::PageFlags(u32::from_le_bytes(bytes[12..16].try_into().unwrap())).size() <= self.min_physical
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use optistream_rage::rsc7::PageFlags;

    const MIB: u64 = 1024 * 1024;

    fn preset_cfg(id: &str) -> Config {
        Config::from_settings(&presets().into_iter().find(|p| p.id == id).unwrap().values)
    }

    /// 16-byte RSC7 header declaring `graphics_mib` MiB of physical memory (multiple of 16).
    fn header(graphics_mib: u32) -> Vec<u8> {
        // Base shift 11: base page = 0x2000 << 11 = 16 MiB.
        let mut b = vec![0u8; 16];
        b[12..16].copy_from_slice(&PageFlags::from_counts(11, [graphics_mib / 16, 0, 0, 0, 0], 0).0.to_le_bytes());
        b
    }

    #[test]
    fn every_preset_sets_every_param_within_its_range() {
        let specs = params();
        for p in presets() {
            for spec in &specs {
                let v = p.values.0.get(spec.key).unwrap_or_else(|| panic!("{}: {} missing", p.id, spec.key));
                match (&spec.kind, v) {
                    (ParamKind::Bool, Value::Bool(_)) => {}
                    (ParamKind::Int { min, max, .. }, Value::Int(x)) => assert!((min..=max).contains(&x), "{}: {}", p.id, spec.key),
                    (ParamKind::Float { min, max, .. }, Value::Float(x)) => assert!(*x >= *min && *x <= *max, "{}: {}", p.id, spec.key),
                    (ParamKind::Choice(c), Value::Int(x)) => assert!(c.iter().any(|(k, _)| k == x), "{}: {}", p.id, spec.key),
                    (k, v) => panic!("{}: {} has {v:?} for {k:?}", p.id, spec.key),
                }
            }
        }
    }

    #[test]
    fn every_preset_parses_to_a_valid_config() {
        for p in presets() {
            let c = Config::from_settings(&p.values);
            assert_eq!((c.budget, c.min_physical), (16 * MIB, 16 * MIB), "{}", p.id);
            assert!((1..=4).contains(&c.max_steps), "{}", p.id);
            assert!(c.min_size >= 128, "{}", p.id);
            assert!(c.max_loss > 0.0, "{}", p.id);
            // Forcing always goes further than the limits of the preset.
            assert!(FORCE_MAX_STEPS >= c.max_steps && FORCE_MIN_SIZE <= c.min_size, "{}", p.id);
        }
    }

    #[test]
    fn presets_get_stronger_in_order() {
        let ids: Vec<&str> = presets().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["leger", "equilibre", "fort"]);
        let (l, e, f) = (preset_cfg("leger"), preset_cfg("equilibre"), preset_cfg("fort"));
        assert!(!l.budget_enabled && !l.force);
        for (weak, strong) in [(&l, &e), (&e, &f)] {
            assert!(strong.budget_enabled >= weak.budget_enabled);
            assert!(strong.force >= weak.force);
            assert!(strong.max_steps >= weak.max_steps);
            assert!(strong.min_size <= weak.min_size);
            assert!(strong.max_loss >= weak.max_loss);
        }
        assert_eq!(e.max_loss, DEFAULT_MAX_LOSS);
    }

    #[test]
    fn empty_settings_give_the_defaults() {
        let c = Config::from_settings(&Settings::default());
        assert_eq!((c.min_physical, c.budget), (16 * MIB, 16 * MIB));
        assert!(c.convert_formats && c.fix_mips && c.force_pot && c.budget_enabled && !c.force);
        assert_eq!((c.max_steps, c.min_size), (2, 512));
        assert_eq!(c.max_loss, DEFAULT_MAX_LOSS);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let s = |pairs: &[(&str, Value)]| pairs.iter().fold(Settings::default(), |s, (k, v)| s.with(k, *v));
        let low = Config::from_settings(&s(&[
            (OPT_MIN_PHYSICAL, Value::Int(-1)),
            (BUDGET_MIB, Value::Int(0)),
            (BUDGET_MAX_STEPS, Value::Int(-2)),
            (BUDGET_MIN_SIZE, Value::Int(0)),
            (BUDGET_MAX_LOSS, Value::Float(-0.5)),
        ]));
        assert_eq!((low.min_physical, low.budget), (0, MIB));
        assert_eq!((low.max_steps, low.min_size), (1, 4));
        assert_eq!(low.max_loss, 0.0);
        let high = Config::from_settings(&s(&[(BUDGET_MAX_STEPS, Value::Int(10)), (BUDGET_MIN_SIZE, Value::Int(65_536))]));
        assert_eq!((high.max_steps, high.min_size), (4, 4096));
        // An integer given for a float parameter still counts.
        assert_eq!(Config::from_settings(&s(&[(BUDGET_MAX_LOSS, Value::Int(5))])).max_loss, 5.0);
    }

    #[test]
    fn texture_policy_keeps_the_corrections_and_adds_halvings() {
        let mut c = preset_cfg("fort");
        c.convert_formats = false;
        let p = c.texture_policy(3);
        assert_eq!(p.halvings, 3);
        assert!(!p.halve && !p.alpha_mask && !p.convert_formats && p.fix_mips && p.force_pot);
    }

    #[test]
    fn min_physical_threshold() {
        let mut c = preset_cfg("equilibre");
        assert!(c.below_min_physical(&header(16)));
        assert!(!c.below_min_physical(&header(32)));
        assert!(!c.below_min_physical(&header(16)[..15]));
        c.min_physical = 0;
        assert!(!c.below_min_physical(&header(0)));
    }
}
