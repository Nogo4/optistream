//! Alpha handling: colour bleeding under cutouts and alpha-test coverage preserved across mips.

use crate::codec::Rgba8;
use crate::ops::{half_f, unpremultiply, ColorSpace, ImageF};

/// Successive 2x2 averages of a premultiplied image, down to 1x1 (level 0 excluded).
pub(super) fn coverage_pyramid(img: &ImageF) -> Vec<ImageF> {
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
pub(super) fn visible_neighbour(pyramid: &[ImageF], x: u32, y: u32) -> [f32; 4] {
    for (k, level) in pyramid.iter().enumerate() {
        let shift = k as u32 + 1;
        let p = level.get_pixel((x >> shift).min(level.width() - 1), (y >> shift).min(level.height() - 1)).0;
        if p[3] > 1e-6 {
            return unpremultiply(p);
        }
    }
    [0.0; 4]
}

/// Alpha the game's cutout shaders test against (texels at or above it are drawn).
const ALPHA_TEST: f32 = 0.5;

/// Largest alpha scale tried to bring a level's coverage back up.
const MAX_ALPHA_SCALE: f32 = 8.0;

/// Share of texels whose alpha, multiplied by `scale`, passes the alpha test.
pub(crate) fn coverage(alpha: impl Iterator<Item = f32>, scale: f32) -> f32 {
    let (mut passed, mut n) = (0usize, 0usize);
    for a in alpha {
        passed += (a * scale >= ALPHA_TEST) as usize;
        n += 1;
    }
    passed as f32 / n.max(1) as f32
}

/// Scales the alpha of `level` by the smallest factor that brings its coverage up to `target`.
/// Never lowers it: levels that already cover enough are left as they are.
pub(super) fn preserve_coverage(level: &mut Rgba8, target: f32) {
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
