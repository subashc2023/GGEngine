//! Skies: what a world's [`Environment`](crate::Environment) shows behind it and lights it by: an
//! image, or an atmosphere made from its sun.

use gg_core::math::Vec3;
use gg_renderer::Texture;

use crate::Reflect;

/// What the view shows behind the world in place of the clear color, and lights its models by from
/// all around, through the split sum, as an [`Environment`](crate::Environment) has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum Sky {
    /// The view's clear color shows, and nothing lights the world from all around.
    #[default]
    ClearColor,
    /// An equirectangular HDR image (like a Radiance `.hdr`), scaled by the environment's sky
    /// luminance. Its middle column faces +X, a quarter across faces -Z, its top row is up: as in
    /// Khronos's glTF Sample Viewer and three.js.
    ///
    /// Its sun lights once. With a shining directional light standing for the sun, the sky's light
    /// leaves out the sun, found by contrast with the sky round it, so the sun lights through that
    /// light alone. It should shine from where the sky shows the sun.
    Image(Texture),
    /// Hillaire's atmosphere, made from the world's brightest shining directional light, the sun.
    /// The sun's disk is drawn where it shines from, red and dim as it sets, gone at night when the
    /// sky is dark too. Its light reaches the world with its intensity taken outside the
    /// atmosphere, dimmed by the air to the viewer. Lit things are hazed by distance, a world unit
    /// taken as a meter.
    Atmosphere(Atmosphere),
}

/// Earth's atmosphere as the renderer's defaults have it; the fields' defaults.
const EARTH: gg_renderer::Atmosphere = gg_renderer::Atmosphere::EARTH;

/// Hillaire's atmosphere ("A Scalable and Production Ready Sky and Atmosphere Rendering Technique",
/// EGSR 2020), Earth's by default.
///
/// It models Rayleigh and Mie scattering and ozone absorption over an Earth-radius planet (6,360
/// km) up to 100 km above the ground, as in the paper. Scattering and absorption are in millionths
/// per meter.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Atmosphere {
    /// Rayleigh scattering at the ground per color, thinning over a scale height of 8 km. Makes
    /// the sky blue and sunsets red. Earth: (5.802, 13.558, 33.1).
    #[reflect(min = 0.0, default = EARTH.rayleigh)]
    pub rayleigh: Vec3,
    /// Mie (haze) scattering at the ground, thinning by a scale height of 1.2 km. Whitens the sky
    /// round the sun and toward the horizon. Earth: 3.996.
    #[reflect(min = 0.0, default = EARTH.mie)]
    pub mie: f32,
    /// Haze absorption, same units. Earth: 0.444.
    #[reflect(min = 0.0, default = EARTH.mie_absorption)]
    pub mie_absorption: f32,
    /// Haze scattering direction by Henyey-Greenstein: -1 all back, 1 all onward. 0.8 is a glow
    /// round the sun.
    #[reflect(min = -0.99, max = 0.99, default = EARTH.mie_anisotropy)]
    pub mie_anisotropy: f32,
    /// Ozone absorption per color, densest 25 km up and zero 15 km above and below. Earth:
    /// (0.650, 1.881, 0.085). Keeps twilight skies blue.
    #[reflect(min = 0.0, default = EARTH.ozone)]
    pub ozone: Vec3,
    /// Ground albedo: shows below the horizon and lights the air from below.
    #[reflect(color, default = EARTH.ground_albedo)]
    pub ground_albedo: Vec3,
    /// The world origin's height above the ground, in meters. The viewer's is this plus its Y,
    /// kept 10 m above the ground and below the atmosphere's top.
    pub altitude: f32,
    /// The angle the sun's disk spans: 0.5357°.
    #[reflect(degrees, min = 0.0, default = EARTH.sun_size)]
    pub sun_size: f32,
}

impl Default for Atmosphere {
    /// Earth's, with the origin on the ground.
    fn default() -> Self {
        let gg_renderer::Atmosphere {
            rayleigh,
            mie,
            mie_absorption,
            mie_anisotropy,
            ozone,
            ground_albedo,
            altitude,
            sun_size,
        } = EARTH;
        Self {
            rayleigh,
            mie,
            mie_absorption,
            mie_anisotropy,
            ozone,
            ground_albedo,
            altitude,
            sun_size,
        }
    }
}

impl Atmosphere {
    /// It as the renderer takes it.
    pub(crate) fn rendered(&self) -> gg_renderer::Atmosphere {
        let &Self {
            rayleigh,
            mie,
            mie_absorption,
            mie_anisotropy,
            ozone,
            ground_albedo,
            altitude,
            sun_size,
        } = self;
        gg_renderer::Atmosphere {
            rayleigh,
            mie,
            mie_absorption,
            mie_anisotropy,
            ozone,
            ground_albedo,
            altitude,
            sun_size,
        }
    }
}
