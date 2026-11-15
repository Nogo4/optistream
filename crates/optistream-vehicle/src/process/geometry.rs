//! Geometry reduction of a model above the virtual memory budget.
//!
//! The planner is shared with the clothing method ([`optistream_mesh::budget`]); vehicles
//! only choose the limits and keep the triangle order of every geometry (vehicle shaders
//! are matched by hash only: safe for alpha).

use anyhow::Result;
use optistream_mesh::budget::{self, GeoPlan, Limits};
use optistream_rage::drawable::DrawableInfo;
use optistream_rage::Resource;

use crate::config::Config;

/// Plans the geometry reduction of a file whose declared virtual memory is above the budget.
pub fn plan(res: &Resource, drawables: &[DrawableInfo], cfg: &Config) -> Result<Option<GeoPlan>> {
    if !cfg.geo_enabled || res.system_flags.size() <= cfg.budget {
        return Ok(None);
    }
    let limits = Limits { budget: cfg.budget, high_floor: cfg.geo_high_floor, high_error: cfg.geo_high_error };
    budget::plan(res, drawables, drawables, &limits, |_, _| Some(true)).map(Some)
}

/// Writes the plan: counts patched in the structures, system segment re-laid out.
pub fn apply(res: &mut Resource, drawables: &[DrawableInfo], plan: &GeoPlan) -> Result<()> {
    budget::apply(res, drawables, drawables, plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::presets;

    #[test]
    fn plan_does_nothing_when_disabled_or_under_budget() {
        let res = Resource {
            version: 162,
            system_flags: optistream_rage::rsc7::PageFlags::from_counts(11, [2, 0, 0, 0, 0], 0),
            graphics_flags: optistream_rage::rsc7::PageFlags(0),
            system: Vec::new(),
            graphics: Vec::new(),
        };
        let mut cfg = presets().into_iter().find(|p| p.id == "equilibre").map(|p| Config::from_settings(&p.values)).unwrap();
        // 32 MiB declared against a 64 MiB budget: under it.
        cfg.budget = 64 * 1024 * 1024;
        assert!(plan(&res, &[], &cfg).unwrap().is_none());
        // Above the budget but the reduction is off.
        cfg.budget = 16 * 1024 * 1024;
        cfg.geo_enabled = false;
        assert!(plan(&res, &[], &cfg).unwrap().is_none());
    }
}
