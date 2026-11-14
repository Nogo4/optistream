//! Decision and execution: keep, slice the stored mips, or re-encode from the best level.

use anyhow::{Context, Result};
use optistream_rage::texture::format as f;

use crate::format::{self as fmt, chain_size, full_levels, level_dims};
use crate::ops::{self, ColorSpace};
use crate::plan::mips::{append_mips, available_levels, keep_stored, level_matches_top, slice};
use crate::{codec, Encoded, TextureMeta, TexturePolicy, TextureRole};

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
    for _ in 0..policy.halve as u32 + policy.halvings {
        if w.min(h) < 8 {
            break;
        }
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
