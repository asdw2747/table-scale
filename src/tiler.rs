//! Paper geometry and tiling calculations for battle map printing.

use crate::scale::mm_to_inches;

const EPSILON: f64 = 1e-9;

/// A hard safety limit that keeps the preview and exports responsive.
pub const MAX_PAGE_COUNT: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeMode {
    Grid,
    Exact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquareUnit {
    Inches,
    Millimeters,
}

impl SquareUnit {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inches => "in",
            Self::Millimeters => "mm",
        }
    }
}

/// UI bounds for a square size. These are unit-aware so that switching to
/// millimetres does not clamp a standard 1-inch (25.4 mm) square.
pub fn square_size_bounds(unit: SquareUnit) -> std::ops::RangeInclusive<f64> {
    match unit {
        SquareUnit::Inches => 0.1..=10.0,
        SquareUnit::Millimeters => 1.0..=100.0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaperPreset {
    Letter,
    Legal,
    Tabloid,
}

pub const PAPER_PRESETS: [PaperPreset; 3] = [
    PaperPreset::Letter,
    PaperPreset::Legal,
    PaperPreset::Tabloid,
];

impl PaperPreset {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Letter => "Letter",
            Self::Legal => "Legal",
            Self::Tabloid => "Tabloid",
        }
    }

    /// Portrait width and height in inches.
    pub const fn size_in(self) -> (f64, f64) {
        match self {
            Self::Letter => (8.5, 11.0),
            Self::Legal => (8.5, 14.0),
            Self::Tabloid => (11.0, 17.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

#[derive(Clone, Debug)]
pub struct TilerSettings {
    pub mode: SizeMode,
    pub grid_width: u32,
    pub grid_height: u32,
    pub square_size: f64,
    pub square_unit: SquareUnit,
    pub exact_width_in: f64,
    pub exact_height_in: f64,
    pub paper: PaperPreset,
    pub orientation: Orientation,
    pub margin_in: f64,
    pub overlap_in: f64,
}

impl Default for TilerSettings {
    fn default() -> Self {
        Self {
            mode: SizeMode::Grid,
            grid_width: 19,
            grid_height: 19,
            square_size: 1.0,
            square_unit: SquareUnit::Inches,
            exact_width_in: 22.0,
            exact_height_in: 28.0,
            paper: PaperPreset::Letter,
            orientation: Orientation::Portrait,
            margin_in: 0.25,
            overlap_in: 0.25,
        }
    }
}

impl TilerSettings {
    pub fn square_size_in(&self) -> f64 {
        match self.square_unit {
            SquareUnit::Inches => self.square_size,
            SquareUnit::Millimeters => mm_to_inches(self.square_size),
        }
    }

    /// The finished map size in inches.
    pub fn map_dimensions_in(&self) -> (f64, f64) {
        match self.mode {
            SizeMode::Grid => {
                let square = self.square_size_in();
                (
                    self.grid_width as f64 * square,
                    self.grid_height as f64 * square,
                )
            }
            SizeMode::Exact => (self.exact_width_in, self.exact_height_in),
        }
    }

    /// The selected paper size after applying orientation.
    pub fn paper_size_in(&self) -> (f64, f64) {
        let (width, height) = self.paper.size_in();
        match self.orientation {
            Orientation::Portrait => (width, height),
            Orientation::Landscape => (height, width),
        }
    }

    pub fn printable_area_in(&self) -> (f64, f64) {
        let (paper_w, paper_h) = self.paper_size_in();
        (
            paper_w - 2.0 * self.margin_in,
            paper_h - 2.0 * self.margin_in,
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        let (map_w, map_h) = self.map_dimensions_in();
        if !map_w.is_finite() || !map_h.is_finite() || map_w <= 0.0 || map_h <= 0.0 {
            return Err("Enter a map size greater than zero.".to_owned());
        }

        if self.mode == SizeMode::Grid && (self.grid_width == 0 || self.grid_height == 0) {
            return Err("Grid width and height must be at least one square.".to_owned());
        }

        if !self.margin_in.is_finite() || self.margin_in < 0.0 {
            return Err("Printer margin cannot be negative.".to_owned());
        }

        let (paper_w, paper_h) = self.paper_size_in();
        if self.margin_in * 2.0 >= paper_w.min(paper_h) {
            return Err("The printer margin leaves no printable area on this paper.".to_owned());
        }

        let (printable_w, printable_h) = self.printable_area_in();
        if !self.overlap_in.is_finite() || self.overlap_in < 0.0 {
            return Err("Overlap cannot be negative.".to_owned());
        }
        if self.overlap_in + EPSILON >= printable_w.min(printable_h) {
            return Err("Overlap must be smaller than the printable area.".to_owned());
        }

        let columns = tile_count_along(map_w, printable_w, self.overlap_in);
        let rows = tile_count_along(map_h, printable_h, self.overlap_in);
        let pages = columns
            .checked_mul(rows)
            .ok_or_else(|| "The requested map is too large to tile safely.".to_owned())?;
        if pages > MAX_PAGE_COUNT {
            return Err(format!(
                "This map would require {pages} pages; TableScale supports up to {MAX_PAGE_COUNT} pages per export. Increase the square size, use larger paper, or reduce the margin/overlap."
            ));
        }

        Ok(())
    }

    /// Build the complete page grid, or return a user-facing validation error.
    pub fn layout(&self) -> Result<TileLayout, String> {
        self.validate()?;

        let (map_width_in, map_height_in) = self.map_dimensions_in();
        let (paper_width_in, paper_height_in) = self.paper_size_in();
        let (printable_width_in, printable_height_in) = self.printable_area_in();
        let step_x_in = printable_width_in - self.overlap_in;
        let step_y_in = printable_height_in - self.overlap_in;
        let columns = tile_count_along(map_width_in, printable_width_in, self.overlap_in);
        let rows = tile_count_along(map_height_in, printable_height_in, self.overlap_in);
        let page_capacity = columns
            .checked_mul(rows)
            .ok_or_else(|| "The requested map is too large to tile safely.".to_owned())?;

        let mut tiles = Vec::with_capacity(page_capacity);
        for row in 0..rows {
            let y_in = row as f64 * step_y_in;
            let height_in = (map_height_in - y_in).min(printable_height_in);
            for column in 0..columns {
                let x_in = column as f64 * step_x_in;
                let width_in = (map_width_in - x_in).min(printable_width_in);
                if width_in <= 0.0 || height_in <= 0.0 {
                    return Err("The selected margin and overlap produce an empty tile.".to_owned());
                }
                tiles.push(Tile {
                    index: row * columns + column,
                    row,
                    column,
                    x_in,
                    y_in,
                    width_in,
                    height_in,
                });
            }
        }

        Ok(TileLayout {
            map_width_in,
            map_height_in,
            paper_width_in,
            paper_height_in,
            printable_width_in,
            printable_height_in,
            margin_in: self.margin_in,
            overlap_in: self.overlap_in,
            step_x_in,
            step_y_in,
            columns,
            rows,
            tiles,
        })
    }
}

/// Number of sheets needed along one axis.
pub fn tile_count_along(map_in: f64, printable_in: f64, overlap_in: f64) -> usize {
    let step = printable_in - overlap_in;
    if step <= 0.0 || !step.is_finite() {
        return 1;
    }
    let raw = (map_in - overlap_in) / step;
    if !raw.is_finite() {
        return 1;
    }
    (raw - EPSILON).ceil().max(1.0) as usize
}

#[derive(Clone, Debug, PartialEq)]
pub struct TileLayout {
    pub map_width_in: f64,
    pub map_height_in: f64,
    pub paper_width_in: f64,
    pub paper_height_in: f64,
    pub printable_width_in: f64,
    pub printable_height_in: f64,
    pub margin_in: f64,
    pub overlap_in: f64,
    pub step_x_in: f64,
    pub step_y_in: f64,
    pub columns: usize,
    pub rows: usize,
    pub tiles: Vec<Tile>,
}

impl TileLayout {
    pub fn page_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn effective_dpi(&self, image_width_px: u32, image_height_px: u32) -> f64 {
        if self.map_width_in <= 0.0 || self.map_height_in <= 0.0 {
            return 0.0;
        }
        (image_width_px as f64 / self.map_width_in).min(image_height_px as f64 / self.map_height_in)
    }

    pub fn page_label(&self, tile: &Tile) -> String {
        format!("Page {} of {}", tile.page_number(), self.page_count())
    }

    pub fn sheet_label(&self) -> String {
        format!(
            "{} \u{00d7} {} sheets \u{00b7} {} pages",
            self.columns,
            self.rows,
            self.page_count()
        )
    }

    pub fn has_overlap_right(&self, tile: &Tile) -> bool {
        self.overlap_in > 0.0 && tile.column + 1 < self.columns
    }

    pub fn has_overlap_bottom(&self, tile: &Tile) -> bool {
        self.overlap_in > 0.0 && tile.row + 1 < self.rows
    }

    pub fn aspect_ratio(&self) -> f64 {
        if self.map_height_in > 0.0 {
            self.map_width_in / self.map_height_in
        } else {
            1.0
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Zero-based row-major page index.
    pub index: usize,
    pub row: usize,
    pub column: usize,
    /// Position of this tile's top-left corner in map coordinates, in inches.
    pub x_in: f64,
    pub y_in: f64,
    pub width_in: f64,
    pub height_in: f64,
}

impl Tile {
    pub fn page_number(&self) -> usize {
        self.index + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    fn settings_22x28() -> TilerSettings {
        TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 22.0,
            exact_height_in: 28.0,
            ..TilerSettings::default()
        }
    }

    #[test]
    fn paper_presets_have_correct_dimensions() {
        assert_eq!(PaperPreset::Letter.size_in(), (8.5, 11.0));
        assert_eq!(PaperPreset::Legal.size_in(), (8.5, 14.0));
        assert_eq!(PaperPreset::Tabloid.size_in(), (11.0, 17.0));
    }

    #[test]
    fn orientation_swaps_paper_dimensions() {
        let mut settings = TilerSettings {
            orientation: Orientation::Landscape,
            ..TilerSettings::default()
        };
        assert_eq!(settings.paper_size_in(), (11.0, 8.5));
        settings.orientation = Orientation::Portrait;
        assert_eq!(settings.paper_size_in(), (8.5, 11.0));
    }

    #[test]
    fn grid_dimensions_use_inches_or_millimetres() {
        let mut settings = TilerSettings {
            grid_width: 19,
            grid_height: 19,
            square_size: 1.0,
            ..TilerSettings::default()
        };
        assert_eq!(settings.map_dimensions_in(), (19.0, 19.0));

        settings.grid_width = 12;
        settings.grid_height = 12;
        assert_eq!(settings.map_dimensions_in(), (12.0, 12.0));

        settings.grid_width = 19;
        settings.grid_height = 19;
        settings.square_unit = SquareUnit::Millimeters;
        settings.square_size = 25.4;
        let (width, height) = settings.map_dimensions_in();
        assert_close(width, 19.0);
        assert_close(height, 19.0);
    }

    #[test]
    fn square_size_bounds_cover_one_inch_in_both_units() {
        assert!(square_size_bounds(SquareUnit::Inches).contains(&1.0));
        assert!(square_size_bounds(SquareUnit::Millimeters).contains(&25.4));
    }

    #[test]
    fn letter_22x28_needs_three_by_three_pages() {
        let layout = settings_22x28().layout().unwrap();
        assert_eq!((layout.columns, layout.rows), (3, 3));
        assert_eq!(layout.page_count(), 9);
        assert_close(layout.printable_width_in, 8.0);
        assert_close(layout.printable_height_in, 10.5);
        assert_close(layout.step_x_in, 7.75);
        assert_close(layout.step_y_in, 10.25);
        assert_eq!(layout.tiles[8].x_in, 15.5);
        assert_eq!(layout.tiles[8].y_in, 20.5);
        assert_eq!(layout.tiles[8].width_in, 6.5);
        assert_eq!(layout.tiles[8].height_in, 7.5);
    }

    #[test]
    fn letter_19x19_needs_three_by_two_pages() {
        let layout = TilerSettings::default().layout().unwrap();
        assert_eq!((layout.columns, layout.rows), (3, 2));
        assert_eq!(layout.page_count(), 6);
        assert_eq!(layout.tiles.last().unwrap().width_in, 3.5);
        assert_eq!(layout.tiles.last().unwrap().height_in, 8.75);
    }

    #[test]
    fn exact_printable_area_is_one_page() {
        let settings = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 8.0,
            exact_height_in: 10.5,
            ..TilerSettings::default()
        };
        let layout = settings.layout().unwrap();
        assert_eq!((layout.columns, layout.rows), (1, 1));
        assert_eq!(layout.page_count(), 1);
    }

    #[test]
    fn landscape_letter_22x28_needs_three_by_four_pages() {
        let settings = TilerSettings {
            orientation: Orientation::Landscape,
            ..settings_22x28()
        };
        let layout = settings.layout().unwrap();
        assert_eq!((layout.columns, layout.rows), (3, 4));
        assert_eq!(layout.page_count(), 12);
    }

    #[test]
    fn legal_and_tabloid_page_counts() {
        let legal = TilerSettings {
            paper: PaperPreset::Legal,
            ..settings_22x28()
        }
        .layout()
        .unwrap();
        assert_eq!((legal.columns, legal.rows), (3, 3));

        let tabloid = TilerSettings {
            paper: PaperPreset::Tabloid,
            ..settings_22x28()
        }
        .layout()
        .unwrap();
        assert_eq!((tabloid.columns, tabloid.rows), (3, 2));
    }

    #[test]
    fn zero_overlap_still_tiles_the_map() {
        let settings = TilerSettings {
            overlap_in: 0.0,
            ..TilerSettings::default()
        };
        let layout = settings.layout().unwrap();
        assert_eq!((layout.columns, layout.rows), (3, 2));
        assert_close(layout.step_x_in, 8.0);
        assert_close(layout.step_y_in, 10.5);
    }

    #[test]
    fn final_tiles_cover_the_map_exactly() {
        let layout = settings_22x28().layout().unwrap();
        let last = layout.tiles.last().unwrap();
        assert_close(last.x_in + last.width_in, layout.map_width_in);
        assert_close(last.y_in + last.height_in, layout.map_height_in);
        for tile in &layout.tiles {
            assert!(tile.x_in >= 0.0);
            assert!(tile.y_in >= 0.0);
            assert!(tile.x_in + tile.width_in <= layout.map_width_in + 1e-9);
            assert!(tile.y_in + tile.height_in <= layout.map_height_in + 1e-9);
            assert!(tile.width_in <= layout.printable_width_in + 1e-9);
            assert!(tile.height_in <= layout.printable_height_in + 1e-9);
        }
    }

    #[test]
    fn every_final_row_and_column_is_reachable() {
        let layout = settings_22x28().layout().unwrap();
        for column in 0..layout.columns {
            assert!(layout.tiles.iter().any(|tile| tile.column == column));
        }
        for row in 0..layout.rows {
            assert!(layout.tiles.iter().any(|tile| tile.row == row));
        }
    }

    #[test]
    fn effective_dpi_uses_the_tighter_axis() {
        let layout = settings_22x28().layout().unwrap();
        assert_close(layout.effective_dpi(4400, 5600), 200.0);
        assert_close(layout.effective_dpi(6600, 7000), 250.0);
    }

    #[test]
    fn overlap_must_be_smaller_than_printable_area() {
        let settings = TilerSettings {
            overlap_in: 8.0,
            ..settings_22x28()
        };
        assert!(settings.layout().is_err());
    }

    #[test]
    fn oversized_margin_is_rejected() {
        let settings = TilerSettings {
            margin_in: 5.0,
            ..settings_22x28()
        };
        assert!(settings.layout().is_err());
    }

    #[test]
    fn zero_map_size_is_rejected() {
        let settings = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 0.0,
            exact_height_in: 10.0,
            ..TilerSettings::default()
        };
        assert!(settings.layout().is_err());
    }

    #[test]
    fn page_numbers_are_row_major() {
        let layout = settings_22x28().layout().unwrap();
        assert_eq!(layout.tiles[0].page_number(), 1);
        assert_eq!(layout.tiles[1].page_number(), 2);
        assert_eq!(layout.tiles[3].page_number(), 4);
        assert_eq!(layout.tiles[8].page_number(), 9);
        assert_eq!(layout.page_label(&layout.tiles[4]), "Page 5 of 9");
    }

    #[test]
    fn oversized_page_grids_are_rejected_before_allocation() {
        let settings = TilerSettings {
            grid_width: 200,
            grid_height: 200,
            square_size: 25.0,
            ..TilerSettings::default()
        };
        let error = settings.layout().unwrap_err();
        assert!(error.contains("page"));
        assert!(settings.validate().is_err());
    }

    #[test]
    fn page_limit_allows_normal_large_maps() {
        let settings = TilerSettings {
            mode: SizeMode::Exact,
            exact_width_in: 100.0,
            exact_height_in: 100.0,
            ..TilerSettings::default()
        };
        let layout = settings.layout().unwrap();
        assert!(layout.page_count() <= MAX_PAGE_COUNT);
    }
}
