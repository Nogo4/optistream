//! What is still wrong with each file after optimisation, and report merging.

use std::collections::HashSet;
use std::path::PathBuf;

use optistream_core::{AnalyzedFile, FileReport, FileStatus, Issue, Report, Severity};

/// What is still wrong with a file after its optimisation.
pub(super) struct Remaining {
    /// Warnings and errors found by re-analysing the written file.
    pub(super) issues: Vec<Issue>,
    /// Warnings reported by the optimiser itself (tolerance reached, textures skipped...).
    pub(super) notes: Vec<String>,
    pub(super) worst: Severity,
}

pub(super) fn remaining_entry(f: &FileReport, checked: Option<&AnalyzedFile>) -> Option<Remaining> {
    let mut issues: Vec<Issue> = Vec::new();
    if let FileStatus::Error(e) = &f.status {
        issues.push(Issue::error(format!("échec de l'optimisation : {e}")));
    }
    match checked.map(|c| &c.analysis) {
        Some(Ok(a)) => issues.extend(a.issues.iter().filter(|i| i.severity >= Severity::Warning).cloned()),
        Some(Err(e)) => issues.push(Issue::error(format!("fichier produit illisible : {e}"))),
        None => {}
    }
    let notes = f.warnings.clone();
    if issues.is_empty() && notes.is_empty() {
        return None;
    }
    let worst = issues.iter().map(|i| i.severity).max().unwrap_or(Severity::Warning);
    Some(Remaining { issues, notes, worst })
}

/// Folds a partial run into the previous report, keeping each file's original "before".
pub(super) fn merge_report(old: &mut Report, new: Report, selection: &HashSet<PathBuf>) {
    // Files of this run are either written now or listed again in the new run.
    old.not_written.retain(|p| !selection.contains(p));
    old.not_written.extend(new.not_written);
    old.not_written.sort();
    old.copied_files += new.copied_files;
    for mut f in new.files {
        match old.files.iter_mut().find(|x| x.rel == f.rel) {
            Some(slot) => {
                f.before = slot.before;
                *slot = f;
            }
            None => old.files.push(f),
        }
    }
    old.seconds += new.seconds;
    old.cancelled = new.cancelled;
    old.resource_issues = new.resource_issues;
}
