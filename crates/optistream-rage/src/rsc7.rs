use std::io::{Read, Write};

use anyhow::{bail, ensure, Context, Result};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};

pub const RSC7_MAGIC: u32 = 0x3743_5352;
pub const SYSTEM_BASE: u64 = 0x5000_0000;
pub const GRAPHICS_BASE: u64 = 0x6000_0000;
const SEGMENT_MASK: u64 = 0xF000_0000;

/// RSC7 page flags (`SystemFlags` / `GraphicsFlags` in the header).
///
/// Layout: bits 0-3 base shift, then page counts for sizes 16x, 8x, 4x, 2x, 1x the base
/// size (base = 0x2000 << shift), then four single "tail" pages of 1/2 .. 1/16 the base,
/// and the resource version nibble in bits 28-31. Pages are laid out from largest to smallest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(pub u32);

impl PageFlags {
    pub fn base_shift(self) -> u32 {
        self.0 & 0xF
    }

    pub fn base_size(self) -> u64 {
        0x2000u64 << self.base_shift()
    }

    pub fn version_nibble(self) -> u32 {
        self.0 >> 28
    }

    /// Page counts, ordered from the largest page size to the smallest tail page.
    pub fn page_counts(self) -> [u32; 9] {
        let v = self.0;
        [
            (v >> 4) & 0x1,
            (v >> 5) & 0x3,
            (v >> 7) & 0xF,
            (v >> 11) & 0x3F,
            (v >> 17) & 0x7F,
            (v >> 24) & 0x1,
            (v >> 25) & 0x1,
            (v >> 26) & 0x1,
            (v >> 27) & 0x1,
        ]
    }

    /// Page sizes matching [`Self::page_counts`].
    pub fn page_sizes(self) -> [u64; 9] {
        let b = self.base_size();
        [b << 4, b << 3, b << 2, b << 1, b, b >> 1, b >> 2, b >> 3, b >> 4]
    }

    pub fn page_count(self) -> u32 {
        self.page_counts().iter().sum()
    }

    /// Declared segment size: what FiveM reports as virtual (system) or physical (graphics) memory.
    pub fn size(self) -> u64 {
        self.page_counts()
            .iter()
            .zip(self.page_sizes())
            .map(|(&c, s)| c as u64 * s)
            .sum()
    }

    /// Builds flags from a base shift and counts for the five main page sizes
    /// (index 0 = base size, index 4 = 16x base). Tail pages are never emitted.
    pub fn from_counts(base_shift: u32, counts: [u32; 5], version_nibble: u32) -> Self {
        let mut v = base_shift & 0xF;
        v |= (counts[4] & 0x1) << 4;
        v |= (counts[3] & 0x3) << 5;
        v |= (counts[2] & 0xF) << 7;
        v |= (counts[1] & 0x3F) << 11;
        v |= (counts[0] & 0x7F) << 17;
        v |= (version_nibble & 0xF) << 28;
        PageFlags(v)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    System,
    Graphics,
}

/// A decompressed RSC7 resource: header fields plus both memory segments.
#[derive(Clone, Debug)]
pub struct Resource {
    pub version: u32,
    pub system_flags: PageFlags,
    pub graphics_flags: PageFlags,
    pub system: Vec<u8>,
    pub graphics: Vec<u8>,
}

impl Resource {
    pub fn is_rsc7(bytes: &[u8]) -> bool {
        bytes.len() >= 16 && u32::from_le_bytes(bytes[0..4].try_into().unwrap()) == RSC7_MAGIC
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(Self::is_rsc7(bytes), "not an RSC7 resource (encrypted or unknown format)");
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let system_flags = PageFlags(u32::from_le_bytes(bytes[8..12].try_into().unwrap()));
        let graphics_flags = PageFlags(u32::from_le_bytes(bytes[12..16].try_into().unwrap()));
        let sys_size = system_flags.size() as usize;
        let gfx_size = graphics_flags.size() as usize;

        let mut data = Vec::with_capacity(sys_size + gfx_size);
        DeflateDecoder::new(&bytes[16..])
            .read_to_end(&mut data)
            .context("RSC7 payload is not valid deflate data")?;
        if data.len() < sys_size + gfx_size {
            // Some tools truncate trailing zero pages; the game zero-fills them.
            data.resize(sys_size + gfx_size, 0);
        }
        let graphics = data[sys_size..sys_size + gfx_size].to_vec();
        data.truncate(sys_size);

        Ok(Resource { version, system_flags, graphics_flags, system: data, graphics })
    }

    pub fn to_bytes(&self, level: u32) -> Result<Vec<u8>> {
        ensure!(
            self.system.len() as u64 == self.system_flags.size(),
            "system segment size does not match its page flags"
        );
        ensure!(
            self.graphics.len() as u64 == self.graphics_flags.size(),
            "graphics segment size does not match its page flags"
        );
        let mut out = Vec::with_capacity(16 + (self.system.len() + self.graphics.len()) / 3);
        out.extend_from_slice(&RSC7_MAGIC.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.system_flags.0.to_le_bytes());
        out.extend_from_slice(&self.graphics_flags.0.to_le_bytes());
        let mut enc = DeflateEncoder::new(out, Compression::new(level.min(9)));
        enc.write_all(&self.system)?;
        enc.write_all(&self.graphics)?;
        Ok(enc.finish()?)
    }

    pub fn resolve(&self, ptr: u64) -> Option<(Segment, usize)> {
        let off = (ptr & !SEGMENT_MASK) as usize;
        match ptr & SEGMENT_MASK {
            SYSTEM_BASE if off < self.system.len() => Some((Segment::System, off)),
            GRAPHICS_BASE if off < self.graphics.len() => Some((Segment::Graphics, off)),
            _ => None,
        }
    }

    pub fn segment(&self, seg: Segment) -> &[u8] {
        match seg {
            Segment::System => &self.system,
            Segment::Graphics => &self.graphics,
        }
    }

    pub fn segment_mut(&mut self, seg: Segment) -> &mut Vec<u8> {
        match seg {
            Segment::System => &mut self.system,
            Segment::Graphics => &mut self.graphics,
        }
    }

    /// Returns `len` bytes at a resource pointer, in whichever segment it targets.
    pub fn slice(&self, ptr: u64, len: usize) -> Result<&[u8]> {
        let Some((seg, off)) = self.resolve(ptr) else {
            bail!("invalid resource pointer 0x{ptr:X}");
        };
        let data = self.segment(seg);
        ensure!(off + len <= data.len(), "block at 0x{ptr:X} (+{len}) overruns its segment");
        Ok(&data[off..off + len])
    }

    /// Resolves a pointer that must target the system segment.
    pub fn sys_offset(&self, ptr: u64) -> Result<usize> {
        match self.resolve(ptr) {
            Some((Segment::System, off)) => Ok(off),
            _ => bail!("expected a system pointer, got 0x{ptr:X}"),
        }
    }

    pub fn u8_at(&self, off: usize) -> Result<u8> {
        self.system.get(off).copied().context("read past the system segment")
    }

    pub fn u16_at(&self, off: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.sys_bytes::<2>(off)?))
    }

    pub fn u32_at(&self, off: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.sys_bytes::<4>(off)?))
    }

    pub fn u64_at(&self, off: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.sys_bytes::<8>(off)?))
    }

    fn sys_bytes<const N: usize>(&self, off: usize) -> Result<[u8; N]> {
        self.system
            .get(off..off + N)
            .map(|b| b.try_into().unwrap())
            .context("read past the system segment")
    }

    pub fn set_u8(&mut self, off: usize, v: u8) {
        self.system[off] = v;
    }

    pub fn set_u16(&mut self, off: usize, v: u16) {
        self.system[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_u32(&mut self, off: usize, v: u32) {
        self.system[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_u64(&mut self, off: usize, v: u64) {
        self.system[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Reads a NUL-terminated string from the system segment.
    pub fn cstr_at(&self, ptr: u64) -> Result<String> {
        if ptr == 0 {
            return Ok(String::new());
        }
        let off = self.sys_offset(ptr)?;
        let rest = &self.system[off..];
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    /// Offset of the `ResourcePagesInfo` block referenced by the root structure, if any.
    pub fn pages_info_offset(&self) -> Result<Option<usize>> {
        let ptr = self.u64_at(0x08)?;
        if ptr == 0 {
            return Ok(None);
        }
        Ok(Some(self.sys_offset(ptr)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_roundtrip() {
        let f = PageFlags::from_counts(3, [5, 2, 1, 0, 1], 0xA);
        assert_eq!(f.base_shift(), 3);
        assert_eq!(f.version_nibble(), 0xA);
        let base = 0x2000u64 << 3;
        assert_eq!(f.size(), base * 5 + base * 2 * 2 + base * 4 + base * 16);
        assert_eq!(f.page_count(), 9);
    }

    #[test]
    fn codewalker_examples() {
        // ytd example from CodeWalker: size 8192, ss 0, s4 = 1.
        assert_eq!(PageFlags(1 << 17).size(), 8192);
        // ydd example: ss 1, s3 1, s4 12, s6 1, s7 3 => 663552.
        let v = 1 | (1 << 24) | (12 << 17) | (1 << 7) | (3 << 5);
        assert_eq!(PageFlags(v).size(), 663_552);
    }
}
