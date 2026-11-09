//! Environments: the component that says how the game's view shows its world's light, and the sky
//! around it, whatever viewpoint it sees from, and which of a world's environments the view renders
//! with.

use gg_core::math::libm::exp2f;
use gg_renderer::{AmbientOcclusion, Antialiasing, Fog, Grade, Sky, Texture, Tonemapper};
use tracing::warn;

use crate::{Component, Entity, EntityId, World};

/// How the game's view shows its world's light, whatever viewpoint it sees from, the editor's and
/// the tool API's looks' too, so that they show what the game does: its exposure, the tonemapper
/// that maps the view's colors, drawn in high dynamic range, into the range it shows, how it's
/// antialiased, the sky it shows behind the world and lights it by, how much it blooms, how it's
/// graded, and how what's near a point hides that light from it. A world has one, on an entity of
/// its own; of several, the view renders with the one of the lowest ID, warning
/// (`engine.environment_tie`). A world without one isn't tonemapped, antialiased, bloomed or
/// graded, so that what's in the range a view shows, as a 2D game's sprites, shows as it's drawn,
/// each pixel what's at its center, and lit shading exposes its light as the default environment's
/// exposure does.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Environment {
    /// How much light the view takes in, as a camera's exposure value at ISO 100, EV100: lit
    /// shading multiplies light, in lux and candela, by 1 / (1.2 × 2^EV100), so that each step up
    /// halves what shows. 15, the default, suits a sunny day; an overcast one about 12, and a lit
    /// room about 7. What isn't lit, as sprites, text and unlit materials, shows its colors at any.
    #[reflect(default = 15.0)]
    pub exposure: f32,
    /// How the view's colors, drawn in high dynamic range, map into the range it shows: Khronos PBR
    /// Neutral by default.
    pub tonemapper: Tonemapper,
    /// How the view is antialiased: temporal antialiasing by default, each frame drawn a fraction
    /// of a pixel off and resolved against those before it, before it's tonemapped, which a game's
    /// canvas is drawn after; or none.
    pub antialiasing: Antialiasing,
    /// The sky: an image of the world around, equirectangular, its middle column the way toward
    /// +X and a quarter of the way across the way toward -Z, its top row straight up, as Khronos's
    /// glTF Sample Viewer and three.js take one, which the view shows behind the world, in place of
    /// the clear color, and lights its models by from all around: a texture of colors, as a
    /// Radiance HDR's, `.hdr`, of high dynamic range, is. `None`, the default, for none. Its sun
    /// lights once: in a world with a directional light that shines, which stands for the sun, the
    /// sky's light keeps each of its texels to 10⁷ candela per square meter, leaving out the sun's
    /// disk, so that the sun lights through that light alone, which should shine from where the
    /// sky shows it.
    pub sky: Option<Texture>,
    /// The luminance, in candela per square meter, that a texel of the sky's of 1 stands for,
    /// which its colors are multiplied by, before the exposure, as light is: 30,000 by default,
    /// Filament's default for an environment's intensity, which suits a sunny day's exposure.
    #[reflect(default = 30_000.0)]
    pub sky_luminance: f32,
    /// How much the view blooms, light spreading round what's bright in it as through a camera's
    /// lens, from 0, for none, to 1: its colors, before they're tonemapped and before a game's
    /// canvas, lerped by this toward a weighted mean of them blurred further and further, which
    /// spreads light without adding any. 0.04, the default, is subtle: a glow round what's far
    /// brighter than white.
    #[reflect(min = 0.0, max = 1.0, default = 0.04)]
    pub bloom: f32,
    /// How the view's light is graded, before a game's canvas, which isn't graded, and before it's
    /// tonemapped: white balance, contrast, saturation and ASC CDL's slope, offset and power. The
    /// default, the identity, leaves it as it is.
    pub grade: Grade,
    /// How what's near each point the view shows darkens the light that reaches it from all
    /// around, the sky's, never the lights', as XeGTAO's ground-truth ambient occlusion finds it
    /// from the view's depth: on by default, with XeGTAO's radius of half a unit; `None` for none.
    /// Under a perspective with a sky alone, as without one there's no such light.
    #[reflect(default = Some(AmbientOcclusion::default()))]
    pub ambient_occlusion: Option<AmbientOcclusion>,
    /// The height fog the view is in, densest low down and thinning with height, lit by the sky
    /// from all around, or without one by a luminance of its own, and by the sun, the brightest
    /// directional light, as [`Fog`] says: `None`, the default, for none.
    pub fog: Option<Fog>,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            exposure: 15.0,
            tonemapper: Tonemapper::default(),
            antialiasing: Antialiasing::default(),
            sky: None,
            sky_luminance: 30_000.0,
            bloom: 0.04,
            grade: Grade::IDENTITY,
            ambient_occlusion: Some(AmbientOcclusion::default()),
            fog: None,
        }
    }
}

impl Environment {
    /// What lit shading multiplies light by at its exposure: 1 over the largest luminance that
    /// doesn't saturate a camera of that exposure value, 1.2 × 2^EV100, as Frostbite's and
    /// Filament's do (Lagarde and de Rousiers, "Moving Frostbite to Physically Based Rendering",
    /// 2014).
    pub fn light_scale(&self) -> f32 {
        1.0 / (1.2 * exp2f(self.exposure))
    }

    /// The sky the view shows, as the renderer takes it, if it has one.
    pub(crate) fn sky(&self) -> Option<Sky> {
        self.sky.map(|texture| Sky {
            texture,
            luminance: self.sky_luminance,
        })
    }
}

/// Environments that several entities have at once: the one the view renders with, of the lowest
/// ID, and how many there are.
pub(crate) type Tie = (EntityId, usize);

/// The environment the view renders `world` with: that of the lowest ID among its entities with an
/// [`Environment`], if any. Several warn (`engine.environment_tie`) as they come to be and as they
/// change, which `warned` keeps track of, rather than every frame.
pub(crate) fn seen(world: &World, warned: &mut Option<Tie>) -> Option<Environment> {
    let mut lowest: Option<(EntityId, Environment)> = None;
    let mut count = 0;
    for (entity, &environment) in world.query::<(Entity, &Environment)>() {
        let id = world.id(entity).expect("a query's entities are alive");
        count += 1;
        if lowest.is_none_or(|(low, _)| id < low) {
            lowest = Some((id, environment));
        }
    }
    let tie = lowest.filter(|_| count > 1).map(|(seen, _)| (seen, count));
    if let Some((seen, count)) = tie
        && tie != *warned
    {
        warn!(
            code = "engine.environment_tie",
            seen = %seen,
            count,
            "{count} entities have an Environment: the view renders with the one of the lowest \
             ID, {seen}"
        );
    }
    *warned = tie;
    lowest.map(|(_, environment)| environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_of_exposure_halves_the_light_that_shows() {
        let at = |exposure| Environment {
            exposure,
            ..Environment::default()
        };
        assert_eq!(at(0.0).light_scale(), 1.0 / 1.2);
        assert_eq!(at(15.0).light_scale() * 2.0, at(14.0).light_scale());
        // The sun's 100,000 lux on a white surface, which reflects 1/π of it a steradian, comes
        // to about 0.8 on a sunny day's exposure.
        let white = 100_000.0 / std::f32::consts::PI * at(15.0).light_scale();
        assert!((0.8..0.82).contains(&white), "{white}");
    }

    /// Fog, none by default, reflects whole: its description's default is the default fog, which an
    /// environment's fog switched on takes, and an environment in fog comes back from its value as
    /// it was, every field of its fog's among it.
    #[test]
    fn fog_reflects_whole() {
        use gg_core::math::Vec3;

        use crate::reflect::{Meta, Reflect, Types};

        let mut types = Types::default();
        let fog = Fog::describe(&mut types).expect("described");
        let default = (types.default_value(&fog, &Meta::default())).expect("a default");
        assert_eq!(Fog::from_value(&default).expect("made"), Fog::default());
        let foggy = Environment {
            fog: Some(Fog {
                density: 0.3,
                falloff: 0.9,
                height: 1.5,
                start: 2.0,
                opacity: 0.8,
                color: Vec3::new(0.9, 0.95, 1.0),
                luminance: 500.0,
                sun: 0.4,
                sharpness: 0.7,
            }),
            ..Environment::default()
        };
        let made = Environment::from_value(&foggy.to_value());
        assert_eq!(made.expect("made"), foggy);
        let clear = Environment::from_value(&Environment::default().to_value());
        assert_eq!(clear.expect("made").fog, None);
    }

    /// Of several environments the view renders with the one of the lowest ID, which a tie keeps
    /// warned of; with none, there's none.
    #[test]
    fn the_lowest_ids_environment_is_seen() {
        let mut world = World::default();
        world.register::<Environment>().expect("described");
        let mut warned = None;
        assert_eq!(seen(&world, &mut warned), None);
        let dim = Environment {
            exposure: 7.0,
            tonemapper: Tonemapper::Agx,
            ..Environment::default()
        };
        let first = world.spawn(dim);
        assert_eq!(seen(&world, &mut warned), Some(dim));
        assert_eq!(warned, None);
        let second = world.spawn(Environment::default());
        let [first, second] = [first, second].map(|entity| world.id(entity).expect("alive"));
        let lowest = first.min(second);
        let expected = if lowest == first {
            dim
        } else {
            Environment::default()
        };
        assert_eq!(seen(&world, &mut warned), Some(expected));
        assert_eq!(warned, Some((lowest, 2)));
    }
}
