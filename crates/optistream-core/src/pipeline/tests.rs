//! End-to-end tests of both passes with a fake method on a temporary resource folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{bail, Result};

use super::size::tests::{mib16_pages, rsc7_header};
use crate::method::{FileAnalysis, FileCtx, FileOutcome, Issue, Method, PreviewItem};
use crate::settings::{ParamSpec, Preset, Settings};
use crate::{analyze, optimize, FileStatus, Output, Progress, Severity};

/// Handles `.ydd` files. Content drives the outcome: `fail` errors, `panic` panics,
/// `keep` is left unchanged, anything else is uppercased.
#[derive(Default)]
struct Fake {
    /// Lowercased relative path -> files linked to it.
    links: HashMap<String, Vec<String>>,
    prepared: AtomicUsize,
}

impl Fake {
    fn linked(pairs: &[(&str, &str)]) -> Self {
        let mut links: HashMap<String, Vec<String>> = HashMap::new();
        for (a, b) in pairs {
            links.entry(a.to_string()).or_default().push(b.to_string());
        }
        Fake { links, ..Default::default() }
    }
}

impl Method for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn label(&self) -> &'static str {
        "Fake"
    }
    fn description(&self) -> &'static str {
        ""
    }
    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }
    fn presets(&self) -> Vec<Preset> {
        Vec::new()
    }
    fn default_preset(&self) -> &'static str {
        ""
    }

    fn handles(&self, ctx: &FileCtx) -> bool {
        ctx.file_name_lower().ends_with(".ydd")
    }

    fn analyze_file(&self, ctx: &FileCtx, data: &[u8], _: &Settings) -> Result<FileAnalysis> {
        if data == b"panic" {
            panic!("analysis failed");
        }
        if data == b"fail" {
            bail!("illisible");
        }
        Ok(FileAnalysis {
            kind: "ydd".into(),
            skipped: ctx.file_name_lower().starts_with("skip"),
            ..Default::default()
        })
    }

    fn optimize_file(&self, _: &FileCtx, data: &[u8], _: &Settings) -> Result<FileOutcome> {
        match data {
            b"panic" => panic!("optimisation failed"),
            b"fail" => bail!("broken"),
            b"keep" => Ok(FileOutcome { warnings: vec!["nothing to do".into()], ..Default::default() }),
            _ => Ok(FileOutcome {
                bytes: Some(data.to_ascii_uppercase()),
                changes: vec!["upper".into()],
                warnings: Vec::new(),
            }),
        }
    }

    fn resource_checks(&self, files: &[FileCtx]) -> Vec<Issue> {
        vec![Issue::info(format!("{} files", files.len()))]
    }

    fn prepare(&self, _: &[FileCtx], _: &Settings) -> Result<()> {
        self.prepared.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn linked_files(&self, ctx: &FileCtx) -> Vec<String> {
        self.links.get(&ctx.rel_lower()).cloned().unwrap_or_default()
    }

    fn preview(&self, _: &FileCtx, _: &[u8], _: &Settings) -> Result<Vec<PreviewItem>> {
        Ok(Vec::new())
    }
}

/// Temporary resource folder named `myres` inside a fresh temporary directory.
struct Fixture {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(files: &[(&str, &[u8])]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("myres");
        for (rel, data) in files {
            write(&root, rel, data);
        }
        Fixture { tmp, root }
    }

    fn out(&self) -> PathBuf {
        self.tmp.path().join("out")
    }
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn read(root: &Path, rel: &str) -> Vec<u8> {
    fs::read(root.join(rel)).unwrap()
}

fn sel(paths: &[&str]) -> HashSet<PathBuf> {
    paths.iter().map(|p| Path::new(p).components().collect()).collect()
}

fn export(dir: PathBuf) -> Output {
    Output::Export { dir, overwrite: false }
}

fn status_of(report: &crate::Report, rel: &str) -> FileStatus {
    let rel: PathBuf = Path::new(rel).components().collect();
    report.files.iter().find(|f| f.rel == rel).unwrap_or_else(|| panic!("{} missing", rel.display())).status.clone()
}

#[test]
fn analysis_covers_handled_files_only() {
    let fx = Fixture::new(&[("fxmanifest.lua", b""), ("stream/a.ydd", b"a"), ("stream/b.ydd", b"fail"), ("stream/c.ytd", b"")]);
    let method = Fake::default();
    let progress = Progress::default();
    let a = analyze(&method, &fx.root, &Settings::default(), None, &progress).unwrap();
    assert_eq!(a.files.len(), 2);
    assert_eq!(a.other_files, 2);
    assert_eq!(a.files[0].analysis.as_ref().unwrap().kind, "ydd");
    assert_eq!(a.files[1].analysis.as_ref().unwrap_err(), "illisible");
    assert_eq!(a.resource_issues[0].text, "2 files");
    assert_eq!(method.prepared.load(Ordering::Relaxed), 1);
    assert_eq!((progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)), (2, 2));
}

#[test]
fn analysis_survives_a_panicking_file() {
    let fx = Fixture::new(&[("a.ydd", b"panic"), ("b.ydd", b"b")]);
    let a = analyze(&Fake::default(), &fx.root, &Settings::default(), None, &Progress::default()).unwrap();
    assert!(a.files[0].analysis.as_ref().unwrap_err().contains("internal crash: analysis failed"));
    assert!(a.files[1].analysis.is_ok());
}

#[test]
fn oversized_files_get_size_issues() {
    let big = rsc7_header(mib16_pages(2), mib16_pages(2));
    let fx = Fixture::new(&[("big.ydd", &big), ("skip_big.ydd", &big), ("small.ydd", &rsc7_header(0, mib16_pages(1)))]);
    let a = analyze(&Fake::default(), &fx.root, &Settings::default(), None, &Progress::default()).unwrap();
    let issues = |i: usize| a.files[i].analysis.as_ref().unwrap().issues.clone();

    let big = issues(0);
    assert_eq!(big.iter().map(|i| i.severity).collect::<Vec<_>>(), [Severity::Warning, Severity::Error]);
    assert!(big[1].text.contains("declared physical memory 32.0 MiB"));
    // Files the settings leave untouched only get informational notes.
    let skipped = issues(1);
    assert!(skipped.iter().all(|i| i.severity == Severity::Info && i.text.contains("ignored")));
    assert!(issues(2).is_empty());
}

#[test]
fn analysis_selection_is_extended_to_linked_files() {
    let fx = Fixture::new(&[("stream/a.ydd", b"a"), ("stream/a_tex.ydd", b"t"), ("stream/b.ydd", b"b")]);
    let method = Fake::linked(&[("stream/a.ydd", "stream/a_tex.ydd")]);
    let only = sel(&["stream/a.ydd"]);
    let a = analyze(&method, &fx.root, &Settings::default(), Some(&only), &Progress::default()).unwrap();
    let rels: Vec<String> = a.files.iter().map(|f| crate::normalize_rel(&f.rel)).collect();
    assert_eq!(rels, ["stream/a.ydd", "stream/a_tex.ydd"]);
    // Resource-wide checks still cover every handled file.
    assert_eq!(a.resource_issues[0].text, "3 files");
}

#[test]
fn export_writes_a_complete_copy() {
    let fx = Fixture::new(&[
        ("fxmanifest.lua", b"manifest"),
        ("stream/a.ydd", b"abc"),
        ("stream/keep.ydd", b"keep"),
        ("stream/fail.ydd", b"fail"),
        ("stream/panic.ydd", b"panic"),
    ]);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &export(fx.out()), None, &Progress::default()).unwrap();
    let dest = fx.out().join("myres");
    assert_eq!(report.destination, dest);
    assert!(!report.in_place && !report.cancelled);
    assert_eq!(report.copied_files, 1);
    assert_eq!(read(&dest, "fxmanifest.lua"), b"manifest");
    assert_eq!(read(&dest, "stream/a.ydd"), b"ABC");
    // Unchanged and failed files are still written with their original content.
    assert_eq!(read(&dest, "stream/keep.ydd"), b"keep");
    assert_eq!(read(&dest, "stream/fail.ydd"), b"fail");
    assert_eq!(read(&dest, "stream/panic.ydd"), b"panic");
    // The source is never touched on export.
    assert_eq!(read(&fx.root, "stream/a.ydd"), b"abc");

    assert_eq!(status_of(&report, "stream/a.ydd"), FileStatus::Optimized);
    assert_eq!(status_of(&report, "stream/keep.ydd"), FileStatus::Unchanged);
    assert_eq!(status_of(&report, "stream/fail.ydd"), FileStatus::Error("broken".into()));
    assert!(matches!(status_of(&report, "stream/panic.ydd"), FileStatus::Error(e) if e.contains("internal crash")));
    // Report files are in resource order.
    let rels: Vec<&PathBuf> = report.files.iter().map(|f| &f.rel).collect();
    assert!(rels.windows(2).all(|w| w[0] < w[1]));
    let keep = report.files.iter().find(|f| f.rel.ends_with("keep.ydd")).unwrap();
    assert_eq!(keep.warnings, ["nothing to do"]);
}

#[test]
fn export_refuses_a_non_empty_destination() {
    let fx = Fixture::new(&[("a.ydd", b"a")]);
    write(&fx.out(), "myres/old.txt", b"old");
    let err = optimize(&Fake::default(), &fx.root, &Settings::default(), &export(fx.out()), None, &Progress::default()).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");

    let overwrite = Output::Export { dir: fx.out(), overwrite: true };
    optimize(&Fake::default(), &fx.root, &Settings::default(), &overwrite, None, &Progress::default()).unwrap();
    assert_eq!(read(&fx.out().join("myres"), "a.ydd"), b"A");
}

#[test]
fn export_refuses_the_source_or_a_folder_inside_it() {
    let fx = Fixture::new(&[("a.ydd", b"a")]);
    let run = |dir: PathBuf| optimize(&Fake::default(), &fx.root, &Settings::default(), &export(dir), None, &Progress::default());
    let err = run(fx.tmp.path().to_path_buf()).unwrap_err();
    assert!(err.to_string().contains("source folder itself"), "{err}");
    fs::create_dir_all(fx.root.join("export")).unwrap();
    let err = run(fx.root.join("export")).unwrap_err();
    assert!(err.to_string().contains("inside the source resource"), "{err}");
    assert_eq!(read(&fx.root, "a.ydd"), b"a");
}

#[test]
#[ignore = "known bug: export_root canonicalizes the source but not a destination that does not exist yet, so the check is bypassed"]
fn export_refuses_a_missing_folder_inside_the_source() {
    let fx = Fixture::new(&[("a.ydd", b"a")]);
    let output = export(fx.root.join("export"));
    let err = optimize(&Fake::default(), &fx.root, &Settings::default(), &output, None, &Progress::default()).unwrap_err();
    assert!(err.to_string().contains("inside the source resource"), "{err}");
    assert!(!fx.root.join("export").exists());
}

#[test]
fn export_selection_into_a_new_destination_copies_everything() {
    let fx = Fixture::new(&[("fxmanifest.lua", b"m"), ("a.ydd", b"a"), ("b.ydd", b"b")]);
    let only = sel(&["a.ydd"]);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &export(fx.out()), Some(&only), &Progress::default()).unwrap();
    let dest = fx.out().join("myres");
    assert_eq!(read(&dest, "a.ydd"), b"A");
    // Not selected: copied as is.
    assert_eq!(read(&dest, "b.ydd"), b"b");
    assert_eq!(report.copied_files, 2);
    assert_eq!(report.files.len(), 1);
}

#[test]
fn export_selection_into_an_existing_destination_updates_only_the_selection() {
    let fx = Fixture::new(&[("fxmanifest.lua", b"new"), ("a.ydd", b"a"), ("a_tex.ydd", b"t"), ("b.ydd", b"b")]);
    let dest = fx.out().join("myres");
    write(&dest, "fxmanifest.lua", b"old");
    write(&dest, "b.ydd", b"old");
    let method = Fake::linked(&[("a.ydd", "a_tex.ydd")]);
    let only = sel(&["a.ydd"]);
    let report = optimize(&method, &fx.root, &Settings::default(), &export(fx.out()), Some(&only), &Progress::default()).unwrap();
    assert_eq!(read(&dest, "a.ydd"), b"A");
    // Linked to the selection: processed in the same run.
    assert_eq!(read(&dest, "a_tex.ydd"), b"T");
    assert_eq!(read(&dest, "fxmanifest.lua"), b"old");
    assert_eq!(read(&dest, "b.ydd"), b"old");
    assert_eq!(report.copied_files, 0);
    assert_eq!(report.files.len(), 2);
}

#[test]
fn in_place_replaces_processed_files_atomically() {
    let fx = Fixture::new(&[("fxmanifest.lua", b"m"), ("a.ydd", b"a"), ("a.ytd", b"x"), ("keep.ydd", b"keep"), ("fail.ydd", b"fail")]);
    let progress = Progress::default();
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &Output::InPlace, None, &progress).unwrap();
    assert!(report.in_place);
    assert_eq!(report.destination, fx.root);
    assert_eq!(report.copied_files, 0);
    assert_eq!(read(&fx.root, "a.ydd"), b"A");
    assert_eq!(read(&fx.root, "a.ytd"), b"x");
    assert_eq!(read(&fx.root, "keep.ydd"), b"keep");
    assert_eq!(read(&fx.root, "fail.ydd"), b"fail");
    // Only processed files count as steps in place, and no temporary file is left behind.
    assert_eq!(progress.total.load(Ordering::Relaxed), 3);
    let leftovers: Vec<_> = fs::read_dir(&fx.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn in_place_selection_leaves_other_files_alone() {
    let fx = Fixture::new(&[("a.ydd", b"a"), ("b.ydd", b"b")]);
    let only = sel(&["b.ydd"]);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &Output::InPlace, Some(&only), &Progress::default()).unwrap();
    assert_eq!(read(&fx.root, "a.ydd"), b"a");
    assert_eq!(read(&fx.root, "b.ydd"), b"B");
    assert_eq!(report.files.len(), 1);
}

#[test]
fn cancelled_export_reports_every_missing_file() {
    let fx = Fixture::new(&[("fxmanifest.lua", b"m"), ("a.ydd", b"a"), ("b.ydd", b"b")]);
    let progress = Progress::default();
    progress.cancel.store(true, Ordering::Relaxed);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &export(fx.out()), None, &progress).unwrap();
    assert!(report.cancelled);
    assert!(report.files.is_empty());
    assert_eq!(report.not_written, [PathBuf::from("a.ydd"), PathBuf::from("b.ydd"), PathBuf::from("fxmanifest.lua")]);
    assert!(report.to_text().contains("WARNING: 3 file(s) not written"));
}

#[test]
fn cancelled_in_place_run_touches_nothing() {
    let fx = Fixture::new(&[("a.ydd", b"a")]);
    let progress = Progress::default();
    progress.cancel.store(true, Ordering::Relaxed);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &Output::InPlace, None, &progress).unwrap();
    assert!(report.cancelled && report.files.is_empty() && report.not_written.is_empty());
    assert_eq!(read(&fx.root, "a.ydd"), b"a");
}

#[test]
fn report_groups_files_by_resource() {
    let fx = Fixture::new(&[
        ("[cat]/res1/fxmanifest.lua", b""),
        ("[cat]/res1/stream/a.ydd", b"a"),
        ("[cat]/res2/fxmanifest.lua", b""),
        ("[cat]/res2/b.ydd", b"fail"),
    ]);
    let report = optimize(&Fake::default(), &fx.root, &Settings::default(), &export(fx.out()), None, &Progress::default()).unwrap();
    let resources: Vec<PathBuf> = report.files.iter().map(|f| f.resource.clone()).collect();
    assert_eq!(resources, [Path::new("[cat]").join("res1"), Path::new("[cat]").join("res2")]);
    assert_eq!(report.by_resource().len(), 2);
    assert!(report.to_text().contains("Per resource"));
}
