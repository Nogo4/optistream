//! Comparison tab: before/after textures and meshes.

use std::collections::HashSet;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui, Vec2};
use egui_material_icons::icons::*;
use optistream_core::{PreviewContent, PreviewItem};

use crate::app::preview::{load_image, load_preview, side};
use crate::app::App;
use crate::render::{self, View};
use crate::theme::{self, ACCENT, BORDER, CARD, WEAK};

impl App {
    pub(in crate::app) fn comparison_tab(&mut self, ui: &mut Ui) {
        let Some(file) = &self.preview_file else {
            theme::empty_state(
                ui,
                ICON_COMPARE,
                "Aucune comparaison",
                "Sélectionne un fichier dans l'analyse puis « Comparer avant / après ».",
            );
            return;
        };
        let mut optimize_now = None;
        let mut tick = None;
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_DEPLOYED_CODE, 18.0, ACCENT));
            ui.add(egui::Label::new(RichText::new(file.display().to_string()).strong()).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Optimiser ce fichier")).color(theme::ACCENT_TEXT).strong())
                    .fill(ACCENT);
                if ui.add_enabled(self.job.is_none(), b).on_hover_text("Applique les réglages actuels à ce seul fichier (selon la sortie choisie)").clicked() {
                    optimize_now = Some(HashSet::from([file.clone()]));
                }
                let mut v = self.checked.contains(file);
                if ui.checkbox(&mut v, "Ajouter à la sélection").changed() {
                    tick = Some(v);
                }
            });
        });
        let file = file.clone();
        if let Some(v) = tick {
            if v {
                self.checked.insert(file.clone());
            } else {
                self.checked.remove(&file);
            }
        }
        if let Some(only) = optimize_now {
            let ctx = ui.ctx().clone();
            self.request_optimize(&ctx, Some(only));
        }
        ui.horizontal(|ui| {
            ui.label(theme::icon(ICON_ZOOM_IN, 18.0, WEAK));
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=4.0).logarithmic(true).show_value(true));
            ui.add_space(10.0);
            ui.label(theme::icon(ICON_ROTATE_RIGHT, 18.0, WEAK));
            ui.add(egui::Slider::new(&mut self.yaw, -180.0..=180.0).suffix("°"));
            ui.add_space(10.0);
            ui.checkbox(&mut self.wireframe, theme::label(ICON_GRID_ON, "Fil de fer"));
            if ui
                .checkbox(&mut self.show_alpha, theme::label(ICON_OPACITY, "Alpha"))
                .on_hover_text("Sinon seules les couleurs RVB sont affichées (l'alpha des spéculaires = brillance).")
                .changed()
            {
                let ctx = ui.ctx().clone();
                let items: Vec<PreviewItem> = self.previews.drain(..).map(|p| p.item).collect();
                self.previews = items.into_iter().enumerate().map(|(i, item)| load_preview(&ctx, i, item, self.show_alpha)).collect();
            }
        });
        theme::icon_line(ui, ICON_INFO, WEAK, "Réglages actuels appliqués ; rien n'est écrit sur le disque.");
        ui.add_space(4.0);
        if self.previews.is_empty() {
            theme::empty_state(ui, ICON_COMPARE, "Rien à comparer", "Ce fichier ne contient ni maillage ni texture modifiable.");
            return;
        }

        let ctx = ui.ctx().clone();
        let view = View { yaw: self.yaw.to_radians(), wireframe: self.wireframe };
        let zoom = self.zoom;
        egui::ScrollArea::both().show(ui, |ui| {
            for (i, p) in self.previews.iter_mut().enumerate() {
                let is_mesh = matches!(p.item.content, PreviewContent::Mesh { .. });
                if let PreviewContent::Mesh { before, after } = &p.item.content {
                    if p.rendered != Some((view.yaw, view.wireframe)) {
                        let frame = *p.frame.get_or_insert_with(|| render::bounds(before));
                        p.before = Some(load_image(&ctx, &format!("mb{i}"), &render::render(before, frame, &view, 512)));
                        p.after = after.as_ref().map(|a| load_image(&ctx, &format!("ma{i}"), &render::render(a, frame, &view, 512)));
                        p.rendered = Some((view.yaw, view.wireframe));
                    }
                }
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(if is_mesh { ICON_VIEW_IN_AR } else { ICON_IMAGE }, 18.0, ACCENT));
                            ui.label(RichText::new(&p.item.title).strong());
                        });
                        ui.add_space(4.0);
                        let size = p.before.as_ref().map(|t| t.size_vec2() * zoom).unwrap_or_default();
                        // Same display width as "before": a smaller texture shows its loss of detail.
                        // A cropped one keeps its own proportions instead of being stretched.
                        let after_size = match p.after.as_ref().map(|t| t.size_vec2()) {
                            Some(a) if a.x > 0.0 && size.x > 0.0 && (a.y / a.x - size.y / size.x).abs() > 1e-3 => {
                                Vec2::new(size.x, size.x * a.y / a.x)
                            }
                            _ => size,
                        };
                        ui.horizontal_top(|ui| {
                            side(ui, "Avant", &p.item.before_desc, p.before.as_ref(), size, WEAK);
                            ui.add_space(8.0);
                            side(ui, "Après", &p.item.after_desc, p.after.as_ref(), after_size, ACCENT);
                        });
                    });
                ui.add_space(8.0);
            }
        });
    }
}
