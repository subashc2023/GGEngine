//! Environments: the component that says how the game's view shows its world's light and sky,
//! whatever viewpoint it sees from, and which of a world's environments the view renders with.

use gg_renderer::{AmbientOcclusion, Antialiasing, Fog, Grade, Reflections, Tonemapper};
use tracing::warn;

use crate::{Component, DepthOfField, Entity, EntityId, Lens, MotionBlur, Reflect, Sky, World};

/// How the game's view shows its world's light, from any viewpoint.
///
/// The editor's and the tool API's looks use it too. A world has one, on an entity of its own; of several, the view uses
/// the lowest ID and warns (`engine.environment_tie`). Without one, a world isn't tonemapped,
/// antialiased, bloomed or graded, so in-range content like 2D sprites shows as drawn, each pixel
/// what's at its center, and lit shading exposes as the default environment does.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Environment {
    /// Exposure value at ISO 100 (EV100). Lit shading scales light (lux, candela) by
    /// 1 / (1.2 × 2^EV100): each step up halves what shows. 15 (default) suits a sunny day, about
    /// 12 overcast, about 7 a lit room. Unlit content (sprites, text, unlit materials) is
    /// unaffected.
    #[reflect(default = 15.0)]
    pub exposure: f32,
    /// How exposure adapts to the view's light, as eyes do ([`Adaptation`]): `exposure` until
    /// first metered, then toward the meter. `None` (default) keeps `exposure`.
    pub adaptation: Option<Adaptation>,
    /// Maps the view's HDR colors into the shown range. Khronos PBR Neutral by default.
    pub tonemapper: Tonemapper,
    /// Temporal (default): each frame is drawn a fraction of a pixel off and resolved against
    /// earlier ones, before tonemapping; a game's canvas is drawn after. SMAA: each frame alone,
    /// sharper, no ghosts. Or none.
    pub antialiasing: Antialiasing,
    /// What shows behind the world in place of the clear color and lights models from all
    /// around ([`Sky`]). Default: none.
    pub sky: Sky,
    /// Candela per square meter that 1 in an image sky's texels stands for; colors are scaled by
    /// it before exposure, as light is. 30,000 by default, Filament's default, suiting a sunny
    /// day's exposure. An atmosphere uses its sun's.
    #[reflect(default = 30_000.0)]
    pub sky_luminance: f32,
    /// Bloom, from 0 for none to 1. Before tonemapping and a game's canvas, colors are lerped by
    /// this toward a weighted mean of progressively blurrier copies: light spreads, none is
    /// added. 0.04 (default) is a subtle glow round what's far brighter than white.
    #[reflect(min = 0.0, max = 1.0, default = 0.04)]
    pub bloom: f32,
    /// Grading before tonemapping (a game's canvas isn't graded): white balance, contrast,
    /// saturation, ASC CDL slope, offset and power. The default, identity, changes nothing.
    pub grade: Grade,
    /// How nearby geometry darkens the sky's light (never the lights') at each point, by
    /// XeGTAO's ambient occlusion from the view's depth. On by default, at XeGTAO's radius of
    /// half a unit; `None` for none. Perspective views with a sky only.
    #[reflect(default = Some(AmbientOcclusion::default()))]
    pub ambient_occlusion: Option<AmbientOcclusion>,
    /// Height fog, densest low and thinning upward, lit by the sky (without one, its own
    /// luminance) and the sun, the brightest directional light ([`Fog`]). `None` (default) for
    /// none. Fog with a non-finite number is drawn as none, with a warning (`engine.bad_fog`).
    pub fog: Option<Fog>,
    /// Opaque surfaces reflect the view: a ray traced through the depth replaces the sky's
    /// specular light where it hits, blurred by roughness ([`Reflections`]). Perspective views
    /// with a sky only. `None` (default) for none: it costs a trace per pixel and two view-sized
    /// images, worth it for glossy surfaces.
    pub reflections: Option<Reflections>,
    /// Blur by distance from a focus, before a game's canvas ([`DepthOfField`]). `None`
    /// (default) for none.
    pub depth_of_field: Option<DepthOfField>,
    /// Blur along motion while the shutter's open, before a game's canvas ([`MotionBlur`]).
    /// `None` (default) for none.
    pub motion_blur: Option<MotionBlur>,
    /// Lens and film effects, before a game's canvas: vignette, chromatic aberration, film grain;
    /// none by default.
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
    /// What lit shading multiplies light by at its exposure, per [`gg_renderer::light_scale`]:
    /// 1 / (1.2 × 2^EV100). An adapting exposure starts from it.
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

/// The renderer's default adaptation; the fields' defaults.
fn adapted() -> gg_renderer::Adaptation {
    gg_renderer::Adaptation::default()
}

/// How an [`Environment`]'s exposure adapts to what the view sees, as eyes do between sunlight and
/// shade.
///
/// Each frame meters the view's light, before bloom and a game's canvas, from a luminance
/// histogram, as Unreal's histogram exposure does: the mean log2 luminance of pixels between two
/// percentiles, over 2^-12 (darker than starlight) to 2^20 candela per square meter in eighths of
/// a stop; darker pixels, like a black sky, are left out. The exposure moves toward
/// log2(luminance × 100 / 12.5), a reflected-light meter's at ISO 100, less the compensation,
/// kept between the bounds. It moves at its own speeds by the simulation's time, so a paused
/// game's holds.
///
/// Until a metering returns, two frames later, frames shade at `exposure`; that first metering is
/// taken at once. A change to the bounds, compensation or speeds shows in the frame it's made, so
/// a script can ease the compensation each tick. A change to the percentiles waits for a
/// metering by them, taken at once. The tool API's look adapts apart from the game's view, so
/// looking round a dark room exposes neither the view nor a capture of it for the room.
///
/// What isn't lit (sprites, text, unlit materials) shows its colors at any exposure but is
/// metered as though lit, brighter at darker exposures, so a view of little else runs to a bound.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Adaptation {
    /// The lowest exposure value, EV100, it adapts to; a scene calling for lower shows darker. 0,
    /// about a street at night, so a moonlit world shows as dark as it is.
    #[reflect(default = adapted().min)]
    pub min: f32,
    /// The highest; a scene calling for higher shows brighter. 18, past snow in sunlight.
    #[reflect(default = adapted().max)]
    pub max: f32,
    /// How many stops brighter than the meter calls for the view is exposed, as a camera's
    /// exposure compensation: 1 doubles the brightness, -1 halves it.
    pub compensation: f32,
    /// How fast the exposure rises toward a brighter view's, in stops a second: 3, as Unreal's.
    #[reflect(min = 0.0, default = adapted().speed_up)]
    pub speed_up: f32,
    /// How fast it falls toward a darker view's, in stops a second: 1, slower, as eyes adapt to
    /// the dark.
    #[reflect(min = 0.0, default = adapted().speed_down)]
    pub speed_down: f32,
    /// The luminance percentile, as a fraction, below which pixels aren't metered: 0.1, leaving
    /// out the darkest tenth.
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

/// Several entities' environments: the lowest ID, which the view renders with, and how many there
/// are.
pub(crate) type Tie = (EntityId, usize);

/// The environment the view renders `world` with: the lowest ID's [`Environment`], if any. Several
/// warn (`engine.environment_tie`) when they arise and change, not every frame: `warned` tracks
/// it.
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
        // The sun's 100,000 lux on a white surface, reflecting 1/π of it per steradian, comes to
        // about 0.8 at a sunny day's exposure.
        let white = 100_000.0 / std::f32::consts::PI * at(15.0).light_scale();
        assert!((0.8..0.82).contains(&white), "{white}");
    }

    /// Fog reflects whole: its description's default is the default fog, which switching an
    /// environment's fog on takes, and a foggy environment round-trips through its value, every
    /// fog field included.
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

    /// Reflections reflect whole: their description's default is the default reflections, which
    /// switching them on takes, and a reflecting environment round-trips through its value, every
    /// field included.
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

    /// An adaptation reflects whole: its description's default is the renderer's default, which
    /// switching it on takes, and an adapting environment round-trips through its value, handing
    /// the renderer each field unchanged.
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

    /// The camera's effects reflect whole: their descriptions' defaults are their defaults, which
    /// switching them on takes, and an environment with all of them round-trips through its value.
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

    /// An environment hands the renderer its image sky at its luminance, or its atmosphere; never
    /// both, and neither without a sky.
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

    /// Of several environments the view renders with the lowest ID, and a tie is warned of; with
    /// none, there's none.
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
