//! Groups of files sharing one UV layout: drawable, first-person model and variations.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;

use crate::crop::plan::plan_group;
use crate::crop::{Group, RunPlan, Window};
use crate::naming;

/// Kept area above which cropping is not worth it.
pub(super) const MAX_AREA: f64 = 0.5;

/// Members of a group before planning.
#[derive(Default)]
pub(super) struct Members {
    pub(super) ydds: Vec<String>,
    pub(super) ytds: Vec<String>,
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

fn name_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, n)| n)
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Drawable (lowercased path) a diffuse variation belongs to, if it exists.
pub(crate) fn drawable_of(ytd: &str, files: &HashSet<String>) -> Option<String> {
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
