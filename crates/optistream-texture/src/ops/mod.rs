//! Resampling and mip generation.

mod coverage;
#[cfg(test)]
mod tests;

use image::{imageops, ImageBuffer, Rgba};

use crate::codec::Rgba8;
use crate::ops::coverage::{coverage, coverage_pyramid, preserve_coverage, visible_neighbour};

/// How texel values must be filtered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// Colour and data textures: filter the stored values as-is.
    ///
    /// Diffuses are deliberately not filtered in linear light: averaging in linear light
    /// brightens fine dark detail (measured on Oren diffuses: up to +12/255 on opaque
    /// textures), which shows in game as lighter clothes. Stored-value averaging keeps the
    /// perceived brightness (+0.06/255 on the same textures) and matches how the stored
    /// mips of the source files average.
    Linear,
    /// Tangent-space normals: Z rebuilt from RG, filtered, then the vector is renormalised.
    Normal,
    /// Colour texture whose alpha is a coverage mask (drawn only by alpha shaders): filtered
    /// premultiplied, so the colour of transparent texels (often white or black) does not
    /// bleed into the cutout edges. Texels left transparent get the colour of their visible
    /// neighbours, which keeps the game's bilinear filtering clean at the edges too.
    AlphaMask,
}

/// Output alpha under which a texel takes the colour of its visible neighbours.
const MIN_COVERAGE: f32 = 1.0 / 255.0;

type ImageF = ImageBuffer<Rgba<f32>, Vec<f32>>;

fn to_float(img: &Rgba8, cs: ColorSpace) -> ImageF {
    let data = img
        .data
        .chunks_exact(4)
        .flat_map(|p| {
            let c = |i: usize| p[i] as f32 / 255.0;
            let b = match cs {
                // The shader only reads RG and rebuilds Z: the stored B may be anything
                // (0, noise, nothing after an ATI2 decode). Filtering and renormalising with
                // it would shrink X and Y, so rebuild it the way the game does.
                ColorSpace::Normal => {
                    let (x, y) = (c(0) * 2.0 - 1.0, c(1) * 2.0 - 1.0);
                    (1.0 - x * x - y * y).max(0.0).sqrt() * 0.5 + 0.5
                }
                _ => c(2),
            };
            let a = c(3);
            match cs {
                ColorSpace::AlphaMask => [c(0) * a, c(1) * a, b * a, a],
                _ => [c(0), c(1), b, a],
            }
        })
        .collect();
    ImageF::from_raw(img.width, img.height, data).unwrap()
}

/// Renormalises a tangent-space normal stored in `[0, 1]` (alpha untouched).
fn renormalize(p: [f32; 4]) -> [f32; 4] {
    let (x, y, z) = (p[0] * 2.0 - 1.0, p[1] * 2.0 - 1.0, p[2] * 2.0 - 1.0);
    let len = (x * x + y * y + z * z).sqrt();
    if len > 1e-4 {
        [(x / len) * 0.5 + 0.5, (y / len) * 0.5 + 0.5, (z / len) * 0.5 + 0.5, p[3]]
    } else {
        p
    }
}

fn to_u8(img: &ImageF, cs: ColorSpace) -> Rgba8 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut data = Vec::with_capacity(img.as_raw().len());
    let mut coverage: Option<Vec<ImageF>> = None;
    for (x, y, p) in img.enumerate_pixels() {
        let [r, g, b, a] = match cs {
            ColorSpace::Normal => renormalize(p.0),
            ColorSpace::Linear => p.0,
            ColorSpace::AlphaMask if p.0[3] >= MIN_COVERAGE => unpremultiply(p.0),
            ColorSpace::AlphaMask => {
                let pyramid = coverage.get_or_insert_with(|| coverage_pyramid(img));
                let [r, g, b, _] = visible_neighbour(pyramid, x, y);
                [r, g, b, p.0[3]]
            }
        };
        data.extend_from_slice(&[q(r), q(g), q(b), q(a)]);
    }
    Rgba8 { width: img.width(), height: img.height(), data }
}

fn unpremultiply(p: [f32; 4]) -> [f32; 4] {
    let a = p[3];
    [p[0] / a, p[1] / a, p[2] / a, a]
}

/// Rebuilds Z from RG and renormalises every texel of a normal map.
pub fn renormalize_normals(img: &Rgba8) -> Rgba8 {
    to_u8(&to_float(img, ColorSpace::Normal), ColorSpace::Normal)
}

/// Resamples to an arbitrary size (used once to reach the target top level).
pub fn resize(img: &Rgba8, width: u32, height: u32, cs: ColorSpace) -> Rgba8 {
    if img.width == width && img.height == height {
        return img.clone();
    }
    let src = to_float(img, cs);
    let dst = imageops::resize(&src, width, height, imageops::FilterType::CatmullRom);
    to_u8(&dst, cs)
}

/// Halves both sides with a 2x2 box filter, staying in floating point.
fn half_f(src: &ImageF, cs: ColorSpace) -> ImageF {
    let (sw, sh) = (src.width(), src.height());
    let (w, h) = ((sw / 2).max(1), (sh / 2).max(1));
    let mut dst = ImageF::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 4];
            let xs = [(2 * x).min(sw - 1), (2 * x + 1).min(sw - 1)];
            let ys = [(2 * y).min(sh - 1), (2 * y + 1).min(sh - 1)];
            for &yy in &ys {
                for &xx in &xs {
                    let p = src.get_pixel(xx, yy).0;
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
            }
            let mut px = acc.map(|v| v / 4.0);
            if cs == ColorSpace::Normal {
                // Renormalise at every level, before the next one is built from it.
                px = renormalize(px);
            }
            dst.put_pixel(x, y, Rgba(px));
        }
    }
    dst
}

/// Halves both sides with a 2x2 box filter.
pub fn half(img: &Rgba8, cs: ColorSpace) -> Rgba8 {
    to_u8(&half_f(&to_float(img, cs), cs), cs)
}

/// Halves `img` `times` times with the 2x2 box filter, staying in floating point.
pub fn half_n(img: &Rgba8, times: u32, cs: ColorSpace) -> Rgba8 {
    let mut cur = to_float(img, cs);
    for _ in 0..times {
        cur = half_f(&cur, cs);
    }
    to_u8(&cur, cs)
}

/// Builds `levels` mip images starting from `top` (included, kept as is).
///
/// Every level is computed from the previous one in floating point: quantisation to 8 bits
/// only happens once per level, for its output.
///
/// Cutouts (`AlphaMask`): a box filter averages thin strands (hair, lace) into texels under
/// the alpha test, so they thin out and vanish at distance. Each generated level gets its
/// alpha scaled so that at least the share of texels passing the test at the top level
/// still passes it (alpha-to-coverage preservation).
pub fn mip_chain(top: Rgba8, levels: u32, cs: ColorSpace) -> Vec<Rgba8> {
    let mut cur = to_float(&top, cs);
    let target = (cs == ColorSpace::AlphaMask).then(|| coverage(top.data.chunks_exact(4).map(|p| p[3] as f32 / 255.0), 1.0));
    let mut out = vec![top];
    while (out.len() as u32) < levels {
        cur = half_f(&cur, cs);
        let mut level = to_u8(&cur, cs);
        if let Some(target) = target {
            preserve_coverage(&mut level, target);
        }
        out.push(level);
    }
    out
}

/// Downscales for on-screen previews (nearest power of two not above `max_side`).
pub fn preview(img: &Rgba8, max_side: u32) -> Rgba8 {
    let mut cur = img.clone();
    while cur.width.max(cur.height) > max_side && cur.width > 1 && cur.height > 1 {
        cur = half(&cur, ColorSpace::Linear);
    }
    cur
}
