//! Pixel decoding to RGBA8 and encoding to the vanilla output formats.

use anyhow::{bail, ensure, Result};
use rayon::prelude::*;
use optistream_rage::texture::format as f;

use crate::format::level_size;

/// Straight-alpha RGBA8 image.
#[derive(Clone, Debug)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba8 {
    pub fn new(width: u32, height: u32) -> Self {
        Rgba8 { width, height, data: vec![0; width as usize * height as usize * 4] }
    }

    /// Whether any pixel is meaningfully transparent.
    pub fn alpha_used(&self) -> bool {
        self.data.chunks_exact(4).any(|p| p[3] < 250)
    }
}

pub fn can_decode(format: u32) -> bool {
    matches!(
        format,
        f::DXT1 | f::DXT3 | f::DXT5 | f::BC7 | f::ATI1 | f::ATI2 | f::A8R8G8B8 | f::X8R8G8B8
            | f::A8B8G8R8 | f::A1R5G5B5 | f::A8 | f::L8
    )
}

/// Decodes one mip level.
pub fn decode(format: u32, width: u32, height: u32, data: &[u8]) -> Result<Rgba8> {
    let need = level_size(format, width, height);
    ensure!(data.len() >= need, "texture level truncated ({} < {need} bytes)", data.len());
    let (w, h) = (width as usize, height as usize);
    let mut img = Rgba8::new(width, height);

    let block = |func: fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>,
                 img: &mut Rgba8|
     -> Result<()> {
        let mut px = vec![0u32; w * h];
        func(data, w, h, &mut px).map_err(anyhow::Error::msg)?;
        // texture2ddecoder packs pixels as little-endian BGRA.
        for (dst, p) in img.data.chunks_exact_mut(4).zip(px) {
            let [b, g, r, a] = p.to_le_bytes();
            dst.copy_from_slice(&[r, g, b, a]);
        }
        Ok(())
    };

    match format {
        // texture2ddecoder applies the DXT1 three-colour rule to DXT3/DXT5 colour blocks
        // (black texels where GPUs interpolate), so the DXT family is decoded here.
        f::DXT1 | f::DXT3 | f::DXT5 => decode_dxt(format, &mut img, data),
        f::BC7 => block(texture2ddecoder::decode_bc7, &mut img)?,
        f::ATI1 => block(texture2ddecoder::decode_bc4, &mut img)?,
        f::ATI2 => block(texture2ddecoder::decode_bc5, &mut img)?,
        f::A8R8G8B8 | f::X8R8G8B8 => {
            for (dst, s) in img.data.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
                let a = if format == f::X8R8G8B8 { 255 } else { s[3] };
                dst.copy_from_slice(&[s[2], s[1], s[0], a]);
            }
        }
        f::A8B8G8R8 => img.data.copy_from_slice(&data[..need]),
        f::A1R5G5B5 => {
            for (dst, s) in img.data.chunks_exact_mut(4).zip(data.chunks_exact(2)) {
                let v = u16::from_le_bytes([s[0], s[1]]);
                let c = |x: u16| ((x & 0x1F) * 255 / 31) as u8;
                dst.copy_from_slice(&[c(v >> 10), c(v >> 5), c(v), if v & 0x8000 != 0 { 255 } else { 0 }]);
            }
        }
        f::A8 => {
            for (dst, &a) in img.data.chunks_exact_mut(4).zip(data) {
                dst.copy_from_slice(&[255, 255, 255, a]);
            }
        }
        f::L8 => {
            for (dst, &l) in img.data.chunks_exact_mut(4).zip(data) {
                dst.copy_from_slice(&[l, l, l, 255]);
            }
        }
        other => bail!("cannot decode texture format 0x{other:08X}"),
    }
    Ok(img)
}

fn rgb565(v: u16) -> [u8; 3] {
    let (r, g, b) = ((v >> 11) & 0x1F, (v >> 5) & 0x3F, v & 0x1F);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// Palette of a DXT colour block. `allow_punch_through` is only true for DXT1: DXT3 and
/// DXT5 always interpolate four colours, whatever the endpoint order.
fn color_palette(block: &[u8], allow_punch_through: bool) -> [[u8; 4]; 4] {
    let (v0, v1) = (u16::from_le_bytes([block[0], block[1]]), u16::from_le_bytes([block[2], block[3]]));
    let (c0, c1) = (rgb565(v0), rgb565(v1));
    let mix = |wa: u16, wb: u16, d: u16| -> [u8; 4] {
        let ch = |i: usize| ((c0[i] as u16 * wa + c1[i] as u16 * wb) / d) as u8;
        [ch(0), ch(1), ch(2), 255]
    };
    let (p0, p1) = ([c0[0], c0[1], c0[2], 255], [c1[0], c1[1], c1[2], 255]);
    if v0 > v1 || !allow_punch_through {
        [p0, p1, mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [p0, p1, mix(1, 1, 2), [0, 0, 0, 0]]
    }
}

/// Alphas of a DXT5 alpha block, indexed by texel.
fn dxt5_alphas(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (block[0] as u16, block[1] as u16);
    let mut table = [0u8; 8];
    table[0] = a0 as u8;
    table[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u16 {
            table[i as usize + 1] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u16 {
            table[i as usize + 1] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let bits = block[2..8].iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
    std::array::from_fn(|t| table[((bits >> (3 * t)) & 7) as usize])
}

fn decode_dxt(format: u32, img: &mut Rgba8, data: &[u8]) {
    let (w, h) = (img.width as usize, img.height as usize);
    let block_len = if format == f::DXT1 { 8 } else { 16 };
    let blocks_x = w.div_ceil(4);
    for (i, block) in data.chunks_exact(block_len).take(blocks_x * h.div_ceil(4)).enumerate() {
        let (bx, by) = (i % blocks_x * 4, i / blocks_x * 4);
        let color = &block[block_len - 8..];
        let palette = color_palette(color, format == f::DXT1);
        let indices = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
        let alphas: Option<[u8; 16]> = match format {
            f::DXT3 => Some(std::array::from_fn(|t| ((block[t / 2] >> (4 * (t % 2))) & 0xF) * 17)),
            f::DXT5 => Some(dxt5_alphas(block)),
            _ => None,
        };
        for t in 0..16 {
            let (x, y) = (bx + t % 4, by + t / 4);
            if x >= w || y >= h {
                continue;
            }
            let mut px = palette[((indices >> (2 * t)) & 3) as usize];
            if let Some(a) = &alphas {
                px[3] = a[t];
            }
            let o = (y * w + x) * 4;
            img.data[o..o + 4].copy_from_slice(&px);
        }
    }
}

/// Encodes one level into DXT1, DXT5 or A8.
///
/// Colour blocks come from squish's cluster fit (texpresso) with uniform channel weights,
/// then near-grey blocks are re-fitted on grey 565 endpoints. The previous fast encoder
/// (intel_tex_2) turned the subtle detail of dark clothes into green/magenta blotches that
/// the game lighting brightens; on a near-black shirt the dark-texel error drops from 1.76
/// to 0.98 and the brightness drift from -0.72 to +0.06.
pub fn encode(format: u32, img: &Rgba8) -> Result<Vec<u8>> {
    encode_with(format, img, false)
}

/// Like [`encode`]; with `alpha_weighted`, DXT5 colour endpoints are fitted on the visible
/// texels only (alpha used as a coverage mask): +0.5 dB on average on Oren cutout diffuses,
/// more than 1 dB on one in seven.
pub fn encode_with(format: u32, img: &Rgba8, alpha_weighted: bool) -> Result<Vec<u8>> {
    if matches!(format, f::DXT1 | f::DXT5) && (img.width % 4 != 0 || img.height % 4 != 0) {
        return encode_with(format, &pad_to_blocks(img), alpha_weighted);
    }
    let (squish, block_len) = match format {
        f::DXT1 => (texpresso::Format::Bc1, 8),
        f::DXT5 => (texpresso::Format::Bc3, 16),
        f::A8 => return Ok(img.data.chunks_exact(4).map(|p| p[3]).collect()),
        other => bail!("cannot encode texture format 0x{other:08X}"),
    };
    // squish turns texels under alpha 128 into DXT1 punch-through: DXT1 output is opaque.
    let mut pixels = img.data.clone();
    if format == f::DXT1 {
        pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    let alpha_weighted = alpha_weighted && format == f::DXT5;
    let params = texpresso::Params {
        algorithm: texpresso::Algorithm::ClusterFit,
        weights: texpresso::COLOUR_WEIGHTS_UNIFORM,
        weigh_colour_by_alpha: alpha_weighted,
    };
    // Blocks are stored row by row, so horizontal strips encode independently.
    let (w, row_bytes) = (img.width as usize, img.width as usize * 4);
    let strips: Vec<Vec<u8>> = pixels
        .par_chunks(row_bytes * STRIP_ROWS)
        .map(|rows| {
            let h = rows.len() / row_bytes;
            let mut out = vec![0u8; squish.compressed_size(w, h)];
            squish.compress(rows, w, h, params, &mut out);
            for (i, block) in out.chunks_exact_mut(block_len).enumerate() {
                let (bx, by) = (i % (w / 4) * 4, i / (w / 4) * 4);
                // The grey refit weighs every texel alike: on cutout edges it would undo the
                // alpha-weighted fit.
                if alpha_weighted && block_has_hidden_texels(rows, w, bx, by) {
                    continue;
                }
                let texels = block_texels(rows, w, bx, by);
                refine_grey(&texels, &mut block[block_len - 8..], format == f::DXT1);
            }
            out
        })
        .collect();
    Ok(strips.concat())
}

/// Pads a level to a multiple of 4 px by repeating its last row and column: the block grid
/// of a DXT level is `ceil(w / 4) x ceil(h / 4)`, whatever the level size.
fn pad_to_blocks(img: &Rgba8) -> Rgba8 {
    let (w, h) = (img.width.div_ceil(4) * 4, img.height.div_ceil(4) * 4);
    let mut out = Rgba8::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let src = ((y.min(img.height - 1) * img.width + x.min(img.width - 1)) * 4) as usize;
            let dst = ((y * w + x) * 4) as usize;
            out.data[dst..dst + 4].copy_from_slice(&img.data[src..src + 4]);
        }
    }
    out
}

/// Rows per parallel encoding strip (a multiple of the 4 px block height).
const STRIP_ROWS: usize = 64;

/// Largest per-texel channel spread for a block to count as grey.
const GREY_CHROMA: i32 = 6;

/// Brightest channel value under which a block counts as dark.
const DARK_LUMA: i32 = 96;

/// Largest per-texel channel spread for a dark block to be refitted on grey endpoints.
const DARK_CHROMA: i32 = 24;

/// How far (0..255) grey endpoints may sit outside the block's brightness range.
const GREY_MARGIN: i32 = 10;

fn block_has_hidden_texels(rows: &[u8], width: usize, bx: usize, by: usize) -> bool {
    (0..16).any(|t| rows[((by + t / 4) * width + bx + t % 4) * 4 + 3] < 128)
}

fn block_texels(rows: &[u8], width: usize, bx: usize, by: usize) -> [[i32; 3]; 16] {
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
fn refine_grey(texels: &[[i32; 3]; 16], block: &mut [u8], allow_punch_through: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Colour block with c0 = pure blue < c1 = pure red, every texel on index 3.
    const COLOR_C0_LT_C1: [u8; 8] = [0x1F, 0x00, 0x00, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF];

    #[test]
    fn dxt5_colour_block_is_always_four_colour() {
        // GPUs ignore the c0/c1 order in DXT3/DXT5: index 3 is (c0 + 2·c1) / 3, never black.
        let mut block = vec![255u8, 255, 0, 0, 0, 0, 0, 0];
        block.extend_from_slice(&COLOR_C0_LT_C1);
        let img = decode(f::DXT5, 4, 4, &block).unwrap();
        assert_eq!(&img.data[..4], &[170, 0, 85, 255]);
    }

    #[test]
    fn dxt3_colour_block_is_always_four_colour() {
        let mut block = vec![0xFFu8; 8];
        block.extend_from_slice(&COLOR_C0_LT_C1);
        let img = decode(f::DXT3, 4, 4, &block).unwrap();
        assert_eq!(&img.data[..4], &[170, 0, 85, 255]);
    }

    #[test]
    fn dxt1_three_colour_mode_keeps_punch_through() {
        let img = decode(f::DXT1, 4, 4, &COLOR_C0_LT_C1).unwrap();
        assert_eq!(&img.data[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn strip_encoding_matches_whole_image() {
        // 128 x 200: several strips, the last one shorter.
        let mut img = Rgba8::new(128, 200);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = (i.wrapping_mul(2_654_435_761) >> 7) as u8;
        }
        for (format, squish, block_len) in [(f::DXT1, texpresso::Format::Bc1, 8), (f::DXT5, texpresso::Format::Bc3, 16)] {
            let mut pixels = img.data.clone();
            if format == f::DXT1 {
                pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
            }
            let params = texpresso::Params {
                algorithm: texpresso::Algorithm::ClusterFit,
                weights: texpresso::COLOUR_WEIGHTS_UNIFORM,
                weigh_colour_by_alpha: false,
            };
            let mut whole = vec![0u8; squish.compressed_size(128, 200)];
            squish.compress(&pixels, 128, 200, params, &mut whole);
            for (i, block) in whole.chunks_exact_mut(block_len).enumerate() {
                let texels = block_texels(&pixels, 128, i % 32 * 4, i / 32 * 4);
                refine_grey(&texels, &mut block[block_len - 8..], format == f::DXT1);
            }
            assert_eq!(encode(format, &img).unwrap(), whole);
        }
    }

    fn dark_grey_noise(w: u32, h: u32) -> Rgba8 {
        // Near-black fabric: greys between 10 and 30.
        let mut img = Rgba8::new(w, h);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let v = 10 + ((i.wrapping_mul(2_654_435_761) >> 11) % 21) as u8;
            p.copy_from_slice(&[v, v, v, 255]);
        }
        img
    }

    #[test]
    fn dark_greys_stay_grey_and_keep_their_brightness() {
        let img = dark_grey_noise(64, 64);
        for format in [f::DXT1, f::DXT5] {
            let dec = decode(format, 64, 64, &encode(format, &img).unwrap()).unwrap();
            let (mut tint, mut shift) = (0i32, 0i64);
            for (s, d) in img.data.chunks_exact(4).zip(dec.data.chunks_exact(4)) {
                tint = tint.max(*d[..3].iter().max().unwrap() as i32 - *d[..3].iter().min().unwrap() as i32);
                shift += (0..3).map(|c| d[c] as i64 - s[c] as i64).sum::<i64>();
            }
            let shift = shift as f64 / (64.0 * 64.0 * 3.0);
            // Grey 565 endpoints may differ by one level between G and R/B.
            assert!(tint <= 2, "format {format:X}: tint {tint}");
            assert!(shift.abs() <= 0.5, "format {format:X}: shift {shift}");
        }
    }

    #[test]
    fn dxt1_ignores_alpha() {
        // Normal maps from DXT5 sources go to DXT1 whatever their alpha: never punch-through.
        let mut img = dark_grey_noise(16, 16);
        img.data.chunks_exact_mut(4).for_each(|p| p[3] = 0);
        let dec = decode(f::DXT1, 16, 16, &encode(f::DXT1, &img).unwrap()).unwrap();
        assert!(dec.data.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn dxt5_alpha_six_value_mode() {
        // a0 = 10 <= a1 = 200: indices 6 and 7 are 0 and 255. Texel 0 -> index 6, texel 1 -> 7.
        let mut block = vec![10u8, 200, 0b1111_1110, 0, 0, 0, 0, 0];
        block.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let img = decode(f::DXT5, 4, 4, &block).unwrap();
        assert_eq!((img.data[3], img.data[7]), (0, 255));
        assert_eq!(&img.data[..3], &[255, 255, 255]);
    }
}

#[cfg(test)]
mod pad_tests {
    use super::*;

    #[test]
    fn non_multiple_of_four_levels_encode_with_the_block_grid_size() {
        let mut img = Rgba8::new(7, 3);
        img.data.iter_mut().for_each(|v| *v = 200);
        for (format, size) in [(f::DXT1, 2 * 8), (f::DXT5, 2 * 16)] {
            assert_eq!(encode(format, &img).unwrap().len(), size);
            assert_eq!(size, level_size(format, 7, 3));
        }
    }
}

#[cfg(test)]
mod dark_tint_tests {
    use super::*;

    #[test]
    fn slightly_tinted_dark_fabric_stays_nearly_grey() {
        // Near-black fabric with a faint, noisy tint (spread up to 12): must not turn green/magenta.
        let mut img = Rgba8::new(64, 64);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            let n = (i.wrapping_mul(2_654_435_761) >> 11) as u32;
            let v = 10 + (n % 21) as i32;
            let off = |k: u32| ((n >> k) % 13) as i32 - 6;
            p.copy_from_slice(&[(v + off(5)).clamp(0, 255) as u8, (v + off(9)).clamp(0, 255) as u8, (v + off(13)).clamp(0, 255) as u8, 255]);
        }
        let mean_tint = |im: &Rgba8| {
            let sum: i64 = im.data.chunks_exact(4).map(|p| (p[..3].iter().max().unwrap() - p[..3].iter().min().unwrap()) as i64).sum();
            sum as f64 / (im.data.len() / 4) as f64
        };
        for format in [f::DXT1, f::DXT5] {
            let dec = decode(format, 64, 64, &encode(format, &img).unwrap()).unwrap();
            // 2.85 with the grey refit limited to near-grey blocks, 1.88 with the dark-block limit.
            assert!(mean_tint(&dec) <= 2.3, "format {format:X}: mean tint {}", mean_tint(&dec));
        }
    }
}
