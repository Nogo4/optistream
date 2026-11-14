//! Optimisation of one clothing file: meshes then textures, rebuilt into a new resource.

use std::collections::HashSet;

use anyhow::{bail, Result};

use crate::config::Config;
use crate::process::meshes::{optimize_meshes, plan_reorder, write_indices};
use crate::process::textures::optimize_textures;
use crate::process::{parse, FileInfo, Kind};
use crate::crop;

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(
    kind: Kind,
    bytes: &[u8],
    cfg: &Config,
    info: &FileInfo,
) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, info)?;
    let mut warnings = Vec::new();
    let mut changes = Vec::new();
    let mut decimated = HashSet::new();
    if kind == Kind::Ydd && cfg.mesh_enabled && !info.cloth {
        changes.extend(optimize_meshes(&mut p, cfg, &info.slot, &mut warnings, &mut decimated)?);
    }
    // Planned before the textures (system segment offsets do not move), applied after:
    // on its own, a reordering only rewrites the file for a clear gain.
    let reorder = if kind == Kind::Ydd && cfg.reorder && !info.cloth {
        Some(plan_reorder(&p, &decimated)?)
    } else {
        None
    };
    if let (Kind::Ydd, Some(w)) = (kind, &info.crop) {
        let n = crop::remap_uvs(&mut p.res, &p.drawables, w)?;
        changes.push(format!("UV recadrées sur {} ({n} sommets)", w.describe()));
    }
    changes.extend(optimize_textures(&mut p, cfg, info, &mut warnings)?);
    if let Some(plan) = reorder.filter(|r| !r.writes.is_empty() && (!changes.is_empty() || r.worth_alone())) {
        for (g, indices) in &plan.writes {
            write_indices(&mut p.res, g, indices)?;
        }
        changes.push(plan.describe());
    }
    if changes.is_empty() {
        return Ok((None, changes, warnings));
    }
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, info)?;
    if check.textures.len() != p.textures.len() || check.drawables.len() != p.drawables.len() {
        bail!("vérification après écriture échouée");
    }
    Ok((Some(out), changes, warnings))
}

// ---------------------------------------------------------------- previews
