//! Multi-page PDF and 300 DPI PNG tile export.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use png::{BitDepth, ColorType, PixelDimensions, Unit};
use printpdf::{
    BuiltinFont, Color, Line, LineDashPattern, LinePoint, Mm, Op, PdfDocument, PdfFontHandle,
    PdfPage, PdfSaveOptions, Point, Pt, RawImage, RawImageData, RawImageFormat, Rect, Rgb,
    TextItem, WindingOrder, XObjectTransform,
};

use crate::tiler::{Tile, TileLayout};

pub const OUTPUT_DPI: f64 = 300.0;
const PDF_MAX_DPI: f64 = 300.0;
const POINTS_PER_INCH: f64 = 72.0;
pub const PIXELS_PER_METRE_AT_300_DPI: u32 = 11_811;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Pdf,
    PngTiles,
}

#[derive(Debug)]
pub struct ExportSummary {
    pub kind: ExportKind,
    pub output: PathBuf,
    pub pages: usize,
}

impl ExportSummary {
    pub fn headline(&self) -> String {
        match self.kind {
            ExportKind::Pdf => format!("PDF exported \u{00b7} {}", plural_pages(self.pages)),
            ExportKind::PngTiles => {
                format!("{} PNG tiles saved", self.pages)
            }
        }
    }
}

fn plural_pages(pages: usize) -> String {
    if pages == 1 {
        "1 page".to_owned()
    } else {
        format!("{pages} pages")
    }
}

pub struct ExportJob {
    receiver: Receiver<Result<ExportSummary, String>>,
    progress: Arc<AtomicU32>,
    pub description: String,
}

impl ExportJob {
    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn poll(&self) -> Option<Result<ExportSummary, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("The export worker stopped unexpectedly.".to_owned()))
            }
        }
    }
}

pub fn start_pdf_export(
    path: PathBuf,
    layout: TileLayout,
    source: Arc<DynamicImage>,
    include_guides: bool,
) -> ExportJob {
    let progress = Arc::new(AtomicU32::new(0));
    let worker_progress = Arc::clone(&progress);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let result = (|| {
            let bytes = build_pdf_bytes(&layout, &source, include_guides, |fraction| {
                worker_progress.store((fraction * 1000.0) as u32, Ordering::Relaxed);
            })?;
            write_file_atomic(&path, &bytes)?;
            Ok(ExportSummary {
                kind: ExportKind::Pdf,
                output: path,
                pages: layout.page_count(),
            })
        })();
        let _ = sender.send(result);
    });

    ExportJob {
        receiver,
        progress,
        description: "Building multi-page PDF\u{2026}".to_owned(),
    }
}

pub fn start_png_export(
    parent: PathBuf,
    layout: TileLayout,
    source: Arc<DynamicImage>,
    stem: String,
) -> ExportJob {
    let progress = Arc::new(AtomicU32::new(0));
    let worker_progress = Arc::clone(&progress);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let result = (|| {
            let stem = sanitize_stem(&stem);
            let final_folder = unique_tile_folder(&parent, &stem)?;
            let temporary_folder = parent.join(format!(".{stem}-building-{}", unique_token()));
            std::fs::create_dir_all(&temporary_folder)
                .map_err(|error| format!("Could not create tile folder: {error}"))?;
            let total = layout.page_count().max(1);
            let export_result = (|| {
                for (index, tile) in layout.tiles.iter().enumerate() {
                    let rgb = render_tile_rgb(&source, &layout, tile, OUTPUT_DPI);
                    let filename = tile_filename(tile, layout.page_count(), &stem);
                    write_png_file(&temporary_folder.join(filename), &rgb, OUTPUT_DPI)?;
                    let fraction = 0.05 + 0.88 * (index + 1) as f32 / total as f32;
                    worker_progress.store((fraction * 1000.0) as u32, Ordering::Relaxed);
                }
                write_print_manifest(&temporary_folder.join("PRINTING.txt"), &layout, &stem)?;
                worker_progress.store(970, Ordering::Relaxed);
                std::fs::rename(&temporary_folder, &final_folder)
                    .map_err(|error| format!("Could not finalize tile folder: {error}"))
            })();
            if let Err(error) = export_result {
                let _ = std::fs::remove_dir_all(&temporary_folder);
                return Err(error);
            }
            worker_progress.store(1000, Ordering::Relaxed);
            Ok(ExportSummary {
                kind: ExportKind::PngTiles,
                output: final_folder,
                pages: layout.page_count(),
            })
        })();
        let _ = sender.send(result);
    });

    ExportJob {
        receiver,
        progress,
        description: "Rendering 300 DPI tiles\u{2026}".to_owned(),
    }
}

/// Build a self-contained, true-scale PDF in memory.
pub fn build_pdf_bytes<F>(
    layout: &TileLayout,
    source: &DynamicImage,
    include_guides: bool,
    mut report: F,
) -> Result<Vec<u8>, String>
where
    F: FnMut(f32),
{
    let (raster, dpi) = prepare_pdf_raster(source, layout)
        .map_err(|error| format!("Could not prepare the map raster for PDF export: {error}"))?;
    report(0.12);

    let mut doc = PdfDocument::new("TableScale Battle Map");
    let (raster_w, raster_h) = raster.dimensions();
    let image_id = {
        let raw = RawImage {
            pixels: RawImageData::U8(raster.into_raw()),
            width: raster_w as usize,
            height: raster_h as usize,
            data_format: RawImageFormat::RGB8,
            tag: Vec::new(),
        };
        doc.add_image(&raw)
    };

    let paper_h_pt = (layout.paper_height_in * POINTS_PER_INCH) as f32;
    let margin_pt = (layout.margin_in * POINTS_PER_INCH) as f32;
    let content_left = margin_pt;
    let content_top = paper_h_pt - margin_pt;
    let map_w_pt = (layout.map_width_in * POINTS_PER_INCH) as f32;
    let map_h_pt = (layout.map_height_in * POINTS_PER_INCH) as f32;
    let natural_w_pt = raster_w as f32 / dpi as f32 * 72.0;
    let natural_h_pt = raster_h as f32 / dpi as f32 * 72.0;
    let scale_x = if natural_w_pt > 0.0 {
        map_w_pt / natural_w_pt
    } else {
        1.0
    };
    let scale_y = if natural_h_pt > 0.0 {
        map_h_pt / natural_h_pt
    } else {
        1.0
    };

    let total_pages = layout.page_count().max(1);
    let mut pages = Vec::with_capacity(total_pages);

    for (index, tile) in layout.tiles.iter().enumerate() {
        let tile_x = (tile.x_in * POINTS_PER_INCH) as f32;
        let tile_y = (tile.y_in * POINTS_PER_INCH) as f32;
        let area = page_content_area(layout, tile);
        let page_left = area.left;
        let page_bottom = area.bottom;
        let tile_w = area.width;
        let tile_h = area.height;
        let mut ops = Vec::new();

        ops.push(Op::SaveGraphicsState);
        ops.push(Op::DrawRectangle {
            rectangle: Rect {
                x: Pt(page_left),
                y: Pt(page_bottom),
                width: Pt(tile_w),
                height: Pt(tile_h),
                mode: Some(printpdf::PaintMode::Clip),
                winding_order: Some(WindingOrder::NonZero),
            },
        });

        ops.push(Op::UseXobject {
            id: image_id.clone(),
            transform: XObjectTransform {
                translate_x: Some(Pt(content_left - tile_x)),
                translate_y: Some(Pt(content_top - map_h_pt + tile_y)),
                scale_x: Some(scale_x),
                scale_y: Some(scale_y),
                dpi: Some(dpi as f32),
                ..Default::default()
            },
        });

        if include_guides {
            add_page_guides(&mut ops, layout, tile, area);
        }

        ops.push(Op::RestoreGraphicsState);

        pages.push(PdfPage::new(
            Mm((layout.paper_width_in * 25.4) as f32),
            Mm((layout.paper_height_in * 25.4) as f32),
            ops,
        ));
        report(0.12 + 0.82 * (index + 1) as f32 / total_pages as f32);
    }

    doc.with_pages(pages);
    report(0.96);
    let mut warnings = Vec::new();
    let bytes = doc.save(&PdfSaveOptions::default(), &mut warnings);
    if !bytes.starts_with(b"%PDF-") {
        return Err("The PDF writer returned an invalid document.".to_owned());
    }
    report(1.0);
    Ok(bytes)
}

/// Downsample very high-resolution maps to at most 300 DPI for the PDF.
/// Lower-resolution sources are kept at native resolution and simply placed
/// at the correct physical size, avoiding pointless upsampling.
fn prepare_pdf_raster(
    source: &DynamicImage,
    layout: &TileLayout,
) -> Result<(RgbImage, f64), String> {
    let effective_dpi = layout.effective_dpi(source.width(), source.height());
    if !effective_dpi.is_finite() || effective_dpi <= 0.0 {
        return Err("The map resolution could not be determined.".to_owned());
    }

    if effective_dpi > PDF_MAX_DPI {
        let width = (layout.map_width_in * PDF_MAX_DPI).round().max(1.0) as u32;
        let height = (layout.map_height_in * PDF_MAX_DPI).round().max(1.0) as u32;
        let resized = source.resize_exact(width, height, FilterType::Lanczos3);
        Ok((dynamic_to_rgb8(&resized), PDF_MAX_DPI))
    } else {
        Ok((dynamic_to_rgb8(source), effective_dpi))
    }
}

#[derive(Clone, Copy)]
struct PageArea {
    left: f32,
    top: f32,
    bottom: f32,
    width: f32,
    height: f32,
}

fn page_content_area(layout: &TileLayout, tile: &Tile) -> PageArea {
    let content_top = (layout.paper_height_in - layout.margin_in) * POINTS_PER_INCH;
    let tile_h = (tile.height_in * POINTS_PER_INCH) as f32;
    PageArea {
        left: (layout.margin_in * POINTS_PER_INCH) as f32,
        top: content_top as f32,
        bottom: (content_top - tile.height_in * POINTS_PER_INCH) as f32,
        width: (tile.width_in * POINTS_PER_INCH) as f32,
        height: tile_h,
    }
}

fn add_page_guides(ops: &mut Vec<Op>, layout: &TileLayout, tile: &Tile, area: PageArea) {
    let PageArea {
        left: page_left,
        top: page_top,
        bottom: page_bottom,
        width: tile_w,
        height: tile_h,
    } = area;
    let guide = Color::Rgb(Rgb {
        r: 0.62,
        g: 0.65,
        b: 0.69,
        icc_profile: None,
    });
    let dark = Color::Rgb(Rgb {
        r: 0.12,
        g: 0.13,
        b: 0.15,
        icc_profile: None,
    });
    let white = Color::Rgb(Rgb {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        icc_profile: None,
    });

    // Thin cut border around the printable map area.
    ops.push(Op::SetOutlineColor { col: guide.clone() });
    ops.push(Op::SetOutlineThickness { pt: Pt(0.35) });
    ops.push(Op::DrawRectangle {
        rectangle: Rect {
            x: Pt(page_left + 0.8),
            y: Pt(page_bottom + 0.8),
            width: Pt((tile_w - 1.6).max(0.5)),
            height: Pt((tile_h - 1.6).max(0.5)),
            mode: Some(printpdf::PaintMode::Stroke),
            winding_order: None,
        },
    });

    // Alignment guides at the inner edge of each overlap band.
    ops.push(Op::SetOutlineColor { col: guide.clone() });
    ops.push(Op::SetOutlineThickness { pt: Pt(0.5) });
    ops.push(Op::SetLineDashPattern {
        dash: LineDashPattern::new(0.0, &[3.0, 3.0]),
    });

    if layout.has_overlap_right(tile) {
        let guide_x = page_left + ((tile.width_in - layout.overlap_in) * POINTS_PER_INCH) as f32;
        let top = page_top - 22.0;
        let bottom = page_bottom + 14.0;
        if top > bottom {
            ops.push(Op::DrawLine {
                line: simple_line(guide_x, bottom, guide_x, top),
            });
        }
    }

    if layout.has_overlap_bottom(tile) {
        let guide_y = page_top - ((tile.height_in - layout.overlap_in) * POINTS_PER_INCH) as f32;
        let left = page_left + 14.0;
        let right = page_left + tile_w - 14.0;
        if right > left {
            ops.push(Op::DrawLine {
                line: simple_line(left, guide_y, right, guide_y),
            });
        }
    }

    ops.push(Op::SetLineDashPattern {
        dash: LineDashPattern::solid(),
    });

    // Small corner crop ticks.
    ops.push(Op::SetOutlineThickness { pt: Pt(0.7) });
    let inset = 2.0;
    let tick = 8.0;
    let left = page_left + inset;
    let right = page_left + tile_w - inset;
    let bottom = page_bottom + inset;
    let top = page_top - inset;
    for (x, y, dx, dy) in [
        (left, top, 1.0, -1.0),
        (right, top, -1.0, -1.0),
        (left, bottom, 1.0, 1.0),
        (right, bottom, -1.0, 1.0),
    ] {
        ops.push(Op::DrawLine {
            line: simple_line(x, y, x + dx * tick, y),
        });
        ops.push(Op::DrawLine {
            line: simple_line(x, y, x, y + dy * tick),
        });
    }

    // Page identification badge in the top-left of the printable area.
    let full_label = format!(
        "Page {} of {}  |  R{} C{}",
        tile.page_number(),
        layout.page_count(),
        tile.row + 1,
        tile.column + 1
    );
    let compact_label = format!("P{}/{}", tile.page_number(), layout.page_count());
    let available_w = tile_w - 10.0;
    if available_w < 30.0 {
        return;
    }
    let estimate_width =
        |text: &str, font_size: f32| text.chars().count() as f32 * font_size * 0.6 + 8.0;
    let full_width = estimate_width(&full_label, 7.0);
    let compact_width = estimate_width(&compact_label, 7.5);
    let (label, mut box_w, font_size) = if full_width <= available_w {
        (full_label, full_width, 7.0_f32)
    } else if compact_width <= available_w {
        (compact_label, compact_width, 7.5_f32)
    } else {
        return;
    };
    box_w = box_w.min(available_w);
    let box_h = 13.0;
    let box_x = page_left + 5.0;
    let box_y = page_top - 18.0;
    if box_y < page_bottom + 2.0 || box_y + box_h > page_top - 1.0 {
        return;
    }

    ops.push(Op::SetFillColor { col: white });
    ops.push(Op::SetOutlineColor { col: guide });
    ops.push(Op::SetOutlineThickness { pt: Pt(0.35) });
    ops.push(Op::DrawRectangle {
        rectangle: Rect {
            x: Pt(box_x),
            y: Pt(box_y),
            width: Pt(box_w),
            height: Pt(box_h),
            mode: Some(printpdf::PaintMode::FillStroke),
            winding_order: None,
        },
    });

    ops.push(Op::StartTextSection);
    ops.push(Op::SetFillColor { col: dark });
    ops.push(Op::SetFont {
        font: PdfFontHandle::Builtin(BuiltinFont::HelveticaBold),
        size: Pt(font_size),
    });
    ops.push(Op::SetTextCursor {
        pos: Point {
            x: Pt(box_x + 4.0),
            y: Pt(box_y + 4.0),
        },
    });
    ops.push(Op::ShowText {
        items: vec![TextItem::Text(label)],
    });
    ops.push(Op::EndTextSection);
}

fn simple_line(x1: f32, y1: f32, x2: f32, y2: f32) -> Line {
    Line {
        points: vec![
            LinePoint {
                p: Point {
                    x: Pt(x1),
                    y: Pt(y1),
                },
                bezier: false,
            },
            LinePoint {
                p: Point {
                    x: Pt(x2),
                    y: Pt(y2),
                },
                bezier: false,
            },
        ],
        is_closed: false,
    }
}

/// Render one physical tile at `dpi`, stretching the source to the target
/// physical dimensions exactly as previewed.
fn render_tile_rgb(source: &DynamicImage, layout: &TileLayout, tile: &Tile, dpi: f64) -> RgbImage {
    let px_per_in_x = source.width() as f64 / layout.map_width_in;
    let px_per_in_y = source.height() as f64 / layout.map_height_in;
    let source_w = source.width() as i64;
    let source_h = source.height() as i64;
    let x0 = ((tile.x_in * px_per_in_x).round() as i64).clamp(0, source_w.saturating_sub(1));
    let y0 = ((tile.y_in * px_per_in_y).round() as i64).clamp(0, source_h.saturating_sub(1));
    let x1 = (((tile.x_in + tile.width_in) * px_per_in_x).round() as i64).clamp(x0 + 1, source_w);
    let y1 = (((tile.y_in + tile.height_in) * px_per_in_y).round() as i64).clamp(y0 + 1, source_h);
    let crop_w = (x1 - x0) as u32;
    let crop_h = (y1 - y0) as u32;
    let target_w = (tile.width_in * dpi).round().max(1.0) as u32;
    let target_h = (tile.height_in * dpi).round().max(1.0) as u32;

    let cropped = source.crop_imm(x0 as u32, y0 as u32, crop_w, crop_h);
    let rendered = if cropped.width() == target_w && cropped.height() == target_h {
        cropped
    } else {
        cropped.resize_exact(target_w, target_h, FilterType::Lanczos3)
    };
    dynamic_to_rgb8(&rendered)
}

fn dynamic_to_rgb8(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = RgbImage::new(width, height);
    for (src, dst) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = src[3] as f32 / 255.0;
        if alpha >= 0.999 {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
        } else {
            dst[0] = (src[0] as f32 * alpha + 255.0 * (1.0 - alpha)).round() as u8;
            dst[1] = (src[1] as f32 * alpha + 255.0 * (1.0 - alpha)).round() as u8;
            dst[2] = (src[2] as f32 * alpha + 255.0 * (1.0 - alpha)).round() as u8;
        }
    }
    rgb
}

fn tile_filename(tile: &Tile, total: usize, stem: &str) -> String {
    let width = total.to_string().len().max(2);
    format!(
        "{stem}_page-{page:0width$}_of-{total:0width$}_r{row:02}_c{col:02}.png",
        page = tile.page_number(),
        width = width,
        total = total,
        row = tile.row + 1,
        col = tile.column + 1,
    )
}

fn write_png_file(path: &Path, image: &RgbImage, dpi: f64) -> Result<(), String> {
    let file = File::create(path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, image.width(), image.height());
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    let pixels_per_metre = if (dpi - OUTPUT_DPI).abs() < 1e-6 {
        PIXELS_PER_METRE_AT_300_DPI
    } else {
        (dpi / 0.0254).round() as u32
    };
    encoder.set_pixel_dims(Some(PixelDimensions {
        xppu: pixels_per_metre,
        yppu: pixels_per_metre,
        unit: Unit::Meter,
    }));
    let _ = encoder.add_text_chunk("Software".to_owned(), "TableScale".to_owned());
    let mut png_writer = encoder
        .write_header()
        .map_err(|error| format!("Could not write PNG header: {error}"))?;
    png_writer
        .write_image_data(image.as_raw())
        .map_err(|error| format!("Could not write PNG data: {error}"))?;
    png_writer
        .finish()
        .map_err(|error| format!("Could not finish PNG: {error}"))
}

pub fn sanitize_stem(stem: &str) -> String {
    let mut sanitized = String::with_capacity(stem.len());
    let mut last_dash = false;
    for character in stem.chars() {
        if character.is_alphanumeric() {
            for lowercase in character.to_lowercase() {
                sanitized.push(lowercase);
            }
            last_dash = false;
        } else if !last_dash {
            sanitized.push('-');
            last_dash = true;
        }
    }
    let trimmed = sanitized.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "battle-map".to_owned()
    } else {
        trimmed
    }
}

fn unique_token() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("table-scale-export");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(".{file_name}.{}.part", unique_token()));
    if let Err(error) = std::fs::write(&temporary, bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Could not write temporary export: {error}"));
    }
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Could not finalize export: {error}"));
    }
    Ok(())
}

fn write_print_manifest(path: &Path, layout: &TileLayout, stem: &str) -> Result<(), String> {
    let mut text = String::new();
    text.push_str("TableScale 300 DPI PNG tile set\n");
    text.push_str("==================================\n\n");
    text.push_str("Print every tile at 100% / Actual Size. Never use Fit to Page.\n");
    text.push_str("PNG tiles contain the map content only; use borderless printing or the PDF export when your printer has fixed margins.\n");
    text.push_str(&format!(
        "Finished map: {:.2} x {:.2} in\n",
        layout.map_width_in, layout.map_height_in
    ));
    text.push_str(&format!(
        "Paper: {:.2} x {:.2} in; printable area: {:.2} x {:.2} in\n",
        layout.paper_width_in,
        layout.paper_height_in,
        layout.printable_width_in,
        layout.printable_height_in
    ));
    text.push_str(&format!(
        "Printer margin: {:.2} in; overlap: {:.2} in; sheets: {} x {} ({} pages)\n\n",
        layout.margin_in,
        layout.overlap_in,
        layout.columns,
        layout.rows,
        layout.page_count()
    ));
    text.push_str("Page | Row,Col | Tile size | File\n");
    text.push_str(
        "-----+---------+-----------+-----
",
    );
    for tile in &layout.tiles {
        let filename = tile_filename(tile, layout.page_count(), stem);
        text.push_str(&format!(
            "{:02} | {},{} | {:.2} x {:.2} in | {}\n",
            tile.page_number(),
            tile.row + 1,
            tile.column + 1,
            tile.width_in,
            tile.height_in,
            filename
        ));
    }
    text.push_str("\nAssemble pages in row-major order. Trim the printed cut border from PDF tiles; PNG tiles need no margin trim.\n");
    std::fs::write(path, text).map_err(|error| format!("Could not write PRINTING.txt: {error}"))
}

fn unique_tile_folder(parent: &Path, stem: &str) -> Result<PathBuf, String> {
    let base = format!("{}-300dpi-tiles", sanitize_stem(stem));
    let first = parent.join(&base);
    if !first.exists() {
        return Ok(first);
    }
    for index in 2..10_000 {
        let candidate = parent.join(format!("{base}-{index}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Could not find an unused tile folder name near {}.",
        first.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiler::{PaperPreset, SizeMode, TilerSettings};
    use image::{Rgb, RgbImage};

    fn tiny_source(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([40, 90, 120])))
    }

    #[test]
    fn pdf_has_a_valid_header_and_pages() {
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 9.0,
            exact_height_in: 12.0,
            paper: PaperPreset::Letter,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let source = tiny_source(900, 1200);
        let bytes = build_pdf_bytes(&layout, &source, true, |_| {}).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 1000);
        assert_eq!(layout.page_count(), 4);
    }

    #[test]
    fn every_page_content_area_stays_at_the_top_of_the_sheet() {
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 22.0,
            exact_height_in: 28.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let expected_top = (layout.paper_height_in - layout.margin_in) * POINTS_PER_INCH;
        for tile in &layout.tiles {
            let area = page_content_area(&layout, tile);
            assert!((area.top - expected_top as f32).abs() < 1e-4);
            assert!(
                (area.bottom - (expected_top - tile.height_in * POINTS_PER_INCH) as f32).abs()
                    < 1e-4
            );
            assert!((area.width - (tile.width_in * POINTS_PER_INCH) as f32).abs() < 1e-4);
            assert!((area.height - (tile.height_in * POINTS_PER_INCH) as f32).abs() < 1e-4);
        }
    }

    #[test]
    fn generated_pdf_has_correct_pages_and_letter_media_boxes() {
        let source = tiny_source(900, 1200);
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 22.0,
            exact_height_in: 28.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let bytes = build_pdf_bytes(&layout, &source, true, |_| {}).unwrap();
        let document = lopdf::Document::load_mem(&bytes).unwrap();
        let pages = document.get_pages();
        assert_eq!(pages.len(), 9);
        for page_id in pages.values() {
            let page = document.get_dictionary(*page_id).unwrap();
            let media_box = page.get(b"MediaBox").unwrap().as_array().unwrap();
            let values: Vec<f32> = media_box
                .iter()
                .map(|value| value.as_float().unwrap())
                .collect();
            assert!((values[2] - 612.0).abs() < 0.1);
            assert!((values[3] - 792.0).abs() < 0.1);
            let content = document.get_and_decode_page_content(*page_id).unwrap();
            assert!(
                !content.operations.is_empty(),
                "page {page_id:?} has no content operations"
            );
            let operators: Vec<&str> = content
                .operations
                .iter()
                .map(|operation| operation.operator.as_str())
                .collect();
            assert!(
                operators.contains(&"Do"),
                "page lacks an image draw operation"
            );
            assert!(operators.contains(&"W"), "page lacks a clip operation");
            assert!(operators.contains(&"n"), "page lacks a clip end operation");
        }
    }

    #[test]
    fn narrow_partial_pages_do_not_panic_when_guides_are_drawn() {
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 8.25,
            exact_height_in: 10.5,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        assert_eq!(layout.page_count(), 2);
        let source = tiny_source(825, 1050);
        let bytes = build_pdf_bytes(&layout, &source, true, |_| {}).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
    }

    #[test]
    fn png_tile_has_exact_300_dpi_pixel_dimensions() {
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 1.0,
            exact_height_in: 1.0,
            margin_in: 0.0,
            overlap_in: 0.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        assert_eq!(layout.page_count(), 1);
        let source = tiny_source(120, 120);
        let rgb = render_tile_rgb(&source, &layout, &layout.tiles[0], OUTPUT_DPI);
        assert_eq!(rgb.dimensions(), (300, 300));
    }

    #[test]
    fn png_files_are_numbered_and_dpi_tagged() {
        let temp = std::env::temp_dir().join(format!(
            "tablescale-png-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 1.0,
            exact_height_in: 1.0,
            margin_in: 0.0,
            overlap_in: 0.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let tile = &layout.tiles[0];
        let stem = sanitize_stem("sample map");
        let filename = tile_filename(tile, layout.page_count(), &stem);
        assert!(filename.starts_with("sample-map_page-01_of-01_r01_c01"));

        let image = RgbImage::from_pixel(300, 300, Rgb([1, 2, 3]));
        let path = temp.join("tile.png");
        write_png_file(&path, &image, OUTPUT_DPI).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().unwrap();
        let dims = reader.info().pixel_dims.unwrap();
        assert_eq!(dims.xppu, PIXELS_PER_METRE_AT_300_DPI);
        assert_eq!(dims.yppu, PIXELS_PER_METRE_AT_300_DPI);
        assert_eq!(dims.unit, Unit::Meter);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn png_manifest_contains_page_order_and_instructions() {
        let temp = std::env::temp_dir().join(format!(
            "tablescale-manifest-test-{}-{}",
            std::process::id(),
            unique_token()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 9.0,
            exact_height_in: 12.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let path = temp.join("PRINTING.txt");
        write_print_manifest(&path, &layout, "sample-map").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Print every tile at 100%"));
        assert!(text.contains("sample-map_page-01_of-04_r01_c01.png"));
        assert!(text.contains("Page | Row,Col | Tile size | File"));
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn atomic_file_writes_replace_existing_output() {
        let temp = std::env::temp_dir().join(format!(
            "tablescale-atomic-test-{}-{}",
            std::process::id(),
            unique_token()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let path = temp.join("output.pdf");
        std::fs::write(&path, b"old").unwrap();
        write_file_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let leftovers = std::fs::read_dir(&temp)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn png_export_job_creates_tiles_and_manifest() {
        let temp = std::env::temp_dir().join(format!(
            "tablescale-png-export-test-{}-{}",
            std::process::id(),
            unique_token()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let layout = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 1.0,
            exact_height_in: 1.0,
            margin_in: 0.0,
            overlap_in: 0.0,
            ..TilerSettings::default()
        }
        .layout()
        .unwrap();
        let job = start_png_export(
            temp.clone(),
            layout,
            Arc::new(tiny_source(120, 120)),
            "Sample Map".to_owned(),
        );
        let start = std::time::Instant::now();
        let result = loop {
            if let Some(result) = job.poll() {
                break result;
            }
            if start.elapsed() > std::time::Duration::from_secs(10) {
                panic!("PNG export job timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        .unwrap();
        assert_eq!(result.pages, 1);
        assert!(result.output.ends_with("sample-map-300dpi-tiles"));
        assert!(result.output.join("PRINTING.txt").is_file());
        let tiles: Vec<_> = std::fs::read_dir(&result.output)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "png")
            })
            .collect();
        assert_eq!(tiles.len(), 1);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(sanitize_stem("The Crypt: Level 2!"), "the-crypt-level-2");
        assert_eq!(sanitize_stem("***"), "battle-map");
        assert_eq!(sanitize_stem("Dungeon_01"), "dungeon-01");
        assert_eq!(sanitize_stem("地図"), "地図");
    }

    #[test]
    fn new_tile_folders_do_not_overwrite_existing_ones() {
        let temp = std::env::temp_dir().join(format!(
            "tablescale-folder-test-{}-{}",
            std::process::id(),
            unique_token()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::create_dir_all(temp.join("crypt-300dpi-tiles")).unwrap();
        let chosen = unique_tile_folder(&temp, "crypt").unwrap();
        assert_eq!(
            chosen.file_name().unwrap().to_str().unwrap(),
            "crypt-300dpi-tiles-2"
        );
        let _ = std::fs::remove_dir_all(temp);
    }
}
