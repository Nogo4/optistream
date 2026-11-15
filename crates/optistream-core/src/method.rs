//! The extension point: one `Method` per kind of resource (clothing today; vehicles,
//! MLOs, ... later). The pipeline and the UI only talk to this trait.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use crate::settings::{ParamSpec, Preset, Settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub text: String,
}

impl Issue {
    pub fn info(text: impl Into<String>) -> Self {
        Issue { severity: Severity::Info, text: text.into() }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Issue { severity: Severity::Warning, text: text.into() }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Issue { severity: Severity::Error, text: text.into() }
    }
}

/// Context of one file inside the resource being processed.
#[derive(Clone, Debug)]
pub struct FileCtx {
    pub root: PathBuf,
    /// Path relative to `root`.
    pub rel: PathBuf,
    /// Lowercased relative paths (with `/`) of every file in the resource.
    pub all_files: Arc<HashSet<String>>,
}

impl FileCtx {
    pub fn file_name_lower(&self) -> String {
        self.rel.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
    }

    pub fn rel_lower(&self) -> String {
        normalize_rel(&self.rel)
    }

    /// Folder (relative to `root`, original case) of the resource holding this file: its
    /// nearest ancestor with a manifest, or the root itself. A category folder such as
    /// `[oren-mappings]` holds many resources.
    pub fn resource_dir(&self) -> PathBuf {
        let mut dir = self.rel.parent();
        while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            if MANIFESTS.iter().any(|m| self.all_files.contains(&normalize_rel(&d.join(m)))) {
                return d.to_path_buf();
            }
            dir = d.parent();
        }
        PathBuf::new()
    }

    /// Whether a file with the same stem but another extension exists next to this one.
    pub fn has_sibling_with_ext(&self, ext: &str) -> bool {
        let rel = self.rel_lower();
        match rel.rfind('.') {
            Some(dot) => self.all_files.contains(&format!("{}.{ext}", &rel[..dot])),
            None => false,
        }
    }
}

pub fn normalize_rel(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// File names marking the folder of a FiveM resource.
pub const MANIFESTS: [&str; 2] = ["fxmanifest.lua", "__resource.lua"];

/// Lowercased form of [`FileCtx::resource_dir`] working on normalized paths (`""` = root).
pub fn resource_prefix(rel_lower: &str, all_files: &HashSet<String>) -> String {
    let mut dir = rel_lower;
    while let Some(slash) = dir.rfind('/') {
        dir = &dir[..slash];
        if MANIFESTS.iter().any(|m| all_files.contains(&format!("{dir}/{m}"))) {
            return dir.to_owned();
        }
    }
    String::new()
}

/// Analysis of one file, computed without modifying anything.
#[derive(Clone, Debug, Default, Serialize)]
pub struct FileAnalysis {
    /// Short kind label ("ydd", "ytd", ...).
    pub kind: String,
    /// Main figures shown in the table (e.g. "LOD High: 107 342 tris").
    pub summary: String,
    /// High LOD triangle count, when relevant.
    pub triangles: Option<u64>,
    pub texture_count: usize,
    pub issues: Vec<Issue>,
    /// The current settings leave this file untouched (threshold, file type filter...):
    /// its size warnings are informational, there is nothing left to fix with these settings.
    pub skipped: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FileOutcome {
    /// New file content, `None` when the file is left unchanged.
    pub bytes: Option<Vec<u8>>,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
}

/// RGBA8 image for comparisons.
#[derive(Clone, Debug)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Triangle mesh for comparisons (positions only).
#[derive(Clone, Debug, Default)]
pub struct PreviewMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl PreviewMesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug)]
pub enum PreviewContent {
    Texture { before: PreviewImage, after: Option<PreviewImage> },
    Mesh { before: PreviewMesh, after: Option<PreviewMesh> },
}

#[derive(Clone, Debug)]
pub struct PreviewItem {
    pub title: String,
    pub before_desc: String,
    pub after_desc: String,
    pub content: PreviewContent,
}

pub trait Method: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn description(&self) -> &'static str;

    fn params(&self) -> Vec<ParamSpec>;
    fn presets(&self) -> Vec<Preset>;
    fn default_preset(&self) -> &'static str;

    /// Whether this method processes the file (others are copied untouched on export).
    fn handles(&self, ctx: &FileCtx) -> bool;

    fn analyze_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileAnalysis>;

    fn optimize_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileOutcome>;

    /// Checks spanning the whole resource (e.g. drawable counts per component).
    fn resource_checks(&self, ctx_files: &[FileCtx]) -> Vec<Issue>;

    /// Called once at the start of every analysis and optimisation, with every file of the
    /// resource, before any file is read or written: decisions spanning several files are
    /// taken here, from the files as they are before the run.
    fn prepare(&self, _files: &[FileCtx], _settings: &Settings) -> Result<()> {
        Ok(())
    }

    /// Files (lowercased relative paths, see [`normalize_rel`]) that must be processed in the
    /// same run as this one: a selection containing it is extended to them.
    fn linked_files(&self, _ctx: &FileCtx) -> Vec<String> {
        Vec::new()
    }

    /// Before/after comparisons for one file, computed without writing anything.
    fn preview(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<Vec<PreviewItem>>;

    fn default_settings(&self) -> Settings {
        self.presets()
            .into_iter()
            .find(|p| p.id == self.default_preset())
            .map(|p| p.values)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(rel: &Path, files: &[&str]) -> FileCtx {
        FileCtx {
            root: PathBuf::from("root"),
            rel: rel.to_path_buf(),
            all_files: Arc::new(files.iter().map(|f| f.to_string()).collect()),
        }
    }

    #[test]
    fn normalize_rel_lowercases_and_uses_forward_slashes() {
        assert_eq!(normalize_rel(Path::new("Stream\\Sub/A.YDD")), "stream/sub/a.ydd");
        assert_eq!(normalize_rel(&Path::new("Cat").join("Res").join("X.ytd")), "cat/res/x.ytd");
    }

    #[test]
    fn file_name_and_rel_are_lowercased() {
        let c = ctx(&Path::new("Stream").join("Hat_000_U.YDD"), &[]);
        assert_eq!(c.file_name_lower(), "hat_000_u.ydd");
        assert_eq!(c.rel_lower(), "stream/hat_000_u.ydd");
    }

    #[test]
    fn resource_dir_is_the_nearest_folder_with_a_manifest() {
        let files = ["[cat]/res1/fxmanifest.lua", "[cat]/res2/__resource.lua"];
        let rel = Path::new("[Cat]").join("Res1").join("stream").join("a.ydd");
        assert_eq!(ctx(&rel, &files).resource_dir(), Path::new("[Cat]").join("Res1"));
        let rel = Path::new("[cat]").join("res2").join("a.ydd");
        assert_eq!(ctx(&rel, &files).resource_dir(), Path::new("[cat]").join("res2"));
    }

    #[test]
    fn resource_dir_defaults_to_the_root() {
        let rel = Path::new("stream").join("a.ydd");
        assert_eq!(ctx(&rel, &["fxmanifest.lua"]).resource_dir(), PathBuf::new());
        assert_eq!(ctx(Path::new("a.ydd"), &[]).resource_dir(), PathBuf::new());
    }

    #[test]
    fn resource_prefix_matches_resource_dir() {
        let files: HashSet<String> = ["[cat]/res1/fxmanifest.lua".to_owned()].into();
        assert_eq!(resource_prefix("[cat]/res1/stream/a.ydd", &files), "[cat]/res1");
        assert_eq!(resource_prefix("[cat]/other/a.ydd", &files), "");
        assert_eq!(resource_prefix("a.ydd", &files), "");
    }

    #[test]
    fn sibling_with_another_extension() {
        let files = ["stream/a.ydd", "stream/a.ytd", "stream/noext"];
        let c = ctx(&Path::new("stream").join("A.YDD"), &files);
        assert!(c.has_sibling_with_ext("ytd"));
        assert!(!c.has_sibling_with_ext("yft"));
        assert!(!ctx(&Path::new("stream").join("noext"), &files).has_sibling_with_ext("ytd"));
    }

    #[test]
    fn severities_are_ordered() {
        assert!(Severity::Info < Severity::Warning && Severity::Warning < Severity::Error);
        assert_eq!(Issue::warning("w").severity, Severity::Warning);
        assert_eq!(Issue::error(String::from("e")).text, "e");
    }

    #[test]
    fn preview_mesh_triangles() {
        let m = PreviewMesh { positions: vec![[0.0; 3]; 4], indices: vec![0, 1, 2, 2, 1, 3, 0] };
        assert_eq!(m.triangles(), 2);
    }

    struct Presets(&'static str);

    impl Method for Presets {
        fn id(&self) -> &'static str {
            "p"
        }
        fn label(&self) -> &'static str {
            "P"
        }
        fn description(&self) -> &'static str {
            ""
        }
        fn params(&self) -> Vec<ParamSpec> {
            Vec::new()
        }
        fn presets(&self) -> Vec<Preset> {
            ["low", "high"]
                .into_iter()
                .enumerate()
                .map(|(i, id)| Preset {
                    id,
                    label: id,
                    description: "",
                    values: Settings::default().with("level", crate::Value::Int(i as i64)),
                })
                .collect()
        }
        fn default_preset(&self) -> &'static str {
            self.0
        }
        fn handles(&self, _: &FileCtx) -> bool {
            false
        }
        fn analyze_file(&self, _: &FileCtx, _: &[u8], _: &Settings) -> Result<FileAnalysis> {
            unreachable!()
        }
        fn optimize_file(&self, _: &FileCtx, _: &[u8], _: &Settings) -> Result<FileOutcome> {
            unreachable!()
        }
        fn resource_checks(&self, _: &[FileCtx]) -> Vec<Issue> {
            Vec::new()
        }
        fn preview(&self, _: &FileCtx, _: &[u8], _: &Settings) -> Result<Vec<PreviewItem>> {
            unreachable!()
        }
    }

    #[test]
    fn default_settings_come_from_the_default_preset() {
        assert_eq!(Presets("high").default_settings().int("level", -1), 1);
        assert_eq!(Presets("missing").default_settings(), Settings::default());
    }
}
