//! Skin-safe decimation and index reordering of drawable geometries.
//!
//! Edge collapses only ever keep original vertices, so bone indices and weights stay
//! exactly as authored. Open borders (neck, cuffs, splits between geometries) are locked
//! to avoid gaps with the body or between parts, and an error bound stops the
//! simplification before the shape visibly degrades, even if the budget is not reached.
//!
//! Index reordering (vertex cache, overdraw, degenerate and duplicate removal) only
//! rewrites the index buffer: vertices keep their content and their order.

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use meshopt::{SimplifyOptions, SimplifyVertex};
use optistream_rage::drawable::{component, semantic, VertexDecl};

/// Overdraw optimisation may worsen the vertex cache by this factor at most.
const OVERDRAW_THRESHOLD: f32 = 1.05;
/// Cache size used to measure the ACMR (average cache miss ratio).
const ACMR_CACHE_SIZE: u32 = 16;

/// Vertex buffer and 16-bit index list of one geometry.
pub struct GeometryData<'a> {
    pub decl: VertexDecl,
    pub stride: usize,
    pub vertices: &'a [u8],
    pub indices: &'a [u16],
}

#[derive(Clone, Debug)]
pub struct Decimated {
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    /// Relative error reported by the simplifier (fraction of the mesh extent).
    pub error: f32,
}

impl Decimated {
    pub fn vertex_count(&self, stride: usize) -> usize {
        self.vertices.len() / stride
    }
}

/// New index list of a geometry whose vertices are left untouched.
#[derive(Clone, Debug)]
pub struct Reordered {
    pub indices: Vec<u16>,
    /// Degenerate and exact duplicate triangles removed.
    pub removed_triangles: usize,
    pub acmr_before: f32,
    pub acmr_after: f32,
}

fn read_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn read_f16(b: &[u8], o: usize) -> f32 {
    half_to_f32(u16::from_le_bytes([b[o], b[o + 1]]))
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let man = (h & 0x3FF) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Decodes up to `n` floats of a vertex component.
fn component_floats(decl: &VertexDecl, sem: u32, v: &[u8], n: usize, out: &mut Vec<f32>) -> bool {
    if !decl.has(sem) {
        return false;
    }
    let o = decl.component_offset(sem);
    let t = decl.component_type(sem);
    let vals: Vec<f32> = match t {
        component::FLOAT => vec![read_f32(v, o)],
        component::FLOAT2 => (0..2).map(|i| read_f32(v, o + 4 * i)).collect(),
        component::FLOAT3 => (0..3).map(|i| read_f32(v, o + 4 * i)).collect(),
        component::FLOAT4 => (0..4).map(|i| read_f32(v, o + 4 * i)).collect(),
        component::HALF2 => (0..2).map(|i| read_f16(v, o + 2 * i)).collect(),
        component::HALF4 => (0..4).map(|i| read_f16(v, o + 2 * i)).collect(),
        component::COLOUR | component::UBYTE4 => (0..4).map(|i| v[o + i] as f32 / 255.0).collect(),
        component::RGBA8_SNORM => (0..4).map(|i| (v[o + i] as i8) as f32 / 127.0).collect(),
        _ => return false,
    };
    out.extend(vals.iter().copied().chain(std::iter::repeat(0.0)).take(n));
    true
}

/// Byte range `(offset, size)` of a vertex component, if present.
fn component_range(decl: &VertexDecl, sem: u32) -> Option<(usize, usize)> {
    decl.has(sem).then(|| (decl.component_offset(sem), component::size(decl.component_type(sem))))
}

/// Float3 positions of every vertex, if the declaration stores them that way.
fn positions(geom: &GeometryData) -> Option<Vec<[f32; 3]>> {
    if !geom.decl.has(semantic::POSITION) || geom.decl.component_type(semantic::POSITION) != component::FLOAT3 {
        return None;
    }
    let o = geom.decl.component_offset(semantic::POSITION);
    Some(
        geom.vertices
            .chunks_exact(geom.stride)
            .map(|v| [read_f32(v, o), read_f32(v, o + 4), read_f32(v, o + 8)])
            .collect(),
    )
}

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

/// Simplifier locks, per vertex.
///
/// - `Protect` on every position shared by vertices that differ in UV0, blend weights,
///   blend indices or Colour0: under `Permissive`, collapses would otherwise cross such
///   discontinuities like a simple hard edge (UVs sliding, cracks when animated, emissive
///   and non-emissive colours mixing).
/// - `Priority` on both ends of every edge between vertices whose dominant bone differs:
///   blend indices are not part of the attribute metric, so two vertices fully bound to
///   different bones look identical to the simplifier.
///
/// `indices` are the welded triangles given to the simplifier.
fn build_locks(geom: &GeometryData, indices: &[u32]) -> Vec<SimplifyVertex> {
    let stride = geom.stride;
    let decl = &geom.decl;
    let vcount = geom.vertices.len() / stride;
    let mut locks = vec![SimplifyVertex::None; vcount];
    let Some((pos_off, _)) = component_range(decl, semantic::POSITION) else {
        return locks;
    };

    let ranges: Vec<(usize, usize)> =
        [semantic::TEXCOORD0, semantic::BLEND_WEIGHTS, semantic::BLEND_INDICES, semantic::COLOUR0]
            .iter()
            .filter_map(|&s| component_range(decl, s))
            .collect();
    if !ranges.is_empty() {
        let key = |v: &[u8]| -> Vec<u8> { ranges.iter().flat_map(|&(o, n)| v[o..o + n].iter().copied()).collect() };
        let mut attrs_of_pos: HashMap<&[u8], Vec<u8>> = HashMap::new();
        let mut protected_pos: HashSet<&[u8]> = HashSet::new();
        for v in geom.vertices.chunks_exact(stride) {
            let p = &v[pos_off..pos_off + 12];
            let k = key(v);
            match attrs_of_pos.get(p) {
                Some(first) if *first != k => {
                    protected_pos.insert(p);
                }
                Some(_) => {}
                None => {
                    attrs_of_pos.insert(p, k);
                }
            }
        }
        for (lock, v) in locks.iter_mut().zip(geom.vertices.chunks_exact(stride)) {
            if protected_pos.contains(&v[pos_off..pos_off + 12]) {
                *lock |= SimplifyVertex::Protect;
            }
        }
    }

    let skin = component_range(decl, semantic::BLEND_WEIGHTS).zip(component_range(decl, semantic::BLEND_INDICES));
    if let Some(((wo, 4), (io, 4))) = skin {
        // Bone with the largest weight (first one on ties); weights and indices share the
        // same byte order.
        let dominant: Vec<u8> = geom
            .vertices
            .chunks_exact(stride)
            .map(|v| {
                let mut best = 0;
                for k in 1..4 {
                    if v[wo + k] > v[wo + best] {
                        best = k;
                    }
                }
                v[io + best]
            })
            .collect();
        for t in indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (a as usize, b as usize);
                if dominant[a] != dominant[b] {
                    locks[a] |= SimplifyVertex::Priority;
                    locks[b] |= SimplifyVertex::Priority;
                }
            }
        }
    }
    locks
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

/// Simplifies a geometry towards `target_triangles`, never exceeding `max_error`.
///
/// With `order_sensitive`, the surviving triangles keep their original relative order
/// (alpha blended shaders); otherwise they are reordered for the vertex cache and overdraw.
///
/// Returns `None` when the gain would be negligible (less than 5 % of the triangles).
pub fn decimate(
    geom: &GeometryData,
    target_triangles: usize,
    max_error: f32,
    order_sensitive: bool,
) -> Result<Option<Decimated>> {
    let stride = geom.stride;
    if geom.decl.is_cloth() {
        bail!("cloth geometry must not be decimated");
    }
    let Some(positions) = positions(geom) else {
        return Ok(None);
    };
    let vcount = geom.vertices.len() / stride;
    let tri_count = geom.indices.len() / 3;
    if target_triangles >= tri_count || vcount == 0 {
        return Ok(None);
    }
    if geom.indices.iter().any(|&i| i as usize >= vcount) {
        bail!("index out of range in geometry");
    }

    // Attributes guiding the error metric: normals, UVs, blend weights, vertex colours.
    const ATTRS: [(u32, usize, f32); 5] = [
        (semantic::NORMAL, 3, 0.5),
        (semantic::TEXCOORD0, 2, 1.0),
        (semantic::BLEND_WEIGHTS, 4, 0.25),
        (semantic::COLOUR0, 4, 0.1),
        (semantic::COLOUR1, 4, 0.1),
    ];
    let present: Vec<(u32, usize, f32)> =
        ATTRS.iter().copied().filter(|&(s, _, _)| geom.decl.has(s)).collect();
    let attr_stride: usize = present.iter().map(|&(_, n, _)| n).sum();
    let mut attrs = Vec::with_capacity(vcount * attr_stride);
    let mut weights = Vec::with_capacity(attr_stride);
    for &(_, n, w) in &present {
        weights.extend(std::iter::repeat(w).take(n));
    }
    for v in geom.vertices.chunks_exact(stride) {
        for &(s, n, _) in &present {
            if !component_floats(&geom.decl, s, v, n, &mut attrs) {
                attrs.extend(std::iter::repeat(0.0).take(n));
            }
        }
    }

    // Many exports are triangle soups (3 unique vertices per triangle): every edge would
    // look like an open border or a seam and stay locked. Weld vertices that only differ
    // by tangent frame (export noise); the representative of each group is an original
    // vertex, so the output stays verbatim.
    let ignored: Vec<(usize, usize)> = [semantic::TANGENT, semantic::BINORMAL]
        .iter()
        .filter_map(|&s| component_range(&geom.decl, s))
        .collect();
    let key = |v: &[u8]| -> Vec<u8> {
        let mut k = v.to_vec();
        for &(o, n) in &ignored {
            k[o..o + n].fill(0);
        }
        k
    };
    let mut first_of: HashMap<Vec<u8>, u32> = HashMap::with_capacity(vcount);
    let weld: Vec<u32> = geom
        .vertices
        .chunks_exact(stride)
        .enumerate()
        .map(|(i, v)| *first_of.entry(key(v)).or_insert(i as u32))
        .collect();
    let indices: Vec<u32> = geom.indices.iter().map(|&i| weld[i as usize]).collect();

    // Discontinuities left after welding are hard normal edges, UV seams or skinning /
    // colour discontinuities. Collapses may cross hard normal edges (faceted exports would
    // never simplify otherwise); the others are protected, and bone boundaries preferred.
    let locks = build_locks(geom, &indices);

    let mut error = 0f32;
    let result = meshopt::simplify_with_attributes_and_locks_decoder(
        &indices,
        &positions,
        &attrs,
        &weights,
        attr_stride * 4,
        &locks,
        target_triangles * 3,
        max_error,
        SimplifyOptions::LockBorder | SimplifyOptions::Permissive | SimplifyOptions::PreserveFolds,
        Some(&mut error),
    );
    if result.is_empty() || result.len() * 100 > indices.len() * 95 {
        return Ok(None);
    }

    // The simplifier keeps the input order of the surviving triangles.
    let result = if order_sensitive {
        result
    } else {
        let mut r = meshopt::optimize_vertex_cache(&result, vcount);
        meshopt::optimize_overdraw_in_place_decoder(&mut r, &positions, OVERDRAW_THRESHOLD);
        r
    };
    // Compact the vertex buffer in first-use order.
    let mut remap = vec![u32::MAX; vcount];
    let mut vertices = Vec::new();
    let mut new_indices = Vec::with_capacity(result.len());
    let mut next = 0u32;
    for &i in &result {
        let i = i as usize;
        if remap[i] == u32::MAX {
            remap[i] = next;
            next += 1;
            vertices.extend_from_slice(&geom.vertices[i * stride..(i + 1) * stride]);
        }
        new_indices.push(remap[i] as u16);
    }
    Ok(Some(Decimated { vertices, indices: new_indices, error }))
}

#[cfg(test)]
mod tests {
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
}
