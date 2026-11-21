//! Analysis of one parsed vehicle file: texture issues and the memory budget.

use optistream_core::{mib, FileAnalysis, Issue};
use optistream_i18n::{t, tf};
use optistream_rage::texture::format as tf;
use optistream_texture::format::{self as fmt, full_levels, level_dims};
use optistream_texture::plan;

use crate::config::Config;
use crate::process::parse::tex_data;
use crate::process::{Class, Parsed, Tex};
use optistream_mapping::roles::Usage;

fn tex_issues(t: &Tex, data: Option<&[u8]>, cfg: &Config, out: &mut Vec<Issue>) {
    let e = &t.entry;
    let Usage::Role(role) = t.usage else { return };
    let policy = cfg.texture_policy(0);
    let (w, h) = (e.width as u32, e.height as u32);
    let mut problems = Vec::new();
    if policy.force_pot && (!fmt::is_pot(w) || !fmt::is_pot(h)) {
        problems.push(optistream_i18n::t("vehicle.issue.not_pot").to_owned());
    }
    if policy.convert_formats {
        match e.format {
            tf::BC7 | tf::DXT3 => problems.push(tf!("vehicle.issue.format", format = fmt::name(e.format))),
            tf::A8R8G8B8 | tf::A8B8G8R8 | tf::X8R8G8B8 | tf::A1R5G5B5 | tf::A16B16G16R16 => {
                problems.push(tf!("vehicle.issue.uncompressed", format = fmt::name(e.format)))
            }
            _ => {}
        }
    }
    if fmt::layout(e.format).is_none() {
        out.push(Issue::info(tf!("vehicle.issue.unsupported_format", name = e.name, role = role.label(), format = fmt::name(e.format))));
    }
    let full = full_levels(w, h);
    if policy.fix_mips && (e.levels as u32) < full {
        let (tw, th) = plan::target_dims(&t.meta(), &policy);
        let kept_as_stored = (0..(e.levels as u32).max(1)).any(|l| level_dims(w, h, l) == (tw, th));
        if !(kept_as_stored && data.is_some_and(|d| plan::punch_through_blocks_mips(&t.meta(), d))) {
            problems.push(if e.levels <= 1 { optistream_i18n::t("vehicle.issue.no_mips").to_owned() } else { tf!("vehicle.issue.incomplete_mips", levels = e.levels, full = full) });
        }
    }
    if !problems.is_empty() {
        out.push(Issue::warning(tf!("vehicle.issue.texture", name = e.name, label = t.label(), problems = problems.join(", "))));
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
        parts.push(tf!("vehicle.summary.textures", count = p.textures.len(), max = max));
    } else {
        parts.push(t("vehicle.summary.no_textures").to_owned());
    }
    let physical = p.res.graphics_flags.size();
    if cfg.budget_enabled && physical > cfg.budget && !p.textures.is_empty() {
        a.issues.push(Issue::info(tf!("vehicle.issue.over_budget", budget = mib(cfg.budget))));
    }
    let liveries = p.textures.iter().filter(|t| t.livery).count();
    let hi = p.textures.iter().filter(|t| t.class == Class::HiCopy).count();
    if liveries > 0 || hi > 0 {
        a.issues.push(Issue::info(tf!("vehicle.issue.liveries", liveries = liveries, hi = hi)));
    }
    let protected = p.textures.iter().filter(|t| t.usage == Usage::Protected).count();
    if protected > 0 {
        a.issues.push(Issue::info(tf!("vehicle.issue.protected", count = protected)));
    }
    let tris: u64 = p.drawables.iter().map(|d| d.lod_triangles(0)).sum();
    if tris > 0 {
        a.triangles = Some(tris);
        parts.push(tf!("vehicle.summary.triangles", count = tris));
    }
    let virt = p.res.system_flags.size();
    if p.kind != crate::process::Kind::Ytd && virt > cfg.budget {
        let lods: Vec<String> =
            (0..4).map(|l| p.drawables.iter().map(|d| d.lod_triangles(l)).sum::<u64>().to_string()).collect();
        let text = tf!("vehicle.issue.virtual_over_budget", virt = mib(virt), budget = mib(cfg.budget), lods = lods.join(" / "));
        a.issues.push(if cfg.geo_enabled {
            Issue::info(tf!("vehicle.issue.geo_reduced", text = text))
        } else {
            Issue::warning(tf!("vehicle.issue.geo_disabled", text = text))
        });
    }
    a.summary = parts.join(" · ");
    a
}

// ---------------------------------------------------------------- planning
