//! Full re-layout of the graphics segment.
//!
//! The graphics segment of ped resources only holds raw pixel data blocks, referenced
//! from structures in the system segment. Rebuilding it means: pack the new blocks into
//! pages, rewrite every referencing pointer, recompute the graphics page flags and the
//! page count stored in `ResourcePagesInfo`.

use std::collections::HashSet;

use anyhow::{ensure, Result};

use crate::pages;
use crate::rsc7::{PageFlags, Resource, GRAPHICS_BASE};

/// A graphics block and the system-segment offsets of the u64 pointer fields referencing it.
pub struct GfxBlock {
    pub data: Vec<u8>,
    pub refs: Vec<usize>,
}

/// Scans 8-byte aligned values of the system segment for pointers into the graphics
/// segment that are not among the known pointer fields. Raw data ranges (vertex/index
/// buffers) are excluded since they are not structures. Graphics blocks are always
/// 16-byte aligned, which filters out string bytes that happen to look like pointers.
pub fn find_unknown_graphics_refs(
    res: &Resource,
    known_fields: &HashSet<usize>,
    raw_ranges: &[(usize, usize)],
) -> Vec<usize> {
    let gfx_len = res.graphics.len() as u64;
    let sys = &res.system;
    let mut found = Vec::new();
    let mut off = 0;
    while off + 8 <= sys.len() {
        if let Some(&(s, l)) = raw_ranges.iter().find(|&&(s, l)| off >= s && off < s + l) {
            off = (s + l).div_ceil(8) * 8;
            continue;
        }
        let v = u64::from_le_bytes(sys[off..off + 8].try_into().unwrap());
        if v >= GRAPHICS_BASE && v < GRAPHICS_BASE + gfx_len && v % 16 == 0 && !known_fields.contains(&off) {
            found.push(off);
        }
        off += 8;
    }
    found
}

/// Replaces the graphics segment with `blocks`, re-laid out with the tightest page layout.
pub fn rebuild(res: &mut Resource, blocks: Vec<GfxBlock>) -> Result<()> {
    let sys_pages = res.system_flags.page_count();
    // The game fills one 8-byte slot per page in ResourcePagesInfo: never need more slots
    // than the file reserved.
    let mut info = res.pages_info_offset()?;
    let reserved = match info {
        Some(o) => res.u8_at(o + 8)? as u32 + res.u8_at(o + 9)? as u32,
        None => 128,
    };
    let max_pages = if reserved == 0 {
        // Some exporters leave the counts at 0 (and reserve no slots); the game loads them
        // anyway. Keep it that way and never use more pages than the original layout.
        info = None;
        res.graphics_flags.page_count().max(1)
    } else {
        ensure!(reserved >= sys_pages, "ResourcePagesInfo smaller than the system page count");
        (128 - sys_pages.min(128)).min(reserved - sys_pages)
    };

    let sizes: Vec<u64> = blocks.iter().map(|b| b.data.len() as u64).collect();
    let layout = pages::pack(&sizes, max_pages, false)?;

    let mut graphics = vec![0u8; layout.size() as usize];
    for (block, &off) in blocks.iter().zip(&layout.offsets) {
        let off = off as usize;
        graphics[off..off + block.data.len()].copy_from_slice(&block.data);
        for &field in &block.refs {
            res.set_u64(field, GRAPHICS_BASE + off as u64);
        }
    }
    res.graphics = graphics;
    let version = res.graphics_flags.version_nibble();
    res.graphics_flags = PageFlags(layout.flags.0 | (version << 28));
    if let Some(o) = info {
        res.set_u8(o + 9, layout.flags.page_count() as u8);
    }
    Ok(())
}
