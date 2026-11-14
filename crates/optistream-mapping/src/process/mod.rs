//! Parsing, analysis and optimisation of one map file (`.ytd`, `.ydr`, `.ydd`).
//!
//! Two stages per file:
//! 1. corrections (format conversion, power of two, complete mips), as for clothing;
//! 2. when the declared physical memory is still above the budget, targeted reduction:
//!    the textures losing the least detail per byte saved are halved one step at a time
//!    until the file fits, within the limits (steps per texture, minimum size, maximum
//!    loss against the original). A file that cannot fit stays above and says why.

mod parse;
mod analyze;
mod budget;
mod optimize;
mod preview;
#[cfg(test)]
mod tests;

pub use parse::{parse, read_drawables};
pub use analyze::analyze;
pub use budget::{plan_file, BlockPlan, FilePlan};
pub use optimize::optimize;
pub use preview::preview;

use optistream_rage::drawable::DrawableInfo;
use optistream_rage::texture::TextureEntry;
use optistream_rage::Resource;
use optistream_texture::TextureMeta;

use crate::roles::Usage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ytd,
    Ydr,
    Ydd,
}

impl Kind {
    pub fn of(file_name_lower: &str) -> Option<Kind> {
        match file_name_lower.rsplit('.').next()? {
            "ytd" => Some(Kind::Ytd),
            "ydr" => Some(Kind::Ydr),
            "ydd" => Some(Kind::Ydd),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ytd => "ytd",
            Kind::Ydr => "ydr",
            Kind::Ydd => "ydd",
        }
    }
}

pub struct Tex {
    pub entry: TextureEntry,
    pub usage: Usage,
    /// Text or graphics read up close (name): kept at its size by the targeted reduction.
    pub readable: bool,
    pub data_off: Option<usize>,
    pub data_len: usize,
}

impl Tex {
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
    pub textures: Vec<Tex>,
}
