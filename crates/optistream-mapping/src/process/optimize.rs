//! Optimisation of one map file: the plan applied and rebuilt into a new resource.

use anyhow::{bail, Result};
use optistream_core::mib;
use optistream_rage::gfx::{self, GfxBlock};
use optistream_rage::texture::{self as rtex};

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
        warnings.push(format!("non modifié : le résultat ({}) serait plus lourd que l'original ({})", mib(fp.physical_after), mib(before)));
        return Ok((None, Vec::new(), warnings));
    }
    let any_change = !changes.is_empty();
    if !any_change && fp.physical_after >= p.res.graphics_flags.size() {
        return Ok((None, changes, warnings));
    }
    let mut new_blocks = Vec::with_capacity(fp.blocks.len());
    for b in fp.blocks {
        let refs: Vec<usize> = b.users.iter().map(|&u| p.textures[u].entry.data_ptr_field()).collect();
        match b.enc {
            Some(enc) => {
                for &u in &b.users {
                    rtex::write_texture_header(
                        &mut p.res,
                        p.textures[u].entry.struct_offset,
                        enc.width as u16,
                        enc.height as u16,
                        enc.stride,
                        enc.format,
                        enc.levels as u8,
                    );
                }
                new_blocks.push(GfxBlock { data: enc.data, refs });
            }
            None => {
                let len = p.textures[b.users[0]].data_len;
                new_blocks.push(GfxBlock { data: p.res.graphics[b.off..b.off + len].to_vec(), refs });
            }
        }
    }
    if !any_change {
        changes.push("pages graphiques recompactées".to_owned());
    }
    gfx::rebuild(&mut p.res, new_blocks)?;
    let out = p.res.to_bytes(9)?;
    // Sanity check: the result must parse back with the same structure.
    let check = parse(kind, &out, index)?;
    if check.textures.len() != p.textures.len() || check.drawables.len() != p.drawables.len() {
        bail!("vérification après écriture échouée");
    }
    Ok((Some(out), changes, warnings))
}

// ---------------------------------------------------------------- previews
