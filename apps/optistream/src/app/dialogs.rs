//! Modal dialogs (close confirmation, overwrite confirmation...).

use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, RichText};
use egui_material_icons::icons::*;

use optistream_i18n::{t, tf};

use crate::app::App;
use crate::theme::{self, ERR};

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if self.confirm_close {
            egui::Window::new(theme::label(ICON_WARNING, t("app.dialog.close.title")))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(t("app.dialog.close.line1"));
                    ui.label(t("app.dialog.close.line2"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let quit = egui::Button::new(RichText::new(t("app.dialog.close.quit")).color(Color32::WHITE)).fill(ERR);
                        if ui.add(quit).clicked() {
                            if let Some(job) = &self.job {
                                job.progress.cancel.store(true, Ordering::Relaxed);
                            }
                            self.allow_close = true;
                            self.confirm_close = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button(t("app.dialog.close.continue")).clicked() {
                            self.confirm_close = false;
                        }
                    });
                });
        }
        if let Some(only) = self.confirm_in_place.clone() {
            let mut decision = None;
            egui::Window::new(theme::label(ICON_WARNING, t("app.dialog.in_place.title")))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    match &only {
                        None => ui.label(t("app.dialog.in_place.all")),
                        Some(sel) if sel.len() == 1 => ui.label(t("app.dialog.in_place.one")),
                        Some(sel) => ui.label(tf!("app.dialog.in_place.many", count = sel.len())),
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
                        let confirm = egui::Button::new(RichText::new(theme::label(ICON_BOLT, t("app.dialog.in_place.confirm"))).color(Color32::WHITE)).fill(ERR);
                        if ui.add(confirm).clicked() {
                            decision = Some(true);
                        }
                        if ui.button(t("app.common.cancel")).clicked() {
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
            egui::Window::new(theme::label(ICON_ERROR, t("app.common.error")))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(RichText::new(err).color(ERR));
                    ui.add_space(8.0);
                    if ui.button(t("app.common.ok")).clicked() {
                        self.error = None;
                    }
                });
        }
    }
}
