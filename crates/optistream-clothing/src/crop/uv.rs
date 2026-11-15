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

#[cfg(test)]
mod tests {
    use optistream_rage::drawable::VertexDecl;

    use super::*;

    fn geometry(decl: VertexDecl) -> GeometryInfo {
        GeometryInfo {
            offset: 0,
            vb_offset: 0,
            ib_offset: 0,
            decl,
            shader_index: None,
            vertex_stride: decl.stride,
            vertex_count: 0,
            vertex_ptr: 0,
            vertex_ptr2: 0,
            index_ptr: 0,
            index_count: 0,
        }
    }

    /// Position (Float3), normal (Float3) and TEXCOORD0 of type `uv`.
    fn decl_with_uv(uv: u8) -> VertexDecl {
        VertexDecl {
            flags: (1 << semantic::POSITION) | (1 << semantic::NORMAL) | (1 << semantic::TEXCOORD0),
            stride: (24 + component::size(uv)) as u16,
            count: 3,
            types: (component::FLOAT3 as u64)
                | ((component::FLOAT3 as u64) << (semantic::NORMAL * 4))
                | ((uv as u64) << (semantic::TEXCOORD0 * 4)),
        }
    }

    #[test]
    fn half_decoding_handles_special_values() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xB800), -0.5);
        assert!(half_to_f32(0x8000) == 0.0 && half_to_f32(0x8000).is_sign_negative());
        // Subnormals are normalised to the exact f32 value.
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x0200), 2f32.powi(-15));
        assert_eq!(half_to_f32(0x83FF), -1023.0 * 2f32.powi(-24));
        assert_eq!(half_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(half_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(half_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn every_finite_half_survives_a_round_trip() {
        for h in 0..=u16::MAX {
            // NaN payloads are not preserved; everything else must be exact (both signs,
            // subnormals and infinities included).
            if h & 0x7C00 == 0x7C00 && h & 0x3FF != 0 {
                continue;
            }
            assert_eq!(f32_to_half(half_to_f32(h)), h, "0x{h:04X}");
        }
    }

    #[test]
    fn half_encoding_rounds_to_a_neighbour_and_saturates() {
        assert_eq!(f32_to_half(1.0), 0x3C00);
        assert_eq!(f32_to_half(65504.0), 0x7BFF);
        // Beyond the half range: infinity of the same sign.
        assert_eq!(f32_to_half(1e6), 0x7C00);
        assert_eq!(f32_to_half(-1e6), 0xFC00);
        assert_eq!(f32_to_half(65520.0), 0x7C00);
        // Far below the smallest subnormal: a signed zero.
        assert_eq!(f32_to_half(1e-10), 0x0000);
        assert_eq!(f32_to_half(-1e-10), 0x8000);
        // A mantissa carry moves to the next exponent.
        assert_eq!(f32_to_half(2047.9), 0x6800);
        // Any value in range lands on the closest half: neither neighbour is nearer.
        for k in 0..4000 {
            let x = -3.0 + k as f32 * 0.001_51;
            let h = f32_to_half(x);
            let err = (half_to_f32(h) - x).abs();
            for n in [h.wrapping_add(1), h.wrapping_sub(1)] {
                let other = (half_to_f32(n) - x).abs();
                assert!(other.is_nan() || err <= other, "{x}: 0x{h:04X} vs 0x{n:04X}");
            }
        }
    }

    #[test]
    fn uv_read_write_round_trip_at_the_component_offset() {
        let mut v = vec![0xEEu8; 40];
        write_uv(&mut v, 24, component::FLOAT2, [0.3, -0.7]);
        assert_eq!(read_uv(&v, 24, component::FLOAT2), [0.3, -0.7]);
        // Bytes around the component are left alone.
        assert!(v[..24].iter().chain(&v[32..]).all(|&b| b == 0xEE));

        let mut v = vec![0xEEu8; 32];
        write_uv(&mut v, 24, component::HALF2, [0.25, 0.8]);
        let [u, w] = read_uv(&v, 24, component::HALF2);
        assert_eq!(u, 0.25);
        assert!((w - 0.8).abs() < 1e-3);
        assert!(v[..24].iter().chain(&v[28..]).all(|&b| b == 0xEE));
    }

    #[test]
    fn uv_layout_accepts_float2_and_half2_only() {
        assert_eq!(uv_layout(&geometry(decl_with_uv(component::FLOAT2))).unwrap(), Some((24, component::FLOAT2)));
        assert_eq!(uv_layout(&geometry(decl_with_uv(component::HALF2))).unwrap(), Some((24, component::HALF2)));
        assert!(uv_layout(&geometry(decl_with_uv(component::FLOAT3))).is_err());
        assert!(uv_layout(&geometry(decl_with_uv(component::HALF4))).is_err());
        // No TEXCOORD0: nothing sampled, nothing to do.
        let mut d = decl_with_uv(component::FLOAT2);
        d.flags &= !(1 << semantic::TEXCOORD0);
        assert_eq!(uv_layout(&geometry(d)).unwrap(), None);
        // Cloth geometry is refused before anything else.
        let mut d = decl_with_uv(component::FLOAT2);
        d.flags |= 0x8000_0000;
        assert!(uv_layout(&geometry(d)).is_err());
    }

    #[test]
    fn window_mapping_is_reversible_through_half_storage() {
        // A UV written in the window then mapped back stays within half precision.
        let w = Window {
            u: crate::crop::Axis { start: 0.25, size: 0.25 },
            v: crate::crop::Axis { start: 0.5, size: 0.5 },
        };
        let mut v = vec![0u8; 4];
        let uv = [0.3f32, 0.9f32];
        write_uv(&mut v, 0, component::HALF2, w.map(uv));
        let [mu, mv] = read_uv(&v, 0, component::HALF2);
        let back = [mu as f64 * 0.25 + 0.25, mv as f64 * 0.5 + 0.5];
        assert!((back[0] - uv[0] as f64).abs() < 1e-3 && (back[1] - uv[1] as f64).abs() < 1e-3);
    }
}
