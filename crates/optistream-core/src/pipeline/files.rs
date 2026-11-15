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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_passes_results_through() {
        assert_eq!(guarded(|| Ok(5)).unwrap(), 5);
        assert!(guarded(|| -> Result<()> { bail!("boom") }).is_err());
    }

    #[test]
    fn guarded_turns_panics_into_errors() {
        let msg = |e: anyhow::Error| e.to_string();
        assert_eq!(msg(guarded(|| -> Result<()> { panic!("static") }).unwrap_err()), "plantage interne : static");
        let n = 3;
        assert_eq!(msg(guarded(|| -> Result<()> { panic!("value {n}") }).unwrap_err()), "plantage interne : value 3");
        let e = guarded(|| -> Result<()> { std::panic::panic_any(42u8) }).unwrap_err();
        assert_eq!(msg(e), "plantage interne : cause inconnue");
    }

    #[test]
    fn contexts_list_every_file_sorted_with_shared_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("stream")).unwrap();
        std::fs::write(dir.path().join("fxmanifest.lua"), "").unwrap();
        std::fs::write(dir.path().join("stream").join("b.ydd"), "").unwrap();
        std::fs::write(dir.path().join("stream").join("a.ytd"), "").unwrap();
        let ctxs = contexts(dir.path()).unwrap();
        let rels: Vec<String> = ctxs.iter().map(|c| c.rel_lower()).collect();
        assert_eq!(rels, ["fxmanifest.lua", "stream/a.ytd", "stream/b.ydd"]);
        assert!(ctxs.iter().all(|c| c.root == dir.path() && c.all_files.len() == 3));
        assert!(ctxs[2].all_files.contains("stream/b.ydd"));
    }

    #[test]
    fn selection() {
        let ctx = FileCtx { root: PathBuf::new(), rel: PathBuf::from("a.ydd"), all_files: Default::default() };
        assert!(is_selected(None, &ctx));
        let sel: HashSet<PathBuf> = [PathBuf::from("a.ydd")].into();
        assert!(is_selected(Some(&sel), &ctx));
        let other: HashSet<PathBuf> = [PathBuf::from("b.ydd")].into();
        assert!(!is_selected(Some(&other), &ctx));
    }
}
