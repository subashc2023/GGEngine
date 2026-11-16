//! Color grading (`shaders/grade.slang`), which the bloom pass applies before the canvas and the
//! tonemapper.

use bytemuck::NoUninit;
use gg_core::math::libm::{exp2f, log2f};
use gg_core::math::{Mat3, Vec2, Vec3};

/// How the view's linear light is graded before tonemapping, as
/// [`RenderData::grade`](crate::RenderData::grade) has it, so the tonemapper rolls off what it
/// brightens. In order: white balance, contrast, ASC CDL's slope, offset and power, saturation.
///
/// - Each step keeps light within a half float's largest, 65,504, as the view's colors hold.
/// - A game's canvas, drawn after, isn't graded.
/// - The default, the identity, costs nothing.
/// - A field that isn't finite is taken as the identity's, and one past its range as its range's
///   end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    /// White balance from -1 (cooler, bluer) to 1 (warmer, amber), by von Kries in CAT02's LMS. The
    /// view balances to D65 from daylight of 4,000 K at -1, or as many mireds the other way at 1
    /// (about 17,400 K). 0 for none.
    pub temperature: f32,
    /// White balance tint from -1 (greener) to 1 (magenta): the daylight balanced from moves toward
    /// green by 0.05 of CIE's y at 1. 0 for none.
    pub tint: f32,
    /// Each channel's power round mid gray (0.18), from 0 to [`MOST`](Self::MOST): above 1 more
    /// contrast, below less. 1 for none.
    pub contrast: f32,
    /// ASC CDL's saturation round Rec. 709 luminance: 0 is gray, above 1 more colorful. 1 for none.
    pub saturation: f32,
    /// ASC CDL's slope, multiplying each channel, from 0 to [`MOST`](Self::MOST). 1 for none.
    pub slope: Vec3,
    /// ASC CDL's offset, added to each channel after the slope. 0 for none.
    pub offset: Vec3,
    /// ASC CDL's power, raising each channel round 1 after the offset, from 0 to
    /// [`MOST`](Self::MOST). 1 for none.
    pub power: Vec3,
}

impl Grade {
    pub const IDENTITY: Self = Self {
        temperature: 0.0,
        tint: 0.0,
        contrast: 1.0,
        saturation: 1.0,
        slope: Vec3::ONE,
        offset: Vec3::ZERO,
        power: Vec3::ONE,
    };

    /// The most contrast and each channel of slope and power may be: two stops of slope. Contrast
    /// or power of 4 take light barely 4 stops above mid gray past a half float's largest.
    pub const MOST: f32 = 4.0;

    /// Whether it's the identity once fields that aren't finite are taken as the identity's. The
    /// view isn't graded then.
    pub fn is_identity(&self) -> bool {
        self.finite() == Self::IDENTITY
    }

    /// What it makes of finite, non-negative linear sRGB `light`, as the grading pass computes it
    /// in 32-bit floats.
    pub fn graded(&self, light: Vec3) -> Vec3 {
        if self.is_identity() {
            return light;
        }
        let grade = self.finite();
        let brightest = Vec3::splat(BRIGHTEST);
        let light = (grade.balance() * light).clamp(Vec3::ZERO, brightest);
        let contrast = Vec3::splat(grade.contrast);
        let light = (raised(light / MID_GRAY, contrast) * MID_GRAY).min(brightest);
        let light = (light * grade.slope + grade.offset).max(Vec3::ZERO);
        let light = raised(light, grade.power).min(brightest);
        let luma = light.dot(LUMA);
        (luma + grade.saturation * (light - luma)).clamp(Vec3::ZERO, brightest)
    }

    /// Its slope, finite and in range, as the grading pass takes it.
    pub(crate) fn slope(&self) -> Vec3 {
        self.finite().slope
    }

    pub(crate) fn data(&self) -> GradeData {
        let grade = self.finite();
        let balance = grade.balance().transpose().to_cols_array_2d();
        let row = |i: usize, w: f32| {
            let [x, y, z] = balance[i];
            [x, y, z, w]
        };
        GradeData {
            balance: [
                row(0, grade.contrast),
                row(1, grade.saturation),
                row(2, 0.0),
            ],
            slope: grade.slope.extend(0.0).to_array(),
            offset: grade.offset.extend(0.0).to_array(),
            power: grade.power.extend(0.0).to_array(),
        }
    }

    /// It with fields that aren't finite the identity's, and each within its range.
    fn finite(&self) -> Self {
        let or = |value: f32, identity: f32| if value.is_finite() { value } else { identity };
        let or3 = |value: Vec3, identity: Vec3| {
            Vec3::new(
                or(value.x, identity.x),
                or(value.y, identity.y),
                or(value.z, identity.z),
            )
        };
        let identity = Self::IDENTITY;
        let (least, most) = (Vec3::ZERO, Vec3::splat(Self::MOST));
        Self {
            temperature: or(self.temperature, 0.0).clamp(-1.0, 1.0),
            tint: or(self.tint, 0.0).clamp(-1.0, 1.0),
            contrast: or(self.contrast, 1.0).clamp(0.0, Self::MOST),
            saturation: or(self.saturation, 1.0).max(0.0),
            slope: or3(self.slope, identity.slope).clamp(least, most),
            offset: or3(self.offset, identity.offset),
            power: or3(self.power, identity.power).clamp(least, most),
        }
    }

    /// Its white balance, linear sRGB to linear sRGB.
    fn balance(&self) -> Mat3 {
        white_balance(self.temperature, self.tint)
    }
}

impl Default for Grade {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// `Grade` in `shaders/grade.slang`, field for field: the white balance's rows, the first two's
/// fourth components the contrast and the saturation, then ASC CDL's slope, offset and power.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct GradeData {
    balance: [[f32; 4]; 3],
    slope: [f32; 4],
    offset: [f32; 4],
    power: [f32; 4],
}

impl GradeData {
    /// What a pass that doesn't grade pushes.
    pub(crate) const NONE: Self = Self {
        balance: [[0.0; 4]; 3],
        slope: [0.0; 4],
        offset: [0.0; 4],
        power: [0.0; 4],
    };
}

/// A half float's largest finite value, the view colors': `BRIGHTEST` in `shaders/common.slang`.
const BRIGHTEST: f32 = 65504.0;

const MID_GRAY: f32 = 0.18;

/// Luminance weights: `LUMA` in `shaders/grade.slang`.
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// `light` to the `power`, per channel, 0 giving 0 at any power (even 0) rather than NaN, as
/// `raised` in `shaders/grade.slang` does.
fn raised(light: Vec3, power: Vec3) -> Vec3 {
    let raise = |light: f32, power: f32| exp2f(power * log2f(light.max(1.0e-10)));
    Vec3::new(
        raise(light.x, power.x),
        raise(light.y, power.y),
        raise(light.z, power.z),
    )
}

/// Linear sRGB (Rec. 709 primaries, D65 white) to CIE XYZ, per IEC 61966-2-1, row by row.
const XYZ_FROM_SRGB: [[f32; 3]; 3] = [
    [0.4124, 0.3576, 0.1805],
    [0.2126, 0.7152, 0.0722],
    [0.0193, 0.1192, 0.9505],
];

/// CIE XYZ to CIECAM02's chromatic adaptation LMS, CAT02, row by row.
const LMS_FROM_XYZ: [[f32; 3]; 3] = [
    [0.7328, 0.4296, -0.1624],
    [-0.7036, 1.6975, 0.0061],
    [0.0030, 0.0136, 0.9834],
];

fn rows(rows: [[f32; 3]; 3]) -> Mat3 {
    Mat3::from_cols_array_2d(&rows).transpose()
}

/// D65's 6,504 K in mireds (a million over kelvin), whose equal steps look about equally far apart.
const D65_MIREDS: f32 = 1.0e6 / 6504.0;

/// Mireds per unit of temperature: D65's to 4,000 K's at -1.
const MIREDS: f32 = 1.0e6 / 4000.0 - D65_MIREDS;

/// The tint's shift per unit, in CIE's y.
const TINT: f32 = 0.05;

/// Daylight's chromaticity at `mireds`, by CIE's daylight locus (CIE 15:2004), which spans 4,000 K
/// to 25,000 K.
fn daylight(mireds: f32) -> Vec2 {
    let t = mireds * 1.0e-3;
    let x = if mireds >= 1.0e6 / 7000.0 {
        -4.6070 * t * t * t + 2.9678 * t * t + 0.09911 * t + 0.244063
    } else {
        -2.0064 * t * t * t + 1.9018 * t * t + 0.24748 * t + 0.237040
    };
    Vec2::new(x, -3.0 * x * x + 2.870 * x - 0.275)
}

/// The LMS of the white at chromaticity `xy`, luminance 1.
fn white(xy: Vec2) -> Vec3 {
    let xyz = Vec3::new(xy.x / xy.y, 1.0, (1.0 - xy.x - xy.y) / xy.y);
    rows(LMS_FROM_XYZ) * xyz
}

/// The white balance [`Grade`] describes, linear sRGB to linear sRGB: into LMS, each channel scaled
/// by D65 daylight's over the balanced-from white's, and back.
fn white_balance(temperature: f32, tint: f32) -> Mat3 {
    if temperature == 0.0 && tint == 0.0 {
        return Mat3::IDENTITY;
    }
    let neutral = daylight(D65_MIREDS);
    let from = daylight(D65_MIREDS - MIREDS * temperature) + Vec2::new(0.0, TINT * tint);
    let lms_from_srgb = rows(LMS_FROM_XYZ) * rows(XYZ_FROM_SRGB);
    let scale = Mat3::from_diagonal(white(neutral) / white(from));
    lms_from_srgb.inverse() * scale * lms_from_srgb
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity is the default and leaves light alone; a changed field doesn't. Fields that
    /// aren't finite are the identity's, those out of range clamped.
    #[test]
    fn the_identity_is_the_default_and_no_number_is_taken_as_it() {
        assert!(Grade::default().is_identity());
        let light = Vec3::new(0.8, 0.1, 0.05);
        assert_eq!(Grade::IDENTITY.graded(light), light);
        let warm = Grade {
            temperature: 0.1,
            ..Grade::IDENTITY
        };
        assert!(!warm.is_identity());
        let broken = Grade {
            contrast: f32::NAN,
            slope: Vec3::new(1.0, f32::INFINITY, 1.0),
            ..Grade::IDENTITY
        };
        assert!(broken.is_identity());
        let past = Grade {
            contrast: 20.0,
            power: Vec3::splat(-1.0),
            ..Grade::IDENTITY
        };
        let ends = past.finite();
        assert_eq!((ends.contrast, ends.power), (Grade::MOST, Vec3::ZERO));
    }

    /// A grade keeps a half float's brightest light finite and no brighter: the most contrast,
    /// slope and power, saturated, give a bright color its darkest channel at 0, not ∞ − ∞ (NaN).
    #[test]
    fn a_grade_keeps_light_within_a_half_float() {
        let wild = Grade {
            contrast: Grade::MOST,
            saturation: 8.0,
            slope: Vec3::splat(Grade::MOST),
            power: Vec3::splat(Grade::MOST),
            ..Grade::IDENTITY
        };
        for light in [Vec3::splat(BRIGHTEST), Vec3::new(BRIGHTEST, 30.0, 0.0)] {
            let graded = wild.graded(light);
            assert!(graded.is_finite(), "{light} grades to {graded}");
            assert!(graded.max_element() <= BRIGHTEST && graded.min_element() >= 0.0);
        }
        let white = wild.graded(Vec3::splat(BRIGHTEST));
        assert!((white - BRIGHTEST).abs().max_element() < 1.0, "{white}");
    }

    /// Saturation keeps grays gray, as Rec. 709's weights sum to 1, and moves colors from their
    /// luminance and back.
    #[test]
    fn saturation_keeps_grays_and_moves_colors_from_their_luminance() {
        let more = Grade {
            saturation: 2.0,
            ..Grade::IDENTITY
        };
        let gray = more.graded(Vec3::splat(0.5));
        assert!(
            (gray - Vec3::splat(0.5)).abs().max_element() < 1.0e-6,
            "{gray}"
        );
        let red = Vec3::new(0.4, 0.2, 0.1);
        let luma = red.dot(LUMA);
        let expected = (luma + 2.0 * (red - luma)).max(Vec3::ZERO);
        assert!((more.graded(red) - expected).abs().max_element() < 1.0e-6);
    }

    /// White balance leaves white alone at 0, warms it toward amber or cools it toward blue with
    /// temperature, and tints it magenta (more red and blue than green) with tint. Gray stays gray
    /// at D65's daylight.
    #[test]
    fn white_balance_warms_cools_and_tints_white() {
        assert_eq!(white_balance(0.0, 0.0), Mat3::IDENTITY);
        let white = |temperature, tint| white_balance(temperature, tint) * Vec3::ONE;
        let warm = white(0.5, 0.0);
        assert!(warm.x > 1.0 && warm.z < 1.0, "{warm}");
        let cool = white(-0.5, 0.0);
        assert!(cool.x < 1.0 && cool.z > 1.0, "{cool}");
        let magenta = white(0.0, 0.5);
        assert!(magenta.x > magenta.y && magenta.z > magenta.y, "{magenta}");
        // D65's daylight is within a thousandth of sRGB's white in x and y, so the matrices' white
        // is close to gray.
        let neutral = daylight(D65_MIREDS);
        assert!((neutral - Vec2::new(0.3127, 0.3290)).abs().max_element() < 1.0e-3);
        // -1 balances from 4,000 K, and 1 as many mireds the other way.
        let [cold, hot] = [-1.0, 1.0].map(|t| D65_MIREDS - MIREDS * t);
        assert!((cold - 250.0).abs() < 1.0e-3, "{cold}");
        assert!((1.0e6 / hot - 17_391.0).abs() < 1.0, "{}", 1.0e6 / hot);
    }
}
