//! Texture optimisation of one file, with the UV crop window when the group has one.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Result};
use optistream_i18n::{self as i18n, tf};
use optistream_rage::gfx::{self, GfxBlock};
use optistream_rage::texture::{self as rtex};
use optistream_rage::pages;
use optistream_texture::format::{self as fmt};
use optistream_texture::{plan, resource as texres};
use rayon::prelude::*;

use crate::config::Config;
use crate::crop::{self, Window};
use crate::process::parse::role_of;
use crate::process::{FileInfo, Parsed, TexItem};

/// Optimises textures and re-lays out the graphics segment. Returns change descriptions.
/// Optimises one texture, after cutting it to the crop window if there is one. A cropped
/// texture is always rewritten (its dimensions changed) and always gets its mips back.
pub(super) fn optimize_texture(t: &TexItem, original: &[u8], cfg: &Config, skin: bool, window: Option<&Window>) -> Result<Option<plan::Encoded>> {
    let Some(role) = role_of(t.usage) else {
        ensure!(window.is_none(), i18n::t("clothing.error.protected_in_crop"));
        return Ok(None);
    };
    let mut policy = cfg.texture_policy(role, skin, t.alpha_mask);
    let cropped = match window {
        Some(w) => crop::crop_texture(&t.meta(), original, w)?,
        None => None,
    };
    let Some((meta, data)) = cropped else {
        return plan::optimize(&t.meta(), original, role, &policy);
    };
    policy.fix_mips = true;
    let mut enc = match plan::optimize(&meta, &data, role, &policy)? {
        Some(enc) => enc,
        None => plan::Encoded {
            width: meta.width,
            height: meta.height,
            format: meta.format,
            levels: 1,
            stride: fmt::stride(meta.format, meta.width, meta.height),
            data,
            changes: Vec::new(),
        },
    };
    // Changes are listed against the original texture, not the cropped intermediate.
    enc.changes.retain(|c| !c.starts_with("mips "));
    enc.changes.insert(
        0,
        tf!(
            "clothing.change.texture_cropped",
            from_width = t.entry.width,
            from_height = t.entry.height,
            to_width = meta.width,
            to_height = meta.height
        ),
    );
    if enc.levels != t.entry.levels as u32 {
        enc.changes.push(tf!("clothing.change.mips", before = t.entry.levels, after = enc.levels));
    }
    Ok(Some(enc))
}

/// Whether a change line is the crop line [`optimize_texture`] puts first (in any language:
/// compared with the text of the message before its first placeholder).
pub(super) fn is_crop_change(change: &str) -> bool {
    let template = i18n::t("clothing.change.texture_cropped");
    change.starts_with(&template[..template.find('{').unwrap_or(template.len())])
}

/// Unique pixel blocks (graphics offset → textures using them), or why the textures of the
/// file cannot be rewritten.
pub(super) fn texture_blocks(p: &Parsed) -> Result<BTreeMap<usize, Vec<usize>>, String> {
    let users: Vec<texres::BlockUser> = p
        .textures
        .iter()
        .map(|t| texres::BlockUser { name: &t.entry.name, data_off: t.data_off, ptr_field: t.entry.data_ptr_field(), name_field: t.entry.name_ptr_field() })
        .collect();
    texres::texture_blocks(&p.res, &users, &texres::raw_ranges(&p.res, &p.drawables))
}

pub(super) fn optimize_textures(p: &mut Parsed, cfg: &Config, info: &FileInfo, warnings: &mut Vec<String>) -> Result<Vec<String>> {
    let skin = info.skin;
    if p.textures.is_empty() && p.res.graphics.is_empty() {
        return Ok(Vec::new());
    }
    let blocks = match texture_blocks(p) {
        Ok(b) => b,
        // The UVs of the group are rewritten: leaving these textures whole would misplace them.
        Err(why) if info.crop.is_some() => bail!(tf!("clothing.error.crop_impossible", reason = why)),
        Err(why) => {
            warnings.push(why);
            return Ok(Vec::new());
        }
    };

    // Pixel blocks are independent: encode them in parallel, then apply in order.
    let blocks: Vec<(usize, Vec<usize>)> = blocks.into_iter().collect();
    let encoded: Vec<Option<plan::Encoded>> = blocks
        .par_iter()
        .map(|(off, users)| {
            let first = &p.textures[users[0]];
            let same_header = users.iter().all(|&u| {
                let e = &p.textures[u].entry;
                (e.width, e.height, e.format, e.levels) == (first.entry.width, first.entry.height, first.entry.format, first.entry.levels)
            });
            if !same_header {
                ensure!(info.crop.is_none(), tf!("clothing.error.shared_data", name = first.entry.name));
                return Ok(None);
            }
            let original = &p.res.graphics[*off..*off + first.data_len];
            optimize_texture(first, original, cfg, skin, info.crop.as_ref())
                .map_err(|e| anyhow::anyhow!(tf!("clothing.error.texture", name = first.entry.name, error = format!("{e:#}"))))
        })
        .collect::<Result<_>>()?;

    let mut changes = Vec::new();
    let mut new_blocks = Vec::with_capacity(blocks.len());
    let mut any_change = false;
    for ((off, users), encoded) in blocks.iter().zip(encoded) {
        let (off, users) = (*off, users);
        let first = &p.textures[users[0]];
        let refs: Vec<usize> = users.iter().map(|&u| p.textures[u].entry.data_ptr_field()).collect();
        match encoded {
            Some(enc) => {
                any_change = true;
                for &u in users {
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
                changes.push(tf!("clothing.change.texture", name = first.entry.name, changes = enc.changes.join(", ")));
                new_blocks.push(GfxBlock { data: enc.data, refs });
            }
            None => new_blocks.push(GfxBlock { data: p.res.graphics[off..off + first.data_len].to_vec(), refs }),
        }
    }

    if new_blocks.is_empty() {
        return Ok(Vec::new());
    }
    if !any_change {
        // A pure repack is still worth it when the page layout shrinks.
        let sizes: Vec<u64> = new_blocks.iter().map(|b| b.data.len() as u64).collect();
        let layout = pages::pack(&sizes, 128, false)?;
        if layout.size() >= p.res.graphics_flags.size() {
            return Ok(Vec::new());
        }
        changes.push(i18n::t("clothing.change.graphics_repacked").to_owned());
    }
    gfx::rebuild(&mut p.res, new_blocks)?;
    Ok(changes)
}
