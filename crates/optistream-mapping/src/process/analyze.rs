//! Analysis of one parsed file: texture issues and the memory budget.

use optistream_core::{mib, FileAnalysis, Issue};
use optistream_rage::texture::format as tf;
use optistream_texture::format::{self as fmt, full_levels, level_dims};
use optistream_texture::plan;

use crate::config::Config;
use crate::process::parse::tex_data;
use crate::process::{Parsed, Tex};
use crate::roles::Usage;

fn tex_issues(t: &Tex, data: Option<&[u8]>, cfg: &Config, out: &mut Vec<Issue>) {
    let e = &t.entry;
    let Usage::Role(role) = t.usage else { return };
    let policy = cfg.texture_policy(0);
    let (w, h) = (e.width as u32, e.height as u32);
    let mut problems = Vec::new();
    if policy.force_pot && (!fmt::is_pot(w) || !fmt::is_pot(h)) {
        problems.push("pas une puissance de 2".to_owned());
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
        out.push(Issue::info(format!("texture {} ({}) : format {} non géré, laissée telle quelle", e.name, role.label(), fmt::name(e.format))));
    }
    let full = full_levels(w, h);
    if policy.fix_mips && (e.levels as u32) < full {
        let (tw, th) = plan::target_dims(&t.meta(), &policy);
        let kept_as_stored = (0..(e.levels as u32).max(1)).any(|l| level_dims(w, h, l) == (tw, th));
        if !(kept_as_stored && data.is_some_and(|d| plan::punch_through_blocks_mips(&t.meta(), d))) {
            problems.push(if e.levels <= 1 { "sans mipmaps".to_owned() } else { format!("mipmaps incomplets ({}/{full})", e.levels) });
        }
    }
    if !problems.is_empty() {
        out.push(Issue::warning(format!("texture {} ({}) : {}", e.name, role.label(), problems.join(", "))));
    }
}

pub fn analyze(p: &Parsed, cfg: &Config) -> FileAnalysis {
    let mut a = FileAnalysis { kind: p.kind.label().into(), texture_count: p.textures.len(), ..Default::default() };
    for t in &p.textures {
        tex_issues(t, tex_data(&p.res, t), cfg, &mut a.issues);
    }
    let mut parts = Vec::new();
    if !p.textures.is_empty() {
        let max = p.textures.iter().map(|t| t.entry.width.max(t.entry.height)).max().unwrap_or(0);
        parts.push(format!("{} texture(s), max {max} px", p.textures.len()));
    } else {
        parts.push("aucune texture".to_owned());
    }
    let physical = p.res.graphics_flags.size();
    if cfg.budget_enabled && physical > cfg.budget && !p.textures.is_empty() {
        a.issues.push(Issue::info(format!(
            "au-dessus du budget de {} : réduction ciblée des textures qui perdent le moins",
            mib(cfg.budget)
        )));
    }
    let protected = p.textures.iter().filter(|t| t.usage == Usage::Protected).count();
    if protected > 0 {
        a.issues.push(Issue::info(format!("{protected} texture(s) protégée(s) (émissive, palette, lookup…) : non modifiées")));
    }
    a.summary = parts.join(" · ");
    a
}

// ---------------------------------------------------------------- planning
