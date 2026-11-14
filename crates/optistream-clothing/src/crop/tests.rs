use optistream_rage::texture::format as f;
use optistream_texture::TextureMeta;

use crate::crop::plan::axis;
use crate::crop::uv::{f32_to_half, half_to_f32};
use super::*;

#[test]
fn axis_is_a_power_of_two_holding_the_used_range() {
    // Vestium's measured case: 0.22 of a 4096 axis is used.
    let a = axis(0.40, 0.62, 4096, 4096);
    assert_eq!(a.size, 0.25);
    assert!(a.start <= 0.40 && a.start + a.size >= 0.62);
    assert_eq!((a.start * 4096.0) as u32 % 4, 0);
    // Nearly everything used: nothing to gain.
    assert_eq!(axis(0.02, 0.97, 2048, 2048), Axis::FULL);
    // Near the edge, the window is pushed back inside the texture.
    let a = axis(0.80, 0.99, 1024, 1024);
    assert!(a.start + a.size <= 1.0 && a.start <= 0.80);
}

#[test]
fn window_is_aligned_for_every_texture_size() {
    let a = axis(0.13, 0.30, 512, 2048);
    for n in [512u32, 1024, 2048] {
        assert_eq!((a.start * n as f64).round() as u32 % 4, 0, "{n}");
    }
}

#[test]
fn remapped_uvs_land_in_the_same_texels() {
    let w = Window { u: Axis { start: 0.25, size: 0.5 }, v: Axis { start: 0.5, size: 0.25 } };
    let (x0, y0, cw, ch) = w.rect(1024, 1024);
    let uv = [0.4f32, 0.6f32];
    let m = w.map(uv);
    // Same pixel in the cropped texture as in the original one.
    assert!(((m[0] * cw as f32) + x0 as f32 - uv[0] * 1024.0).abs() < 1e-3);
    assert!(((m[1] * ch as f32) + y0 as f32 - uv[1] * 1024.0).abs() < 1e-3);
}

#[test]
fn dxt_crop_copies_blocks() {
    // 16x16 DXT1: block (bx, by) holds bytes all equal to bx + 4 * by.
    let mut data = Vec::new();
    for by in 0..4u8 {
        for bx in 0..4u8 {
            data.extend_from_slice(&[bx + 4 * by; 8]);
        }
    }
    let meta = TextureMeta { width: 16, height: 16, format: f::DXT1, levels: 1 };
    let w = Window { u: Axis { start: 0.5, size: 0.5 }, v: Axis { start: 0.25, size: 0.5 } };
    let (m, out) = crop_texture(&meta, &data, &w).unwrap().unwrap();
    assert_eq!((m.width, m.height, m.levels), (8, 8, 1));
    let firsts: Vec<u8> = out.chunks_exact(8).map(|b| b[0]).collect();
    assert_eq!(firsts, [6, 7, 10, 11]);
}

#[test]
fn only_small_uniform_textures_are_exempt() {
    let meta = TextureMeta { width: 8, height: 8, format: f::A8R8G8B8, levels: 1 };
    let flat = [90u8, 90, 90, 255].repeat(64);
    assert!(exempt(&meta, &flat));
    // A small texture with a pattern is sampled differently once the UVs change.
    let checker: Vec<u8> = (0..64).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
    assert!(!exempt(&meta, &checker));
    let big = TextureMeta { width: 64, height: 64, ..meta };
    assert!(!exempt(&big, &[90u8, 90, 90, 255].repeat(64 * 64)));
}

#[test]
fn half_round_trip() {
    for x in [0.0f32, 0.25, 0.5, 0.999, 1.0, -0.3, 2.5] {
        assert!((half_to_f32(f32_to_half(x)) - x).abs() < 1e-3, "{x}");
    }
}
