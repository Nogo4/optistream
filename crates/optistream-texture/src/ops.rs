//! Resampling and mip generation.

use image::{imageops, ImageBuffer, Rgba};

use crate::codec::Rgba8;

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

/// Successive 2x2 averages of a premultiplied image, down to 1x1 (level 0 excluded).
fn coverage_pyramid(img: &ImageF) -> Vec<ImageF> {
    let mut levels: Vec<ImageF> = Vec::new();
    loop {
        let cur = levels.last().unwrap_or(img);
        if cur.width() == 1 && cur.height() == 1 {
            return levels;
        }
        // Negative CatmullRom lobes must not cancel out visible colour.
        let mut next = half_f(cur, ColorSpace::AlphaMask);
        next.pixels_mut().for_each(|p| p.0 = p.0.map(|v| v.max(0.0)));
        levels.push(next);
    }
}

/// Colour of the closest coarser level that holds visible texels above `(x, y)`.
fn visible_neighbour(pyramid: &[ImageF], x: u32, y: u32) -> [f32; 4] {
    for (k, level) in pyramid.iter().enumerate() {
        let shift = k as u32 + 1;
        let p = level.get_pixel((x >> shift).min(level.width() - 1), (y >> shift).min(level.height() - 1)).0;
        if p[3] > 1e-6 {
            return unpremultiply(p);
        }
    }
    [0.0; 4]
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

/// Alpha the game's cutout shaders test against (texels at or above it are drawn).
const ALPHA_TEST: f32 = 0.5;

/// Largest alpha scale tried to bring a level's coverage back up.
const MAX_ALPHA_SCALE: f32 = 8.0;

/// Share of texels whose alpha, multiplied by `scale`, passes the alpha test.
fn coverage(alpha: impl Iterator<Item = f32>, scale: f32) -> f32 {
    let (mut passed, mut n) = (0usize, 0usize);
    for a in alpha {
        passed += (a * scale >= ALPHA_TEST) as usize;
        n += 1;
    }
    passed as f32 / n.max(1) as f32
}

/// Scales the alpha of `level` by the smallest factor that brings its coverage up to `target`.
/// Never lowers it: levels that already cover enough are left as they are.
fn preserve_coverage(level: &mut Rgba8, target: f32) {
    let alpha = |img: &Rgba8| img.data.chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect::<Vec<_>>();
    let a = alpha(level);
    let cov = |s: f32| coverage(a.iter().copied(), s);
    if target <= 0.0 || cov(1.0) >= target {
        return;
    }
    let (mut lo, mut hi) = (1.0f32, MAX_ALPHA_SCALE);
    if cov(hi) < target {
        lo = hi;
    } else {
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if cov(mid) >= target {
                hi = mid;
            } else {
                lo = mid;
            }
        }
    }
    let scale = if lo == MAX_ALPHA_SCALE { lo } else { hi };
    for p in level.data.chunks_exact_mut(4) {
        p[3] = (p[3] as f32 * scale).round().min(255.0) as u8;
    }
}

/// Downscales for on-screen previews (nearest power of two not above `max_side`).
pub fn preview(img: &Rgba8, max_side: u32) -> Rgba8 {
    let mut cur = img.clone();
    while cur.width.max(cur.height) > max_side && cur.width > 1 && cur.height > 1 {
        cur = half(&cur, ColorSpace::Linear);
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Rgba8 {
        let mut img = Rgba8::new(w, h);
        for p in img.data.chunks_exact_mut(4) {
            p.copy_from_slice(&px);
        }
        img
    }

    #[test]
    fn normal_z_is_rebuilt_from_rg() {
        // x = 0.6, y = 0 with a stored B of 0 (z = -1): renormalising with B shrank x to 0.51.
        let r = ((0.6f32 * 0.5 + 0.5) * 255.0).round() as i32;
        let img = solid(4, 4, [r as u8, 128, 0, 255]);
        let out = half(&img, ColorSpace::Normal);
        assert!((out.data[0] as i32 - r).abs() <= 1, "R = {}", out.data[0]);
        // Rebuilt z = 0.8, stored as 0.9 * 255.
        assert!((out.data[2] as i32 - 230).abs() <= 1, "B = {}", out.data[2]);
    }

    #[test]
    fn well_formed_normal_is_unchanged() {
        // (0.6, 0, 0.8) stored with its correct Z.
        let px = [204, 128, 230, 255];
        let out = half(&solid(8, 8, px), ColorSpace::Normal);
        for (a, b) in out.data[..4].iter().zip(px) {
            assert!((*a as i32 - b as i32).abs() <= 1, "{:?}", &out.data[..4]);
        }
    }

    #[test]
    fn fine_dark_detail_keeps_its_brightness() {
        // One-texel black/white checker: linear-light averaging gave 188, i.e. lighter clothes.
        let mut img = Rgba8::new(8, 8);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let v = if (i % 8 + i / 8) % 2 == 0 { 0 } else { 255 };
            p.copy_from_slice(&[v, v, v, 255]);
        }
        for out in [half(&img, ColorSpace::Linear), resize(&img, 4, 4, ColorSpace::Linear)] {
            for p in out.data.chunks_exact(4) {
                // CatmullRom lands a few levels off at the image border.
                assert!((p[0] as i32 - 128).abs() <= 4, "{:?}", &p[..4]);
            }
        }
    }

    #[test]
    fn float_mip_chain_matches_direct_average() {
        let (w, h) = (256u32, 256u32);
        let mut img = Rgba8::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, ((x + y) / 2) as u8, 255]);
            }
        }
        let chain = mip_chain(img.clone(), 7, ColorSpace::Linear); // 256 .. 4
        let last = chain.last().unwrap();
        assert_eq!((last.width, last.height), (4, 4));
        let block = w / 4;
        for by in 0..4 {
            for bx in 0..4 {
                for c in 0..3 {
                    let mut acc = 0f64;
                    for y in by * block..(by + 1) * block {
                        for x in bx * block..(bx + 1) * block {
                            acc += img.data[((y * w + x) * 4) as usize + c] as f64;
                        }
                    }
                    let direct = acc / (block * block) as f64;
                    let got = last.data[((by * 4 + bx) * 4) as usize + c] as f64;
                    assert!((got - direct).abs() <= 1.0, "4x4 ({bx},{by}) c{c}: {got} vs {direct}");
                }
            }
        }
    }

    /// One opaque column out of four (hair strands): 25 % of the texels pass the alpha test.
    fn strands(side: u32) -> Rgba8 {
        let mut img = Rgba8::new(side, side);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let a = if i as u32 % side % 4 == 0 { 255 } else { 0 };
            p.copy_from_slice(&[60, 40, 20, a]);
        }
        img
    }

    fn passing(img: &Rgba8) -> f32 {
        coverage(img.data.chunks_exact(4).map(|p| p[3] as f32 / 255.0), 1.0)
    }

    #[test]
    fn thin_strands_keep_their_coverage() {
        // Box filtering alone: alpha 0.25 at level 2, nothing passes the test any more.
        let plain = mip_chain(strands(64), 4, ColorSpace::Linear);
        assert_eq!(passing(&plain[2]), 0.0);
        let kept = mip_chain(strands(64), 4, ColorSpace::AlphaMask);
        for level in &kept[1..] {
            assert!(passing(level) >= 0.25, "{}x{}: {}", level.width, level.height, passing(level));
        }
        // Colour is untouched by the alpha scale.
        assert_eq!(&kept[2].data[..3], &[60, 40, 20]);
    }

    #[test]
    fn coverage_is_never_lowered() {
        // A solid cutout already covers everything: alpha stays as filtered.
        let chain = mip_chain(solid(16, 16, [10, 20, 30, 200]), 3, ColorSpace::AlphaMask);
        assert!(chain.iter().all(|l| l.data.chunks_exact(4).all(|p| p[3] == 200)));
    }
}
