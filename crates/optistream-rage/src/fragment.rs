//! Fragment types (`fragType`, YFT root), Legacy layout (v162).
//!
//! Only the drawables are decoded: the primary drawable (shaders, embedded texture
//! dictionary, LODs), the optional drawable array, and the undamaged / damaged drawables
//! of every physics child. Child drawables have no shader group of their own: their
//! geometries index the primary drawable's shaders.

use std::collections::HashSet;

use anyhow::{Context, Result};

use crate::drawable::{read_drawable, DrawableInfo};
use crate::rsc7::{Resource, Segment};

pub mod field {
    pub const DRAWABLE: usize = 0x30;
    pub const DRAWABLE_ARRAY: usize = 0x38;
    pub const DRAWABLE_ARRAY_COUNT: usize = 0x48;
    pub const PHYSICS_LOD_GROUP: usize = 0xF0;
}

/// `fragPhysicsLODGroup`: three LOD pointers.
pub mod lod_group_field {
    pub const LODS: [usize; 3] = [0x10, 0x18, 0x20];
}

/// `fragPhysicsLOD`.
pub mod lod_field {
    pub const CHILDREN: usize = 0xD0;
    pub const CHILDREN_COUNT: usize = 0x11E;
}

/// `fragTypeChild`.
pub mod child_field {
    pub const DRAWABLE: usize = 0xA0;
    pub const DAMAGED_DRAWABLE: usize = 0xB0;
}

#[derive(Clone, Debug)]
pub struct FragmentInfo {
    pub primary: DrawableInfo,
    /// Every other drawable met (drawable array, child drawables), each listed once.
    pub others: Vec<DrawableInfo>,
}

impl FragmentInfo {
    pub fn drawables(&self) -> impl Iterator<Item = &DrawableInfo> {
        std::iter::once(&self.primary).chain(&self.others)
    }
}

fn sys_ptr(res: &Resource, off: usize) -> Result<Option<usize>> {
    let p = res.u64_at(off)?;
    Ok(match res.resolve(p) {
        Some((Segment::System, o)) if p != 0 => Some(o),
        _ => None,
    })
}

/// Reads the fragment at the root of a YFT.
pub fn read_fragment(res: &Resource) -> Result<FragmentInfo> {
    let d = sys_ptr(res, field::DRAWABLE)?.context("fragment without a drawable")?;
    let primary = read_drawable(res, d, 0)?;
    let mut seen: HashSet<usize> = HashSet::from([d]);
    let mut others = Vec::new();
    let mut add = |off: usize, others: &mut Vec<DrawableInfo>| -> Result<()> {
        if seen.insert(off) {
            others.push(read_drawable(res, off, 0)?);
        }
        Ok(())
    };

    if let Some(list) = sys_ptr(res, field::DRAWABLE_ARRAY)? {
        let count = res.u32_at(field::DRAWABLE_ARRAY_COUNT)? as usize;
        for i in 0..count {
            if let Some(o) = sys_ptr(res, list + i * 8)? {
                add(o, &mut others)?;
            }
        }
    }

    if let Some(group) = sys_ptr(res, field::PHYSICS_LOD_GROUP)? {
        for lod_off in lod_group_field::LODS {
            let Some(lod) = sys_ptr(res, group + lod_off)? else { continue };
            let Some(children) = sys_ptr(res, lod + lod_field::CHILDREN)? else { continue };
            let count = res.u8_at(lod + lod_field::CHILDREN_COUNT)? as usize;
            for i in 0..count {
                let Some(child) = sys_ptr(res, children + i * 8)? else { continue };
                // The damaged field is not always a drawable (some exporters store another
                // structure there): it is only read when it carries the same vtable as the
                // child's drawable.
                let Some(o) = sys_ptr(res, child + child_field::DRAWABLE)? else { continue };
                add(o, &mut others)?;
                let vft = res.u64_at(o)?;
                if let Some(dmg) = sys_ptr(res, child + child_field::DAMAGED_DRAWABLE)? {
                    if res.u64_at(dmg)? == vft {
                        add(dmg, &mut others)?;
                    }
                }
            }
        }
    }
    Ok(FragmentInfo { primary, others })
}
