//! Top bar and status bar.

use std::sync::atomic::Ordering;

use eframe::egui::{self, RichText, Ui};
use egui_material_icons::icons::*;

use crate::app::App;
use crate::theme::{self, ACCENT, TEXT, WEAK};

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(theme::icon(ICON_SPEED, 26.0, ACCENT));
            ui.label(RichText::new("OptiStream").size(20.0).strong().color(TEXT));
            ui.label(RichText::new("Optimisation des streams FiveM").color(WEAK));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("GTA V Legacy").small().color(WEAK));
                ui.label(theme::icon(ICON_SPORTS_ESPORTS, 16.0, WEAK));
            });
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            match &self.job {
                Some(job) => {
                    let p = &job.progress;
                    let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
                    ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
                    ui.label(RichText::new(format!("{} en cours", job.label)).strong());
                    ui.add(
                        egui::ProgressBar::new(p.fraction())
                            .desired_width(260.0)
                            .fill(ACCENT)
                            .text(RichText::new(format!("{done} / {total}")).small()),
                    );
                    if let Ok(cur) = p.current.lock() {
                        ui.add(egui::Label::new(RichText::new(cur.as_str()).small().color(WEAK)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(theme::label(ICON_STOP_CIRCLE, "Annuler")).clicked() {
                            p.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                None => {
                    ui.label(theme::icon(ICON_TASK_ALT, 15.0, ACCENT));
                    ui.label(RichText::new(&self.status).small().color(WEAK));
                }
            }
        });
    }
}
