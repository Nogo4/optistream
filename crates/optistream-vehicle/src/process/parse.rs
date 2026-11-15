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
