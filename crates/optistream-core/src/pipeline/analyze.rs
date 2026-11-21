//! Analysis pass: every handled file analysed in parallel.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::fs;

use anyhow::Result;
use optistream_i18n::{t, tf};
use rayon::prelude::*;
use serde::Serialize;

use crate::method::{FileAnalysis, FileCtx, Issue, Method};
use crate::pipeline::files::{guarded, is_selected, pool};
use crate::pipeline::FIVEM_WARN_BYTES;
use crate::settings::Settings;
use crate::{contexts, mib, Progress, Selection, SizeInfo};

#[derive(Clone, Debug, Serialize)]
pub struct AnalyzedFile {
    pub rel: PathBuf,
    pub size: SizeInfo,
    pub analysis: Result<FileAnalysis, String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub root: PathBuf,
    pub files: Vec<AnalyzedFile>,
    pub resource_issues: Vec<Issue>,
    /// Files the method does not process (manifests, metas, ymt, ...).
    pub other_files: usize,
}

/// Runs the method's preparation, then extends a selection to the files linked to it.
pub(crate) fn prepare(method: &dyn Method, ctxs: &[FileCtx], settings: &Settings, only: Selection) -> Result<Option<HashSet<PathBuf>>> {
    method.prepare(ctxs, settings)?;
    let Some(sel) = only else { return Ok(None) };
    let by_lower: std::collections::HashMap<String, &PathBuf> = ctxs.iter().map(|c| (c.rel_lower(), &c.rel)).collect();
    let mut out = sel.clone();
    for ctx in ctxs.iter().filter(|c| sel.contains(&c.rel)) {
        out.extend(method.linked_files(ctx).iter().filter_map(|l| by_lower.get(l)).map(|&p| p.clone()));
    }
    Ok(Some(out))
}

/// Analyses the resource, or only the selected files (resource-wide checks always cover everything).
pub fn analyze(
    method: &dyn Method,
    root: &Path,
    settings: &Settings,
    only: Selection,
    progress: &Progress,
) -> Result<Analysis> {
    let ctxs = contexts(root)?;
    let expanded = prepare(method, &ctxs, settings, only)?;
    let only = expanded.as_ref().or(only);
    let handled: Vec<&FileCtx> = ctxs.iter().filter(|c| method.handles(c)).collect();
    let targets: Vec<&FileCtx> = handled.iter().copied().filter(|c| is_selected(only, c)).collect();
    progress.total.store(targets.len(), Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    let files: Vec<AnalyzedFile> = pool()?.install(|| {
        targets
            .par_iter()
            .filter(|_| !progress.is_cancelled())
            .map(|ctx| {
                let path = root.join(&ctx.rel);
                let res = fs::read(&path).map_err(anyhow::Error::from).and_then(|data| {
                    let size = SizeInfo::of(&data);
                    let mut a = guarded(|| method.analyze_file(ctx, &data, settings))?;
                    // A file the settings leave untouched only gets informational size notes:
                    // it must not show up as something left to fix.
                    let skipped = if a.skipped { t("core.analysis.skipped") } else { "" };
                    if size.graphics > FIVEM_WARN_BYTES {
                        let text = tf!("core.analysis.physical_over", size = mib(size.graphics), skipped = skipped);
                        a.issues.insert(0, if a.skipped { Issue::info(text) } else { Issue::error(text) });
                    }
                    if size.system > FIVEM_WARN_BYTES {
                        let text = tf!("core.analysis.virtual_over", size = mib(size.system), skipped = skipped);
                        a.issues.insert(0, if a.skipped { Issue::info(text) } else { Issue::warning(text) });
                    }
                    Ok((size, a))
                });
                progress.step(&ctx.rel);
                match res {
                    Ok((size, a)) => AnalyzedFile { rel: ctx.rel.clone(), size, analysis: Ok(a) },
                    Err(e) => AnalyzedFile {
                        rel: ctx.rel.clone(),
                        size: SizeInfo::default(),
                        analysis: Err(format!("{e:#}")),
                    },
                }
            })
            .collect()
    });

    let owned: Vec<FileCtx> = handled.into_iter().cloned().collect();
    Ok(Analysis {
        root: root.to_path_buf(),
        resource_issues: method.resource_checks(&owned),
        other_files: ctxs.len() - owned.len(),
        files,
    })
}
