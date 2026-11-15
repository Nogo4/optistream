//! Optimisation of one clothing file: meshes then textures, rebuilt into a new resource.

use std::collections::HashSet;

use anyhow::{bail, Result};
use optistream_mesh::budget;
use optistream_rage::drawable::{self, DrawableInfo, GeometryInfo};
use optistream_rage::Resource;

use crate::config::Config;
use crate::process::analyze::has_cloth;
use crate::process::meshes::{optimize_meshes, plan_reorder, write_indices};
use crate::process::textures::optimize_textures;
use crate::process::{parse, FileInfo, Kind, Parsed};
use crate::{crop, shaders};

/// Full optimisation of one file. Returns `(new bytes, changes, warnings)`.
pub fn optimize(
    kind: Kind,
    bytes: &[u8],
    cfg: &Config,
    info: &FileInfo,
) -> Result<(Option<Vec<u8>>, Vec<String>, Vec<String>)> {
    let mut p = parse(kind, bytes, info)?;
    // Drawables as first read: the room allocated to each vertex / index buffer.
    let original = p.drawables.clone();
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
    if kind == Kind::Ydd {
        virtual_memory(&mut p, &original, cfg, info.cloth, &mut changes, &mut warnings)?;
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
    let geoms = |ds: &[DrawableInfo]| ds.iter().map(|d| d.geometries().count()).sum::<usize>();
    if geoms(&check.drawables) != geoms(&p.drawables) {
        bail!("vérification après écriture échouée (géométries)");
    }
    Ok((Some(out), changes, warnings))
}

/// Virtual memory stage, after every in-place edit: the system segment is re-laid out with
/// each buffer cut to its used length (decimated geometries keep a zero-filled tail until
/// then) and, if still above 16 MiB, geometry is reduced until it gets under
/// ([`optistream_mesh::budget`]). A file with cloth (`.yld`) and drawables with cloth
/// geometry are only re-laid out: their vertices are never merged or decimated.
fn virtual_memory(
    p: &mut Parsed,
    original: &[DrawableInfo],
    cfg: &Config,
    cloth: bool,
    changes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let before = p.res.system_flags.size();
    // `None`: no gain, the resource is left as it is.
    let outcome = (|| -> Result<(Option<Resource>, budget::GeoPlan)> {
        let current = drawable::read_dictionary(&p.res, 0)?;
        let policy = |di: usize, g: &GeometryInfo| -> Option<bool> {
            let d = current.get(di)?;
            (!cloth && !has_cloth(d)).then(|| shaders::is_order_sensitive(g.shader_index.and_then(|i| d.shaders.get(i as usize))))
        };
        let gp = budget::plan(&p.res, original, &current, &cfg.geo_limits(), policy)?;
        if gp.virtual_after >= before {
            return Ok((None, gp));
        }
        // Applied on a copy: a layout refused by the re-layout keeps the result so far.
        let mut res = p.res.clone();
        budget::apply(&mut res, original, &current, &gp)?;
        Ok((Some(res), gp))
    })();
    match outcome {
        Ok((res, gp)) => {
            warnings.extend(gp.warnings);
            if let Some(res) = res {
                p.res = res;
                changes.push(format!("mémoire virtuelle : {} → {}", budget::mib(before), budget::mib(gp.virtual_after)));
                changes.extend(gp.changes);
            }
        }
        // Layout not understood (shared or aliased buffers, pointer into geometry data):
        // the file keeps its in-place edits.
        Err(e) => {
            if before > crate::config::GEO_BUDGET {
                warnings.push(format!("mémoire virtuelle non réduite : {e:#}"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- previews
