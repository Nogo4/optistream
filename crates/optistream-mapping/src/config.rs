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
