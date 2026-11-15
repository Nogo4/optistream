//! Lossless index reordering: vertex cache, overdraw, degenerate and duplicate removal.

use std::collections::HashSet;

use anyhow::{bail, Result};

use crate::vertex::positions;
use crate::{ACMR_CACHE_SIZE, GeometryData, OVERDRAW_THRESHOLD, Reordered};

fn acmr(indices: &[u32], vertex_count: usize) -> f32 {
    if indices.is_empty() {
        return 0.0;
    }
    meshopt::analyze_vertex_cache(indices, vertex_count, ACMR_CACHE_SIZE, 0, 0).acmr
}

/// Average cache miss ratio of a geometry as stored (0 when it cannot be measured).
pub fn index_acmr(geom: &GeometryData) -> f32 {
    let vcount = geom.vertices.len() / geom.stride.max(1);
    if geom.indices.iter().any(|&i| i as usize >= vcount) {
        return 0.0;
    }
    let idx: Vec<u32> = geom.indices.iter().map(|&i| i as u32).collect();
    acmr(&idx[..idx.len() / 3 * 3], vcount)
}

/// Drops triangles with a repeated index and, if `dedup`, exact duplicates (same vertices
/// in the same winding: double-sided faces, wound the other way, are kept).
fn clean_triangles(indices: &[u32], dedup: bool) -> Vec<u32> {
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut out = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (t[0], t[1], t[2]);
        if a == b || b == c || a == c {
            continue;
        }
        if dedup {
            // Rotate so that the smallest index comes first, keeping the winding.
            let key = if a < b && a < c {
                [a, b, c]
            } else if b < c {
                [b, c, a]
            } else {
                [c, a, b]
            };
            if !seen.insert(key) {
                continue;
            }
        }
        out.extend_from_slice(t);
    }
    out
}

/// Improves the index order of a geometry: vertex cache, then overdraw, after removing
/// degenerate and exactly duplicated triangles. Only the index buffer changes.
///
/// With `order_sensitive` (alpha blended or unknown shaders, where the draw order of the
/// triangles defines the blending), triangles are neither reordered nor deduplicated
/// (a duplicate blends twice): only degenerate ones are dropped.
///
/// Returns `None` when there is nothing to remove and the ACMR gain is under 5 %.
pub fn reorder_indices(geom: &GeometryData, order_sensitive: bool) -> Result<Option<Reordered>> {
    if geom.decl.is_cloth() {
        bail!("cloth geometry must not be modified");
    }
    let vcount = geom.vertices.len() / geom.stride;
    let n = geom.indices.len();
    if vcount == 0 || n < 3 || n % 3 != 0 || geom.indices.iter().any(|&i| i as usize >= vcount) {
        return Ok(None);
    }
    let src: Vec<u32> = geom.indices.iter().map(|&i| i as u32).collect();
    let mut idx = clean_triangles(&src, !order_sensitive);
    if idx.is_empty() {
        // Never leave a geometry without triangles.
        return Ok(None);
    }
    let removed_triangles = (src.len() - idx.len()) / 3;
    let acmr_before = acmr(&src, vcount);
    if !order_sensitive {
        idx = meshopt::optimize_vertex_cache(&idx, vcount);
        if let Some(pos) = positions(geom) {
            meshopt::optimize_overdraw_in_place_decoder(&mut idx, &pos, OVERDRAW_THRESHOLD);
        }
    }
    let acmr_after = acmr(&idx, vcount);
    if removed_triangles == 0 && acmr_after > acmr_before * 0.95 {
        return Ok(None);
    }
    Ok(Some(Reordered {
        indices: idx.iter().map(|&i| i as u16).collect(),
        removed_triangles,
        acmr_before,
        acmr_after,
    }))
}

#[cfg(test)]
mod tests {
    use optistream_rage::drawable::{component, semantic, VertexDecl};

    use super::*;

    /// Quads of an `(n + 1)` x `(n + 1)` vertex grid, row by row.
    fn grid_indices(n: u32) -> Vec<u32> {
        let w = n + 1;
        let mut idx = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                idx.extend_from_slice(&[a, a + 1, a + w, a + 1, a + w + 1, a + w]);
            }
        }
        idx
    }

    fn pos_decl() -> VertexDecl {
        VertexDecl { flags: 1 << semantic::POSITION, stride: 12, count: 1, types: component::FLOAT3 as u64 }
    }

    #[test]
    fn acmr_is_zero_without_triangles_and_bounded_otherwise() {
        assert_eq!(acmr(&[], 10), 0.0);
        // Triangle soup: every vertex is used once, so every reference misses.
        let soup: Vec<u32> = (0..30).collect();
        assert!((acmr(&soup, 30) - 3.0).abs() < 1e-6);
        // A shared-vertex grid listed row by row reuses vertices: well under 3, never under 0.5.
        let a = acmr(&grid_indices(16), 17 * 17);
        assert!(a > 0.5 && a < 3.0, "{a}");
        // The same triangle drawn again only hits the cache.
        assert!((acmr(&[0, 1, 2, 0, 1, 2], 3) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn clean_triangles_drops_every_degenerate_pattern() {
        let idx = [0, 0, 1, 2, 3, 3, 4, 5, 4, 6, 7, 8];
        assert_eq!(clean_triangles(&idx, false), [6, 7, 8]);
        assert_eq!(clean_triangles(&idx, true), [6, 7, 8]);
    }

    #[test]
    fn clean_triangles_dedups_rotations_only_when_asked() {
        // Original, its two rotations, the back face and an unrelated triangle.
        let idx = [3, 1, 2, 1, 2, 3, 2, 3, 1, 3, 2, 1, 4, 5, 6];
        assert_eq!(clean_triangles(&idx, true), [3, 1, 2, 3, 2, 1, 4, 5, 6]);
        assert_eq!(clean_triangles(&idx, false), idx);
    }

    #[test]
    fn clean_triangles_keeps_first_occurrence_and_order() {
        let idx = [7, 8, 9, 1, 2, 3, 8, 9, 7, 4, 5, 6, 2, 3, 1];
        assert_eq!(clean_triangles(&idx, true), [7, 8, 9, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn clean_triangles_ignores_a_trailing_partial_triangle() {
        assert_eq!(clean_triangles(&[0, 1, 2, 3, 4], true), [0, 1, 2]);
    }

    #[test]
    fn index_acmr_matches_acmr_and_rejects_invalid_buffers() {
        let v = vec![0u8; 4 * 4 * 12];
        let idx: Vec<u16> = grid_indices(3).iter().map(|&i| i as u16).collect();
        let g = GeometryData { decl: pos_decl(), stride: 12, vertices: &v, indices: &idx };
        let expected = acmr(&grid_indices(3), 16);
        assert_eq!(index_acmr(&g), expected);
        // A trailing partial triangle is ignored.
        let mut longer = idx.clone();
        longer.push(0);
        let g = GeometryData { indices: &longer, ..g };
        assert_eq!(index_acmr(&g), expected);
        // An index past the vertex buffer: not measurable.
        let bad = [0u16, 1, 16];
        let g = GeometryData { indices: &bad, ..g };
        assert_eq!(index_acmr(&g), 0.0);
        // A zero stride does not divide by zero.
        let g = GeometryData { stride: 0, indices: &idx, ..g };
        assert!(index_acmr(&g).is_finite());
    }

    #[test]
    fn reorder_leaves_malformed_or_fully_degenerate_buffers_alone() {
        let v = vec![0u8; 16 * 12];
        let decl = pos_decl();
        let cases: [&[u16]; 4] = [
            &[0, 1],          // fewer than 3 indices
            &[0, 1, 2, 3],    // not a multiple of 3
            &[0, 1, 16],      // index out of range
            &[0, 0, 1, 2, 2, 2], // only degenerates: never leave the geometry empty
        ];
        for idx in cases {
            let g = GeometryData { decl, stride: 12, vertices: &v, indices: idx };
            assert!(reorder_indices(&g, false).unwrap().is_none(), "{idx:?}");
        }
        let g = GeometryData { decl, stride: 12, vertices: &[], indices: &[0, 1, 2] };
        assert!(reorder_indices(&g, false).unwrap().is_none());
    }

    #[test]
    fn reorder_never_grows_and_keeps_indices_in_range() {
        let n = 8u32;
        let vcount = ((n + 1) * (n + 1)) as usize;
        let mut v = Vec::new();
        for i in 0..vcount {
            for f in [(i % 9) as f32, (i / 9) as f32, 0.0] {
                v.extend_from_slice(&f.to_le_bytes());
            }
        }
        // Reverse the triangle order so that the cache gain is clear, then add a degenerate.
        let mut tris: Vec<u16> = grid_indices(n).as_chunks::<3>().0.iter().rev().flatten().map(|&i| i as u16).collect();
        tris.extend_from_slice(&[4, 4, 5]);
        let g = GeometryData { decl: pos_decl(), stride: 12, vertices: &v, indices: &tris };
        // A degenerate triangle was present: the result must not be None.
        let r = reorder_indices(&g, false).unwrap().expect("degenerate removed");
        assert_eq!(r.removed_triangles, 1);
        assert_eq!(r.indices.len(), tris.len() - 3);
        assert!(r.indices.iter().all(|&i| (i as usize) < vcount));
        assert!(r.acmr_after <= r.acmr_before * 1.05);
    }
}
