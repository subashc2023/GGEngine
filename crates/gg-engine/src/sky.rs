//! Skies: what a world's [`Environment`](crate::Environment) shows behind it and lights it by from
//! all around: an image of the world around, or an atmosphere made from its sun.

use gg_core::math::Vec3;
use gg_renderer::Texture;

use crate::Reflect;

/// What the view shows behind the world, in place of the clear color, and lights its models by
/// from all around, through the split sum, as an [`Environment`](crate::Environment) has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum Sky {
    /// None: the view's clear color shows, and nothing lights the world from all around.
    #[default]
    ClearColor,
    /// An image of the world around, equirectangular, its middle column the way toward +X and a
    /// quarter of the way across the way toward -Z, its top row straight up, as Khronos's glTF
    /// Sample Viewer and three.js take one: a texture of colors, as a Radiance HDR's, `.hdr`, of
    /// high dynamic range, is, which the environment's sky luminance scales. Its sun lights once:
    /// in a world with a directional light that shines, which stands for the sun, the sky's light
    /// keeps each of its texels to 10⁷ candela per square meter, leaving out the sun's disk, so
    /// that the sun lights through that light alone, which should shine from where the sky shows
    /// it.
    Image(Texture),
    /// Hillaire's atmosphere, made from its sun, the world's brightest directional light that
    /// shines: the sky's light where it shines from, its disk drawn there, red and dim as it sets
    /// and gone at night, when the sky's dark too; and the light that reaches the world from it,
    /// whose intensity is taken outside the atmosphere, dimmed by the air between it and the
    /// viewer. What the world's lights light is hazed by how far it is from the viewer, the world's
    /// units taken as meters.
    Atmosphere(Atmosphere),
}

/// Earth's atmosphere, as the renderer's defaults have it, which the fields' defaults are.
const EARTH: gg_renderer::Atmosphere = gg_renderer::Atmosphere::EARTH;

/// Hillaire's atmosphere ("A Scalable and Production Ready Sky and Atmosphere Rendering
/// Technique", EGSR 2020): Rayleigh's and Mie's scattering and ozone's absorption over a planet of
/// Earth's radius, 6,360 km, up to its top, 100 km above the ground, as the paper's. Earth's by
/// default, its scattering and absorption in millionths per meter, as they're quoted.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Atmosphere {
    /// Rayleigh's scattering at the ground, of red, green and blue, thinning with height by a
    /// scale height of 8 km: what makes the sky blue and the sunset red. Earth's is (5.802,
    /// 13.558, 33.1).
    #[reflect(min = 0.0, default = EARTH.rayleigh)]
    pub rayleigh: Vec3,
    /// Mie's scattering at the ground, the haze's, thinning by a scale height of 1.2 km, which
    /// whitens the sky round the sun and toward the horizon. Earth's is 3.996.
    #[reflect(min = 0.0, default = EARTH.mie)]
    pub mie: f32,
    /// What the haze absorbs, likewise: 0.444 for Earth's.
    #[reflect(min = 0.0, default = EARTH.mie_absorption)]
    pub mie_absorption: f32,
    /// How much of its light the haze scatters onward, from -1 all back the way it came to 1 all
    /// onward, by Henyey-Greenstein's phase function: 0.8, a glow round the sun.
    #[reflect(min = -0.99, max = 0.99, default = EARTH.mie_anisotropy)]
    pub mie_anisotropy: f32,
    /// Ozone's absorption, of red, green and blue, where it's densest, 25 km up, thinning to none
    /// 15 km above and below: (0.650, 1.881, 0.085), which keeps a twilight sky blue.
    #[reflect(min = 0.0, default = EARTH.ozone)]
    pub ozone: Vec3,
    /// The planet's ground's albedo, which shows below the horizon and lights the air from below.
    #[reflect(color, default = EARTH.ground_albedo)]
    pub ground_albedo: Vec3,
    /// How far above the planet's ground the world's origin is, in meters: the viewer is as high
    /// as this and its Y, kept 10 m above the ground and below the atmosphere's top.
    pub altitude: f32,
    /// The angle the sun's disk spans across: 0.5357°.
    #[reflect(degrees, min = 0.0, default = EARTH.sun_size)]
    pub sun_size: f32,
}

impl Default for Atmosphere {
    /// Earth's, the world's origin on the ground.
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
