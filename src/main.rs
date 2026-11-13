#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod export;
mod scale;
mod theme;
mod tiler;

use eframe::egui;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("TableScale")
        .with_app_id("table-scale")
        .with_inner_size([1180.0, 720.0])
        .with_min_inner_size([900.0, 560.0])
        .with_icon(load_icon());

    let native_options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };

    let result = eframe::run_native(
        "TableScale",
        native_options,
        Box::new(|cc| Ok(Box::new(app::TableScaleApp::new(cc)))),
    );

    if let Err(error) = &result {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("TableScale could not start")
            .set_description(format!(
                "TableScale failed to open its window.\n\n{error}\n\nA working OpenGL driver is required."
            ))
            .show();
    }

    result
}

fn load_icon() -> egui::IconData {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");
    if let Ok(image) = image::load_from_memory(ICON_BYTES) {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        return egui::IconData {
            rgba: rgba.into_raw(),
            width,
            height,
        };
    }
    fallback_icon()
}

/// A deterministic fallback in case the bundled PNG cannot be decoded.
fn fallback_icon() -> egui::IconData {
    const SIZE: usize = 256;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let mut put = |x: usize, y: usize, color: [u8; 4]| {
        let index = (y * SIZE + x) * 4;
        rgba[index..index + 4].copy_from_slice(&color);
    };

    let background = [0x16, 0x19, 0x20, 255];
    let border = [0x3a, 0x40, 0x4e, 255];
    let teal = [0x6f, 0xb3, 0xb8, 110];
    let brass = [0xe0, 0xa4, 0x58, 255];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let inside = rounded_rect_contains(x as f32, y as f32, 10.0, 10.0, 246.0, 246.0, 54.0);
            if inside {
                put(x, y, background);
            }
            let near_border =
                rounded_rect_contains(x as f32, y as f32, 10.0, 10.0, 246.0, 246.0, 54.0)
                    && !rounded_rect_contains(x as f32, y as f32, 16.0, 16.0, 240.0, 240.0, 49.0);
            if near_border {
                put(x, y, border);
            }
        }
    }

    for position in [92usize, 164] {
        for offset in 0..172 {
            put(52 + offset, position, teal);
            put(position, 52 + offset, teal);
        }
    }

    let center = 128.0_f32;
    let radius = 74.0_f32;
    for angle_step in 0..720 {
        let angle = angle_step as f32 * std::f32::consts::PI / 360.0;
        let x = center + radius * angle.cos();
        let y = center + radius * angle.sin();
        stamp(&mut put, x as i32, y as i32, brass);
    }
    for angle_step in 0..720 {
        let angle = angle_step as f32 * std::f32::consts::PI / 360.0;
        let x = center + radius * 0.58 * (angle + std::f32::consts::FRAC_PI_4).cos();
        let y = center + radius * 0.58 * (angle + std::f32::consts::FRAC_PI_4).sin();
        stamp(&mut put, x as i32, y as i32, brass);
    }
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if dx * dx + dy * dy <= 12.0 * 12.0 {
                put(x, y, teal);
            }
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

fn stamp(put: &mut impl FnMut(usize, usize, [u8; 4]), x: i32, y: i32, color: [u8; 4]) {
    for dy in -1..=1 {
        for dx in -1..=1 {
            let px = x + dx;
            let py = y + dy;
            if (0..256).contains(&px) && (0..256).contains(&py) {
                put(px as usize, py as usize, color);
            }
        }
    }
}

fn rounded_rect_contains(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    radius: f32,
) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}
