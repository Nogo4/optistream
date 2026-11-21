//! Parsing, analysis and optimisation of one vehicle file (`.ytd`, `.yft`, `.ydr`, `.ydd`).
//!
//! Two stages per file, as for the mappings (shared engine, `optistream_texture::budget`):
//! 1. corrections (format conversion, power of two, complete mips);
//! 2. when the declared physical memory is still above the budget, targeted reduction
//!    with the vehicle priorities ([`Class`]).

mod parse;
mod analyze;
mod budget;
mod geometry;
mod optimize;
mod preview;
#[cfg(test)]
mod tests;

pub use parse::{parse, read_drawables, ytd_names};
pub use analyze::analyze;
pub use budget::plan_file;
pub use optimize::optimize;
pub use preview::preview;

use std::collections::HashSet;

use optistream_i18n::t;
use optistream_mapping::roles::Usage;
use optistream_rage::drawable::DrawableInfo;
use optistream_rage::texture::TextureEntry;
use optistream_rage::Resource;
use optistream_texture::TextureMeta;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ytd,
    Yft,
    Ydr,
    Ydd,
}

impl Kind {
    pub fn of(file_name_lower: &str) -> Option<Kind> {
        match file_name_lower.rsplit('.').next()? {
            "ytd" => Some(Kind::Ytd),
            "yft" => Some(Kind::Yft),
            "ydr" => Some(Kind::Ydr),
            "ydd" => Some(Kind::Ydd),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ytd => "ytd",
            Kind::Yft => "yft",
            Kind::Ydr => "ydr",
            Kind::Ydd => "ydd",
        }
    }
}

/// What the reduction may do with a texture, in priority order of the design: a copy in
/// the `+hi.ytd` first, then lights / glass by name, then liveries and big textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Base copy of a texture of the neighbouring `+hi.ytd`: only seen from afar.
    HiCopy,
    /// Lights, sirens, glass (name) or unknown role: last, only when forcing.
    Last,
    /// Livery sheet: reduced first, never under 1024 px.
    Livery,
    /// Smaller side ≥ 1024 px.
    Big,
    /// Everything else: only with "Reduce small textures too".
    Small,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::HiCopy => t("vehicle.class.hi_copy"),
            Class::Last => t("vehicle.class.last"),
            Class::Livery => t("vehicle.class.livery"),
            Class::Big => t("vehicle.class.big"),
            Class::Small => t("vehicle.class.small"),
        }
    }
}

/// Context of a file beyond its bytes.
#[derive(Clone, Debug, Default)]
pub struct FileInfo {
    /// Lowercased texture names of the neighbouring `+hi.ytd` (for a base `.ytd`).
    pub hi_names: HashSet<String>,
    /// The file is a livery mod part (`*_liv_*.yft`, `*livery*.yft`).
    pub livery_file: bool,
}

pub struct Tex {
    pub entry: TextureEntry,
    pub usage: Usage,
    /// A livery, whatever its class (hard floor of 1024 px).
    pub livery: bool,
    pub class: Class,
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

    pub fn label(&self) -> String {
        match self.usage {
            Usage::Protected => t("vehicle.tex.protected").to_owned(),
            Usage::Role(r) => format!("{}, {}", r.label(), self.class.label()),
        }
    }
}

pub struct Parsed {
    pub res: Resource,
    pub kind: Kind,
    pub drawables: Vec<DrawableInfo>,
    pub textures: Vec<Tex>,
}
