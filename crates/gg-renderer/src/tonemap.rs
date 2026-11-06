//! Tonemapping: the pass that maps the game's view, drawn in colors of high dynamic range, into the
//! range the view's image shows, 0 to 1, by a tonemapper.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_gpu::{Blend, ColorImage, Device, GraphicsDesc, Pass, Pipeline};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tonemap.spv"));

/// How the game's view, drawn in colors of high dynamic range, maps into the range its image shows,
/// 0 to 1, from linear RGB, as [`RenderData::tonemapper`](crate::RenderData::tonemapper) names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tonemapper {
    /// Khronos PBR Neutral, which shows base colors up to about 0.76 as they are, less the 0.04
    /// of light that a dielectric reflects whatever its color, so that glTF materials keep their
    /// colors, and rolls brighter light off toward white, desaturating it a little.
    #[default]
    PbrNeutral,
    /// ACES, as Stephen Hill fitted its reference rendering and output transforms: the film look,
    /// of higher contrast, darkening mid-tones, shifting hues and saturating what's bright.
    Aces,
    /// AgX, Blender's default since 4.0, as three.js has it: a filmic curve that keeps very bright,
    /// saturated light from clipping to its primaries, but shifts every color a little.
    Agx,
    /// None: each channel clamped to 0 to 1, so that what's in that range shows as it was drawn,
    /// as the view of a world without an environment does.
    None,
}

impl Tonemapper {
    /// Each tonemapper, in the order of their indices.
    pub const ALL: [Self; 4] = [Self::PbrNeutral, Self::Aces, Self::Agx, Self::None];

    /// Its index in [`ALL`](Self::ALL), which the tonemap shader switches on.
    pub fn index(self) -> u32 {
        match self {
            Self::PbrNeutral => 0,
            Self::Aces => 1,
            Self::Agx => 2,
            Self::None => 3,
        }
    }
}

/// Maps the view's colors of high dynamic range into the view's image, of [`ColorImage::FORMAT`],
/// each pixel onto the one of the image.
pub(crate) struct TonemapPass {
    pipeline: Pipeline,
}

impl TonemapPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let pipeline = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "tonemap",
                spirv: SPIRV,
                vertex: c"vertex_main",
                mesh: None,
                fragment: Some(c"fragment_main"),
                format: ColorImage::FORMAT,
                blend: Blend::Opaque,
                depth_write: false,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws over the whole of the pass's target, the view's image, the view's colors, whose index
    /// [`Pass::sampled`] gave this pass as `colors`, mapped by `tonemapper`.
    ///
    /// # Panics
    ///
    /// If the pass's target isn't of [`ColorImage::FORMAT`], without IDs or a depth image.
    pub(crate) fn draw(&self, pass: &mut Pass<'_, '_>, colors: u32, tonemapper: Tonemapper) {
        pass.set_pipeline(&self.pipeline);
        let push = TonemapPush {
            image: colors,
            tonemapper: tonemapper.index(),
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
    }
}

/// What the tonemap pass pushes: `Push` in `shaders/tonemap.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct TonemapPush {
    /// The index of the view's colors in the bindless set.
    image: u32,
    /// The tonemapper's [`index`](Tonemapper::index).
    tonemapper: u32,
}
