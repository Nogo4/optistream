//! Skin-safe decimation and index reordering of drawable geometries.
//!
//! Edge collapses only ever keep original vertices, so bone indices and weights stay
//! exactly as authored. Open borders (neck, cuffs, splits between geometries) are locked
//! to avoid gaps with the body or between parts, and an error bound stops the
//! simplification before the shape visibly degrades, even if the budget is not reached.
//!
//! Index reordering (vertex cache, overdraw, degenerate and duplicate removal) only
//! rewrites the index buffer: vertices keep their content and their order.

pub mod budget;
mod vertex;
mod reorder;
mod simplify;
#[cfg(test)]
mod tests;

pub use reorder::{index_acmr, reorder_indices};
pub use simplify::decimate;

use optistream_rage::drawable::VertexDecl;

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

/// Lossless merge of byte-identical vertices (meshes exported as a triangle soup): the
/// first occurrence is kept, in order, and the indices are remapped (triangle order
/// unchanged). `None` if no vertex is repeated or an index is out of range.
pub fn dedupe_vertices(geom: &GeometryData) -> Option<Decimated> {
    let stride = geom.stride;
    if stride == 0 || geom.vertices.len() % stride != 0 {
        return None;
    }
    let count = geom.vertices.len() / stride;
    let mut first: std::collections::HashMap<&[u8], u16> = std::collections::HashMap::with_capacity(count);
    let mut remap = Vec::with_capacity(count);
    let mut vertices = Vec::with_capacity(geom.vertices.len());
    for v in geom.vertices.chunks_exact(stride) {
        let next = (vertices.len() / stride) as u16;
        let id = *first.entry(v).or_insert_with(|| {
            vertices.extend_from_slice(v);
            next
        });
        remap.push(id);
    }
    if vertices.len() == geom.vertices.len() {
        return None;
    }
    let indices = geom.indices.iter().map(|&i| remap.get(i as usize).copied()).collect::<Option<Vec<u16>>>()?;
    Some(Decimated { vertices, indices, error: 0.0 })
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
