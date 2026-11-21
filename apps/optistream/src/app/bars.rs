//! Top bar and status bar.

use std::sync::atomic::Ordering;

use eframe::egui::{self, CornerRadius, RichText, Stroke, Ui, Vec2};
use egui_material_icons::icons::*;
use optistream_i18n::{t, tf, Lang};

use crate::app::App;
use crate::theme::{self, ACCENT, BG, BORDER, TEXT, WEAK};

/// Compact EN / FR segmented selector; the whole UI follows on the next frame.
fn language_selector(ui: &mut Ui) {
    let current = optistream_i18n::lang();
    // Right-to-left layout: add the last language first so they read in order.
    for lang in Lang::ALL.into_iter().rev() {
        let on = lang == current;
        let text = RichText::new(lang.code().to_ascii_uppercase())
            .small()
            .strong()
            .color(if on { theme::ACCENT_TEXT } else { WEAK });
        let b = egui::Button::new(text)
            .fill(if on { ACCENT } else { BG })
            .stroke(Stroke::new(1.0, if on { ACCENT } else { BORDER }))
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(32.0, 22.0));
        let hint = format!("{}\n{}", lang.native_name(), t("app.top.lang_hint"));
        if ui.add(b).on_hover_text(hint).clicked() && !on {
            optistream_i18n::set_lang(lang);
        }
    }
    ui.label(theme::icon(ICON_TRANSLATE, 16.0, WEAK));
}

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(theme::icon(ICON_SPEED, 26.0, ACCENT));
            ui.label(RichText::new("OptiStream").size(20.0).strong().color(TEXT));
            ui.label(RichText::new(t("app.top.tagline")).color(WEAK));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                language_selector(ui);
                ui.add_space(16.0);
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
                    ui.label(RichText::new(tf!("app.status.running", task = job.label)).strong());
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
                        if ui.button(theme::label(ICON_STOP_CIRCLE, t("app.common.cancel"))).clicked() {
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
