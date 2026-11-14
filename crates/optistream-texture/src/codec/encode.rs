//! DXT1/DXT5 encoding (squish), strip by strip, padded to the block grid.

use anyhow::{bail, Result};
use optistream_rage::texture::format as f;
use rayon::prelude::*;

use crate::codec::refine::{block_has_hidden_texels, block_texels, refine_grey, STRIP_ROWS};
use crate::Rgba8;

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
