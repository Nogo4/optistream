//! Reading a map file: embedded drawables, textures and the role of each texture.

use anyhow::Result;
use optistream_rage::drawable::{self, DrawableInfo};
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::Resource;
use optistream_texture::resource as texres;

use crate::process::{Kind, Parsed, Tex};
use crate::roles::{self, RoleIndex, Usage};

/// Drawables of a `.ydr` (one, at the root) or a `.ydd` (dictionary at the root).
pub fn read_drawables(res: &Resource, kind: Kind) -> Result<Vec<DrawableInfo>> {
    match kind {
        Kind::Ytd => Ok(Vec::new()),
        Kind::Ydr => Ok(vec![drawable::read_drawable(res, 0, 0)?]),
        Kind::Ydd => drawable::read_dictionary(res, 0),
    }
}

pub(crate) fn tex(res: &Resource, entry: TextureEntry, usage: Usage) -> Tex {
    let (data_off, data_len) = match texres::locate(res, &entry) {
        Some((off, len)) => (Some(off), len),
        None => (None, 0),
    };
    let readable = roles::readable_by_name(&entry.name);
    Tex { entry, usage, readable, data_off, data_len }
}

/// Role of a texture: its own drawable's shaders, then the resource's, then its name.
pub(crate) fn usage(entry: &TextureEntry, own: Option<&RoleIndex>, resource: &RoleIndex) -> Usage {
    let name = &entry.name;
    own.and_then(|i| i.get(name)).or_else(|| resource.get(name)).unwrap_or_else(|| {
        if roles::is_lookup_shaped(entry.width, entry.height, entry.format) {
            Usage::Protected
        } else {
            roles::usage_from_name(name)
        }
    })
}

pub fn parse(kind: Kind, bytes: &[u8], index: &RoleIndex) -> Result<Parsed> {
    let res = Resource::parse(bytes)?;
    let drawables = read_drawables(&res, kind)?;
    let mut textures = Vec::new();
    if kind == Kind::Ytd {
        for entry in rtex::read_dictionary(&res, 0)? {
            let u = usage(&entry, None, index);
            textures.push(tex(&res, entry, u));
        }
    } else {
        let mut seen = std::collections::HashSet::new();
        for d in &drawables {
            let Some(td) = d.texture_dict else { continue };
            if !seen.insert(td) {
                continue;
            }
            let mut own = RoleIndex::default();
            own.add_shaders(&d.shaders);
            for entry in rtex::read_dictionary(&res, td)? {
                let u = usage(&entry, Some(&own), index);
                textures.push(tex(&res, entry, u));
            }
        }
    }
    Ok(Parsed { res, kind, drawables, textures })
}

pub(super) fn tex_data<'a>(res: &'a Resource, t: &Tex) -> Option<&'a [u8]> {
    t.data_off.map(|off| &res.graphics[off..off + t.data_len])
}

// ---------------------------------------------------------------- analysis
