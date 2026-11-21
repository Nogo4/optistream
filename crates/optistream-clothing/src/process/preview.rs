//! Before/after previews of textures and LOD meshes.

use anyhow::Result;
use optistream_i18n::{self as i18n, tf};
use optistream_core::{PreviewContent, PreviewImage, PreviewItem, PreviewMesh};
use optistream_mesh::Decimated;
use optistream_rage::drawable::{self, GeometryInfo};
use optistream_rage::Resource;
use optistream_texture::format::{self as fmt};
use optistream_texture::{ops, plan, Rgba8, TextureMeta};

use crate::config::Config;
use crate::process::analyze::has_cloth;
use crate::process::meshes::{decimate_lod, read_geometry, shared_vertex_buffers};
use crate::process::parse::role_of;
use crate::process::textures::{is_crop_change, optimize_texture};
use crate::process::{group, parse, FileInfo, Kind};

fn preview_image(img: &Rgba8) -> PreviewImage {
    let small = ops::preview(img, 1024);
    PreviewImage { width: small.width, height: small.height, rgba: small.data }
}

fn tex_desc(w: u32, h: u32, format: u32, levels: u32, bytes: usize) -> String {
    tf!(
        "clothing.preview.texture_desc",
        width = w,
        height = h,
        format = fmt::name(format),
        levels = levels,
        size = format!("{:.2}", bytes as f64 / 1048576.0)
    )
}

fn lod_mesh(res: &Resource, geoms: &[GeometryInfo], replaced: Option<&[Option<Decimated>]>) -> Result<PreviewMesh> {
    let mut m = PreviewMesh::default();
    for (i, g) in geoms.iter().enumerate() {
        if g.decl.component_type(drawable::semantic::POSITION) != drawable::component::FLOAT3 {
            continue;
        }
        let data = read_geometry(res, g)?;
        let stride = g.vertex_stride as usize;
        let (verts, idx): (&[u8], Vec<u16>) = match replaced.and_then(|r| r[i].as_ref()) {
            Some(d) => (&d.vertices, d.indices.clone()),
            None => (&data.vertices, data.indices.clone()),
        };
        let po = g.decl.component_offset(drawable::semantic::POSITION);
        let base = m.positions.len() as u32;
        for v in verts.chunks_exact(stride) {
            let f = |o: usize| f32::from_le_bytes(v[po + o..po + o + 4].try_into().unwrap());
            m.positions.push([f(0), f(4), f(8)]);
        }
        m.indices.extend(idx.iter().map(|&i| base + i as u32));
    }
    Ok(m)
}

pub fn preview(kind: Kind, bytes: &[u8], cfg: &Config, info: &FileInfo) -> Result<Vec<PreviewItem>> {
    let (slot, cloth_file) = (info.slot.as_str(), info.cloth);
    let p = parse(kind, bytes, info)?;
    let mut items = Vec::new();

    if kind == Kind::Ydd {
        let shared = shared_vertex_buffers(&p.drawables);
        for (di, d) in p.drawables.iter().enumerate() {
            let geoms = &d.lods[0];
            if geoms.is_empty() {
                continue;
            }
            let before = lod_mesh(&p.res, geoms, None)?;
            let cloth = cloth_file || has_cloth(d);
            let budget = cfg.budget_for_slot(slot);
            let (after, after_desc) = if cloth {
                (None, i18n::t("clothing.preview.cloth_unchanged").to_owned())
            } else if !cfg.mesh_enabled || d.lod_triangles(0) <= budget {
                (None, i18n::t("clothing.preview.within_budget").to_owned())
            } else {
                let plan = decimate_lod(&p.res, d, geoms, &shared, budget, cfg.max_error)?;
                let m = lod_mesh(&p.res, geoms, Some(&plan))?;
                let desc = tf!("clothing.preview.triangles", triangles = group(m.triangles() as u64));
                (Some(m), desc)
            };
            items.push(PreviewItem {
                title: if p.drawables.len() > 1 {
                    tf!("clothing.preview.mesh_title_indexed", index = di)
                } else {
                    i18n::t("clothing.preview.mesh_title").to_owned()
                },
                before_desc: tf!(
                    "clothing.preview.mesh_desc",
                    triangles = group(before.triangles() as u64),
                    vertices = group(d.lod_vertices(0))
                ),
                after_desc,
                content: PreviewContent::Mesh { before, after },
            });
        }
    }

    for t in &p.textures {
        let Some(off) = t.data_off else { continue };
        let data = &p.res.graphics[off..off + t.data_len];
        let meta = t.meta();
        let before = match plan::decode_top(&meta, data) {
            Ok(img) => img,
            Err(_) => continue,
        };
        let before_desc = tex_desc(meta.width, meta.height, meta.format, meta.levels, t.data_len);
        let (after, after_desc) = match role_of(t.usage) {
            None => (None, i18n::t("clothing.preview.protected_unchanged").to_owned()),
            Some(_) => match optimize_texture(t, data, cfg, info.skin, info.crop.as_ref())? {
                Some(enc) => {
                    let m = TextureMeta { width: enc.width, height: enc.height, format: enc.format, levels: enc.levels };
                    let img = plan::decode_top(&m, &enc.data)?;
                    let mut desc = tex_desc(enc.width, enc.height, enc.format, enc.levels, enc.data.len());
                    if let Some(c) = enc.changes.first().filter(|c| is_crop_change(c)) {
                        desc = format!("{desc} · {c}");
                    }
                    (Some(preview_image(&img)), desc)
                }
                None => (None, i18n::t("clothing.preview.already_compliant").to_owned()),
            },
        };
        let role = role_of(t.usage).map(|r| r.label()).unwrap_or(i18n::t("clothing.preview.protected"));
        items.push(PreviewItem {
            title: tf!("clothing.preview.texture_title", name = t.entry.name, role = role),
            before_desc,
            after_desc,
            content: PreviewContent::Texture { before: preview_image(&before), after },
        });
    }
    Ok(items)
}
