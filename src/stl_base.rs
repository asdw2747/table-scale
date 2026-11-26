//! Parametric miniature base geometry, watertight validation, and binary STL export.
//!
//! The mesh is built from indexed rings and triangulated planar faces. Text is
//! converted from embedded font outlines into bottom-face holes and integrated
//! cavity/protrusion walls, then the whole shell is orientation-normalized and
//! checked for manifold edges before it can be exported.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::Path;

use ab_glyph::{Font, FontRef, OutlineCurve};
use glam::{DVec2, DVec3};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use spade::handles::FixedVertexHandle;
use spade::{ConstrainedDelaunayTriangulation, HasPosition, Point2, Triangulation};

const EPSILON: f64 = 1e-7;
const MERGE_EPSILON: f64 = 1e-6;
const WELD_SCALE: f64 = 1_000_000.0;
const CORNER_SEGMENTS: usize = 10;
const CIRCLE_SEGMENTS: usize = 128;
const TEXT_CURVE_STEPS: usize = 8;
pub const FONT_BYTES: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseShape {
    Circle,
    Pill,
    RoundedSquare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameplateStyle {
    Engrave,
    Emboss,
}

impl NameplateStyle {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Engrave => "Engrave",
            Self::Emboss => "Emboss",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MagnetSpec {
    pub diameter_mm: f64,
    pub depth_mm: f64,
    pub radial_tolerance_mm: f64,
}

impl MagnetSpec {
    pub fn effective_radius_mm(&self) -> f64 {
        (self.diameter_mm + 2.0 * self.radial_tolerance_mm) * 0.5
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopRecessSpec {
    pub rim_width_mm: f64,
    pub depth_mm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NameplateSpec {
    pub text: String,
    pub style: NameplateStyle,
    pub depth_mm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BaseParams {
    pub shape: BaseShape,
    pub width_mm: f64,
    pub depth_mm: f64,
    pub height_mm: f64,
    pub bevel_width_mm: f64,
    pub bevel_angle_deg: f64,
    pub corner_radius_mm: f64,
    pub top_recess: Option<TopRecessSpec>,
    pub magnet: Option<MagnetSpec>,
    pub nameplate: Option<NameplateSpec>,
}

impl Default for BaseParams {
    fn default() -> Self {
        Self {
            shape: BaseShape::Circle,
            width_mm: 25.0,
            depth_mm: 25.0,
            height_mm: 3.5,
            bevel_width_mm: 1.5,
            bevel_angle_deg: 45.0,
            corner_radius_mm: 5.0,
            top_recess: None,
            magnet: Some(MagnetSpec {
                diameter_mm: 5.0,
                depth_mm: 2.0,
                radial_tolerance_mm: 0.15,
            }),
            nameplate: Some(NameplateSpec {
                text: "Magneto".to_owned(),
                style: NameplateStyle::Engrave,
                depth_mm: 0.4,
            }),
        }
    }
}

impl BaseParams {
    pub fn with_name(&self, name: impl Into<String>) -> Self {
        let mut next = self.clone();
        if let Some(nameplate) = &mut next.nameplate {
            nameplate.text = name.into();
        }
        next
    }

    pub fn bevel_height_mm(&self) -> f64 {
        if self.bevel_width_mm <= EPSILON {
            0.0
        } else {
            self.bevel_width_mm * self.bevel_angle_deg.to_radians().tan()
        }
    }

    pub fn min_dimension_mm(&self) -> f64 {
        self.width_mm.min(self.depth_mm)
    }

    pub fn is_round(&self) -> bool {
        self.shape == BaseShape::Circle && (self.width_mm - self.depth_mm).abs() < 1e-4
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.width_mm.is_finite()
            || !self.depth_mm.is_finite()
            || self.width_mm < 5.0
            || self.depth_mm < 5.0
            || self.width_mm > 200.0
            || self.depth_mm > 200.0
        {
            return Err("Base width and depth must be between 5 and 200 mm.".to_owned());
        }
        if !self.height_mm.is_finite() || self.height_mm < 1.0 || self.height_mm > 50.0 {
            return Err("Base height must be between 1 and 50 mm.".to_owned());
        }
        if !self.bevel_width_mm.is_finite()
            || self.bevel_width_mm < 0.0
            || self.bevel_width_mm > self.min_dimension_mm() * 0.3
        {
            return Err("Bevel width is outside the safe range for this base.".to_owned());
        }
        if !self.bevel_angle_deg.is_finite()
            || self.bevel_angle_deg < 1.0
            || self.bevel_angle_deg > 80.0
        {
            return Err("Bevel angle must be between 1 and 80 degrees.".to_owned());
        }
        let bevel_height = self.bevel_height_mm();
        if bevel_height >= self.height_mm - 0.4 {
            return Err("The bevel is taller than the base body.".to_owned());
        }
        if self.shape == BaseShape::RoundedSquare {
            let max_radius = self.min_dimension_mm() * 0.45;
            if !self.corner_radius_mm.is_finite()
                || self.corner_radius_mm < 0.2
                || self.corner_radius_mm > max_radius
            {
                return Err(format!(
                    "Rounded-square corner radius must be between 0.2 and {max_radius:.2} mm."
                ));
            }
        }
        // Validate the top contour early. This catches bevel/lip combinations
        // that would collapse small rounded shapes.
        shape_ring(self, self.bevel_width_mm)?;
        if let Some(recess) = &self.top_recess {
            if !recess.rim_width_mm.is_finite() || recess.rim_width_mm < 0.2 {
                return Err("Lip rim width must be at least 0.2 mm.".to_owned());
            }
            if !recess.depth_mm.is_finite()
                || recess.depth_mm <= 0.0
                || recess.depth_mm >= self.height_mm - bevel_height - 0.3
            {
                return Err("Lip depth is outside the safe range.".to_owned());
            }
            shape_ring(self, self.bevel_width_mm + recess.rim_width_mm)?;
        }
        if let Some(magnet) = &self.magnet {
            if !magnet.diameter_mm.is_finite()
                || !magnet.depth_mm.is_finite()
                || !magnet.radial_tolerance_mm.is_finite()
                || magnet.diameter_mm <= 0.0
                || magnet.depth_mm <= 0.0
                || magnet.radial_tolerance_mm < 0.0
            {
                return Err("Magnet diameter, depth, and tolerance must be positive.".to_owned());
            }
            if magnet.effective_radius_mm() * 2.0 >= self.min_dimension_mm() - 1.6 {
                return Err("The magnet pocket is too large for this base.".to_owned());
            }
            let top_clearance = self
                .top_recess
                .as_ref()
                .map(|recess| recess.depth_mm)
                .unwrap_or(0.0);
            if magnet.depth_mm >= self.height_mm - top_clearance - 0.5 {
                return Err("The magnet pocket is too deep for this base.".to_owned());
            }
        }
        if let Some(nameplate) = &self.nameplate {
            if nameplate.text.trim().is_empty() {
                return Err("Enter nameplate text or disable the nameplate.".to_owned());
            }
            if nameplate.text.chars().count() > 32 {
                return Err("Nameplate text is limited to 32 characters.".to_owned());
            }
            if !nameplate.depth_mm.is_finite()
                || nameplate.depth_mm < 0.1
                || nameplate.depth_mm > 1.2
            {
                return Err("Nameplate depth must be between 0.1 and 1.2 mm.".to_owned());
            }
            if nameplate.depth_mm >= self.height_mm - 0.5 {
                return Err("Nameplate depth is too large for this base.".to_owned());
            }
        }
        Ok(())
    }

    /// Safe filename stem for the generated STL.
    pub fn file_name(&self, character_name: &str) -> String {
        let name = sanitize_name(character_name);
        let size = if self.is_round() {
            format_mm(self.width_mm)
        } else {
            format!("{}x{}", format_mm(self.width_mm), format_mm(self.depth_mm))
        };
        format!("{name}_{size}mm.stl")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NameplatePlacement {
    Line {
        baseline_y_mm: f64,
        width_mm: f64,
        font_size_mm: f64,
    },
    Arc {
        radius_mm: f64,
        span_radians: f64,
        font_size_mm: f64,
    },
}

impl NameplatePlacement {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Line { .. } => "Below-center line",
            Self::Arc { .. } => "Bottom arc",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NameplateInfo {
    pub style: NameplateStyle,
    pub placement: NameplatePlacement,
    pub font_size_mm: f64,
    pub depth_mm: f64,
    pub bounds_min: DVec2,
    pub bounds_max: DVec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MagnetInfo {
    pub center: DVec2,
    pub effective_radius_mm: f64,
    pub depth_mm: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshFeatures {
    pub magnet: Option<MagnetInfo>,
    pub nameplate: Option<NameplateInfo>,
}

#[derive(Clone, Debug)]
pub struct BaseMesh {
    pub vertices: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
    pub normals: Vec<DVec3>,
    pub bounds_min: DVec3,
    pub bounds_max: DVec3,
    pub watertight: bool,
    pub features: MeshFeatures,
}

impl BaseMesh {
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    pub fn dimensions_mm(&self) -> DVec3 {
        self.bounds_max - self.bounds_min
    }

    /// Enclosed volume in cubic millimetres.
    pub fn volume_mm3(&self) -> f64 {
        self.triangles
            .iter()
            .map(|triangle| signed_triangle_volume(&self.vertices, *triangle))
            .sum::<f64>()
            .abs()
    }

    pub fn validate_topology(&self) -> Result<(), String> {
        validate_manifold(&self.vertices, &self.triangles)
    }
}

// ---------------------------------------------------------------------------
// Outline geometry
// ---------------------------------------------------------------------------

fn shape_ring(params: &BaseParams, inset: f64) -> Result<Vec<DVec2>, String> {
    if inset < 0.0 {
        return Err("Inset cannot be negative.".to_owned());
    }
    match params.shape {
        BaseShape::Circle => {
            let rx = params.width_mm * 0.5 - inset;
            let ry = params.depth_mm * 0.5 - inset;
            if rx <= 0.05 || ry <= 0.05 {
                return Err("The selected rim/lip leaves no top surface.".to_owned());
            }
            Ok((0..CIRCLE_SEGMENTS)
                .map(|index| {
                    let angle = std::f64::consts::TAU * index as f64 / CIRCLE_SEGMENTS as f64;
                    DVec2::new(rx * angle.cos(), ry * angle.sin())
                })
                .collect())
        }
        BaseShape::Pill => {
            let width = params.width_mm - 2.0 * inset;
            let depth = params.depth_mm - 2.0 * inset;
            if width <= 0.1 || depth <= 0.1 {
                return Err("The selected rim/lip leaves no top surface.".to_owned());
            }
            let radius = width.min(depth) * 0.5;
            Ok(rounded_rect_ring(width, depth, radius))
        }
        BaseShape::RoundedSquare => {
            let width = params.width_mm - 2.0 * inset;
            let depth = params.depth_mm - 2.0 * inset;
            let radius = params.corner_radius_mm - inset;
            if width <= 0.1 || depth <= 0.1 || radius <= 0.05 {
                return Err("The selected rim/lip is too wide for the corner radius.".to_owned());
            }
            Ok(rounded_rect_ring(width, depth, radius))
        }
    }
}

fn rounded_rect_ring(width: f64, depth: f64, radius: f64) -> Vec<DVec2> {
    let r = radius.min(width * 0.5).min(depth * 0.5);
    let corners = [
        (width * 0.5 - r, depth * 0.5 - r, 0.0_f64),
        (
            -width * 0.5 + r,
            depth * 0.5 - r,
            std::f64::consts::FRAC_PI_2,
        ),
        (-width * 0.5 + r, -depth * 0.5 + r, std::f64::consts::PI),
        (
            width * 0.5 - r,
            -depth * 0.5 + r,
            std::f64::consts::PI * 1.5,
        ),
    ];
    let mut points = Vec::with_capacity(4 * (CORNER_SEGMENTS + 1));
    for (cx, cy, start) in corners {
        for step in 0..=CORNER_SEGMENTS {
            let angle = start + std::f64::consts::FRAC_PI_2 * step as f64 / CORNER_SEGMENTS as f64;
            points.push(DVec2::new(cx + r * angle.cos(), cy + r * angle.sin()));
        }
    }
    points
}

fn point_in_polygon(point: DVec2, ring: &[DVec2]) -> bool {
    let mut inside = false;
    let mut previous = ring.len().saturating_sub(1);
    for index in 0..ring.len() {
        let a = ring[previous];
        let b = ring[index];
        if (a.y > point.y) != (b.y > point.y) {
            let denominator = b.y - a.y;
            if denominator.abs() > EPSILON {
                let x = (b.x - a.x) * (point.y - a.y) / denominator + a.x;
                if point.x < x {
                    inside = !inside;
                }
            }
        }
        previous = index;
    }
    inside
}

// ---------------------------------------------------------------------------
// Mesh building and manifold validation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
    lookup: HashMap<(i64, i64, i64), u32>,
}

impl MeshBuilder {
    fn vertex(&mut self, point: DVec3) -> u32 {
        let key = (
            (point.x * WELD_SCALE).round() as i64,
            (point.y * WELD_SCALE).round() as i64,
            (point.z * WELD_SCALE).round() as i64,
        );
        if let Some(index) = self.lookup.get(&key) {
            return *index;
        }
        let index = self.vertices.len() as u32;
        self.vertices.push(point);
        self.lookup.insert(key, index);
        index
    }

    fn ring(&mut self, points: &[DVec2], z: f64) -> Vec<u32> {
        points
            .iter()
            .map(|point| self.vertex(DVec3::new(point.x, point.y, z)))
            .collect()
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        if a == b || b == c || c == a {
            return;
        }
        let area = (self.vertices[b as usize] - self.vertices[a as usize])
            .cross(self.vertices[c as usize] - self.vertices[a as usize])
            .length_squared();
        if area > 1e-18 {
            self.triangles.push([a, b, c]);
        }
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }

    fn wall(&mut self, low: &[u32], high: &[u32]) {
        if low.len() != high.len() || low.len() < 3 {
            return;
        }
        for index in 0..low.len() {
            let next = (index + 1) % low.len();
            self.quad(low[index], low[next], high[next], high[index]);
        }
    }

    fn face(&mut self, outer: &[u32]) {
        let _ = self.face_with_holes(outer, &[]);
    }

    fn face_with_holes(&mut self, outer: &[u32], holes: &[Vec<u32>]) -> Result<(), String> {
        if outer.len() < 3 {
            return Ok(());
        }
        let mut rings: Vec<Vec<u32>> = Vec::with_capacity(holes.len() + 1);
        rings.push(normalized_ring(outer, &self.vertices, true));
        for hole in holes {
            rings.push(normalized_ring(hole, &self.vertices, false));
        }
        for ring in &mut rings {
            clean_ring(ring);
        }
        if rings[0].len() < 3 {
            return Ok(());
        }
        rings.retain(|ring| ring.len() >= 3);
        let outer_coords = ring_coords(&self.vertices, &rings[0]);
        let hole_coords: Vec<Vec<DVec2>> = rings
            .iter()
            .skip(1)
            .map(|ring| ring_coords(&self.vertices, ring))
            .collect();

        // A constrained Delaunay triangulation avoids the fragile hole-bridge
        // construction used by ear clipping. Text glyphs commonly share exact
        // baselines and cap heights, which can make bridge edges collinear with
        // cavity boundaries; spade inserts the contours as constraints instead
        // and keeps every boundary vertex.
        let mut cdt = ConstrainedDelaunayTriangulation::<TriPoint>::default();
        let mut handles: Vec<Option<FixedVertexHandle>> = vec![None; self.vertices.len()];
        for ring in &rings {
            for id in ring {
                if handles[*id as usize].is_none() {
                    let point = self.vertices[*id as usize];
                    let vertex = TriPoint {
                        id: *id,
                        position: Point2::new(point.x, point.y),
                    };
                    let handle = cdt
                        .insert(vertex)
                        .map_err(|error| format!("Could not triangulate a face: {error}"))?;
                    handles[*id as usize] = Some(handle);
                }
            }
        }
        for ring in &rings {
            for index in 0..ring.len() {
                let from = handles[ring[index] as usize]
                    .ok_or_else(|| "Missing triangulation vertex.".to_owned())?;
                let to = handles[ring[(index + 1) % ring.len()] as usize]
                    .ok_or_else(|| "Missing triangulation vertex.".to_owned())?;
                if from == to {
                    continue;
                }
                if !cdt.can_add_constraint(from, to) {
                    return Err(
                        "A generated face has self-intersecting or touching contours.".to_owned(),
                    );
                }
                cdt.add_constraint(from, to);
            }
        }

        let first_triangle = self.triangles.len();
        for face in cdt.inner_faces() {
            let vertices = face.vertices();
            let ids = [
                vertices[0].data().id,
                vertices[1].data().id,
                vertices[2].data().id,
            ];
            let a = self.vertices[ids[0] as usize];
            let b = self.vertices[ids[1] as usize];
            let c = self.vertices[ids[2] as usize];
            let centroid = DVec2::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
            if !point_in_polygon(centroid, &outer_coords) {
                continue;
            }
            if hole_coords
                .iter()
                .any(|hole| point_in_polygon(centroid, hole))
            {
                continue;
            }
            self.triangle(ids[0], ids[1], ids[2]);
        }

        verify_face_boundary(&self.vertices, &self.triangles[first_triangle..], &rings)
    }

    fn finish(mut self, features: MeshFeatures) -> Result<BaseMesh, String> {
        orient_and_validate(&mut self.vertices, &mut self.triangles)?;
        let (bounds_min, bounds_max) = bounds(&self.vertices)?;
        let normals = self
            .triangles
            .iter()
            .map(|triangle| {
                let a = self.vertices[triangle[0] as usize];
                let b = self.vertices[triangle[1] as usize];
                let c = self.vertices[triangle[2] as usize];
                (b - a).cross(c - a).normalize_or_zero()
            })
            .collect();
        validate_manifold(&self.vertices, &self.triangles)?;
        Ok(BaseMesh {
            vertices: self.vertices,
            triangles: self.triangles,
            normals,
            bounds_min,
            bounds_max,
            watertight: true,
            features,
        })
    }
}

fn normalized_ring(ring: &[u32], vertices: &[DVec3], want_ccw: bool) -> Vec<u32> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let area = signed_area_3d(ring, vertices);
    let is_ccw = area > 0.0;
    if is_ccw == want_ccw {
        ring.to_vec()
    } else {
        let mut reversed = ring.to_vec();
        reversed.reverse();
        reversed
    }
}

fn signed_area_3d(ring: &[u32], vertices: &[DVec3]) -> f64 {
    let mut area = 0.0;
    for index in 0..ring.len() {
        let a = vertices[ring[index] as usize];
        let b = vertices[ring[(index + 1) % ring.len()] as usize];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

fn clean_ring(ring: &mut Vec<u32>) {
    let mut index = 1;
    while index < ring.len() {
        if ring[index] == ring[index - 1] {
            ring.remove(index);
        } else {
            index += 1;
        }
    }
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
}

fn ring_coords(vertices: &[DVec3], ring: &[u32]) -> Vec<DVec2> {
    ring.iter()
        .map(|id| {
            let point = vertices[*id as usize];
            DVec2::new(point.x, point.y)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct TriPoint {
    id: u32,
    position: Point2<f64>,
}

impl HasPosition for TriPoint {
    type Scalar = f64;

    fn position(&self) -> Point2<f64> {
        self.position
    }
}

fn verify_face_boundary(
    vertices: &[DVec3],
    triangles: &[[u32; 3]],
    rings: &[Vec<u32>],
) -> Result<(), String> {
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for triangle in triangles {
        for index in 0..3 {
            *counts
                .entry(edge_key(triangle[index], triangle[(index + 1) % 3]))
                .or_default() += 1;
        }
    }
    for ring in rings {
        for index in 0..ring.len() {
            let a = ring[index];
            let b = ring[(index + 1) % ring.len()];
            let count = counts.get(&edge_key(a, b)).copied().unwrap_or(0);
            if count != 1 {
                let pa = vertices[a as usize];
                let pb = vertices[b as usize];
                return Err(format!(
                    "Generated face boundary is not closed: edge {pa:?} -> {pb:?} has {count} incident face triangles."
                ));
            }
        }
    }
    Ok(())
}

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn orient_and_validate(vertices: &mut [DVec3], triangles: &mut [[u32; 3]]) -> Result<(), String> {
    let mut edges: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
    for (triangle_index, triangle) in triangles.iter().enumerate() {
        for index in 0..3 {
            let a = triangle[index];
            let b = triangle[(index + 1) % 3];
            let same_direction = a < b;
            edges
                .entry(edge_key(a, b))
                .or_default()
                .push((triangle_index, same_direction));
        }
    }
    for (edge, uses) in &edges {
        if uses.len() != 2 {
            let a = vertices[edge.0 as usize];
            let b = vertices[edge.1 as usize];
            return Err(format!(
                "Mesh is not watertight: edge {:?} ({a:?} -> {b:?}) has {} incident faces.",
                edge,
                uses.len()
            ));
        }
    }

    let mut adjacency = vec![Vec::<(usize, bool)>::new(); triangles.len()];
    for uses in edges.values() {
        if let [(first, first_dir), (second, second_dir)] = uses.as_slice() {
            let same_direction = first_dir == second_dir;
            adjacency[*first].push((*second, same_direction));
            adjacency[*second].push((*first, same_direction));
        }
    }

    let mut flips = vec![None; triangles.len()];
    let mut components = Vec::new();
    for start in 0..triangles.len() {
        if flips[start].is_some() {
            continue;
        }
        flips[start] = Some(false);
        let mut queue = VecDeque::from([start]);
        let mut component = Vec::new();
        while let Some(current) = queue.pop_front() {
            component.push(current);
            let current_flip = flips[current].unwrap_or(false);
            for (neighbor, parity) in &adjacency[current] {
                let required = current_flip ^ *parity;
                match flips[*neighbor] {
                    Some(existing) if existing != required => {
                        return Err("Mesh has a non-orientable face cycle.".to_owned());
                    }
                    Some(_) => {}
                    None => {
                        flips[*neighbor] = Some(required);
                        queue.push_back(*neighbor);
                    }
                }
            }
        }
        components.push(component);
    }

    for (triangle, should_flip) in triangles.iter_mut().zip(flips.iter()) {
        if should_flip.unwrap_or(false) {
            triangle.swap(1, 2);
        }
    }

    // Recheck directions after the parity propagation.
    let mut oriented_edges: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
    for (triangle_index, triangle) in triangles.iter().enumerate() {
        for index in 0..3 {
            let a = triangle[index];
            let b = triangle[(index + 1) % 3];
            oriented_edges
                .entry(edge_key(a, b))
                .or_default()
                .push((triangle_index, a < b));
        }
    }
    for uses in oriented_edges.values() {
        if uses.len() != 2 || uses[0].1 == uses[1].1 {
            return Err("Mesh orientation could not be made consistent.".to_owned());
        }
    }

    // Flip whole connected components if their signed volume is negative.
    for component in components {
        let volume: f64 = component
            .iter()
            .map(|index| signed_triangle_volume(vertices, triangles[*index]))
            .sum();
        if volume.abs() < 1e-9 {
            return Err("Mesh contains a zero-volume shell.".to_owned());
        }
        if volume < 0.0 {
            for index in component {
                triangles[index].swap(1, 2);
            }
        }
    }
    Ok(())
}

fn signed_triangle_volume(vertices: &[DVec3], triangle: [u32; 3]) -> f64 {
    let a = vertices[triangle[0] as usize];
    let b = vertices[triangle[1] as usize];
    let c = vertices[triangle[2] as usize];
    a.dot(b.cross(c)) / 6.0
}

fn validate_manifold(vertices: &[DVec3], triangles: &[[u32; 3]]) -> Result<(), String> {
    let mut edges: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::new();
    for (triangle_index, triangle) in triangles.iter().enumerate() {
        if triangle[0] == triangle[1] || triangle[1] == triangle[2] || triangle[2] == triangle[0] {
            return Err("Mesh contains a degenerate triangle.".to_owned());
        }
        for index in 0..3 {
            let a = triangle[index];
            let b = triangle[(index + 1) % 3];
            edges
                .entry(edge_key(a, b))
                .or_default()
                .push((triangle_index, a < b));
        }
    }
    for (edge, uses) in edges {
        if uses.len() != 2 {
            let a = vertices[edge.0 as usize];
            let b = vertices[edge.1 as usize];
            return Err(format!(
                "Mesh is not watertight: edge {:?} ({a:?} -> {b:?}) has {} incident faces.",
                edge,
                uses.len()
            ));
        }
        if uses[0].1 == uses[1].1 {
            return Err("Mesh contains an inconsistently oriented edge.".to_owned());
        }
    }
    Ok(())
}

fn bounds(vertices: &[DVec3]) -> Result<(DVec3, DVec3), String> {
    let Some(first) = vertices.first() else {
        return Err("Mesh contains no vertices.".to_owned());
    };
    let mut min = *first;
    let mut max = *first;
    for vertex in vertices.iter().skip(1) {
        min = min.min(*vertex);
        max = max.max(*vertex);
    }
    Ok((min, max))
}

// ---------------------------------------------------------------------------
// Nameplate outline layout
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct GlyphComponent {
    outer: Vec<DVec2>,
    holes: Vec<Vec<DVec2>>,
}

#[derive(Clone, Debug)]
struct TextLayout {
    components: Vec<GlyphComponent>,
    placement: NameplatePlacement,
    bounds_min: DVec2,
    bounds_max: DVec2,
}

fn union_components(components: &[GlyphComponent]) -> Result<Vec<GlyphComponent>, String> {
    if components.is_empty() {
        return Ok(Vec::new());
    }
    let mut contours: Vec<Vec<[f64; 2]>> = Vec::new();
    for component in components {
        let mut outer = component.outer.clone();
        orient_ring(&mut outer, false);
        contours.push(outer.iter().map(|point| [point.x, point.y]).collect());
        for hole in &component.holes {
            let mut hole = hole.clone();
            orient_ring(&mut hole, true);
            contours.push(hole.iter().map(|point| [point.x, point.y]).collect());
        }
    }
    let mut overlay = FloatOverlay::<[f64; 2]>::from_subj(&contours);
    let shapes = overlay.overlay(OverlayRule::Subject, FillRule::NonZero);
    let mut result = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let mut contours = shape.into_iter();
        let Some(outer) = contours.next() else {
            continue;
        };
        let outer = outer
            .into_iter()
            .map(|point| DVec2::new(point[0], point[1]))
            .collect::<Vec<_>>();
        if outer.len() < 3 {
            continue;
        }
        let holes = contours
            .filter_map(|hole| {
                let hole = hole
                    .into_iter()
                    .map(|point| DVec2::new(point[0], point[1]))
                    .collect::<Vec<_>>();
                (hole.len() >= 3).then_some(hole)
            })
            .collect();
        result.push(GlyphComponent { outer, holes });
    }
    if result.is_empty() {
        return Err("The nameplate outline collapsed while merging letter shapes.".to_owned());
    }
    Ok(result)
}

fn orient_ring(ring: &mut [DVec2], clockwise: bool) {
    let area = signed_area(ring);
    let currently_clockwise = area < 0.0;
    if currently_clockwise != clockwise {
        ring.reverse();
    }
}

fn layout_nameplate(
    params: &BaseParams,
    nameplate: &NameplateSpec,
) -> Result<Option<TextLayout>, String> {
    if nameplate.text.trim().is_empty() {
        return Ok(None);
    }
    let font = FontRef::try_from_slice(FONT_BYTES)
        .map_err(|error| format!("Could not parse the embedded nameplate font: {error}"))?;
    let min_dim = params.min_dimension_mm();
    let max_font = (min_dim * 0.18).clamp(1.2, 4.0);
    let min_font = 1.0;
    let mut size = max_font;
    while size >= min_font - 1e-9 {
        let Some(local) = layout_local_text(&font, &nameplate.text, params, size) else {
            return Err("The embedded font could not outline the nameplate text.".to_owned());
        };
        let mut result = None;
        if params.is_round() {
            if let Some(layout) = place_arc(params, &local, size) {
                result = Some(layout);
            }
        }
        if result.is_none() {
            result = place_line(params, &local, size);
        }
        if let Some(mut layout) = result {
            layout.components = union_components(&layout.components)?;
            let (bounds_min, bounds_max) = component_bounds(&layout.components)
                .ok_or_else(|| "The nameplate outline has no area.".to_owned())?;
            layout.bounds_min = bounds_min;
            layout.bounds_max = bounds_max;
            return Ok(Some(layout));
        }
        size -= 0.2;
    }
    Err(format!(
        "Could not fit nameplate \"{}\" on this base. Shorten the text or choose a larger base.",
        nameplate.text
    ))
}

#[derive(Clone, Debug)]
struct LocalText {
    components: Vec<GlyphComponent>,
    min_x: f64,
    max_x: f64,
    max_y: f64,
}

fn layout_local_text(
    font: &FontRef<'_>,
    text: &str,
    _params: &BaseParams,
    font_size_mm: f64,
) -> Option<LocalText> {
    let units_per_em = font.units_per_em().unwrap_or(1000.0) as f64;
    let scale = font_size_mm / units_per_em;
    let mut cursor = 0.0;
    let mut previous = None;
    let mut components = Vec::new();
    for character in text.chars() {
        let glyph_id = font.glyph_id(character);
        if let Some(previous_id) = previous {
            cursor += font.kern_unscaled(previous_id, glyph_id) as f64 * scale;
        }
        if let Some(outline) = font.outline(glyph_id) {
            let rings = flatten_outline(&outline, scale, cursor);
            components.extend(group_glyph_components(&rings));
        }
        cursor += font.h_advance_unscaled(glyph_id) as f64 * scale;
        previous = Some(glyph_id);
    }
    if components.is_empty() {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for component in &components {
        for ring in std::iter::once(&component.outer).chain(component.holes.iter()) {
            for point in ring {
                min_x = min_x.min(point.x);
                max_x = max_x.max(point.x);
                min_y = min_y.min(point.y);
                max_y = max_y.max(point.y);
            }
        }
    }
    if !min_x.is_finite() || !max_x.is_finite() || max_x - min_x <= EPSILON {
        return None;
    }
    Some(LocalText {
        components,
        min_x,
        max_x,
        max_y,
    })
}

fn flatten_outline(outline: &ab_glyph::Outline, scale: f64, x_offset: f64) -> Vec<Vec<DVec2>> {
    let mut rings = Vec::new();
    let mut current = Vec::new();
    for curve in &outline.curves {
        let (start, end, samples) = flatten_curve(curve, scale, x_offset);
        if current.is_empty() {
            current.push(start);
        } else if current
            .last()
            .map(|point| point.distance(start) > MERGE_EPSILON)
            .unwrap_or(true)
        {
            finish_ring(&mut rings, &mut current);
            current.push(start);
        }
        current.extend(samples);
        if current
            .first()
            .map(|first| first.distance(end) <= MERGE_EPSILON)
            .unwrap_or(false)
        {
            finish_ring(&mut rings, &mut current);
        }
    }
    finish_ring(&mut rings, &mut current);
    rings
}

fn flatten_curve(curve: &OutlineCurve, scale: f64, x_offset: f64) -> (DVec2, DVec2, Vec<DVec2>) {
    let transform = |point: ab_glyph::Point| {
        DVec2::new(point.x as f64 * scale + x_offset, point.y as f64 * scale)
    };
    match *curve {
        OutlineCurve::Line(start, end) => {
            let start = transform(start);
            let end = transform(end);
            (start, end, vec![end])
        }
        OutlineCurve::Quad(start, control, end) => {
            let start = transform(start);
            let control = transform(control);
            let end = transform(end);
            let mut samples = Vec::with_capacity(TEXT_CURVE_STEPS);
            for step in 1..=TEXT_CURVE_STEPS {
                let t = step as f64 / TEXT_CURVE_STEPS as f64;
                let one_minus = 1.0 - t;
                samples.push(
                    start * (one_minus * one_minus)
                        + control * (2.0 * one_minus * t)
                        + end * (t * t),
                );
            }
            (start, end, samples)
        }
        OutlineCurve::Cubic(start, control_a, control_b, end) => {
            let start = transform(start);
            let control_a = transform(control_a);
            let control_b = transform(control_b);
            let end = transform(end);
            let mut samples = Vec::with_capacity(TEXT_CURVE_STEPS);
            for step in 1..=TEXT_CURVE_STEPS {
                let t = step as f64 / TEXT_CURVE_STEPS as f64;
                let one_minus = 1.0 - t;
                samples.push(
                    start * (one_minus * one_minus * one_minus)
                        + control_a * (3.0 * one_minus * one_minus * t)
                        + control_b * (3.0 * one_minus * t * t)
                        + end * (t * t * t),
                );
            }
            (start, end, samples)
        }
    }
}

fn finish_ring(rings: &mut Vec<Vec<DVec2>>, current: &mut Vec<DVec2>) {
    if current.len() >= 3 {
        if current
            .first()
            .zip(current.last())
            .map(|(first, last)| first.distance(*last) <= MERGE_EPSILON)
            .unwrap_or(false)
        {
            current.pop();
        }
        if current.len() >= 3 && !current.iter().any(|point| !point.x.is_finite()) {
            rings.push(std::mem::take(current));
        }
    }
    current.clear();
}

fn group_glyph_components(rings: &[Vec<DVec2>]) -> Vec<GlyphComponent> {
    let mut outers = Vec::new();
    let mut holes = Vec::new();
    for ring in rings {
        let area = signed_area(ring);
        if area < -EPSILON {
            outers.push(ring.clone());
        } else if area > EPSILON {
            holes.push(ring.clone());
        }
    }
    if outers.is_empty() {
        // A few fonts emit the opposite winding. Treat the positive loops as
        // outer components so the nameplate still renders.
        for ring in rings {
            if signed_area(ring) > EPSILON {
                outers.push(ring.clone());
            }
        }
    }
    let mut components: Vec<GlyphComponent> = outers
        .into_iter()
        .map(|outer| GlyphComponent {
            outer,
            holes: Vec::new(),
        })
        .collect();
    for hole in holes {
        let sample = hole[0];
        let mut best = None;
        let mut best_area = f64::INFINITY;
        for (index, component) in components.iter().enumerate() {
            if point_in_polygon(sample, &component.outer) {
                let area = signed_area(&component.outer).abs();
                if area < best_area {
                    best_area = area;
                    best = Some(index);
                }
            }
        }
        if let Some(index) = best {
            components[index].holes.push(hole);
        } else {
            components.push(GlyphComponent {
                outer: hole,
                holes: Vec::new(),
            });
        }
    }
    components
}

fn signed_area(ring: &[DVec2]) -> f64 {
    let mut area = 0.0;
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

fn place_line(params: &BaseParams, local: &LocalText, font_size_mm: f64) -> Option<TextLayout> {
    let margin = 0.7;
    let inset = shape_ring(params, margin).ok()?;
    let pocket_radius = params
        .magnet
        .as_ref()
        .map(MagnetSpec::effective_radius_mm)
        .unwrap_or(0.0);
    let top_limit = if pocket_radius > 0.0 {
        -(pocket_radius + 0.7)
    } else {
        -params.min_dimension_mm() * 0.08
    };
    let x_shift = -(local.min_x + local.max_x) * 0.5;
    let baseline = top_limit - local.max_y;
    let components = transform_components(&local.components, |point| {
        DVec2::new(point.x + x_shift, point.y + baseline)
    });
    let (min, max) = component_bounds(&components)?;
    if !all_inside(&components, &inset, pocket_radius) {
        return None;
    }
    // Flip the line vertically in place so the nameplate reads correctly when
    // the finished base is turned over to inspect its underside. Arc text is
    // already laid out with its tops pointing outward.
    let mirror = min.y + max.y;
    let components =
        transform_components(&components, |point| DVec2::new(point.x, mirror - point.y));
    let mirrored_baseline = mirror - baseline;
    Some(TextLayout {
        components,
        placement: NameplatePlacement::Line {
            baseline_y_mm: mirrored_baseline,
            width_mm: max.x - min.x,
            font_size_mm,
        },
        bounds_min: min,
        bounds_max: max,
    })
}

fn place_arc(params: &BaseParams, local: &LocalText, font_size_mm: f64) -> Option<TextLayout> {
    let margin = 0.8;
    let max_radius = params.min_dimension_mm() * 0.5 - margin - 0.2;
    let radius = max_radius - local.max_y;
    if radius <= 0.5 {
        return None;
    }
    let width = local.max_x - local.min_x;
    let span = width / radius;
    if span > std::f64::consts::PI * 1.35 {
        return None;
    }
    let center_x = (local.min_x + local.max_x) * 0.5;
    let components = transform_components(&local.components, |point| {
        let x = point.x - center_x;
        let theta = -std::f64::consts::FRAC_PI_2 + x / radius;
        let r = radius + point.y;
        DVec2::new(r * theta.cos(), r * theta.sin())
    });
    let (min, max) = component_bounds(&components)?;
    let inset = shape_ring(params, margin).ok()?;
    let pocket_radius = params
        .magnet
        .as_ref()
        .map(MagnetSpec::effective_radius_mm)
        .unwrap_or(0.0);
    if !all_inside(&components, &inset, pocket_radius) {
        return None;
    }
    Some(TextLayout {
        components,
        placement: NameplatePlacement::Arc {
            radius_mm: radius,
            span_radians: span,
            font_size_mm,
        },
        bounds_min: min,
        bounds_max: max,
    })
}

fn transform_components(
    components: &[GlyphComponent],
    transform: impl Fn(DVec2) -> DVec2,
) -> Vec<GlyphComponent> {
    components
        .iter()
        .map(|component| GlyphComponent {
            outer: component
                .outer
                .iter()
                .map(|point| transform(*point))
                .collect(),
            holes: component
                .holes
                .iter()
                .map(|hole| hole.iter().map(|point| transform(*point)).collect())
                .collect(),
        })
        .collect()
}

fn component_bounds(components: &[GlyphComponent]) -> Option<(DVec2, DVec2)> {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for component in components {
        for ring in std::iter::once(&component.outer).chain(component.holes.iter()) {
            for point in ring {
                min = min.min(*point);
                max = max.max(*point);
            }
        }
    }
    (min.x.is_finite() && min.y.is_finite()).then_some((min, max))
}

fn all_inside(components: &[GlyphComponent], inset: &[DVec2], pocket_radius: f64) -> bool {
    let clearance = pocket_radius + 0.45;
    for component in components {
        for ring in std::iter::once(&component.outer).chain(component.holes.iter()) {
            for point in ring {
                if !point_in_polygon(*point, inset) {
                    return false;
                }
                if pocket_radius > 0.0 && point.length() < clearance {
                    return false;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Base generation
// ---------------------------------------------------------------------------

pub fn generate_base(params: &BaseParams) -> Result<BaseMesh, String> {
    params.validate()?;
    let text_layout = params
        .nameplate
        .as_ref()
        .map(|nameplate| layout_nameplate(params, nameplate))
        .transpose()?
        .flatten();
    let text_layout = text_layout.as_ref();
    let mut builder = MeshBuilder::default();
    let mut features = MeshFeatures::default();
    let height = params.height_mm;
    let bevel_height = params.bevel_height_mm();
    let bevel_start_z = height - bevel_height;
    let bottom_points = shape_ring(params, 0.0)?;
    let top_points = shape_ring(params, params.bevel_width_mm)?;
    let bottom_ring = builder.ring(&bottom_points, 0.0);
    let bevel_start_ring = builder.ring(&bottom_points, bevel_start_z);
    let top_ring = builder.ring(&top_points, height);
    if bevel_height > EPSILON {
        builder.wall(&bottom_ring, &bevel_start_ring);
        builder.wall(&bevel_start_ring, &top_ring);
    } else {
        builder.wall(&bottom_ring, &top_ring);
    }

    if let Some(recess) = &params.top_recess {
        let lip_points = shape_ring(params, params.bevel_width_mm + recess.rim_width_mm)?;
        let lip_ring = builder.ring(&lip_points, height);
        builder.face_with_holes(&top_ring, std::slice::from_ref(&lip_ring))?;
        let floor_ring = builder.ring(&lip_points, height - recess.depth_mm);
        builder.wall(&lip_ring, &floor_ring);
        builder.face(&floor_ring);
    } else {
        builder.face(&top_ring);
    }

    let mut bottom_holes = Vec::new();

    if let Some(magnet) = &params.magnet {
        let radius = magnet.effective_radius_mm();
        let points = (0..64)
            .map(|index| {
                let angle = std::f64::consts::TAU * index as f64 / 64.0;
                DVec2::new(radius * angle.cos(), radius * angle.sin())
            })
            .collect::<Vec<_>>();
        let bottom = builder.ring(&points, 0.0);
        let ceiling = builder.ring(&points, magnet.depth_mm);
        builder.wall(&bottom, &ceiling);
        builder.face(&ceiling);
        bottom_holes.push(bottom);
        features.magnet = Some(MagnetInfo {
            center: DVec2::ZERO,
            effective_radius_mm: radius,
            depth_mm: magnet.depth_mm,
        });
    }

    if let (Some(nameplate), Some(layout)) = (&params.nameplate, text_layout) {
        let face_z = match nameplate.style {
            NameplateStyle::Engrave => nameplate.depth_mm,
            NameplateStyle::Emboss => -nameplate.depth_mm,
        };
        for component in &layout.components {
            let outer_bottom = builder.ring(&component.outer, 0.0);
            bottom_holes.push(outer_bottom.clone());
            let outer_face = builder.ring(&component.outer, face_z);
            builder.wall(&outer_bottom, &outer_face);
            let mut face_holes = Vec::with_capacity(component.holes.len());
            for hole in &component.holes {
                let hole_face = builder.ring(hole, face_z);
                let hole_bottom = builder.ring(hole, 0.0);
                builder.wall(&hole_face, &hole_bottom);
                builder.face(&hole_bottom);
                face_holes.push(hole_face);
            }
            builder.face_with_holes(&outer_face, &face_holes)?;
        }
        features.nameplate = Some(NameplateInfo {
            style: nameplate.style,
            placement: layout.placement.clone(),
            font_size_mm: layout.placement.font_size(),
            depth_mm: nameplate.depth_mm,
            bounds_min: layout.bounds_min,
            bounds_max: layout.bounds_max,
        });
    }

    builder.face_with_holes(&bottom_ring, &bottom_holes)?;
    builder.finish(features)
}

impl NameplatePlacement {
    pub fn font_size(&self) -> f64 {
        match self {
            Self::Line { font_size_mm, .. } | Self::Arc { font_size_mm, .. } => *font_size_mm,
        }
    }
}

// ---------------------------------------------------------------------------
// STL output and names
// ---------------------------------------------------------------------------

pub fn binary_stl_bytes(mesh: &BaseMesh, label: &str) -> Result<Vec<u8>, String> {
    let count = u32::try_from(mesh.triangles.len())
        .map_err(|_| "Too many triangles for binary STL.".to_owned())?;
    let mut bytes = Vec::with_capacity(84 + mesh.triangles.len() * 50);
    let mut header = format!("TableScale {label}").into_bytes();
    header.resize(80, 0);
    bytes.extend_from_slice(&header[..80]);
    bytes.extend_from_slice(&count.to_le_bytes());
    for (triangle, normal) in mesh.triangles.iter().zip(&mesh.normals) {
        let normal = normal.normalize_or_zero();
        for value in [normal.x, normal.y, normal.z] {
            bytes.extend_from_slice(&(value as f32).to_le_bytes());
        }
        for id in triangle {
            let vertex = mesh.vertices[*id as usize];
            for value in [vertex.x, vertex.y, vertex.z] {
                bytes.extend_from_slice(&(value as f32).to_le_bytes());
            }
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    Ok(bytes)
}

pub fn write_stl(path: &Path, mesh: &BaseMesh, label: &str) -> Result<(), String> {
    let bytes = binary_stl_bytes(mesh, label)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("base.stl");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(".{file_name}.{}.part", std::process::id()));
    let mut file = std::fs::File::create(&temporary)
        .map_err(|error| format!("Could not create temporary STL: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("Could not write STL: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush STL: {error}"))?;
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("Could not finalize STL: {error}"))
}

pub fn sanitize_name(name: &str) -> String {
    let mut result = String::new();
    let mut previous_dash = false;
    for character in name.trim().chars() {
        if character.is_alphanumeric() || character == '-' || character == '_' {
            for lowercase in character.to_lowercase() {
                result.push(lowercase);
            }
            previous_dash = false;
        } else if !previous_dash {
            result.push('-');
            previous_dash = true;
        }
    }
    let result = result.trim_matches('-').to_owned();
    if result.is_empty() {
        "base".to_owned()
    } else {
        result
    }
}

fn format_mm(value: f64) -> String {
    let mut formatted = format!("{value:.2}");
    while formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle_params() -> BaseParams {
        BaseParams::default()
    }

    #[test]
    fn default_circle_is_watertight_and_bounded() {
        let mesh = generate_base(&circle_params()).unwrap();
        assert!(mesh.watertight);
        assert_eq!(mesh.validate_topology(), Ok(()));
        assert!((mesh.dimensions_mm().x - 25.0).abs() < 0.02);
        assert!((mesh.dimensions_mm().y - 25.0).abs() < 0.02);
        assert!((mesh.dimensions_mm().z - 3.5).abs() < 0.02);
        assert!(mesh.features.magnet.is_some());
        assert!(mesh.features.nameplate.is_some());
    }

    #[test]
    fn all_shapes_are_watertight() {
        for shape in [BaseShape::Circle, BaseShape::Pill, BaseShape::RoundedSquare] {
            let params = BaseParams {
                shape,
                width_mm: 40.0,
                depth_mm: 25.0,
                ..circle_params()
            };
            let mesh = generate_base(&params).unwrap();
            assert!(mesh.watertight, "{shape:?}");
            assert_eq!(mesh.validate_topology(), Ok(()), "{shape:?}");
            assert!((mesh.dimensions_mm().x - 40.0).abs() < 0.02);
            assert!((mesh.dimensions_mm().y - 25.0).abs() < 0.02);
        }
    }

    #[test]
    fn preset_sizes_are_exact() {
        for size in [25.0, 32.0, 35.0, 40.0, 50.0, 65.0, 80.0] {
            let params = BaseParams {
                width_mm: size,
                depth_mm: size,
                ..circle_params()
            };
            let mesh = generate_base(&params).unwrap();
            assert!((mesh.dimensions_mm().x - size).abs() < 0.02);
            assert!((mesh.dimensions_mm().y - size).abs() < 0.02);
            assert_eq!(mesh.validate_topology(), Ok(()));
        }
    }

    #[test]
    fn round_bases_use_arc_text_and_other_shapes_use_lines() {
        let circle = generate_base(&circle_params()).unwrap();
        let placement = &circle.features.nameplate.as_ref().unwrap().placement;
        assert!(
            matches!(placement, NameplatePlacement::Arc { .. }),
            "expected arc placement, got {placement:?}"
        );
        let pill = BaseParams {
            shape: BaseShape::Pill,
            width_mm: 50.0,
            depth_mm: 25.0,
            ..circle_params()
        };
        let pill = generate_base(&pill).unwrap();
        let placement = &pill.features.nameplate.as_ref().unwrap().placement;
        assert!(
            matches!(placement, NameplatePlacement::Line { .. }),
            "expected line placement, got {placement:?}"
        );
    }

    #[test]
    fn binary_stl_header_and_count_are_valid() {
        let mesh = generate_base(&circle_params()).unwrap();
        let bytes = binary_stl_bytes(&mesh, "circle").unwrap();
        assert!(bytes.len() >= 84);
        assert_eq!(&bytes[..10], b"TableScale");
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert_eq!(count, mesh.triangle_count());
        assert_eq!(bytes.len(), 84 + count * 50);
    }

    #[test]
    fn magnet_pocket_adds_a_manifold_cavity() {
        let without = BaseParams {
            magnet: None,
            ..circle_params()
        };
        let with = circle_params();
        let a = generate_base(&without).unwrap();
        let b = generate_base(&with).unwrap();
        assert!(a.triangle_count() < b.triangle_count());
        assert_eq!(b.validate_topology(), Ok(()));
        let magnet = b.features.magnet.as_ref().unwrap();
        assert!((magnet.effective_radius_mm - 2.65).abs() < 0.001);
        assert!((magnet.depth_mm - 2.0).abs() < 0.001);
    }

    #[test]
    fn nameplate_styles_and_examples_are_watertight() {
        for text in ["Magneto", "Shorty", "Thor"] {
            for style in [NameplateStyle::Engrave, NameplateStyle::Emboss] {
                let params = BaseParams {
                    nameplate: Some(NameplateSpec {
                        text: text.to_owned(),
                        style,
                        depth_mm: 0.4,
                    }),
                    ..circle_params()
                };
                let mesh = generate_base(&params)
                    .unwrap_or_else(|error| panic!("{text} {style:?}: {error}"));
                assert_eq!(mesh.validate_topology(), Ok(()), "{text} {style:?}");
                let nameplate = mesh.features.nameplate.as_ref().unwrap();
                assert_eq!(nameplate.style, style);
                assert!(nameplate.font_size_mm >= 1.0);
                let pocket_radius = mesh.features.magnet.as_ref().unwrap().effective_radius_mm;
                assert!(
                    nameplate.bounds_min.length() > pocket_radius,
                    "{text} {style:?} intersects magnet"
                );
            }
        }
    }

    #[test]
    fn top_lip_is_watertight_and_keeps_the_body_height() {
        let params = BaseParams {
            top_recess: Some(TopRecessSpec {
                rim_width_mm: 1.2,
                depth_mm: 0.6,
            }),
            ..circle_params()
        };
        let mesh = generate_base(&params).unwrap();
        assert_eq!(mesh.validate_topology(), Ok(()));
        assert!(mesh.volume_mm3() > 1000.0);
        assert!((mesh.bounds_max.z - 3.5).abs() < 0.02);
        assert!(mesh.triangle_count() > 1000);
    }

    #[test]
    fn zero_bevel_and_no_magnet_is_still_watertight() {
        let params = BaseParams {
            bevel_width_mm: 0.0,
            magnet: None,
            nameplate: None,
            ..circle_params()
        };
        let mesh = generate_base(&params).unwrap();
        assert_eq!(mesh.validate_topology(), Ok(()));
        assert!((mesh.dimensions_mm().x - 25.0).abs() < 0.02);
        assert!((mesh.dimensions_mm().z - 3.5).abs() < 0.02);
    }

    #[test]
    fn engraved_nameplate_keeps_exact_body_height() {
        let mesh = generate_base(&circle_params()).unwrap();
        assert!((mesh.dimensions_mm().z - 3.5).abs() < 0.02);
    }

    #[test]
    fn embossed_nameplate_extends_below_the_base() {
        let params = BaseParams {
            nameplate: Some(NameplateSpec {
                text: "Thor".to_owned(),
                style: NameplateStyle::Emboss,
                depth_mm: 0.4,
            }),
            ..circle_params()
        };
        let mesh = generate_base(&params).unwrap_or_else(|error| panic!("Thor emboss: {error}"));
        assert!((mesh.bounds_min.z + 0.4).abs() < 0.02);
        assert!((mesh.bounds_max.z - 3.5).abs() < 0.02);
    }

    #[test]
    fn invalid_dimensions_are_rejected() {
        let params = BaseParams {
            width_mm: 0.0,
            ..circle_params()
        };
        assert!(params.validate().is_err());
        let params = BaseParams {
            magnet: Some(MagnetSpec {
                diameter_mm: 80.0,
                depth_mm: 2.0,
                radial_tolerance_mm: 0.15,
            }),
            ..circle_params()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn nameplate_outline_union_handles_tricky_strings() {
        let names = [
            "Magneto",
            "Professor X",
            "Spider-Man",
            "Ms. Marvel",
            "M'Baku",
            "Thor",
            "___",
            "OOO",
            "8",
        ];
        let shapes = [
            (BaseShape::Circle, 25.0, 25.0, 2.0),
            (BaseShape::Circle, 80.0, 80.0, 12.0),
            (BaseShape::Pill, 50.0, 25.0, 5.0),
            (BaseShape::RoundedSquare, 40.0, 32.0, 6.0),
        ];
        for name in names {
            for (shape, width, depth, radius) in shapes {
                let params = BaseParams {
                    shape,
                    width_mm: width,
                    depth_mm: depth,
                    corner_radius_mm: radius,
                    nameplate: Some(NameplateSpec {
                        text: name.to_owned(),
                        style: NameplateStyle::Engrave,
                        depth_mm: 0.4,
                    }),
                    ..BaseParams::default()
                };
                let mesh = generate_base(&params)
                    .unwrap_or_else(|error| panic!("{name} {shape:?} {width}x{depth}: {error}"));
                assert_eq!(
                    mesh.validate_topology(),
                    Ok(()),
                    "{name} {shape:?} {width}x{depth}"
                );
            }
        }
    }

    #[test]
    fn binary_stl_written_to_disk_roundtrips() {
        let mesh = generate_base(&circle_params()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "tablescale-stl-roundtrip-{}.stl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        write_stl(&path, &mesh, "roundtrip").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() >= 84);
        assert_eq!(&bytes[..10], b"TableScale");
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert_eq!(count, mesh.triangle_count());
        assert_eq!(bytes.len(), 84 + count * 50);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn filenames_include_shape_size_and_character() {
        let circle = BaseParams::default();
        assert_eq!(circle.file_name("Magneto"), "magneto_25mm.stl");
        let pill = BaseParams {
            shape: BaseShape::Pill,
            width_mm: 50.0,
            depth_mm: 25.0,
            ..circle_params()
        };
        assert_eq!(pill.file_name("Shorty!"), "shorty_50x25mm.stl");
    }
}
