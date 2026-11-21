//! Optimisation of one map file: the plan applied and rebuilt into a new resource.

use anyhow::{bail, Result};
use optistream_core::mib;
use optistream_i18n::{t, tf};
use optistream_texture::budget;

use crate::config::Config;
use crate::process::budget::describe;
use crate::process::{parse, plan_file, Kind};
use crate::roles::RoleIndex;

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(kind: Kind, bytes: &[u8], cfg: &Config, index: &RoleIndex) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, index)?;
    if p.textures.is_empty() {
        return Ok((None, Vec::new(), Vec::new()));
    }
    let fp = plan_file(&p, cfg)?;
    let mut changes: Vec<String> = fp.blocks.iter().filter_map(|b| describe(&p, b)).collect();
    let mut warnings = fp.warnings;
    if fp.blocks.is_empty() {
        return Ok((None, changes, warnings));
    }
    let before = p.res.graphics_flags.size();
    if fp.physical_after > before {
        // Format fixes alone would make the file heavier (BC7 → DXT5 is the same size,
        // uncompressed alpha textures...): keep the original.
        warnings.push(tf!("mapping.warning.heavier", after = mib(fp.physical_after), before = mib(before)));
        return Ok((None, Vec::new(), warnings));
    }
    let any_change = !changes.is_empty();
    if !any_change && fp.physical_after >= p.res.graphics_flags.size() {
        return Ok((None, changes, warnings));
    }
    if !any_change {
        changes.push(t("mapping.change.recompacted").to_owned());
    }
    let entries: Vec<_> = p.textures.iter().map(|t| t.entry.clone()).collect();
    let lens: Vec<usize> = p.textures.iter().map(|t| t.data_len).collect();
    budget::apply(&mut p.res, &entries, &lens, fp.blocks)?;
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, index)?;
    if check.textures.len() != p.textures.len() || check.drawables.len() != p.drawables.len() {
        bail!(t("mapping.error.verify"));
    }
    Ok((Some(out), changes, warnings))
}

// ---------------------------------------------------------------- previews
