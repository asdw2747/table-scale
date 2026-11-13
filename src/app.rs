//! TableScale application shell and both tab UIs.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Label,
    Layout, Margin, ProgressBar, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use image::DynamicImage;

use crate::export::{self, ExportJob};
use crate::scale::{self, SCALE_PRESETS};
use crate::theme;
use crate::tiler::{Orientation, SizeMode, SquareUnit, TileLayout, TilerSettings, PAPER_PRESETS};

const SETTINGS_WIDTH: f32 = 342.0;
const PREVIEW_MIN_HEIGHT: f32 = 180.0;
const MAX_IMAGE_PIXELS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Scaler,
    Tiler,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeightMode {
    RealWorld,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TilerAction {
    BrowseMap,
    ExportPdf,
    ExportPng,
}

#[derive(Clone)]
struct MapInfoView {
    texture: TextureHandle,
    name: String,
    width: u32,
    height: u32,
}

struct MapDocument {
    path: PathBuf,
    name: String,
    stem: String,
    source: Arc<DynamicImage>,
    texture: TextureHandle,
    width: u32,
    height: u32,
}

impl MapDocument {
    fn view(&self) -> MapInfoView {
        MapInfoView {
            texture: self.texture.clone(),
            name: self.name.clone(),
            width: self.width,
            height: self.height,
        }
    }
}

struct LoadedMap {
    path: PathBuf,
    name: String,
    stem: String,
    source: DynamicImage,
    preview: image::RgbaImage,
}

struct MapLoadJob {
    receiver: Receiver<Result<LoadedMap, String>>,
    name: String,
}

impl MapLoadJob {
    fn poll(&self) -> Option<Result<LoadedMap, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("The map loader stopped unexpectedly.".to_owned()))
            }
        }
    }
}

#[derive(Default)]
struct PreviewState {
    zoom: f32,
    pan: Vec2,
    hovered_page: Option<usize>,
}

impl PreviewState {
    fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.hovered_page = None;
    }
}

struct Toast {
    text: String,
    error: bool,
    created: Instant,
    ttl: Duration,
}

impl Toast {
    fn active(&self) -> bool {
        self.created.elapsed() < self.ttl
    }
}

pub struct TableScaleApp {
    tab: Tab,
    source_idx: usize,
    target_idx: usize,
    custom_source_mm: f64,
    custom_target_mm: f64,
    height_mode: HeightMode,
    subject_feet: f64,
    subject_inches: f64,
    current_height_mm: f64,
    tiler: TilerSettings,
    map: Option<MapDocument>,
    map_loading: Option<MapLoadJob>,
    map_error: Option<String>,
    preview: PreviewState,
    include_guides: bool,
    allow_stretched_export: bool,
    export_job: Option<ExportJob>,
    toast: Option<Toast>,
}

impl TableScaleApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        Self {
            tab: Tab::Scaler,
            source_idx: 0,
            target_idx: 2,
            custom_source_mm: 28.0,
            custom_target_mm: 35.0,
            height_mode: HeightMode::RealWorld,
            subject_feet: 6.0,
            subject_inches: 0.0,
            current_height_mm: 28.0,
            tiler: TilerSettings::default(),
            map: None,
            map_loading: None,
            map_error: None,
            preview: PreviewState {
                zoom: 1.0,
                ..Default::default()
            },
            include_guides: true,
            allow_stretched_export: false,
            export_job: None,
            toast: None,
        }
    }

    fn source_scale_mm(&self) -> f64 {
        if self.source_idx < SCALE_PRESETS.len() {
            SCALE_PRESETS[self.source_idx].nominal_height_mm()
        } else {
            self.custom_source_mm
        }
    }

    fn target_scale_mm(&self) -> f64 {
        if self.target_idx < SCALE_PRESETS.len() {
            SCALE_PRESETS[self.target_idx].nominal_height_mm()
        } else {
            self.custom_target_mm
        }
    }

    fn source_is_ratio(&self) -> bool {
        self.source_idx < SCALE_PRESETS.len() && SCALE_PRESETS[self.source_idx].is_ratio()
    }

    fn target_is_ratio(&self) -> bool {
        self.target_idx < SCALE_PRESETS.len() && SCALE_PRESETS[self.target_idx].is_ratio()
    }

    fn select_label(&self, idx: usize) -> String {
        if idx < SCALE_PRESETS.len() {
            SCALE_PRESETS[idx].label().to_owned()
        } else {
            "Custom mm".to_owned()
        }
    }

    fn set_toast(&mut self, text: impl Into<String>, error: bool, ttl_secs: f32) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
            created: Instant::now(),
            ttl: Duration::from_secs_f32(ttl_secs),
        });
    }

    fn expire_toast(&mut self) {
        if let Some(toast) = &self.toast {
            if !toast.active() {
                self.toast = None;
            }
        }
    }

    fn poll_export(&mut self) {
        let result = self.export_job.as_ref().and_then(|job| job.poll());
        if let Some(result) = result {
            self.export_job = None;
            match result {
                Ok(summary) => {
                    self.set_toast(
                        format!(
                            "{}  \u{00b7}  {}",
                            summary.headline(),
                            summary.output.display()
                        ),
                        false,
                        9.0,
                    );
                }
                Err(error) => {
                    self.set_toast(error, true, 10.0);
                }
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let mut paths: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if paths.is_empty() {
            return;
        }
        if let Some(position) = paths.iter().position(|path| is_supported_image(path)) {
            let path = paths.swap_remove(position);
            self.tab = Tab::Tiler;
            self.load_map(ctx, &path);
        } else {
            self.set_toast("Drop a PNG, JPG, or WebP battle map.", true, 4.0);
        }
    }

    fn load_map(&mut self, ctx: &egui::Context, path: &Path) {
        if self.map_loading.is_some() {
            self.set_toast("Wait for the current map to finish loading.", true, 3.0);
            return;
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("battle-map")
            .to_owned();
        self.map = None;
        self.map_error = None;
        self.allow_stretched_export = false;
        self.preview.reset();
        self.tab = Tab::Tiler;

        let path_owned = path.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = load_map_worker(&path_owned);
            let _ = sender.send(result);
        });
        self.map_loading = Some(MapLoadJob { receiver, name });
        ctx.request_repaint();
    }

    fn poll_map_load(&mut self, ctx: &egui::Context) {
        let result = self.map_loading.as_ref().and_then(|job| job.poll());
        let Some(result) = result else {
            return;
        };
        self.map_loading.take();
        match result {
            Ok(loaded) => {
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [
                        loaded.preview.width() as usize,
                        loaded.preview.height() as usize,
                    ],
                    loaded.preview.as_raw(),
                );
                let texture = ctx.load_texture(
                    format!("battle-map-preview-{}", loaded.path.display()),
                    color_image,
                    TextureOptions::LINEAR,
                );
                let name = loaded.name.clone();
                self.map = Some(MapDocument {
                    path: loaded.path,
                    name: name.clone(),
                    stem: loaded.stem,
                    width: loaded.source.width(),
                    height: loaded.source.height(),
                    source: Arc::new(loaded.source),
                    texture,
                });
                self.map_error = None;
                self.allow_stretched_export = false;
                self.preview.reset();
                self.set_toast(format!("Loaded {name}"), false, 3.0);
            }
            Err(error) => {
                self.map_error = Some(error);
                self.set_toast("Could not load that battle map.", true, 5.0);
            }
        }
        ctx.request_repaint();
    }

    fn choose_map_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Battle map", &["png", "jpg", "jpeg", "webp"])
            .set_title("Open a battle map");
        if let Some(map) = &self.map {
            if let Some(parent) = map.path.parent() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(path) = dialog.pick_file() {
            self.tab = Tab::Tiler;
            self.load_map(ctx, &path);
        }
    }

    fn choose_pdf_export(&mut self, ctx: &egui::Context) {
        if self.export_job.is_some() {
            return;
        }
        let Some(map) = &self.map else {
            self.set_toast("Load a battle map before exporting.", true, 4.0);
            return;
        };
        let layout = match self.tiler.layout() {
            Ok(layout) => layout,
            Err(error) => {
                self.set_toast(error, true, 5.0);
                return;
            }
        };
        let default_name = format!("{}-table-scale.pdf", export::sanitize_stem(&map.stem));
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_title("Export 1:1 battle map PDF")
            .set_file_name(default_name);
        if let Some(parent) = map.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("pdf"))
        {
            path.set_extension("pdf");
        }
        let source = Arc::clone(&map.source);
        self.export_job = Some(export::start_pdf_export(
            path,
            layout,
            source,
            self.include_guides,
        ));
        self.toast = None;
        ctx.request_repaint();
    }

    fn choose_png_export(&mut self, ctx: &egui::Context) {
        if self.export_job.is_some() {
            return;
        }
        let Some(map) = &self.map else {
            self.set_toast("Load a battle map before exporting.", true, 4.0);
            return;
        };
        let layout = match self.tiler.layout() {
            Ok(layout) => layout,
            Err(error) => {
                self.set_toast(error, true, 5.0);
                return;
            }
        };
        let mut dialog = rfd::FileDialog::new().set_title("Choose a folder for PNG tiles");
        if let Some(parent) = map.path.parent() {
            dialog = dialog.set_directory(parent);
        }
        let Some(folder) = dialog.pick_folder() else {
            return;
        };
        let source = Arc::clone(&map.source);
        let stem = map.stem.clone();
        self.export_job = Some(export::start_png_export(folder, layout, source, stem));
        self.toast = None;
        ctx.request_repaint();
    }

    fn aspect_mismatch(&self) -> Option<(f64, f64)> {
        let map = self.map.as_ref()?;
        let (map_w, map_h) = self.tiler.map_dimensions_in();
        if map_w <= 0.0 || map_h <= 0.0 {
            return None;
        }
        let source_aspect = map.width as f64 / map.height.max(1) as f64;
        let target_aspect = map_w / map_h;
        if (source_aspect / target_aspect - 1.0).abs() > 0.01 {
            Some((source_aspect, target_aspect))
        } else {
            None
        }
    }

    fn handle_tiler_action(&mut self, ctx: &egui::Context, action: TilerAction) {
        match action {
            TilerAction::BrowseMap => self.choose_map_dialog(ctx),
            TilerAction::ExportPdf => self.choose_pdf_export(ctx),
            TilerAction::ExportPng => self.choose_png_export(ctx),
        }
    }

    fn header_ui(&mut self, ui: &mut Ui) {
        egui::Panel::top("tablescale-header")
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin {
                left: 18,
                right: 18,
                top: 14,
                bottom: 10,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (mark_rect, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
                    ui.painter()
                        .rect_filled(mark_rect, CornerRadius::same(10), theme::ACCENT);
                    ui.painter().text(
                        mark_rect.center(),
                        Align2::CENTER_CENTER,
                        "TS",
                        FontId::new(13.0, FontFamily::Proportional),
                        theme::ACCENT_INK,
                    );
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("TableScale")
                                .size(23.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new("3D miniature scale converter & battle map print tiler")
                                .size(11.5)
                                .color(theme::MUTED),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        theme::pill(ui, format!("v{}", env!("CARGO_PKG_VERSION")), theme::MUTED);
                        theme::pill(ui, "MIT", theme::MUTED);
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("PRINT-READY \u{00b7} 1:1")
                                .size(10.0)
                                .color(theme::TEAL),
                        );
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if tab_button(ui, "Miniature Scaler", self.tab == Tab::Scaler) {
                        self.tab = Tab::Scaler;
                    }
                    if tab_button(ui, "Battle Map Tiler", self.tab == Tab::Tiler) {
                        self.tab = Tab::Tiler;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Drop a map anywhere in the Tiler tab")
                                .size(11.0)
                                .color(theme::DIM),
                        );
                    });
                });
            });
    }

    fn status_ui(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("tablescale-status")
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin {
                left: 18,
                right: 18,
                top: 5,
                bottom: 6,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("TableScale {}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(theme::DIM),
                    );
                    ui.label(RichText::new("\u{00b7}").size(11.0).color(theme::DIM));
                    ui.label(
                        RichText::new("MIT licensed \u{00b7} built with egui")
                            .size(11.0)
                            .color(theme::DIM),
                    );
                    if let Some(job) = &self.export_job {
                        ui.add_space(10.0);
                        ui.spinner();
                        ui.add(
                            ProgressBar::new(job.progress())
                                .desired_width(110.0)
                                .show_percentage(),
                        );
                        ui.label(
                            RichText::new(&job.description)
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(toast) = &self.toast {
                            let color = if toast.error {
                                theme::ERROR
                            } else {
                                theme::SUCCESS
                            };
                            ui.add(
                                Label::new(RichText::new(&toast.text).size(11.0).color(color))
                                    .truncate(),
                            )
                            .on_hover_text(&toast.text);
                        } else {
                            ui.label(
                                RichText::new("Ready \u{00b7} files never leave your machine")
                                    .size(11.0)
                                    .color(theme::DIM),
                            );
                        }
                    });
                });
            });
    }

    fn scaler_ui(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Scale without guessing")
                        .size(20.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new(
                        "Convert between common tabletop scales or enter exact millimetres.",
                    )
                    .size(12.0)
                    .color(theme::MUTED),
                );
            });
        });
        ui.add_space(10.0);

        ScrollArea::vertical()
            .id_salt("scaler-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |columns| {
                    self.scale_calculator_card(&mut columns[0]);
                    self.target_height_card(&mut columns[1]);
                });
                ui.add_space(12.0);
                self.base_reference_card(ui);
                ui.add_space(20.0);
            });
    }

    fn scale_calculator_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Scale percentage", Some("Slicer input"));
            theme::hint(
                ui,
                "Pick a source and target, or switch either side to custom millimetres.",
            );
            ui.add_space(10.0);

            ui.label(RichText::new("Source scale").size(11.5).color(theme::MUTED));
            egui::ComboBox::from_id_salt("source-scale")
                .selected_text(self.select_label(self.source_idx))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (index, preset) in SCALE_PRESETS.iter().enumerate() {
                        ui.selectable_value(&mut self.source_idx, index, preset.label());
                    }
                    ui.selectable_value(
                        &mut self.source_idx,
                        SCALE_PRESETS.len(),
                        "Custom mm",
                    );
                });
            if self.source_idx >= SCALE_PRESETS.len() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Source mm").size(11.5).color(theme::MUTED));
                    ui.add(
                        egui::DragValue::new(&mut self.custom_source_mm)
                            .range(1.0..=1000.0)
                            .speed(0.25)
                            .fixed_decimals(2)
                            .suffix(" mm"),
                    );
                });
            }
            ui.add_space(8.0);

            ui.label(RichText::new("Target scale").size(11.5).color(theme::MUTED));
            egui::ComboBox::from_id_salt("target-scale")
                .selected_text(self.select_label(self.target_idx))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (index, preset) in SCALE_PRESETS.iter().enumerate() {
                        ui.selectable_value(&mut self.target_idx, index, preset.label());
                    }
                    ui.selectable_value(
                        &mut self.target_idx,
                        SCALE_PRESETS.len(),
                        "Custom mm",
                    );
                });
            if self.target_idx >= SCALE_PRESETS.len() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Target mm").size(11.5).color(theme::MUTED));
                    ui.add(
                        egui::DragValue::new(&mut self.custom_target_mm)
                            .range(1.0..=1000.0)
                            .speed(0.25)
                            .fixed_decimals(2)
                            .suffix(" mm"),
                    );
                });
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Swap scales").clicked() {
                    std::mem::swap(&mut self.source_idx, &mut self.target_idx);
                }
                if self.source_idx >= SCALE_PRESETS.len()
                    && self.target_idx >= SCALE_PRESETS.len()
                {
                    ui.label(
                        RichText::new("Both sides use custom millimetres.")
                            .size(11.0)
                            .color(theme::DIM),
                    );
                }
            });

            ui.add_space(12.0);
            let source_mm = self.source_scale_mm();
            let target_mm = self.target_scale_mm();
            let percent = scale::scale_percent(source_mm, target_mm);

            Frame::new()
                .fill(theme::SURFACE_ALT)
                .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("SLICER SCALE")
                                    .size(10.0)
                                    .color(theme::MUTED),
                            );
                            if let Some(percent) = percent {
                                ui.label(
                                    RichText::new(format!("{percent:.2}%"))
                                        .size(36.0)
                                        .strong()
                                        .color(theme::ACCENT_BRIGHT),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Invalid")
                                        .size(28.0)
                                        .strong()
                                        .color(theme::ERROR),
                                );
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let copy = ui.add_enabled(
                                percent.is_some(),
                                egui::Button::new(
                                    RichText::new("Copy %").strong().color(theme::ACCENT_INK),
                                )
                                .fill(theme::ACCENT)
                                .stroke(Stroke::new(1.0, theme::ACCENT_BRIGHT))
                                .min_size(vec2(88.0, 32.0)),
                            );
                            if copy.clicked() {
                                if let Some(percent) = percent {
                                    ui.ctx().copy_text(format!("{percent:.2}%"));
                                    self.set_toast("Scale percentage copied.", false, 2.2);
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                    if let Some(percent) = percent {
                        let factor = percent / 100.0;
                        ui.label(
                            RichText::new(format!(
                                "{target_mm:.2} mm \u{00f7} {source_mm:.2} mm \u{00d7} 100  =  {percent:.2}%"
                            ))
                            .size(11.5)
                            .color(theme::MUTED),
                        );
                        ui.label(
                            RichText::new(format!("Scale factor: \u{00d7}{factor:.4}"))
                                .size(11.5)
                                .color(theme::MUTED),
                        );
                    } else {
                        ui.label(
                            RichText::new("Enter positive millimetre values.")
                                .size(12.0)
                                .color(theme::ERROR),
                        );
                    }
                });

            if self.source_is_ratio() || self.target_is_ratio() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Ratio scales use a 6 ft (1828.8 mm) reference figure for a nominal human height.",
                    )
                    .size(11.0)
                    .color(theme::DIM),
                );
            }
            ui.add_space(6.0);
            if let Some(preset) = self.selected_preset_note(self.source_idx) {
                ui.label(RichText::new(preset).size(11.0).color(theme::DIM));
            }
            if let Some(preset) = self.selected_preset_note(self.target_idx) {
                ui.label(RichText::new(preset).size(11.0).color(theme::DIM));
            }
        });
    }

    fn selected_preset_note(&self, idx: usize) -> Option<String> {
        if idx < SCALE_PRESETS.len() {
            let preset = SCALE_PRESETS[idx];
            if preset.is_ratio() {
                Some(format!(
                    "{} \u{2248} {:.2} mm nominal height",
                    preset.label(),
                    preset.nominal_height_mm()
                ))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn target_height_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Target height", Some("35mm reference"));
            theme::hint(
                ui,
                "Size a model from a real-world subject, or normalize an existing print.",
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if choice_pill(
                    ui,
                    self.height_mode == HeightMode::RealWorld,
                    "Real-world subject",
                ) {
                    self.height_mode = HeightMode::RealWorld;
                }
                if choice_pill(ui, self.height_mode == HeightMode::Model, "Current model") {
                    self.height_mode = HeightMode::Model;
                }
            });
            ui.add_space(10.0);

            match self.height_mode {
                HeightMode::RealWorld => {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Height").size(11.5).color(theme::MUTED));
                            ui.horizontal(|ui| {
                                let feet = ui.add(
                                    egui::DragValue::new(&mut self.subject_feet)
                                        .range(1.0..=100.0)
                                        .speed(1.0)
                                        .fixed_decimals(0)
                                        .suffix(" ft"),
                                );
                                let inches = ui.add(
                                    egui::DragValue::new(&mut self.subject_inches)
                                        .range(0.0..=11.9)
                                        .speed(0.25)
                                        .fixed_decimals(1)
                                        .suffix(" in"),
                                );
                                if feet.changed() {
                                    self.subject_feet = self.subject_feet.clamp(1.0, 100.0).round();
                                }
                                if inches.changed() {
                                    self.subject_inches = self.subject_inches.clamp(0.0, 11.9);
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (feet, label) in [
                            (6.0, "6 ft Human"),
                            (8.0, "8 ft Ogre"),
                            (10.0, "10 ft Boss"),
                            (12.0, "12 ft Giant"),
                        ] {
                            if ui.small_button(label).clicked() {
                                self.subject_feet = feet;
                                self.subject_inches = 0.0;
                            }
                        }
                    });

                    let total_feet = self.subject_feet + self.subject_inches / 12.0;
                    let height_35 = scale::height_for_subject(total_feet, 35.0);
                    ui.add_space(10.0);
                    Frame::new()
                        .fill(theme::SURFACE_ALT)
                        .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(14)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label(
                                RichText::new("AT 35MM SCALE")
                                    .size(10.0)
                                    .color(theme::MUTED),
                            );
                            if let Some(height) = height_35 {
                                ui.label(
                                    RichText::new(format!("{height:.2} mm"))
                                        .size(32.0)
                                        .strong()
                                        .color(theme::ACCENT_BRIGHT),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "A {:.2} ft subject is {:.2}\u{00d7} the 6 ft reference.",
                                        total_feet,
                                        total_feet / 6.0
                                    ))
                                    .size(11.0)
                                    .color(theme::MUTED),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Enter a positive height.")
                                        .size(12.0)
                                        .color(theme::ERROR),
                                );
                            }
                        });
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        for target in [28.0, 32.0, 54.0, 75.0] {
                            if let Some(height) = scale::height_for_subject(total_feet, target) {
                                theme::pill(
                                    ui,
                                    format!("{target:.0}mm: {height:.2} mm"),
                                    theme::MUTED,
                                );
                            }
                        }
                    });
                }
                HeightMode::Model => {
                    ui.label(
                        RichText::new("Current printed height")
                            .size(11.5)
                            .color(theme::MUTED),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.current_height_mm)
                            .range(1.0..=1000.0)
                            .speed(0.1)
                            .fixed_decimals(2)
                            .suffix(" mm"),
                    );
                    ui.add_space(6.0);
                    let source_mm = self.source_scale_mm();
                    let target = scale::target_height_mm(self.current_height_mm, source_mm, 35.0);
                    let percent = scale::scale_percent(source_mm, 35.0);
                    ui.add_space(4.0);
                    Frame::new()
                        .fill(theme::SURFACE_ALT)
                        .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(14)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label(
                                RichText::new("HEIGHT AT 35MM SCALE")
                                    .size(10.0)
                                    .color(theme::MUTED),
                            );
                            if let Some(target) = target {
                                ui.label(
                                    RichText::new(format!("{target:.2} mm"))
                                        .size(32.0)
                                        .strong()
                                        .color(theme::ACCENT_BRIGHT),
                                );
                                if let Some(percent) = percent {
                                    ui.label(
                                        RichText::new(format!(
                                            "Slicer scale from {}: {:.2}%",
                                            self.select_label(self.source_idx),
                                            percent
                                        ))
                                        .size(11.0)
                                        .color(theme::MUTED),
                                    );
                                }
                            } else {
                                ui.label(
                                    RichText::new("Choose a valid source scale.")
                                        .size(12.0)
                                        .color(theme::ERROR),
                                );
                            }
                        });
                    ui.label(
                        RichText::new(format!(
                            "Uses the source scale selected above: {} ({:.2} mm).",
                            self.select_label(self.source_idx),
                            source_mm
                        ))
                        .size(11.0)
                        .color(theme::DIM),
                    );
                }
            }
        });
    }

    fn base_reference_card(&mut self, ui: &mut Ui) {
        let recommended = if self.height_mode == HeightMode::RealWorld {
            let feet = self.subject_feet + self.subject_inches / 12.0;
            scale::recommended_base_mm(feet)
        } else {
            let source_mm = self.source_scale_mm();
            let implied_feet = if source_mm > 0.0 {
                self.current_height_mm / source_mm * 6.0
            } else {
                6.0
            };
            scale::recommended_base_mm(implied_feet)
        };

        theme::card().show(ui, |ui| {
            theme::section_title(ui, "Base diameter reference", Some("Quick guide"));
            theme::hint(
                ui,
                "Base diameters vary by game system \u{2014} these are common tabletop conventions.",
            );
            ui.add_space(10.0);
            let bases = [
                (
                    25.0,
                    "25mm",
                    "Standard",
                    "Humanoids, 28\u{2013}32mm characters, dungeon heroes.",
                ),
                (
                    40.0,
                    "40mm",
                    "Medium",
                    "Heroic 35mm characters, bulky monsters, elites.",
                ),
                (
                    50.0,
                    "50mm",
                    "Large",
                    "Ogres, trolls, mounted heroes and brutes.",
                ),
                (
                    65.0,
                    "65mm+",
                    "Huge",
                    "Giants, dragons, siege beasts and scenery pieces.",
                ),
            ];
            ui.columns(4, |columns| {
                for (index, (diameter, label, tier, description)) in bases.iter().enumerate() {
                    let selected = (*diameter - recommended).abs() < 0.1;
                    let frame = if selected {
                        Frame::new()
                            .fill(Color32::from_rgb(0x2b, 0x24, 0x18))
                            .stroke(Stroke::new(1.0, theme::ACCENT))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(12)
                    } else {
                        theme::inset_card()
                    };
                    frame.show(&mut columns[index], |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(*label)
                                .size(22.0)
                                .strong()
                                .color(if selected {
                                    theme::ACCENT_BRIGHT
                                } else {
                                    theme::TEXT
                                }),
                        );
                        ui.label(RichText::new(*tier).size(11.5).color(theme::TEAL));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(*description)
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                        if selected {
                            ui.add_space(5.0);
                            theme::pill(ui, "Recommended", theme::ACCENT);
                        }
                    });
                }
            });
        });
    }

    fn tiler_ui(&mut self, ui: &mut Ui) {
        let map_info = self.map.as_ref().map(MapDocument::view);
        let layout_result = self.tiler.layout();
        let layout = layout_result.as_ref().ok();
        let layout_error = layout_result.as_ref().err().map(String::as_str);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Tiled battle maps, true to scale")
                        .size(20.0)
                        .strong()
                        .color(theme::TEXT),
                );
                ui.label(
                    RichText::new(
                        "Print on Letter, Legal, or Tabloid sheets with trim margins and glue overlap.",
                    )
                    .size(12.0)
                    .color(theme::MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(map) = &map_info {
                    ui.add(
                        Label::new(
                            RichText::new(format!(
                                "{} \u{00b7} {} \u{00d7} {} px",
                                map.name, map.width, map.height
                            ))
                            .size(11.0)
                            .color(theme::DIM),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&map.name);
                }
            });
        });
        ui.add_space(10.0);

        let full_height = ui.available_height();
        let mut action = None;

        ui.horizontal_top(|ui| {
            let mut sidebar_action = None;
            ui.allocate_ui_with_layout(
                vec2(SETTINGS_WIDTH, full_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ScrollArea::vertical()
                        .id_salt("tiler-settings")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            sidebar_action = self.tiler_settings_ui(ui, layout, &map_info);
                            ui.add_space(16.0);
                        });
                },
            );
            if sidebar_action.is_some() {
                action = sidebar_action;
            }

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            let preview_width = ui.available_width();
            ui.allocate_ui_with_layout(
                vec2(preview_width, full_height),
                Layout::top_down(Align::Min),
                |ui| {
                    if let Some(preview_action) = preview_panel(
                        ui,
                        map_info.as_ref(),
                        layout,
                        layout_error,
                        self.map_loading.as_ref().map(|job| job.name.as_str()),
                        &mut self.preview,
                    ) {
                        action = Some(preview_action);
                    }
                },
            );
        });

        if let Some(action) = action {
            self.handle_tiler_action(ui.ctx(), action);
        }
    }

    fn tiler_settings_ui(
        &mut self,
        ui: &mut Ui,
        layout: Option<&TileLayout>,
        map: &Option<MapInfoView>,
    ) -> Option<TilerAction> {
        let mut action = None;
        let dimensions_before = self.tiler.map_dimensions_in();

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Map source", Some("PNG \u{00b7} JPG \u{00b7} WebP"));
            ui.add_space(6.0);
            if let Some(loading) = &self.map_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!("Loading {}...", loading.name))
                            .size(13.0)
                            .strong()
                            .color(theme::TEXT),
                    );
                });
                ui.label(
                    RichText::new("Decoding the image in the background.")
                        .size(11.0)
                        .color(theme::MUTED),
                );
            } else if let Some(map) = map {
                ui.add(
                    Label::new(
                        RichText::new(&map.name)
                            .size(13.0)
                            .strong()
                            .color(theme::TEXT),
                    )
                    .truncate(),
                );
                ui.label(
                    RichText::new(format!("{} \u{00d7} {} px", map.width, map.height))
                        .size(11.0)
                        .color(theme::MUTED),
                );
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::new("Replace image\u{2026}"),
                    )
                    .clicked()
                {
                    action = Some(TilerAction::BrowseMap);
                }
            } else {
                ui.add_space(2.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 38.0],
                        egui::Button::new(
                            RichText::new("Browse battle map\u{2026}")
                                .strong()
                                .color(theme::ACCENT_INK),
                        )
                        .fill(theme::ACCENT)
                        .stroke(Stroke::new(1.0, theme::ACCENT_BRIGHT)),
                    )
                    .clicked()
                {
                    action = Some(TilerAction::BrowseMap);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("or drag and drop a map anywhere")
                        .size(11.0)
                        .color(theme::DIM),
                );
            }
            if let Some(error) = &self.map_error {
                ui.add_space(6.0);
                Frame::new()
                    .fill(Color32::from_rgb(0x2b, 0x1b, 0x1e))
                    .stroke(Stroke::new(1.0, theme::ERROR))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(9)
                    .show(ui, |ui| {
                        ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
                    });
            }
        });

        ui.add_space(10.0);

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Target size", Some("Physical output"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if choice_pill(
                    ui,
                    self.tiler.mode == SizeMode::Grid,
                    "Grid count",
                ) {
                    self.tiler.mode = SizeMode::Grid;
                }
                if choice_pill(
                    ui,
                    self.tiler.mode == SizeMode::Exact,
                    "Exact inches",
                ) {
                    self.tiler.mode = SizeMode::Exact;
                }
            });
            ui.add_space(8.0);

            match self.tiler.mode {
                SizeMode::Grid => {
                    ui.columns(2, |columns| {
                        ui_field_label(&mut columns[0], "Grid width");
                        columns[0].add(
                            egui::DragValue::new(&mut self.tiler.grid_width)
                                .range(1..=200)
                                .suffix(" sq"),
                        );
                        ui_field_label(&mut columns[1], "Grid height");
                        columns[1].add(
                            egui::DragValue::new(&mut self.tiler.grid_height)
                                .range(1..=200)
                                .suffix(" sq"),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui_field_label(ui, "Square size");
                        let current_unit = self.tiler.square_unit;
                        ui.add(
                            egui::DragValue::new(&mut self.tiler.square_size)
                                .range(crate::tiler::square_size_bounds(current_unit))
                                .speed(0.02)
                                .fixed_decimals(2)
                                .suffix(format!(" {}", current_unit.label())),
                        );
                        let square_in = self.tiler.square_size_in();
                        if choice_pill(
                            ui,
                            current_unit == SquareUnit::Inches,
                            "in",
                        ) && current_unit != SquareUnit::Inches
                        {
                            self.tiler.square_size = square_in;
                            self.tiler.square_unit = SquareUnit::Inches;
                        }
                        if choice_pill(
                            ui,
                            current_unit == SquareUnit::Millimeters,
                            "mm",
                        ) && current_unit != SquareUnit::Millimeters
                        {
                            self.tiler.square_size = crate::scale::inches_to_mm(square_in);
                            self.tiler.square_unit = SquareUnit::Millimeters;
                        }
                    });
                    let square_in = self.tiler.square_size_in();
                    ui.label(
                        RichText::new(format!(
                            "Each square: {:.2} in / {:.1} mm",
                            square_in,
                            crate::scale::inches_to_mm(square_in)
                        ))
                        .size(11.0)
                        .color(theme::DIM),
                    );
                }
                SizeMode::Exact => {
                    ui.columns(2, |columns| {
                        ui_field_label(&mut columns[0], "Width");
                        columns[0].add(
                            egui::DragValue::new(&mut self.tiler.exact_width_in)
                                .range(0.1..=200.0)
                                .speed(0.1)
                                .fixed_decimals(2)
                                .suffix(" in"),
                        );
                        ui_field_label(&mut columns[1], "Height");
                        columns[1].add(
                            egui::DragValue::new(&mut self.tiler.exact_height_in)
                                .range(0.1..=200.0)
                                .speed(0.1)
                                .fixed_decimals(2)
                                .suffix(" in"),
                        );
                    });
                }
            }

            ui.add_space(8.0);
            let (map_w, map_h) = self.tiler.map_dimensions_in();
            ui.label(
                RichText::new(format!(
                    "Finished map: {map_w:.2} \u{00d7} {map_h:.2} in  ({:.2} \u{00d7} {:.2} cm)",
                    map_w * 2.54,
                    map_h * 2.54
                ))
                .size(11.5)
                .color(theme::MUTED),
            );

            if let Some(map) = map {
                let source_aspect = map.width as f64 / map.height.max(1) as f64;
                let target_aspect = if map_h > 0.0 { map_w / map_h } else { 1.0 };
                if target_aspect > 0.0
                    && (source_aspect / target_aspect - 1.0).abs() > 0.01
                {
                    ui.add_space(7.0);
                    Frame::new()
                        .fill(Color32::from_rgb(0x2b, 0x25, 0x1a))
                        .stroke(Stroke::new(1.0, theme::WARNING))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(9)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Aspect mismatch: source is {source_aspect:.2}:1, target is {target_aspect:.2}:1. The map will stretch."
                                ))
                                .size(11.0)
                                .color(theme::WARNING),
                            );
                            if ui.small_button("Match image aspect").clicked() && map_w > 0.0 {
                                self.tiler.mode = SizeMode::Exact;
                                self.tiler.exact_width_in = map_w;
                                self.tiler.exact_height_in = map_w / source_aspect;
                            }
                        });
                }
            }
        });

        ui.add_space(10.0);

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Paper & assembly", Some("Printer setup"));
            ui.add_space(6.0);
            let (sheet_w, sheet_h) = self.tiler.paper_size_in();
            egui::ComboBox::from_id_salt("paper-size")
                .selected_text(format!(
                    "{}  ({sheet_w:.1} \u{00d7} {sheet_h:.1} in)",
                    self.tiler.paper.label()
                ))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for paper in PAPER_PRESETS {
                        let (w, h) = paper.size_in();
                        ui.selectable_value(
                            &mut self.tiler.paper,
                            paper,
                            format!("{}  ({w:.1} \u{00d7} {h:.1} in)", paper.label()),
                        );
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if choice_pill(
                    ui,
                    self.tiler.orientation == Orientation::Portrait,
                    "Portrait",
                ) {
                    self.tiler.orientation = Orientation::Portrait;
                }
                if choice_pill(
                    ui,
                    self.tiler.orientation == Orientation::Landscape,
                    "Landscape",
                ) {
                    self.tiler.orientation = Orientation::Landscape;
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui_field_label(ui, "Printer margin");
                ui.add(
                    egui::DragValue::new(&mut self.tiler.margin_in)
                        .range(0.0..=2.0)
                        .speed(0.01)
                        .fixed_decimals(2)
                        .suffix(" in"),
                );
            });
            ui.horizontal(|ui| {
                ui_field_label(ui, "Glue/tape overlap");
                ui.add(
                    egui::DragValue::new(&mut self.tiler.overlap_in)
                        .range(0.0..=2.0)
                        .speed(0.01)
                        .fixed_decimals(2)
                        .suffix(" in"),
                );
            });
            let (printable_w, printable_h) = self.tiler.printable_area_in();
            ui.label(
                RichText::new(format!(
                    "Printable area per sheet: {printable_w:.2} \u{00d7} {printable_h:.2} in"
                ))
                .size(11.0)
                .color(theme::DIM),
            );
            if let Some(layout) = layout {
                ui.label(
                    RichText::new(format!(
                        "Sheet step: {:.2} \u{00d7} {:.2} in",
                        layout.step_x_in, layout.step_y_in
                    ))
                    .size(11.0)
                    .color(theme::DIM),
                );
            }
        });

        ui.add_space(10.0);

        theme::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            theme::section_title(ui, "Export", Some("Print at 100%"));
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.include_guides,
                "Include alignment guides & page labels in PDF",
            );
            ui.label(
                RichText::new(
                    "PNG tiles are clean, content-only 300 DPI images numbered by filename; a PRINTING.txt manifest is included.",
                )
                .size(11.0)
                .color(theme::DIM),
            );
            ui.add_space(8.0);

            if let Some(error) = self.tiler.validate().err() {
                ui.label(RichText::new(error).size(11.5).color(theme::ERROR));
                ui.add_space(4.0);
            }

            let mismatch = self.aspect_mismatch();
            if let Some((source_aspect, target_aspect)) = mismatch {
                Frame::new()
                    .fill(Color32::from_rgb(0x2b, 0x25, 0x1a))
                    .stroke(Stroke::new(1.0, theme::WARNING))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(9)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "Exporting will stretch the map ({source_aspect:.2}:1 source vs {target_aspect:.2}:1 target)."
                            ))
                            .size(11.0)
                            .color(theme::WARNING),
                        );
                        ui.checkbox(
                            &mut self.allow_stretched_export,
                            "Allow stretched export",
                        );
                    });
                ui.add_space(6.0);
            }

            let can_export = map.is_some()
                && self.map_loading.is_none()
                && self.tiler.validate().is_ok()
                && self.export_job.is_none()
                && (mismatch.is_none() || self.allow_stretched_export);
            let pdf_button = egui::Button::new(
                RichText::new("Export multi-page PDF")
                    .strong()
                    .color(theme::ACCENT_INK),
            )
            .fill(theme::ACCENT)
            .stroke(Stroke::new(1.0, theme::ACCENT_BRIGHT))
            .min_size(vec2(ui.available_width(), 36.0));
            if ui.add_enabled(can_export, pdf_button).clicked() {
                action = Some(TilerAction::ExportPdf);
            }
            ui.add_space(5.0);
            let png_button = egui::Button::new(RichText::new("Export 300 DPI PNG tiles").strong())
                .fill(theme::SURFACE_HIGH)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .min_size(vec2(ui.available_width(), 34.0));
            if ui.add_enabled(can_export, png_button).clicked() {
                action = Some(TilerAction::ExportPng);
            }

            if let Some(job) = &self.export_job {
                ui.add_space(8.0);
                ui.add(
                    ProgressBar::new(job.progress())
                        .desired_width(ui.available_width())
                        .show_percentage()
                        .text(&job.description),
                );
            } else if map.is_none() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Load a map to enable export.")
                        .size(11.0)
                        .color(theme::DIM),
                );
            }
        });

        ui.add_space(10.0);

        Frame::new()
            .fill(Color32::from_rgb(0x17, 0x1b, 0x20))
            .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new("PRINTING CHECKLIST")
                        .size(10.0)
                        .color(theme::TEAL),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new("\u{2022} Print at 100% / Actual Size \u{2014} never \u{201c}Fit to Page\u{201d}.")
                        .size(11.0)
                        .color(theme::MUTED),
                );
                ui.label(
                    RichText::new(format!(
                        "\u{2022} Trim to the printed cut border ({:.2} in margins; remove any partial-edge blank).",
                        self.tiler.margin_in
                    ))
                    .size(11.0)
                    .color(theme::MUTED),
                );
                ui.label(
                    RichText::new(format!(
                        "\u{2022} Overlap {:.2} in using the dashed alignment guides.",
                        self.tiler.overlap_in
                    ))
                    .size(11.0)
                    .color(theme::MUTED),
                );
                ui.label(
                    RichText::new(
                        "\u{2022} PDF includes margins; PNG tiles are content-only. PDF is safest for most printers.",
                    )
                    .size(11.0)
                    .color(theme::MUTED),
                );
            });

        let dimensions_after = self.tiler.map_dimensions_in();
        if (dimensions_before.0 - dimensions_after.0).abs() > 1e-9
            || (dimensions_before.1 - dimensions_after.1).abs() > 1e-9
        {
            self.allow_stretched_export = false;
        }

        action
    }
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("The image is empty.".to_owned());
    }
    let pixels = width as u64 * height as u64;
    if pixels > MAX_IMAGE_PIXELS {
        return Err(format!(
            "This image is too large: {width} \u{00d7} {height} px ({:.1} MP). The limit is {:.0} MP.",
            pixels as f64 / 1_000_000.0,
            MAX_IMAGE_PIXELS as f64 / 1_000_000.0
        ));
    }
    Ok(())
}

fn load_map_worker(path: &Path) -> Result<LoadedMap, String> {
    let reader = image::ImageReader::open(path)
        .map_err(|error| format!("Could not open image: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Could not identify the image format: {error}"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("Could not read image dimensions: {error}"))?;
    validate_image_dimensions(width, height)?;

    let image = image::ImageReader::open(path)
        .map_err(|error| format!("Could not reopen image: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Could not identify the image format: {error}"))?
        .decode()
        .map_err(|error| {
            format!("Could not decode this image. Use PNG, JPG, or WebP. ({error})")
        })?;
    validate_image_dimensions(image.width(), image.height())?;
    let preview = image.thumbnail(2048, 2048).to_rgba8();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("battle-map")
        .to_owned();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("battle-map")
        .to_owned();
    Ok(LoadedMap {
        path: path.to_path_buf(),
        name,
        stem,
        source: image,
        preview,
    })
}

impl eframe::App for TableScaleApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_export();
        self.poll_map_load(&ctx);
        self.handle_dropped_files(&ctx);
        self.expire_toast();

        let command = ctx.input(|input| input.modifiers.command);
        if command && ctx.input(|input| input.key_pressed(egui::Key::Num1)) {
            self.tab = Tab::Scaler;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Num2)) {
            self.tab = Tab::Tiler;
        }

        self.header_ui(ui);
        self.status_ui(ui);

        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin {
                left: 18,
                right: 18,
                top: 8,
                bottom: 8,
            }))
            .show(ui, |ui| match self.tab {
                Tab::Scaler => self.scaler_ui(ui),
                Tab::Tiler => self.tiler_ui(ui),
            });

        if self.export_job.is_some() || self.map_loading.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        if let Some(toast) = &self.toast {
            let remaining = toast.ttl.saturating_sub(toast.created.elapsed());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining.min(Duration::from_millis(250)));
            }
        }
    }
}

fn tab_button(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let button = egui::Button::new(RichText::new(label).size(13.5).color(if selected {
        theme::ACCENT_BRIGHT
    } else {
        theme::MUTED
    }))
    .min_size(vec2(172.0, 34.0));
    let button = if selected {
        button
            .fill(Color32::from_rgb(0x30, 0x27, 0x19))
            .stroke(Stroke::new(1.0, theme::ACCENT))
    } else {
        button.frame(false)
    };
    ui.add(button).clicked()
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
        .min_size(vec2(0.0, 26.0)),
    )
    .clicked()
}

fn ui_field_label(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).size(11.0).color(theme::MUTED));
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
        .unwrap_or(false)
}

fn preview_panel(
    ui: &mut Ui,
    map: Option<&MapInfoView>,
    layout: Option<&TileLayout>,
    layout_error: Option<&str>,
    loading: Option<&str>,
    preview: &mut PreviewState,
) -> Option<TilerAction> {
    let mut action = None;
    let total_pages = layout.map(TileLayout::page_count).unwrap_or(0);
    let hovered_page = preview
        .hovered_page
        .filter(|index| *index < total_pages)
        .map(|index| index + 1)
        .unwrap_or_else(|| if total_pages > 0 { 1 } else { 0 });

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Page grid preview")
                .size(16.0)
                .strong()
                .color(theme::TEXT),
        );
        if let Some(layout) = layout {
            theme::pill(ui, layout.sheet_label(), theme::TEAL);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if total_pages > 0 {
                ui.label(
                    RichText::new(format!("Page {hovered_page} of {total_pages}"))
                        .size(12.0)
                        .strong()
                        .color(theme::TEXT),
                );
            }
        });
    });
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        if let Some(layout) = layout {
            theme::pill(
                ui,
                format!(
                    "Map {:.2} \u{00d7} {:.2} in",
                    layout.map_width_in, layout.map_height_in
                ),
                theme::MUTED,
            );
            theme::pill(
                ui,
                format!(
                    "Printable {:.2} \u{00d7} {:.2} in",
                    layout.printable_width_in, layout.printable_height_in
                ),
                theme::MUTED,
            );
            if let Some(map) = map {
                let dpi = layout.effective_dpi(map.width, map.height);
                theme::pill(
                    ui,
                    format!("{dpi:.1} DPI source"),
                    if dpi >= 300.0 {
                        theme::SUCCESS
                    } else {
                        theme::WARNING
                    },
                );
            }
        } else if let Some(error) = layout_error {
            ui.label(RichText::new(error).size(11.5).color(theme::ERROR));
        }
    });
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let fit = ui
            .add(egui::Button::new(RichText::new("Fit").size(11.5)).small())
            .clicked();
        if fit {
            preview.reset();
        }
        ui.add(
            egui::Slider::new(&mut preview.zoom, 0.35..=6.0)
                .logarithmic(true)
                .show_value(false)
                .text("Zoom"),
        );
        ui.label(
            RichText::new(format!("\u{00d7}{:.2}", preview.zoom))
                .size(11.0)
                .color(theme::DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new("Scroll to zoom \u{00b7} drag to pan")
                    .size(10.5)
                    .color(theme::DIM),
            );
        });
    });
    ui.add_space(6.0);

    let available = ui.available_size();
    let canvas_size = vec2(available.x.max(200.0), available.y.max(PREVIEW_MIN_HEIGHT));
    let (rect, response) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(12), theme::PREVIEW_BG);
    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    let canvas = rect.shrink(14.0);

    if map.is_none() {
        if let Some(name) = loading {
            draw_loading_state(&painter, canvas, name);
        } else {
            draw_empty_state(&painter, canvas);
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                action = Some(TilerAction::BrowseMap);
            }
        }
        return action;
    }

    let Some(map) = map else {
        return action;
    };
    let Some(layout) = layout else {
        painter.text(
            canvas.center(),
            Align2::CENTER_CENTER,
            layout_error.unwrap_or("Fix the tiling settings to preview the page grid."),
            FontId::new(13.0, FontFamily::Proportional),
            theme::ERROR,
        );
        return action;
    };

    if response.dragged() {
        preview.pan += response.drag_delta();
    }
    if response.hovered() {
        let pinch = ui.input(|input| input.zoom_delta());
        if (pinch - 1.0).abs() > 0.0001 {
            preview.zoom = (preview.zoom * pinch).clamp(0.35, 6.0);
        } else {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                preview.zoom = (preview.zoom * (scroll * 0.0015).exp()).clamp(0.35, 6.0);
            }
        }
    }

    let fit = fit_rect(canvas, layout.aspect_ratio());
    let display_size = fit.size() * preview.zoom;
    let max_pan = vec2(
        ((display_size.x - canvas.width()) * 0.5).max(0.0),
        ((display_size.y - canvas.height()) * 0.5).max(0.0),
    );
    preview.pan.x = preview.pan.x.clamp(-max_pan.x, max_pan.x);
    preview.pan.y = preview.pan.y.clamp(-max_pan.y, max_pan.y);
    let display_rect = Rect::from_center_size(canvas.center() + preview.pan, display_size);

    let map_painter = ui.painter_at(canvas);
    map_painter.rect_filled(
        display_rect.expand(5.0),
        CornerRadius::same(3),
        Color32::from_black_alpha(80),
    );
    map_painter.image(
        map.texture.id(),
        display_rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );

    let scale_x = display_rect.width() / layout.map_width_in as f32;
    let scale_y = display_rect.height() / layout.map_height_in as f32;
    let map_to_screen = |x: f64, y: f64| {
        pos2(
            display_rect.left() + x as f32 * scale_x,
            display_rect.top() + y as f32 * scale_y,
        )
    };

    // Overlap bands behind the page lines.
    for tile in &layout.tiles {
        if layout.has_overlap_right(tile) {
            let left = map_to_screen(tile.x_in + tile.width_in - layout.overlap_in, tile.y_in);
            let right = map_to_screen(tile.x_in + tile.width_in, tile.y_in + tile.height_in);
            let band = Rect::from_min_max(left, right);
            map_painter.rect_filled(band, CornerRadius::ZERO, theme::TEAL_SOFT);
        }
        if layout.has_overlap_bottom(tile) {
            let left = map_to_screen(tile.x_in, tile.y_in + tile.height_in - layout.overlap_in);
            let right = map_to_screen(tile.x_in + tile.width_in, tile.y_in + tile.height_in);
            let band = Rect::from_min_max(left, right);
            map_painter.rect_filled(band, CornerRadius::ZERO, theme::TEAL_SOFT);
        }
    }

    let pointer = response
        .hover_pos()
        .filter(|position| canvas.contains(*position));
    let mut hovered = None;
    let mut tile_rects = Vec::with_capacity(layout.tiles.len());
    for tile in &layout.tiles {
        let tile_rect = Rect::from_min_max(
            map_to_screen(tile.x_in, tile.y_in),
            map_to_screen(tile.x_in + tile.width_in, tile.y_in + tile.height_in),
        );
        tile_rects.push(tile_rect);
        if let Some(pointer) = pointer {
            if tile_rect.contains(pointer) && hovered.is_none() {
                hovered = Some(tile.index);
            }
        }
    }
    preview.hovered_page = hovered;

    for (tile, tile_rect) in layout.tiles.iter().zip(tile_rects.iter()) {
        map_painter.rect_stroke(
            *tile_rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(111, 179, 184, 210)),
            StrokeKind::Inside,
        );
        if hovered == Some(tile.index) {
            map_painter.rect_filled(
                *tile_rect,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(224, 164, 88, 24),
            );
            map_painter.rect_stroke(
                *tile_rect,
                CornerRadius::ZERO,
                Stroke::new(2.0, theme::ACCENT),
                StrokeKind::Inside,
            );
        }

        if layout.has_overlap_right(tile) {
            let guide_x = display_rect.left()
                + (tile.x_in + tile.width_in - layout.overlap_in) as f32 * scale_x;
            let top = tile_rect.top() + 8.0;
            let bottom = tile_rect.bottom() - 8.0;
            if bottom > top {
                map_painter.extend(egui::Shape::dashed_line(
                    &[pos2(guide_x, top), pos2(guide_x, bottom)],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(111, 179, 184, 150)),
                    4.0,
                    4.0,
                ));
            }
        }
        if layout.has_overlap_bottom(tile) {
            let guide_y = display_rect.top()
                + (tile.y_in + tile.height_in - layout.overlap_in) as f32 * scale_y;
            let left = tile_rect.left() + 8.0;
            let right = tile_rect.right() - 8.0;
            if right > left {
                map_painter.extend(egui::Shape::dashed_line(
                    &[pos2(left, guide_y), pos2(right, guide_y)],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(111, 179, 184, 150)),
                    4.0,
                    4.0,
                ));
            }
        }

        if tile_rect.width() > 42.0 && tile_rect.height() > 32.0 {
            let center = tile_rect.center();
            map_painter.circle_filled(center, 11.0, Color32::from_black_alpha(195));
            map_painter.circle_stroke(center, 11.0, Stroke::new(1.0, theme::TEAL));
            map_painter.text(
                center,
                Align2::CENTER_CENTER,
                tile.page_number().to_string(),
                FontId::new(11.0, FontFamily::Proportional),
                if hovered == Some(tile.index) {
                    theme::ACCENT_BRIGHT
                } else {
                    theme::TEXT
                },
            );
        }
    }

    map_painter.rect_stroke(
        display_rect,
        CornerRadius::same(1),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(224, 164, 88, 180)),
        StrokeKind::Inside,
    );

    let info_text = if let Some(index) = hovered {
        let tile = &layout.tiles[index];
        format!(
            "{}  \u{00b7}  Row {}, Col {}  \u{00b7}  {:.2} \u{00d7} {:.2} in",
            layout.page_label(tile),
            tile.row + 1,
            tile.column + 1,
            tile.width_in,
            tile.height_in
        )
    } else {
        "Hover a page for its number and sheet size".to_owned()
    };
    draw_info_badge(&map_painter, canvas, &info_text);

    if response.hovered() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }

    action
}

fn fit_rect(rect: Rect, aspect: f64) -> Rect {
    let aspect = aspect.max(0.001) as f32;
    let available = rect.size();
    let mut width = available.x;
    let mut height = width / aspect;
    if height > available.y {
        height = available.y;
        width = height * aspect;
    }
    Rect::from_center_size(rect.center(), vec2(width, height))
}

fn draw_loading_state(painter: &egui::Painter, rect: Rect, name: &str) {
    let center = rect.center();
    painter.text(
        pos2(center.x, center.y - 26.0),
        Align2::CENTER_CENTER,
        "Loading battle map",
        FontId::new(20.0, FontFamily::Proportional),
        theme::TEXT,
    );
    painter.text(
        pos2(center.x, center.y + 2.0),
        Align2::CENTER_CENTER,
        name,
        FontId::new(12.0, FontFamily::Proportional),
        theme::MUTED,
    );
    painter.text(
        pos2(center.x, center.y + 24.0),
        Align2::CENTER_CENTER,
        "Decoding in the background...",
        FontId::new(11.0, FontFamily::Proportional),
        theme::DIM,
    );
}

fn draw_empty_state(painter: &egui::Painter, rect: Rect) {
    let center = rect.center();
    painter.text(
        pos2(center.x, center.y - 58.0),
        Align2::CENTER_CENTER,
        "Drop a battle map",
        FontId::new(22.0, FontFamily::Proportional),
        theme::TEXT,
    );
    painter.text(
        pos2(center.x, center.y - 28.0),
        Align2::CENTER_CENTER,
        "PNG, JPG, or WebP \u{00b7} click anywhere to browse",
        FontId::new(12.0, FontFamily::Proportional),
        theme::MUTED,
    );
    let grid_size = 54.0_f32;
    let grid_origin = pos2(center.x - grid_size * 0.5, center.y + 16.0);
    let cell = grid_size / 3.0;
    for line in 0..=3 {
        let offset = line as f32 * cell;
        painter.line_segment(
            [
                pos2(grid_origin.x, grid_origin.y + offset),
                pos2(grid_origin.x + grid_size, grid_origin.y + offset),
            ],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(111, 179, 184, 90)),
        );
        painter.line_segment(
            [
                pos2(grid_origin.x + offset, grid_origin.y),
                pos2(grid_origin.x + offset, grid_origin.y + grid_size),
            ],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(111, 179, 184, 90)),
        );
    }
    painter.rect_stroke(
        Rect::from_min_size(grid_origin, vec2(grid_size, grid_size)),
        CornerRadius::ZERO,
        Stroke::new(1.0, theme::TEAL),
        StrokeKind::Inside,
    );
}

fn draw_info_badge(painter: &egui::Painter, canvas: Rect, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::new(11.0, FontFamily::Proportional),
        theme::MUTED,
    );
    let padding = vec2(9.0, 5.0);
    let size = galley.size() + padding * 2.0;
    let origin = pos2(canvas.left() + 2.0, canvas.bottom() - size.y - 2.0);
    let badge = Rect::from_min_size(origin, size);
    painter.rect_filled(
        badge,
        CornerRadius::same(6),
        Color32::from_rgba_unmultiplied(8, 9, 12, 220),
    );
    painter.galley(origin + padding, galley, theme::MUTED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_pixel_limit_accepts_large_but_sane_maps() {
        assert!(validate_image_dimensions(10_000, 10_000).is_ok());
    }

    #[test]
    fn image_pixel_limit_rejects_oversized_images() {
        assert!(validate_image_dimensions(10_001, 10_000).is_err());
        assert!(validate_image_dimensions(0, 100).is_err());
    }
}
