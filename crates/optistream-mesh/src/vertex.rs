//! Reading vertex components (positions, half floats) out of raw vertex buffers.

use optistream_rage::drawable::{component, semantic, VertexDecl};

use crate::GeometryData;

pub(super) fn read_f32(b: &[u8], o: usize) -> f32 {
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
pub(super) fn component_floats(decl: &VertexDecl, sem: u32, v: &[u8], n: usize, out: &mut Vec<f32>) -> bool {
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
pub(super) fn component_range(decl: &VertexDecl, sem: u32) -> Option<(usize, usize)> {
    decl.has(sem).then(|| (decl.component_offset(sem), component::size(decl.component_type(sem))))
}

/// Float3 positions of every vertex, if the declaration stores them that way.
pub(super) fn positions(geom: &GeometryData) -> Option<Vec<[f32; 3]>> {
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
