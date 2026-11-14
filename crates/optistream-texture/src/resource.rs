//! Locating texture pixel data inside a resource, shared by every method that rewrites
//! textures (YTD roots and dictionaries embedded in drawables).

use std::collections::{BTreeMap, HashSet};

use optistream_rage::drawable::DrawableInfo;
use optistream_rage::gfx;
use optistream_rage::texture::TextureEntry;
use optistream_rage::{Resource, Segment};

use crate::format::{chain_size, game_chain_size};

/// Pixel data location of a texture in the graphics segment: `(offset, length)`, or `None`
/// when the data lives elsewhere (such a texture cannot be rewritten).
pub fn locate(res: &Resource, entry: &TextureEntry) -> Option<(usize, usize)> {
    match res.resolve(entry.data_ptr) {
        Some((Segment::Graphics, off)) => {
            let exact = chain_size(entry.format, entry.width as u32, entry.height as u32, entry.levels.max(1) as u32);
            let game = game_chain_size(entry.stride as u32, entry.height as u32, entry.levels as u32);
            Some((off, exact.max(game).min(res.graphics.len() - off)))
        }
        _ => None,
    }
}

/// System-segment ranges holding raw vertex/index data (not structures) of the drawables.
pub fn raw_ranges(res: &Resource, drawables: &[DrawableInfo]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for d in drawables {
        for g in d.geometries() {
            let vlen = g.vertex_count as usize * g.vertex_stride as usize;
            for ptr in [g.vertex_ptr, g.vertex_ptr2] {
                if let Some((Segment::System, off)) = res.resolve(ptr) {
                    out.push((off, vlen));
                }
            }
            if let Some((Segment::System, off)) = res.resolve(g.index_ptr) {
                out.push((off, g.index_count as usize * 2));
            }
        }
    }
    out.sort_unstable();
    out
}

/// One texture as seen by [`texture_blocks`].
pub struct BlockUser<'a> {
    pub name: &'a str,
    pub data_off: Option<usize>,
    /// System offset of the texture's data pointer field.
    pub ptr_field: usize,
}

/// Unique pixel blocks (graphics offset → indices of the textures using them), or why the
/// textures of the file cannot be rewritten.
///
/// Graphics data is only reachable through pointers: once every pointer into the segment
/// is a known texture data field, unreferenced bytes (extra mip levels beyond the declared
/// count, tool padding) are dead and can be dropped. Any unknown pointer leaves the file's
/// textures untouched.
pub fn texture_blocks(
    res: &Resource,
    users: &[BlockUser],
    raw: &[(usize, usize)],
) -> Result<BTreeMap<usize, Vec<usize>>, String> {
    let mut blocks: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, u) in users.iter().enumerate() {
        match u.data_off {
            Some(off) => blocks.entry(off).or_default().push(i),
            None => return Err(format!("texture {} : données hors du segment graphique, ignorée", u.name)),
        }
    }
    let fields: HashSet<usize> = users.iter().map(|u| u.ptr_field).collect();
    let unknown = gfx::find_unknown_graphics_refs(res, &fields, raw);
    if !unknown.is_empty() {
        let list: Vec<String> =
            unknown.iter().take(3).map(|&o| format!("0x{o:X}→0x{:X}", res.u64_at(o).unwrap_or(0))).collect();
        return Err(format!(
            "textures non modifiées : {} référence(s) graphique(s) inconnue(s) ({})",
            unknown.len(),
            list.join(", ")
        ));
    }
    Ok(blocks)
}
