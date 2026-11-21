//! Visual identity: dark theme with a teal accent, icon font, shared widgets.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, Response, RichText, Stroke, TextStyle, Ui, Vec2,
};
use egui_material_icons::{icons, MaterialIcon};
use optistream_core::Severity;

pub const ACCENT: Color32 = Color32::from_rgb(72, 190, 170);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(12, 32, 29);
pub const WARN: Color32 = Color32::from_rgb(232, 180, 84);
pub const ERR: Color32 = Color32::from_rgb(236, 104, 94);
pub const BG: Color32 = Color32::from_rgb(19, 21, 25);
pub const PANEL: Color32 = Color32::from_rgb(24, 27, 32);
pub const CARD: Color32 = Color32::from_rgb(31, 35, 41);
pub const FIELD: Color32 = Color32::from_rgb(40, 45, 52);
pub const BORDER: Color32 = Color32::from_rgb(48, 54, 62);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 231);
pub const WEAK: Color32 = Color32::from_rgb(140, 148, 158);

pub fn apply(ctx: &egui::Context) {
    egui_material_icons::initialize(ctx);
    ctx.global_style_mut(|s| {
        s.interaction.selectable_labels = false;
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;

        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.extreme_bg_color = BG;
        v.faint_bg_color = Color32::from_rgb(28, 31, 37);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(6);
        }
        v.widgets.inactive.weak_bg_fill = FIELD;
        v.widgets.inactive.bg_fill = FIELD;
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(50, 56, 64);
        v.widgets.hovered.bg_fill = Color32::from_rgb(50, 56, 64);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        v.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.5);
        v.widgets.active.bg_fill = ACCENT.gamma_multiply(0.5);
    });
}

/// Icon followed by a label, in a single string (the icon font is a fallback of the text font).
pub fn label(icon: MaterialIcon, text: &str) -> String {
    format!("{}  {text}", icon.codepoint)
}

pub fn icon(icon: MaterialIcon, size: f32, color: Color32) -> RichText {
    RichText::new(icon.codepoint).size(size).color(color)
}

pub fn severity_icon(s: Severity) -> (MaterialIcon, Color32) {
    match s {
        Severity::Info => (icons::ICON_INFO, WEAK),
        Severity::Warning => (icons::ICON_WARNING, WARN),
        Severity::Error => (icons::ICON_ERROR, ERR),
    }
}

/// A line with a coloured icon followed by wrapping text.
pub fn icon_line(ui: &mut Ui, ic: MaterialIcon, color: Color32, text: impl Into<String>) {
    ui.horizontal_top(|ui| {
        ui.label(icon(ic, 16.0, color));
        ui.add(egui::Label::new(RichText::new(text.into()).color(color)).wrap());
    });
}

/// Rounded section of the side panel with a numbered title.
pub fn card<R>(ui: &mut Ui, step: u8, ic: MaterialIcon, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(ACCENT.gamma_multiply(0.18))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(7, 1))
                    .show(ui, |ui| {
                        ui.label(RichText::new(step.to_string()).color(ACCENT).strong());
                    });
                ui.label(icon(ic, 17.0, ACCENT));
                ui.label(RichText::new(title).strong().size(15.0));
            });
            ui.add_space(6.0);
            add(ui)
        })
        .inner
}

pub fn primary_button(ui: &mut Ui, enabled: bool, ic: MaterialIcon, text: &str) -> Response {
    let fill = if enabled { ACCENT } else { FIELD };
    let color = if enabled { ACCENT_TEXT } else { WEAK };
    let b = egui::Button::new(RichText::new(label(ic, text)).color(color).strong().size(15.0))
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(ui.available_width(), 36.0));
    ui.add_enabled(enabled, b)
}

pub fn secondary_button(ui: &mut Ui, enabled: bool, ic: MaterialIcon, text: &str, width: f32) -> Response {
    let b = egui::Button::new(RichText::new(label(ic, text)).size(14.5))
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(width, 32.0));
    ui.add_enabled(enabled, b)
}

/// Key figure tile.
pub fn stat(ui: &mut Ui, ic: MaterialIcon, title: &str, value: String, color: Color32) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(150.0, 44.0));
            ui.set_max_height(44.0);
            ui.horizontal_centered(|ui| {
                ui.label(icon(ic, 26.0, color));
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).small().color(WEAK));
                    ui.label(RichText::new(value).size(19.0).strong().color(TEXT));
                });
            });
        });
}

/// Centered placeholder for empty views.
pub fn empty_state(ui: &mut Ui, ic: MaterialIcon, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 70.0).max(20.0));
        ui.label(icon(ic, 56.0, BORDER));
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(17.0).strong());
        ui.label(RichText::new(hint).color(WEAK));
    });
}

/// Read-only path field with a browse button.
pub fn path_field(ui: &mut Ui, path: &mut Option<std::path::PathBuf>, title: &str) {
    ui.horizontal(|ui| {
        let browse = egui::Button::new(icon(icons::ICON_FOLDER_OPEN, 18.0, ACCENT))
            .min_size(Vec2::new(34.0, 30.0))
            .corner_radius(CornerRadius::same(6));
        let field_width = ui.available_width() - 42.0;
        egui::Frame::new()
            .fill(BG)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(field_width - 18.0);
                let (text, color) = match path {
                    Some(p) => (p.display().to_string(), TEXT),
                    None => (optistream_i18n::t("app.path.none").to_owned(), WEAK),
                };
                ui.add(egui::Label::new(RichText::new(&text).color(color).small()).truncate())
                    .on_hover_text(text);
            });
        if ui.add(browse).on_hover_text(title).clicked() {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(p) = path.as_ref() {
                dialog = dialog.set_directory(p);
            }
            if let Some(p) = dialog.pick_folder() {
                *path = Some(p);
            }
        }
    });
}
