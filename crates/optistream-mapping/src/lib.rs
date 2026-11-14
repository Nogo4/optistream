//! OptiStream method for mappings and MLOs.
//!
//! Handles `.ytd` and the textures embedded in `.ydr` / `.ydd`. Never touches `.ymap`,
//! `.ytyp`, `.ybn`, `.yft`, meshes, texture names or counts, and never reads encrypted
//! (FXAP) files.
//!
//! The folder given can be one resource or a whole category (`[oren-mappings]`): every
//! folder holding a `fxmanifest.lua` (or `__resource.lua`) is a resource of its own, and
//! texture roles are looked up within the resource of each file only.

pub mod config;
pub mod process;
pub mod roles;
mod method;

pub use method::MappingMethod;
