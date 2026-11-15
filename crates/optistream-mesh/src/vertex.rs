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

#[cfg(test)]
mod tests {
    use super::*;

    /// Declaration storing `comps` (semantic, component type), in semantic order.
    fn decl(comps: &[(u32, u8)]) -> VertexDecl {
        let mut d = VertexDecl { flags: 0, stride: 0, count: comps.len() as u8, types: 0 };
        for &(s, t) in comps {
            d.flags |= 1 << s;
            d.types |= (t as u64) << (s * 4);
            d.stride += component::size(t) as u16;
        }
        d
    }

    fn f32s(fs: &[f32]) -> Vec<u8> {
        fs.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn half_decodes_normal_values_exactly() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x4248), 3.140625);
        // Largest finite half and smallest normal half.
        assert_eq!(half_to_f32(0x7BFF), 65504.0);
        assert_eq!(half_to_f32(0x0400), 2f32.powi(-14));
    }

    #[test]
    fn half_decodes_zeros_subnormals_and_infinities() {
        let z = half_to_f32(0x0000);
        assert!(z == 0.0 && z.is_sign_positive());
        let nz = half_to_f32(0x8000);
        assert!(nz == 0.0 && nz.is_sign_negative());
        // Subnormals: mantissa x 2^-24, continuous with the smallest normal value.
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x03FF), 1023.0 * 2f32.powi(-24));
        assert_eq!(half_to_f32(0x8001), -(2f32.powi(-24)));
        assert!(half_to_f32(0x03FF) < half_to_f32(0x0400));
        assert_eq!(half_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(half_to_f32(0xFC00), f32::NEG_INFINITY);
        // NaN payloads never decode to a finite value.
        assert!(!half_to_f32(0x7E00).is_finite());
    }

    #[test]
    fn half_decoding_is_monotonic_over_positive_values() {
        let mut prev = half_to_f32(0);
        for h in 1..0x7C00u16 {
            let x = half_to_f32(h);
            assert!(x > prev, "0x{h:04X}");
            prev = x;
        }
    }

    #[test]
    fn read_f16_is_little_endian_at_any_offset() {
        let b = [0xAA, 0x00, 0x3C, 0x00, 0xC0];
        assert_eq!(read_f16(&b, 1), 1.0);
        assert_eq!(read_f16(&b, 3), -2.0);
    }

    #[test]
    fn component_floats_decodes_every_storage_type() {
        let cases: [(u8, Vec<u8>, Vec<f32>); 8] = [
            (component::FLOAT, f32s(&[1.5]), vec![1.5]),
            (component::FLOAT2, f32s(&[1.0, -2.0]), vec![1.0, -2.0]),
            (component::FLOAT3, f32s(&[1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0]),
            (component::FLOAT4, f32s(&[1.0, 2.0, 3.0, 4.0]), vec![1.0, 2.0, 3.0, 4.0]),
            (component::HALF2, vec![0x00, 0x3C, 0x00, 0x38], vec![1.0, 0.5]),
            (component::HALF4, vec![0x00, 0x3C, 0x00, 0x38, 0x00, 0xC0, 0x00, 0x00], vec![1.0, 0.5, -2.0, 0.0]),
            (component::COLOUR, vec![0, 255, 51, 102], vec![0.0, 1.0, 0.2, 0.4]),
            (component::RGBA8_SNORM, vec![127, 0x81, 0, 0xFF], vec![1.0, -1.0, 0.0, -1.0 / 127.0]),
        ];
        for (ty, bytes, want) in cases {
            // The component follows a Float3 position: its offset is 12.
            let d = decl(&[(semantic::POSITION, component::FLOAT3), (semantic::COLOUR0, ty)]);
            let mut v = f32s(&[9.0, 9.0, 9.0]);
            v.extend(&bytes);
            let mut out = Vec::new();
            assert!(component_floats(&d, semantic::COLOUR0, &v, want.len(), &mut out), "type {ty}");
            assert_eq!(out.len(), want.len());
            for (a, b) in out.iter().zip(&want) {
                assert!((a - b).abs() < 1e-6, "type {ty}: {out:?} != {want:?}");
            }
        }
    }

    #[test]
    fn component_floats_pads_or_truncates_to_n() {
        let d = decl(&[(semantic::TEXCOORD0, component::FLOAT2)]);
        let v = f32s(&[0.25, 0.75]);
        let mut out = vec![42.0];
        assert!(component_floats(&d, semantic::TEXCOORD0, &v, 4, &mut out));
        // Appended after the existing content, padded with zeros.
        assert_eq!(out, [42.0, 0.25, 0.75, 0.0, 0.0]);
        let mut out = Vec::new();
        assert!(component_floats(&d, semantic::TEXCOORD0, &v, 1, &mut out));
        assert_eq!(out, [0.25]);
    }

    #[test]
    fn component_floats_refuses_missing_or_unknown_components() {
        let d = decl(&[(semantic::POSITION, component::FLOAT3)]);
        let v = f32s(&[1.0, 2.0, 3.0]);
        let mut out = Vec::new();
        assert!(!component_floats(&d, semantic::NORMAL, &v, 3, &mut out));
        // Declared but with an undecodable type (NOTHING).
        let d = VertexDecl { flags: 1 << semantic::NORMAL, ..d };
        assert!(!component_floats(&d, semantic::NORMAL, &v, 3, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn component_range_follows_the_declaration_order() {
        let d = decl(&[
            (semantic::POSITION, component::FLOAT3),
            (semantic::BLEND_WEIGHTS, component::COLOUR),
            (semantic::NORMAL, component::FLOAT3),
            (semantic::TEXCOORD0, component::HALF2),
        ]);
        assert_eq!(component_range(&d, semantic::POSITION), Some((0, 12)));
        assert_eq!(component_range(&d, semantic::BLEND_WEIGHTS), Some((12, 4)));
        assert_eq!(component_range(&d, semantic::NORMAL), Some((16, 12)));
        assert_eq!(component_range(&d, semantic::TEXCOORD0), Some((28, 4)));
        assert_eq!(component_range(&d, semantic::BLEND_INDICES), None);
    }

    #[test]
    fn positions_reads_float3_and_refuses_other_layouts() {
        let d = decl(&[(semantic::POSITION, component::FLOAT3), (semantic::TEXCOORD0, component::FLOAT2)]);
        let mut v = f32s(&[1.0, 2.0, 3.0, 0.1, 0.2]);
        v.extend(f32s(&[-4.0, 5.0, -6.0, 0.3, 0.4]));
        let g = GeometryData { decl: d, stride: 20, vertices: &v, indices: &[] };
        assert_eq!(positions(&g), Some(vec![[1.0, 2.0, 3.0], [-4.0, 5.0, -6.0]]));

        let half = decl(&[(semantic::POSITION, component::HALF4)]);
        let g = GeometryData { decl: half, stride: 8, vertices: &[0; 16], indices: &[] };
        assert_eq!(positions(&g), None);
        let none = decl(&[(semantic::TEXCOORD0, component::FLOAT2)]);
        let g = GeometryData { decl: none, stride: 8, vertices: &[0; 16], indices: &[] };
        assert_eq!(positions(&g), None);
    }
}
