//! Analysis and optimisation of a whole resource folder with any [`Method`].

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use crate::method::{normalize_rel, FileAnalysis, FileCtx, Issue, Method};
use crate::settings::Settings;

/// FiveM starts warning about streamed assets above this declared size.
pub const FIVEM_WARN_BYTES: u64 = 16 * 1024 * 1024;

/// Shared progress state, polled by the UI.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.done.load(Ordering::Relaxed) as f32 / total as f32
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn step(&self, label: &Path) {
        self.done.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut c) = self.current.lock() {
            *c = label.display().to_string();
        }
    }
}

/// Declared memory of a file, read from its RSC7 header (no decompression).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct SizeInfo {
    pub disk: u64,
    pub system: u64,
    pub graphics: u64,
}

impl SizeInfo {
    pub fn of(bytes: &[u8]) -> Self {
        let mut s = SizeInfo { disk: bytes.len() as u64, ..Default::default() };
        if optistream_rage::Resource::is_rsc7(bytes) {
            let flag = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
            s.system = optistream_rage::PageFlags(flag(8)).size();
            s.graphics = optistream_rage::PageFlags(flag(12)).size();
        }
        s
    }

    /// Whether FiveM prints an "oversized asset" style warning for this file.
    pub fn fivem_warning(&self) -> bool {
        self.graphics > FIVEM_WARN_BYTES || self.system > FIVEM_WARN_BYTES
    }
}

pub fn mib(bytes: u64) -> String {
    format!("{:.1} Mio", bytes as f64 / (1024.0 * 1024.0))
}

fn list_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            out.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

/// Every file of the resource with its context (relative path, sibling list).
pub fn contexts(root: &Path) -> Result<Vec<FileCtx>> {
    let files = list_files(root)?;
    let all: Arc<HashSet<String>> = Arc::new(files.iter().map(|p| normalize_rel(p)).collect());
    Ok(files
        .into_iter()
        .map(|rel| FileCtx { root: root.to_path_buf(), rel, all_files: all.clone() })
        .collect())
}

fn pool() -> Result<rayon::ThreadPool> {
    // Every logical core; the cap only bounds memory on very large CPUs (one file each).
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(32);
    Ok(rayon::ThreadPoolBuilder::new().num_threads(threads).build()?)
}

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

/// Runs one file's work, turning a panic into an error so a single bad file can never
/// abort the whole run (and leave an export half-written).
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "cause inconnue".to_owned());
            bail!("plantage interne : {msg}")
        }
    }
}

/// Optional subset of files (paths relative to the resource root) to work on.
pub type Selection<'a> = Option<&'a HashSet<PathBuf>>;

fn is_selected(only: Selection, ctx: &FileCtx) -> bool {
    only.is_none_or(|s| s.contains(&ctx.rel))
}

/// Runs the method's preparation, then extends a selection to the files linked to it.
fn prepare(method: &dyn Method, ctxs: &[FileCtx], settings: &Settings, only: Selection) -> Result<Option<HashSet<PathBuf>>> {
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
                    let skipped = a.skipped.then_some(" — ignoré avec les réglages actuels").unwrap_or("");
                    if size.graphics > FIVEM_WARN_BYTES {
                        let text = format!("mémoire physique déclarée {} (FiveM avertit au-delà de 16 Mio){skipped}", mib(size.graphics));
                        a.issues.insert(0, if a.skipped { Issue::info(text) } else { Issue::error(text) });
                    }
                    if size.system > FIVEM_WARN_BYTES {
                        let text = format!("mémoire virtuelle déclarée {} (> 16 Mio){skipped}", mib(size.system));
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

#[derive(Clone, Debug)]
pub enum Output {
    /// Overwrite the source files (no backup).
    InPlace,
    /// Write a full copy of the resource into `dir/<resource name>`.
    Export { dir: PathBuf, overwrite: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FileStatus {
    Optimized,
    Unchanged,
    Error(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct FileReport {
    pub rel: PathBuf,
    /// Folder of the resource holding the file (see [`FileCtx::resource_dir`]).
    pub resource: PathBuf,
    pub status: FileStatus,
    pub before: SizeInfo,
    pub after: SizeInfo,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub method: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub in_place: bool,
    pub files: Vec<FileReport>,
    pub copied_files: usize,
    pub resource_issues: Vec<Issue>,
    pub seconds: f64,
    pub cancelled: bool,
    /// Files of this run that were never written (cancelled run): an export is then incomplete.
    pub not_written: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Totals {
    pub processed: usize,
    pub optimized: usize,
    pub errors: usize,
    pub disk_before: u64,
    pub disk_after: u64,
    pub graphics_before: u64,
    pub graphics_after: u64,
    pub system_before: u64,
    pub system_after: u64,
    pub warnings_before: usize,
    pub warnings_after: usize,
}

fn add_file(t: &mut Totals, f: &FileReport) {
    t.processed += 1;
    match f.status {
        FileStatus::Optimized => t.optimized += 1,
        FileStatus::Error(_) => t.errors += 1,
        FileStatus::Unchanged => {}
    }
    t.disk_before += f.before.disk;
    t.disk_after += f.after.disk;
    t.graphics_before += f.before.graphics;
    t.graphics_after += f.after.graphics;
    t.system_before += f.before.system;
    t.system_after += f.after.system;
    t.warnings_before += f.before.fivem_warning() as usize;
    t.warnings_after += f.after.fivem_warning() as usize;
}

impl Report {
    pub fn totals(&self) -> Totals {
        let mut t = Totals::default();
        for f in &self.files {
            add_file(&mut t, f);
        }
        t
    }

    /// Totals per resource (a category folder holds many), heaviest remaining warnings
    /// first. Only the resources with at least one optimised file or warning are listed.
    pub fn by_resource(&self) -> Vec<(PathBuf, Totals)> {
        let mut map: std::collections::BTreeMap<&Path, Totals> = std::collections::BTreeMap::new();
        for f in &self.files {
            add_file(map.entry(f.resource.as_path()).or_default(), f);
        }
        let mut out: Vec<(PathBuf, Totals)> = map
            .into_iter()
            .filter(|(_, t)| t.optimized > 0 || t.warnings_before > 0 || t.errors > 0)
            .map(|(p, t)| (p.to_path_buf(), t))
            .collect();
        out.sort_by(|a, b| b.1.warnings_after.cmp(&a.1.warnings_after).then(a.0.cmp(&b.0)));
        out
    }

    pub fn to_text(&self) -> String {
        let t = self.totals();
        let mut s = String::new();
        s += &format!("OptiStream — {}\n", self.method);
        s += &format!("Source      : {}\n", self.source.display());
        s += &format!(
            "Destination : {}{}\n",
            self.destination.display(),
            if self.in_place { " (modification sur place)" } else { "" }
        );
        s += &format!("Durée       : {:.0} s{}\n\n", self.seconds, if self.cancelled { " (annulé)" } else { "" });
        s += &format!(
            "Fichiers traités : {}, optimisés : {}, erreurs : {}, autres fichiers copiés : {}\n",
            t.processed, t.optimized, t.errors, self.copied_files
        );
        s += &format!("Disque           : {} → {}\n", mib(t.disk_before), mib(t.disk_after));
        s += &format!("Mémoire physique : {} → {}\n", mib(t.graphics_before), mib(t.graphics_after));
        s += &format!("Mémoire virtuelle: {} → {}\n", mib(t.system_before), mib(t.system_after));
        s += &format!("Fichiers > 16 Mio (avertissement FiveM) : {} → {}\n", t.warnings_before, t.warnings_after);
        if !self.not_written.is_empty() {
            s += &format!(
                "\nATTENTION : {} fichier(s) non écrit(s), la destination est incomplète :\n",
                self.not_written.len()
            );
            for p in &self.not_written {
                s += &format!("  {}\n", p.display());
            }
        }
        let resources = self.by_resource();
        if resources.len() > 1 || resources.first().is_some_and(|(p, _)| !p.as_os_str().is_empty()) {
            s += "\nPar ressource (> 16 Mio avant → après, mémoire physique) :\n";
            for (dir, r) in &resources {
                let name = if dir.as_os_str().is_empty() { "(racine)".to_owned() } else { dir.display().to_string() };
                s += &format!(
                    "  {name} : {} → {}, {} → {}, {} optimisé(s){}\n",
                    r.warnings_before,
                    r.warnings_after,
                    mib(r.graphics_before),
                    mib(r.graphics_after),
                    r.optimized,
                    if r.errors > 0 { format!(", {} erreur(s)", r.errors) } else { String::new() }
                );
            }
        }
        if !self.resource_issues.is_empty() {
            s += "\nContrôles de la ressource :\n";
            for i in &self.resource_issues {
                s += &format!("  [{:?}] {}\n", i.severity, i.text);
            }
        }
        s += "\nDétail :\n";
        for f in &self.files {
            match &f.status {
                FileStatus::Unchanged if f.warnings.is_empty() => continue,
                FileStatus::Error(e) => s += &format!("- {} : ERREUR {}\n", f.rel.display(), e),
                _ => {
                    s += &format!(
                        "- {} : physique {} → {}, disque {} → {}\n",
                        f.rel.display(),
                        mib(f.before.graphics),
                        mib(f.after.graphics),
                        mib(f.before.disk),
                        mib(f.after.disk)
                    );
                    for c in &f.changes {
                        s += &format!("    · {c}\n");
                    }
                    for w in &f.warnings {
                        s += &format!("    ! {w}\n");
                    }
                }
            }
        }
        s
    }
}

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
