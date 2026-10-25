//! UV cropping: textures cut down to the part of the sheet the meshes actually sample, UVs
//! rewritten so that the garment looks the same.
//!
//! A drawable, its first-person model (`_1.ydd`) and every diffuse variation (`.ytd`) share
//! one UV layout: they form a group, cropped with the same window or not at all. The window
//! is a power-of-two fraction of each texture, aligned on 4×4 blocks, so the top level of a
//! DXT texture is copied block for block (lossless) and stays a power of two.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{ensure, Result};
use optistream_rage::drawable::{self, component, semantic, vb_field, DrawableInfo, GeometryInfo};
use optistream_rage::joaat::joaat;
use optistream_rage::rsc7::PageFlags;
use optistream_rage::texture::{self as rtex, TextureEntry};
use optistream_rage::{Resource, Segment};
use optistream_texture::format::{self as fmt, Layout};
use optistream_texture::{TextureMeta, TextureRole};
use rayon::prelude::*;

use crate::naming;
use crate::process::{self, Kind};
use crate::shaders::{self, Usage};

/// Part of a texture kept on one axis, in UV units: `size` is a power of two (1 = whole axis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub start: f64,
    pub size: f64,
}

impl Axis {
    const FULL: Axis = Axis { start: 0.0, size: 1.0 };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub u: Axis,
    pub v: Axis,
}

impl Window {
    /// Kept share of the texture area.
    pub fn area(&self) -> f64 {
        self.u.size * self.v.size
    }

    /// Pixel rectangle `(x0, y0, width, height)` of a `w`×`h` texture (V grows downwards, D3D).
    pub fn rect(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let px = |a: &Axis, n: u32| ((a.start * n as f64).round() as u32, (a.size * n as f64).round() as u32);
        let ((x0, cw), (y0, ch)) = (px(&self.u, w), px(&self.v, h));
        (x0, y0, cw, ch)
    }

    fn map(&self, uv: [f32; 2]) -> [f32; 2] {
        [
            ((uv[0] as f64 - self.u.start) / self.u.size) as f32,
            ((uv[1] as f64 - self.v.start) / self.v.size) as f32,
        ]
    }

    pub fn describe(&self) -> String {
        format!("{:.0} % × {:.0} % de la feuille", self.u.size * 100.0, self.v.size * 100.0)
    }
}

/// Files sharing one UV layout, and the window they are cropped with.
#[derive(Debug)]
pub struct Group {
    /// Lowercased relative paths of every member.
    pub files: Vec<String>,
    pub window: Window,
    /// Largest declared physical memory among the members: the memory threshold applies to
    /// the group as a whole (all of it is processed, or none).
    pub max_physical: u64,
}

/// Group of every cropped file, by lowercased relative path.
pub type RunPlan = HashMap<String, Arc<Group>>;

/// Shaders whose every texture is sampled with TEXCOORD0 and no UV transform (no detail
/// map, palette, wrinkle mask, cloth or emissive special case).
const CROP_SHADERS: &[&str] = &[
    "ped", "ped_default", "ped_default_mp", "ped_alpha", "ped_default_cutout", "ped_hair_cutout_alpha",
    "ped_hair_spiked", "ped_nopeddamagedecals", "default",
];

/// Placeholders bound to unused samplers: sampling them with other UVs changes nothing.
const PLACEHOLDERS: &[&str] = &["givemechecker", "none"];

/// Samplers read without UVs: a palette is looked up by the diffuse value, not by position.
const UV_FREE_SAMPLERS: &[&str] = &["TextureSamplerDiffPal", "TintPaletteSampler"];

/// UV slack accepted outside `[0, 1]` (export rounding); beyond, the texture tiles.
const UV_EPSILON: f64 = 1e-3;

/// Uniform textures up to this side (placeholders, flat maps) look the same whatever the
/// UVs: they are left whole and do not constrain the window.
const TINY_SIDE: u32 = 8;

/// Largest per-channel spread of a texture still considered uniform.
const UNIFORM_TOLERANCE: u8 = 2;

/// Whether a texture can be left whole although the UVs change: small and uniform.
pub fn exempt(meta: &TextureMeta, data: &[u8]) -> bool {
    if meta.width.max(meta.height) > TINY_SIDE {
        return false;
    }
    let Ok(img) = optistream_texture::codec::decode(meta.format, meta.width, meta.height, data) else {
        return false;
    };
    (0..4).all(|c| {
        let values = img.data.iter().skip(c).step_by(4);
        let (lo, hi) = values.fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        hi - lo <= UNIFORM_TOLERANCE
    })
}

/// Kept area above which cropping is not worth it.
const MAX_AREA: f64 = 0.5;

/// Members of a group before planning.
#[derive(Default)]
struct Members {
    ydds: Vec<String>,
    ytds: Vec<String>,
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

fn name_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, n)| n)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Drawable (lowercased path) a diffuse variation belongs to, if it exists.
fn drawable_of(ytd: &str, files: &HashSet<String>) -> Option<String> {
    naming::drawable_names(name_of(ytd)).into_iter().map(|n| join(dir_of(ytd), &n)).find(|r| files.contains(r))
}

/// Candidate groups: one per drawable that is neither skin-dependent (`_r`, its UVs map the
/// MP skin), a first-person model nor a cloth piece.
fn find_groups(files: &HashSet<String>) -> HashMap<String, Members> {
    let mut groups: HashMap<String, Members> = HashMap::new();
    for rel in files.iter().filter(|r| r.ends_with(".ydd")) {
        let stem = &rel[..rel.len() - 4];
        if stem.ends_with("_r") || stem.ends_with("_1") || files.contains(&format!("{stem}.yld")) {
            continue;
        }
        let mut m = Members { ydds: vec![rel.clone()], ytds: Vec::new() };
        let first_person = format!("{stem}_1.ydd");
        if files.contains(&first_person) {
            if files.contains(&format!("{stem}_1.yld")) {
                continue;
            }
            m.ydds.push(first_person);
        }
        groups.insert(rel.clone(), m);
    }
    for rel in files.iter().filter(|r| r.ends_with(".ytd") && naming::is_variation_texture(name_of(r))) {
        if let Some(m) = drawable_of(rel, files).and_then(|d| groups.get_mut(&d)) {
            m.ytds.push(rel.clone());
        }
    }
    groups
}

/// Plans every group of the resource (from the files as they are on disk).
pub fn plan_run(root: &Path, files: &HashSet<String>) -> RunPlan {
    let groups: Vec<Members> = find_groups(files).into_values().collect();
    let planned: Vec<Arc<Group>> =
        groups.par_iter().filter_map(|m| plan_group(root, m).ok().flatten()).map(Arc::new).collect();
    let mut out = RunPlan::new();
    for g in planned {
        for f in &g.files {
            out.insert(f.clone(), g.clone());
        }
    }
    out
}

/// Every candidate group with its outcome: the window, `None` (nothing to gain) or why it
/// was refused. For diagnostics (`optistream-cli crops`).
pub fn diagnose(root: &Path, files: &HashSet<String>) -> Vec<(String, Result<Option<Window>, String>)> {
    let mut groups: Vec<(String, Members)> = find_groups(files).into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    groups
        .par_iter()
        .map(|(main, m)| (main.clone(), plan_group(root, m).map(|g| g.map(|g| g.window)).map_err(|e| format!("{e:#}"))))
        .collect()
}

/// Plans the group a single file belongs to (comparisons outside a run).
pub fn plan_for(root: &Path, files: &HashSet<String>, rel: &str) -> Option<Arc<Group>> {
    let main = if rel.ends_with(".ytd") {
        drawable_of(rel, files)?
    } else {
        let stem = &rel[..rel.len().saturating_sub(4)];
        stem.strip_suffix("_1").map(|s| format!("{s}.ydd")).filter(|m| files.contains(m)).unwrap_or_else(|| rel.to_owned())
    };
    let mut groups = find_groups(files);
    let m = groups.remove(&main)?;
    plan_group(root, &m).ok().flatten().map(Arc::new)
}

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
type Bounds = [f64; 4];

fn plan_group(root: &Path, m: &Members) -> Result<Option<Group>> {
    let mut extent = Extent::default();
    let mut bounds: Bounds = [f64::MAX, f64::MIN, f64::MAX, f64::MIN];
    let mut max_physical = 0;
    for rel in &m.ydds {
        let bytes = std::fs::read(root.join(rel))?;
        max_physical = max_physical.max(physical_size(&bytes));
        process::crop_ready(Kind::Ydd, &bytes)?;
        let res = Resource::parse(&bytes)?;
        let drawables = drawable::read_dictionary(&res, 0)?;
        ensure!(!drawables.is_empty(), "aucun drawable");
        let mut dicts = HashSet::new();
        for d in &drawables {
            let embedded: HashSet<String> = match d.texture_dict {
                Some(td) => rtex::read_dictionary(&res, td)?.into_iter().map(|e| e.name.to_ascii_lowercase()).collect(),
                None => HashSet::new(),
            };
            check_shaders(d, &embedded)?;
            uv_bounds(&res, d, &mut bounds)?;
            if let Some(td) = d.texture_dict.filter(|&td| dicts.insert(td)) {
                for e in rtex::read_dictionary(&res, td)? {
                    extent.add(&res, &e)?;
                }
            }
        }
    }
    for rel in &m.ytds {
        let bytes = std::fs::read(root.join(rel))?;
        max_physical = max_physical.max(physical_size(&bytes));
        process::crop_ready(Kind::Ytd, &bytes)?;
        let res = Resource::parse(&bytes)?;
        for e in rtex::read_dictionary(&res, 0)? {
            extent.add(&res, &e)?;
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
/// embedded normal and specular maps, or the unused-sampler placeholder.
fn check_shaders(d: &DrawableInfo, embedded: &HashSet<String>) -> Result<()> {
    for s in &d.shaders {
        let name = shaders::shader_name(s).unwrap_or("?");
        ensure!(CROP_SHADERS.contains(&name), "shader {name}");
        for (param, tex) in &s.textures {
            let tex = tex.to_ascii_lowercase();
            if tex.is_empty() || PLACEHOLDERS.contains(&tex.as_str()) || UV_FREE_SAMPLERS.iter().any(|s| joaat(s) == *param) {
                continue;
            }
            match shaders::sampler_usage(*param) {
                Some(Usage::Role(TextureRole::Diffuse)) => {}
                Some(Usage::Role(TextureRole::Normal | TextureRole::Specular)) => {
                    ensure!(embedded.contains(&tex), "texture externe {tex}")
                }
                _ => anyhow::bail!("sampler 0x{param:08X} ({tex})"),
            }
        }
    }
    Ok(())
}

/// Reads TEXCOORD0 of a vertex (`Float2` or `Half2`).
fn read_uv(v: &[u8], off: usize, ty: u8) -> [f32; 2] {
    match ty {
        component::FLOAT2 => [0, 4].map(|o| f32::from_le_bytes(v[off + o..off + o + 4].try_into().unwrap())),
        _ => [0, 2].map(|o| half_to_f32(u16::from_le_bytes([v[off + o], v[off + o + 1]]))),
    }
}

fn write_uv(v: &mut [u8], off: usize, ty: u8, uv: [f32; 2]) {
    for (i, x) in uv.into_iter().enumerate() {
        match ty {
            component::FLOAT2 => v[off + 4 * i..off + 4 * i + 4].copy_from_slice(&x.to_le_bytes()),
            _ => v[off + 2 * i..off + 2 * i + 2].copy_from_slice(&f32_to_half(x).to_le_bytes()),
        }
    }
}

/// TEXCOORD0 layout of a geometry: `None` when it has none (nothing sampled).
fn uv_layout(g: &GeometryInfo) -> Result<Option<(usize, u8)>> {
    ensure!(!g.decl.is_cloth(), "géométrie cloth");
    if !g.decl.has(semantic::TEXCOORD0) {
        return Ok(None);
    }
    let ty = g.decl.component_type(semantic::TEXCOORD0);
    ensure!(matches!(ty, component::FLOAT2 | component::HALF2), "UV de type {ty}");
    Ok(Some((g.decl.component_offset(semantic::TEXCOORD0), ty)))
}

/// Grows `b` with the UVs of the vertices the triangles actually use.
fn uv_bounds(res: &Resource, d: &DrawableInfo, b: &mut Bounds) -> Result<()> {
    for g in d.geometries() {
        let Some((off, ty)) = uv_layout(g)? else { continue };
        let stride = g.vertex_stride as usize;
        let ptr = if g.vertex_ptr != 0 { g.vertex_ptr } else { g.vertex_ptr2 };
        ensure!(matches!(res.resolve(ptr), Some((Segment::System, _))), "sommets hors du segment système");
        let verts = res.slice(ptr, g.vertex_count as usize * stride)?;
        let indices = res.slice(g.index_ptr, g.index_count as usize * 2)?;
        for i in indices.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize) {
            ensure!(i < g.vertex_count as usize, "indice hors du tampon de sommets");
            let [u, v] = read_uv(&verts[i * stride..], off, ty);
            ensure!(u.is_finite() && v.is_finite(), "UV invalide");
            b[0] = b[0].min(u as f64);
            b[1] = b[1].max(u as f64);
            b[2] = b[2].min(v as f64);
            b[3] = b[3].max(v as f64);
        }
    }
    Ok(())
}

/// Smallest power-of-two part of an axis holding `[lo, hi]` plus a safety margin, starting
/// on a 4-texel boundary of the smallest texture (hence of every texture of the group).
fn axis(lo: f64, hi: f64, min_n: u32, max_n: u32) -> Axis {
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

/// Rewrites TEXCOORD0 of every vertex of the drawables into the window.
pub fn remap_uvs(res: &mut Resource, drawables: &[DrawableInfo], w: &Window) -> Result<usize> {
    let mut seen = HashSet::new();
    let mut count = 0;
    for g in drawables.iter().flat_map(|d| d.geometries()) {
        let Some((off, ty)) = uv_layout(g)? else { continue };
        let stride = g.vertex_stride as usize;
        // Decimation may have shortened the buffer: read its current vertex count.
        let vcount = res.u32_at(g.vb_offset + vb_field::VERTEX_COUNT)? as usize;
        let mut targets = vec![g.vertex_ptr, g.vertex_ptr2];
        targets.retain(|&p| p != 0);
        targets.dedup();
        for ptr in targets {
            let start = res.sys_offset(ptr)?;
            if !seen.insert(start) {
                continue;
            }
            for v in res.system[start..start + vcount * stride].chunks_exact_mut(stride) {
                let uv = w.map(read_uv(v, off, ty));
                write_uv(v, off, ty, uv);
            }
            count += vcount;
        }
    }
    Ok(count)
}

/// Top level of a texture cut to the window (block for block for DXT), or `None` for small
/// uniform textures, which are left whole ([`exempt`]). `data` holds at least the top level.
pub fn crop_texture(meta: &TextureMeta, data: &[u8], w: &Window) -> Result<Option<(TextureMeta, Vec<u8>)>> {
    if exempt(meta, data) {
        return Ok(None);
    }
    let (x0, y0, cw, ch) = w.rect(meta.width, meta.height);
    ensure!(x0 + cw <= meta.width && y0 + ch <= meta.height && cw > 0 && ch > 0, "fenêtre hors de la texture");
    let (unit, bytes) = match fmt::layout(meta.format) {
        Some(Layout::Block(b)) => (4, b),
        Some(Layout::Pixel(b)) => (1, b),
        None => anyhow::bail!("format non géré"),
    };
    ensure!(x0 % unit == 0 && y0 % unit == 0 && cw % unit == 0 && ch % unit == 0, "fenêtre non alignée sur les blocs");
    let row = (meta.width / unit) as usize * bytes;
    let (cols, rows) = ((cw / unit) as usize, (ch / unit) as usize);
    ensure!(data.len() >= row * (meta.height / unit) as usize, "données de texture tronquées");
    let mut out = Vec::with_capacity(cols * rows * bytes);
    for r in 0..rows {
        let start = (y0 / unit) as usize * row + r * row + (x0 / unit) as usize * bytes;
        out.extend_from_slice(&data[start..start + cols * bytes]);
    }
    Ok(Some((TextureMeta { width: cw, height: ch, format: meta.format, levels: 1 }, out)))
}

fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
        (0x1F, m) => sign | 0x7F80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

fn f32_to_half(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let man = bits & 0x7F_FFFF;
    if exp >= 0x1F {
        return sign | 0x7C00;
    }
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    // Round to nearest; a carry into the exponent is the correct result.
    sign | (((exp as u32) << 10) + ((man + 0x1000) >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use optistream_rage::texture::format as f;

    #[test]
    fn axis_is_a_power_of_two_holding_the_used_range() {
        // Vestium's measured case: 0.22 of a 4096 axis is used.
        let a = axis(0.40, 0.62, 4096, 4096);
        assert_eq!(a.size, 0.25);
        assert!(a.start <= 0.40 && a.start + a.size >= 0.62);
        assert_eq!((a.start * 4096.0) as u32 % 4, 0);
        // Nearly everything used: nothing to gain.
        assert_eq!(axis(0.02, 0.97, 2048, 2048), Axis::FULL);
        // Near the edge, the window is pushed back inside the texture.
        let a = axis(0.80, 0.99, 1024, 1024);
        assert!(a.start + a.size <= 1.0 && a.start <= 0.80);
    }

    #[test]
    fn window_is_aligned_for_every_texture_size() {
        let a = axis(0.13, 0.30, 512, 2048);
        for n in [512u32, 1024, 2048] {
            assert_eq!((a.start * n as f64).round() as u32 % 4, 0, "{n}");
        }
    }

    #[test]
    fn remapped_uvs_land_in_the_same_texels() {
        let w = Window { u: Axis { start: 0.25, size: 0.5 }, v: Axis { start: 0.5, size: 0.25 } };
        let (x0, y0, cw, ch) = w.rect(1024, 1024);
        let uv = [0.4f32, 0.6f32];
        let m = w.map(uv);
        // Same pixel in the cropped texture as in the original one.
        assert!(((m[0] * cw as f32) + x0 as f32 - uv[0] * 1024.0).abs() < 1e-3);
        assert!(((m[1] * ch as f32) + y0 as f32 - uv[1] * 1024.0).abs() < 1e-3);
    }

    #[test]
    fn dxt_crop_copies_blocks() {
        // 16x16 DXT1: block (bx, by) holds bytes all equal to bx + 4 * by.
        let mut data = Vec::new();
        for by in 0..4u8 {
            for bx in 0..4u8 {
                data.extend_from_slice(&[bx + 4 * by; 8]);
            }
        }
        let meta = TextureMeta { width: 16, height: 16, format: f::DXT1, levels: 1 };
        let w = Window { u: Axis { start: 0.5, size: 0.5 }, v: Axis { start: 0.25, size: 0.5 } };
        let (m, out) = crop_texture(&meta, &data, &w).unwrap().unwrap();
        assert_eq!((m.width, m.height, m.levels), (8, 8, 1));
        let firsts: Vec<u8> = out.chunks_exact(8).map(|b| b[0]).collect();
        assert_eq!(firsts, [6, 7, 10, 11]);
    }

    #[test]
    fn only_small_uniform_textures_are_exempt() {
        let meta = TextureMeta { width: 8, height: 8, format: f::A8R8G8B8, levels: 1 };
        let flat = [90u8, 90, 90, 255].repeat(64);
        assert!(exempt(&meta, &flat));
        // A small texture with a pattern is sampled differently once the UVs change.
        let checker: Vec<u8> = (0..64).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
        assert!(!exempt(&meta, &checker));
        let big = TextureMeta { width: 64, height: 64, ..meta };
        assert!(!exempt(&big, &[90u8, 90, 90, 255].repeat(64 * 64)));
    }

    #[test]
    fn half_round_trip() {
        for x in [0.0f32, 0.25, 0.5, 0.999, 1.0, -0.3, 2.5] {
            assert!((half_to_f32(f32_to_half(x)) - x).abs() < 1e-3, "{x}");
        }
    }
}
