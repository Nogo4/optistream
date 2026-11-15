//! OptiStream method for vehicles.
//!
//! Handles `.ytd` and the textures embedded in `.yft` (liveries, mod parts), `.ydr` and
//! `.ydd`. Same two stages as the mappings (corrections, then targeted reduction under
//! the memory budget) with vehicle priorities: big textures first (liveries, body sheets,
//! base copies of the `+hi.ytd` textures), small parts kept, lights and glass last, and
//! liveries never below 1024 px. Never touches meshes, `.meta`, texture names or counts,
//! and never reads encrypted (FXAP) files.
//!
//! The folder given can be one resource or a whole category (`[oren-vehicles]`): every
//! folder holding a `fxmanifest.lua` is a resource of its own.

pub mod config;
pub mod process;
pub mod roles;
mod method;

pub use method::VehicleMethod;
