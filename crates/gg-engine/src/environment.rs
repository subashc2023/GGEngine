//! Environments: the component that says how the game's view shows its world's light, and the sky
//! around it, whatever viewpoint it sees from, and which of a world's environments the view renders
//! with.

use gg_renderer::{AmbientOcclusion, Antialiasing, Fog, Grade, Reflections, Tonemapper};
use tracing::warn;

use crate::{Component, DepthOfField, Entity, EntityId, Lens, MotionBlur, Reflect, Sky, World};

/// How the game's view shows its world's light, whatever viewpoint it sees from, the editor's and
/// the tool API's looks' too, so that they show what the game does: its exposure, the tonemapper
/// that maps the view's colors, drawn in high dynamic range, into the range it shows, how it's
/// antialiased, the sky it shows behind the world and lights it by, how much it blooms, how it's
/// graded, how what's near a point hides that light from it, the fog it's in, how what's opaque
/// reflects what the view shows, and the camera's depth of field, motion blur and lens. A world has
/// one, on an entity of its own; of several, the view renders with the one of the lowest ID, warning
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
    /// How the exposure adapts to what the view sees, as eyes adapt between sunlight and shade, if
    /// it does, as [`Adaptation`] says: from `exposure` until the view's light is first metered,
    /// then toward what the meter calls for. `None`, the default, keeps `exposure`.
    pub adaptation: Option<Adaptation>,
    /// How the view's colors, drawn in high dynamic range, map into the range it shows: Khronos PBR
    /// Neutral by default.
    pub tonemapper: Tonemapper,
    /// How the view is antialiased: temporal antialiasing by default, each frame drawn a fraction
    /// of a pixel off and resolved against those before it, before it's tonemapped, which a game's
    /// canvas is drawn after; SMAA, each frame alone, at the same point, sharper and without ghosts;
    /// or none.
    pub antialiasing: Antialiasing,
    /// The sky, which the view shows behind the world, in place of the clear color, and lights its
    /// models by from all around: an image of the world around, or an atmosphere made from its
    /// sun, as [`Sky`] says. None, the default, for none.
    pub sky: Sky,
    /// The luminance, in candela per square meter, that a texel of an image sky's of 1 stands for,
    /// which its colors are multiplied by, before the exposure, as light is: 30,000 by default,
    /// Filament's default for an environment's intensity, which suits a sunny day's exposure. An
    /// atmosphere's is its sun's.
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
    /// directional light, as [`Fog`] says: `None`, the default, for none. Fog with a number that
    /// isn't finite, a NaN or an infinity, is drawn as none, with a warning (`engine.bad_fog`).
    pub fog: Option<Fog>,
    /// How what's opaque reflects what the view shows: each point seen in its mirror, by a ray
    /// traced through the view's depth, in place of the sky's specular light where it finds
    /// something, blurred as the point's roughness spreads it, as [`Reflections`] says. Under a
    /// perspective with a sky alone, as without one there's no specular light from all around for
    /// it to take the place of. `None`, the default, for none: it costs a trace of every pixel and
    /// two images of the view's size, which only a glossy surface makes worth it.
    pub reflections: Option<Reflections>,
    /// How what's nearer or farther than a focus blurs, as through a camera's lens, before a
    /// game's canvas, which isn't blurred, as [`DepthOfField`] says: `None`, the default, for none.
    pub depth_of_field: Option<DepthOfField>,
    /// How what moves across the view blurs along its way while the shutter's open, before a
    /// game's canvas, which isn't blurred, as [`MotionBlur`] says: `None`, the default, for none.
    pub motion_blur: Option<MotionBlur>,
    /// What a camera's lens and film do to the view, before a game's canvas, which shows none of it:
    /// a vignette, chromatic aberration and film grain, none of them by default.
    pub lens: Lens,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            exposure: 15.0,
            adaptation: None,
            tonemapper: Tonemapper::default(),
            antialiasing: Antialiasing::default(),
            sky: Sky::ClearColor,
            sky_luminance: 30_000.0,
            bloom: 0.04,
            grade: Grade::IDENTITY,
            ambient_occlusion: Some(AmbientOcclusion::default()),
            fog: None,
            reflections: None,
            depth_of_field: None,
            motion_blur: None,
            lens: Lens::NONE,
        }
    }
}

impl Environment {
    /// What lit shading multiplies light by at its exposure, as [`gg_renderer::light_scale`] has
    /// it: 1 over 1.2 × 2^EV100. An exposure that adapts starts from it.
    pub fn light_scale(&self) -> f32 {
        gg_renderer::light_scale(self.exposure)
    }

    /// The image sky the view shows, as the renderer takes it, if it has one.
    pub(crate) fn sky(&self) -> Option<gg_renderer::Sky> {
        match self.sky {
            Sky::Image(texture) => Some(gg_renderer::Sky {
                texture,
                luminance: self.sky_luminance,
            }),
            Sky::ClearColor | Sky::Atmosphere(_) => None,
        }
    }

    /// The atmosphere the view shows, as the renderer takes it, if it has one.
    pub(crate) fn atmosphere(&self) -> Option<gg_renderer::Atmosphere> {
        match self.sky {
            Sky::Atmosphere(atmosphere) => Some(atmosphere.rendered()),
            Sky::ClearColor | Sky::Image(_) => None,
        }
    }
}

/// The renderer's default adaptation, which the fields' defaults are.
fn adapted() -> gg_renderer::Adaptation {
    gg_renderer::Adaptation::default()
}

/// How an [`Environment`]'s exposure adapts to what the view sees, as eyes adapt between sunlight
/// and shade. Each frame meters the view's light, before it blooms and before a game's canvas, from
/// a histogram of its luminance, as Unreal's histogram exposure does: the mean log2 of the luminance
/// of its pixels between two percentiles of it, from 2^-12 candela per square meter, darker than
/// starlight, to 2^20, in eighths of a stop, what's darker, as a black sky, left out. The exposure
/// it calls for, log2(luminance × 100 / 12.5) as a reflected-light meter's at ISO 100, less the
/// compensation and kept between the bounds, is what the exposure moves toward, at speeds of its
/// own, by the simulation's time, so that a paused game's holds. The first frames, until a
/// metering comes back two frames later, shade at the environment's `exposure`, and that metering
/// is taken at once rather than moved toward. A change to the bounds, the compensation or the speeds
/// shows in the frame it's made, as a script easing the compensation each tick wants, the exposure
/// adapting on; one to the percentiles waits for a metering by them, taken at once. The tool API's
/// look adapts apart from the game's own view, so that a look round a dark room exposes neither
/// the game's view nor a capture of it for the room. What isn't lit, as sprites, text and unlit
/// materials, still shows its colors at any exposure, and is metered as though it were lit,
/// brighter the darker the exposure, so a view of little else runs to one bound or the other.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Adaptation {
    /// The lowest exposure value, EV100, it adapts to, at which what calls for a lower one shows
    /// darker: 0, about a street at night, so that a moonlit world shows as dark as it is.
    #[reflect(default = adapted().min)]
    pub min: f32,
    /// The highest, at which what calls for a higher one shows brighter: 18, past snow in sunlight.
    #[reflect(default = adapted().max)]
    pub max: f32,
    /// How many stops brighter than the meter calls for the view is exposed, as a camera's exposure
    /// compensation: 1 brightens it twice over, and -1 darkens it by half.
    pub compensation: f32,
    /// How fast the exposure rises toward a brighter view's, in stops a second: 3, as Unreal's.
    #[reflect(min = 0.0, default = adapted().speed_up)]
    pub speed_up: f32,
    /// How fast it falls toward a darker view's: 1, slower, as eyes adapt to the dark.
    #[reflect(min = 0.0, default = adapted().speed_down)]
    pub speed_down: f32,
    /// The percentile of the view's pixels by their luminance, as a fraction, below which they
    /// aren't metered: 0.1, leaving out the darkest tenth.
    #[reflect(min = 0.0, max = 1.0, default = adapted().low_percentile)]
    pub low_percentile: f32,
    /// The percentile above which they aren't: 0.9, leaving out the brightest tenth.
    #[reflect(min = 0.0, max = 1.0, default = adapted().high_percentile)]
    pub high_percentile: f32,
}

impl Default for Adaptation {
    fn default() -> Self {
        let gg_renderer::Adaptation {
            min,
            max,
            compensation,
            speed_up,
            speed_down,
            low_percentile,
            high_percentile,
        } = adapted();
        Self {
            min,
            max,
            compensation,
            speed_up,
            speed_down,
            low_percentile,
            high_percentile,
        }
    }
}

impl Adaptation {
    /// It as the renderer takes it.
    pub(crate) fn rendered(&self) -> gg_renderer::Adaptation {
        let &Self {
            min,
            max,
            compensation,
            speed_up,
            speed_down,
            low_percentile,
            high_percentile,
        } = self;
        gg_renderer::Adaptation {
            min,
            max,
            compensation,
            speed_up,
            speed_down,
            low_percentile,
            high_percentile,
        }
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

    /// Reflections, none by default, reflect whole: their description's default is the default
    /// reflections, which an environment's reflections switched on take, and an environment that
    /// reflects comes back from its value as it was, every field of its reflections' among it.
    #[test]
    fn reflections_reflect_whole() {
        use crate::reflect::{Meta, Reflect, Types};

        let mut types = Types::default();
        let reflections = Reflections::describe(&mut types).expect("described");
        let default = (types.default_value(&reflections, &Meta::default())).expect("a default");
        let made = Reflections::from_value(&default).expect("made");
        assert_eq!(made, Reflections::default());
        let glossy = Environment {
            reflections: Some(Reflections {
                distance: 30.0,
                thickness: 0.5,
                roughness: 0.4,
            }),
            ..Environment::default()
        };
        let made = Environment::from_value(&glossy.to_value());
        assert_eq!(made.expect("made"), glossy);
        let plain = Environment::from_value(&Environment::default().to_value());
        assert_eq!(plain.expect("made").reflections, None);
    }

    /// An adaptation, none by default, reflects whole: its description's default is the renderer's
    /// default, which an environment's adaptation switched on takes, and an environment that adapts
    /// comes back from its value as it was, handing the renderer each field as it has it.
    #[test]
    fn adaptation_reflects_whole() {
        use crate::reflect::{Meta, Reflect, Types};

        let mut types = Types::default();
        let described = Adaptation::describe(&mut types).expect("described");
        let default = (types.default_value(&described, &Meta::default())).expect("a default");
        let made = Adaptation::from_value(&default).expect("made");
        assert_eq!(made, Adaptation::default());
        assert_eq!(made.rendered(), gg_renderer::Adaptation::default());
        let adaptation = Adaptation {
            min: -3.0,
            max: 14.0,
            compensation: 0.5,
            speed_up: 2.5,
            speed_down: 0.75,
            low_percentile: 0.2,
            high_percentile: 0.95,
        };
        let adapting = Environment {
            adaptation: Some(adaptation),
            ..Environment::default()
        };
        let made = Environment::from_value(&adapting.to_value()).expect("made");
        assert_eq!(made, adapting);
        let rendered = gg_renderer::Adaptation {
            min: -3.0,
            max: 14.0,
            compensation: 0.5,
            speed_up: 2.5,
            speed_down: 0.75,
            low_percentile: 0.2,
            high_percentile: 0.95,
        };
        assert_eq!(adaptation.rendered(), rendered);
        assert_eq!(Environment::default().adaptation, None);
    }

    /// The camera's effects, none by default, reflect whole: their descriptions' defaults are their
    /// defaults, which an environment's switched on take, and an environment with all of them comes
    /// back from its value as it was, every field among it.
    #[test]
    fn the_cameras_effects_reflect_whole() {
        use crate::reflect::{Meta, Reflect, Types};

        let mut types = Types::default();
        let described = DepthOfField::describe(&mut types).expect("described");
        let default = (types.default_value(&described, &Meta::default())).expect("a default");
        let focus = DepthOfField::from_value(&default).expect("made");
        assert_eq!(focus, DepthOfField::default());
        assert_eq!(focus.rendered(), gg_renderer::DepthOfField::default());
        let described = MotionBlur::describe(&mut types).expect("described");
        let default = (types.default_value(&described, &Meta::default())).expect("a default");
        let blur = MotionBlur::from_value(&default).expect("made");
        assert_eq!(blur.rendered(), gg_renderer::MotionBlur::default());
        let filmed = Environment {
            depth_of_field: Some(DepthOfField {
                focus_distance: 7.5,
                f_number: 2.8,
            }),
            motion_blur: Some(MotionBlur { shutter: 0.25 }),
            lens: Lens {
                vignette: 0.4,
                chromatic_aberration: 1.5,
                grain: 0.1,
            },
            ..Environment::default()
        };
        let made = Environment::from_value(&filmed.to_value());
        assert_eq!(made.expect("made"), filmed);
        let plain = Environment::from_value(&Environment::default().to_value()).expect("made");
        assert_eq!(
            (plain.depth_of_field, plain.motion_blur, plain.lens),
            (None, None, Lens::NONE)
        );
        assert_eq!(filmed.lens.rendered().grain, 0.1);
    }

    /// An environment hands the renderer its image sky, at its luminance, or its atmosphere as it
    /// has it, never both, and neither without a sky.
    #[test]
    fn an_environment_hands_the_renderer_its_image_sky_or_its_atmosphere() {
        let with = |sky| Environment {
            sky,
            sky_luminance: 8000.0,
            ..Environment::default()
        };
        let image = with(Sky::Image(gg_renderer::Texture::WHITE));
        let shown = gg_renderer::Sky {
            texture: gg_renderer::Texture::WHITE,
            luminance: 8000.0,
        };
        assert_eq!((image.sky(), image.atmosphere()), (Some(shown), None));
        let hazy = crate::Atmosphere {
            mie: 20.0,
            altitude: 300.0,
            ..crate::Atmosphere::default()
        };
        let aired = with(Sky::Atmosphere(hazy));
        let rendered = gg_renderer::Atmosphere {
            mie: 20.0,
            altitude: 300.0,
            ..gg_renderer::Atmosphere::EARTH
        };
        assert_eq!((aired.sky(), aired.atmosphere()), (None, Some(rendered)));
        let clear = Environment::default();
        assert_eq!((clear.sky(), clear.atmosphere()), (None, None));
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
