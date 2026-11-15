//! Parameters, presets and their typed view.

use optistream_core::{ParamKind, ParamSpec, Preset, Settings, Value};
use optistream_texture::budget::Limits;
use optistream_texture::TexturePolicy;

pub const OPT_MIN_PHYSICAL: &str = "opt.min_physical_mib";
pub const TEX_CONVERT: &str = "tex.convert_formats";
pub const TEX_FIX_MIPS: &str = "tex.fix_mips";
pub const TEX_FORCE_POT: &str = "tex.force_pot";
pub const BUDGET_ENABLED: &str = "budget.enabled";
pub const BUDGET_MIB: &str = "budget.mib";
pub const BUDGET_SMALL: &str = "budget.small";
pub const BUDGET_MAX_STEPS: &str = "budget.max_steps";
pub const BUDGET_MIN_SIZE: &str = "budget.min_size";
pub const BUDGET_MAX_LOSS: &str = "budget.max_loss_pct";
pub const BUDGET_FORCE: &str = "budget.force";
pub const GEO_ENABLED: &str = "geo.enabled";
pub const GEO_HIGH_FLOOR: &str = "geo.high_floor_pct";
pub const GEO_HIGH_ERROR: &str = "geo.high_error_pct";

/// Smaller side a livery never goes under, even when forcing (user decision): its texts
/// (POLICE, unit numbers) must stay sharp up close.
pub const LIVERY_FLOOR: u32 = 1024;
/// Smaller side from which a texture counts as big (reduced first).
pub const BIG_SIDE: u32 = 1024;
/// Floors of the forced reduction: never down to a few dozen pixels on a vehicle.
pub const FORCE_MIN_SIZE: u32 = 256;
pub const FORCE_MAX_STEPS: u32 = 3;

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
        flag(BUDGET_ENABLED, "Réduire jusqu'au budget", "Réduction ciblée", "Si le fichier dépasse encore le budget après les corrections, les grosses textures (livrées, feuilles de carrosserie ≥ 1024 px, textures qui ont une copie dans le +hi.ytd) sont divisées par 2, celles qui perdent le moins d'abord, jusqu'à passer sous le budget."),
        ParamSpec {
            key: BUDGET_MIB,
            label: "Budget",
            group: "Réduction ciblée",
            help: "Mémoire physique visée par fichier. 16 Mio : plus d'avertissement FiveM.",
            kind: ParamKind::Int { min: 4, max: 64, step: 2, unit: "Mio" },
        },
        flag(BUDGET_SMALL, "Réduire aussi les petites textures", "Réduction ciblée", "Après les grosses textures, les textures de moins de 1024 px (pièces, intérieur) peuvent aussi être réduites, dans les limites ci-dessous."),
        ParamSpec {
            key: BUDGET_MAX_STEPS,
            label: "Divisions max par texture",
            group: "Réduction ciblée",
            help: "1 division = largeur et hauteur ÷ 2 (mémoire ÷ 4).",
            kind: ParamKind::Int { min: 1, max: 3, step: 1, unit: "" },
        },
        ParamSpec {
            key: BUDGET_MIN_SIZE,
            label: "Taille minimale",
            group: "Réduction ciblée",
            help: "Une texture n'est jamais réduite sous cette taille (petit côté). Les livrées ne descendent jamais sous 1024 px.",
            kind: ParamKind::Choice(vec![(256, "256 px"), (512, "512 px"), (1024, "1024 px")]),
        },
        ParamSpec {
            key: BUDGET_MAX_LOSS,
            label: "Perte maximale",
            group: "Réduction ciblée",
            help: "Écart moyen toléré entre la texture réduite et l'originale, en % de la plage noir → blanc. Repère : une livrée 2048 → 1024 ≈ 2,4 %. Une texture qui perdrait plus n'est pas réduite.",
            kind: ParamKind::Float { min: 0.1, max: 10.0, step: 0.1, unit: "%" },
        },
        flag(BUDGET_FORCE, "Forcer sous le budget", "Réduction ciblée", "Si les limites ci-dessus ne suffisent pas, la réduction continue au-delà (perte sans limite, 3 divisions, jamais sous 256 px ni sous 1024 px pour une livrée) ; les feux, gyrophares et vitres (nom) passent en dernier. Les émissives et palettes ne sont jamais réduites."),
        flag(GEO_ENABLED, "Réduire la géométrie au-dessus du budget", "Géométrie", "Si la mémoire virtuelle d'un modèle (.yft, .ydr) dépasse le budget, ses polygones sont réduits jusqu'à passer dessous : LODs lointains d'abord (VLow, Low, Med), le LOD High seulement si ça ne suffit pas. Bords, coutures, pièces mobiles et skinning sont préservés ; la collision et la physique ne sont jamais touchées."),
        ParamSpec {
            key: GEO_HIGH_FLOOR,
            label: "LOD High : triangles gardés au minimum",
            group: "Géométrie",
            help: "Le LOD High (vu de près) n'est jamais réduit sous cette part de ses triangles.",
            kind: ParamKind::Int { min: 50, max: 100, step: 5, unit: "%" },
        },
        ParamSpec {
            key: GEO_HIGH_ERROR,
            label: "LOD High : tolérance de déformation",
            group: "Géométrie",
            help: "Écart maximal toléré sur le LOD High, en % de la taille de la pièce. Les LODs lointains tolèrent 2 à 5 %.",
            kind: ParamKind::Float { min: 0.1, max: 3.0, step: 0.1, unit: "%" },
        },
    ]
}

#[allow(clippy::too_many_arguments)]
fn preset(id: &'static str, label: &'static str, description: &'static str, budget: bool, small: bool, steps: i64, loss: f64, force: bool, geo: Option<(i64, f64)>) -> Preset {
    let values = Settings::default()
        .with(OPT_MIN_PHYSICAL, Value::Int(16))
        .with(TEX_CONVERT, Value::Bool(true))
        .with(TEX_FIX_MIPS, Value::Bool(true))
        .with(TEX_FORCE_POT, Value::Bool(true))
        .with(BUDGET_ENABLED, Value::Bool(budget))
        .with(BUDGET_MIB, Value::Int(16))
        .with(BUDGET_SMALL, Value::Bool(small))
        .with(BUDGET_MAX_STEPS, Value::Int(steps))
        .with(BUDGET_MIN_SIZE, Value::Int(256))
        .with(BUDGET_MAX_LOSS, Value::Float(loss))
        .with(BUDGET_FORCE, Value::Bool(force))
        .with(GEO_ENABLED, Value::Bool(geo.is_some()))
        .with(GEO_HIGH_FLOOR, Value::Int(geo.map_or(80, |g| g.0)))
        .with(GEO_HIGH_ERROR, Value::Float(geo.map_or(0.5, |g| g.1)));
    Preset { id, label, description, values }
}

pub fn presets() -> Vec<Preset> {
    vec![
        preset("leger", "Léger", "Textures : conversion, puissance de 2, mipmaps. Aucune réduction de taille ni de géométrie.", false, false, 1, 3.0, false, None),
        preset("equilibre", "Équilibré", "Comme Léger, puis réduction des grosses textures seulement (livrées, ≥ 1024 px, copies du +hi.ytd) jusqu'à 16 Mio : 1 division, perte faible. Géométrie au-dessus de 16 Mio : LODs lointains, High gardé à 80 % au moins.", true, false, 1, 3.0, false, Some((80, 0.5))),
        preset("fort", "Fort", "Réduction plus poussée (petites textures comprises, 2 divisions, perte moyenne) ; si ça ne suffit pas, réduction forcée jusqu'à 16 Mio, jamais sous 256 px ni sous 1024 px pour une livrée. Géométrie : High gardé à 60 % au moins.", true, true, 2, 5.0, true, Some((60, 1.0))),
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
    pub small: bool,
    pub max_steps: u32,
    pub min_size: u32,
    pub max_loss: f64,
    pub force: bool,
    pub geo_enabled: bool,
    /// Share of the High LOD triangles always kept.
    pub geo_high_floor: f64,
    /// Error tolerance of the High LOD (fraction of the geometry extent).
    pub geo_high_error: f32,
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
            small: s.bool(BUDGET_SMALL, false),
            max_steps: s.int(BUDGET_MAX_STEPS, 1).clamp(1, 3) as u32,
            min_size: s.int(BUDGET_MIN_SIZE, 256).clamp(4, 4096) as u32,
            max_loss: s.float(BUDGET_MAX_LOSS, 3.0).max(0.0),
            force: s.bool(BUDGET_FORCE, false),
            geo_enabled: s.bool(GEO_ENABLED, false),
            geo_high_floor: s.int(GEO_HIGH_FLOOR, 80).clamp(50, 100) as f64 / 100.0,
            geo_high_error: (s.float(GEO_HIGH_ERROR, 0.5).clamp(0.0, 10.0) / 100.0) as f32,
        }
    }

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

    pub fn limits(&self) -> Limits {
        Limits {
            enabled: self.budget_enabled,
            budget: self.budget,
            max_loss: self.max_loss,
            force: self.force,
            force_min_size: FORCE_MIN_SIZE,
            force_max_steps: FORCE_MAX_STEPS,
            policy: self.texture_policy(0),
        }
    }

    /// Whether the declared physical memory of an RSC7 file (header bytes) does not exceed
    /// the threshold, nor its virtual memory when the geometry reduction is on.
    pub fn below_min_physical(&self, bytes: &[u8]) -> bool {
        use optistream_rage::rsc7::PageFlags;
        if self.min_physical == 0 || bytes.len() < 16 {
            return false;
        }
        let flags = |at: usize| PageFlags(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())).size();
        flags(12) <= self.min_physical && (!self.geo_enabled || flags(8) <= self.min_physical)
    }
}
