//! Optimisation of one vehicle file: textures (graphics segment), then geometry (system
//! segment) when the model is above the virtual memory budget.

use anyhow::{bail, Result};
use optistream_core::mib;
use optistream_i18n::{t, tf};
use optistream_texture::budget;

use crate::config::Config;
use crate::process::budget::describe;
use crate::process::{geometry, parse, plan_file, FileInfo, Kind};
use crate::roles::VehicleIndex;

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(
    kind: Kind,
    bytes: &[u8],
    cfg: &Config,
    index: &VehicleIndex,
    info: &FileInfo,
) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, index, info)?;
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut written = false;

    // Textures. The graphics re-layout only patches pointer fields of the system segment:
    // the structure offsets read by the geometry stage stay valid.
    if !p.textures.is_empty() {
        let fp = plan_file(&p, cfg)?;
        let tex_changes: Vec<String> = fp.blocks.iter().filter_map(|b| describe(&p, b)).collect();
        warnings.extend(fp.warnings);
        let before = p.res.graphics_flags.size();
        if fp.physical_after > before {
            warnings.push(tf!("vehicle.warn.textures_heavier", after = mib(fp.physical_after), before = mib(before)));
        } else if !fp.blocks.is_empty() && (!tex_changes.is_empty() || fp.physical_after < before) {
            if tex_changes.is_empty() {
                changes.push(t("vehicle.change.repacked").to_owned());
            }
            changes.extend(tex_changes);
            let entries: Vec<_> = p.textures.iter().map(|t| t.entry.clone()).collect();
            let lens: Vec<usize> = p.textures.iter().map(|t| t.data_len).collect();
            budget::apply(&mut p.res, &entries, &lens, fp.blocks)?;
            written = true;
        }
    }

    // Geometry (virtual memory).
    if kind != Kind::Ytd {
        match geometry::plan(&p.res, &p.drawables, cfg) {
            Ok(Some(gp)) => {
                warnings.extend(gp.warnings.iter().cloned());
                if !gp.writes.is_empty() && gp.virtual_after < p.res.system_flags.size() {
                    // Applied on a copy: a layout refused by the re-layout keeps the texture result.
                    let mut res = p.res.clone();
                    match geometry::apply(&mut res, &p.drawables, &gp) {
                        Ok(()) => {
                            p.res = res;
                            changes.extend(gp.changes.iter().cloned());
                            written = true;
                        }
                        Err(e) => warnings.push(tf!("vehicle.warn.geometry_unchanged", error = format!("{e:#}"))),
                    }
                }
            }
            Ok(None) => {}
            // Layout not understood (shared or aliased buffers, pointer into geometry data):
            // the textures are still written, the geometry is left as it is.
            Err(e) => warnings.push(tf!("vehicle.warn.geometry_unchanged", error = format!("{e:#}"))),
        }
    }

    if !written {
        return Ok((None, changes, warnings));
    }
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, index, info)?;
    if check.textures.len() != p.textures.len() || check.drawables.len() != p.drawables.len() {
        bail!(t("vehicle.error.verify"));
    }
    let geoms = |ds: &[optistream_rage::drawable::DrawableInfo]| ds.iter().map(|d| d.geometries().count()).sum::<usize>();
    if geoms(&check.drawables) != geoms(&p.drawables) {
        bail!(t("vehicle.error.verify_geometries"));
    }
    Ok((Some(out), changes, warnings))
}
