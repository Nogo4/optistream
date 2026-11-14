//! Reading drawable dictionaries, drawables, models, geometries and shaders from a resource.

use std::collections::HashSet;

use anyhow::{ensure, Result};

use crate::drawable::{geom_field, ib_field, vb_field, DrawableInfo, GeometryInfo, ShaderInfo, VertexDecl};
use crate::rsc7::Resource;

/// Reads a `pgDictionary<rmcDrawable>` (YDD root) at `off`.
pub fn read_dictionary(res: &Resource, off: usize) -> Result<Vec<DrawableInfo>> {
    let hashes_ptr = res.u64_at(off + 0x20)?;
    let hashes_count = res.u16_at(off + 0x28)? as usize;
    let list_ptr = res.u64_at(off + 0x30)?;
    let count = res.u16_at(off + 0x38)? as usize;
    if list_ptr == 0 || count == 0 {
        return Ok(Vec::new());
    }
    let hashes = if hashes_ptr != 0 { Some(res.sys_offset(hashes_ptr)?) } else { None };
    let list = res.sys_offset(list_ptr)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let dptr = res.u64_at(list + i * 8)?;
        if dptr == 0 {
            continue;
        }
        let hash = match hashes {
            Some(h) if i < hashes_count => res.u32_at(h + i * 4)?,
            _ => 0,
        };
        out.push(read_drawable(res, res.sys_offset(dptr)?, hash)?);
    }
    Ok(out)
}

pub fn read_drawable(res: &Resource, d: usize, hash: u32) -> Result<DrawableInfo> {
    let shader_group = res.u64_at(d + 0x10)?;
    let skeleton = res.u64_at(d + 0x18)?;

    let mut shaders = Vec::new();
    let mut texture_dict = None;
    if shader_group != 0 {
        let sg = res.sys_offset(shader_group)?;
        let td = res.u64_at(sg + 0x08)?;
        if td != 0 {
            texture_dict = Some(res.sys_offset(td)?);
        }
        let sh_ptr = res.u64_at(sg + 0x10)?;
        let sh_count = res.u16_at(sg + 0x18)? as usize;
        if sh_ptr != 0 {
            let list = res.sys_offset(sh_ptr)?;
            for i in 0..sh_count {
                let p = res.u64_at(list + i * 8)?;
                if p != 0 {
                    shaders.push(read_shader(res, res.sys_offset(p)?)?);
                }
            }
        }
    }

    let mut seen = HashSet::new();
    let mut lods: [Vec<GeometryInfo>; 4] = Default::default();
    for (lod, list) in lods.iter_mut().enumerate() {
        let header = res.u64_at(d + 0x50 + lod * 8)?;
        if header == 0 {
            continue;
        }
        let h = res.sys_offset(header)?;
        let models_ptr = res.u64_at(h)?;
        let models_count = res.u16_at(h + 8)? as usize;
        if models_ptr == 0 {
            continue;
        }
        let models = res.sys_offset(models_ptr)?;
        for m in 0..models_count {
            let mptr = res.u64_at(models + m * 8)?;
            if mptr == 0 {
                continue;
            }
            read_model(res, res.sys_offset(mptr)?, &mut seen, list)?;
        }
    }

    Ok(DrawableInfo { offset: d, hash, has_skeleton: skeleton != 0, shaders, texture_dict, lods })
}

fn read_model(
    res: &Resource,
    m: usize,
    seen: &mut HashSet<usize>,
    out: &mut Vec<GeometryInfo>,
) -> Result<()> {
    let geoms_ptr = res.u64_at(m + 0x08)?;
    let geoms_count = res.u16_at(m + 0x10)? as usize;
    let mapping_ptr = res.u64_at(m + 0x20)?;
    if geoms_ptr == 0 {
        return Ok(());
    }
    let geoms = res.sys_offset(geoms_ptr)?;
    let mapping = if mapping_ptr != 0 { Some(res.sys_offset(mapping_ptr)?) } else { None };
    for i in 0..geoms_count {
        let gptr = res.u64_at(geoms + i * 8)?;
        if gptr == 0 {
            continue;
        }
        let g = res.sys_offset(gptr)?;
        if !seen.insert(g) {
            continue;
        }
        let shader_index = match mapping {
            Some(mo) => Some(res.u16_at(mo + i * 2)?),
            None => None,
        };
        out.push(read_geometry(res, g, shader_index)?);
    }
    Ok(())
}

fn read_geometry(res: &Resource, g: usize, shader_index: Option<u16>) -> Result<GeometryInfo> {
    let vb = res.sys_offset(res.u64_at(g + geom_field::VERTEX_BUFFER_PTR)?)?;
    let ib = res.sys_offset(res.u64_at(g + geom_field::INDEX_BUFFER_PTR)?)?;
    let info = res.sys_offset(res.u64_at(vb + vb_field::INFO_PTR)?)?;
    let decl = VertexDecl {
        flags: res.u32_at(info)?,
        stride: res.u16_at(info + 4)?,
        count: res.u8_at(info + 7)?,
        types: res.u64_at(info + 8)?,
    };
    let vertex_stride = res.u16_at(vb + vb_field::STRIDE)?;
    ensure!(vertex_stride > 0, "geometry with a zero vertex stride");
    Ok(GeometryInfo {
        offset: g,
        vb_offset: vb,
        ib_offset: ib,
        decl,
        shader_index,
        vertex_stride,
        vertex_count: res.u32_at(vb + vb_field::VERTEX_COUNT)?,
        vertex_ptr: res.u64_at(vb + vb_field::DATA_PTR1)?,
        vertex_ptr2: res.u64_at(vb + vb_field::DATA_PTR2)?,
        index_ptr: res.u64_at(ib + ib_field::INDICES_PTR)?,
        index_count: res.u32_at(ib + ib_field::INDICES_COUNT)?,
    })
}

fn read_shader(res: &Resource, s: usize) -> Result<ShaderInfo> {
    let params_ptr = res.u64_at(s)?;
    let name_hash = res.u32_at(s + 0x08)?;
    let count = res.u8_at(s + 0x10)? as usize;
    let file_hash = res.u32_at(s + 0x18)?;
    let mut textures = Vec::new();
    if params_ptr != 0 && count > 0 {
        let p = res.sys_offset(params_ptr)?;
        // Parameter headers, then inline vector data, then one name hash per parameter.
        let mut data_len = 0usize;
        let mut tex_params = Vec::new();
        for i in 0..count {
            let kind = res.u8_at(p + i * 16)?;
            let data_ptr = res.u64_at(p + i * 16 + 8)?;
            match kind {
                0 => tex_params.push((i, data_ptr)),
                n => data_len += 16 * n as usize,
            }
        }
        let hashes = p + count * 16 + data_len;
        for (i, data_ptr) in tex_params {
            let hash = res.u32_at(hashes + i * 4)?;
            let name = if data_ptr != 0 {
                res.cstr_at(res.u64_at(res.sys_offset(data_ptr)? + 0x28)?)?
            } else {
                String::new()
            };
            textures.push((hash, name));
        }
    }
    Ok(ShaderInfo { name_hash, file_hash, textures })
}
