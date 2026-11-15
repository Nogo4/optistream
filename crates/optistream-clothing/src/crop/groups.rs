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

#[cfg(test)]
mod tests {
    use super::*;

    fn set(files: &[&str]) -> HashSet<String> {
        files.iter().map(|f| f.to_string()).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn path_helpers_split_and_join_on_the_last_slash() {
        assert_eq!(dir_of("a/b/jbib_000_u.ydd"), "a/b");
        assert_eq!(name_of("a/b/jbib_000_u.ydd"), "jbib_000_u.ydd");
        assert_eq!(dir_of("jbib_000_u.ydd"), "");
        assert_eq!(name_of("jbib_000_u.ydd"), "jbib_000_u.ydd");
        assert_eq!(join("", "x.ydd"), "x.ydd");
        assert_eq!(join("a/b", "x.ydd"), "a/b/x.ydd");
        for rel in ["a/b/x.ydd", "x.ydd"] {
            assert_eq!(join(dir_of(rel), name_of(rel)), rel);
        }
    }

    #[test]
    fn drawable_of_finds_the_drawable_in_the_same_folder_only() {
        let files = set(&["m^jbib_048_u.ydd", "s/m^lowr_003_r.ydd", "props/p_head_010.ydd"]);
        assert_eq!(drawable_of("m^jbib_diff_048_a_uni.ytd", &files).as_deref(), Some("m^jbib_048_u.ydd"));
        assert_eq!(drawable_of("s/m^lowr_diff_003_b_whi.ytd", &files).as_deref(), Some("s/m^lowr_003_r.ydd"));
        assert_eq!(drawable_of("props/p_head_diff_010_c.ytd", &files).as_deref(), Some("props/p_head_010.ydd"));
        // Same name, other folder or other index: not this drawable.
        assert_eq!(drawable_of("other/m^jbib_diff_048_a_uni.ytd", &files), None);
        assert_eq!(drawable_of("m^jbib_diff_049_a_uni.ytd", &files), None);
        // Not a variation at all.
        assert_eq!(drawable_of("m^jbib_048_u.ytd", &files), None);
    }

    #[test]
    fn drawable_of_prefers_the_universal_drawable() {
        let files = set(&["jbib_001_u.ydd", "jbib_001_r.ydd", "jbib_001.ydd"]);
        assert_eq!(drawable_of("jbib_diff_001_a_uni.ytd", &files).as_deref(), Some("jbib_001_u.ydd"));
    }

    #[test]
    fn find_groups_gathers_a_drawable_its_first_person_model_and_variations() {
        let files = set(&[
            "c/m^jbib_000_u.ydd",
            "c/m^jbib_000_u_1.ydd",
            "c/m^jbib_diff_000_a_uni.ytd",
            "c/m^jbib_diff_000_b_uni.ytd",
            // Embedded-style dictionary sharing the drawable name: not a variation.
            "c/m^jbib_000_u.ytd",
            // Variation of a drawable that does not exist.
            "c/m^jbib_diff_007_a_uni.ytd",
            "c/m^lowr_001_u.ydd",
        ]);
        let g = find_groups(&files);
        assert_eq!(sorted(g.keys().cloned().collect()), ["c/m^jbib_000_u.ydd", "c/m^lowr_001_u.ydd"]);
        let m = &g["c/m^jbib_000_u.ydd"];
        assert_eq!(m.ydds, ["c/m^jbib_000_u.ydd", "c/m^jbib_000_u_1.ydd"]);
        assert_eq!(sorted(m.ytds.clone()), ["c/m^jbib_diff_000_a_uni.ytd", "c/m^jbib_diff_000_b_uni.ytd"]);
        let lowr = &g["c/m^lowr_001_u.ydd"];
        assert_eq!(lowr.ydds, ["c/m^lowr_001_u.ydd"]);
        assert!(lowr.ytds.is_empty());
    }

    #[test]
    fn find_groups_skips_skin_cloth_and_first_person_only_pieces() {
        let files = set(&[
            // Skin-dependent: its variations are not attached anywhere else.
            "uppr_001_r.ydd",
            "uppr_diff_001_a_whi.ytd",
            // Cloth piece.
            "jbib_002_u.ydd",
            "jbib_002_u.yld",
            // First-person model with cloth: the whole group is refused.
            "jbib_003_u.ydd",
            "jbib_003_u_1.ydd",
            "jbib_003_u_1.yld",
            // A lone first-person model never leads a group.
            "jbib_004_u_1.ydd",
        ]);
        assert!(find_groups(&files).is_empty());
    }

    #[test]
    fn find_groups_handles_props_and_root_level_files() {
        let files = set(&["p_head_005.ydd", "p_head_diff_005_a.ytd", "p_head_diff_005_b.ytd", "p_eyes_diff_005_a.ytd"]);
        let g = find_groups(&files);
        assert_eq!(g.len(), 1);
        assert_eq!(sorted(g["p_head_005.ydd"].ytds.clone()), ["p_head_diff_005_a.ytd", "p_head_diff_005_b.ytd"]);
    }

    #[test]
    fn every_file_belongs_to_at_most_one_group() {
        let files = set(&[
            "a/jbib_000_u.ydd",
            "a/jbib_000_u_1.ydd",
            "a/jbib_diff_000_a_uni.ytd",
            "b/jbib_000_u.ydd",
            "b/jbib_diff_000_a_uni.ytd",
            "a/jbib_001_u.ydd",
            "a/jbib_diff_001_a_uni.ytd",
        ]);
        let g = find_groups(&files);
        assert_eq!(g.len(), 3);
        let mut all: Vec<&String> = g.values().flat_map(|m| m.ydds.iter().chain(&m.ytds)).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        assert_eq!(n, files.len());
        assert_eq!(g["b/jbib_000_u.ydd"].ytds, ["b/jbib_diff_000_a_uni.ytd"]);
    }
}
