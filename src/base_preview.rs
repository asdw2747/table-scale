//! Lightweight software 3D preview for generated bases.
//!
//! The preview projects the indexed mesh with an orbiting orthographic camera
//! and paints depth-sorted triangles through egui. No GPU scene graph or extra
//! renderer is required, which keeps the base editor available in the same
//! static binary as the 2D map tools.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, Ui, Vec2,
};
use glam::camera::rh::{proj::opengl as projection_rh, view::look_at_mat4};
use glam::{Mat4, Vec3};

use crate::stl_base::{BaseMesh, NameplateStyle};
use crate::theme;

const MIN_CANVAS_HEIGHT: f32 = 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaseCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub zoom: f32,
    pub pan: Vec2,
    pub wireframe: bool,
}

impl Default for BaseCamera {
    fn default() -> Self {
        Self {
            yaw: -45.0_f32.to_radians(),
            pitch: 26.0_f32.to_radians(),
            roll: 0.0,
            zoom: 1.16,
            pan: Vec2::ZERO,
            wireframe: false,
        }
    }
}

impl BaseCamera {
    pub fn reset_iso(&mut self) {
        let wireframe = self.wireframe;
        *self = Self::default();
        self.wireframe = wireframe;
    }

    pub fn reset_top(&mut self) {
        self.yaw = -90.0_f32.to_radians();
        self.pitch = 89.9_f32.to_radians();
        self.roll = 0.0;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    pub fn reset_bottom(&mut self) {
        self.yaw = -90.0_f32.to_radians();
        self.pitch = -89.9_f32.to_radians();
        self.roll = 0.0;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    pub fn reset_front(&mut self) {
        self.yaw = -90.0_f32.to_radians();
        self.pitch = 8.0_f32.to_radians();
        self.roll = 0.0;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }
}

struct Projection {
    matrix: Mat4,
    eye: Vec3,
    forward: Vec3,
    rect: Rect,
    pan: Vec2,
}

impl Projection {
    fn project(&self, point: Vec3) -> Pos2 {
        let clip = self.matrix * point.extend(1.0);
        if clip.w.abs() < 1e-6 {
            return self.rect.center();
        }
        let ndc = clip.truncate() / clip.w;
        Pos2::new(
            self.rect.center().x + self.pan.x + ndc.x * self.rect.width() * 0.5,
            self.rect.center().y + self.pan.y - ndc.y * self.rect.height() * 0.5,
        )
    }

    fn depth(&self, point: Vec3) -> f32 {
        (point - self.eye).dot(self.forward)
    }
}

fn make_projection(rect: Rect, mesh: &BaseMesh, camera: &BaseCamera) -> Projection {
    let center_d = (mesh.bounds_min + mesh.bounds_max) * 0.5;
    let center = Vec3::new(center_d.x as f32, center_d.y as f32, center_d.z as f32);
    let size = mesh.bounds_max - mesh.bounds_min;
    let radius = (size.length() * 0.5).max(1.0) as f32;
    let distance = radius * 5.0;
    let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
    let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
    let direction = Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch);
    let eye = center + direction * distance;
    let forward = (center - eye).normalize_or_zero();
    let mut right = forward.cross(Vec3::Z);
    if right.length_squared() < 1e-5 {
        right = Vec3::X;
    } else {
        right = right.normalize();
    }
    let mut up = right.cross(forward).normalize_or_zero();
    if camera.roll.abs() > 1e-5 {
        let (roll_sin, roll_cos) = camera.roll.sin_cos();
        up = up * roll_cos - right * roll_sin;
    }
    let aspect = (rect.width() / rect.height().max(1.0)).max(0.2);
    let half_height = radius * 0.95 / camera.zoom.clamp(0.2, 8.0);
    let half_width = half_height * aspect;
    let view = look_at_mat4(eye, center, up);
    let projection = projection_rh::orthographic(
        -half_width,
        half_width,
        -half_height,
        half_height,
        0.01,
        distance + radius * 6.0,
    );
    Projection {
        matrix: projection * view,
        eye,
        forward,
        rect,
        pan: camera.pan,
    }
}

/// Draw the interactive base viewport.
pub fn viewport(
    ui: &mut Ui,
    mesh: Option<&BaseMesh>,
    error: Option<&str>,
    camera: &mut BaseCamera,
) -> Response {
    let desired = Vec2::new(
        ui.available_width().max(240.0),
        ui.available_height().max(MIN_CANVAS_HEIGHT),
    );
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(12), theme::PREVIEW_BG);

    if let Some(mesh) = mesh {
        let projection = make_projection(rect, mesh, camera);
        paint_grid(&painter, &projection, mesh);
        if camera.wireframe {
            paint_wireframe(&painter, &projection, mesh);
        } else {
            paint_solid(&painter, &projection, mesh);
        }
        painter.text(
            rect.left_top() + Vec2::new(14.0, 12.0),
            Align2::LEFT_TOP,
            format!(
                "{:.1} \u{00d7} {:.1} \u{00d7} {:.1} mm  \u{00b7}  {} triangles",
                mesh.dimensions_mm().x,
                mesh.dimensions_mm().y,
                mesh.dimensions_mm().z,
                mesh.triangle_count()
            ),
            FontId::proportional(11.0),
            theme::DIM,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Generating preview\u{2026}",
            FontId::proportional(14.0),
            theme::MUTED,
        );
    }

    if let Some(error) = error {
        let error_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 12.0, rect.bottom() - 54.0),
            Pos2::new(rect.right() - 12.0, rect.bottom() - 12.0),
        );
        painter.rect_filled(
            error_rect,
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(0x4a, 0x1f, 0x24, 220),
        );
        painter.text(
            error_rect.center(),
            Align2::CENTER_CENTER,
            error,
            FontId::proportional(11.5),
            theme::ERROR,
        );
    } else {
        painter.text(
            rect.left_bottom() + Vec2::new(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            "DRAG ORBIT  \u{00b7}  SHIFT-DRAG PAN  \u{00b7}  SCROLL ZOOM  \u{00b7}  DOUBLE-CLICK RESET",
            FontId::proportional(10.0),
            theme::DIM,
        );
    }

    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );

    if response.dragged_by(PointerButton::Primary) {
        let delta = ui.input(|input| input.pointer.delta());
        if ui.input(|input| input.modifiers.shift) {
            camera.pan += delta;
        } else {
            camera.yaw -= delta.x * 0.008;
            camera.pitch = (camera.pitch + delta.y * 0.008)
                .clamp(-89.9_f32.to_radians(), 89.9_f32.to_radians());
        }
    }
    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.01 {
            camera.zoom = (camera.zoom * (scroll * 0.0015).exp()).clamp(0.25, 6.0);
        }
    }
    if response.double_clicked() {
        camera.reset_iso();
    }

    response
}

fn shade(normal: Vec3) -> Color32 {
    let key_light = Vec3::new(-0.45, -0.3, 0.84).normalize();
    let fill_light = Vec3::new(0.42, 0.28, -0.86).normalize();
    let key = normal.dot(key_light).clamp(0.0, 1.0);
    let fill = normal.dot(fill_light).clamp(0.0, 1.0);
    let rim = (1.0 - normal.z.abs()).powf(1.6) * 0.16;
    let value = 0.27 + 0.55 * key + 0.36 * fill + rim;
    let base = [0x7d, 0xa9, 0xac];
    let warm = [0xc9, 0xa5, 0x79];
    let mix = (normal.z.max(0.0) * 0.16).clamp(0.0, 1.0);
    let channels = [
        base[0] as f32 * (1.0 - mix) + warm[0] as f32 * mix,
        base[1] as f32 * (1.0 - mix) + warm[1] as f32 * mix,
        base[2] as f32 * (1.0 - mix) + warm[2] as f32 * mix,
    ];
    Color32::from_rgb(
        (channels[0] * value).clamp(0.0, 255.0) as u8,
        (channels[1] * value).clamp(0.0, 255.0) as u8,
        (channels[2] * value).clamp(0.0, 255.0) as u8,
    )
}

fn magnet_pocket(mesh: &BaseMesh, point: Vec3, normal: Vec3) -> bool {
    let Some(info) = mesh.features.magnet.as_ref() else {
        return false;
    };
    if normal.z > -0.9 {
        return false;
    }
    let dx = point.x as f64 - info.center.x;
    let dy = point.y as f64 - info.center.y;
    if dx * dx + dy * dy > info.effective_radius_mm * info.effective_radius_mm {
        return false;
    }
    ((point.z as f64) - info.depth_mm).abs() < 0.03
}

fn nameplate_highlighted(mesh: &BaseMesh, point: Vec3, normal: Vec3) -> bool {
    let Some(info) = mesh.features.nameplate.as_ref() else {
        return false;
    };
    if (point.x as f64) < info.bounds_min.x
        || (point.x as f64) > info.bounds_max.x
        || (point.y as f64) < info.bounds_min.y
        || (point.y as f64) > info.bounds_max.y
    {
        return false;
    }
    if normal.z > -0.9 {
        return false;
    }
    let target_z = match info.style {
        NameplateStyle::Engrave => info.depth_mm,
        NameplateStyle::Emboss => -info.depth_mm,
    };
    ((point.z as f64) - target_z).abs() < 0.03
}

fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn paint_solid(painter: &egui::Painter, projection: &Projection, mesh: &BaseMesh) {
    let mut triangles = Vec::with_capacity(mesh.triangles.len());
    for (triangle, normal) in mesh.triangles.iter().zip(&mesh.normals) {
        let a = mesh.vertices[triangle[0] as usize].as_vec3();
        let b = mesh.vertices[triangle[1] as usize].as_vec3();
        let c = mesh.vertices[triangle[2] as usize].as_vec3();
        let normal = normal.as_vec3().normalize_or_zero();
        if normal.dot(projection.forward) > -0.001 {
            continue;
        }
        let centroid = (a + b + c) / 3.0;
        let depth = projection.depth(centroid);
        triangles.push((
            depth,
            [
                projection.project(a),
                projection.project(b),
                projection.project(c),
            ],
            normal,
            centroid,
        ));
    }
    let mut colored = Vec::with_capacity(triangles.len());
    for (depth, points, normal, centroid) in triangles {
        let mut color = shade(normal);
        if nameplate_highlighted(mesh, centroid, normal) {
            color = mix_color(color, theme::ACCENT_BRIGHT, 0.42);
        } else if magnet_pocket(mesh, centroid, normal) {
            color = mix_color(color, Color32::from_rgb(0x0c, 0x0e, 0x12), 0.46);
        }
        colored.push((depth, points, color));
    }
    colored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut paint_mesh = egui::Mesh::default();
    paint_mesh.reserve_triangles(colored.len());
    for (_, points, color) in colored {
        let base = paint_mesh.vertices.len() as u32;
        for point in points {
            paint_mesh.colored_vertex(point, color);
        }
        paint_mesh.add_triangle(base, base + 1, base + 2);
    }
    painter.add(egui::Shape::mesh(paint_mesh));
}

fn paint_wireframe(painter: &egui::Painter, projection: &Projection, mesh: &BaseMesh) {
    let mut edges: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut lines: Vec<(f32, Pos2, Pos2, Color32)> = Vec::new();
    let mut min_depth = f32::INFINITY;
    let mut max_depth = f32::NEG_INFINITY;
    for (triangle, normal) in mesh.triangles.iter().zip(&mesh.normals) {
        let normal = normal.as_vec3().normalize_or_zero();
        if normal.dot(projection.forward) > -0.001 {
            continue;
        }
        for index in 0..3 {
            let a = triangle[index];
            let b = triangle[(index + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            if !edges.insert(key) {
                continue;
            }
            let pa = mesh.vertices[a as usize].as_vec3();
            let pb = mesh.vertices[b as usize].as_vec3();
            let depth = projection.depth((pa + pb) * 0.5);
            min_depth = min_depth.min(depth);
            max_depth = max_depth.max(depth);
            lines.push((
                depth,
                projection.project(pa),
                projection.project(pb),
                Color32::WHITE,
            ));
        }
    }
    let span = (max_depth - min_depth).max(1e-4);
    for (depth, a, b, _) in lines {
        let t = 1.0 - ((depth - min_depth) / span).clamp(0.0, 1.0);
        let color = Color32::from_rgba_unmultiplied(
            (0x6f as f32 + 0x45 as f32 * t) as u8,
            (0xb3 as f32 + 0x35 as f32 * t) as u8,
            (0xb8 as f32 + 0x2a as f32 * t) as u8,
            150,
        );
        painter.line_segment([a, b], Stroke::new(1.0, color));
    }
}

fn paint_grid(painter: &egui::Painter, projection: &Projection, mesh: &BaseMesh) {
    let center_d = (mesh.bounds_min + mesh.bounds_max) * 0.5;
    let center = Vec3::new(center_d.x as f32, center_d.y as f32, center_d.z as f32);
    let size = mesh.bounds_max - mesh.bounds_min;
    let extent = (size.x.max(size.y) * 2.4).max(20.0) as f32;
    let z = mesh.bounds_min.z as f32 - 0.02;
    let spacing = nice_step(extent / 10.0);
    let count = (extent / spacing).ceil() as i32;
    let color = Color32::from_rgba_unmultiplied(0x6f, 0xb3, 0xb8, 22);
    let strong = Color32::from_rgba_unmultiplied(0x6f, 0xb3, 0xb8, 42);
    for index in -count..=count {
        let offset = index as f32 * spacing;
        let a = Vec3::new(center.x - extent, center.y + offset, z);
        let b = Vec3::new(center.x + extent, center.y + offset, z);
        painter.line_segment(
            [projection.project(a), projection.project(b)],
            Stroke::new(1.0, color),
        );
        let c = Vec3::new(center.x + offset, center.y - extent, z);
        let d = Vec3::new(center.x + offset, center.y + extent, z);
        painter.line_segment(
            [projection.project(c), projection.project(d)],
            Stroke::new(1.0, color),
        );
    }
    let x0 = Vec3::new(center.x - extent, center.y, z);
    let x1 = Vec3::new(center.x + extent, center.y, z);
    let y0 = Vec3::new(center.x, center.y - extent, z);
    let y1 = Vec3::new(center.x, center.y + extent, z);
    painter.line_segment(
        [projection.project(x0), projection.project(x1)],
        Stroke::new(1.0, strong),
    );
    painter.line_segment(
        [projection.project(y0), projection.project(y1)],
        Stroke::new(1.0, strong),
    );
}

fn nice_step(raw: f32) -> f32 {
    let base = 10.0_f32.powf(raw.max(1e-3).log10().floor());
    let normalized = raw / base;
    let step = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    base * step
}
