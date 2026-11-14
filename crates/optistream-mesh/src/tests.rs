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
