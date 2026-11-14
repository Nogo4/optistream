//! OptiStream method for addon clothing and EUP packs.
//!
//! Handles `.ydd` (meshes + embedded normal/spec) and `.ytd` (diffuse variations).
//! Never renames files, never renumbers drawables, never touches `.ymt`, `.yld`,
//! palettes, render targets, emissive textures or cloth geometry.

pub mod config;
pub mod crop;
pub mod naming;
pub mod process;
pub mod shaders;
mod method;

pub use method::ClothingMethod;

/// Drawables above this index per component are invisible to other players.
pub const MAX_DRAWABLES_PER_COMPONENT: usize = 128;
