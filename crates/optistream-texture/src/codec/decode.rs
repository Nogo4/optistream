//! Decoding of every supported pixel format (DXT, BC7, ATI, uncompressed) to RGBA8.

use anyhow::{bail, ensure, Result};
use optistream_rage::texture::format as f;

use crate::format::level_size;
use crate::Rgba8;

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
        f::A16B16G16R16 => {
            // Little-endian 16-bit channels in R, G, B, A order, rounded to 8 bits.
            for (dst, s) in img.data.iter_mut().zip(data.chunks_exact(2)) {
                let v = u16::from_le_bytes([s[0], s[1]]) as u32;
                *dst = ((v * 255 + 32767) / 65535) as u8;
            }
        }
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

pub(super) fn rgb565(v: u16) -> [u8; 3] {
    let (r, g, b) = ((v >> 11) & 0x1F, (v >> 5) & 0x3F, v & 0x1F);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// Palette of a DXT colour block. `allow_punch_through` is only true for DXT1: DXT3 and
/// DXT5 always interpolate four colours, whatever the endpoint order.
pub(super) fn color_palette(block: &[u8], allow_punch_through: bool) -> [[u8; 4]; 4] {
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
