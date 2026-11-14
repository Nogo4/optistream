//! Optimisation pass: linked files processed together, written atomically to the output.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;
use std::fs;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::method::{FileCtx, Method};
use crate::pipeline::analyze::prepare;
use crate::pipeline::files::{guarded, is_selected, pool};
use crate::settings::Settings;
use crate::{contexts, FileReport, FileStatus, Output, Progress, Report, Selection, SizeInfo};

fn export_root(root: &Path, dir: &Path) -> Result<PathBuf> {
    let name = root.file_name().context("source folder has no name")?;
    let dest = dir.join(name);
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (src, dst_parent) = (canon(root), canon(dir));
    if dst_parent.join(name) == src {
        bail!("le dossier d'export ne peut pas être le dossier source lui-même");
    }
    if dst_parent.starts_with(&src) {
        bail!("le dossier d'export ne peut pas se trouver à l'intérieur de la ressource source");
    }
    Ok(dest)
}

/// Groups the files the method links together (a drawable and its texture variations), in
/// the order of their first member.
fn linked_units<'a>(method: &dyn Method, targets: &[&'a FileCtx]) -> Vec<Vec<&'a FileCtx>> {
    let index: std::collections::HashMap<String, usize> =
        targets.iter().enumerate().map(|(i, c)| (c.rel_lower(), i)).collect();
    let mut taken = vec![false; targets.len()];
    let mut units = Vec::new();
    for start in 0..targets.len() {
        if taken[start] {
            continue;
        }
        taken[start] = true;
        let mut unit = Vec::new();
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            unit.push(targets[i]);
            for linked in method.linked_files(targets[i]) {
                if let Some(&j) = index.get(&linked).filter(|&&j| !taken[j]) {
                    taken[j] = true;
                    stack.push(j);
                }
            }
        }
        units.push(unit);
    }
    units
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    // Appended to the whole file name: files that only differ by their extension are
    // written in parallel and must not share a temporary file.
    let mut name = path.file_name().context("file without a name")?.to_os_string();
    name.push(".optistream.tmp");
    let tmp = path.with_file_name(name);
    fs::write(&tmp, bytes).with_context(|| format!("écriture de {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("remplacement de {}", path.display()))?;
    Ok(())
}

/// Optimises the resource, or only the selected files.
///
/// With a selection: in place, only those files are replaced; on export, a missing
/// destination is created as a full copy (only the selection optimised), while an existing
/// destination only gets the selected files updated.
pub fn optimize(
    method: &dyn Method,
    root: &Path,
    settings: &Settings,
    output: &Output,
    only: Selection,
    progress: &Progress,
) -> Result<Report> {
    let start = Instant::now();
    let ctxs = contexts(root)?;
    let expanded = prepare(method, &ctxs, settings, only)?;
    let only = expanded.as_ref().or(only);
    let (dest, in_place, update_only) = match output {
        Output::InPlace => (root.to_path_buf(), true, false),
        Output::Export { dir, overwrite } => {
            let dest = export_root(root, dir)?;
            let exists = dest.exists() && fs::read_dir(&dest)?.next().is_some();
            if exists && only.is_none() && !overwrite {
                bail!("le dossier {} existe déjà et n'est pas vide", dest.display());
            }
            (dest, false, exists && only.is_some())
        }
    };
    // Files that are neither processed nor copied in this run.
    let skipped = |ctx: &FileCtx| !is_selected(only, ctx) && (in_place || update_only);
    let work: Vec<&FileCtx> = ctxs.iter().filter(|c| !skipped(c)).collect();

    let process = |ctx: &FileCtx| method.handles(ctx) && is_selected(only, ctx);
    // In place nothing is copied: only the processed files count.
    let steps = work.iter().filter(|c| !in_place || process(c)).count();
    progress.total.store(steps, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let mut not_written = Vec::new();

    // Copies first (ymt, meta, manifest, untouched files): a heavy run interrupted later
    // must never leave an export without them.
    let mut copied = 0usize;
    if !in_place {
        for ctx in work.iter().filter(|c| !process(c)) {
            if progress.is_cancelled() {
                not_written.push(ctx.rel.clone());
                continue;
            }
            let dst = dest.join(&ctx.rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(root.join(&ctx.rel), &dst)
                .with_context(|| format!("copie de {}", ctx.rel.display()))?;
            copied += 1;
            progress.step(&ctx.rel);
        }
    }

    let mut targets: Vec<&FileCtx> = work.iter().copied().filter(|c| process(c)).collect();
    // Biggest files first: a large file started last would otherwise run alone at the end.
    targets.sort_by_cached_key(|c| std::cmp::Reverse(fs::metadata(root.join(&c.rel)).map(|m| m.len()).unwrap_or(0)));
    // Linked files are rewritten consistently with each other (shared UV crop): a cancelled
    // run processes all of a unit or none of it, never half.
    let units = linked_units(method, &targets);
    let run_file = |ctx: &FileCtx| -> FileReport {
                let src = root.join(&ctx.rel);
                let dst = dest.join(&ctx.rel);
                let report = (|| -> Result<FileReport> {
                    if !in_place {
                        if let Some(parent) = dst.parent() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                    let data = fs::read(&src)?;
                    let before = SizeInfo::of(&data);
                    let outcome = match guarded(|| method.optimize_file(ctx, &data, settings)) {
                        Ok(o) => o,
                        Err(e) => {
                            // The original is still written on export, so the resource stays complete.
                            if !in_place {
                                fs::write(&dst, &data)?;
                            }
                            return Ok(FileReport {
                                rel: ctx.rel.clone(),
                                resource: ctx.resource_dir(),
                                status: FileStatus::Error(format!("{e:#}")),
                                before,
                                after: before,
                                changes: Vec::new(),
                                warnings: Vec::new(),
                            });
                        }
                    };
                    let (status, after) = match &outcome.bytes {
                        Some(bytes) => {
                            if in_place {
                                write_atomic(&dst, bytes)?;
                            } else {
                                fs::write(&dst, bytes)?;
                            }
                            (FileStatus::Optimized, SizeInfo::of(bytes))
                        }
                        None => {
                            if !in_place {
                                fs::write(&dst, &data)?;
                            }
                            (FileStatus::Unchanged, before)
                        }
                    };
                    Ok(FileReport {
                        rel: ctx.rel.clone(),
                        resource: ctx.resource_dir(),
                        status,
                        before,
                        after,
                        changes: outcome.changes,
                        warnings: outcome.warnings,
                    })
                })();
                progress.step(&ctx.rel);
                report.unwrap_or_else(|e| FileReport {
                    rel: ctx.rel.clone(),
                    resource: ctx.resource_dir(),
                    status: FileStatus::Error(format!("{e:#}")),
                    before: SizeInfo::default(),
                    after: SizeInfo::default(),
                    changes: Vec::new(),
                    warnings: Vec::new(),
                })
    };
    let mut results: Vec<(PathBuf, Option<FileReport>)> = pool()?.install(|| {
        units
            .par_iter()
            .with_max_len(1)
            .flat_map_iter(|unit| -> Vec<(PathBuf, Option<FileReport>)> {
                if progress.is_cancelled() {
                    return unit.iter().map(|ctx| (ctx.rel.clone(), None)).collect();
                }
                unit.par_iter().map(|ctx| (ctx.rel.clone(), Some(run_file(ctx)))).collect()
            })
            .collect()
    });
    // Back to the resource order for the report.
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let mut files = Vec::with_capacity(results.len());
    for (rel, report) in results {
        match report {
            Some(r) => files.push(r),
            // On export the file is missing from the destination; in place it is simply untouched.
            None if !in_place => not_written.push(rel),
            None => {}
        }
    }
    not_written.sort();

    let handled: Vec<FileCtx> = ctxs.iter().filter(|c| method.handles(c)).cloned().collect();
    Ok(Report {
        method: method.label().to_owned(),
        source: root.to_path_buf(),
        destination: dest,
        in_place,
        files,
        copied_files: copied,
        resource_issues: method.resource_checks(&handled),
        seconds: start.elapsed().as_secs_f64(),
        cancelled: progress.is_cancelled(),
        not_written,
    })
}
