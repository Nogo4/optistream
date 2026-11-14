//! Analysis tab: per-file issues and sizes.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use egui_material_icons::icons::*;
use optistream_core::pipeline::FIVEM_WARN_BYTES;
use optistream_core::mib;

use crate::app::App;
use crate::theme::{self, ACCENT, BORDER, CARD, ERR, TEXT, WARN, WEAK};

impl App {
    pub(in crate::app) fn analysis_tab(&mut self, ui: &mut Ui) {
        let Some(a) = &self.analysis else {
            theme::empty_state(
                ui,
                ICON_TABLE_CHART,
                "Aucune analyse",
                "Choisis le dossier de la ressource puis lance « Analyser ».",
            );
            return;
        };
        let warned = a.files.iter().filter(|f| f.size.fivem_warning()).count();
        let with_issues = a
            .files
            .iter()
            .filter(|f| f.analysis.as_ref().map(|x| !x.issues.is_empty()).unwrap_or(true))
            .count();
        let gfx: u64 = a.files.iter().map(|f| f.size.graphics).sum();
        ui.horizontal_top(|ui| {
            theme::stat(ui, ICON_FOLDER, "Fichiers", a.files.len().to_string(), WEAK);
            theme::stat(ui, ICON_CHECKLIST, "À optimiser", with_issues.to_string(), WARN);
            theme::stat(ui, ICON_WARNING, "> 16 Mio (FiveM)", warned.to_string(), if warned > 0 { ERR } else { ACCENT });
            theme::stat(ui, ICON_MEMORY, "Mémoire physique", mib(gfx), ACCENT);
        });
        ui.add_space(6.0);
        if self.analysis_settings.as_ref() != Some(&self.settings) {
            theme::icon_line(ui, ICON_INFO, WARN, "Les réglages ont changé depuis l'analyse : relance « Analyser » pour mettre à jour.");
        }
        for i in &a.resource_issues {
            let (ic, c) = theme::severity_icon(i.severity);
            theme::icon_line(ui, ic, c, &i.text);
        }
        ui.add_space(4.0);

        let filter = self.filter.to_lowercase();
        let rows: Vec<usize> = a
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let problems = f.analysis.as_ref().map(|x| !x.issues.is_empty()).unwrap_or(true);
                (!self.only_problems || problems)
                    && (filter.is_empty() || f.rel.to_string_lossy().to_lowercase().contains(&filter))
            })
            .map(|(i, _)| i)
            .collect();

        let mut optimize_now: Option<HashSet<PathBuf>> = None;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.only_problems, "Seulement les fichiers à optimiser");
            ui.add_space(12.0);
            ui.label(theme::icon(ICON_FILTER_LIST, 18.0, WEAK));
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filtrer par nom…").desired_width(240.0));
            if !self.checked.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new(theme::label(ICON_BOLT, &format!("Optimiser la sélection ({})", self.checked.len()))).color(theme::ACCENT_TEXT).strong())
                        .fill(ACCENT);
                    if ui.add_enabled(self.job.is_none(), b).clicked() {
                        optimize_now = Some(self.checked.clone());
                    }
                    if ui.button(theme::label(ICON_CLOSE, "Vider")).on_hover_text("Décocher tous les fichiers").clicked() {
                        self.checked.clear();
                    }
                });
            }
        });
        ui.add_space(4.0);

        let mut clicked = None;
        let all_checked = !rows.is_empty() && rows.iter().all(|&i| self.checked.contains(&a.files[i].rel));
        let mut toggle_all = None;
        let detail_height = if self.selected.is_some() { 190.0 } else { 0.0 };
        let table_height = ui.available_height() - detail_height;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(table_height)
            .column(Column::exact(26.0))
            .column(Column::exact(28.0))
            .column(Column::remainder().at_least(260.0).clip(true))
            .column(Column::exact(44.0))
            .column(Column::exact(84.0))
            .column(Column::exact(84.0))
            .column(Column::initial(240.0).clip(true))
            .header(24.0, |mut h| {
                h.col(|ui| {
                    let mut v = all_checked;
                    if ui.checkbox(&mut v, "").on_hover_text("Cocher / décocher les fichiers affichés").changed() {
                        toggle_all = Some(v);
                    }
                });
                for t in ["", "Fichier", "Type", "Physique", "Virtuelle", "Résumé"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(WEAK));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, rows.len(), |mut row| {
                    let idx = rows[row.index()];
                    let f = &a.files[idx];
                    row.set_selected(self.selected == Some(idx));
                    let (kind, summary, worst, count) = match &f.analysis {
                        Ok(x) => (x.kind.clone(), x.summary.clone(), x.issues.iter().map(|i| i.severity).max(), x.issues.len()),
                        Err(e) => ("?".into(), e.clone(), Some(optistream_core::Severity::Error), 1),
                    };
                    row.col(|ui| {
                        let mut v = self.checked.contains(&f.rel);
                        if ui.checkbox(&mut v, "").changed() {
                            if v {
                                self.checked.insert(f.rel.clone());
                            } else {
                                self.checked.remove(&f.rel);
                            }
                        }
                    });
                    row.col(|ui| {
                        let (ic, c) = match worst {
                            Some(s) => theme::severity_icon(s),
                            None => (ICON_CHECK_CIRCLE, ACCENT),
                        };
                        ui.label(theme::icon(ic, 17.0, c)).on_hover_text(match count {
                            0 => "Conforme".to_owned(),
                            n => format!("{n} point(s) à corriger"),
                        });
                    });
                    row.col(|ui| {
                        ui.label(f.rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                            .on_hover_text(f.rel.display().to_string());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(kind).small().color(WEAK));
                    });
                    row.col(|ui| {
                        let c = if f.size.graphics > FIVEM_WARN_BYTES { ERR } else { TEXT };
                        ui.label(RichText::new(mib(f.size.graphics)).color(c));
                    });
                    row.col(|ui| {
                        let c = if f.size.system > FIVEM_WARN_BYTES { WARN } else { TEXT };
                        ui.label(RichText::new(mib(f.size.system)).color(c));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(summary).small().color(WEAK));
                    });
                    if row.response().clicked() {
                        clicked = Some(idx);
                    }
                });
            });
        if let Some(i) = clicked {
            self.selected = Some(i);
        }
        if let Some(v) = toggle_all {
            for &i in &rows {
                if v {
                    self.checked.insert(a.files[i].rel.clone());
                } else {
                    self.checked.remove(&a.files[i].rel);
                }
            }
        }

        let mut compare = None;
        if let Some(sel) = self.selected {
            if let Some(f) = a.files.get(sel) {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new(theme::label(ICON_COMPARE, "Comparer avant / après")))
                                .clicked()
                            {
                                compare = Some(f.rel.clone());
                            }
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new(theme::label(ICON_BOLT, "Optimiser ce fichier")))
                                .clicked()
                            {
                                optimize_now = Some(HashSet::from([f.rel.clone()]));
                            }
                            ui.add(egui::Label::new(RichText::new(f.rel.display().to_string()).strong()).truncate());
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("issues").max_height(120.0).show(ui, |ui| match &f.analysis {
                            Ok(x) if x.issues.is_empty() => theme::icon_line(ui, ICON_CHECK_CIRCLE, ACCENT, "Rien à signaler."),
                            Ok(x) => {
                                for i in &x.issues {
                                    let (ic, c) = theme::severity_icon(i.severity);
                                    theme::icon_line(ui, ic, c, &i.text);
                                }
                            }
                            Err(e) => theme::icon_line(ui, ICON_ERROR, ERR, e),
                        });
                    });
            }
        }
        let ctx = ui.ctx().clone();
        if let Some(rel) = compare {
            self.start_preview(&ctx, rel);
        }
        if let Some(only) = optimize_now {
            self.request_optimize(&ctx, Some(only));
        }
    }
}
