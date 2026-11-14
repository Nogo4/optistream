//! Side panel: resource type, source folder, settings, output and actions.

use std::path::PathBuf;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui, Vec2};
use egui_material_icons::icons::*;
use optistream_core::{ParamKind, Value};

use crate::app::{App, OutputMode};
use crate::theme::{self, ACCENT, BG, BORDER, TEXT, WARN, WEAK};

impl App {
    pub(super) fn side_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        ui.add_space(10.0);
        // Steps scroll; the action buttons stay pinned at the bottom.
        let actions_height = 96.0;
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - actions_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.steps(ui, busy));
        ui.add_space(8.0);
        self.actions(ui, &ctx, busy);
    }

    pub(super) fn steps(&mut self, ui: &mut Ui, busy: bool) {
        ui.add_enabled_ui(!busy, |ui| {
            theme::card(ui, 1, ICON_APPAREL, "Ressource", |ui| {
                ui.label(RichText::new("Type").small().color(WEAK));
                let current = self.methods[self.method].label();
                let mut changed = false;
                egui::ComboBox::from_id_salt("method")
                    .selected_text(current)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (i, m) in self.methods.iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.method, i, m.label()).changed();
                        }
                    });
                if changed {
                    self.settings = self.method().default_settings();
                    self.analysis = None;
                }
                ui.label(RichText::new(self.methods[self.method].description()).small().color(WEAK));
                ui.add_space(4.0);
                ui.label(RichText::new("Dossier de la ressource (ou d'une catégorie [...])").small().color(WEAK));
                theme::path_field(ui, &mut self.source, "Choisir le dossier de la ressource");
            });
            ui.add_space(8.0);
            theme::card(ui, 2, ICON_TUNE, "Réglages", |ui| self.settings_ui(ui));
            ui.add_space(8.0);
            theme::card(ui, 3, ICON_OUTPUT, "Sortie", |ui| self.output_ui(ui));
        });
    }

    pub(super) fn actions(&mut self, ui: &mut Ui, ctx: &egui::Context, busy: bool) {
        let ready = self.source.is_some() && !busy;
        let half = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            if theme::secondary_button(ui, ready, ICON_SEARCH, "Analyser", half).clicked() {
                self.start_analysis(ctx);
            }
            if theme::secondary_button(ui, ready && self.selected.is_some(), ICON_COMPARE, "Comparer", half)
                .on_hover_text("Comparer avant / après le fichier sélectionné dans l'analyse")
                .clicked()
            {
                if let Some(rel) = self.selected_rel() {
                    self.start_preview(ctx, rel);
                }
            }
        });
        ui.add_space(4.0);
        // Ticked files turn the main action into a partial optimisation.
        let text = match self.checked.len() {
            0 => "Optimiser toute la ressource".to_owned(),
            n => format!("Optimiser la sélection ({n})"),
        };
        if theme::primary_button(ui, ready, ICON_BOLT, &text).clicked() {
            let only = (!self.checked.is_empty()).then(|| self.checked.clone());
            self.request_optimize(ctx, only);
        }
        ui.add_space(10.0);
    }

    pub(super) fn selected_rel(&self) -> Option<PathBuf> {
        let a = self.analysis.as_ref()?;
        Some(a.files.get(self.selected?)?.rel.clone())
    }

    pub(super) fn output_ui(&mut self, ui: &mut Ui) {
        let option = |ui: &mut Ui, current: &mut OutputMode, value: OutputMode, ic, title: &str, hint: &str| {
            let selected = *current == value;
            let stroke = if selected { Stroke::new(1.5, ACCENT) } else { Stroke::new(1.0, BORDER) };
            let resp = egui::Frame::new()
                .fill(if selected { ACCENT.gamma_multiply(0.08) } else { BG })
                .stroke(stroke)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ic, 20.0, if selected { ACCENT } else { WEAK }));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(hint).small().color(WEAK));
                        });
                    });
                })
                .response
                .interact(egui::Sense::click());
            if resp.clicked() {
                *current = value;
            }
        };
        option(ui, &mut self.output, OutputMode::Export, ICON_DRIVE_FILE_MOVE, "Exporter", "Nouvelle ressource optimisée, l'originale reste intacte");
        if self.output == OutputMode::Export {
            ui.add_space(2.0);
            theme::path_field(ui, &mut self.export_dir, "Choisir le dossier d'export");
            if let (Some(src), Some(dir)) = (&self.source, &self.export_dir) {
                if let Some(name) = src.file_name() {
                    ui.horizontal(|ui| {
                        ui.label(theme::icon(ICON_ARROW_FORWARD, 13.0, WEAK));
                        ui.add(egui::Label::new(RichText::new(dir.join(name).display().to_string()).small().color(WEAK)).truncate());
                    });
                }
            }
            ui.checkbox(&mut self.overwrite, "Écraser si le dossier existe déjà");
            ui.add_space(4.0);
        }
        option(ui, &mut self.output, OutputMode::InPlace, ICON_EDIT_DOCUMENT, "Modifier sur place", "Remplace les fichiers du dossier source");
        if self.output == OutputMode::InPlace {
            theme::icon_line(ui, ICON_WARNING, WARN, "Sans sauvegarde : les fichiers d'origine sont remplacés.");
        }
    }

    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let method = self.method();
        let presets = method.presets();
        let active = presets.iter().find(|p| p.values == self.settings).map(|p| p.id);

        // Segmented preset selector.
        let width = (ui.available_width() - 8.0 * (presets.len() as f32 - 1.0)) / presets.len() as f32;
        ui.horizontal(|ui| {
            for p in &presets {
                let on = active == Some(p.id);
                let b = egui::Button::new(RichText::new(p.label).color(if on { theme::ACCENT_TEXT } else { TEXT }).strong())
                    .fill(if on { ACCENT } else { BG })
                    .stroke(Stroke::new(1.0, if on { ACCENT } else { BORDER }))
                    .corner_radius(CornerRadius::same(6))
                    .min_size(Vec2::new(width, 28.0));
                if ui.add(b).on_hover_text(p.description).clicked() {
                    self.settings = p.values.clone();
                }
            }
        });
        match presets.iter().find(|p| Some(p.id) == active) {
            Some(p) => {
                ui.label(RichText::new(p.description).small().color(WEAK));
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(theme::icon(ICON_TUNE, 14.0, WARN));
                    ui.label(RichText::new("Réglages personnalisés").small().color(WARN));
                    if ui.small_button(theme::label(ICON_RESTART_ALT, "Par défaut")).clicked() {
                        self.settings = method.default_settings();
                    }
                });
            }
        }
        ui.add_space(4.0);

        let params = method.params();
        let mut groups: Vec<&str> = Vec::new();
        for p in &params {
            if !groups.contains(&p.group) {
                groups.push(p.group);
            }
        }
        for group in groups {
            let ic = match group {
                "Textures" => ICON_TEXTURE,
                "Maillages" => ICON_VIEW_IN_AR,
                _ => ICON_TUNE,
            };
            egui::CollapsingHeader::new(theme::label(ic, group)).id_salt(group).default_open(false).show(ui, |ui| {
                egui::Grid::new(group).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for p in params.iter().filter(|p| p.group == group) {
                        ui.label(RichText::new(p.label).small()).on_hover_text(p.help);
                        match &p.kind {
                            ParamKind::Bool => {
                                let mut v = self.settings.bool(p.key, false);
                                if ui.checkbox(&mut v, "").on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Bool(v));
                                }
                            }
                            ParamKind::Int { min, max, step, unit } => {
                                let mut v = self.settings.int(p.key, *min);
                                let w = egui::DragValue::new(&mut v)
                                    .range(*min..=*max)
                                    .speed(*step as f64 / 4.0)
                                    .suffix(format!(" {unit}"));
                                if ui.add(w).on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Int(v));
                                }
                            }
                            ParamKind::Float { min, max, step, unit } => {
                                let mut v = self.settings.float(p.key, *min);
                                let w = egui::DragValue::new(&mut v)
                                    .range(*min..=*max)
                                    .speed(*step)
                                    .fixed_decimals(2)
                                    .suffix(format!(" {unit}"));
                                if ui.add(w).on_hover_text(p.help).changed() {
                                    self.settings.set(p.key, Value::Float(v));
                                }
                            }
                            ParamKind::Choice(options) => {
                                let mut v = self.settings.int(p.key, options[0].0);
                                let text = options.iter().find(|o| o.0 == v).map(|o| o.1).unwrap_or("?");
                                egui::ComboBox::from_id_salt(p.key).selected_text(text).width(90.0).show_ui(ui, |ui| {
                                    for (val, label) in options {
                                        if ui.selectable_value(&mut v, *val, *label).changed() {
                                            self.settings.set(p.key, Value::Int(v));
                                        }
                                    }
                                });
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        }
    }
}
