//! Reading a clothing YDD/YTD: drawables, textures and the role of each texture.

use std::collections::HashSet;

use anyhow::{ensure, Result};
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::{drawable, Resource};
use optistream_texture::{resource as texres, TextureRole};

use crate::process::textures::texture_blocks;
use crate::process::{FileInfo, Kind, Parsed, TexItem};
use crate::shaders::{self, Usage};

pub fn parse(kind: Kind, bytes: &[u8], info: &FileInfo) -> Result<Parsed> {
    let res = Resource::parse(bytes)?;
    let mut drawables = Vec::new();
    let mut textures = Vec::new();
    match kind {
        Kind::Ytd => {
            for entry in rtex::read_dictionary(&res, 0)? {
                // `_n` / `_s` are also variation letters (`p_head_diff_000_n`, `bagdad_n`): a
                // variation diffuse taken for a normal map was filtered as one and lost its alpha.
                let usage = match shaders::usage_from_name(&entry.name) {
                    Usage::Role(_) if info.variation => Usage::Role(TextureRole::Diffuse),
                    other => other,
                };
                textures.push(tex_item(&res, entry, usage, info.alpha_mask));
            }
        }
        Kind::Ydd => {
            drawables = drawable::read_dictionary(&res, 0)?;
            let mut seen = HashSet::new();
            for d in &drawables {
                let Some(td) = d.texture_dict else { continue };
                if !seen.insert(td) {
                    continue;
                }
                let usages = shaders::usages_from_shaders(&d.shaders);
                let masked = shaders::alpha_masked_diffuses(&d.shaders);
                for entry in rtex::read_dictionary(&res, td)? {
                    let usage = usages
                        .get(&entry.name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or_else(|| shaders::usage_from_name(&entry.name));
                    let alpha_mask = masked.contains(&entry.name.to_ascii_lowercase());
                    textures.push(tex_item(&res, entry, usage, alpha_mask));
                }
            }
        }
    }
    Ok(Parsed { res, kind, drawables, textures })
}

/// Whether every diffuse of a YDD is drawn by alpha shaders only (what its variations inherit).
pub fn drawable_alpha_mask(bytes: &[u8]) -> Result<bool> {
    let res = Resource::parse(bytes)?;
    let drawables = drawable::read_dictionary(&res, 0)?;
    Ok(shaders::all_diffuse_shaders_use_alpha(drawables.iter().flat_map(|d| &d.shaders)))
}

fn tex_item(res: &Resource, entry: TextureEntry, usage: Usage, alpha_mask: bool) -> TexItem {
    let (data_off, data_len) = match texres::locate(res, &entry) {
        Some((off, len)) => (Some(off), len),
        None => (None, 0),
    };
    TexItem { entry, usage, alpha_mask, data_off, data_len }
}

pub(super) fn role_of(usage: Usage) -> Option<TextureRole> {
    match usage {
        Usage::Role(r) => Some(r),
        Usage::Protected => None,
    }
}

/// Pixel data of a texture (all stored levels), if it lives in the graphics segment.
pub(super) fn tex_data<'a>(res: &'a Resource, t: &TexItem) -> Option<&'a [u8]> {
    t.data_off.map(|off| &res.graphics[off..off + t.data_len])
}

/// Whether every texture of the file can be cropped: a group is only planned when all of
/// its members can be rewritten, otherwise UVs and textures would disagree.
/// Returns the parsed file, for the planning that follows.
pub fn crop_ready(kind: Kind, bytes: &[u8]) -> Result<Parsed> {
    let p = parse(kind, bytes, &FileInfo::default())?;
    let blocks = texture_blocks(&p).map_err(anyhow::Error::msg)?;
    for users in blocks.values() {
        let e = &p.textures[users[0]].entry;
        ensure!(
            users.iter().all(|&u| {
                let o = &p.textures[u].entry;
                (o.width, o.height, o.format, o.levels) == (e.width, e.height, e.format, e.levels)
            }),
            "texture {} : données partagées par des en-têtes différents",
            e.name
        );
    }
    Ok(p)
}
