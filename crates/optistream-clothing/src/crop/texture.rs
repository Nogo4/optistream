//! Cropping one texture to the window, and textures too small or uniform to matter.

use anyhow::{ensure, Result};
use optistream_i18n::t;
use optistream_texture::format::{self as fmt, Layout};
use optistream_texture::TextureMeta;

use crate::crop::{TINY_SIDE, UNIFORM_TOLERANCE, Window};

/// Whether a texture can be left whole although the UVs change: small and uniform.
pub fn exempt(meta: &TextureMeta, data: &[u8]) -> bool {
    if meta.width.max(meta.height) > TINY_SIDE {
        return false;
    }
    let Ok(img) = optistream_texture::codec::decode(meta.format, meta.width, meta.height, data) else {
        return false;
    };
    (0..4).all(|c| {
        let values = img.data.iter().skip(c).step_by(4);
        let (lo, hi) = values.fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        hi - lo <= UNIFORM_TOLERANCE
    })
}

/// Top level of a texture cut to the window (block for block for DXT), or `None` for small
/// uniform textures, which are left whole ([`exempt`]). `data` holds at least the top level.
pub fn crop_texture(meta: &TextureMeta, data: &[u8], w: &Window) -> Result<Option<(TextureMeta, Vec<u8>)>> {
    if exempt(meta, data) {
        return Ok(None);
    }
    let (x0, y0, cw, ch) = w.rect(meta.width, meta.height);
    ensure!(x0 + cw <= meta.width && y0 + ch <= meta.height && cw > 0 && ch > 0, t("clothing.crop.window_outside"));
    let (unit, bytes) = match fmt::layout(meta.format) {
        Some(Layout::Block(b)) => (4, b),
        Some(Layout::Pixel(b)) => (1, b),
        None => anyhow::bail!(t("clothing.crop.unsupported_format")),
    };
    ensure!(x0 % unit == 0 && y0 % unit == 0 && cw % unit == 0 && ch % unit == 0, t("clothing.crop.window_unaligned"));
    let row = (meta.width / unit) as usize * bytes;
    let (cols, rows) = ((cw / unit) as usize, (ch / unit) as usize);
    ensure!(data.len() >= row * (meta.height / unit) as usize, t("clothing.crop.truncated_data"));
    let mut out = Vec::with_capacity(cols * rows * bytes);
    for r in 0..rows {
        let start = (y0 / unit) as usize * row + r * row + (x0 / unit) as usize * bytes;
        out.extend_from_slice(&data[start..start + cols * bytes]);
    }
    Ok(Some((TextureMeta { width: cw, height: ch, format: meta.format, levels: 1 }, out)))
}
