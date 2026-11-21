//! [`Method`] implementation for mappings: resource detection, role index per resource.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use optistream_i18n::{t, tf};
use optistream_core::{
    resource_prefix, FileAnalysis, FileCtx, FileOutcome, Issue, MANIFESTS, Method, ParamSpec, Preset, PreviewItem,
    Settings,
};
use optistream_rage::Resource;
use rayon::prelude::*;

use crate::config::{self, Config};
use crate::process::Kind;
use crate::roles::RoleIndex;
use crate::process;

type IndexCell = Arc<OnceLock<Arc<RoleIndex>>>;

/// Mapping method. Keeps the role index of each resource met during the current run.
#[derive(Default)]
pub struct MappingMethod {
    indexes: Mutex<HashMap<(PathBuf, String), IndexCell>>,
}

impl MappingMethod {
    pub fn new() -> Self {
        Self::default()
    }

    /// Role index of the resource holding `ctx`, built on first use from every readable
    /// drawable of that resource (concurrent callers wait for the same build).
    fn index_for(&self, ctx: &FileCtx) -> Arc<RoleIndex> {
        let prefix = resource_prefix(&ctx.rel_lower(), &ctx.all_files);
        let cell = self.indexes.lock().unwrap().entry((ctx.root.clone(), prefix.clone())).or_default().clone();
        cell.get_or_init(|| Arc::new(build_index(&ctx.root, &prefix, &ctx.all_files))).clone()
    }
}

fn is_manifest(name: &str) -> bool {
    MANIFESTS.contains(&name)
}

fn build_index(root: &Path, prefix: &str, all_files: &HashSet<String>) -> RoleIndex {
    let drawables: Vec<&String> = all_files
        .iter()
        .filter(|f| matches!(Kind::of(f), Some(Kind::Ydr | Kind::Ydd)))
        .filter(|f| resource_prefix(f, all_files) == prefix)
        .collect();
    let parts: Vec<RoleIndex> = drawables
        .par_iter()
        .filter_map(|rel| {
            let bytes = std::fs::read(root.join(rel.as_str())).ok()?;
            if !Resource::is_rsc7(&bytes) {
                return None;
            }
            let res = Resource::parse(&bytes).ok()?;
            let ds = process::read_drawables(&res, Kind::of(rel)?).ok()?;
            let mut idx = RoleIndex::default();
            idx.add_shaders(ds.iter().flat_map(|d| &d.shaders));
            Some(idx)
        })
        .collect();
    let mut out = RoleIndex::default();
    for p in &parts {
        out.merge(p);
    }
    out
}

/// Why the settings or the file itself leave it untouched, if they do.
fn skip_reason(cfg: &Config, data: &[u8]) -> Option<String> {
    if data.starts_with(b"FXAP") {
        return Some(t("mapping.skip.encrypted").to_owned());
    }
    if !Resource::is_rsc7(data) {
        return Some(t("mapping.skip.unknown_format").to_owned());
    }
    cfg.below_min_physical(data)
        .then(|| tf!("mapping.skip.below_min_physical", size = cfg.min_physical / (1024 * 1024)))
}

fn kind_of(ctx: &FileCtx) -> Option<Kind> {
    Kind::of(&ctx.file_name_lower())
}

impl Method for MappingMethod {
    fn id(&self) -> &'static str {
        "mapping"
    }

    fn label(&self) -> &'static str {
        t("mapping.method.label")
    }

    fn description(&self) -> &'static str {
        t("mapping.method.description")
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
            return Ok(FileAnalysis { kind: kind.label().into(), summary: tf!("mapping.skip.summary", reason = reason), skipped: true, ..Default::default() });
        }
        let p = process::parse(kind, data, &self.index_for(ctx))?;
        Ok(process::analyze(&p, &cfg))
    }

    fn optimize_file(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<FileOutcome> {
        let cfg = Config::from_settings(settings);
        let kind = kind_of(ctx).expect("handled file");
        if skip_reason(&cfg, data).is_some() {
            return Ok(FileOutcome::default());
        }
        let (bytes, changes, warnings) = process::optimize(kind, data, &cfg, &self.index_for(ctx))?;
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
            vec![Issue::info(tf!("mapping.resources_detected", count = resources.len()))]
        } else {
            Vec::new()
        }
    }

    fn preview(&self, ctx: &FileCtx, data: &[u8], settings: &Settings) -> Result<Vec<PreviewItem>> {
        let cfg = Config::from_settings(settings);
        let kind = kind_of(ctx).expect("handled file");
        if let Some(reason) = skip_reason(&cfg, data) {
            anyhow::bail!(tf!("mapping.skip.preview", reason = reason));
        }
        process::preview(kind, data, &cfg, &self.index_for(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_manifest_wins() {
        let files: HashSet<String> = [
            "[cat]/a/fxmanifest.lua",
            "[cat]/a/stream/x.ytd",
            "[cat]/a/stream/sub/y.ytd",
            "[cat]/b/__resource.lua",
            "[cat]/b/stream/z.ydr",
            "loose.ytd",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(resource_prefix("[cat]/a/stream/sub/y.ytd", &files), "[cat]/a");
        assert_eq!(resource_prefix("[cat]/b/stream/z.ydr", &files), "[cat]/b");
        assert_eq!(resource_prefix("loose.ytd", &files), "");
    }
}
