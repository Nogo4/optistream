//! `rage::pgDictionary<grcTexture>` (YTD root, or embedded in a drawable's shader group).

use anyhow::{ensure, Result};

use crate::rsc7::Resource;

/// Size of a Legacy `grcTexture` structure.
pub const TEXTURE_STRUCT_SIZE: usize = 0x90;

pub mod field {
    pub const NAME_PTR: usize = 0x28;
    pub const WIDTH: usize = 0x50;
    pub const HEIGHT: usize = 0x52;
    pub const DEPTH: usize = 0x54;
    pub const STRIDE: usize = 0x56;
    pub const FORMAT: usize = 0x58;
    pub const LEVELS: usize = 0x5D;
    pub const DATA_PTR: usize = 0x70;
}

/// D3D9 texture formats used by Legacy resources.
pub mod format {
    pub const A8R8G8B8: u32 = 21;
    pub const X8R8G8B8: u32 = 22;
    pub const A1R5G5B5: u32 = 25;
    pub const A8: u32 = 28;
    pub const A8B8G8R8: u32 = 32;
    pub const L8: u32 = 50;
    pub const DXT1: u32 = 0x3154_5844;
    pub const DXT3: u32 = 0x3354_5844;
    pub const DXT5: u32 = 0x3554_5844;
    pub const ATI1: u32 = 0x3149_5441;
    pub const ATI2: u32 = 0x3249_5441;
    pub const BC7: u32 = 0x2037_4342;
}

#[derive(Clone, Debug)]
pub struct TextureEntry {
    /// Offset of the `grcTexture` structure in the system segment.
    pub struct_offset: usize,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub format: u32,
    pub levels: u8,
    pub data_ptr: u64,
}

impl TextureEntry {
    /// Offset of the pixel data pointer field, to be patched when graphics are re-laid out.
    pub fn data_ptr_field(&self) -> usize {
        self.struct_offset + field::DATA_PTR
    }
}

/// Reads a `pgDictionary<grcTexture>` located at `off` in the system segment.
pub fn read_dictionary(res: &Resource, off: usize) -> Result<Vec<TextureEntry>> {
    let list_ptr = res.u64_at(off + 0x30)?;
    let count = res.u16_at(off + 0x38)? as usize;
    if list_ptr == 0 || count == 0 {
        return Ok(Vec::new());
    }
    let list = res.sys_offset(list_ptr)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let tptr = res.u64_at(list + i * 8)?;
        if tptr == 0 {
            continue;
        }
        out.push(read_texture(res, res.sys_offset(tptr)?)?);
    }
    Ok(out)
}

pub fn read_texture(res: &Resource, t: usize) -> Result<TextureEntry> {
    ensure!(t + TEXTURE_STRUCT_SIZE <= res.system.len(), "texture structure overruns segment");
    Ok(TextureEntry {
        struct_offset: t,
        name: res.cstr_at(res.u64_at(t + field::NAME_PTR)?)?,
        width: res.u16_at(t + field::WIDTH)?,
        height: res.u16_at(t + field::HEIGHT)?,
        depth: res.u16_at(t + field::DEPTH)?,
        stride: res.u16_at(t + field::STRIDE)?,
        format: res.u32_at(t + field::FORMAT)?,
        levels: res.u8_at(t + field::LEVELS)?,
        data_ptr: res.u64_at(t + field::DATA_PTR)?,
    })
}

/// Writes new dimensions/format/levels into a texture structure (data pointer is patched separately).
pub fn write_texture_header(
    res: &mut Resource,
    t: usize,
    width: u16,
    height: u16,
    stride: u16,
    format: u32,
    levels: u8,
) {
    res.set_u16(t + field::WIDTH, width);
    res.set_u16(t + field::HEIGHT, height);
    res.set_u16(t + field::STRIDE, stride);
    res.set_u32(t + field::FORMAT, format);
    res.set_u8(t + field::LEVELS, levels);
}
