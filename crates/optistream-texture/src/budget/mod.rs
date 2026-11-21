//! Memory budget of one resource: corrections, then the cheapest halvings until the
//! declared physical memory fits.
//!
//! Shared by the methods that rewrite whole texture sets (mappings, vehicles). The engine
//! knows nothing about resource types: each method describes every texture with a
//! [`Rule`] (its tier, its floors, whether it may be reduced at all) and the engine plans:
//! 1. corrections only (format conversion, power of two, complete mips);
//! 2. while the file is above the budget, the halving that loses the least detail per byte
//!    saved, lowest tier first, steps within the limits before any step past them.
//!
//! The plan is checked with the real packer ([`optistream_rage::gfx::packed_size`]) and
//! written with [`apply`].

mod plan;
mod write;
#[cfg(test)]
mod tests;

pub use plan::plan_file;
pub use write::apply;

use optistream_i18n::tf;
use optistream_rage::texture::TextureEntry;

use crate::format::{chain_size, full_levels};
use crate::{Encoded, TextureMeta, TexturePolicy, TextureRole};

/// Why a texture keeps its size, for the warning of a file left above the budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    /// Catalogue key of the reason (`"texture.hold.protected"`), translated when the warning
    /// is built; text that is not a key is shown as is.
    pub label: &'static str,
    /// Order of the reasons in the warning (lowest first).
    pub rank: u8,
}

/// Holds every method shares.
pub const HOLD_PROTECTED: Hold = Hold { label: "texture.hold.protected", rank: 20 };

/// When a texture may be halved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Within the limits, then past them when forcing.
    Normal,
    /// Only when forcing (every step counts as forced); otherwise kept, for this reason.
    ForceOnly(Hold),
    /// Never; kept for this reason.
    Never(Hold),
}

/// How the engine may reduce one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub access: Access,
    /// Lower tiers are spent first (within the limits, then past them).
    pub tier: u8,
    /// Smaller side floor within the limits.
    pub min_size: u32,
    /// Halvings within the limits.
    pub max_steps: u32,
    /// Smaller side never reached, even when forcing (0: none).
    pub hard_floor: u32,
    /// Reason given when the hard floor stops the texture.
    pub hard_floor_hold: Option<Hold>,
    /// Steps within the limits whatever their loss (the hard floor is the quality bar).
    pub ignore_loss: bool,
}

/// One texture of the file as the engine sees it.
pub struct BudgetTex<'a> {
    pub entry: &'a TextureEntry,
    pub data_off: Option<usize>,
    pub data_len: usize,
    /// `None`: never modified (emissive, palette, lookup...).
    pub role: Option<TextureRole>,
    pub rule: Rule,
}

impl BudgetTex<'_> {
    pub fn meta(&self) -> TextureMeta {
        TextureMeta {
            width: self.entry.width as u32,
            height: self.entry.height as u32,
            format: self.entry.format,
            levels: self.entry.levels as u32,
        }
    }
}

/// Settings of a whole run.
#[derive(Clone, Debug)]
pub struct Limits {
    pub enabled: bool,
    pub budget: u64,
    pub max_loss: f64,
    pub force: bool,
    /// Floors of the forced reduction.
    pub force_min_size: u32,
    pub force_max_steps: u32,
    /// Policy of `halvings` halvings on top of the corrections.
    pub policy: TexturePolicy,
}

impl Limits {
    pub fn texture_policy(&self, halvings: u32) -> TexturePolicy {
        TexturePolicy { halvings, ..self.policy.clone() }
    }
}

/// One unique pixel block and what the plan does with it.
pub struct BlockPlan {
    pub off: usize,
    /// Indices into the textures given to [`plan_file`] sharing this block.
    pub users: Vec<usize>,
    /// New content, `None` to keep the stored bytes.
    pub enc: Option<Encoded>,
    /// Halvings applied by the targeted reduction.
    pub steps: u32,
    /// Measured loss of those halvings (percent, against the original).
    pub loss: f64,
    /// Whether the reduction went past the configured limits ("Forcer sous le budget").
    pub forced: bool,
}

pub struct FilePlan {
    pub blocks: Vec<BlockPlan>,
    pub warnings: Vec<String>,
    /// Declared physical memory after the plan.
    pub physical_after: u64,
}

/// Targeted reduction candidate.
pub(crate) struct Candidate {
    pub(crate) block: usize,
    /// Format, dimensions and levels after the corrections (stage 1).
    pub(crate) format: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) levels: u32,
    /// `loss[k]` = loss of `k` halvings (index 0 = 0.0); only admissible steps are listed.
    pub(crate) loss: Vec<f64>,
    /// `key[k]` of step `k`: the rule's tier, plus [`PAST`] once past the limits. Lower keys
    /// are always spent first.
    pub(crate) key: Vec<u8>,
}

/// Added to the tier of a step past the limits: every such step comes after all steps within.
pub(crate) const PAST: u8 = 4;

impl Candidate {
    pub(crate) fn size(&self, steps: u32, fix_mips: bool) -> u64 {
        let (w, h) = (self.width >> steps, self.height >> steps);
        let levels = if fix_mips { full_levels(w, h) } else { self.levels.saturating_sub(steps).max(1).min(full_levels(w, h)) };
        chain_size(self.format, w, h, levels) as u64
    }
}

pub(crate) fn mib(bytes: u64) -> String {
    tf!("core.size.mib", size = format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)))
}
