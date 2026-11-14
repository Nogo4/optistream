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
