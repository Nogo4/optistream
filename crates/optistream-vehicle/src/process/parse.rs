//! Reading a vehicle file: drawables, textures, role and class of each texture.

use std::collections::HashSet;

use anyhow::Result;
use optistream_mapping::roles::{self as mroles, Usage};
use optistream_rage::drawable::{self, DrawableInfo};
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::{fragment, Resource};
use optistream_texture::resource as texres;
use optistream_texture::TextureRole;

use crate::config::BIG_SIDE;
use crate::process::{Class, FileInfo, Kind, Parsed, Tex};
use crate::roles::{self, VehicleIndex};

/// Drawables of a `.ydr` (one), a `.ydd` (dictionary) or a `.yft` (primary first, then
/// the drawable array and the child drawables).
pub fn read_drawables(res: &Resource, kind: Kind) -> Result<Vec<DrawableInfo>> {
    match kind {
        Kind::Ytd => Ok(Vec::new()),
        Kind::Ydr => Ok(vec![drawable::read_drawable(res, 0, 0)?]),
        Kind::Ydd => drawable::read_dictionary(res, 0),
        Kind::Yft => {
            let f = fragment::read_fragment(res)?;
            Ok(std::iter::once(f.primary).chain(f.others).collect())
        }
    }
}

/// Role of a texture: its own drawable's shaders, then the resource's, then its name.
fn usage(entry: &TextureEntry, own: Option<&VehicleIndex>, resource: &VehicleIndex) -> Usage {
    let name = &entry.name;
    own.and_then(|i| i.roles.get(name)).or_else(|| resource.roles.get(name)).unwrap_or_else(|| {
        if mroles::is_lookup_shaped(entry.width, entry.height, entry.format) {
            Usage::Protected
        } else {
            roles::usage_from_name(name)
        }
    })
}

/// Class of a texture, first matching rule of the design order.
fn class(entry: &TextureEntry, usage: Usage, livery: bool, hi_copy: bool) -> Class {
    if hi_copy {
        Class::HiCopy
    } else if roles::light_or_glass_by_name(&entry.name) || usage == Usage::Role(TextureRole::Other) {
        Class::Last
    } else if livery {
        Class::Livery
    } else if (entry.width.min(entry.height) as u32) >= BIG_SIDE {
        Class::Big
    } else {
        Class::Small
    }
}

fn tex(res: &Resource, entry: TextureEntry, own: Option<&VehicleIndex>, index: &VehicleIndex, info: &FileInfo) -> Tex {
    let (data_off, data_len) = match texres::locate(res, &entry) {
        Some((off, len)) => (Some(off), len),
        None => (None, 0),
    };
    let lower = entry.name.to_ascii_lowercase();
    let usage = usage(&entry, own, index);
    let sampled_as_livery = own.is_some_and(|i| i.liveries.contains(&lower)) || index.liveries.contains(&lower);
    // A livery part's embedded textures are liveries, unless a shader says they are maps.
    let livery = sampled_as_livery
        || roles::livery_by_name(&entry.name)
        || (info.livery_file && matches!(usage, Usage::Role(TextureRole::Diffuse)));
    let class = class(&entry, usage, livery, info.hi_names.contains(&lower));
    Tex { entry, usage, livery, class, data_off, data_len }
}

pub fn parse(kind: Kind, bytes: &[u8], index: &VehicleIndex, info: &FileInfo) -> Result<Parsed> {
    let res = Resource::parse(bytes)?;
    let drawables = read_drawables(&res, kind)?;
    let mut textures = Vec::new();
    if kind == Kind::Ytd {
        for entry in rtex::read_dictionary(&res, 0)? {
            let t = tex(&res, entry, None, index, info);
            textures.push(t);
        }
    } else {
        // Child drawables of a fragment have no shaders of their own: the primary's apply.
        let mut own = VehicleIndex::default();
        for d in &drawables {
            own.add_shaders(&d.shaders);
        }
        let mut seen = HashSet::new();
        for d in &drawables {
            let Some(td) = d.texture_dict else { continue };
            if !seen.insert(td) {
                continue;
            }
            for entry in rtex::read_dictionary(&res, td)? {
                let t = tex(&res, entry, Some(&own), index, info);
                textures.push(t);
            }
        }
    }
    Ok(Parsed { res, kind, drawables, textures })
}

pub(super) fn tex_data<'a>(res: &'a Resource, t: &Tex) -> Option<&'a [u8]> {
    t.data_off.map(|off| &res.graphics[off..off + t.data_len])
}

/// Texture names of a `.ytd` (lowercased), for the `+hi.ytd` lookup.
pub fn ytd_names(bytes: &[u8]) -> Result<HashSet<String>> {
    let res = Resource::parse(bytes)?;
    Ok(rtex::read_dictionary(&res, 0)?.into_iter().map(|e| e.name.to_ascii_lowercase()).collect())
}

#[cfg(test)]
mod tests {
    use optistream_rage::drawable::ShaderInfo;
    use optistream_rage::joaat::joaat;
    use optistream_rage::texture::format as tf;

    use super::*;

    fn entry(name: &str, width: u16, height: u16, format: u32) -> TextureEntry {
        TextureEntry { struct_offset: 0, name: name.into(), width, height, depth: 1, stride: width, format, levels: 1, data_ptr: 0 }
    }

    const DIFFUSE: Usage = Usage::Role(TextureRole::Diffuse);

    #[test]
    fn class_follows_the_design_order() {
        let body = entry("body_d", 2048, 2048, tf::DXT5);
        // A +hi copy wins over everything, even a light or a livery.
        assert_eq!(class(&entry("tfstanier_lights", 2048, 2048, tf::DXT5), DIFFUSE, true, true), Class::HiCopy);
        // Then lights / glass by name, and unknown roles.
        assert_eq!(class(&entry("tfstanier_lights", 2048, 2048, tf::DXT5), DIFFUSE, true, false), Class::Last);
        assert_eq!(class(&body, Usage::Role(TextureRole::Other), true, false), Class::Last);
        // Then liveries, whatever their size.
        assert_eq!(class(&entry("x_sign_1", 512, 512, tf::DXT5), DIFFUSE, true, false), Class::Livery);
        assert_eq!(class(&body, DIFFUSE, false, false), Class::Big);
        assert_eq!(class(&entry("belt_d", 512, 512, tf::DXT5), DIFFUSE, false, false), Class::Small);
    }

    #[test]
    fn big_is_decided_by_the_smaller_side() {
        assert_eq!(class(&entry("strip_d", 4096, 512, tf::DXT5), DIFFUSE, false, false), Class::Small);
        assert_eq!(class(&entry("sheet_d", 1024, 2048, tf::DXT5), DIFFUSE, false, false), Class::Big);
        assert_eq!(class(&entry("sheet_d", 1023, 4096, tf::DXT5), DIFFUSE, false, false), Class::Small);
    }

    #[test]
    fn usage_prefers_own_shaders_then_resource_then_shape_then_name() {
        let shader = |sampler: &str, name: &str| ShaderInfo { name_hash: 0, file_hash: 0, textures: vec![(joaat(sampler), name.into())] };
        let mut own = VehicleIndex::default();
        own.add_shaders(&[shader("BumpSampler", "Body_D")]);
        let mut resource = VehicleIndex::default();
        resource.add_shaders(&[shader("SpecSampler", "body_d"), shader("SpecSampler", "seat_d")]);

        let body = entry("body_d", 1024, 1024, tf::DXT5);
        assert_eq!(usage(&body, Some(&own), &resource), Usage::Role(TextureRole::Normal));
        assert_eq!(usage(&body, None, &resource), Usage::Role(TextureRole::Specular));
        assert_eq!(usage(&entry("seat_d", 512, 512, tf::DXT5), Some(&own), &resource), Usage::Role(TextureRole::Specular));
        // Unknown to every shader: an uncompressed strip is a lookup table, else the name decides.
        let empty = VehicleIndex::default();
        assert_eq!(usage(&entry("ramp", 256, 4, tf::A8R8G8B8), None, &empty), Usage::Protected);
        assert_eq!(usage(&entry("SEAT NM", 512, 512, tf::DXT5), None, &empty), Usage::Role(TextureRole::Normal));
    }
}
