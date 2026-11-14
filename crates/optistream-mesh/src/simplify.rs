//! Skin-safe decimation: border, bone and seam locks, then meshopt simplification.

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use meshopt::{SimplifyOptions, SimplifyVertex};
use optistream_rage::drawable::semantic;

use crate::vertex::{component_floats, component_range, positions};
use crate::{Decimated, GeometryData, OVERDRAW_THRESHOLD};

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
pub(super) fn build_locks(geom: &GeometryData, indices: &[u32]) -> Vec<SimplifyVertex> {
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
