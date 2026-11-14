//! Background jobs: analysis, optimisation and preview, started and polled.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;
use optistream_core::{mib, Analysis, Output};

use crate::app::preview::load_preview;
use crate::app::state::{merge_report, remaining_entry};
use crate::app::{App, OutputMode, Tab};
use crate::jobs::{self, JobResult};

impl App {
    pub(super) fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.job else { return };
        let Some(result) = job.take() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        self.job = None;
        match result {
            Ok(JobResult::Analysis { analysis, partial }) => {
                self.status = format!("Analyse terminée : {} fichier(s)", analysis.files.len());
                if partial {
                    self.merge_analysis(analysis);
                } else {
                    self.analysis = Some(analysis);
                    self.selected = None;
                    self.checked.clear();
                }
                self.tab = Tab::Analysis;
            }
            Ok(JobResult::Report { report, verification, selection }) => {
                let partial = selection.is_some();
                let t = report.totals();
                self.status = format!(
                    "Optimisation terminée : {} / {} fichier(s) optimisé(s), mémoire physique {} → {}",
                    t.optimized,
                    t.processed,
                    mib(t.graphics_before),
                    mib(t.graphics_after)
                );
                // What is left to fix in the files that were just written.
                let same_target = self.report.as_ref().is_some_and(|r| r.destination == report.destination);
                if !(partial && same_target) {
                    self.remaining.clear();
                }
                for f in &report.files {
                    let checked = verification.as_ref().and_then(|v| v.files.iter().find(|x| x.rel == f.rel));
                    match remaining_entry(f, checked) {
                        Some(r) => {
                            self.remaining.insert(f.rel.clone(), r);
                        }
                        None => {
                            self.remaining.remove(&f.rel);
                        }
                    }
                }
                // In place, the source is the destination: keep the analysis table truthful.
                if report.in_place {
                    if let Some(v) = verification {
                        self.merge_analysis(v);
                    }
                }
                if partial {
                    self.checked.clear();
                }
                match (&mut self.report, &selection, same_target) {
                    (Some(old), Some(sel), true) => merge_report(old, report, sel),
                    _ => self.report = Some(report),
                }
                self.tab = Tab::Report;
            }
            Ok(JobResult::Preview(file, items)) => {
                self.status = "Comparaison prête".to_owned();
                self.preview_file = Some(file);
                self.previews = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| load_preview(ctx, i, item, self.show_alpha))
                    .collect();
                self.tab = Tab::Comparison;
            }
            Err(e) => {
                self.status = "Erreur".to_owned();
                self.error = Some(e);
            }
        }
    }

    /// Replaces the analysed entries of a partial analysis, keeping the others.
    pub(super) fn merge_analysis(&mut self, fresh: Analysis) {
        let Some(a) = &mut self.analysis else {
            self.analysis = Some(fresh);
            return;
        };
        for f in fresh.files {
            match a.files.iter_mut().find(|x| x.rel == f.rel) {
                Some(slot) => *slot = f,
                None => a.files.push(f),
            }
        }
        a.resource_issues = fresh.resource_issues;
    }

    pub(super) fn start_analysis(&mut self, ctx: &egui::Context) {
        if let Some(src) = self.source.clone() {
            self.analysis_settings = Some(self.settings.clone());
            self.job = Some(jobs::analyze(ctx.clone(), self.method(), src, self.settings.clone(), None));
        }
    }

    /// Optimises everything (`None`) or the given files, asking first when working in place.
    pub(super) fn request_optimize(&mut self, ctx: &egui::Context, only: Option<HashSet<PathBuf>>) {
        if self.output == OutputMode::InPlace {
            self.confirm_in_place = Some(only);
        } else {
            self.start_optimize(ctx, only);
        }
    }

    pub(super) fn start_optimize(&mut self, ctx: &egui::Context, only: Option<HashSet<PathBuf>>) {
        let Some(src) = self.source.clone() else { return };
        let output = match self.output {
            OutputMode::InPlace => Output::InPlace,
            OutputMode::Export => match &self.export_dir {
                Some(d) => Output::Export { dir: d.clone(), overwrite: self.overwrite },
                None => {
                    self.error = Some("Choisis un dossier d'export.".into());
                    return;
                }
            },
        };
        self.job = Some(jobs::optimize(ctx.clone(), self.method(), src, self.settings.clone(), output, only));
    }

    pub(super) fn start_preview(&mut self, ctx: &egui::Context, rel: PathBuf) {
        let Some(src) = self.source.clone() else { return };
        let file = match optistream_core::contexts(&src) {
            Ok(all) => all.into_iter().find(|c| c.rel == rel),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        if let Some(file) = file {
            self.job = Some(jobs::preview(ctx.clone(), self.method(), file, self.settings.clone()));
        }
    }
}
