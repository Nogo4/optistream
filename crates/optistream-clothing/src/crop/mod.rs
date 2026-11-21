//! UV cropping: textures cut down to the part of the sheet the meshes actually sample, UVs
//! rewritten so that the garment looks the same.
//!
//! A drawable, its first-person model (`_1.ydd`) and every diffuse variation (`.ytd`) share
//! one UV layout: they form a group, cropped with the same window or not at all. The window
//! is a power-of-two fraction of each texture, aligned on 4×4 blocks, so the top level of a
//! DXT texture is copied block for block (lossless) and stays a power of two.

mod texture;
mod groups;
mod plan;
mod uv;
#[cfg(test)]
mod tests;

pub use texture::{crop_texture, exempt};
pub use groups::{diagnose, plan_for, plan_run};
pub(crate) use groups::drawable_of;
pub use uv::remap_uvs;

use std::collections::HashMap;
use std::sync::Arc;

/// Part of a texture kept on one axis, in UV units: `size` is a power of two (1 = whole axis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub start: f64,
    pub size: f64,
}

impl Axis {
    const FULL: Axis = Axis { start: 0.0, size: 1.0 };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub u: Axis,
    pub v: Axis,
}

impl Window {
    /// Kept share of the texture area.
    pub fn area(&self) -> f64 {
        self.u.size * self.v.size
    }

    /// Pixel rectangle `(x0, y0, width, height)` of a `w`×`h` texture (V grows downwards, D3D).
    pub fn rect(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let px = |a: &Axis, n: u32| ((a.start * n as f64).round() as u32, (a.size * n as f64).round() as u32);
        let ((x0, cw), (y0, ch)) = (px(&self.u, w), px(&self.v, h));
        (x0, y0, cw, ch)
    }

    fn map(&self, uv: [f32; 2]) -> [f32; 2] {
        [
            ((uv[0] as f64 - self.u.start) / self.u.size) as f32,
            ((uv[1] as f64 - self.v.start) / self.v.size) as f32,
        ]
    }

    pub fn describe(&self) -> String {
        optistream_i18n::tf!("clothing.crop.window", u = format!("{:.0}", self.u.size * 100.0), v = format!("{:.0}", self.v.size * 100.0))
    }
}

/// Files sharing one UV layout, and the window they are cropped with.
#[derive(Debug)]
pub struct Group {
    /// Lowercased relative paths of every member.
    pub files: Vec<String>,
    pub window: Window,
    /// Largest declared physical memory among the members: the memory threshold applies to
    /// the group as a whole (all of it is processed, or none).
    pub max_physical: u64,
}

/// Group of every cropped file, by lowercased relative path.
pub type RunPlan = HashMap<String, Arc<Group>>;

/// Shaders whose every texture is sampled with TEXCOORD0 and no UV transform (no detail
/// map, palette, wrinkle mask, cloth or emissive special case).
const CROP_SHADERS: &[&str] = &[
    "ped", "ped_default", "ped_default_mp", "ped_alpha", "ped_default_cutout", "ped_hair_cutout_alpha",
    "ped_hair_spiked", "ped_nopeddamagedecals", "default",
];

/// Placeholders bound to unused samplers: sampling them with other UVs changes nothing.
const PLACEHOLDERS: &[&str] = &["givemechecker", "none"];

/// Samplers read without UVs: a palette is looked up by the diffuse value, not by position.
const UV_FREE_SAMPLERS: &[&str] = &["TextureSamplerDiffPal", "TintPaletteSampler"];

/// UV slack accepted outside `[0, 1]` (export rounding); beyond, the texture tiles.
const UV_EPSILON: f64 = 1e-3;

/// Uniform textures up to this side (placeholders, flat maps) look the same whatever the
/// UVs: they are left whole and do not constrain the window.
const TINY_SIDE: u32 = 8;

/// Largest per-channel spread of a texture still considered uniform.
const UNIFORM_TOLERANCE: u8 = 2;
