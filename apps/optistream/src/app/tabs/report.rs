//! Report tab: optimisation outcome per file and per resource.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Ui};
use egui_material_icons::icons::*;
use optistream_core::{mib, FileStatus};

use crate::app::App;
use crate::theme::{self, ACCENT, BG, CARD, ERR, TEXT, WARN, WEAK};

impl App {
    pub(in crate::app) fn report_tab(&mut self, ui: &mut Ui) {
        let Some(r) = &self.report else {
            theme::empty_state(ui, ICON_DESCRIPTION, "Aucun rapport", "Le rapport apparaît ici après « Optimiser ».");
            return;
        };
        let t = r.totals();
        ui.horizontal_top(|ui| {
            theme::stat(ui, ICON_TASK_ALT, "Optimisés", format!("{} / {}", t.optimized, t.processed), ACCENT);
            theme::stat(ui, ICON_ERROR, "Erreurs", t.errors.to_string(), if t.errors > 0 { ERR } else { WEAK });
            theme::stat(ui, ICON_MEMORY, "Mémoire physique", format!("{} → {}", mib(t.graphics_before), mib(t.graphics_after)), ACCENT);
            theme::stat(ui, ICON_HARD_DRIVE, "Disque", format!("{} → {}", mib(t.disk_before), mib(t.disk_after)), WEAK);
            theme::stat(ui, ICON_WARNING, "> 16 Mio", format!("{} → {}", t.warnings_before, t.warnings_after), WARN);
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_FOLDER, 16.0, WEAK));
            ui.add(egui::Label::new(RichText::new(r.destination.display().to_string()).color(WEAK)).truncate());
            if ui.button(theme::label(ICON_FOLDER_OPEN, "Ouvrir")).clicked() {
                let _ = std::process::Command::new("explorer").arg(&r.destination).spawn();
            }
            if ui.button(theme::label(ICON_SAVE, "Enregistrer le rapport")).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_file_name("optistream-rapport.txt").save_file() {
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
            egui::CollapsingHeader::new(RichText::new(format!("Par ressource ({})", resources.len())).strong())
                .id_salt("report_by_resource")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("report_by_resource_scroll").max_height(220.0).show(ui, |ui| {
                        egui::Grid::new("report_by_resource_grid").striped(true).num_columns(4).show(ui, |ui| {
                            for h in ["Ressource", "> 16 Mio", "Mémoire physique", "Optimisés"] {
                                ui.label(RichText::new(h).small().color(WEAK));
                            }
                            ui.end_row();
                            for (dir, t) in &resources {
                                let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "(racine)".to_owned());
                                ui.label(name).on_hover_text(dir.display().to_string());
                                let color = if t.warnings_after > 0 { WARN } else { ACCENT };
                                ui.label(RichText::new(format!("{} → {}", t.warnings_before, t.warnings_after)).color(color));
                                ui.label(format!("{} → {}", mib(t.graphics_before), mib(t.graphics_after)));
                                ui.label(format!("{} / {}", t.optimized, t.processed));
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
                            ui.label(RichText::new(format!("Export incomplet : {} fichier(s) non écrit(s)", r.not_written.len())).strong().color(ERR));
                            ui.label(RichText::new("L'optimisation a été interrompue. Ne mets pas cette ressource en production telle quelle.").small());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new(theme::label(ICON_RESTART_ALT, "Compléter l'export")).color(Color32::WHITE).strong()).fill(ERR);
                            if ui.add_enabled(idle, b).on_hover_text("Écrit uniquement les fichiers manquants dans la destination").clicked() {
                                reoptimize = Some(r.not_written.iter().cloned().collect());
                            }
                        });
                    });
                });
        } else if r.cancelled {
            theme::icon_line(ui, ICON_WARNING, WARN, "Optimisation annulée : seule une partie des fichiers a été traitée.");
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Files still carrying warnings after the run, each fixable on its own.
            if self.remaining.is_empty() {
                theme::icon_line(ui, ICON_CHECK_CIRCLE, ACCENT, "Aucun avertissement restant sur les fichiers optimisés.");
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
                            ui.label(RichText::new(format!("À revoir ({})", self.remaining.len())).strong().size(15.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let all = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Tout ré-optimiser")).color(theme::ACCENT_TEXT).strong())
                                    .fill(ACCENT);
                                if ui.add_enabled(idle, all).on_hover_text("Relance ces fichiers avec les réglages actuels").clicked() {
                                    reoptimize = Some(self.remaining.keys().cloned().collect());
                                }
                            });
                        });
                        ui.label(
                            RichText::new("Ajuste les réglages à gauche (tolérance, tailles…), compare, puis relance fichier par fichier.")
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
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_BOLT, "Optimiser"))).clicked() {
                                                reoptimize = Some(HashSet::from([rel.clone()]));
                                            }
                                            if ui.add_enabled(idle, egui::Button::new(theme::label(ICON_COMPARE, "Comparer"))).clicked() {
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

            egui::CollapsingHeader::new(theme::label(ICON_CHECKLIST, "Détail des modifications"))
                .default_open(false)
                .show(ui, |ui| {
                    for f in &r.files {
                        let name = f.rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        match &f.status {
                            FileStatus::Unchanged if f.warnings.is_empty() => continue,
                            FileStatus::Error(e) => theme::icon_line(ui, ICON_ERROR, ERR, format!("{name} : {e}")),
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
