//! Pure math for miniature scales, heights, and base sizes.

/// A standard 6 ft human in millimetres. Ratio scales use this as their
/// reference height when converted to a nominal miniature height.
pub const REFERENCE_HEIGHT_MM: f64 = 1828.8;
pub const REFERENCE_HEIGHT_FT: f64 = 6.0;
pub const MM_PER_INCH: f64 = 25.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalePreset {
    Mm28,
    Mm32,
    Mm35,
    Mm54,
    Mm75,
    Ratio64,
    Ratio48,
    Ratio35,
    Ratio12,
}

pub const SCALE_PRESETS: [ScalePreset; 9] = [
    ScalePreset::Mm28,
    ScalePreset::Mm32,
    ScalePreset::Mm35,
    ScalePreset::Mm54,
    ScalePreset::Mm75,
    ScalePreset::Ratio64,
    ScalePreset::Ratio48,
    ScalePreset::Ratio35,
    ScalePreset::Ratio12,
];

impl ScalePreset {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mm28 => "28mm",
            Self::Mm32 => "32mm",
            Self::Mm35 => "35mm",
            Self::Mm54 => "54mm",
            Self::Mm75 => "75mm",
            Self::Ratio64 => "1:64",
            Self::Ratio48 => "1:48",
            Self::Ratio35 => "1:35",
            Self::Ratio12 => "1:12",
        }
    }

    pub const fn is_ratio(self) -> bool {
        matches!(
            self,
            Self::Ratio64 | Self::Ratio48 | Self::Ratio35 | Self::Ratio12
        )
    }

    /// The nominal height of a 6 ft human at this scale.
    pub fn nominal_height_mm(self) -> f64 {
        match self {
            Self::Mm28 => 28.0,
            Self::Mm32 => 32.0,
            Self::Mm35 => 35.0,
            Self::Mm54 => 54.0,
            Self::Mm75 => 75.0,
            Self::Ratio64 => REFERENCE_HEIGHT_MM / 64.0,
            Self::Ratio48 => REFERENCE_HEIGHT_MM / 48.0,
            Self::Ratio35 => REFERENCE_HEIGHT_MM / 35.0,
            Self::Ratio12 => REFERENCE_HEIGHT_MM / 12.0,
        }
    }
}

pub fn mm_to_inches(mm: f64) -> f64 {
    mm / MM_PER_INCH
}

pub fn inches_to_mm(inches: f64) -> f64 {
    inches * MM_PER_INCH
}

/// Exact slicer scale percentage for moving from `source_mm` to `target_mm`.
pub fn scale_percent(source_mm: f64, target_mm: f64) -> Option<f64> {
    if source_mm.is_finite() && target_mm.is_finite() && source_mm > 0.0 && target_mm > 0.0 {
        Some(target_mm / source_mm * 100.0)
    } else {
        None
    }
}

/// Height at a target miniature scale for a real-world subject.
///
/// The target scale is defined by the height of a 6 ft human, so a 10 ft
/// subject at 35mm scale is `35 * 10 / 6 = 58.33mm`.
pub fn height_for_subject(subject_height_ft: f64, target_scale_mm: f64) -> Option<f64> {
    if subject_height_ft.is_finite()
        && subject_height_ft > 0.0
        && target_scale_mm.is_finite()
        && target_scale_mm > 0.0
    {
        Some(target_scale_mm * subject_height_ft / REFERENCE_HEIGHT_FT)
    } else {
        None
    }
}

/// Resize an existing model from one nominal scale to another.
pub fn target_height_mm(
    current_mm: f64,
    source_scale_mm: f64,
    target_scale_mm: f64,
) -> Option<f64> {
    if current_mm.is_finite()
        && current_mm > 0.0
        && source_scale_mm.is_finite()
        && source_scale_mm > 0.0
        && target_scale_mm.is_finite()
        && target_scale_mm > 0.0
    {
        Some(current_mm * target_scale_mm / source_scale_mm)
    } else {
        None
    }
}

/// A friendly recommendation from the prompt's base-diameter guide.
pub fn recommended_base_mm(subject_height_ft: f64) -> f64 {
    if !subject_height_ft.is_finite() || subject_height_ft <= 7.0 {
        25.0
    } else if subject_height_ft <= 10.0 {
        40.0
    } else if subject_height_ft <= 14.0 {
        50.0
    } else {
        65.0
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

    #[test]
    fn scale_percentage_28_to_35_is_125_percent() {
        assert_close(scale_percent(28.0, 35.0).unwrap(), 125.0);
    }

    #[test]
    fn custom_scale_percentage_is_exact() {
        assert_close(scale_percent(32.0, 28.0).unwrap(), 87.5);
        assert_close(scale_percent(35.0, 28.0).unwrap(), 80.0);
    }

    #[test]
    fn ratio_presets_use_a_six_foot_reference() {
        assert_close(ScalePreset::Ratio64.nominal_height_mm(), 28.575);
        assert_close(ScalePreset::Ratio48.nominal_height_mm(), 38.1);
        assert_close(
            ScalePreset::Ratio35.nominal_height_mm(),
            52.251_428_571_428_57,
        );
        assert_close(ScalePreset::Ratio12.nominal_height_mm(), 152.4);
    }

    #[test]
    fn ratio_to_miniature_conversion() {
        let source = ScalePreset::Ratio64.nominal_height_mm();
        let target = ScalePreset::Mm35.nominal_height_mm();
        assert_close(
            target / source * 100.0,
            35.0 * 64.0 / REFERENCE_HEIGHT_MM * 100.0,
        );
    }

    #[test]
    fn real_world_height_at_35mm() {
        assert_close(height_for_subject(6.0, 35.0).unwrap(), 35.0);
        assert_close(height_for_subject(10.0, 35.0).unwrap(), 175.0 / 3.0);
        assert_close(height_for_subject(10.0, 28.0).unwrap(), 140.0 / 3.0);
    }

    #[test]
    fn current_model_height_at_35mm() {
        assert_close(target_height_mm(28.0, 28.0, 35.0).unwrap(), 35.0);
        assert_close(target_height_mm(25.0, 32.0, 35.0).unwrap(), 27.343_75);
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(scale_percent(0.0, 35.0).is_none());
        assert!(scale_percent(-1.0, 35.0).is_none());
        assert!(height_for_subject(f64::NAN, 35.0).is_none());
        assert!(target_height_mm(28.0, 0.0, 35.0).is_none());
    }

    #[test]
    fn bases_follow_the_reference_guide() {
        assert_close(recommended_base_mm(6.0), 25.0);
        assert_close(recommended_base_mm(8.0), 40.0);
        assert_close(recommended_base_mm(12.0), 50.0);
        assert_close(recommended_base_mm(20.0), 65.0);
    }

    #[test]
    fn inches_and_millimetres_round_trip() {
        assert_close(mm_to_inches(25.4), 1.0);
        assert_close(inches_to_mm(1.0), 25.4);
    }
}
