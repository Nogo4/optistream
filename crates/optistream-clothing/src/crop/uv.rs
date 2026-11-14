//! UV reading and rewriting in vertex buffers (float and half float).

use std::collections::HashSet;

use anyhow::{ensure, Result};
use optistream_rage::drawable::{component, semantic, vb_field, DrawableInfo, GeometryInfo};
use optistream_rage::{Resource, Segment};

use crate::crop::plan::Bounds;
use crate::crop::Window;

/// Reads TEXCOORD0 of a vertex (`Float2` or `Half2`).
fn read_uv(v: &[u8], off: usize, ty: u8) -> [f32; 2] {
    match ty {
        component::FLOAT2 => [0, 4].map(|o| f32::from_le_bytes(v[off + o..off + o + 4].try_into().unwrap())),
        _ => [0, 2].map(|o| half_to_f32(u16::from_le_bytes([v[off + o], v[off + o + 1]]))),
    }
}

fn write_uv(v: &mut [u8], off: usize, ty: u8, uv: [f32; 2]) {
    for (i, x) in uv.into_iter().enumerate() {
        match ty {
            component::FLOAT2 => v[off + 4 * i..off + 4 * i + 4].copy_from_slice(&x.to_le_bytes()),
            _ => v[off + 2 * i..off + 2 * i + 2].copy_from_slice(&f32_to_half(x).to_le_bytes()),
        }
    }
}

/// TEXCOORD0 layout of a geometry: `None` when it has none (nothing sampled).
fn uv_layout(g: &GeometryInfo) -> Result<Option<(usize, u8)>> {
    ensure!(!g.decl.is_cloth(), "géométrie cloth");
    if !g.decl.has(semantic::TEXCOORD0) {
        return Ok(None);
    }
    let ty = g.decl.component_type(semantic::TEXCOORD0);
    ensure!(matches!(ty, component::FLOAT2 | component::HALF2), "UV de type {ty}");
    Ok(Some((g.decl.component_offset(semantic::TEXCOORD0), ty)))
}

/// Grows `b` with the UVs of the vertices the triangles actually use.
pub(super) fn uv_bounds(res: &Resource, d: &DrawableInfo, b: &mut Bounds) -> Result<()> {
    for g in d.geometries() {
        let Some((off, ty)) = uv_layout(g)? else { continue };
        let stride = g.vertex_stride as usize;
        let ptr = if g.vertex_ptr != 0 { g.vertex_ptr } else { g.vertex_ptr2 };
        ensure!(matches!(res.resolve(ptr), Some((Segment::System, _))), "sommets hors du segment système");
        let verts = res.slice(ptr, g.vertex_count as usize * stride)?;
        let indices = res.slice(g.index_ptr, g.index_count as usize * 2)?;
        for i in indices.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize) {
            ensure!(i < g.vertex_count as usize, "indice hors du tampon de sommets");
            let [u, v] = read_uv(&verts[i * stride..], off, ty);
            ensure!(u.is_finite() && v.is_finite(), "UV invalide");
            b[0] = b[0].min(u as f64);
            b[1] = b[1].max(u as f64);
            b[2] = b[2].min(v as f64);
            b[3] = b[3].max(v as f64);
        }
    }
    Ok(())
}

/// Rewrites TEXCOORD0 of every vertex of the drawables into the window.
pub fn remap_uvs(res: &mut Resource, drawables: &[DrawableInfo], w: &Window) -> Result<usize> {
    let mut seen = HashSet::new();
    let mut count = 0;
    for g in drawables.iter().flat_map(|d| d.geometries()) {
        let Some((off, ty)) = uv_layout(g)? else { continue };
        let stride = g.vertex_stride as usize;
        // Decimation may have shortened the buffer: read its current vertex count.
        let vcount = res.u32_at(g.vb_offset + vb_field::VERTEX_COUNT)? as usize;
        let mut targets = vec![g.vertex_ptr, g.vertex_ptr2];
        targets.retain(|&p| p != 0);
        targets.dedup();
        for ptr in targets {
            let start = res.sys_offset(ptr)?;
            if !seen.insert(start) {
                continue;
            }
            for v in res.system[start..start + vcount * stride].chunks_exact_mut(stride) {
                let uv = w.map(read_uv(v, off, ty));
                write_uv(v, off, ty, uv);
            }
            count += vcount;
        }
    }
    Ok(count)
}

pub(super) fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
        (0x1F, m) => sign | 0x7F80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

pub(super) fn f32_to_half(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let man = bits & 0x7F_FFFF;
    if exp >= 0x1F {
        return sign | 0x7C00;
    }
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    // Round to nearest; a carry into the exponent is the correct result.
    sign | (((exp as u32) << 10) + ((man + 0x1000) >> 13)) as u16
}
