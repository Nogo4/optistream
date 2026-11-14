//! Stored mip levels: availability, fidelity to the top level, slicing and completion.

use anyhow::{Context, Result};
use optistream_rage::texture::format as f;

use crate::codec::{self, Rgba8};
use crate::format::{self as fmt, chain_size, level_dims, level_size};
use crate::ops::{self, ColorSpace};
use crate::{Encoded, TextureMeta, TextureRole};

/// Number of levels fully present in `len` bytes (some tools store fewer than declared).
pub(super) fn available_levels(meta: &TextureMeta, len: usize) -> u32 {
    let mut total = 0;
    for l in 0..meta.levels.max(1) {
        let (w, h) = level_dims(meta.width, meta.height, l);
        total += level_size(meta.format, w, h);
        if total > len {
            return l.max(1);
        }
    }
    meta.levels.max(1)
}

/// Largest mean per-channel gap (0..255) between a stored mip and a box downscale of
/// level 0, within one brightness band, that slicing still accepts. Faulty source mips
/// measured +4 to +20 (dark areas turning lighter and grey/blue on Oren clothes); DXT noise
/// is zero-mean and stays well under this.
const MIP_COLOR_TOLERANCE: f64 = 2.0;

/// Brightness bands checked separately: a whole-texture mean hides dark areas going grey.
const BAND_EDGES: [u32; 2] = [64, 160];

/// Fewest texels a band needs before its mean is trusted.
const MIN_BAND_TEXELS: f64 = 256.0;

/// Largest mean angle (degrees) between the normals of a stored mip and of a box downscale
/// of level 0 that slicing still accepts. Sound Oren normal maps stay under 3°; the faulty
/// one measured (`hair_normal_007`, background zeroed in its mips) is far above.
const MIP_NORMAL_TOLERANCE_DEG: f64 = 5.0;

/// Whether stored level `level` has the colours of level 0 (safe to promote to the top).
///
/// The stored level is compared with a 2x2 box downscale of level 0, texel by texel. Colour
/// textures: the signed per-channel mean gap is measured in each brightness band of the
/// reference (specular alpha is gloss, so it is compared too and never masks a texel).
/// Normal maps: the mean angle between the normals, as the shader rebuilds them from RG.
pub(super) fn level_matches_top(meta: &TextureMeta, data: &[u8], level: u32, role: TextureRole) -> Result<bool> {
    let cs = role.color_space();
    let top = codec::decode(meta.format, meta.width, meta.height, data).context("decoding texture")?;
    let reference = ops::half_n(&top, level, cs);
    let (w, h) = level_dims(meta.width, meta.height, level);
    let offset = chain_size(meta.format, meta.width, meta.height, level);
    let mip = codec::decode(meta.format, w, h, &data[offset..]).context("decoding texture")?;
    if (reference.width, reference.height) != (w, h) {
        return Ok(false);
    }
    if role == TextureRole::Normal {
        let mip = ops::renormalize_normals(&mip);
        let vector = |p: &[u8]| [0, 1, 2].map(|c| p[c] as f64 / 127.5 - 1.0);
        let total: f64 = reference
            .data
            .chunks_exact(4)
            .zip(mip.data.chunks_exact(4))
            .map(|(r, m)| {
                let (a, b) = (vector(r), vector(m));
                let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let len = (a.iter().map(|v| v * v).sum::<f64>() * b.iter().map(|v| v * v).sum::<f64>()).sqrt();
                (dot / len.max(1e-9)).clamp(-1.0, 1.0).acos().to_degrees()
            })
            .sum();
        return Ok(total / (w * h) as f64 <= MIP_NORMAL_TOLERANCE_DEG);
    }
    let alpha_is_gloss = role == TextureRole::Specular;
    let mut gap = [[0f64; 4]; BAND_EDGES.len() + 1];
    let mut count = [0f64; BAND_EDGES.len() + 1];
    for (r, m) in reference.data.chunks_exact(4).zip(mip.data.chunks_exact(4)) {
        // Transparent texels are not seen: their colour may differ freely.
        if r[3] < 128 && !alpha_is_gloss {
            continue;
        }
        let luma = (r[0] as u32 + r[1] as u32 + r[2] as u32) / 3;
        let band = BAND_EDGES.iter().filter(|&&e| luma >= e).count();
        for c in 0..if alpha_is_gloss { 4 } else { 3 } {
            gap[band][c] += m[c] as f64 - r[c] as f64;
        }
        count[band] += 1.0;
    }
    Ok(gap.iter().zip(count).all(|(g, n)| {
        n < MIN_BAND_TEXELS || g.iter().all(|v| (v / n).abs() <= MIP_COLOR_TOLERANCE)
    }))
}

/// Keeps the stored levels starting at the one matching `(tw, th)`, dropping levels under 4 px.
pub(crate) fn slice(meta: &TextureMeta, data: &[u8], stored: u32, tw: u32, th: u32) -> Option<Encoded> {
    let first = (0..stored).find(|&l| level_dims(meta.width, meta.height, l) == (tw, th))?;
    let mut levels = 0;
    for l in first..stored {
        let (w, h) = level_dims(meta.width, meta.height, l);
        if w.min(h) < 4 && levels > 0 {
            break;
        }
        levels += 1;
    }
    if first == 0 && levels == meta.levels {
        return None;
    }
    let start = chain_size(meta.format, meta.width, meta.height, first);
    let len = chain_size(meta.format, tw, th, levels);
    Some(Encoded {
        width: tw,
        height: th,
        format: meta.format,
        levels,
        stride: fmt::stride(meta.format, tw, th),
        data: data[start..start + len].to_vec(),
        changes: Vec::new(),
    })
}

/// Keeps the stored levels as they are (same dimensions, same format).
pub(super) fn keep_stored(meta: &TextureMeta, data: &[u8], stored: u32) -> Encoded {
    Encoded {
        width: meta.width,
        height: meta.height,
        format: meta.format,
        levels: stored,
        stride: fmt::stride(meta.format, meta.width, meta.height),
        data: data[..chain_size(meta.format, meta.width, meta.height, stored)].to_vec(),
        changes: Vec::new(),
    }
}

/// Generates the missing lower levels from the last stored one.
///
/// Returns `false` when the chain must stay short (format not handled, or punch-through
/// DXT1 alpha that the DXT1 encoder cannot reproduce).
pub(super) fn append_mips(enc: &mut Encoded, want: u32, cs: ColorSpace) -> Result<bool> {
    if enc.format != f::DXT1 && enc.format != f::DXT5 {
        return Ok(false);
    }
    let last = enc.levels - 1;
    let (lw, lh) = level_dims(enc.width, enc.height, last);
    let offset = chain_size(enc.format, enc.width, enc.height, last);
    let img = codec::decode(enc.format, lw, lh, &enc.data[offset..])?;
    if enc.format == f::DXT1 && img.alpha_used() {
        return Ok(false);
    }
    let chain = ops::mip_chain(img, want - last, cs);
    for level in chain.iter().skip(1) {
        enc.data.extend_from_slice(&codec::encode_with(enc.format, level, cs == ColorSpace::AlphaMask)?);
    }
    enc.levels = want;
    Ok(true)
}

/// Whether the mips of a DXT1 texture cannot be completed because its last stored level
/// uses punch-through (1-bit) alpha. `data` holds all stored levels.
pub fn punch_through_blocks_mips(meta: &TextureMeta, data: &[u8]) -> bool {
    if meta.format != f::DXT1 || meta.width < 4 || meta.height < 4 {
        return false;
    }
    let last = available_levels(meta, data.len()) - 1;
    let (lw, lh) = level_dims(meta.width, meta.height, last);
    let offset = chain_size(meta.format, meta.width, meta.height, last);
    codec::decode(meta.format, lw, lh, &data[offset..]).is_ok_and(|img| img.alpha_used())
}

/// Decodes the top level for display.
pub fn decode_top(meta: &TextureMeta, data: &[u8]) -> Result<Rgba8> {
    codec::decode(meta.format, meta.width, meta.height, data)
}
