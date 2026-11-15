//! Writing a plan: texture headers patched in place, graphics segment rebuilt.

use anyhow::Result;
use optistream_rage::gfx::{self, GfxBlock};
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::Resource;

use super::BlockPlan;

/// Applies `blocks` (planned over `entries`, same indices) to `res`. `data_len[i]` is the
/// stored pixel data length of texture `i`.
pub fn apply(res: &mut Resource, entries: &[TextureEntry], data_len: &[usize], blocks: Vec<BlockPlan>) -> Result<()> {
    let mut new_blocks = Vec::with_capacity(blocks.len());
    for b in blocks {
        let refs: Vec<usize> = b.users.iter().map(|&u| entries[u].data_ptr_field()).collect();
        match b.enc {
            Some(enc) => {
                for &u in &b.users {
                    rtex::write_texture_header(
                        res,
                        entries[u].struct_offset,
                        enc.width as u16,
                        enc.height as u16,
                        enc.stride,
                        enc.format,
                        enc.levels as u8,
                    );
                }
                new_blocks.push(GfxBlock { data: enc.data, refs });
            }
            None => {
                let len = data_len[b.users[0]];
                new_blocks.push(GfxBlock { data: res.graphics[b.off..b.off + len].to_vec(), refs });
            }
        }
    }
    gfx::rebuild(res, new_blocks)
}
