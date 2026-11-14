//! Skin-safe decimation and index reordering of drawable geometries.
//!
//! Edge collapses only ever keep original vertices, so bone indices and weights stay
//! exactly as authored. Open borders (neck, cuffs, splits between geometries) are locked
//! to avoid gaps with the body or between parts, and an error bound stops the
//! simplification before the shape visibly degrades, even if the budget is not reached.
//!
//! Index reordering (vertex cache, overdraw, degenerate and duplicate removal) only
//! rewrites the index buffer: vertices keep their content and their order.

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

/// New index list of a geometry whose vertices are left untouched.
#[derive(Clone, Debug)]
pub struct Reordered {
    pub indices: Vec<u16>,
    /// Degenerate and exact duplicate triangles removed.
    pub removed_triangles: usize,
    pub acmr_before: f32,
    pub acmr_after: f32,
}
