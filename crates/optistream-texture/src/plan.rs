//! Per-texture optimisation decision and its execution.
//!
//! Decision (DurtyClothTool logic, `tools/texture-optimizer/README.md`): a texture is only
//! touched when its format needs converting (uncompressed, BC7, DXT3), when it is not a
//! power of two, when its mips are incomplete, or when halving is requested. Compressed
//! textures keep their format (DXT1 stays DXT1, DXT5 stays DXT5). Converted textures: normal
//! maps -> DXT5, others -> DXT5 if the alpha is used, else DXT1.
//!
//! Order of preference: keep the texture untouched, else slice existing mip levels
//! (lossless: dropping the top levels is an exact downscale), else re-encode from the
//! best available level.

use anyhow::{Context, Result};
use optistream_rage::texture::format as f;

use crate::codec::{self, Rgba8};
use crate::format::{self as fmt, chain_size, full_levels, level_dims, level_size};
use crate::ops::{self, ColorSpace};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureRole {
    Diffuse,
    Normal,
    Specular,
    /// Colour texture with no known role (treated like a diffuse).
    Other,
}

impl TextureRole {
    pub fn color_space(self) -> ColorSpace {
        match self {
            TextureRole::Normal => ColorSpace::Normal,
            TextureRole::Diffuse | TextureRole::Specular | TextureRole::Other => ColorSpace::Linear,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TextureRole::Diffuse => "diffuse",
            TextureRole::Normal => "normal",
            TextureRole::Specular => "spec",
            TextureRole::Other => "autre",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TexturePolicy {
    /// Halve both sides (after the power-of-two floor), down to 4 px.
    pub halve: bool,
    /// Convert BC7, DXT3 and uncompressed colour formats (normal maps to DXT5, others to
    /// DXT1, or DXT5 when the alpha is used).
    pub convert_formats: bool,
    /// Complete missing mip levels.
    pub fix_mips: bool,
    /// Resize non power-of-two textures down to the nearest power of two.
    pub force_pot: bool,
    /// The diffuse is drawn only by shaders that use its alpha (cutout, blend, decal, hair):
    /// alpha is a coverage mask, so filtering and DXT5 encoding weigh colour by it.
    pub alpha_mask: bool,
}

/// Texture as stored in the resource.
#[derive(Clone, Copy, Debug)]
pub struct TextureMeta {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub levels: u32,
}

/// Result of a texture optimisation.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub levels: u32,
    pub stride: u16,
    pub data: Vec<u8>,
    /// Human-readable list of the changes (French, shown in the report).
    pub changes: Vec<String>,
}

fn pot_floor(v: u32) -> u32 {
    if v <= 1 {
        1
    } else {
        1 << v.ilog2()
    }
}

/// Target top-level dimensions: power-of-two floor (if requested), then halved (if requested).
pub fn target_dims(meta: &TextureMeta, policy: &TexturePolicy) -> (u32, u32) {
    let (mut w, mut h) = (meta.width, meta.height);
    if policy.force_pot {
        w = pot_floor(w);
        h = pot_floor(h);
    }
    if policy.halve && w.min(h) >= 8 {
        w /= 2;
        h /= 2;
    }
    (w, h)
}

fn is_managed_color(format: u32) -> bool {
    matches!(
        format,
        f::DXT1 | f::DXT3 | f::DXT5 | f::BC7 | f::A8R8G8B8 | f::X8R8G8B8 | f::A8B8G8R8 | f::A16B16G16R16 | f::A1R5G5B5
    )
}

/// Optimises one texture. Returns `None` when nothing should change.
///
/// `data` is the full pixel data of the texture (all stored levels).
pub fn optimize(
    meta: &TextureMeta,
    data: &[u8],
    role: TextureRole,
    policy: &TexturePolicy,
) -> Result<Option<Encoded>> {
    if fmt::layout(meta.format).is_none() || meta.width < 4 || meta.height < 4 {
        return Ok(None);
    }
    let (tw, th) = target_dims(meta, policy);
    let want_levels = full_levels(tw, th);
    let stored_levels = available_levels(meta, data.len());

    // Single-channel formats (decals, masks) keep their format: only lossless slicing.
    if !is_managed_color(meta.format) {
        return Ok(slice(meta, data, stored_levels, tw, th).map(|enc| with_change_list(meta, enc)));
    }

    let cs = match role {
        TextureRole::Diffuse | TextureRole::Other if policy.alpha_mask => ColorSpace::AlphaMask,
        _ => role.color_space(),
    };
    let mut alpha_cache: Option<bool> = None;
    let mut alpha_used = |meta: &TextureMeta| -> Result<bool> {
        if let Some(a) = alpha_cache {
            return Ok(a);
        }
        let img = codec::decode(meta.format, meta.width, meta.height, data)?;
        let a = img.alpha_used();
        alpha_cache = Some(a);
        Ok(a)
    };

    // Compressed textures keep their format; only the others are converted.
    let target_format = match meta.format {
        f::DXT1 | f::DXT5 => meta.format,
        other if !policy.convert_formats => other,
        _ if role == TextureRole::Normal => f::DXT5,
        _ if alpha_used(meta)? => f::DXT5,
        _ => f::DXT1,
    };

    let same_size = tw == meta.width && th == meta.height;
    let mips_ok =!policy.fix_mips || stored_levels >= want_levels;
    if same_size && target_format == meta.format && mips_ok {
        return Ok(None);
    }

    // Stored mips made in linear light (or otherwise off) are brighter than the top level:
    // promoting one to the top would show as lighter clothes (normal maps: bent or zeroed
    // normals). Such textures are rebuilt from level 0 instead of being sliced.
    let first_level = (0..stored_levels).find(|&l| level_dims(meta.width, meta.height, l) == (tw, th));
    let stored_mips_ok = match first_level {
        Some(l) if l > 0 => level_matches_top(meta, data, l, role)?,
        _ => true,
    };

    if target_format == meta.format && stored_mips_ok {
        let sliced = slice(meta, data, stored_levels, tw, th);
        let sliced_any = sliced.is_some();
        // Nothing to drop but mips to add: keep every stored level bit-exact rather than
        // decoding and re-compressing the whole texture.
        let base = sliced.or_else(|| same_size.then(|| keep_stored(meta, data, stored_levels)));
        if let Some(mut enc) = base {
            let mut blocked = false;
            if policy.fix_mips && enc.levels < want_levels {
                blocked = !append_mips(&mut enc, want_levels, cs)?;
            }
            if sliced_any || enc.levels != meta.levels {
                return Ok(Some(with_change_list(meta, enc)));
            }
            if blocked {
                // Punch-through DXT1 that only lacks mips: the encoder has no 1-bit alpha
                // mode, and re-encoding to DXT5 would double the size. Keep it bit-exact.
                return Ok(None);
            }
        }
    }

    // Punch-through DXT1 alpha cannot be re-encoded as DXT1: move such textures to DXT5.
    let target_format = if meta.format == f::DXT1
        && target_format == f::DXT1
        && role != TextureRole::Normal
        && alpha_used(meta)?
    {
        f::DXT5
    } else {
        target_format
    };
    if target_format != f::DXT1 && target_format != f::DXT5 {
        // Format conversion disabled and no lossless path: leave it alone.
        return Ok(None);
    }

    // Re-encode from the smallest stored level that is still at least the target size
    // (from level 0 when the stored mips cannot be trusted).
    let src_level = (0..stored_levels)
        .rev()
        .find(|&l| {
            let (w, h) = level_dims(meta.width, meta.height, l);
            w >= tw && h >= th && (l == 0 || stored_mips_ok)
        })
        .unwrap_or(0);
    let (sw, sh) = level_dims(meta.width, meta.height, src_level);
    let offset = chain_size(meta.format, meta.width, meta.height, src_level);
    let src = codec::decode(meta.format, sw, sh, &data[offset..]).context("decoding texture")?;
    let top = ops::resize(&src, tw, th, cs);
    let levels = if policy.fix_mips { want_levels } else { stored_levels.min(want_levels).max(1) };
    let mut out = Vec::with_capacity(chain_size(target_format, tw, th, levels));
    for img in ops::mip_chain(top, levels, cs) {
        out.extend_from_slice(&codec::encode_with(target_format, &img, cs == ColorSpace::AlphaMask)?);
    }
    let enc = Encoded {
        width: tw,
        height: th,
        format: target_format,
        levels,
        stride: fmt::stride(target_format, tw, th),
        data: out,
        changes: Vec::new(),
    };
    Ok(Some(with_change_list(meta, enc)))
}

/// Number of levels fully present in `len` bytes (some tools store fewer than declared).
fn available_levels(meta: &TextureMeta, len: usize) -> u32 {
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
fn level_matches_top(meta: &TextureMeta, data: &[u8], level: u32, role: TextureRole) -> Result<bool> {
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
fn slice(meta: &TextureMeta, data: &[u8], stored: u32, tw: u32, th: u32) -> Option<Encoded> {
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
fn keep_stored(meta: &TextureMeta, data: &[u8], stored: u32) -> Encoded {
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
fn append_mips(enc: &mut Encoded, want: u32, cs: ColorSpace) -> Result<bool> {
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

fn with_change_list(meta: &TextureMeta, mut enc: Encoded) -> Encoded {
    if (enc.width, enc.height) != (meta.width, meta.height) {
        enc.changes.push(format!("{}×{} → {}×{}", meta.width, meta.height, enc.width, enc.height));
    }
    if enc.format != meta.format {
        enc.changes.push(format!("{} → {}", fmt::name(meta.format), fmt::name(enc.format)));
    }
    if enc.levels != meta.levels {
        enc.changes.push(format!("mips {} → {}", meta.levels, enc.levels));
    }
    enc
}

/// Decodes the top level for display.
pub fn decode_top(meta: &TextureMeta, data: &[u8]) -> Result<Rgba8> {
    codec::decode(meta.format, meta.width, meta.height, data)
}
