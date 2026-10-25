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
