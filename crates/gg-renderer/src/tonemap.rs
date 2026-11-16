//! Tonemapping: the view's HDR colors into its image's 0 to 1, dithered against banding in its 8
//! bits.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_gpu::{Blend, ColorImage, Device, GraphicsDesc, Pass, Pipeline};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tonemap.spv"));

/// How the view's HDR linear RGB maps into 0 to 1, as
/// [`RenderData::tonemapper`](crate::RenderData::tonemapper) picks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tonemapper {
    /// Khronos PBR Neutral: base colors up to about 0.76 show as they are, less a dielectric's 0.04
    /// reflection, so glTF materials keep their colors. Brighter light rolls off toward white,
    /// slightly desaturated.
    #[default]
    PbrNeutral,
    /// ACES, as Stephen Hill fitted its reference rendering and output transforms. The film look:
    /// more contrast, darker mid-tones, shifted hues, saturated highlights.
    Aces,
    /// AgX, Blender's default since 4.0, as three.js has it. Very bright saturated light doesn't
    /// clip to its primaries, but every color shifts a little.
    Agx,
    /// Each channel clamped to 0 to 1, undithered, as a world without an environment shows.
    None,
}

impl Tonemapper {
    /// In index order.
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

/// Maps the view's HDR colors into its image, of [`ColorImage::FORMAT`], pixel for pixel.
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
                ids_write: false,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self { pipeline })
    }

    /// Draws the view's colors, at bindless index `colors` from [`Pass::sampled`], over the whole
    /// target, mapped by `tonemapper` and dithered unless it's [`Tonemapper::None`].
    ///
    /// # Panics
    ///
    /// - If the pass's target isn't of [`ColorImage::FORMAT`], without IDs or a depth image.
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

/// `Push` in `shaders/tonemap.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct TonemapPush {
    image: u32,
    /// The tonemapper's [`index`](Tonemapper::index).
    tonemapper: u32,
}
