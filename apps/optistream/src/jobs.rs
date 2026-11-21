//! Background work (analysis, optimisation, previews) so the window stays responsive.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use optistream_core::{Analysis, FileCtx, Method, Output, PreviewItem, Progress, Report, Settings};
use optistream_i18n::t;

pub enum JobResult {
    /// `partial`: only some files were analysed, to be merged into the current analysis.
    Analysis { analysis: Analysis, partial: bool },
    /// Report, plus an analysis of the written files (what is left to fix).
    /// `selection`: the files of a partial run, to be merged into the current report.
    Report { report: Report, verification: Option<Analysis>, selection: Option<HashSet<PathBuf>> },
    Preview(PathBuf, Vec<PreviewItem>),
}

pub struct Job {
    pub label: &'static str,
    pub progress: Arc<Progress>,
    handle: Option<JoinHandle<()>>,
    result: Arc<Mutex<Option<Result<JobResult, String>>>>,
}

impl Job {
    fn spawn(
        label: &'static str,
        ctx: eframe::egui::Context,
        work: impl FnOnce(&Progress) -> Result<JobResult, String> + Send + 'static,
    ) -> Self {
        let progress = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let (p, r) = (progress.clone(), result.clone());
        let handle = std::thread::spawn(move || {
            // A panic must surface as an error instead of a job that never finishes.
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&p)))
                .unwrap_or_else(|_| Err(t("app.job.crashed").to_owned()));
            *r.lock().unwrap() = Some(out);
            ctx.request_repaint();
        });
        Job { label, progress, handle: Some(handle), result }
    }

    /// Returns the result once the job has finished.
    pub fn take(&mut self) -> Option<Result<JobResult, String>> {
        let out = self.result.lock().unwrap().take();
        if out.is_some() {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
        out
    }
}

pub fn analyze(
    ctx: eframe::egui::Context,
    method: Arc<dyn Method>,
    root: PathBuf,
    settings: Settings,
    only: Option<HashSet<PathBuf>>,
) -> Job {
    Job::spawn(t("app.job.analysis"), ctx, move |p| {
        optistream_core::analyze(method.as_ref(), &root, &settings, only.as_ref(), p)
            .map(|analysis| JobResult::Analysis { analysis, partial: only.is_some() })
            .map_err(|e| format!("{e:#}"))
    })
}

pub fn optimize(
    ctx: eframe::egui::Context,
    method: Arc<dyn Method>,
    root: PathBuf,
    settings: Settings,
    output: Output,
    only: Option<HashSet<PathBuf>>,
) -> Job {
    Job::spawn(t("app.job.optimisation"), ctx, move |p| {
        let report = optistream_core::optimize(method.as_ref(), &root, &settings, &output, only.as_ref(), p)
            .map_err(|e| format!("{e:#}"))?;
        // Re-analyse what was written so the report lists what is still left to fix.
        let written: HashSet<PathBuf> = report.files.iter().map(|f| f.rel.clone()).collect();
        if let Ok(mut current) = p.current.lock() {
            *current = t("app.job.verifying").to_owned();
        }
        let verification = if report.cancelled || written.is_empty() {
            None
        } else {
            optistream_core::analyze(method.as_ref(), &report.destination, &settings, Some(&written), p).ok()
        };
        Ok(JobResult::Report { report, verification, selection: only })
    })
}

pub fn preview(ctx: eframe::egui::Context, method: Arc<dyn Method>, file: FileCtx, settings: Settings) -> Job {
    Job::spawn(t("app.job.comparison"), ctx, move |_| {
        let data = std::fs::read(file.root.join(&file.rel)).map_err(|e| e.to_string())?;
        method
            .preview(&file, &data, &settings)
            .map(|items| JobResult::Preview(file.rel.clone(), items))
            .map_err(|e| format!("{e:#}"))
    })
}
