use meshopt::SimplifyVertex;
use optistream_rage::drawable::{component, semantic};

use crate::simplify::build_locks;
use crate::vertex::read_f32;
use super::*;

/// Declaration with position (Float3) + UV (Float2).
fn decl_pos_uv() -> VertexDecl {
    VertexDecl {
        flags: (1 << semantic::POSITION) | (1 << semantic::TEXCOORD0),
        stride: 20,
        count: 2,
        types: (component::FLOAT3 as u64) | ((component::FLOAT2 as u64) << (semantic::TEXCOORD0 * 4)),
    }
}

/// Declaration with position, blend weights, blend indices (Colour) and UV (Float2).
fn decl_skinned() -> VertexDecl {
    VertexDecl {
        flags: (1 << semantic::POSITION)
            | (1 << semantic::BLEND_WEIGHTS)
            | (1 << semantic::BLEND_INDICES)
            | (1 << semantic::TEXCOORD0),
        stride: 28,
        count: 4,
        types: (component::FLOAT3 as u64)
            | ((component::COLOUR as u64) << (semantic::BLEND_WEIGHTS * 4))
            | ((component::COLOUR as u64) << (semantic::BLEND_INDICES * 4))
            | ((component::FLOAT2 as u64) << (semantic::TEXCOORD0 * 4)),
    }
}

fn push_f32s(v: &mut Vec<u8>, fs: &[f32]) {
    for f in fs {
        v.extend_from_slice(&f.to_le_bytes());
    }
}

/// Quads of a `(n + 1)` x `(n + 1)` vertex grid starting at vertex `base`.
fn grid_indices(n: usize, base: u16) -> Vec<u16> {
    let mut idx = Vec::new();
    let w = (n + 1) as u16;
    for y in 0..n as u16 {
        for x in 0..n as u16 {
            let a = base + y * w + x;
            idx.extend_from_slice(&[a, a + 1, a + w, a + 1, a + w + 1, a + w]);
        }
    }
    idx
}

/// A flat grid of `n` x `n` quads with position (Float3) + UV (Float2), shifted by `dx`.
fn grid_at(n: usize, dx: f32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            push_f32s(&mut v, &[fx + dx, fy, 0.0, fx, fy]);
        }
    }
    v
}

fn grid(n: usize) -> (VertexDecl, Vec<u8>, Vec<u16>) {
    (decl_pos_uv(), grid_at(n, 0.0), grid_indices(n, 0))
}

/// Triangle rotated so that its smallest index comes first (winding kept).
fn canonical(t: &[u16]) -> [u16; 3] {
    let m = (0..3).min_by_key(|&k| t[k]).unwrap();
    [t[m], t[(m + 1) % 3], t[(m + 2) % 3]]
}

fn shuffled(idx: &[u16], seed: u64) -> Vec<u16> {
    let mut tris: Vec<[u16; 3]> = idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut s = seed;
    for i in (1..tris.len()).rev() {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        tris.swap(i, (s >> 33) as usize % (i + 1));
    }
    tris.concat()
}

#[test]
fn flat_grid_collapses_and_keeps_vertices_verbatim() {
    let (decl, v, idx) = grid(32);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    let d = decimate(&geom, 200, 0.01, false).unwrap().unwrap();
    assert!(d.indices.len() / 3 <= 2048 / 2);
    // Every output vertex is a byte-exact copy of an input vertex.
    for out in d.vertices.chunks_exact(20) {
        assert!(v.chunks_exact(20).any(|inp| inp == out));
    }
    assert!(d.indices.iter().all(|&i| (i as usize) < d.vertex_count(20)));
}

#[test]
fn cloth_is_refused() {
    let (mut decl, v, idx) = grid(4);
    decl.flags |= 0x8000_0000;
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    assert!(decimate(&geom, 4, 0.01, false).is_err());
    assert!(reorder_indices(&geom, false).is_err());
}

#[test]
fn shuffled_grid_gets_a_better_acmr_and_keeps_every_triangle() {
    let (decl, v, idx) = grid(32);
    let idx = shuffled(&idx, 7);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    let r = reorder_indices(&geom, false).unwrap().unwrap();
    assert!(r.acmr_after < r.acmr_before, "{} -> {}", r.acmr_after, r.acmr_before);
    assert_eq!(r.removed_triangles, 0);
    let mut before: Vec<[u16; 3]> = idx.chunks_exact(3).map(canonical).collect();
    let mut after: Vec<[u16; 3]> = r.indices.chunks_exact(3).map(canonical).collect();
    before.sort_unstable();
    after.sort_unstable();
    assert_eq!(before, after);
}

#[test]
fn degenerate_triangle_is_removed() {
    let (decl, v, mut idx) = grid(4);
    idx.extend_from_slice(&[3, 3, 7]);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    let r = reorder_indices(&geom, false).unwrap().unwrap();
    assert_eq!(r.removed_triangles, 1);
    assert_eq!(r.indices.len(), idx.len() - 3);
    assert!(r.indices.chunks_exact(3).all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]));
}

#[test]
fn exact_duplicate_is_removed_but_opposite_winding_is_kept() {
    let (decl, v, mut idx) = grid(4);
    let first = [idx[0], idx[1], idx[2]];
    // Same triangle, rotated (exact duplicate), then the back face (opposite winding).
    idx.extend_from_slice(&[first[1], first[2], first[0]]);
    idx.extend_from_slice(&[first[0], first[2], first[1]]);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    let r = reorder_indices(&geom, false).unwrap().unwrap();
    assert_eq!(r.removed_triangles, 1);
    let tris: Vec<[u16; 3]> = r.indices.chunks_exact(3).map(canonical).collect();
    assert_eq!(tris.iter().filter(|&&t| t == canonical(&first)).count(), 1);
    assert!(tris.contains(&canonical(&[first[0], first[2], first[1]])));
}

#[test]
fn order_sensitive_only_drops_degenerates() {
    let (decl, v, idx) = grid(8);
    let mut input = shuffled(&idx, 3);
    let dup = [input[0], input[1], input[2]];
    input.extend_from_slice(&dup);
    input.extend_from_slice(&[5, 5, 9]);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &input };
    let r = reorder_indices(&geom, true).unwrap().unwrap();
    assert_eq!(r.removed_triangles, 1);
    // Same triangles in the same order (the duplicate blends twice: kept).
    assert_eq!(r.indices, input[..input.len() - 3]);
    // Nothing to drop: left alone.
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &input[..input.len() - 3] };
    assert!(reorder_indices(&geom, true).unwrap().is_none());
}

#[test]
fn order_sensitive_decimation_keeps_the_triangle_order() {
    // Two separate grids; the triangles of the second one (higher vertex indices) are
    // listed first. Their relative order must survive the decimation.
    let n = 16;
    let mut v = grid_at(n, 0.0);
    v.extend(grid_at(n, 2.0));
    let second = ((n + 1) * (n + 1)) as u16;
    let mut idx = grid_indices(n, second);
    idx.extend(grid_indices(n, 0));
    let geom = GeometryData { decl: decl_pos_uv(), stride: 20, vertices: &v, indices: &idx };
    let d = decimate(&geom, 100, 0.01, true).unwrap().unwrap();
    let in_second: Vec<bool> = d
        .indices
        .chunks_exact(3)
        .map(|t| read_f32(&d.vertices, t[0] as usize * 20) > 1.5)
        .collect();
    let first_of_first = in_second.iter().position(|&s| !s).unwrap();
    assert!(in_second[..first_of_first].iter().all(|&s| s));
    assert!(in_second[first_of_first..].iter().all(|&s| !s));
}

/// Skinned grid: columns left of `split` bound to bone 0, the others to bone 1.
fn skinned_grid(n: usize, split: usize, height: impl Fn(f32, f32) -> f32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            push_f32s(&mut v, &[fx, fy, height(fx, fy)]);
            v.extend_from_slice(&[255, 0, 0, 0]);
            v.extend_from_slice(&[if x < split { 0 } else { 1 }, 0, 0, 0]);
            push_f32s(&mut v, &[fx, fy]);
        }
    }
    v
}

#[test]
fn coincident_vertices_with_different_weights_are_protected() {
    let mut v = skinned_grid(2, 3, |_, _| 0.0);
    let idx = grid_indices(2, 0);
    // Duplicate vertex 4 (centre) with other weights: both copies must be protected.
    let mut copy = v[4 * 28..5 * 28].to_vec();
    copy[12..16].copy_from_slice(&[128, 127, 0, 0]);
    v.extend(copy);
    let geom = GeometryData { decl: decl_skinned(), stride: 28, vertices: &v, indices: &idx };
    let locks = build_locks(&geom, &idx.iter().map(|&i| i as u32).collect::<Vec<_>>());
    assert!(locks[4].contains(SimplifyVertex::Protect));
    assert!(locks[9].contains(SimplifyVertex::Protect));
    assert!(!locks[0].contains(SimplifyVertex::Protect));
}

#[test]
fn bone_boundary_is_kept() {
    let n = 16;
    let split = 8;
    let v = skinned_grid(n, split, |_, _| 0.0);
    let idx = grid_indices(n, 0);
    let geom = GeometryData { decl: decl_skinned(), stride: 28, vertices: &v, indices: &idx };
    let welded: Vec<u32> = idx.iter().map(|&i| i as u32).collect();
    let locks = build_locks(&geom, &welded);
    let w = n + 1;
    for y in 0..=n {
        assert!(locks[y * w + split - 1].contains(SimplifyVertex::Priority));
        assert!(locks[y * w + split].contains(SimplifyVertex::Priority));
        assert!(!locks[y * w].contains(SimplifyVertex::Priority));
    }
    let d = decimate(&geom, 50, 0.01, false).unwrap().unwrap();
    // Triangles mixing both bones only use vertices of the two boundary columns.
    let column = |i: u16| {
        let x = read_f32(&d.vertices, i as usize * 28);
        (x * n as f32).round() as usize
    };
    let bone = |i: u16| d.vertices[i as usize * 28 + 16];
    for t in d.indices.chunks_exact(3) {
        let mixed = t.iter().any(|&i| bone(i) == 0) && t.iter().any(|&i| bone(i) == 1);
        if mixed {
            assert!(t.iter().all(|&i| column(i) == split - 1 || column(i) == split), "{t:?}");
        }
    }
}

#[test]
fn fold_line_is_kept() {
    // A sheet folded onto itself (collar, hem): the lower half faces +Z at z = 0, the
    // upper half comes back above it facing -Z, joined along the fold line y = 1.
    let n = 12;
    let gap = 0.0005;
    let w = n + 1;
    let mut v = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            push_f32s(&mut v, &[fx, fy, 0.0, fx, fy]);
        }
    }
    // Back sheet rows n-1 .. 0 (the fold row n is shared).
    for y in 0..n {
        for x in 0..=n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            push_f32s(&mut v, &[fx, fy, gap, fx, fy]);
        }
    }
    let back = |x: usize, y: usize| -> u16 { if y == n { (n * w + x) as u16 } else { (w * w + y * w + x) as u16 } };
    let mut idx = grid_indices(n, 0);
    for y in 0..n {
        for x in 0..n {
            let (a, b, c, d) = (back(x, y), back(x + 1, y), back(x, y + 1), back(x + 1, y + 1));
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let geom = GeometryData { decl: decl_pos_uv(), stride: 20, vertices: &v, indices: &idx };
    let d = decimate(&geom, 40, 0.05, false).unwrap().unwrap();
    // The two halves still only meet on the fold line: no triangle joins a vertex of
    // the back sheet to a front vertex off the fold (the fold would have receded).
    let at = |i: u16| {
        let o = i as usize * 20;
        (read_f32(&d.vertices, o + 4), read_f32(&d.vertices, o + 8))
    };
    for t in d.indices.chunks_exact(3) {
        let back = t.iter().any(|&i| at(i).1 > 0.0);
        let front_off_fold = t.iter().any(|&i| at(i).1 == 0.0 && at(i).0 < 1.0 - 1e-6);
        assert!(!(back && front_off_fold), "{t:?}");
    }
}

/// Asserts the structural invariants of a decimation result: indices in range, no
/// degenerate triangle, every output vertex used and numbered in first-use order.
fn assert_compact(d: &Decimated, stride: usize) {
    let vcount = d.vertex_count(stride);
    assert_eq!(d.vertices.len() % stride, 0);
    assert_eq!(d.indices.len() % 3, 0);
    assert!(d.indices.iter().all(|&i| (i as usize) < vcount));
    assert!(d.indices.as_chunks::<3>().0.iter().all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]));
    let mut next = 0u16;
    for &i in &d.indices {
        assert!(i <= next, "vertex {i} used before {next}");
        if i == next {
            next += 1;
        }
    }
    assert_eq!(next as usize, vcount, "unused vertices left in the buffer");
}

#[test]
fn build_locks_is_empty_without_positions_or_attributes() {
    // No position: nothing can be compared.
    let decl = VertexDecl {
        flags: 1 << semantic::TEXCOORD0,
        stride: 8,
        count: 1,
        types: (component::FLOAT2 as u64) << (semantic::TEXCOORD0 * 4),
    };
    let v = vec![0u8; 3 * 8];
    let geom = GeometryData { decl, stride: 8, vertices: &v, indices: &[0, 1, 2] };
    assert!(build_locks(&geom, &[0, 1, 2]).iter().all(|&l| l == SimplifyVertex::None));
    // Position only: coincident vertices cannot differ in a locked attribute.
    let decl = VertexDecl { flags: 1, stride: 12, count: 1, types: component::FLOAT3 as u64 };
    let mut v = Vec::new();
    push_f32s(&mut v, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let geom = GeometryData { decl, stride: 12, vertices: &v, indices: &[0, 1, 2] };
    let locks = build_locks(&geom, &[0, 1, 2]);
    assert_eq!(locks.len(), 3);
    assert!(locks.iter().all(|&l| l == SimplifyVertex::None));
}

#[test]
fn coincident_vertices_with_identical_attributes_are_not_protected() {
    let mut v = grid_at(2, 0.0);
    // Exact copy of the centre vertex (same position and UV): no discontinuity.
    let copy = v[4 * 20..5 * 20].to_vec();
    v.extend(copy);
    // A copy of a corner with another UV: a seam.
    let mut seam = v[0..20].to_vec();
    seam[12..16].copy_from_slice(&0.5f32.to_le_bytes());
    v.extend(seam);
    let idx = grid_indices(2, 0);
    let geom = GeometryData { decl: decl_pos_uv(), stride: 20, vertices: &v, indices: &idx };
    let locks = build_locks(&geom, &idx.iter().map(|&i| i as u32).collect::<Vec<_>>());
    assert!(!locks[4].contains(SimplifyVertex::Protect));
    assert!(!locks[9].contains(SimplifyVertex::Protect));
    assert!(locks[0].contains(SimplifyVertex::Protect));
    assert!(locks[10].contains(SimplifyVertex::Protect));
    // A single bone everywhere would add no priority either: there is no skin here.
    assert!(locks.iter().all(|l| !l.contains(SimplifyVertex::Priority)));
}

#[test]
fn decimate_skips_unreachable_or_negligible_targets() {
    let (decl, v, idx) = grid(32);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    let tris = idx.len() / 3;
    // Already within the budget.
    assert!(decimate(&geom, tris, 1.0, false).unwrap().is_none());
    assert!(decimate(&geom, tris + 100, 1.0, false).unwrap().is_none());
    // Less than 5 % of the triangles would go: not worth rewriting the geometry.
    assert!(decimate(&geom, tris - 1, 1.0, false).unwrap().is_none());
}

#[test]
fn decimate_needs_float3_positions_and_valid_indices() {
    let (_, v, idx) = grid(4);
    // Same bytes declared as Float4 positions: positions cannot be read, nothing done.
    let decl = VertexDecl { flags: 1, stride: 20, count: 1, types: component::FLOAT4 as u64 };
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    assert!(decimate(&geom, 4, 1.0, false).unwrap().is_none());
    // An index past the vertex buffer is an error, not a silent skip.
    let mut bad = idx.clone();
    bad[5] = 25;
    let geom = GeometryData { decl: decl_pos_uv(), stride: 20, vertices: &v, indices: &bad };
    assert!(decimate(&geom, 4, 1.0, false).is_err());
}

#[test]
fn decimate_reaches_the_target_when_the_error_allows_it() {
    let (decl, v, idx) = grid(32);
    let geom = GeometryData { decl, stride: 20, vertices: &v, indices: &idx };
    for (target, order_sensitive) in [(500, false), (500, true), (300, false)] {
        let d = decimate(&geom, target, 1.0, order_sensitive).unwrap().unwrap();
        assert!(d.indices.len() / 3 <= target, "{} > {target}", d.indices.len() / 3);
        assert!(d.error >= 0.0 && d.error <= 1.0);
        assert_compact(&d, 20);
    }
}

#[test]
fn stricter_error_bound_keeps_more_triangles() {
    // Bumpy grid: every collapse moves the surface.
    let n = 24;
    let mut v = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            let z = ((x * 7 + y * 13) % 5) as f32 * 0.02;
            push_f32s(&mut v, &[fx, fy, z, fx, fy]);
        }
    }
    let idx = grid_indices(n, 0);
    let geom = GeometryData { decl: decl_pos_uv(), stride: 20, vertices: &v, indices: &idx };
    let tris = |e: f32| decimate(&geom, 10, e, false).unwrap().map_or(idx.len() / 3, |d| d.indices.len() / 3);
    let (strict, loose) = (tris(1e-4), tris(0.5));
    assert!(strict > loose, "{strict} <= {loose}");
    let d = decimate(&geom, 10, 0.5, false).unwrap().unwrap();
    assert!(d.indices.len() < idx.len());
    assert_compact(&d, 20);
}

#[test]
fn triangle_soup_is_welded_before_simplification() {
    // Every triangle has its own 3 vertices; copies only differ by their tangent (export
    // noise), which the weld ignores.
    let decl = VertexDecl {
        flags: (1 << semantic::POSITION) | (1 << semantic::TEXCOORD0) | (1 << semantic::TANGENT),
        stride: 36,
        count: 3,
        types: (component::FLOAT3 as u64)
            | ((component::FLOAT2 as u64) << (semantic::TEXCOORD0 * 4))
            | ((component::FLOAT4 as u64) << (semantic::TANGENT * 4)),
    };
    let n = 16;
    let shared = grid_at(n, 0.0);
    let mut v = Vec::new();
    let mut idx = Vec::new();
    for (k, &i) in grid_indices(n, 0).iter().enumerate() {
        v.extend_from_slice(&shared[i as usize * 20..(i as usize + 1) * 20]);
        push_f32s(&mut v, &[1.0, (k % 7) as f32 * 1e-3, 0.0, 1.0]);
        idx.push(k as u16);
    }
    let geom = GeometryData { decl, stride: 36, vertices: &v, indices: &idx };
    let d = decimate(&geom, 60, 0.01, false).unwrap().expect("welded soup must simplify");
    assert!(d.indices.len() < idx.len() / 2);
    assert_compact(&d, 36);
    // Output vertices are still byte-exact input vertices (tangent included).
    for out in d.vertices.as_chunks::<36>().0 {
        assert!(v.as_chunks::<36>().0.iter().any(|inp| inp == out));
    }
}
