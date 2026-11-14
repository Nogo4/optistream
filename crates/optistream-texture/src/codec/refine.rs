//! Grey refinement of DXT colour blocks: dark, near-grey texels keep their hue and brightness.

use crate::codec::decode::{color_palette, rgb565};

/// Rows per parallel encoding strip (a multiple of the 4 px block height).
pub(super) const STRIP_ROWS: usize = 64;

/// Largest per-texel channel spread for a block to count as grey.
const GREY_CHROMA: i32 = 6;

/// Brightest channel value under which a block counts as dark.
const DARK_LUMA: i32 = 96;

/// Largest per-texel channel spread for a dark block to be refitted on grey endpoints.
const DARK_CHROMA: i32 = 24;

/// How far (0..255) grey endpoints may sit outside the block's brightness range.
const GREY_MARGIN: i32 = 10;

pub(super) fn block_has_hidden_texels(rows: &[u8], width: usize, bx: usize, by: usize) -> bool {
    (0..16).any(|t| rows[((by + t / 4) * width + bx + t % 4) * 4 + 3] < 128)
}

pub(super) fn block_texels(rows: &[u8], width: usize, bx: usize, by: usize) -> [[i32; 3]; 16] {
    std::array::from_fn(|t| {
        let o = ((by + t / 4) * width + bx + t % 4) * 4;
        [rows[o] as i32, rows[o + 1] as i32, rows[o + 2] as i32]
    })
}

/// Weight of tint errors against brightness errors when grey blocks are fitted.
const TINT_WEIGHT: i64 = 3;

/// Error of one texel, split into brightness and tint: with `d` the channel deltas and
/// `s` their sum, `3·s² + w·Σ(3·d − s)²` is `9·Σd²` for w = 1; w > 1 penalises tints.
fn texel_error(p: &[i32; 3], q: &[i32; 3]) -> i64 {
    let d = [0, 1, 2].map(|c| (q[c] - p[c]) as i64);
    let s = d.iter().sum::<i64>();
    3 * s * s + TINT_WEIGHT * d.iter().map(|v| (3 * v - s).pow(2)).sum::<i64>()
}

/// Error of `texels` against their best entry of a four-colour palette, with the index bits
/// of that choice.
fn fit_palette(texels: &[[i32; 3]; 16], palette: &[[i32; 3]; 4]) -> (i64, u32) {
    let (mut err, mut indices) = (0i64, 0u32);
    for (t, p) in texels.iter().enumerate() {
        let (best, slot) = palette
            .iter()
            .enumerate()
            .map(|(j, q)| (texel_error(p, q), j as u32))
            .min()
            .unwrap();
        err += best;
        indices |= slot << (2 * t);
    }
    (err, indices)
}

fn four_colour_palette(v0: u16, v1: u16) -> [[i32; 3]; 4] {
    let [c0, c1] = [v0, v1].map(|v| rgb565(v).map(|c| c as i32));
    let mix = |a: i32, b: i32| std::array::from_fn(|i| (a * c0[i] + b * c1[i]) / 3);
    [c0, c1, mix(2, 1), mix(1, 2)]
}

/// The 32 greys of RGB565: R = B = k with the G closest to them, so the three channels
/// expand to (almost) the same value.
fn grey_endpoints() -> &'static [u16; 32] {
    static GREYS: std::sync::OnceLock<[u16; 32]> = std::sync::OnceLock::new();
    GREYS.get_or_init(|| {
        std::array::from_fn(|k| {
            let k = k as u16;
            let red = rgb565(k << 11)[0] as i32;
            let g = (0..64u16).min_by_key(|&g| (rgb565(g << 5)[1] as i32 - red).abs()).unwrap();
            (k << 11) | (g << 5) | k
        })
    })
}

/// Re-fits a near-grey colour block on grey 565 endpoints when that beats the current block
/// (error with tints weighted up). R and B have 5 bits and G 6: a fit on free endpoints
/// rounds them apart and tints dark greys green or magenta.
pub(super) fn refine_grey(texels: &[[i32; 3]; 16], block: &mut [u8], allow_punch_through: bool) {
    let chroma = texels.iter().map(|p| p.iter().max().unwrap() - p.iter().min().unwrap()).max().unwrap();
    let brightest = texels.iter().map(|p| *p.iter().max().unwrap()).max().unwrap();
    // Dark blocks tolerate more chroma: 565 rounding tints them first, and the grey fit only
    // replaces the current block when it wins under the tint-weighted error.
    let limit = if brightest <= DARK_LUMA { DARK_CHROMA } else { GREY_CHROMA };
    if chroma > limit {
        return;
    }
    let luma = texels.iter().map(|p| (p[0] + p[1] + p[2]) / 3);
    let (lo, hi) = (luma.clone().min().unwrap() - GREY_MARGIN, luma.max().unwrap() + GREY_MARGIN);
    let endpoints: Vec<u16> =
        grey_endpoints().iter().copied().filter(|&v| (lo..=hi).contains(&(rgb565(v)[1] as i32))).collect();
    // The current endpoints compete under the same metric (their indices are re-chosen too).
    let mut current = color_palette(block, allow_punch_through).map(|p| [p[0] as i32, p[1] as i32, p[2] as i32]);
    if color_palette(block, allow_punch_through)[3][3] == 0 {
        // Three-colour DXT1: index 3 is transparent, never a choice for opaque texels (a tie
        // with index 2 keeps index 2).
        current[3] = current[2];
    }
    let (current_err, current_indices) = fit_palette(texels, &current);
    let mut best = (current_err, [block[0], block[1], block[2], block[3]], current_indices);
    for &a in &endpoints {
        for &b in endpoints.iter().filter(|&&b| b < a) {
            let (err, indices) = fit_palette(texels, &four_colour_palette(a, b));
            if err < best.0 {
                let [a0, a1] = a.to_le_bytes();
                let [b0, b1] = b.to_le_bytes();
                best = (err, [a0, a1, b0, b1], indices);
            }
        }
    }
    block[0..4].copy_from_slice(&best.1);
    block[4..8].copy_from_slice(&best.2.to_le_bytes());
}
