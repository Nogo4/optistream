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

mod optimize;
mod mips;

pub use optimize::{optimize, target_dims};
pub use mips::{decode_top, punch_through_blocks_mips};

use crate::ops::ColorSpace;

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
    /// Further halvings applied after `halve` (targeted reduction of a whole file to a
    /// memory budget). Each one is skipped once the smaller side would drop under 4 px.
    pub halvings: u32,
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
