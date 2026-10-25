//! Tiny software rasterizer for before/after mesh comparisons (flat shading + wireframe).

use optistream_core::{PreviewImage, PreviewMesh};

pub type Bounds = ([f32; 3], [f32; 3]);

pub fn bounds(mesh: &PreviewMesh) -> Bounds {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &mesh.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

pub struct View {
    /// Rotation around the vertical (Z) axis, radians.
    pub yaw: f32,
    pub wireframe: bool,
}

/// Renders `mesh` seen from the front (GTA: Z up), framed on `frame` so that two meshes
/// rendered with the same bounds line up exactly.
pub fn render(mesh: &PreviewMesh, frame: Bounds, view: &View, size: usize) -> PreviewImage {
    let (lo, hi) = frame;
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
    let scale = if radius > 0.0 { size as f32 * 0.48 / radius } else { 1.0 };
    let (s, c) = view.yaw.sin_cos();

    // Screen space: x right, y down, depth towards the viewer (larger = closer).
    let project = |p: &[f32; 3]| -> [f32; 3] {
        let (x, y, z) = (p[0] - center[0], p[1] - center[1], p[2] - center[2]);
        let rx = x * c - y * s;
        let ry = x * s + y * c;
        [size as f32 / 2.0 + rx * scale, size as f32 / 2.0 - z * scale, -ry]
    };
    let pts: Vec<[f32; 3]> = mesh.positions.iter().map(project).collect();

    let mut depth = vec![f32::MIN; size * size];
    let mut rgba = vec![0u8; size * size * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[24, 27, 31, 255]);
    }

    for tri in mesh.indices.chunks_exact(3) {
        let (Some(a), Some(b), Some(cc)) =
            (pts.get(tri[0] as usize), pts.get(tri[1] as usize), pts.get(tri[2] as usize))
        else {
            continue;
        };
        // Face normal in view space for flat shading (double-sided).
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-12 {
            continue;
        }
        let light = (n[2] / len).abs() * 0.75 + 0.25;
        let shade = [(110.0 * light) as u8, (190.0 * light) as u8, (180.0 * light) as u8];

        let area = (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        let min_x = a[0].min(b[0]).min(cc[0]).floor().max(0.0) as usize;
        let max_x = a[0].max(b[0]).max(cc[0]).ceil().min(size as f32 - 1.0) as usize;
        let min_y = a[1].min(b[1]).min(cc[1]).floor().max(0.0) as usize;
        let max_y = a[1].max(b[1]).max(cc[1]).ceil().min(size as f32 - 1.0) as usize;
        if min_x > max_x || min_y > max_y {
            continue;
        }
        // Edge lengths, to turn barycentric weights into pixel distances for the wireframe.
        let edge = |p: &[f32; 3], q: &[f32; 3]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt().max(1e-6);
        let (la, lb, lc) = (edge(b, cc), edge(cc, a), edge(a, b));
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b[0] - fx) * (cc[1] - fy) - (b[1] - fy) * (cc[0] - fx)) / area;
                let w1 = ((cc[0] - fx) * (a[1] - fy) - (cc[1] - fy) * (a[0] - fx)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * cc[2];
                let i = y * size + x;
                if z <= depth[i] {
                    continue;
                }
                depth[i] = z;
                // Distance to the closest edge in pixels: weight × (2 × triangle area) / edge length.
                let on_edge = view.wireframe
                    && (w0 * area.abs() / la).min(w1 * area.abs() / lb).min(w2 * area.abs() / lc) < 0.6;
                let col = if on_edge { [shade[0] / 3, shade[1] / 3, shade[2] / 3] } else { shade };
                rgba[i * 4..i * 4 + 3].copy_from_slice(&col);
            }
        }
    }
    PreviewImage { width: size as u32, height: size as u32, rgba }
}
