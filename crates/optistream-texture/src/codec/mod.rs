//! Pixel decoding to RGBA8 and encoding to the vanilla output formats.

mod decode;
mod encode;
mod refine;
#[cfg(test)]
mod tests;

pub use decode::decode;
pub use encode::{encode, encode_with};

use optistream_rage::texture::format as f;

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
            | f::A8B8G8R8 | f::A16B16G16R16 | f::A1R5G5B5 | f::A8 | f::L8
    )
}
