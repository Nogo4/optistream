//! Window of one group: UV bounds of every mesh, shader checks, power-of-two axes.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{ensure, Result};
use optistream_rage::drawable::DrawableInfo;
use optistream_rage::joaat::joaat;
use optistream_rage::rsc7::PageFlags;
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::{Resource, Segment};
use optistream_texture::format::{self as fmt};
use optistream_texture::{TextureMeta, TextureRole};

use crate::crop::groups::{MAX_AREA, Members};
use crate::crop::uv::uv_bounds;
use crate::crop::{exempt, Axis, CROP_SHADERS, Group, PLACEHOLDERS, UV_EPSILON, UV_FREE_SAMPLERS, Window};
use crate::process::{self, Kind};
use crate::shaders::{self, Usage};

/// Texture dimensions that constrain the window.
#[derive(Default)]
struct Extent {
    min_w: u32,
    max_w: u32,
    min_h: u32,
    max_h: u32,
}

impl Extent {
    fn add(&mut self, res: &Resource, e: &TextureEntry) -> Result<()> {
        let (w, h) = (e.width as u32, e.height as u32);
        let meta = TextureMeta { width: w, height: h, format: e.format, levels: e.levels as u32 };
        let data = match res.resolve(e.data_ptr) {
            Some((Segment::Graphics, off)) => &res.graphics[off..],
            _ => anyhow::bail!("texture {} hors du segment graphique", e.name),
        };
        if exempt(&meta, data) {
            return Ok(());
        }
        ensure!(w.min(h) >= 4, "texture {} trop petite pour être recadrée", e.name);
        ensure!(fmt::is_pot(w) && fmt::is_pot(h), "texture {} non puissance de 2", e.name);
        ensure!(fmt::layout(e.format).is_some(), "format de {} non géré", e.name);
        ensure!(shaders::usage_from_name(&e.name) != Usage::Protected, "texture protégée {}", e.name);
        let first = self.max_w == 0;
        self.min_w = if first { w } else { self.min_w.min(w) };
        self.min_h = if first { h } else { self.min_h.min(h) };
        self.max_w = self.max_w.max(w);
        self.max_h = self.max_h.max(h);
        Ok(())
    }
}

/// UV bounding box `[umin, umax, vmin, vmax]`.
pub(super) type Bounds = [f64; 4];

pub(super) fn plan_group(root: &Path, m: &Members) -> Result<Option<Group>> {
    let mut extent = Extent::default();
    let mut bounds: Bounds = [f64::MAX, f64::MIN, f64::MAX, f64::MIN];
    let mut max_physical = 0;
    for rel in &m.ydds {
        let bytes = std::fs::read(root.join(rel))?;
        max_physical = max_physical.max(physical_size(&bytes));
        let p = process::crop_ready(Kind::Ydd, &bytes)?;
        ensure!(!p.drawables.is_empty(), "aucun drawable");
        for d in &p.drawables {
            let embedded: HashSet<String> = match d.texture_dict {
                Some(td) => rtex::read_dictionary(&p.res, td)?.into_iter().map(|e| e.name.to_ascii_lowercase()).collect(),
                None => HashSet::new(),
            };
            check_shaders(d, &embedded, !m.ytds.is_empty())?;
            uv_bounds(&p.res, d, &mut bounds)?;
        }
        // Every embedded dictionary, once.
        for t in &p.textures {
            extent.add(&p.res, &t.entry)?;
        }
    }
    for rel in &m.ytds {
        let bytes = std::fs::read(root.join(rel))?;
        max_physical = max_physical.max(physical_size(&bytes));
        let p = process::crop_ready(Kind::Ytd, &bytes)?;
        for t in &p.textures {
            extent.add(&p.res, &t.entry)?;
        }
    }
    if extent.max_w == 0 || bounds[0] > bounds[1] {
        return Ok(None);
    }
    if bounds[0] < -UV_EPSILON || bounds[1] > 1.0 + UV_EPSILON || bounds[2] < -UV_EPSILON || bounds[3] > 1.0 + UV_EPSILON {
        return Ok(None);
    }
    let window = Window {
        u: axis(bounds[0], bounds[1], extent.min_w, extent.max_w),
        v: axis(bounds[2], bounds[3], extent.min_h, extent.max_h),
    };
    if window.area() > MAX_AREA {
        return Ok(None);
    }
    let files = m.ydds.iter().chain(&m.ytds).cloned().collect();
    Ok(Some(Group { files, window, max_physical }))
}

fn physical_size(bytes: &[u8]) -> u64 {
    bytes.get(12..16).map_or(0, |b| PageFlags(u32::from_le_bytes(b.try_into().unwrap())).size())
}

/// Every shader must be one that samples its textures plainly with TEXCOORD0, and every
/// texture it reads must be cropped with the group: diffuses (variations or embedded),
/// embedded normal and specular maps, or the unused-sampler placeholder. A diffuse that is
/// not embedded must come from a variation of the group (`has_variations`): found nowhere
/// (`.ytd` kept in another folder), it would stay whole under rewritten UVs.
fn check_shaders(d: &DrawableInfo, embedded: &HashSet<String>, has_variations: bool) -> Result<()> {
    for s in &d.shaders {
        let name = shaders::shader_name(s).unwrap_or("?");
        ensure!(CROP_SHADERS.contains(&name), "shader {name}");
        for (param, tex) in &s.textures {
            let tex = tex.to_ascii_lowercase();
            if tex.is_empty() || PLACEHOLDERS.contains(&tex.as_str()) || UV_FREE_SAMPLERS.iter().any(|s| joaat(s) == *param) {
                continue;
            }
            match shaders::sampler_usage(*param) {
                Some(Usage::Role(TextureRole::Diffuse)) => {
                    ensure!(has_variations || embedded.contains(&tex), "diffuse {tex} sans variation .ytd dans le groupe")
                }
                Some(Usage::Role(TextureRole::Normal | TextureRole::Specular)) => {
                    ensure!(embedded.contains(&tex), "texture externe {tex}")
                }
                _ => anyhow::bail!("sampler 0x{param:08X} ({tex})"),
            }
        }
    }
    Ok(())
}

/// Smallest power-of-two part of an axis holding `[lo, hi]` plus a safety margin, starting
/// on a 4-texel boundary of the smallest texture (hence of every texture of the group).
pub(super) fn axis(lo: f64, hi: f64, min_n: u32, max_n: u32) -> Axis {
    // Bilinear filtering reads one texel around the used area; keep a little more.
    let margin = (2.0 / min_n as f64).max(8.0 / max_n as f64);
    let (lo, hi) = ((lo - margin).max(0.0), (hi + margin).min(1.0));
    let grid = 4.0 / min_n as f64;
    let max_shift = (min_n / 4).max(1).ilog2();
    for shift in (1..=max_shift).rev() {
        let size = 1.0 / (1u64 << shift) as f64;
        let mut start = (lo / grid).floor() * grid;
        if start + size > 1.0 {
            start = 1.0 - size;
        }
        if start <= lo + 1e-9 && start + size >= hi - 1e-9 {
            return Axis { start, size };
        }
    }
    Axis::FULL
}

#[cfg(test)]
mod tests {
    use optistream_rage::drawable::ShaderInfo;

    use super::*;

    fn shader(name: &str, textures: &[(&str, &str)]) -> ShaderInfo {
        ShaderInfo {
            name_hash: joaat(name),
            file_hash: 0,
            textures: textures.iter().map(|&(p, t)| (joaat(p), t.to_owned())).collect(),
        }
    }

    fn drawable(shaders: Vec<ShaderInfo>) -> DrawableInfo {
        DrawableInfo {
            offset: 0,
            hash: 0,
            has_skeleton: true,
            shaders,
            texture_dict: None,
            lods: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        }
    }

    fn embedded(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn physical_size_reads_the_graphics_flags_of_the_header() {
        let mut header = vec![0u8; 16];
        // 3 pages of the base size (0x2000 << 1).
        let flags = PageFlags::from_counts(1, [3, 0, 0, 0, 0], 0);
        header[12..16].copy_from_slice(&flags.0.to_le_bytes());
        assert_eq!(physical_size(&header), 3 * 0x4000);
        // Flags of the other segment are not read.
        header[8..12].copy_from_slice(&PageFlags::from_counts(4, [9, 0, 0, 0, 0], 0).0.to_le_bytes());
        assert_eq!(physical_size(&header), 3 * 0x4000);
        // Truncated header: unknown, counted as nothing.
        assert_eq!(physical_size(&header[..15]), 0);
        assert_eq!(physical_size(&[]), 0);
    }

    #[test]
    fn plain_ped_shaders_with_group_textures_are_accepted() {
        let d = drawable(vec![
            shader("ped", &[("DiffuseSampler", "Jbib_Diff_000_A"), ("BumpSampler", "jbib_normal_000"), ("SpecSampler", "jbib_spec_000")]),
            shader("ped_default_cutout", &[("DiffuseSampler", "jbib_diff_000_a")]),
        ]);
        let emb = embedded(&["jbib_normal_000", "jbib_spec_000"]);
        // Diffuse supplied by the variations of the group.
        assert!(check_shaders(&d, &emb, true).is_ok());
        // Without variations, an embedded diffuse is enough (names compared lowercased).
        let emb = embedded(&["jbib_diff_000_a", "jbib_normal_000", "jbib_spec_000"]);
        assert!(check_shaders(&d, &emb, false).is_ok());
    }

    #[test]
    fn shader_is_also_recognised_by_its_file_name() {
        let s = ShaderInfo { name_hash: 0, file_hash: joaat("ped_default.sps"), textures: Vec::new() };
        assert!(check_shaders(&drawable(vec![s]), &HashSet::new(), false).is_ok());
    }

    #[test]
    fn shaders_with_uv_transforms_or_unknown_names_are_refused() {
        for name in ["ped_wrinkle", "ped_cloth", "ped_decal", "ped_emissive", "ped_palette", "not_a_shader"] {
            let d = drawable(vec![shader(name, &[])]);
            assert!(check_shaders(&d, &HashSet::new(), true).is_err(), "{name}");
        }
        // One bad shader among good ones refuses the whole drawable.
        let d = drawable(vec![shader("ped", &[]), shader("ped_fur", &[])]);
        assert!(check_shaders(&d, &HashSet::new(), true).is_err());
    }

    #[test]
    fn diffuse_found_nowhere_in_the_group_is_refused() {
        let d = drawable(vec![shader("ped", &[("DiffuseSampler", "jbib_diff_000_a")])]);
        assert!(check_shaders(&d, &HashSet::new(), false).is_err());
    }

    #[test]
    fn external_normal_or_specular_maps_are_refused_even_with_variations() {
        for p in ["BumpSampler", "SpecSampler"] {
            let d = drawable(vec![shader("ped", &[(p, "shared_map")])]);
            assert!(check_shaders(&d, &HashSet::new(), true).is_err(), "{p}");
            assert!(check_shaders(&d, &embedded(&["shared_map"]), false).is_ok(), "{p}");
        }
    }

    #[test]
    fn placeholders_empty_slots_and_palette_samplers_are_ignored() {
        let d = drawable(vec![shader(
            "ped_default",
            &[
                ("WrinkleMaskSampler_0", "GiveMeChecker"),
                ("SpecSampler", "none"),
                ("BumpSampler", ""),
                ("TextureSamplerDiffPal", "jbib_pal"),
                ("TintPaletteSampler", "tint"),
            ],
        )]);
        assert!(check_shaders(&d, &HashSet::new(), false).is_ok());
    }

    #[test]
    fn protected_or_unknown_samplers_are_refused() {
        for p in ["WrinkleMaskSampler_0", "WrinkleSampler_A", "DetailSampler", "EmissiveSampler"] {
            let d = drawable(vec![shader("ped", &[(p, "some_texture")])]);
            assert!(check_shaders(&d, &embedded(&["some_texture"]), true).is_err(), "{p}");
        }
    }
}
