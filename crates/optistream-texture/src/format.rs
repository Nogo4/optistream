use optistream_rage::texture::format as f;

/// Bytes per 4x4 block for block-compressed formats, bytes per pixel otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Block(usize),
    Pixel(usize),
}

pub fn layout(format: u32) -> Option<Layout> {
    Some(match format {
        f::DXT1 | f::ATI1 => Layout::Block(8),
        f::DXT3 | f::DXT5 | f::ATI2 | f::BC7 => Layout::Block(16),
        f::A8R8G8B8 | f::X8R8G8B8 | f::A8B8G8R8 => Layout::Pixel(4),
        f::A16B16G16R16 => Layout::Pixel(8),
        f::A1R5G5B5 => Layout::Pixel(2),
        f::A8 | f::L8 => Layout::Pixel(1),
        _ => return None,
    })
}

pub fn name(format: u32) -> String {
    match format {
        f::DXT1 => "DXT1".into(),
        f::DXT3 => "DXT3".into(),
        f::DXT5 => "DXT5".into(),
        f::ATI1 => "ATI1".into(),
        f::ATI2 => "ATI2".into(),
        f::BC7 => "BC7".into(),
        f::A8R8G8B8 => "A8R8G8B8".into(),
        f::X8R8G8B8 => "X8R8G8B8".into(),
        f::A8B8G8R8 => "A8B8G8R8".into(),
        f::A16B16G16R16 => "A16B16G16R16".into(),
        f::A1R5G5B5 => "A1R5G5B5".into(),
        f::A8 => "A8".into(),
        f::L8 => "L8".into(),
        other => format!("0x{other:08X}"),
    }
}

/// Exact byte size of one mip level.
pub fn level_size(format: u32, width: u32, height: u32) -> usize {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    match layout(format) {
        Some(Layout::Block(bs)) => w.div_ceil(4) * h.div_ceil(4) * bs,
        Some(Layout::Pixel(bpp)) => w * h * bpp,
        None => 0,
    }
}

pub fn level_dims(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

/// Byte size of the first `levels` mip levels.
pub fn chain_size(format: u32, width: u32, height: u32, levels: u32) -> usize {
    (0..levels)
        .map(|l| {
            let (w, h) = level_dims(width, height, l);
            level_size(format, w, h)
        })
        .sum()
}

/// Byte size the game itself computes for a chain (`stride * height`, quartered per level).
pub fn game_chain_size(stride: u32, height: u32, levels: u32) -> usize {
    let mut len = stride as usize * height as usize;
    let mut total = 0;
    for _ in 0..levels.max(1) {
        total += len;
        len /= 4;
    }
    total
}

/// `Stride` field value: bytes of the top level divided by its height.
pub fn stride(format: u32, width: u32, height: u32) -> u16 {
    (level_size(format, width, height) / height.max(1) as usize) as u16
}

/// Vanilla mip chains stop when the smallest side reaches 4 px (`log2(min) - 1` levels).
pub fn full_levels(width: u32, height: u32) -> u32 {
    let min = width.min(height).max(1);
    if min < 4 {
        return 1;
    }
    min.ilog2() - 1
}

pub fn is_pot(v: u32) -> bool {
    v.is_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_formats_round_up_to_whole_blocks() {
        assert_eq!(level_size(f::DXT1, 4, 4), 8);
        assert_eq!(level_size(f::DXT1, 1, 1), 8);
        assert_eq!(level_size(f::DXT1, 5, 4), 16);
        assert_eq!(level_size(f::DXT5, 8, 8), 64);
        assert_eq!(level_size(f::BC7, 2, 2), 16);
    }

    #[test]
    fn pixel_formats_use_bytes_per_pixel() {
        assert_eq!(level_size(f::A8R8G8B8, 3, 5), 60);
        assert_eq!(level_size(f::A16B16G16R16, 2, 2), 32);
        assert_eq!(level_size(f::A1R5G5B5, 2, 2), 8);
        assert_eq!(level_size(f::L8, 7, 3), 21);
        // A zero side counts as one pixel.
        assert_eq!(level_size(f::A8, 0, 0), 1);
    }

    #[test]
    fn unknown_formats_have_no_layout() {
        assert_eq!(layout(0x1234), None);
        assert_eq!(level_size(0x1234, 64, 64), 0);
        assert_eq!(name(0x1234), "0x00001234");
        assert_eq!(name(f::DXT5), "DXT5");
    }

    #[test]
    fn mip_dimensions_stop_at_one_pixel() {
        assert_eq!(level_dims(256, 64, 0), (256, 64));
        assert_eq!(level_dims(256, 64, 3), (32, 8));
        assert_eq!(level_dims(256, 64, 7), (2, 1));
        assert_eq!(level_dims(256, 64, 20), (1, 1));
    }

    #[test]
    fn chain_size_sums_levels() {
        assert_eq!(chain_size(f::DXT1, 16, 16, 1), 128);
        // 16² + 8² + 4² DXT1 blocks.
        assert_eq!(chain_size(f::DXT1, 16, 16, 3), 128 + 32 + 8);
        assert_eq!(chain_size(f::DXT1, 16, 16, 0), 0);
    }

    #[test]
    fn vanilla_chains_stop_at_4_px() {
        assert_eq!(full_levels(2048, 2048), 10);
        assert_eq!(full_levels(512, 128), 6);
        assert_eq!(full_levels(4, 4), 1);
        assert_eq!(full_levels(2, 256), 1);
        // The last level of a full chain is 4 px on its smaller side.
        let n = full_levels(1024, 256);
        assert_eq!(level_dims(1024, 256, n - 1).1, 4);
    }

    #[test]
    fn stride_and_game_chain_size_agree_with_exact_sizes_on_pot_textures() {
        assert_eq!(stride(f::DXT1, 256, 256), 128);
        assert_eq!(stride(f::A8R8G8B8, 100, 10), 400);
        let levels = full_levels(256, 256);
        let s = stride(f::DXT1, 256, 256) as u32;
        assert_eq!(game_chain_size(s, 256, levels), chain_size(f::DXT1, 256, 256, levels));
        // Zero levels still count the top level.
        assert_eq!(game_chain_size(s, 256, 0), chain_size(f::DXT1, 256, 256, 1));
    }

    #[test]
    fn powers_of_two() {
        assert!(is_pot(1) && is_pot(4096));
        assert!(!is_pot(0) && !is_pot(300));
    }
}
