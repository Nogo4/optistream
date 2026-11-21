//! Report tab: optimisation outcome per file and per resource.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Ui};
use egui_material_icons::icons::*;
use optistream_core::{mib, FileStatus};
use optistream_i18n::{t, tf};

use crate::app::App;
use crate::theme::{self, ACCENT, BG, CARD, ERR, TEXT, WARN, WEAK};

impl App {
    pub(in crate::app) fn report_tab(&mut self, ui: &mut Ui) {
        let Some(r) = &self.report else {
            theme::empty_state(ui, ICON_DESCRIPTION, t("app.report.empty.title"), t("app.report.empty.hint"));
            return;
        };
        let totals = r.totals();
        ui.horizontal_top(|ui| {
            theme::stat(ui, ICON_TASK_ALT, t("app.report.stat.optimised"), format!("{} / {}", totals.optimized, totals.processed), ACCENT);
            theme::stat(ui, ICON_ERROR, t("app.report.stat.errors"), totals.errors.to_string(), if totals.errors > 0 { ERR } else { WEAK });
            theme::stat(ui, ICON_MEMORY, t("app.common.physical_memory"), format!("{} → {}", mib(totals.graphics_before), mib(totals.graphics_after)), ACCENT);
            theme::stat(ui, ICON_HARD_DRIVE, t("app.report.stat.disk"), format!("{} → {}", mib(totals.disk_before), mib(totals.disk_after)), WEAK);
            theme::stat(ui, ICON_WARNING, t("app.report.stat.fivem"), format!("{} → {}", totals.warnings_before, totals.warnings_after), WARN);
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_FOLDER, 16.0, WEAK));
            ui.add(egui::Label::new(RichText::new(r.destination.display().to_string()).color(WEAK)).truncate());
            if ui.button(theme::label(ICON_FOLDER_OPEN, t("app.report.open"))).clicked() {
                let _ = std::process::Command::new("explorer").arg(&r.destination).spawn();
            }
            if ui.button(theme::label(ICON_SAVE, t("app.report.save"))).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title(t("app.report.save")).set_file_name(t("app.report.file_name")).save_file() {
                    if let Err(e) = std::fs::write(&path, r.to_text()) {
                        self.error = Some(e.to_string());
                    }
                }
            }
        });
        // A category folder ([oren-mappings]) holds many resources: totals for each one.
        let resources = r.by_resource();
        if resources.len() > 1 {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new(tf!("app.report.by_resource", count = resources.len())).strong())
                .id_salt("report_by_resource")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("report_by_resource_scroll").max_height(220.0).show(ui, |ui| {
                        egui::Grid::new("report_by_resource_grid").striped(true).num_columns(4).show(ui, |ui| {
                            for h in [t("app.report.col.resource"), t("app.report.stat.fivem"), t("app.common.physical_memory"), t("app.report.stat.optimised")] {
                                ui.label(RichText::new(h).small().color(WEAK));
                            }
                            ui.end_row();
                            for (dir, rt) in &resources {
                                let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| t("app.report.root").to_owned());
                                ui.label(name).on_hover_text(dir.display().to_string());
                                let color = if rt.warnings_after > 0 { WARN } else { ACCENT };
                                ui.label(RichText::new(format!("{} → {}", rt.warnings_before, rt.warnings_after)).color(color));
                                ui.label(format!("{} → {}", mib(rt.graphics_before), mib(rt.graphics_after)));
                                ui.label(format!("{} / {}", rt.optimized, rt.processed));
                                ui.end_row();
                            }
                        });
                    });
                });
        }
        let mut compare = None;
        let mut reoptimize: Option<HashSet<PathBuf>> = None;
        let idle = self.job.is_none();
        if !r.not_written.is_empty() {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(ERR.gamma_multiply(0.12))
                .stroke(Stroke::new(1.0, ERR))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ICON_ERROR, 22.0, ERR));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(tf!("app.report.incomplete", count = r.not_written.len())).strong().color(ERR));
                            ui.label(RichText::new(t("app.report.incomplete_hint")).small());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new(theme::label(ICON_RESTART_ALT, t("app.report.complete_export"))).color(Color32::WHITE).strong()).fill(ERR);
                            if ui.add_enabled(idle, b).on_hover_text(t("app.report.complete_export_hint")).clicked() {
                                reoptimize = Some(r.not_written.iter().cloned().collect());
                            }
                        });
                    });
                });
        } else if r.cancelled {
            theme::icon_line(ui, ICON_WARNING, WARN, t("app.report.cancelled"));
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Files still carrying warnings after the run, each fixable on its own.
            if self.remaining.is_empty() {
                theme::icon_line(ui, ICON_CHECK_CIRCLE, ACCENT, t("app.report.no_remaining"));
            } else {
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, WARN.gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(ICON_WARNING, 20.0, WARN));
                            ui.label(RichText::new(tf!("app.report.to_review", count = self.remaining.len())).strong().size(15.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let all = egui::Button::new(RichText::new(theme::label(ICON_BOLT, t("app.report.reoptimise_all"))).color(theme::ACCENT_TEXT).strong())
                                    .fill(ACCENT);
                                if ui.add_enabled(idle, all).on_hover_text(t("app.report.reoptimise_all_hint")).clicked() {
                                    reoptimize = Some(self.remaining.keys().cloned().collect());
                                }
                            });
                        });
                        ui.label(
                            RichText::new(t("app.report.review_hint"))
                                .small()
                                .color(WEAK),
                        );
                        ui.add_space(6.0);
                        for (rel, rem) in &self.remaining {
                            let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let (ic, color) = theme::severity_icon(rem.worst);
                            egui::Frame::new()
                                .fill(BG)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(theme::icon(ic, 18.0, color));
                                        ui.add(egui::Label::new(RichText::new(&name).strong()).truncate())
                                            .on_hover_text(rel.display().to_string());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_BOLT, t("app.common.optimise")))).clicked() {
                                                reoptimize = Some(HashSet::from([rel.clone()]));
                                            }
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_COMPARE, t("app.common.compare")))).clicked() {
                                                compare = Some(rel.clone());
                                            }
                                        });
                                    });
                                    for i in &rem.issues {
                                        let (ic, c) = theme::severity_icon(i.severity);
                                        theme::icon_line(ui, ic, c, &i.text);
                                    }
                                    for n in &rem.notes {
                                        theme::icon_line(ui, ICON_INFO, WEAK, n);
                                    }
                                });
                            ui.add_space(4.0);
                        }
                    });
            }
            ui.add_space(8.0);

            egui::CollapsingHeader::new(theme::label(ICON_CHECKLIST, t("app.report.details")))
                .default_open(false)
                .show(ui, |ui| {
                    for f in &r.files {
                        let name = f.rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        match &f.status {
                            FileStatus::Unchanged if f.warnings.is_empty() => continue,
                            FileStatus::Error(e) => theme::icon_line(ui, ICON_ERROR, ERR, tf!("app.report.file_error", name = name, error = e)),
                            _ => {
                                let ic = if f.warnings.is_empty() { ICON_CHECK_CIRCLE } else { ICON_WARNING };
                                egui::CollapsingHeader::new(format!(
                                    "{}  {name}   ·   {} → {}",
                                    ic.codepoint,
                                    mib(f.before.graphics),
                                    mib(f.after.graphics)
                                ))
                                .id_salt(&f.rel)
                                .show(ui, |ui| {
                                    for c in &f.changes {
                                        theme::icon_line(ui, ICON_ARROW_FORWARD, TEXT, c);
                                    }
                                    for w in &f.warnings {
                                        theme::icon_line(ui, ICON_WARNING, WARN, w);
                                    }
                                });
                            }
                        }
                    }
                });
        });
        let ctx = ui.ctx().clone();
        if let Some(rel) = compare {
            self.start_preview(&ctx, rel);
        }
        if let Some(only) = reoptimize {
            self.request_optimize(&ctx, Some(only));
        }
    }
}
