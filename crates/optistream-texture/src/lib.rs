//! Texture processing for RAGE (Legacy) textures: decode, resample, mips, DXT encode.

#[cfg(test)]
mod tests;

pub mod codec;
pub mod format;
pub mod ops;
pub mod plan;
pub mod quality;
pub mod resource;

pub use codec::Rgba8;
pub use plan::{optimize, Encoded, TextureMeta, TexturePolicy, TextureRole};
