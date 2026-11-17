//! The camera effects a world's [`Environment`](crate::Environment) sets beyond its light.
//!
//! Depth of field, motion blur, and a lens's vignette, chromatic aberration and film grain.

use crate::Reflect;

/// Depth of field: what's nearer or farther than the focus blurs.
///
/// A thin lens whose focal length is what the field of view gives on a 24 mm-high full-frame
/// sensor. A wider aperture, nearer focus or narrower field of view blurs more. World units are
/// meters. Perspective views only.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct DepthOfField {
    /// Distance ahead of the view that's in focus, in meters: 3 by default.
    #[reflect(min = 0.0, default = gg_renderer::DepthOfField::default().focus_distance)]
    pub focus_distance: f32,
    /// Focal length over aperture diameter: 1.4 by default, a fast lens's widest. Each stop up, by
    /// √2, halves the aperture's area and narrows the blur by √2.
    #[reflect(min = 0.5, default = gg_renderer::DepthOfField::default().f_number)]
    pub f_number: f32,
}

impl Default for DepthOfField {
    fn default() -> Self {
        let gg_renderer::DepthOfField {
            focus_distance,
            f_number,
        } = gg_renderer::DepthOfField::default();
        Self {
            focus_distance,
            f_number,
        }
    }
}

impl DepthOfField {
    /// It as the renderer takes it.
    pub(crate) fn rendered(&self) -> gg_renderer::DepthOfField {
        let &Self {
            focus_distance,
            f_number,
        } = self;
        gg_renderer::DepthOfField {
            focus_distance,
            f_number,
        }
    }
}

/// Motion blur along how far things moved while the shutter was open.
///
/// Models blur by their own motion; particles and a layer's own draws by the camera's alone; a skinned model by its parts'
/// transforms, not its joints'.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct MotionBlur {
    /// The share of the frame the shutter is open: 0.5 by default, film's 180° shutter; 0 blurs
    /// nothing.
    #[reflect(min = 0.0, max = 1.0, default = gg_renderer::MotionBlur::default().shutter)]
    pub shutter: f32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        let gg_renderer::MotionBlur { shutter } = gg_renderer::MotionBlur::default();
        Self { shutter }
    }
}

impl MotionBlur {
    /// It as the renderer takes it.
    pub(crate) fn rendered(&self) -> gg_renderer::MotionBlur {
        gg_renderer::MotionBlur {
            shutter: self.shutter,
        }
    }
}

/// What a camera's lens and film do to the view. Each is 0 for none; [`Lens::NONE`], the default,
/// sets all to 0 and costs nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct Lens {
    /// How much the view darkens toward its corners: 1 leaves them a quarter as bright, 0.5 about
    /// two thirds.
    #[reflect(min = 0.0)]
    pub vignette: f32,
    /// How far red and blue part toward the view's edges, red outward and blue inward: 1 parts them
    /// by a hundredth of the way from the view's middle.
    #[reflect(min = 0.0)]
    pub chromatic_aberration: f32,
    /// The most film grain scales each pixel's light up or down, as a share. The noise changes each
    /// frame, so two captures of a paused game differ where there's grain. 0.1 is subtle.
    #[reflect(min = 0.0, max = 1.0)]
    pub grain: f32,
}

impl Lens {
    /// No effect.
    pub const NONE: Self = Self {
        vignette: 0.0,
        chromatic_aberration: 0.0,
        grain: 0.0,
    };

    /// It as the renderer takes it.
    pub(crate) fn rendered(&self) -> gg_renderer::Lens {
        let &Self {
            vignette,
            chromatic_aberration,
            grain,
        } = self;
        gg_renderer::Lens {
            vignette,
            chromatic_aberration,
            grain,
        }
    }
}
