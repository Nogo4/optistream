//! Declared sizes of a resource file and the FiveM streaming warning threshold.

use serde::Serialize;

/// FiveM starts warning about streamed assets above this declared size.
pub const FIVEM_WARN_BYTES: u64 = 16 * 1024 * 1024;

/// Declared memory of a file, read from its RSC7 header (no decompression).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct SizeInfo {
    pub disk: u64,
    pub system: u64,
    pub graphics: u64,
}

impl SizeInfo {
    pub fn of(bytes: &[u8]) -> Self {
        let mut s = SizeInfo { disk: bytes.len() as u64, ..Default::default() };
        if optistream_rage::Resource::is_rsc7(bytes) {
            let flag = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
            s.system = optistream_rage::PageFlags(flag(8)).size();
            s.graphics = optistream_rage::PageFlags(flag(12)).size();
        }
        s
    }

    /// Whether FiveM prints an "oversized asset" style warning for this file.
    pub fn fivem_warning(&self) -> bool {
        self.graphics > FIVEM_WARN_BYTES || self.system > FIVEM_WARN_BYTES
    }
}

pub fn mib(bytes: u64) -> String {
    format!("{:.1} Mio", bytes as f64 / (1024.0 * 1024.0))
}
