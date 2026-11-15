//! Optimisation report: per-file outcome, totals and text export.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::method::Issue;
use crate::{mib, SizeInfo};

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
    /// Folder of the resource holding the file (see [`FileCtx::resource_dir`](crate::FileCtx::resource_dir)).
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
                    // The virtual memory is only shown when it changed (geometry reduction).
                    let virt = if f.before.system != f.after.system {
                        format!(", virtuelle {} → {}", mib(f.before.system), mib(f.after.system))
                    } else {
                        String::new()
                    };
                    s += &format!(
                        "- {} : physique {} → {}{virt}, disque {} → {}\n",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::size::tests::{mib16_pages, rsc7_header};
    use crate::pipeline::FIVEM_WARN_BYTES;

    fn size(graphics_pages: u32, disk: u64) -> SizeInfo {
        SizeInfo { disk, ..SizeInfo::of(&rsc7_header(0, mib16_pages(graphics_pages))) }
    }

    fn file(rel: &str, resource: &str, status: FileStatus, before: SizeInfo, after: SizeInfo) -> FileReport {
        FileReport {
            rel: rel.into(),
            resource: resource.into(),
            status,
            before,
            after,
            changes: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn report(files: Vec<FileReport>) -> Report {
        Report {
            method: "Test".into(),
            source: "src".into(),
            destination: "dst".into(),
            in_place: false,
            files,
            copied_files: 2,
            resource_issues: Vec::new(),
            seconds: 1.0,
            cancelled: false,
            not_written: Vec::new(),
        }
    }

    #[test]
    fn totals_sum_every_file() {
        let r = report(vec![
            file("a", "", FileStatus::Optimized, size(2, 100), size(1, 60)),
            file("b", "", FileStatus::Unchanged, size(1, 10), size(1, 10)),
            file("c", "", FileStatus::Error("x".into()), size(3, 5), size(3, 5)),
        ]);
        let t = r.totals();
        assert_eq!((t.processed, t.optimized, t.errors), (3, 1, 1));
        assert_eq!((t.disk_before, t.disk_after), (115, 75));
        assert_eq!((t.graphics_before, t.graphics_after), (6 * FIVEM_WARN_BYTES, 5 * FIVEM_WARN_BYTES));
        assert_eq!((t.warnings_before, t.warnings_after), (2, 1));
    }

    #[test]
    fn by_resource_skips_quiet_resources_and_sorts_by_remaining_warnings() {
        let r = report(vec![
            file("q/a", "quiet", FileStatus::Unchanged, size(1, 1), size(1, 1)),
            file("b/a", "b", FileStatus::Optimized, size(2, 1), size(1, 1)),
            file("c/a", "c", FileStatus::Unchanged, size(2, 1), size(2, 1)),
            file("a/a", "a", FileStatus::Error("x".into()), size(1, 1), size(1, 1)),
        ]);
        let names: Vec<PathBuf> = r.by_resource().into_iter().map(|(p, _)| p).collect();
        // "c" still warns; "a" and "b" have no warning left, ordered by name.
        assert_eq!(names, [PathBuf::from("c"), PathBuf::from("a"), PathBuf::from("b")]);
    }

    #[test]
    fn text_summary() {
        let mut opt = file("stream/a.ydd", "", FileStatus::Optimized, size(2, 100), size(1, 60));
        opt.changes.push("textures réduites".into());
        opt.warnings.push("attention".into());
        let mut r = report(vec![
            opt,
            file("stream/quiet.ydd", "", FileStatus::Unchanged, size(1, 1), size(1, 1)),
            file("stream/bad.ydd", "", FileStatus::Error("corrompu".into()), size(1, 1), size(1, 1)),
        ]);
        r.resource_issues.push(Issue::warning("trop de drawables"));
        let text = r.to_text();
        assert!(text.starts_with("OptiStream — Test\n"), "{text}");
        assert!(text.contains("Fichiers traités : 3, optimisés : 1, erreurs : 1, autres fichiers copiés : 2"));
        assert!(text.contains("Fichiers > 16 Mio (avertissement FiveM) : 1 → 0"));
        assert!(text.contains("- stream/a.ydd : physique 32.0 Mio → 16.0 Mio, disque"));
        assert!(text.contains("    · textures réduites\n    ! attention\n"));
        assert!(text.contains("- stream/bad.ydd : ERREUR corrompu"));
        assert!(text.contains("[Warning] trop de drawables"));
        // Unchanged files without warnings are left out of the detail.
        assert!(!text.contains("quiet.ydd"));
        // Same virtual memory before and after: no virtual column.
        assert!(!text.contains("virtuelle 0.0"));
        // A single resource at the root: no per-resource section.
        assert!(!text.contains("Par ressource"));
        assert!(!text.contains("ATTENTION"));
    }

    #[test]
    fn text_flags_incomplete_exports_and_lists_resources() {
        let mut r = report(vec![file("res/a.ydd", "res", FileStatus::Optimized, size(2, 1), size(1, 1))]);
        r.cancelled = true;
        r.not_written.push("res/b.ydd".into());
        let text = r.to_text();
        assert!(text.contains("(annulé)"));
        assert!(text.contains("ATTENTION : 1 fichier(s) non écrit(s)"));
        assert!(text.contains("  res/b.ydd\n"));
        assert!(text.contains("Par ressource"));
        assert!(text.contains("  res : 1 → 0, 32.0 Mio → 16.0 Mio, 1 optimisé(s)\n"));
    }
}
