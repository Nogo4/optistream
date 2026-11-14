//! Central tabs: Analysis, Comparison, Report.

mod analysis;
mod comparison;
mod report;

use eframe::egui::{self, RichText, Stroke, Ui, Vec2};
use egui_material_icons::icons::*;

use crate::app::{App, Tab};
use crate::theme::{self, ACCENT, BORDER, WEAK};

impl App {
    pub(super) fn tab_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (tab, ic, title) in [
                (Tab::Analysis, ICON_TABLE_CHART, "Analyse"),
                (Tab::Comparison, ICON_COMPARE, "Comparaison"),
                (Tab::Report, ICON_DESCRIPTION, "Rapport"),
            ] {
                let on = self.tab == tab;
                let text = RichText::new(theme::label(ic, title)).size(15.0).color(if on { ACCENT } else { WEAK });
                let resp = ui.add(egui::Button::new(text).frame(false).min_size(Vec2::new(0.0, 30.0)));
                if on {
                    let r = resp.rect;
                    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(2.0, ACCENT));
                }
                if resp.clicked() {
                    self.tab = tab;
                }
                ui.add_space(12.0);
            }
        });
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, BORDER));
        ui.add_space(8.0);
    }
}
