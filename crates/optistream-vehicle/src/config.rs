//! Parameters, presets and their typed view.

use optistream_core::{ParamKind, ParamSpec, Preset, Settings, Value};
use optistream_i18n::t;
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
    let (general, textures, budget, geometry) =
        (t("vehicle.group.general"), t("vehicle.group.textures"), t("vehicle.group.budget"), t("vehicle.group.geometry"));
    let mib = t("vehicle.unit.mib");
    vec![
        ParamSpec {
            key: OPT_MIN_PHYSICAL,
            label: t("vehicle.param.min_physical.label"),
            group: general,
            help: t("vehicle.param.min_physical.help"),
            kind: ParamKind::Int { min: 0, max: 128, step: 4, unit: mib },
        },
        flag(TEX_CONVERT, t("vehicle.param.convert.label"), textures, t("vehicle.param.convert.help")),
        flag(TEX_FORCE_POT, t("vehicle.param.force_pot.label"), textures, t("vehicle.param.force_pot.help")),
        flag(TEX_FIX_MIPS, t("vehicle.param.fix_mips.label"), textures, t("vehicle.param.fix_mips.help")),
        flag(BUDGET_ENABLED, t("vehicle.param.budget_enabled.label"), budget, t("vehicle.param.budget_enabled.help")),
        ParamSpec {
            key: BUDGET_MIB,
            label: t("vehicle.param.budget.label"),
            group: budget,
            help: t("vehicle.param.budget.help"),
            kind: ParamKind::Int { min: 4, max: 64, step: 2, unit: mib },
        },
        flag(BUDGET_SMALL, t("vehicle.param.small.label"), budget, t("vehicle.param.small.help")),
        ParamSpec {
            key: BUDGET_MAX_STEPS,
            label: t("vehicle.param.max_steps.label"),
            group: budget,
            help: t("vehicle.param.max_steps.help"),
            kind: ParamKind::Int { min: 1, max: 3, step: 1, unit: "" },
        },
        ParamSpec {
            key: BUDGET_MIN_SIZE,
            label: t("vehicle.param.min_size.label"),
            group: budget,
            help: t("vehicle.param.min_size.help"),
            kind: ParamKind::Choice(vec![(256, "256 px"), (512, "512 px"), (1024, "1024 px")]),
        },
        ParamSpec {
            key: BUDGET_MAX_LOSS,
            label: t("vehicle.param.max_loss.label"),
            group: budget,
            help: t("vehicle.param.max_loss.help"),
            kind: ParamKind::Float { min: 0.1, max: 10.0, step: 0.1, unit: "%" },
        },
        flag(BUDGET_FORCE, t("vehicle.param.force.label"), budget, t("vehicle.param.force.help")),
        flag(GEO_ENABLED, t("vehicle.param.geo_enabled.label"), geometry, t("vehicle.param.geo_enabled.help")),
        ParamSpec {
            key: GEO_HIGH_FLOOR,
            label: t("vehicle.param.geo_high_floor.label"),
            group: geometry,
            help: t("vehicle.param.geo_high_floor.help"),
            kind: ParamKind::Int { min: 50, max: 100, step: 5, unit: "%" },
        },
        ParamSpec {
            key: GEO_HIGH_ERROR,
            label: t("vehicle.param.geo_high_error.label"),
            group: geometry,
            help: t("vehicle.param.geo_high_error.help"),
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
        preset("leger", t("vehicle.preset.light.label"), t("vehicle.preset.light.description"), false, false, 1, 3.0, false, None),
        preset("equilibre", t("vehicle.preset.balanced.label"), t("vehicle.preset.balanced.description"), true, false, 1, 3.0, false, Some((80, 0.5))),
        preset("fort", t("vehicle.preset.strong.label"), t("vehicle.preset.strong.description"), true, true, 2, 5.0, true, Some((60, 1.0))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use optistream_rage::rsc7::PageFlags;

    const MIB: u64 = 1024 * 1024;

    fn preset_cfg(id: &str) -> Config {
        Config::from_settings(&presets().into_iter().find(|p| p.id == id).unwrap().values)
    }

    /// 16-byte RSC7 header with the given declared system (virtual) and graphics (physical) sizes.
    fn header(system_mib: u32, graphics_mib: u32) -> Vec<u8> {
        // Base shift 11: base page = 0x2000 << 11 = 16 MiB.
        let flags = |mib: u32| PageFlags::from_counts(11, [mib, 0, 0, 0, 0], 0).0;
        let mut b = vec![0u8; 16];
        b[8..12].copy_from_slice(&flags(system_mib).to_le_bytes());
        b[12..16].copy_from_slice(&flags(graphics_mib).to_le_bytes());
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
            assert_eq!(c.budget, 16 * MIB, "{}", p.id);
            assert_eq!(c.min_physical, 16 * MIB, "{}", p.id);
            assert!((1..=3).contains(&c.max_steps), "{}", p.id);
            assert!(c.min_size >= 4, "{}", p.id);
            assert!(c.max_loss > 0.0, "{}", p.id);
            assert!((0.5..=1.0).contains(&c.geo_high_floor), "{}", p.id);
            assert!(c.geo_high_error > 0.0 && c.geo_high_error <= 0.1, "{}", p.id);
        }
    }

    #[test]
    fn presets_get_stronger_in_order() {
        let ids: Vec<&str> = presets().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["leger", "equilibre", "fort"]);
        let (l, e, f) = (preset_cfg("leger"), preset_cfg("equilibre"), preset_cfg("fort"));
        // Light only corrects: no size or geometry reduction.
        assert!(!l.budget_enabled && !l.geo_enabled && !l.force && !l.small);
        for (weak, strong) in [(&l, &e), (&e, &f)] {
            assert!(strong.budget_enabled >= weak.budget_enabled);
            assert!(strong.small >= weak.small);
            assert!(strong.force >= weak.force);
            assert!(strong.geo_enabled >= weak.geo_enabled);
            assert!(strong.max_steps >= weak.max_steps);
            assert!(strong.min_size <= weak.min_size);
            assert!(strong.max_loss >= weak.max_loss);
        }
        // Geometry: a stronger preset keeps less of the High LOD and tolerates more error.
        assert!(f.geo_high_floor < e.geo_high_floor);
        assert!(f.geo_high_error > e.geo_high_error);
    }

    #[test]
    fn empty_settings_give_the_defaults() {
        let c = Config::from_settings(&Settings::default());
        assert_eq!(c.min_physical, 16 * MIB);
        assert!(c.convert_formats && c.fix_mips && c.force_pot && c.budget_enabled);
        assert_eq!(c.budget, 16 * MIB);
        assert!(!c.small && !c.force && !c.geo_enabled);
        assert_eq!((c.max_steps, c.min_size), (1, 256));
        assert_eq!(c.max_loss, 3.0);
        assert!((c.geo_high_floor - 0.8).abs() < 1e-12);
        assert!((c.geo_high_error - 0.005).abs() < 1e-7);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let s = |pairs: &[(&str, Value)]| pairs.iter().fold(Settings::default(), |s, (k, v)| s.with(k, *v));
        let low = Config::from_settings(&s(&[
            (OPT_MIN_PHYSICAL, Value::Int(-8)),
            (BUDGET_MIB, Value::Int(-3)),
            (BUDGET_MAX_STEPS, Value::Int(0)),
            (BUDGET_MIN_SIZE, Value::Int(1)),
            (BUDGET_MAX_LOSS, Value::Float(-5.0)),
            (GEO_HIGH_FLOOR, Value::Int(10)),
            (GEO_HIGH_ERROR, Value::Float(-1.0)),
        ]));
        assert_eq!(low.min_physical, 0);
        assert_eq!(low.budget, MIB);
        assert_eq!(low.max_steps, 1);
        assert_eq!(low.min_size, 4);
        assert_eq!(low.max_loss, 0.0);
        assert_eq!(low.geo_high_floor, 0.5);
        assert_eq!(low.geo_high_error, 0.0);

        let high = Config::from_settings(&s(&[
            (BUDGET_MAX_STEPS, Value::Int(99)),
            (BUDGET_MIN_SIZE, Value::Int(100_000)),
            (GEO_HIGH_FLOOR, Value::Int(250)),
            (GEO_HIGH_ERROR, Value::Float(50.0)),
        ]));
        assert_eq!(high.max_steps, 3);
        assert_eq!(high.min_size, 4096);
        assert_eq!(high.geo_high_floor, 1.0);
        assert!((high.geo_high_error - 0.1).abs() < 1e-7);
    }

    #[test]
    fn limits_mirror_the_config_with_vehicle_force_floors() {
        for p in presets() {
            let c = Config::from_settings(&p.values);
            let l = c.limits();
            assert_eq!((l.enabled, l.budget, l.max_loss, l.force), (c.budget_enabled, c.budget, c.max_loss, c.force), "{}", p.id);
            assert_eq!((l.force_min_size, l.force_max_steps), (FORCE_MIN_SIZE, FORCE_MAX_STEPS));
            // Forcing never goes under what the limits already allow.
            assert!(l.force_max_steps >= c.max_steps && l.force_min_size <= c.min_size, "{}", p.id);
            assert_eq!(l.policy.halvings, 0);
            assert!(!l.policy.halve && !l.policy.alpha_mask);
            assert_eq!(l.texture_policy(2).halvings, 2);
        }
        // Liveries keep a floor above the forced floor, and count as big.
        const { assert!(LIVERY_FLOOR >= FORCE_MIN_SIZE && LIVERY_FLOOR >= BIG_SIDE) };
    }

    #[test]
    fn min_physical_threshold_checks_both_segments_with_geometry() {
        let mut c = preset_cfg("equilibre");
        assert!(c.geo_enabled);
        // Both segments at the 16 MiB threshold: skipped.
        assert!(c.below_min_physical(&header(1, 1)));
        // Virtual memory above: the geometry reduction has work, not skipped.
        assert!(!c.below_min_physical(&header(2, 1)));
        // Physical memory above: never skipped.
        assert!(!c.below_min_physical(&header(1, 2)));
        // Without the geometry reduction, only the physical memory counts.
        c.geo_enabled = false;
        assert!(c.below_min_physical(&header(2, 1)));
        // Threshold 0 or a truncated header: always processed.
        assert!(!c.below_min_physical(&header(1, 1)[..15]));
        c.min_physical = 0;
        assert!(!c.below_min_physical(&header(0, 0)));
    }
}
