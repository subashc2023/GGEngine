//! Height fog, and the pass that applies it. The fog thins exponentially with height. It dims the
//! light from what the view shows by its optical depth to the eye, integrated in closed form, and
//! scatters the sky's light and the sun's toward the eye (`shaders/fog.slang`). The pass fogs
//! what's opaque, the sky included, from the view's depth, before what blends, which fogs itself as
//! it draws.
//!
//! Under an atmosphere, the fog is nearer the eye than the air's haze: what's lit is hazed as it
//! shades and fogged after, as is the sky from the atmosphere's table. The atmosphere's ambient
//! light and its sun, dimmed by the air, light the fog.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Result, label};
use gg_gpu::{ComputePass, ComputePipeline, Device, Frame, Graph, ImageId, PassId};

use crate::{Light, LightKind};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fogging.spv"));

/// Pixels across and down per workgroup: `GROUP` in `shaders/fogging.slang`.
const GROUP: u32 = 8;

/// The largest Henyey-Greenstein g of the sun's lobe. At 1 the lobe narrows to an infinitely bright
/// point.
const SHARPEST: f32 = 0.99;

/// Height fog, as [`RenderData::fog`](crate::RenderData::fog) gives it.
///
/// Its density is `density` at `height`, thinning exponentially above by `falloff` and thickening
/// below, from `start` along the way from the eye. It scatters `color` of the light reaching it:
/// the sky's from all around (`luminance` without a sky), and the sun's (the brightest directional
/// light) in a lobe as sharp as `sharpness`. It casts and takes no shadows, so `sun` scales the
/// sun's light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// The extinction at `height`: the share of light taken out per world unit. 0.02 by default,
    /// letting a third of the light through from 50 units away. 0 shows no fog.
    pub density: f32,
    /// How fast it thins with height, per unit: the density at `height + h` is `density ×
    /// e^(-falloff × h)`, halving every 0.69 / `falloff` units up. 0.2 by default; 0 is alike at
    /// every height.
    pub falloff: f32,
    /// Along +Y.
    pub height: f32,
    /// Its distance from the eye, in world units: what's nearer shows unfogged. 0 by default.
    pub start: f32,
    /// The most of the light from behind it that it takes out, 0 to 1. 1 (the default) hides what's
    /// far enough away, the sky included; less lets that much show through at any distance.
    pub opacity: f32,
    /// The share of the light reaching it that it scatters toward the eye rather than absorbs, in
    /// linear RGB. White by default, as water droplets scatter every color alike.
    pub color: Vec3,
    /// The luminance of the light from all around in a world without a sky, in cd/m², before
    /// exposure. 10,000 by default, about a bright day's sky.
    pub luminance: f32,
    /// How much of the sun's light reaches it. 1 (the default) is the open; less stands in for
    /// shadows the fog doesn't take, as of a courtyard's walls.
    pub sun: f32,
    /// How much more of the sun's light it scatters forward than other ways: Henyey-Greenstein's g.
    /// 0 is alike every way; toward 1, a glow round the sun narrows and brightens. Clamped to 0.99.
    /// 0.6 by default.
    pub sharpness: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            density: 0.02,
            falloff: 0.2,
            height: 0.0,
            start: 0.0,
            opacity: 1.0,
            color: Vec3::ONE,
            luminance: 10_000.0,
            sun: 1.0,
            sharpness: 0.6,
        }
    }
}

impl Fog {
    /// The name of its first field that's a NaN or an infinity, if any. Such fog would turn the
    /// view black, so it shows as none.
    pub fn non_finite(&self) -> Option<&'static str> {
        let numbers = [
            ("density", self.density),
            ("falloff", self.falloff),
            ("height", self.height),
            ("start", self.start),
            ("opacity", self.opacity),
            ("color.x", self.color.x),
            ("color.y", self.color.y),
            ("color.z", self.color.z),
            ("luminance", self.luminance),
            ("sun", self.sun),
            ("sharpness", self.sharpness),
        ];
        (numbers.into_iter()).find_map(|(name, number)| (!number.is_finite()).then_some(name))
    }

    /// Whether it fogs anything: not with a density of 0 or less, or a non-finite field.
    pub(crate) fn shows(&self) -> bool {
        self.density > 0.0 && self.non_finite().is_none()
    }
}

/// `Fog` in `shaders/mesh.slang`, field for field, which documents them. The default fogs nothing.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct FogData {
    ambient: Vec3,
    density: f32,
    sun: Vec3,
    falloff: f32,
    to_sun: Vec3,
    height: f32,
    start: f32,
    least: f32,
    lobe: f32,
    unused: u32,
}

impl FogData {
    /// `fog`'s data at `exposure`, or the default if it doesn't show. It's lit from all around by
    /// the sky's luminance `sky`, or its own without a sky, and by [`sun`].
    pub(crate) fn new(fog: Option<Fog>, sky: Option<f32>, lights: &[Light], exposure: f32) -> Self {
        let Some(fog) = fog.filter(Fog::shows) else {
            return Self::default();
        };
        let scattered = fog.color.max(Vec3::ZERO) * exposure;
        let around = sky.unwrap_or(fog.luminance).max(0.0);
        let sun = sun(lights);
        Self {
            ambient: scattered * around,
            density: fog.density,
            sun: sun.map_or(Vec3::ZERO, |(_, light)| {
                scattered * light * fog.sun.max(0.0)
            }),
            falloff: fog.falloff.max(0.0),
            to_sun: sun.map_or(Vec3::Y, |(to_sun, _)| to_sun),
            height: fog.height,
            start: fog.start.max(0.0),
            least: 1.0 - fog.opacity.clamp(0.0, 1.0),
            lobe: fog.sharpness.clamp(0.0, SHARPEST),
            unused: 0,
        }
    }
}

/// The way toward the sun, a unit vector, and its light in lux: the brightest directional light
/// that shines some way, the first among equals, as for shadows.
fn sun(lights: &[Light]) -> Option<(Vec3, Vec3)> {
    let luminance = |color: Vec3| color.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    (lights.iter())
        .filter(|light| light.kind == LightKind::Directional && luminance(light.color) > 0.0)
        .filter_map(|light| Some((-light.direction.try_normalize()?, light.color)))
        .reduce(|best, next| {
            if luminance(next.1) > luminance(best.1) {
                next
            } else {
                best
            }
        })
}

/// `FogPush` in `shaders/fogging.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct FogPush {
    world_from_clip: Mat4,
    frame: u64,
    size: [u32; 2],
    depth: u32,
    target: u32,
}

/// Fogs the view's colors from its depth.
pub(crate) struct FogPass {
    pipeline: ComputePipeline,
}

pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    depth: ImageId,
    colors: ImageId,
}

impl FogPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            pipeline: ComputePipeline::new(device, "fog", SPIRV, c"fog_main")?,
        })
    }

    /// Declares the pass that fogs `colors` from `depth` in `graph`. It also samples `lit`, the
    /// sky's light images, in the frame that prefilters them.
    pub(crate) fn plan(
        graph: &mut Graph,
        size: [u32; 2],
        depth: ImageId,
        colors: ImageId,
        lit: impl Iterator<Item = ImageId>,
    ) -> Plan {
        let pass = lit.fold(
            graph.compute(label!("fog")).sample(depth),
            ComputePass::sample,
        );
        Plan {
            pass: pass.write(colors).add(),
            size,
            depth,
            colors,
        }
    }

    /// Records `plan`'s pass, the frame's next. `clip_from_world` includes antialiasing's offset.
    /// `data` is the frame's data, which the fog and sky shaders read.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        clip_from_world: Mat4,
        data: u64,
    ) {
        let mut pass = frame.compute(plan.pass);
        let push = FogPush {
            world_from_clip: clip_from_world.inverse(),
            frame: data,
            size: plan.size,
            depth: pass.sampled(plan.depth),
            target: pass.storage(plan.colors, 0),
        };
        pass.set_pipeline(&self.pipeline);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directional(direction: Vec3, color: Vec3) -> Light {
        Light {
            kind: LightKind::Directional,
            position: Vec3::ZERO,
            direction,
            color,
            range: None,
            shadow_distance: None,
        }
    }

    /// Fog that doesn't show fogs nothing. Each light is times the exposure, and at least what the
    /// opacity leaves comes through.
    #[test]
    fn fog_takes_its_light_from_the_sky_or_its_own_and_the_brightest_sun() {
        let lights = [
            directional(Vec3::NEG_Y, Vec3::splat(2.0)),
            Light {
                kind: LightKind::Point,
                ..directional(Vec3::NEG_Y, Vec3::splat(100.0))
            },
            directional(Vec3::new(0.0, 0.0, -3.0), Vec3::new(1.0, 4.0, 1.0)),
            directional(Vec3::ZERO, Vec3::splat(100.0)),
            directional(Vec3::X, Vec3::new(1.0, 4.0, 1.0)),
        ];
        let fog = Fog {
            color: Vec3::new(1.0, 0.5, 0.25),
            luminance: 100.0,
            sun: 0.5,
            opacity: 0.75,
            ..Fog::default()
        };
        assert_eq!(FogData::new(None, None, &lights, 1.0), FogData::default());
        let thin = Fog {
            density: 0.0,
            ..fog
        };
        assert_eq!(
            FogData::new(Some(thin), None, &lights, 1.0),
            FogData::default()
        );
        let unlit = FogData::new(Some(fog), None, &lights, 0.5);
        assert_eq!(unlit.ambient, Vec3::new(50.0, 25.0, 12.5));
        // The first of the two equally bright. The point light and the one of no way are skipped.
        assert_eq!(unlit.to_sun, Vec3::Z);
        assert_eq!(unlit.sun, Vec3::new(0.25, 0.5, 0.0625));
        assert_eq!(unlit.least, 0.25);
        let lit = FogData::new(Some(fog), Some(8.0), &[], 0.5);
        assert_eq!(lit.ambient, Vec3::new(4.0, 2.0, 1.0));
        assert_eq!(lit.sun, Vec3::ZERO);
        let sharp = Fog {
            sharpness: 2.0,
            ..fog
        };
        assert_eq!(FogData::new(Some(sharp), None, &[], 1.0).lobe, SHARPEST);
    }

    /// Its light would turn the view black.
    #[test]
    fn fog_with_a_number_that_isnt_finite_names_it_and_fogs_nothing() {
        let finite = Fog::default();
        assert_eq!(finite.non_finite(), None);
        let broken = [
            (
                Fog {
                    density: f32::INFINITY,
                    ..finite
                },
                "density",
            ),
            (
                Fog {
                    falloff: f32::NAN,
                    ..finite
                },
                "falloff",
            ),
            (
                Fog {
                    height: f32::NEG_INFINITY,
                    ..finite
                },
                "height",
            ),
            (
                Fog {
                    start: f32::NAN,
                    ..finite
                },
                "start",
            ),
            (
                Fog {
                    opacity: f32::NAN,
                    ..finite
                },
                "opacity",
            ),
            (
                Fog {
                    color: Vec3::new(1.0, f32::NAN, 1.0),
                    ..finite
                },
                "color.y",
            ),
            (
                Fog {
                    luminance: f32::INFINITY,
                    ..finite
                },
                "luminance",
            ),
            (
                Fog {
                    sun: f32::NAN,
                    ..finite
                },
                "sun",
            ),
            (
                Fog {
                    sharpness: f32::NAN,
                    ..finite
                },
                "sharpness",
            ),
        ];
        for (fog, name) in broken {
            assert_eq!(fog.non_finite(), Some(name), "{fog:?}");
            assert!(!fog.shows(), "{fog:?} shows");
            assert_eq!(
                FogData::new(Some(fog), None, &[], 1.0),
                FogData::default(),
                "{fog:?}"
            );
        }
    }
}
