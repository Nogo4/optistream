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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bare RSC7 header with the given page flags (no payload).
    pub(crate) fn rsc7_header(system: u32, graphics: u32) -> Vec<u8> {
        let mut b = optistream_rage::rsc7::RSC7_MAGIC.to_le_bytes().to_vec();
        b.extend_from_slice(&13u32.to_le_bytes());
        b.extend_from_slice(&system.to_le_bytes());
        b.extend_from_slice(&graphics.to_le_bytes());
        b
    }

    /// Page flags declaring `pages` pages of 16 MiB each.
    pub(crate) fn mib16_pages(pages: u32) -> u32 {
        11 | (pages << 17)
    }

    #[test]
    fn non_rsc7_only_has_a_disk_size() {
        let s = SizeInfo::of(b"not a resource at all");
        assert_eq!((s.disk, s.system, s.graphics), (21, 0, 0));
        assert!(!s.fivem_warning());
    }

    #[test]
    fn rsc7_sizes_come_from_the_header() {
        let s = SizeInfo::of(&rsc7_header(1 << 17, mib16_pages(1)));
        assert_eq!(s.disk, 16);
        assert_eq!(s.system, 8192);
        assert_eq!(s.graphics, FIVEM_WARN_BYTES);
    }

    #[test]
    fn warning_strictly_above_16_mib() {
        assert!(!SizeInfo::of(&rsc7_header(0, mib16_pages(1))).fivem_warning());
        assert!(SizeInfo::of(&rsc7_header(0, mib16_pages(2))).fivem_warning());
        assert!(SizeInfo::of(&rsc7_header(mib16_pages(2), 0)).fivem_warning());
    }

    #[test]
    fn mib_formatting() {
        assert_eq!(mib(0), "0.0 Mio");
        assert_eq!(mib(1024 * 1024 * 3 / 2), "1.5 Mio");
    }
}
