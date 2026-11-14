//! Analysis of one parsed file: texture issues and polygon budgets.

use optistream_core::{FileAnalysis, Issue};
use optistream_rage::drawable::DrawableInfo;
use optistream_rage::texture::format as tf;
use optistream_texture::format::{self as fmt};
use optistream_texture::plan;

use crate::config::Config;
use crate::process::parse::{role_of, tex_data};
use crate::process::{group, FileInfo, Kind, Parsed, TexItem};
use crate::shaders;

fn tex_issues(t: &TexItem, data: Option<&[u8]>, cfg: &Config, skin: bool, out: &mut Vec<Issue>) {
    let e = &t.entry;
    let name = &e.name;
    let Some(role) = role_of(t.usage) else {
        return;
    };
    let policy = cfg.texture_policy(role, skin, t.alpha_mask);
    let (w, h) = (e.width as u32, e.height as u32);
    let mut problems = Vec::new();
    if !fmt::is_pot(w) || !fmt::is_pot(h) {
        if Config::keeps_dims(role, skin) {
            out.push(Issue::info(format!("texture {name} ({}) : {w}×{h}, dimensions conservées (diffuse de peau _r)", role.label())));
        } else if policy.force_pot {
            problems.push("pas une puissance de 2".to_owned());
        }
    }
    if policy.convert_formats {
        match e.format {
            tf::BC7 | tf::DXT3 => problems.push(format!("format {}", fmt::name(e.format))),
            tf::A8R8G8B8 | tf::A8B8G8R8 | tf::X8R8G8B8 | tf::A1R5G5B5 | tf::A16B16G16R16 => {
                problems.push(format!("non compressée ({})", fmt::name(e.format)))
            }
            _ => {}
        }
    }
    if fmt::layout(e.format).is_none() {
        // Left as it is by the optimiser: say so, such a texture is often the heavy one.
        out.push(Issue::info(format!("texture {name} ({}) : format {} non géré, laissée telle quelle", role.label(), fmt::name(e.format))));
    }
    let full = fmt::full_levels(w, h);
    if policy.fix_mips && (e.levels as u32) < full {
        // Kept at a stored size (no re-encoding), a punch-through DXT1 keeps its short chain:
        // nothing the optimiser can do without a 1-bit alpha DXT1 encoder, so not a warning.
        let (tw, th) = plan::target_dims(&t.meta(), &policy);
        let kept_as_stored = (0..(e.levels as u32).max(1)).any(|l| fmt::level_dims(w, h, l) == (tw, th));
        if kept_as_stored && data.is_some_and(|d| plan::punch_through_blocks_mips(&t.meta(), d)) {
            out.push(Issue::info(format!(
                "texture {name} ({}) : mipmaps incomplets ({}/{full}), non complétables : alpha 1 bit DXT1",
                role.label(),
                e.levels
            )));
        } else {
            problems.push(if e.levels <= 1 { "sans mipmaps".to_owned() } else { format!("mipmaps incomplets ({}/{full})", e.levels) });
        }
    }
    if !problems.is_empty() {
        out.push(Issue::warning(format!("texture {} ({}) : {}", name, role.label(), problems.join(", "))));
    }
}

/// Heaviest High LOD among the drawables the optimiser may decimate (the polygon budget
/// applies per drawable), with its index.
pub(super) fn heaviest_high(drawables: &[DrawableInfo]) -> Option<(usize, u64)> {
    drawables
        .iter()
        .enumerate()
        .filter(|(_, d)| !has_cloth(d))
        .map(|(i, d)| (i, d.lod_triangles(0)))
        .max_by_key(|&(i, t)| (t, std::cmp::Reverse(i)))
}

pub fn analyze(p: &Parsed, cfg: &Config, info: &FileInfo) -> FileAnalysis {
    let (slot, cloth_file) = (info.slot.as_str(), info.cloth);
    let mut a = FileAnalysis {
        kind: match p.kind {
            Kind::Ytd => "ytd".into(),
            Kind::Ydd => "ydd".into(),
        },
        texture_count: p.textures.len(),
        ..Default::default()
    };
    for t in &p.textures {
        tex_issues(t, tex_data(&p.res, t), cfg, info.skin, &mut a.issues);
    }
    let mut parts = Vec::new();
    if !p.textures.is_empty() {
        let max = p.textures.iter().map(|t| t.entry.width.max(t.entry.height)).max().unwrap_or(0);
        parts.push(format!("{} texture(s), max {max} px", p.textures.len()));
    }
    if p.kind == Kind::Ydd {
        let tris: u64 = p.drawables.iter().map(|d| d.lod_triangles(0)).sum();
        a.triangles = Some(tris);
        parts.push(format!("High {} tris", group(tris)));
        let budget = cfg.budget_for_slot(slot);
        if cloth_file || p.drawables.iter().any(has_cloth) {
            a.issues.push(Issue::info("cloth : maillage protégé (non décimé)"));
        }
        if !cloth_file {
            if let Some((i, worst)) = heaviest_high(&p.drawables).filter(|&(_, t)| t > budget) {
                let which = if p.drawables.len() > 1 { format!(", drawable #{i}") } else { String::new() };
                a.issues.push(Issue::warning(format!(
                    "LOD High {} triangles > budget {} ({slot}{which})",
                    group(worst),
                    group(budget)
                )));
            }
        }
    }
    a.summary = parts.join(" · ");
    a
}

pub(super) fn has_cloth(d: &DrawableInfo) -> bool {
    d.shaders.iter().any(shaders::is_cloth_shader) || d.geometries().any(|g| g.decl.is_cloth())
}
