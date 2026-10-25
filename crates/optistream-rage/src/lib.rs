//! Low-level access to GTA V Legacy (Gen8) RSC7 resources.
//!
//! OptiStream never rebuilds a resource from a full object model. It parses only the
//! structures it needs, patches fields in place inside the system segment, and fully
//! re-lays out the graphics segment (texture pixel data), recomputing the page flags.

pub mod drawable;
pub mod gfx;
pub mod joaat;
pub mod pages;
pub mod rsc7;
pub mod texture;

pub use rsc7::{PageFlags, Resource, Segment, GRAPHICS_BASE, SYSTEM_BASE};
