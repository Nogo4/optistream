//! Loaded before/after previews and their textures.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, TextureHandle, Ui, Vec2};
use egui_material_icons::icons::*;
use optistream_core::{PreviewContent, PreviewImage, PreviewItem};
use optistream_i18n::t;

use crate::render::Bounds;
use crate::theme::{self, BG, BORDER, WEAK};

pub(super) struct LoadedPreview {
    pub(super) item: PreviewItem,
    pub(super) before: Option<TextureHandle>,
    pub(super) after: Option<TextureHandle>,
    /// Mesh comparison: shared framing and the view the textures were rendered for.
    pub(super) frame: Option<Bounds>,
    pub(super) rendered: Option<(f32, bool)>,
}

/// One half of a before/after comparison.
pub(super) fn side(ui: &mut Ui, title: &str, desc: &str, tex: Option<&TextureHandle>, size: Vec2, color: Color32) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong().color(color));
            ui.label(RichText::new(desc).small().color(WEAK));
        });
        match tex {
            Some(t) => {
                egui::Frame::new().fill(BG).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
                    ui.add(egui::Image::new((t.id(), size)));
                });
            }
            None => {
                egui::Frame::new()
                    .fill(BG)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.set_min_size(size.max(Vec2::new(200.0, 80.0)));
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new(theme::label(ICON_CHECK_CIRCLE, t("app.comparison.unchanged"))).color(WEAK));
                        });
                    });
            }
        }
    });
}

pub(super) fn load_image(ctx: &egui::Context, name: &str, img: &PreviewImage) -> TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

pub(super) fn load_texture_preview(ctx: &egui::Context, name: &str, img: &PreviewImage, show_alpha: bool) -> TextureHandle {
    if show_alpha {
        return load_image(ctx, name, img);
    }
    let mut opaque = img.clone();
    for px in opaque.rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    load_image(ctx, name, &opaque)
}

pub(super) fn load_preview(ctx: &egui::Context, i: usize, item: PreviewItem, show_alpha: bool) -> LoadedPreview {
    let (before, after) = match &item.content {
        PreviewContent::Texture { before, after } => (
            Some(load_texture_preview(ctx, &format!("tb{i}"), before, show_alpha)),
            after.as_ref().map(|a| load_texture_preview(ctx, &format!("ta{i}"), a, show_alpha)),
        ),
        PreviewContent::Mesh { .. } => (None, None),
    };
    LoadedPreview { item, before, after, frame: None, rendered: None }
}
