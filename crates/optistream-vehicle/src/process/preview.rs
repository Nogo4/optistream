//! Before/after previews of the textures of one file.

use std::collections::BTreeMap;

use anyhow::Result;
use optistream_core::{PreviewContent, PreviewImage, PreviewItem};
use optistream_texture::format::{self as fmt};
use optistream_texture::{ops, plan, Rgba8, TextureMeta};

use crate::config::Config;
use crate::process::parse::tex_data;
use crate::process::{parse, plan_file, FileInfo, Kind};
use optistream_texture::budget::BlockPlan;
use crate::roles::VehicleIndex;
use optistream_mapping::roles::Usage;

fn preview_image(img: &Rgba8) -> PreviewImage {
    let small = ops::preview(img, 1024);
    PreviewImage { width: small.width, height: small.height, rgba: small.data }
}

fn tex_desc(w: u32, h: u32, format: u32, levels: u32, bytes: usize) -> String {
    format!("{w}×{h} {} · {levels} mips · {:.2} Mio", fmt::name(format), bytes as f64 / 1048576.0)
}

pub fn preview(kind: Kind, bytes: &[u8], cfg: &Config, index: &VehicleIndex, info: &FileInfo) -> Result<Vec<PreviewItem>> {
    let p = parse(kind, bytes, index, info)?;
    let fp = plan_file(&p, cfg)?;
    let by_texture: BTreeMap<usize, &BlockPlan> = fp.blocks.iter().flat_map(|b| b.users.iter().map(move |&u| (u, b))).collect();
    let mut items = Vec::new();
    // Biggest textures first: they are the ones the reduction is about.
    let mut order: Vec<usize> = (0..p.textures.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(p.textures[i].data_len));
    for i in order {
        let t = &p.textures[i];
        let Some(data) = tex_data(&p.res, t) else { continue };
        let meta = t.meta();
        let Ok(before) = plan::decode_top(&meta, data) else { continue };
        let before_desc = tex_desc(meta.width, meta.height, meta.format, meta.levels, t.data_len);
        let (after, after_desc) = match by_texture.get(&i) {
            _ if t.usage == Usage::Protected => (None, "protégée : non modifiée".to_owned()),
            Some(b) if b.enc.is_some() => {
                let enc = b.enc.as_ref().unwrap();
                let m = TextureMeta { width: enc.width, height: enc.height, format: enc.format, levels: enc.levels };
                let img = plan::decode_top(&m, &enc.data)?;
                let mut desc = tex_desc(enc.width, enc.height, enc.format, enc.levels, enc.data.len());
                if b.steps > 0 {
                    desc += &format!(" · réduction ciblée ÷{} (perte {:.1} %){}", 1u32 << b.steps, b.loss, if b.forced { ", forcée" } else { "" });
                }
                (Some(preview_image(&img)), desc)
            }
            _ => (None, "non modifiée".to_owned()),
        };
        items.push(PreviewItem {
            title: format!("Texture {} ({})", t.entry.name, t.label()),
            before_desc,
            after_desc,
            content: PreviewContent::Texture { before: preview_image(&before), after },
        });
    }
    Ok(items)
}
