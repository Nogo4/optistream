//! [`Method`] implementation for vehicles: role index per folder, `+hi.ytd` lookup.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use optistream_core::{FileAnalysis, FileCtx, FileOutcome, Issue, MANIFESTS, Method, ParamSpec, Preset, PreviewItem, Settings};
use optistream_i18n::{t, tf};
use optistream_rage::Resource;
use rayon::prelude::*;

use crate::config::{self, Config};
use crate::process::{self, FileInfo, Kind};
use crate::roles::{self, VehicleIndex};

type IndexCell = Arc<OnceLock<Arc<VehicleIndex>>>;

/// Vehicle method. Keeps the role index of each folder met during the current run.
///
/// A vehicle's textures (`name.ytd`, `name+hi.ytd`) sit next to its models (`name.yft`,
/// `name_hi.yft`): the index is built from the readable models of the same folder, which
/// stays fast on packs of thousands of vehicles.
#[derive(Default)]
pub struct VehicleMethod {
    indexes: Mutex<HashMap<(PathBuf, String), IndexCell>>,
}

impl VehicleMethod {
    pub fn new() -> Self {
        Self::default()
    }

    fn index_for(&self, ctx: &FileCtx) -> Arc<VehicleIndex> {
        let dir = parent_dir(&ctx.rel_lower());
        let cell = self.indexes.lock().unwrap().entry((ctx.root.clone(), dir.clone())).or_default().clone();
        cell.get_or_init(|| Arc::new(build_index(&ctx.root, &dir, &ctx.all_files))).clone()
    }
}

fn parent_dir(rel_lower: &str) -> String {
    rel_lower.rsplit_once('/').map(|(d, _)| d.to_owned()).unwrap_or_default()
}

fn build_index(root: &Path, dir: &str, all_files: &HashSet<String>) -> VehicleIndex {
    let models: Vec<&String> = all_files
        .iter()
        .filter(|f| matches!(Kind::of(f), Some(Kind::Yft | Kind::Ydr | Kind::Ydd)))
        .filter(|f| parent_dir(f) == dir)
        .collect();
    let parts: Vec<VehicleIndex> = models
        .par_iter()
        .filter_map(|rel| {
            let bytes = std::fs::read(root.join(rel.as_str())).ok()?;
            if !Resource::is_rsc7(&bytes) {
                return None;
            }
            let res = Resource::parse(&bytes).ok()?;
            let ds = process::read_drawables(&res, Kind::of(rel)?).ok()?;
            let mut idx = VehicleIndex::default();
            idx.add_shaders(ds.iter().flat_map(|d| &d.shaders));
            Some(idx)
        })
        .collect();
    let mut out = VehicleIndex::default();
    for p in &parts {
        out.merge(p);
    }
    out
}

/// `+hi.ytd` texture names for a base `.ytd`, livery flag for a `.yft`.
fn file_info(ctx: &FileCtx) -> FileInfo {
    let name = ctx.file_name_lower();
    let mut info = FileInfo { livery_file: name.ends_with(".yft") && roles::livery_file(&name), ..Default::default() };
    if let Some(stem) = name.strip_suffix(".ytd").filter(|s| !s.ends_with("+hi")) {
        let rel = ctx.rel_lower();
        let hi = format!("{}{stem}+hi.ytd", rel.strip_suffix(&name).unwrap_or(""));
        if let Some(actual) = ctx.all_files.iter().find(|f| **f == hi) {
            if let Ok(bytes) = std::fs::read(ctx.root.join(actual.as_str())) {
                if Resource::is_rsc7(&bytes) {
                    info.hi_names = process::ytd_names(&bytes).unwrap_or_default();
                }
            }
        }
    }
    info
}

fn is_manifest(name: &str) -> bool {
    MANIFESTS.contains(&name)
}

/// Why the settings or the file itself leave it untouched, if they do.
fn skip_reason(cfg: &Config, data: &[u8]) -> Option<String> {
    if data.starts_with(b"FXAP") {
        return Some(t("vehicle.skip.encrypted").to_owned());
    }
    if !Resource::is_rsc7(data) {
        return Some(t("vehicle.skip.unknown_format").to_owned());
    }
    cfg.below_min_physical(data)
        .then(|| tf!("vehicle.skip.below_min_physical", mib = cfg.min_physical / (1024 * 1024)))
}

fn kind_of(ctx: &FileCtx) -> Option<Kind> {
    Kind::of(&ctx.file_name_lower())
}

impl Method for VehicleMethod {
    fn id(&self) -> &'static str {
        "vehicle"
    }

    fn label(&self) -> &'static str {
        t("vehicle.method.label")
    }

    fn description(&self) -> &'static str {
        t("vehicle.method.description")
    }

    fn params(&self) -> Vec<ParamSpec> {
        config::params()
    }

    fn presets(&self) -> Vec<Preset> {
        config::presets()
    }

    fn default_preset(&self) -> &'static str {
        "equilibre"
    }

    fn handles(&self, ctx: &FileCtx) -> bool {
        kind_of(ctx).is_some()
    }

    fn analyze_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileAnalysis> {
        let cfg = Config::from_settings(settings);
        let kind = kind_of(ctx).expect("handled file");
        if let Some(reason) = skip_reason(&cfg, data) {
            return Ok(FileAnalysis { kind: kind.label().into(), summary: tf!("vehicle.skip.summary", reason = reason), skipped: true, ..Default::default() });
        }
        let p = process::parse(kind, data, &self.index_for(ctx), &file_info(ctx))?;
        Ok(process::analyze(&p, &cfg))
    }

    fn optimize_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileOutcome> {
        let cfg = Config::from_settings(settings);
        let kind = kind_of(ctx).expect("handled file");
        if skip_reason(&cfg, data).is_some() {
            return Ok(FileOutcome::default());
        }
        let (bytes, changes, warnings) = process::optimize(kind, data, &cfg, &self.index_for(ctx), &file_info(ctx))?;
        Ok(FileOutcome { bytes, changes, warnings })
    }

    fn prepare(&self, _files: &[FileCtx], _settings: &Settings) -> Result<()> {
        // Indexes are rebuilt from the files as they are at the start of each run.
        self.indexes.lock().unwrap().clear();
        Ok(())
    }

    fn resource_checks(&self, files: &[FileCtx]) -> Vec<Issue> {
        let Some(first) = files.first() else { return Vec::new() };
        let resources: BTreeSet<String> = first
            .all_files
            .iter()
            .filter(|f| is_manifest(f.rsplit('/').next().unwrap_or(f)))
            .map(|f| f.rsplit_once('/').map(|(d, _)| d.to_owned()).unwrap_or_default())
            .collect();
        if resources.len() > 1 {
            vec![Issue::info(tf!("vehicle.resources.detected", count = resources.len()))]
        } else {
            Vec::new()
        }
    }

    fn preview(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<Vec<PreviewItem>> {
        let cfg = Config::from_settings(settings);
        let kind = kind_of(ctx).expect("handled file");
        if let Some(reason) = skip_reason(&cfg, data) {
            anyhow::bail!(tf!("vehicle.skip.preview", reason = reason));
        }
        process::preview(kind, data, &cfg, &self.index_for(ctx), &file_info(ctx))
    }
}
