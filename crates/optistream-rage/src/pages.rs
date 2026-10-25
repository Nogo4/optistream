//! Page packing for a resource segment.
//!
//! A segment is a sequence of pages of five possible sizes (base, 2x, 4x, 8x, 16x base),
//! laid out from largest to smallest. A block may never straddle two pages. Unlike
//! CodeWalker, which picks the base size from the smallest block (and so rounds a single
//! 34 MiB texture up to a 64 MiB page), every base shift is tried and the layout with the
//! smallest declared size wins.

use anyhow::{bail, Result};

use crate::rsc7::PageFlags;

pub const BLOCK_ALIGN: u64 = 16;
const MAX_COUNTS: [u32; 5] = [0x7F, 0x3F, 0xF, 0x3, 0x1];

#[derive(Clone, Debug)]
pub struct Layout {
    /// Offset of each input block within the segment, in input order.
    pub offsets: Vec<u64>,
    /// Page flags without the version nibble.
    pub flags: PageFlags,
}

impl Layout {
    pub fn size(&self) -> u64 {
        self.flags.size()
    }
}

fn align(v: u64) -> u64 {
    v.div_ceil(BLOCK_ALIGN) * BLOCK_ALIGN
}

struct Page {
    size_index: usize,
    used: u64,
}

/// Packs blocks into pages. `first_block_at_zero` keeps block 0 at offset 0 (the root
/// structure of a system segment must stay there).
pub fn pack(sizes: &[u64], max_pages: u32, first_block_at_zero: bool) -> Result<Layout> {
    if sizes.is_empty() {
        return Ok(Layout { offsets: Vec::new(), flags: PageFlags(0) });
    }

    let mut best: Option<(u64, u32, Layout)> = None;
    for shift in 0..16u32 {
        let Some(layout) = pack_with_shift(sizes, shift, first_block_at_zero) else {
            continue;
        };
        let count = layout.flags.page_count();
        if count > max_pages {
            continue;
        }
        let size = layout.size();
        let better = match &best {
            None => true,
            Some((s, c, _)) => size < *s || (size == *s && count < *c),
        };
        if better {
            best = Some((size, count, layout));
        }
    }
    match best {
        Some((_, _, layout)) => Ok(layout),
        None => bail!("unable to pack {} blocks into at most {max_pages} pages", sizes.len()),
    }
}

fn pack_with_shift(sizes: &[u64], shift: u32, first_block_at_zero: bool) -> Option<Layout> {
    let base = 0x2000u64 << shift;
    let page_size = |i: usize| base << i;

    let mut order: Vec<usize> = (0..sizes.len()).collect();
    let start = usize::from(first_block_at_zero);
    order[start..].sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));

    let mut pages: Vec<Page> = Vec::new();
    // (page index, offset within page) per block.
    let mut placement = vec![(0usize, 0u64); sizes.len()];

    for (n, &bi) in order.iter().enumerate() {
        let size = sizes[bi].max(1);
        let min_index = (0..5).find(|&i| page_size(i) >= size)?;
        if n == 0 && first_block_at_zero {
            // The root opens the largest page needed by any block so it lands at offset 0.
            let largest = sizes.iter().map(|&s| s.max(1)).max().unwrap();
            let idx = (0..5).find(|&i| page_size(i) >= largest)?;
            pages.push(Page { size_index: idx, used: size });
            placement[bi] = (0, 0);
            continue;
        }
        let mut placed = false;
        for idx in min_index..5 {
            if let Some((pi, page)) = pages
                .iter_mut()
                .enumerate()
                .find(|(_, p)| p.size_index == idx && align(p.used) + size <= page_size(idx))
            {
                let off = align(page.used);
                page.used = off + size;
                placement[bi] = (pi, off);
                placed = true;
                break;
            }
        }
        if !placed {
            pages.push(Page { size_index: min_index, used: size });
            placement[bi] = (pages.len() - 1, 0);
        }
    }

    let mut counts = [0u32; 5];
    for p in &pages {
        counts[p.size_index] += 1;
    }
    if counts.iter().zip(MAX_COUNTS).any(|(&c, m)| c > m) {
        return None;
    }

    // Pages are laid out from the largest size to the smallest, in creation order.
    let mut page_offsets = vec![0u64; pages.len()];
    let mut cursor = 0u64;
    for idx in (0..5).rev() {
        for (pi, p) in pages.iter().enumerate() {
            if p.size_index == idx {
                page_offsets[pi] = cursor;
                cursor += page_size(idx);
            }
        }
    }
    if first_block_at_zero && page_offsets[0] != 0 {
        return None;
    }

    let offsets = placement.iter().map(|&(pi, off)| page_offsets[pi] + off).collect();
    Some(Layout { offsets, flags: PageFlags::from_counts(shift, counts, 0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(sizes: &[u64], layout: &Layout) {
        let flags = layout.flags;
        let mut bounds = Vec::new();
        let mut cursor = 0;
        for (c, s) in flags.page_counts().iter().zip(flags.page_sizes()) {
            for _ in 0..*c {
                bounds.push((cursor, cursor + s));
                cursor += s;
            }
        }
        let mut spans: Vec<(u64, u64)> =
            sizes.iter().zip(&layout.offsets).map(|(&s, &o)| (o, o + s)).collect();
        for &(a, b) in &spans {
            assert_eq!(a % BLOCK_ALIGN, 0);
            assert!(bounds.iter().any(|&(p0, p1)| a >= p0 && b <= p1), "block straddles pages");
        }
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "blocks overlap");
        }
    }

    #[test]
    fn single_block_uses_smallest_fitting_page() {
        // 2048x2048 DXT1 with a full mip chain.
        let sizes = [2_796_192u64];
        let l = pack(&sizes, 128, false).unwrap();
        check(&sizes, &l);
        assert_eq!(l.size(), 4 * 1024 * 1024);
    }

    #[test]
    fn small_blocks_fill_the_large_page() {
        let sizes = [5_000_000u64, 100_000, 100_000, 50_000];
        let l = pack(&sizes, 128, false).unwrap();
        check(&sizes, &l);
        assert_eq!(l.size(), 8 * 1024 * 1024);
    }

    #[test]
    fn many_blocks() {
        let sizes: Vec<u64> = (0..300).map(|i| 1000 + (i * 7919) % 200_000).collect();
        let l = pack(&sizes, 128, false).unwrap();
        check(&sizes, &l);
        let total: u64 = sizes.iter().sum();
        assert!(l.size() >= total);
    }

    #[test]
    fn root_stays_first() {
        let sizes = [64u64, 500_000, 3000, 70_000];
        let l = pack(&sizes, 128, true).unwrap();
        check(&sizes, &l);
        assert_eq!(l.offsets[0], 0);
    }
}
