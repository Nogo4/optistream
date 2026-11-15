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

/// Declared graphics size (what FiveM reports as physical memory) that [`rebuild`] would
/// give to blocks of these sizes.
pub fn packed_size(res: &Resource, sizes: &[u64]) -> Result<u64> {
    let (_, max_pages) = page_limit(res)?;
    Ok(pages::pack(sizes, max_pages, false)?.size())
}

/// `ResourcePagesInfo` offset to update (if any) and the most graphics pages allowed.
fn page_limit(res: &Resource) -> Result<(Option<usize>, u32)> {
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
    Ok((info, max_pages))
}

/// Replaces the graphics segment with `blocks`, re-laid out with the tightest page layout.
pub fn rebuild(res: &mut Resource, blocks: Vec<GfxBlock>) -> Result<()> {
    let (info, max_pages) = page_limit(res)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsc7::SYSTEM_BASE;

    /// Resource with a 256-byte system segment (no ResourcePagesInfo) and `gfx_len` bytes of graphics.
    fn resource(gfx_len: usize) -> Resource {
        let sys = pages::pack(&[256], 128, true).unwrap();
        let gfx = pages::pack(&[gfx_len as u64], 128, false).unwrap();
        Resource {
            version: 13,
            system_flags: sys.flags,
            graphics_flags: PageFlags(gfx.flags.0 | (0xA << 28)),
            system: vec![0; sys.size() as usize],
            graphics: vec![0; gfx.size() as usize],
        }
    }

    #[test]
    fn unknown_refs_skip_known_fields_raw_ranges_and_unaligned_values() {
        let mut res = resource(0x100);
        res.set_u64(0x10, GRAPHICS_BASE + 0x20); // known field
        res.set_u64(0x18, GRAPHICS_BASE + 0x30); // unknown
        res.set_u64(0x20, GRAPHICS_BASE + 0x31); // not 16-byte aligned: not a block pointer
        res.set_u64(0x28, SYSTEM_BASE + 0x40); // system pointer
        res.set_u64(0x40, GRAPHICS_BASE); // inside a raw vertex range
        res.set_u64(0x60, GRAPHICS_BASE + res.graphics.len() as u64); // past the segment
        let known: HashSet<usize> = [0x10].into();
        assert_eq!(find_unknown_graphics_refs(&res, &known, &[(0x3C, 0x10)]), [0x18]);
        assert_eq!(find_unknown_graphics_refs(&res, &HashSet::new(), &[]), [0x10, 0x18, 0x40]);
    }

    #[test]
    fn packed_size_is_what_rebuild_declares() {
        let mut res = resource(0x4000);
        let blocks: Vec<GfxBlock> = [0x3000usize, 0x800, 0x10]
            .iter()
            .enumerate()
            .map(|(i, &len)| GfxBlock { data: vec![i as u8 + 1; len], refs: vec![0x10 + i * 8] })
            .collect();
        let sizes: Vec<u64> = blocks.iter().map(|b| b.data.len() as u64).collect();
        let expected = packed_size(&res, &sizes).unwrap();
        rebuild(&mut res, blocks).unwrap();
        assert_eq!(res.graphics_flags.size(), expected);
        assert_eq!(res.graphics.len() as u64, expected);
        // The version nibble survives the new layout.
        assert_eq!(res.graphics_flags.version_nibble(), 0xA);
    }

    #[test]
    fn rebuild_patches_every_reference() {
        let mut res = resource(0x100);
        let blocks = vec![
            GfxBlock { data: vec![0xAA; 0x40], refs: vec![0x10, 0x18] },
            GfxBlock { data: vec![0xBB; 0x20], refs: vec![0x20] },
        ];
        rebuild(&mut res, blocks).unwrap();
        let a = res.u64_at(0x10).unwrap();
        assert_eq!(res.u64_at(0x18).unwrap(), a);
        let b = res.u64_at(0x20).unwrap();
        assert_ne!(a, b);
        assert_eq!(res.slice(a, 0x40).unwrap(), &[0xAA; 0x40][..]);
        assert_eq!(res.slice(b, 0x20).unwrap(), &[0xBB; 0x20][..]);
        // Every pointer is now a known field: nothing unknown is left.
        let known: HashSet<usize> = [0x10, 0x18, 0x20].into();
        assert!(find_unknown_graphics_refs(&res, &known, &[]).is_empty());
    }

    #[test]
    fn rebuild_updates_the_page_count_in_pages_info() {
        let mut res = resource(0x100);
        // ResourcePagesInfo at 0x80: 1 system page reserved + 7 graphics slots.
        res.set_u64(0x08, SYSTEM_BASE + 0x80);
        res.set_u8(0x88, 1);
        res.set_u8(0x89, 7);
        let blocks = vec![GfxBlock { data: vec![1; 0x2000], refs: vec![0x10] }, GfxBlock { data: vec![2; 0x2000], refs: vec![0x18] }];
        rebuild(&mut res, blocks).unwrap();
        assert_eq!(res.u8_at(0x89).unwrap() as u32, res.graphics_flags.page_count());
    }
}
