//! Modal dialogs (close confirmation, overwrite confirmation...).

use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, RichText};
use egui_material_icons::icons::*;

use crate::app::App;
use crate::theme::{self, ERR};

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if self.confirm_close {
            egui::Window::new(theme::label(ICON_WARNING, "Une tâche est en cours"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Quitter maintenant laisse les fichiers restants non traités :");
                    ui.label("un export serait incomplet (il manquerait des fichiers).");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let quit = egui::Button::new(RichText::new("Quitter quand même").color(Color32::WHITE)).fill(ERR);
                        if ui.add(quit).clicked() {
                            if let Some(job) = &self.job {
                                job.progress.cancel.store(true, Ordering::Relaxed);
                            }
                            self.allow_close = true;
                            self.confirm_close = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Continuer la tâche").clicked() {
                            self.confirm_close = false;
                        }
                    });
                });
        }
        if let Some(only) = self.confirm_in_place.clone() {
            let mut decision = None;
            egui::Window::new(theme::label(ICON_WARNING, "Modifier le dossier directement ?"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    match &only {
                        None => ui.label("Tous les fichiers concernés seront remplacés, sans sauvegarde."),
                        Some(sel) if sel.len() == 1 => ui.label("Ce fichier sera remplacé, sans sauvegarde :"),
                        Some(sel) => ui.label(format!("Ces {} fichiers seront remplacés, sans sauvegarde.", sel.len())),
                    };
                    match (&only, &self.source) {
                        (Some(sel), _) if sel.len() == 1 => {
                            let rel = sel.iter().next().unwrap();
                            ui.label(RichText::new(rel.display().to_string()).strong());
                        }
                        (_, Some(src)) => {
                            ui.label(RichText::new(src.display().to_string()).strong());
                        }
                        _ => {}
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let confirm = egui::Button::new(RichText::new(theme::label(ICON_BOLT, "Confirmer")).color(Color32::WHITE)).fill(ERR);
                        if ui.add(confirm).clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Annuler").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if let Some(go) = decision {
                self.confirm_in_place = None;
                if go {
                    self.start_optimize(ctx, only);
                }
            }
        }
        if let Some(err) = self.error.clone() {
            egui::Window::new(theme::label(ICON_ERROR, "Erreur"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(RichText::new(err).color(ERR));
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        self.error = None;
                    }
                });
        }
    }
}
