//! The camera's effects that a world's [`Environment`](crate::Environment) sets beyond its light:
//! depth of field, from a focus and an aperture; motion blur, from a shutter; and a lens's
//! vignette, chromatic aberration and film grain.

use crate::Reflect;

/// Depth of field: what's nearer or farther than the focus blurred, as through a camera's lens, a
/// thin lens's of the focal length that the view's field of view takes on a full-frame camera's
/// sensor, 24 mm high, so that a wider aperture, a nearer focus or a narrower field of view blurs
/// more, as a camera's do. The world's units are taken as meters. Under a perspective alone, as
/// without one there's no lens.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct DepthOfField {
    /// How far ahead of the view what's in focus is: 3 by default.
    #[reflect(min = 0.0, default = gg_renderer::DepthOfField::default().focus_distance)]
    pub focus_distance: f32,
    /// The lens's focal length over its aperture's diameter: 1.4 by default, a fast lens's widest;
    /// each stop up, by √2, halves the aperture's area and narrows the blur by √2.
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

/// Motion blur: what the view shows blurred along how far it moved across the view while the
/// shutter was open, as a film camera's is: the world's models by their own motion, and what draws
/// none of its own, as particles and a layer's own draws, by the camera's alone, and a skinned
/// model by its parts' transforms, not its joints'.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct MotionBlur {
    /// The share of the frame the shutter is open: a half by default, a 180° shutter's, film's; 0
    /// blurs nothing.
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

/// What a camera's lens and film do to the view, each from 0, for none, which [`Lens::NONE`], the
/// default, has of each, and costs nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct Lens {
    /// How much the view darkens toward its corners, as a lens's light falls off away from its
    /// axis: 1 leaves its corners a quarter as bright, 0.5 about two thirds.
    #[reflect(min = 0.0)]
    pub vignette: f32,
    /// How far red and blue part toward the view's edges, red outward and blue inward, as a lens
    /// bends each color its own way: 1 parts them by a hundredth of the way from the view's middle.
    #[reflect(min = 0.0)]
    pub chromatic_aberration: f32,
    /// How much film grain scales each pixel's light by, either way, as a share, at most, noise
    /// that changes each frame, so that two captures of a paused game differ where there's grain:
    /// 0.1 is subtle.
    #[reflect(min = 0.0, max = 1.0)]
    pub grain: f32,
}

impl Lens {
    /// None of it.
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
