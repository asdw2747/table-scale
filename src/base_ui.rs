//! Settings sidebar and export workflow for the 3D miniature base mode.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, DragValue, Layout, RichText, ScrollArea, Stroke, TextEdit, Ui, Vec2,
};

use crate::base_preview::{self, BaseCamera};
use crate::stl_base::{
    self, BaseMesh, BaseParams, BaseShape, MagnetSpec, NameplateSpec, NameplateStyle, TopRecessSpec,
};
use crate::theme;

const SIDEBAR_WIDTH: f32 = 348.0;
const PREVIEW_CONTROLS_HEIGHT: f32 = 42.0;
const PREVIEW_STATS_HEIGHT: f32 = 84.0;
pub const SIZE_PRESETS_MM: [f64; 7] = [25.0, 32.0, 35.0, 40.0, 50.0, 65.0, 80.0];
pub const MAGNET_PRESETS_MM: [(f64, f64); 3] = [(5.0, 2.0), (6.0, 2.0), (8.0, 2.0)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseAction {
    ExportSingle,
    ExportBatch,
}

pub struct BaseUi {
    params: BaseParams,
    character_name: String,
    batch_roster: String,
    preview: Option<BaseMesh>,
    preview_error: Option<String>,
    dirty: bool,
    changed_at: Instant,
    last_generation_ms: f32,
    camera: BaseCamera,
}

impl Default for BaseUi {
    fn default() -> Self {
        let params = BaseParams::default();
        let mut this = Self {
            params,
            character_name: "Magneto".to_owned(),
            batch_roster: "Magneto\nWolverine\nStorm".to_owned(),
            preview: None,
            preview_error: None,
            dirty: true,
            changed_at: Instant::now(),
            last_generation_ms: 0.0,
            camera: BaseCamera::default(),
        };
        this.rebuild_preview();
        this
    }
}

impl BaseUi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn export_params(&self) -> BaseParams {
        self.params.clone()
    }

    pub fn character_name(&self) -> &str {
        &self.character_name
    }

    pub fn batch_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for line in self.batch_roster.lines() {
            let name = line.trim();
            if !name.is_empty() {
                names.push(name.to_owned());
            }
        }
        names
    }

    pub fn can_export(&self) -> bool {
        self.params.validate().is_ok() && !self.character_name.trim().is_empty()
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<BaseAction> {
        let before = self.params.clone();
        let before_name = self.character_name.clone();
        let mut action = None;

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Resin bases, built to fit")
                        .size(20.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new(
                        "Parametric, watertight STLs with bevels, magnet pockets, and an underside nameplate.",
                    )
                    .size(12.0)
                    .color(theme::MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::pill(ui, "STL \u{00b7} MM", theme::TEAL);
                theme::pill(ui, "WATERTIGHT", theme::SUCCESS);
            });
        });
        ui.add_space(10.0);

        let full_height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(SIDEBAR_WIDTH, full_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ScrollArea::vertical()
                        .id_salt("base-settings")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            action = self.settings_ui(ui);
                            ui.add_space(16.0);
                        });
                },
            );

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            let preview_width = ui.available_width();
            ui.allocate_ui_with_layout(
                Vec2::new(preview_width, full_height),
                Layout::top_down(Align::Min),
                |ui| {
                    self.preview_ui(ui);
                },
            );
        });

        if self.character_name != before_name {
            if let Some(nameplate) = &mut self.params.nameplate {
                if nameplate.text == before_name {
                    nameplate.text = self.character_name.clone();
                }
            }
        }
        if self.params != before || self.character_name != before_name {
            self.dirty = true;
            self.changed_at = Instant::now();
        }
        self.update_preview(ui.ctx());

        action
    }

    fn settings_ui(&mut self, ui: &mut Ui) -> Option<BaseAction> {
        let mut action = None;

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Shape & size", Some("MM"));
            theme::hint(ui, "Pick a common base size or dial in exact millimetres.");
            ui.add_space(8.0);

            let previous_shape = self.params.shape;
            ui.horizontal_wrapped(|ui| {
                for shape in [BaseShape::Circle, BaseShape::Pill, BaseShape::RoundedSquare] {
                    let label = match shape {
                        BaseShape::Circle => "Circle",
                        BaseShape::Pill => "Pill / Oval",
                        BaseShape::RoundedSquare => "Rounded",
                    };
                    if choice_pill(ui, self.params.shape == shape, label) {
                        self.params.shape = shape;
                        if shape == BaseShape::Pill
                            && previous_shape == BaseShape::Circle
                            && (self.params.width_mm - self.params.depth_mm).abs() < 0.01
                        {
                            self.params.depth_mm =
                                (self.params.width_mm * 0.5).clamp(5.0, self.params.width_mm);
                        }
                        if shape == BaseShape::RoundedSquare && self.params.corner_radius_mm < 0.2 {
                            self.params.corner_radius_mm = 5.0;
                        }
                    }
                }
            });
            ui.add_space(8.0);

            ui.label(RichText::new("Presets").size(11.0).color(theme::MUTED));
            ui.horizontal_wrapped(|ui| {
                for size in SIZE_PRESETS_MM {
                    if choice_pill(
                        ui,
                        self.size_preset_selected(size),
                        &format!("{size:.0} mm"),
                    ) {
                        self.apply_size_preset(size);
                    }
                }
            });
            ui.add_space(6.0);

            if self.params.shape == BaseShape::Circle {
                dimension_row(ui, "Diameter", &mut self.params.width_mm, 5.0..=200.0);
                self.params.depth_mm = self.params.width_mm;
            } else {
                dimension_row(ui, "Width", &mut self.params.width_mm, 5.0..=200.0);
                dimension_row(ui, "Depth", &mut self.params.depth_mm, 5.0..=200.0);
            }
            if self.params.shape == BaseShape::RoundedSquare {
                dimension_row(
                    ui,
                    "Corner radius",
                    &mut self.params.corner_radius_mm,
                    0.2..=90.0,
                );
            }
            dimension_row(ui, "Height", &mut self.params.height_mm, 1.0..=50.0);
            dimension_row(
                ui,
                "Bevel width",
                &mut self.params.bevel_width_mm,
                0.0..=40.0,
            );
            angle_row(ui, "Bevel angle", &mut self.params.bevel_angle_deg);
            let bevel_height = self.params.bevel_height_mm();
            ui.label(
                RichText::new(format!(
                    "Bevel rises {bevel_height:.2} mm from the vertical wall."
                ))
                .size(10.5)
                .color(theme::DIM),
            );
        });

        ui.add_space(10.0);
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Top lip", Some("OPTIONAL"));
            let mut enabled = self.params.top_recess.is_some();
            if ui
                .checkbox(&mut enabled, "Recess the top surface")
                .changed()
            {
                self.params.top_recess = enabled.then_some(TopRecessSpec {
                    rim_width_mm: 1.2,
                    depth_mm: 0.6,
                });
            }
            if let Some(recess) = &mut self.params.top_recess {
                dimension_row(ui, "Rim width", &mut recess.rim_width_mm, 0.2..=20.0);
                dimension_row(ui, "Lip depth", &mut recess.depth_mm, 0.05..=20.0);
                theme::hint(
                    ui,
                    "A raised rim keeps the miniature centred while the recess saves material.",
                );
            } else {
                theme::hint(ui, "Off: the top is a flat, bevelled plate.");
            }
        });

        ui.add_space(10.0);
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Magnet pocket", Some("UNDERSIDE"));
            let mut enabled = self.params.magnet.is_some();
            if ui.checkbox(&mut enabled, "Add a magnet pocket").changed() {
                self.params.magnet = enabled.then_some(MagnetSpec {
                    diameter_mm: 5.0,
                    depth_mm: 2.0,
                    radial_tolerance_mm: 0.15,
                });
            }
            if let Some(magnet) = &mut self.params.magnet {
                ui.horizontal_wrapped(|ui| {
                    for (diameter, depth) in MAGNET_PRESETS_MM {
                        let selected = (magnet.diameter_mm - diameter).abs() < 0.01
                            && (magnet.depth_mm - depth).abs() < 0.01;
                        if choice_pill(ui, selected, &format!("{diameter:.0}\u{00d7}{depth:.0}")) {
                            magnet.diameter_mm = diameter;
                            magnet.depth_mm = depth;
                        }
                    }
                });
                dimension_row(ui, "Diameter", &mut magnet.diameter_mm, 1.0..=40.0);
                dimension_row(ui, "Depth", &mut magnet.depth_mm, 0.5..=20.0);
                dimension_row_with_decimals(
                    ui,
                    "Radial tolerance",
                    &mut magnet.radial_tolerance_mm,
                    0.0..=1.0,
                    2,
                );
                ui.label(
                    RichText::new(format!(
                        "Effective pocket \u{00d8} {:.2} mm",
                        magnet.effective_radius_mm() * 2.0
                    ))
                    .size(10.5)
                    .color(theme::DIM),
                );
            } else {
                theme::hint(ui, "Off: the underside is a clean, flat face.");
            }
        });

        ui.add_space(10.0);
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Nameplate", Some("UNDERSIDE"));
            let mut enabled = self.params.nameplate.is_some();
            if ui.checkbox(&mut enabled, "Engrave a name").changed() {
                self.params.nameplate = enabled.then_some(NameplateSpec {
                    text: self.character_name.clone(),
                    style: NameplateStyle::Engrave,
                    depth_mm: 0.4,
                });
            }
            if let Some(nameplate) = &mut self.params.nameplate {
                ui.label(RichText::new("Text").size(11.0).color(theme::MUTED));
                ui.add(
                    TextEdit::singleline(&mut nameplate.text)
                        .char_limit(32)
                        .hint_text("Character name"),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("nameplate-style")
                        .selected_text(nameplate.style.label())
                        .width(132.0)
                        .show_ui(ui, |ui| {
                            for style in [NameplateStyle::Engrave, NameplateStyle::Emboss] {
                                ui.selectable_value(&mut nameplate.style, style, style.label());
                            }
                        });
                    ui.add(
                        DragValue::new(&mut nameplate.depth_mm)
                            .range(0.1..=1.2)
                            .speed(0.02)
                            .fixed_decimals(2)
                            .suffix(" mm"),
                    );
                });
                if let Some(features) = self.preview.as_ref().map(|mesh| &mesh.features) {
                    if let Some(info) = &features.nameplate {
                        ui.label(
                            RichText::new(format!(
                                "{} \u{00b7} {:.1} mm letters",
                                info.placement.label(),
                                info.font_size_mm
                            ))
                            .size(10.5)
                            .color(theme::TEAL),
                        );
                    }
                }
                theme::hint(
                    ui,
                    "Underside text is mirrored so it reads when the base is turned over.",
                );
            } else {
                theme::hint(ui, "Off: no lettering or extra geometry on the underside.");
            }
        });

        ui.add_space(10.0);
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Export", Some("STL"));
            let name_before = self.character_name.clone();
            ui.label(
                RichText::new("Character / file name")
                    .size(11.0)
                    .color(theme::MUTED),
            );
            ui.add(
                TextEdit::singleline(&mut self.character_name)
                    .char_limit(64)
                    .hint_text("Miniature name"),
            );
            if self.character_name != name_before
                && self.character_name.trim() != self.character_name
            {
                self.character_name = self.character_name.trim().to_owned();
            }
            let filename = if self.character_name.trim().is_empty() {
                "name_25mm.stl".to_owned()
            } else {
                self.params.file_name(&self.character_name)
            };
            ui.label(
                RichText::new(format!("File: {filename}"))
                    .size(10.5)
                    .color(theme::DIM),
            );
            ui.add_space(6.0);

            if let Err(error) = self.params.validate() {
                ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
            } else if self.character_name.trim().is_empty() {
                ui.label(
                    RichText::new("Enter a file name before exporting.")
                        .size(11.0)
                        .color(theme::ERROR),
                );
            }

            let ready = self.can_export();
            let export_button = egui::Button::new(
                RichText::new("Export STL")
                    .strong()
                    .color(theme::ACCENT_INK),
            )
            .fill(theme::ACCENT)
            .stroke(Stroke::new(1.0, theme::ACCENT_BRIGHT))
            .min_size(Vec2::new(ui.available_width(), 34.0));
            if ui.add_enabled(ready, export_button).clicked() {
                action = Some(BaseAction::ExportSingle);
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                RichText::new("Roster batch")
                    .size(12.0)
                    .strong()
                    .color(theme::TEXT),
            );
            theme::hint(
                ui,
                "One character name per line. Each name gets its own STL.",
            );
            ui.add(
                TextEdit::multiline(&mut self.batch_roster)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("Magneto\nWolverine\nStorm"),
            );
            let names = self.batch_names();
            let valid = names
                .iter()
                .all(|name| self.params.with_name(name).validate().is_ok());
            ui.label(
                RichText::new(format!(
                    "{} name{} \u{00b7} {}",
                    names.len(),
                    if names.len() == 1 { "" } else { "s" },
                    if valid && !names.is_empty() {
                        "ready"
                    } else if names.is_empty() {
                        "add names to export"
                    } else {
                        "fix invalid names"
                    }
                ))
                .size(10.5)
                .color(if valid && !names.is_empty() {
                    theme::SUCCESS
                } else {
                    theme::DIM
                }),
            );
            if ui
                .add_enabled(
                    valid && !names.is_empty(),
                    egui::Button::new("Export roster STLs")
                        .min_size(Vec2::new(ui.available_width(), 32.0)),
                )
                .clicked()
            {
                action = Some(BaseAction::ExportBatch);
            }
        });

        ui.add_space(10.0);
        theme::hint(
            ui,
            "Every export is re-validated for watertight, manifold geometry before it is written.",
        );

        action
    }

    fn preview_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if choice_pill(ui, !self.camera.wireframe, "Solid") {
                self.camera.wireframe = false;
            }
            if choice_pill(ui, self.camera.wireframe, "Wireframe") {
                self.camera.wireframe = true;
            }
            ui.add_space(8.0);
            if small_button(ui, "Iso") {
                self.camera.reset_iso();
            }
            if small_button(ui, "Top") {
                self.camera.reset_top();
            }
            if small_button(ui, "Bottom") {
                self.camera.reset_bottom();
            }
            if small_button(ui, "Front") {
                self.camera.reset_front();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(error) = &self.preview_error {
                    theme::pill(ui, "INVALID", theme::ERROR);
                    ui.label(RichText::new(error).size(10.5).color(theme::ERROR));
                } else if let Some(mesh) = &self.preview {
                    theme::pill(ui, "WATERTIGHT", theme::SUCCESS);
                    ui.label(
                        RichText::new(format!("{} tris", mesh.triangle_count()))
                            .size(10.5)
                            .color(theme::DIM),
                    );
                } else {
                    ui.spinner();
                }
            });
        });
        ui.add_space(PREVIEW_CONTROLS_HEIGHT - 34.0);

        let stats_height = PREVIEW_STATS_HEIGHT;
        let canvas_height = (ui.available_height() - stats_height).max(180.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), canvas_height),
            Layout::top_down(Align::Min),
            |ui| {
                base_preview::viewport(
                    ui,
                    self.preview.as_ref(),
                    self.preview_error.as_deref(),
                    &mut self.camera,
                );
            },
        );
        ui.add_space(8.0);
        self.stats_ui(ui);
    }

    fn stats_ui(&mut self, ui: &mut Ui) {
        theme::inset_card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(mesh) = &self.preview {
                    let volume_cm3 = mesh.volume_mm3() / 1000.0;
                    stat(ui, "VOLUME", format!("{volume_cm3:.2} cm\u{00b3}"));
                    ui.add_space(18.0);
                    stat(ui, "RESIN EST.", format!("{:.1} g", volume_cm3 * 1.1));
                    ui.add_space(18.0);
                    stat(
                        ui,
                        "SHELL",
                        if mesh.watertight {
                            "Watertight".to_owned()
                        } else {
                            "Check".to_owned()
                        },
                    );
                    ui.add_space(18.0);
                    stat(
                        ui,
                        "NAME",
                        mesh.features
                            .nameplate
                            .as_ref()
                            .map(|info| info.placement.label().to_owned())
                            .unwrap_or_else(|| "Off".to_owned()),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{:.0} ms", self.last_generation_ms))
                                .size(10.0)
                                .color(theme::DIM),
                        );
                    });
                } else {
                    ui.label(
                        RichText::new("Adjust the settings to generate a base.")
                            .size(11.5)
                            .color(theme::MUTED),
                    );
                }
            });
        });
    }

    fn size_preset_selected(&self, size: f64) -> bool {
        match self.params.shape {
            BaseShape::Circle => {
                (self.params.width_mm - size).abs() < 0.01
                    && (self.params.depth_mm - size).abs() < 0.01
            }
            BaseShape::Pill => (self.params.width_mm - size).abs() < 0.01,
            BaseShape::RoundedSquare => {
                (self.params.width_mm - size).abs() < 0.01
                    && (self.params.depth_mm - size).abs() < 0.01
            }
        }
    }

    fn apply_size_preset(&mut self, size: f64) {
        match self.params.shape {
            BaseShape::Circle | BaseShape::RoundedSquare => {
                self.params.width_mm = size;
                self.params.depth_mm = size;
            }
            BaseShape::Pill => {
                let aspect =
                    (self.params.depth_mm / self.params.width_mm.max(0.001)).clamp(0.3, 1.0);
                self.params.width_mm = size;
                self.params.depth_mm = (size * aspect).clamp(5.0, size);
            }
        }
    }

    fn rebuild_preview(&mut self) {
        let started = Instant::now();
        match stl_base::generate_base(&self.params) {
            Ok(mesh) => {
                self.preview = Some(mesh);
                self.preview_error = None;
            }
            Err(error) => {
                self.preview_error = Some(error);
            }
        }
        self.last_generation_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.dirty = false;
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            return;
        }
        let pointer_down = ctx.input(|input| input.pointer.any_down());
        if pointer_down || self.changed_at.elapsed() < Duration::from_millis(140) {
            ctx.request_repaint_after(Duration::from_millis(90));
            return;
        }
        self.rebuild_preview();
        ctx.request_repaint();
    }
}

fn dimension_row(ui: &mut Ui, label: &str, value: &mut f64, range: std::ops::RangeInclusive<f64>) {
    dimension_row_with_decimals(ui, label, value, range, 1);
}

fn dimension_row_with_decimals(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    decimals: usize,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.5).color(theme::MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                DragValue::new(value)
                    .range(range)
                    .speed(if decimals > 1 { 0.01 } else { 0.1 })
                    .fixed_decimals(decimals)
                    .suffix(" mm"),
            );
        });
    });
}

fn angle_row(ui: &mut Ui, label: &str, value: &mut f64) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.5).color(theme::MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                DragValue::new(value)
                    .range(1.0..=80.0)
                    .speed(0.25)
                    .fixed_decimals(0)
                    .suffix("\u{00b0}"),
            );
        });
    });
}

fn choice_pill(ui: &mut Ui, selected: bool, label: &str) -> bool {
    let fill = if selected {
        Color32::from_rgb(0x33, 0x2a, 0x1d)
    } else {
        theme::SURFACE_ALT
    };
    let stroke = if selected {
        Stroke::new(1.0, theme::ACCENT)
    } else {
        Stroke::new(1.0, theme::BORDER_SOFT)
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(11.5).color(if selected {
            theme::ACCENT_BRIGHT
        } else {
            theme::MUTED
        }))
        .fill(fill)
        .stroke(stroke)
        .min_size(Vec2::new(0.0, 26.0)),
    )
    .clicked()
}

fn small_button(ui: &mut Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).color(theme::MUTED))
            .frame(false)
            .min_size(Vec2::new(0.0, 24.0)),
    )
    .clicked()
}

fn stat(ui: &mut Ui, label: &str, value: String) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(9.5).color(theme::DIM));
        ui.label(RichText::new(value).size(13.0).strong().color(theme::TEXT));
    });
}
