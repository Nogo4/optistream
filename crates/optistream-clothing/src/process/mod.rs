//! Parsing, analysis and optimisation of one clothing YDD/YTD.

mod parse;
mod analyze;
mod meshes;
mod textures;
mod optimize;
mod preview;
#[cfg(test)]
mod tests;

pub use parse::{crop_ready, drawable_alpha_mask, parse};
pub use analyze::analyze;
pub use optimize::optimize;
pub use preview::preview;

use optistream_rage::drawable::DrawableInfo;
use optistream_rage::texture::TextureEntry;
use optistream_rage::Resource;
use optistream_texture::TextureMeta;

use crate::crop::Window;
use crate::shaders::Usage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ytd,
    Ydd,
}

/// What the file name and its siblings tell about a file.
#[derive(Clone, Debug, Default)]
pub struct FileInfo {
    /// Component or prop slot (`jbib`, `p_head`, ...).
    pub slot: String,
    /// Cloth piece (`.yld` next to the drawable): meshes are never touched.
    pub cloth: bool,
    /// Diffuse variation of a skin-dependent (`_r`) drawable: dimensions are kept.
    pub skin: bool,
    /// Diffuse variation (`*_diff_*.ytd`): every texture is a diffuse, whatever its name.
    pub variation: bool,
    /// Variation whose drawable only draws its diffuses with alpha shaders (see
    /// [`shaders::all_diffuse_shaders_use_alpha`](crate::shaders::all_diffuse_shaders_use_alpha)).
    pub alpha_mask: bool,
    /// UV crop window of the file's group (see [`crop`](crate::crop)): textures are cut to it and, in a
    /// `.ydd`, TEXCOORD0 is rewritten into it.
    pub crop: Option<Window>,
}

pub struct TexItem {
    pub entry: TextureEntry,
    pub usage: Usage,
    /// Diffuse alpha is a coverage mask (only alpha shaders draw it).
    pub alpha_mask: bool,
    /// Pixel data location in the graphics segment.
    pub data_off: Option<usize>,
    pub data_len: usize,
}

impl TexItem {
    pub fn meta(&self) -> TextureMeta {
        TextureMeta {
            width: self.entry.width as u32,
            height: self.entry.height as u32,
            format: self.entry.format,
            levels: self.entry.levels as u32,
        }
    }
}

pub struct Parsed {
    pub res: Resource,
    pub kind: Kind,
    pub drawables: Vec<DrawableInfo>,
    pub textures: Vec<TexItem>,
}

pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- meshes
