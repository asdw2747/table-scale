//! TableScale's visual language: charcoal surfaces, brass accents, and a
//! muted teal used for the map/page grid.

use std::collections::BTreeMap;

use egui::{Color32, CornerRadius, FontFamily, FontId, Frame, RichText, Stroke, TextStyle, Ui};

pub const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x19);
pub const SURFACE: Color32 = Color32::from_rgb(0x1b, 0x1e, 0x26);
pub const SURFACE_ALT: Color32 = Color32::from_rgb(0x20, 0x24, 0x2e);
pub const SURFACE_HIGH: Color32 = Color32::from_rgb(0x27, 0x2c, 0x38);
pub const BORDER: Color32 = Color32::from_rgb(0x32, 0x38, 0x45);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(0x29, 0x2e, 0x39);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xe6, 0xe0);
pub const MUTED: Color32 = Color32::from_rgb(0x9b, 0x9d, 0xaa);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x8e, 0x9b);
pub const ACCENT: Color32 = Color32::from_rgb(0xe0, 0xa4, 0x58);
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(0xf0, 0xbd, 0x78);
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x1b, 0x12, 0x07);
pub const TEAL: Color32 = Color32::from_rgb(0x6f, 0xb3, 0xb8);
pub const TEAL_SOFT: Color32 = Color32::from_rgba_premultiplied(0x28, 0x50, 0x54, 0x80);
pub const SUCCESS: Color32 = Color32::from_rgb(0x86, 0xc7, 0x8f);
pub const ERROR: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x75);
pub const WARNING: Color32 = Color32::from_rgb(0xe2, 0xb3, 0x5c);
pub const PREVIEW_BG: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x12);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        let mut visuals = egui::Visuals::dark();
        visuals.dark_mode = true;
        visuals.override_text_color = Some(TEXT);
        visuals.weak_text_color = Some(MUTED);
        visuals.panel_fill = BG;
        visuals.window_fill = SURFACE_ALT;
        visuals.extreme_bg_color = Color32::from_rgb(0x10, 0x12, 0x17);
        visuals.faint_bg_color = SURFACE;
        visuals.code_bg_color = SURFACE_ALT;
        visuals.window_corner_radius = CornerRadius::same(12);
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.warn_fg_color = WARNING;
        visuals.error_fg_color = ERROR;
        visuals.hyperlink_color = ACCENT;
        visuals.selection = egui::style::Selection {
            bg_fill: ACCENT,
            stroke: Stroke::new(1.0, ACCENT_INK),
        };
        visuals.slider_trailing_fill = true;
        visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
        visuals.button_frame = true;
        visuals.striped = false;

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = SURFACE;
        widgets.noninteractive.weak_bg_fill = SURFACE;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_SOFT);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.noninteractive.corner_radius = CornerRadius::same(8);

        widgets.inactive.bg_fill = SURFACE_ALT;
        widgets.inactive.weak_bg_fill = SURFACE_ALT;
        widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.inactive.corner_radius = CornerRadius::same(8);

        widgets.hovered.bg_fill = SURFACE_HIGH;
        widgets.hovered.weak_bg_fill = SURFACE_HIGH;
        widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        widgets.hovered.fg_stroke = Stroke::new(1.0, ACCENT_BRIGHT);
        widgets.hovered.corner_radius = CornerRadius::same(8);

        widgets.active.bg_fill = Color32::from_rgb(0x33, 0x2b, 0x1f);
        widgets.active.weak_bg_fill = Color32::from_rgb(0x33, 0x2b, 0x1f);
        widgets.active.bg_stroke = Stroke::new(1.0, ACCENT_BRIGHT);
        widgets.active.fg_stroke = Stroke::new(1.0, ACCENT_BRIGHT);
        widgets.active.corner_radius = CornerRadius::same(8);

        widgets.open.bg_fill = SURFACE_HIGH;
        widgets.open.weak_bg_fill = SURFACE_HIGH;
        widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
        widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.open.corner_radius = CornerRadius::same(8);

        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size = egui::vec2(40.0, 28.0);
        style.spacing.slider_width = 140.0;
        style.spacing.combo_width = 120.0;
        style.spacing.indent = 18.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.bar_inner_margin = 4.0;
        style.spacing.scroll.bar_outer_margin = 4.0;
        style.animation_time = 0.14;

        style.text_styles = BTreeMap::from([
            (
                TextStyle::Heading,
                FontId::new(24.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]);
    });
}

pub fn card() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(16)
}

pub fn inset_card() -> Frame {
    Frame::new()
        .fill(SURFACE_ALT)
        .stroke(Stroke::new(1.0, BORDER_SOFT))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(12)
}

pub fn section_title(ui: &mut Ui, title: &str, detail: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(16.0).strong().color(TEXT));
        if let Some(detail) = detail {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(detail).size(11.5).color(MUTED));
            });
        }
    });
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}

pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    let text = text.into();
    let galley =
        ui.painter()
            .layout_no_wrap(text, FontId::new(11.5, FontFamily::Proportional), color);
    let padding = egui::vec2(8.0, 3.0);
    let size = galley.size() + padding * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 26),
    );
    ui.painter().galley(rect.min + padding, galley, color);
}
