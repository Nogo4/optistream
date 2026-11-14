//! File listing, selection, thread pool and panic guard shared by both passes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Result};

use crate::method::{normalize_rel, FileCtx};

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

pub(super) fn pool() -> Result<rayon::ThreadPool> {
    // Every logical core; the cap only bounds memory on very large CPUs (one file each).
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(32);
    Ok(rayon::ThreadPoolBuilder::new().num_threads(threads).build()?)
}

/// Runs one file's work, turning a panic into an error so a single bad file can never
/// abort the whole run (and leave an export half-written).
pub(super) fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
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

pub(super) fn is_selected(only: Selection, ctx: &FileCtx) -> bool {
    only.is_none_or(|s| s.contains(&ctx.rel))
}
