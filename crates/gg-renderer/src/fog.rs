//! Height fog: a medium filling the world, densest low down and thinning exponentially with height,
//! which dims the light from what the view shows by its optical depth along the way to the eye,
//! integrated in closed form, and scatters the sky's light from all around and the sun's toward
//! it, as `shaders/fog.slang` says; and the pass that fogs what's opaque in the view, the sky among
//! it, from the view's depth, before what blends, which fogs itself as it draws.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec3};
use gg_core::{Result, label};
use gg_gpu::{ComputePass, ComputePipeline, Device, Frame, Graph, ImageId, PassId};

use crate::{Light, LightKind};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fogging.spv"));

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/fogging.slang`.
const GROUP: u32 = 8;

/// The most of Henyey-Greenstein's g the sun's lobe takes, short of 1, where its lobe would narrow
/// to a point of infinite brightness.
const SHARPEST: f32 = 0.99;

/// Height fog, as [`RenderData::fog`](crate::RenderData::fog) gives it: its density, an
/// extinction, the share of light it takes out over a unit of the world, at `height`, thinning
/// exponentially above it by `falloff`, and thickening as far below it, from `start` along the way
/// from the eye. Of the light it takes out, it scatters `color` toward the eye, of the light that
/// reaches it: from all around, the sky's, or without a sky `luminance`, and the sun's, the
/// brightest directional light's, round the way toward it, in a lobe as sharp as `sharpness`. It
/// casts and takes no shadows, so the sun lights all of it, `sun` times as much as it would out in
/// the open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// The share of the light it takes out over a unit of the world at `height`, the fog's
    /// extinction there: 0.02 by default, which lets through a third of the light from 50 units
    /// away at that height, and none at all for 0, which leaves the view as it is without fog.
    pub density: f32,
    /// How fast it thins with height, per unit: its density at `height + h` is `density × e^(-falloff
    /// × h)`, halving every 0.69 / `falloff` units up, and doubling as far down. 0.2 by default; 0
    /// fills the world alike at every height.
    pub falloff: f32,
    /// The height, along +Y, at which its density is `density`.
    pub height: f32,
    /// How far from the eye it starts, in units of the world: what's nearer shows unfogged. 0 by
    /// default.
    pub start: f32,
    /// The most of the light from what's behind it it takes out, from 0 to 1: 1, the default,
    /// hides what's far enough away, the sky among it, behind the fog; less lets that much of it
    /// show through, however far.
    pub opacity: f32,
    /// The share of the light reaching it that it scatters toward the eye, rather than absorbing,
    /// in linear RGB: white by default, as water's droplets scatter every color alike.
    pub color: Vec3,
    /// The luminance, in candela per square meter, of the light that reaches it from all around in
    /// a world without a sky, in its place, before the exposure, as light is: 10,000 by default,
    /// about a bright day's sky, which shows its color at a quarter on a sunny day's exposure.
    pub luminance: f32,
    /// How much of the sun's light reaches it: 1, the default, as out in the open; less where
    /// shadows the fog doesn't take, as a courtyard's walls, would keep much of it out.
    pub sun: f32,
    /// How much more of the sun's light it scatters on toward the way the light goes than any other
    /// way: Henyey-Greenstein's g, from 0, alike every way, toward 1, a glow round the sun that
    /// narrows and brightens as it nears 1, kept within 0.99. 0.6 by default.
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
    /// Whether it fogs anything, as fog of a density of 0 or less, or of none, doesn't.
    pub(crate) fn shows(&self) -> bool {
        self.density > 0.0
    }
}

/// What shaders read of the fog, through the frame's data: `Fog` in `shaders/mesh.slang`, field for
/// field, which says what each is. Its default has none, which fogs nothing.
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
    /// The data of `fog`, if it shows, as a frame of `exposure` lit by `lights` takes it: lit from
    /// all around by its sky's light, of luminance `sky`, in a frame with a sky, or by its own
    /// luminance otherwise; and from the brightest directional light that shines, the first of
    /// those as bright, as the sun. None for fog that doesn't show.
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

/// The way toward the sun, a unit vector, and its light, in lux: of the brightest of `lights` that's
/// directional and shines some way, the first of those as bright, as the shadows take theirs.
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

/// What the fog's pass pushes: `FogPush` in `shaders/fogging.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct FogPush {
    world_from_clip: Mat4,
    frame: u64,
    size: [u32; 2],
    depth: u32,
    target: u32,
}

/// What fogs the view's colors, from its depth.
pub(crate) struct FogPass {
    pipeline: ComputePipeline,
}

/// The fog's pass in a frame's graph: the pass, and the view's size, its depth, which it samples,
/// and its colors, which it fogs.
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

    /// Declares in `graph` the pass that fogs the view's `colors`, of `size` pixels, from its
    /// `depth`, sampling `lit` too, the images the sky's light is in, in the frame that prefilters
    /// them.
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

    /// Records `plan`'s pass, the next of `frame`'s, of the view drawn by `clip_from_world`, from
    /// the world to its clip space, offset as antialiasing offsets it, whose fog and sky shaders
    /// read from the frame's data at the address `data`.
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

    /// Fog that doesn't show, as of no density, or none at all, fogs nothing; fog that does scatters
    /// its color of the sky's light, in a frame with a sky, or else of its own luminance, and of the
    /// brightest directional light's, from the way toward it, each times the exposure, and lets
    /// through at least what its opacity leaves.
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
        // The first of the two as bright, of a way, the point light and the one of no way aside.
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
}
