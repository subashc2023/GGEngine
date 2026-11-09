//! The atmosphere: a sky made from its sun, as Sébastien Hillaire's "A Scalable and Production
//! Ready Sky and Atmosphere Rendering Technique" (EGSR 2020) makes it, after his reference
//! implementation (github.com/sebh/UnrealEngineSkyAtmosphere, MIT licensed, Copyright (c) 2020 Epic
//! Games, Inc.), as `shaders/atmosphere.slang` says: its tables made on the GPU in each frame that
//! shows it, the sky drawn from them behind the world with the sun's disk, the haze it puts between
//! the viewer and what's lit, and the cube the sky's light is prefiltered from, as an image sky's
//! is, again only as the sun turns or the viewer rises past what the light holds.
//!
//! Its sun is the frame's brightest directional light that shines, its illuminance taken outside
//! the atmosphere: the light that reaches the world is dimmed by the transmittance toward the sun
//! from the viewer, once a frame, on the CPU, by the sums the GPU's table makes.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec2, Vec3, libm};
use gg_core::{Result, label};
use gg_gpu::{
    Blend, ComputePipeline, Device, Format, Frame, Graph, GraphicsDesc, ImageId, Pass, PassId,
    Pipeline,
};

use crate::{Light, LightKind, Viewpoint, sky, taa};

static COMPUTE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atmosphere.spv"));
static SKY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atmosphere_sky.spv"));

/// Hillaire's atmosphere, as [`RenderData::atmosphere`](crate::RenderData::atmosphere) gives it:
/// Rayleigh's and Mie's scattering and ozone's absorption over a planet of Earth's radius, 6,360
/// km, up to the atmosphere's top, 100 km above it, as the paper's. The world's units are meters,
/// and its +Y up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atmosphere {
    /// Rayleigh's scattering at the ground, of each channel, in millionths per meter, thinning
    /// exponentially with height, by a scale height of 8 km: what makes the sky blue and sunsets
    /// red.
    pub rayleigh: Vec3,
    /// Mie's scattering at the ground, of every channel alike, in millionths per meter, thinning
    /// by a scale height of 1.2 km: the haze's, which whitens the sky round the sun and toward the
    /// horizon.
    pub mie: f32,
    /// What Mie's haze absorbs, likewise.
    pub mie_absorption: f32,
    /// How much of its light Mie's haze scatters forward, onward from the way it came, by
    /// Henyey-Greenstein's phase function: from -1 all back to 1 all onward, 0 alike every way.
    pub mie_anisotropy: f32,
    /// Ozone's absorption, of each channel, in millionths per meter, where it's densest, 25 km up,
    /// thinning to none 15 km above and below.
    pub ozone: Vec3,
    /// The planet's ground's albedo, which lights the atmosphere from below and shows below the
    /// horizon.
    pub ground_albedo: Vec3,
    /// How far above the planet's ground the world's origin is, in meters: the viewer is as high
    /// as that and its own Y, kept between 10 m and 10 m below the atmosphere's top.
    pub altitude: f32,
    /// The angle the sun's disk spans across, in radians.
    pub sun_size: f32,
}

impl Atmosphere {
    /// Earth's, as the paper's: Rayleigh's scattering of (5.802, 13.558, 33.1), Mie's of 3.996,
    /// absorbing 0.444, anisotropy 0.8, ozone's absorption of (0.650, 1.881, 0.085), all millionths
    /// per meter; ground of albedo 0.3, the world's origin on it, and a sun 0.5357° across.
    pub const EARTH: Self = Self {
        rayleigh: Vec3::new(5.802, 13.558, 33.1),
        mie: 3.996,
        mie_absorption: 0.444,
        mie_anisotropy: 0.8,
        ozone: Vec3::new(0.650, 1.881, 0.085),
        ground_albedo: Vec3::splat(0.3),
        altitude: 0.0,
        sun_size: 0.009_35,
    };
}

impl Default for Atmosphere {
    /// [`Atmosphere::EARTH`].
    fn default() -> Self {
        Self::EARTH
    }
}

/// The planet's radius and the atmosphere's top's, in kilometers, the scale heights its Rayleigh
/// and Mie scattering thin by, and where ozone is densest and how far above and below it thins to
/// none: `atmosphere.slang`'s.
const GROUND: f32 = 6360.0;
const TOP: f32 = 6460.0;
const RAYLEIGH_HEIGHT: f32 = 8.0;
const MIE_HEIGHT: f32 = 1.2;
const OZONE_HEIGHT: f32 = 25.0;
const OZONE_WIDTH: f32 = 15.0;

/// How far above the ground and below the atmosphere's top the viewer is kept, in kilometers:
/// below that, the sky's table's map of the horizon breaks down, and above, the sky is a planet
/// seen from space, which it doesn't draw.
const OFFSET: f32 = 0.01;

/// What the settings' millionths per meter are per kilometer, as the shaders take them.
const PER_KM: f32 = 1.0e-3;

/// How many steps the transmittance takes, as `TRANSMITTANCE_STEPS` in `atmosphere.slang`.
const TRANSMITTANCE_STEPS: u32 = 40;

/// The tables' sizes, as `atmosphere.slang` has them: the transmittance's, the light scattered
/// many times over's, the sky's, and the aerial perspective's, its `SLICES` slices of `SLICES` by
/// `SLICES` froxels side by side.
const TRANSMITTANCE: [u32; 2] = [256, 64];
const MULTIPLE: [u32; 2] = [32, 32];
const SKY_VIEW: [u32; 2] = [192, 108];
const SLICES: u32 = 32;
const AERIAL: [u32; 2] = [SLICES * SLICES, SLICES];

/// The side of the cube the sky's light is prefiltered from: the sky is smooth but at the
/// horizon, and its prefiltered light more so.
pub(crate) const CUBE: u32 = 128;

/// How far the sun may turn, in radians, before the sky's light is prefiltered again, by how high
/// it is: [`LEAST_TURN`] at the horizon and below, where the sky's light falls by half to three
/// quarters each degree, growing by a factor of e every [`TURN_GROWTH`] up, as the light changes
/// more slowly, to [`MOST_TURN`], so that the light changes by no more than about 2% a step
/// anywhere; and [`MOST_TURN`] again past [`NIGHT`], once the sky is dark. And the most the viewer
/// rises or falls, in meters.
const LEAST_TURN: f32 = 0.01 * std::f32::consts::PI / 180.0;
const TURN_GROWTH: f32 = 5.0 * std::f32::consts::PI / 180.0;
const MOST_TURN: f32 = std::f32::consts::PI / 180.0;
const NIGHT: f32 = -18.0 * std::f32::consts::PI / 180.0;
const RISE: f32 = 100.0;

/// How many samples each texel of the sky's prefiltered light takes from the atmosphere's cube,
/// fewer than an image sky's, as the sky is smooth and its light is prefiltered again often.
pub(crate) const SAMPLES: u32 = 64;

/// The threads across and down of the tables' workgroups, as `atmosphere.slang` has them.
const GROUP: u32 = 8;

/// Rec. 709's luminance of linear RGB, which picks the brightest light.
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// The atmosphere as a frame lights it and sees it: its sun, the frame's brightest directional
/// light that shines, the first of those as bright, and how high the viewer is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    atmosphere: Atmosphere,
    /// The light that stands for the sun, by its index among the frame's, if one shines.
    sun: Option<usize>,
    /// The unit way toward the sun: straight up without one.
    to_sun: Vec3,
    /// The sun's illuminance outside the atmosphere, its brightest channel's, in lux: 0 without
    /// one.
    illuminance: f32,
    /// The sun's color, as a share of its illuminance.
    tint: Vec3,
    /// How high the viewer is above the ground, in meters, kept within the atmosphere.
    height: f32,
    /// What of the sun's light reaches the viewer, by each channel, as [`sunlight`] says.
    sunlight: Vec3,
}

/// Whether `light` shines from a sun: a directional light of some light, shining some way.
fn shines(light: &Light) -> bool {
    light.kind == LightKind::Directional
        && light.color.cmpgt(Vec3::ZERO).any()
        && light.direction.try_normalize().is_some()
}

impl Seen {
    /// `atmosphere` under the sun of `lights`, a frame's, seen from `viewpoint`.
    pub(crate) fn new(atmosphere: Atmosphere, lights: &[Light], viewpoint: &Viewpoint) -> Self {
        let luminance = |light: &Light| light.color.dot(LUMINANCE);
        let sun = (lights.iter().enumerate())
            .filter(|(_, light)| shines(light))
            .reduce(|best, next| {
                if luminance(next.1) > luminance(best.1) {
                    next
                } else {
                    best
                }
            })
            .map(|(i, _)| i);
        let (to_sun, illuminance, tint) = match sun {
            Some(i) => {
                let light = &lights[i];
                let brightest = light.color.max_element();
                let to_sun = -light.direction.normalize();
                (to_sun, brightest, (light.color / brightest).max(Vec3::ZERO))
            }
            None => (Vec3::Y, 0.0, Vec3::ZERO),
        };
        let height = (atmosphere.altitude + viewpoint.position.y)
            .clamp(OFFSET * 1000.0, (TOP - GROUND - OFFSET) * 1000.0);
        let radius = GROUND + height / 1000.0;
        Self {
            atmosphere,
            sun,
            to_sun,
            illuminance,
            tint,
            height,
            sunlight: sunlight(&atmosphere, radius, to_sun.y),
        }
    }

    /// The sun's illuminance outside the atmosphere, its brightest channel's, in lux, which the
    /// tables' light, of an illuminance of 1, is multiplied by: 0 without a sun.
    pub(crate) fn illuminance(&self) -> f32 {
        self.illuminance
    }

    /// Whether it dims a light of the frame's, the one that stands for its sun.
    pub(crate) fn dims(&self) -> bool {
        self.sun.is_some()
    }

    /// Whether its sun has set: a light stands for it, and none of the light reaches the viewer.
    pub(crate) fn set(&self) -> bool {
        self.sun.is_some() && self.sunlight == Vec3::ZERO
    }

    /// Dims the light that stands for the sun among `lights`, the frame's it was seen under, by
    /// what of the sun's light reaches the viewer, as [`sunlight`] says.
    pub(crate) fn dim(&self, lights: &mut [Light]) {
        if let Some(sun) = self.sun {
            lights[sun].color *= self.sunlight;
        }
    }

    /// How far the viewer is from the planet's center, in kilometers.
    fn radius(&self) -> f32 {
        GROUND + self.height / 1000.0
    }

    /// The cosine of the angle from the sun's center to its disk's edge.
    fn sun_cosine(&self) -> f32 {
        libm::cosf(self.atmosphere.sun_size.max(1.0e-5) / 2.0)
    }

    /// The luminance of the sun's disk for an illuminance of 1: 1 over the solid angle it spans.
    pub(crate) fn disk(&self) -> f32 {
        1.0 / (2.0 * std::f32::consts::PI * (1.0 - self.sun_cosine())).max(1.0e-12)
    }

    /// What the sky's light is prefiltered from, as [`Lit::near`] compares it.
    pub(crate) fn lit(&self) -> Lit {
        Lit {
            atmosphere: self.atmosphere,
            to_sun: self.to_sun,
            tint: self.tint,
            height: self.height,
        }
    }

    /// The atmosphere's data, as its shaders read it, its froxels seeing along the ways
    /// `ways_from_clip` gives, as [`Viewpoint::ways_from_clip`] gives them unjittered.
    pub(crate) fn data(&self, ways_from_clip: Mat4) -> AtmosphereData {
        let atmosphere = &self.atmosphere;
        let mie = atmosphere.mie.max(0.0) * PER_KM;
        AtmosphereData {
            ways_from_clip,
            rayleigh: atmosphere.rayleigh.max(Vec3::ZERO) * PER_KM,
            mie_scattering: mie,
            ozone: atmosphere.ozone.max(Vec3::ZERO) * PER_KM,
            mie_extinction: mie + atmosphere.mie_absorption.max(0.0) * PER_KM,
            ground_albedo: atmosphere.ground_albedo.clamp(Vec3::ZERO, Vec3::ONE),
            mie_anisotropy: atmosphere.mie_anisotropy.clamp(-0.999, 0.999),
            sun: self.to_sun,
            height: self.radius(),
            tint: self.tint,
            sun_cosine: self.sun_cosine(),
        }
    }
}

/// What the sky's light was prefiltered from: the atmosphere, its sun's way and color, and how
/// high the viewer was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lit {
    atmosphere: Atmosphere,
    to_sun: Vec3,
    tint: Vec3,
    height: f32,
}

impl Lit {
    /// Whether light prefiltered from `held` stands for this: of the same atmosphere and sun's
    /// color, the sun turned no further than [`turn`] lets it at the lower of its two heights,
    /// and the viewer risen or fallen no more than 100 m since. The sun's illuminance isn't among
    /// it, as the light is of an illuminance of 1.
    pub(crate) fn near(&self, held: &Lit) -> bool {
        let lower = libm::asinf(self.to_sun.y.min(held.to_sun.y).clamp(-1.0, 1.0));
        // By the chord between the two ways, which keeps a hundredth of a degree in an f32, as a
        // cosine wouldn't.
        let chord = 2.0 * libm::sinf(turn(lower) / 2.0);
        self.atmosphere == held.atmosphere
            && self.tint == held.tint
            && (self.to_sun - held.to_sun).length() <= chord
            && (self.height - held.height).abs() <= RISE
    }
}

/// How far the sun may turn, in radians, before the sky's light is prefiltered again, at
/// `elevation` radians above the horizon, as [`LEAST_TURN`] says.
fn turn(elevation: f32) -> f32 {
    if elevation < NIGHT {
        MOST_TURN
    } else {
        (LEAST_TURN * libm::expf(elevation / TURN_GROWTH)).clamp(LEAST_TURN, MOST_TURN)
    }
}

/// How far along `way`, a unit vector, from `from` a ray first meets the sphere of `radius` round
/// the planet's center, 0 from inside it; -1 if it doesn't, as `nearest` in `atmosphere.slang`.
fn nearest(from: Vec3, way: Vec3, radius: f32) -> f32 {
    let b = from.dot(way);
    let c = from.dot(from) - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return -1.0;
    }
    let root = discriminant.sqrt();
    let (near, far) = (-b - root, -b + root);
    if near < 0.0 && far < 0.0 {
        -1.0
    } else if near < 0.0 {
        far.max(0.0)
    } else if far < 0.0 {
        near.max(0.0)
    } else {
        near.min(far).max(0.0)
    }
}

/// What takes light away, per kilometer, at a point `radius` kilometers from the planet's center.
fn extinction(atmosphere: &Atmosphere, radius: f32) -> Vec3 {
    let height = radius - GROUND;
    let rayleigh = atmosphere.rayleigh.max(Vec3::ZERO) * libm::expf(-height / RAYLEIGH_HEIGHT);
    let haze = atmosphere.mie.max(0.0) + atmosphere.mie_absorption.max(0.0);
    let mie = haze * libm::expf(-height / MIE_HEIGHT);
    let ozone = (1.0 - (height - OZONE_HEIGHT).abs() / OZONE_WIDTH).clamp(0.0, 1.0);
    (rayleigh + Vec3::splat(mie) + atmosphere.ozone.max(Vec3::ZERO) * ozone) * PER_KM
}

/// The share of light that comes from outside the atmosphere to a point `radius` kilometers from
/// the planet's center along a way of zenith cosine `mu`, or from the ground, if that's in the way:
/// `optical_depth` in `atmosphere.slang`, which the GPU's table of it sums alike.
pub(crate) fn transmittance(atmosphere: &Atmosphere, radius: f32, mu: f32) -> Vec3 {
    let from = Vec3::new(0.0, radius, 0.0);
    let way = Vec3::new((1.0 - mu * mu).max(0.0).sqrt(), mu, 0.0);
    // To the ground, if the way goes into it, and otherwise out of the atmosphere.
    let bottom = nearest(from, way, GROUND);
    let top = nearest(from, way, TOP).max(0.0);
    let end = if bottom > 0.0 { top.min(bottom) } else { top };
    let step = end / TRANSMITTANCE_STEPS as f32;
    let depth: Vec3 = (0..TRANSMITTANCE_STEPS)
        .map(|i| {
            extinction(
                atmosphere,
                (from + way * ((i as f32 + 0.5) * step)).length(),
            )
        })
        .sum();
    let depth = depth * step;
    Vec3::new(
        libm::expf(-depth.x),
        libm::expf(-depth.y),
        libm::expf(-depth.z),
    )
}

/// What of a sun's light reaches a point `radius` kilometers from the planet's center, its center
/// at zenith cosine `mu`: the transmittance toward it, times the share of its disk above the
/// horizon, from all of it with its disk's lowest point at the horizon, to none with its highest,
/// so that the light fades as the sun sets rather than going out at once; none once it's set.
fn sunlight(atmosphere: &Atmosphere, radius: f32, mu: f32) -> Vec3 {
    let share = GROUND / radius;
    let horizon = -(1.0 - share * share).max(0.0).sqrt();
    // By the angles above the horizon of the disk's center and of the horizon.
    let half = (atmosphere.sun_size / 2.0).max(1.0e-6);
    let above = libm::asinf(mu.clamp(-1.0, 1.0)) - libm::asinf(horizon);
    let shown = (above / (2.0 * half) + 0.5).clamp(0.0, 1.0);
    if shown == 0.0 {
        return Vec3::ZERO;
    }
    // Just over the horizon, where a disk partly set still shines.
    transmittance(atmosphere, radius, mu.max(horizon + 1.0e-4)) * shown
}

/// The atmosphere as its shaders read it: `Atmosphere` in `shaders/atmosphere.slang`, field for
/// field, in kilometers.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
pub(crate) struct AtmosphereData {
    /// From clip space to the ways the view sees, unjittered: what the froxels see along.
    ways_from_clip: Mat4,
    rayleigh: Vec3,
    mie_scattering: f32,
    ozone: Vec3,
    mie_extinction: f32,
    ground_albedo: Vec3,
    mie_anisotropy: f32,
    /// The unit way toward the sun, and how far the viewer is from the planet's center.
    sun: Vec3,
    height: f32,
    /// The sun's color, as a share of its illuminance, and the cosine of its disk's radius.
    tint: Vec3,
    sun_cosine: f32,
}

/// The aerial perspective's part of the frame's data: `Aerial` in `shaders/mesh.slang`, field for
/// field, its `luminance` 0 in a frame without an atmosphere.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct AerialData {
    /// From the world to clip space, unjittered, where a point falls among the froxels.
    clip_from_world: Mat4,
    /// The froxels' slot in the bindless set.
    texture: u32,
    /// What their light is multiplied by, before the exposure: the sun's illuminance.
    luminance: f32,
}

impl AerialData {
    /// The froxels in the slot `texture`, as a view of `clip_from_world`, unjittered, finds
    /// points among them, of a sun of `illuminance`.
    pub(crate) fn new(clip_from_world: Mat4, texture: u32, illuminance: f32) -> Self {
        Self {
            clip_from_world,
            texture,
            luminance: illuminance,
        }
    }
}

/// What the atmosphere's passes push: `Push` in `shaders/atmosphere.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Push {
    world_from_clip: Mat4,
    atmosphere: u64,
    transmittance: u32,
    multiple: u32,
    sky_view: u32,
    target: u32,
    size: u32,
    scale: f32,
    disk: f32,
    unused: u32,
}

impl Push {
    /// What a table's pass pushes, of the atmosphere's data at `atmosphere`, writing the storage
    /// slot `target`, before it says what else it reads.
    fn of(atmosphere: u64, target: u32) -> Self {
        Self {
            world_from_clip: Mat4::ZERO,
            atmosphere,
            transmittance: 0,
            multiple: 0,
            sky_view: 0,
            target,
            size: 0,
            scale: 0.0,
            disk: 0.0,
            unused: 0,
        }
    }
}

/// What the sky's draw takes: the atmosphere's data's address, the slots of the transmittance's
/// table and the sky's in its pass, what its light is multiplied by, the sun's illuminance times
/// the exposure, and its disk's luminance for an illuminance of 1.
pub(crate) struct Drawn {
    pub(crate) atmosphere: u64,
    pub(crate) transmittance: u32,
    pub(crate) sky_view: u32,
    pub(crate) scale: f32,
    pub(crate) disk: f32,
}

/// The atmosphere's tables and their passes in a frame's graph, the tables the graph's own.
pub(crate) struct Plan {
    /// The transmittance's, the light scattered many times over's, the sky's and the aerial
    /// perspective's.
    images: [ImageId; 4],
    passes: [PassId; 4],
}

impl Plan {
    /// Those the view's passes sample: the transmittance's table, the sky's and the aerial
    /// perspective's.
    pub(crate) fn sampled(&self) -> [ImageId; 3] {
        let [transmittance, _, sky_view, aerial] = self.images;
        [transmittance, sky_view, aerial]
    }

    pub(crate) fn transmittance(&self) -> ImageId {
        self.images[0]
    }

    pub(crate) fn sky_view(&self) -> ImageId {
        self.images[2]
    }

    pub(crate) fn aerial(&self) -> ImageId {
        self.images[3]
    }
}

/// The atmosphere's pipelines: those that make its tables and its sky's cube, and the one that
/// draws its sky behind the world.
pub(crate) struct AtmospherePass {
    transmittance: ComputePipeline,
    multiple: ComputePipeline,
    sky_view: ComputePipeline,
    aerial: ComputePipeline,
    cube: ComputePipeline,
    draw: Pipeline,
}

impl AtmospherePass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let compute = |name, entry| ComputePipeline::new(device, name, COMPUTE, entry);
        Ok(Self {
            transmittance: compute("atmosphere transmittance", c"transmittance_main")?,
            multiple: compute("atmosphere multiple scattering", c"multiple_main")?,
            sky_view: compute("atmosphere sky view", c"sky_view_main")?,
            aerial: compute("atmosphere aerial perspective", c"aerial_main")?,
            cube: compute("atmosphere cube", c"cube_main")?,
            draw: Pipeline::graphics(
                device,
                &GraphicsDesc {
                    name: "atmosphere sky",
                    spirv: SKY,
                    vertex: c"vertex_main",
                    mesh: None,
                    fragment: Some(c"fragment_main"),
                    format: Format::HDR.with_ids().with_motion().with_depth(),
                    // Drawn first, at depth 0, which the cleared depth passes, writing none, nor
                    // any motion, as an image sky is.
                    blend: Blend::Opaque,
                    depth_write: false,
                    depth_bias: 0.0,
                    depth_clamp: false,
                },
            )?,
        })
    }

    /// Adds to `graph` the passes that make the atmosphere's tables, each the graph's own, each in
    /// a pass that computes, on the graphics queue, as they're images: the transmittance first,
    /// which the rest read, then the light scattered many times over, then the sky's and the
    /// aerial perspective's.
    pub(crate) fn plan(graph: &mut Graph) -> Plan {
        let transmittance = graph.hdr("atmosphere transmittance", TRANSMITTANCE);
        let multiple = graph.hdr("atmosphere multiple scattering", MULTIPLE);
        let sky_view = graph.hdr("atmosphere sky view", SKY_VIEW);
        let aerial = graph.hdr("atmosphere aerial perspective", AERIAL);
        let made = (graph.compute(label!("atmosphere transmittance")))
            .overwrite(transmittance)
            .add();
        let many = (graph.compute(label!("atmosphere multiple scattering")))
            .sample(transmittance)
            .overwrite(multiple)
            .add();
        let seen = (graph.compute(label!("atmosphere sky view")))
            .sample(transmittance)
            .sample(multiple)
            .overwrite(sky_view)
            .add();
        let hazed = (graph.compute(label!("atmosphere aerial perspective")))
            .sample(transmittance)
            .sample(multiple)
            .overwrite(aerial)
            .add();
        Plan {
            images: [transmittance, multiple, sky_view, aerial],
            passes: [made, many, seen, hazed],
        }
    }

    /// Records `plan`'s passes, the next of `frame`'s graph to record, which make the tables of
    /// the atmosphere `data` gives; returns the address its data is uploaded at, for the frame's
    /// other uses of it.
    pub(crate) fn record(
        &self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        data: &AtmosphereData,
    ) -> Result<u64> {
        let groups =
            |[width, height]: [u32; 2], deep| [width.div_ceil(GROUP), height.div_ceil(GROUP), deep];
        let [transmittance, multiple, sky_view, aerial] = plan.images;
        let [made, many, seen, hazed] = plan.passes;
        let mut pass = frame.compute(made);
        let address = pass.upload(bytemuck::bytes_of(data))?.address();
        pass.set_pipeline(&self.transmittance);
        let push = Push::of(address, pass.storage(transmittance, 0));
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(TRANSMITTANCE, 1));
        drop(pass);
        let mut pass = frame.compute(many);
        pass.set_pipeline(&self.multiple);
        let push = Push {
            transmittance: pass.sampled(transmittance),
            ..Push::of(address, pass.storage(multiple, 0))
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        // A workgroup a texel, of a thread a way round it.
        let [width, height] = MULTIPLE;
        pass.dispatch([width, height, 1]);
        drop(pass);
        for (pass, image, pipeline, size, slices) in [
            (seen, sky_view, &self.sky_view, SKY_VIEW, 1),
            (hazed, aerial, &self.aerial, [SLICES; 2], SLICES),
        ] {
            let mut pass = frame.compute(pass);
            pass.set_pipeline(pipeline);
            let push = Push {
                transmittance: pass.sampled(transmittance),
                multiple: pass.sampled(multiple),
                ..Push::of(address, pass.storage(image, 0))
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(size, slices));
        }
        Ok(address)
    }

    /// Records the pass that makes the first level of the cube `prefiltering`, a plan of the sky's
    /// light, prefilters, the next of `frame`'s graph to record, from the sky's table that `plan`
    /// made, without the sun's disk, of the atmosphere's data at `atmosphere`.
    pub(crate) fn cube(
        &self,
        frame: &mut Frame<'_>,
        prefiltering: &sky::Plan,
        plan: &Plan,
        atmosphere: u64,
    ) {
        let (to_cube, cube, face) = prefiltering.to_cube();
        let mut pass = frame.compute(to_cube);
        pass.set_pipeline(&self.cube);
        let push = Push {
            sky_view: pass.sampled(plan.sky_view()),
            size: face,
            ..Push::of(atmosphere, pass.storage(cube, 0))
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([face.div_ceil(GROUP), face.div_ceil(GROUP), 6]);
    }

    /// Draws the atmosphere's sky behind everything the pass draws after it, over the whole of its
    /// target, as `viewpoint` sees it in a view `aspect` times as wide as it's tall, drawn `jitter`
    /// off, in its normalized coordinates, as antialiasing draws it, as `drawn` says.
    ///
    /// # Panics
    ///
    /// If the pass isn't into an image of [`Format::HDR`] with IDs, motion and a depth image.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewpoint: &Viewpoint,
        aspect: f32,
        jitter: Vec2,
        drawn: &Drawn,
    ) {
        pass.begin_label(c"atmosphere sky");
        pass.set_pipeline(&self.draw);
        let push = Push {
            // Back from where the offset drew each pixel's way.
            world_from_clip: viewpoint.ways_from_clip(aspect) * taa::jitter(-jitter),
            transmittance: drawn.transmittance,
            sky_view: drawn.sky_view,
            scale: drawn.scale,
            disk: drawn.disk,
            ..Push::of(drawn.atmosphere, 0)
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
        pass.end_label();
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Quat, to_radians};

    use super::*;
    use crate::Projection;

    fn viewpoint(height: f32) -> Viewpoint {
        Viewpoint {
            position: Vec3::new(3.0, height, -2.0),
            rotation: Quat::IDENTITY,
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
        }
    }

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

    /// Straight up from the ground, the transmittance is what the paper's model gives in closed
    /// form: each scattering's density integrated is its scale height, less what lies past the
    /// atmosphere's top, and ozone's tent 15 km across; and toward the horizon far less light comes
    /// through, the blue least, as a low sun reddens.
    #[test]
    fn the_transmittance_is_the_papers_model() {
        let earth = Atmosphere::EARTH;
        let up = transmittance(&earth, GROUND, 1.0);
        let thickness = TOP - GROUND;
        let integral = |height: f32| height * (1.0 - libm::expf(-thickness / height));
        let depth = (earth.rayleigh * integral(RAYLEIGH_HEIGHT)
            + Vec3::splat((earth.mie + earth.mie_absorption) * integral(MIE_HEIGHT))
            + earth.ozone * OZONE_WIDTH)
            * PER_KM;
        let expected = Vec3::new(
            libm::expf(-depth.x),
            libm::expf(-depth.y),
            libm::expf(-depth.z),
        );
        assert!(
            up.abs_diff_eq(expected, 2.0e-3),
            "{up} straight up, not {expected}"
        );
        // About (0.94, 0.87, 0.76).
        assert!(
            (0.93..0.95).contains(&up.x) && (0.75..0.77).contains(&up.z),
            "{up}"
        );
        let low = transmittance(&earth, GROUND, libm::cosf(to_radians(85.0)));
        assert!(
            low.x > low.y && low.y > low.z && low.x < 0.6 * up.x,
            "{low} 5° up"
        );
    }

    /// The transmittance from `height` km up along a way of zenith cosine `mu` out of the
    /// atmosphere, as the paper models it, summed in doubles by the midpoints of 200,000 steps.
    fn modelled(earth: &Atmosphere, height: f64, mu: f64) -> [f64; 3] {
        let (ground, top) = (f64::from(GROUND), f64::from(TOP));
        let from = ground + height;
        let reach = -from * mu + (from * from * (mu * mu - 1.0) + top * top).sqrt();
        let steps = 200_000;
        let step = reach / f64::from(steps);
        let mut depth = [0.0_f64; 3];
        for i in 0..steps {
            let t = (f64::from(i) + 0.5) * step;
            let up = (from * from + t * t + 2.0 * from * t * mu).sqrt() - ground;
            let rayleigh = libm::exp(-up / 8.0);
            let mie = libm::exp(-up / 1.2);
            let ozone = (1.0 - (up - 25.0).abs() / 15.0).clamp(0.0, 1.0);
            let haze = f64::from(earth.mie + earth.mie_absorption);
            for (channel, depth) in depth.iter_mut().enumerate() {
                *depth += (f64::from(earth.rayleigh[channel]) * rayleigh
                    + haze * mie
                    + f64::from(earth.ozone[channel]) * ozone)
                    * 1.0e-3
                    * step;
            }
        }
        depth.map(|depth| libm::exp(-depth))
    }

    /// Toward a low sun, 85°, 89° and 89.9° from the zenith, 10 m up, as the viewer is kept, the
    /// transmittance the light that reaches the world is dimmed by is the paper's model's, each
    /// channel within 2%, though the blue that comes through near the horizon is a thousandth of
    /// the red's.
    #[test]
    fn toward_a_low_sun_the_transmittance_is_the_papers_model() {
        let earth = Atmosphere::EARTH;
        for zenith in [85.0_f32, 89.0, 89.9] {
            let mu = libm::cosf(to_radians(zenith));
            let got = transmittance(&earth, GROUND + OFFSET, mu);
            let want = modelled(&earth, f64::from(OFFSET), f64::from(mu));
            for (channel, (&got, want)) in got.to_array().iter().zip(want).enumerate() {
                let off = (f64::from(got) / want - 1.0).abs();
                assert!(off < 0.02, "{zenith}° channel {channel}: {got}, not {want}");
            }
        }
    }

    /// The sun is the brightest directional light that shines, the first of those as bright, and
    /// the light that reaches the world from it is its own dimmed by the transmittance toward it,
    /// less as it's high than low, and none once it's set; the others aren't dimmed.
    #[test]
    fn the_brightest_directional_light_is_the_sun_dimmed_by_the_air() {
        let down = Vec3::new(0.0, -1.0, 0.0);
        let mut lights = vec![
            Light {
                kind: LightKind::Point,
                ..directional(down, Vec3::splat(9.0e5))
            },
            directional(down, Vec3::splat(2.0e4)),
            directional(Vec3::ZERO, Vec3::splat(8.0e5)),
            directional(down, Vec3::new(1.0e5, 0.9e5, 0.8e5)),
            directional(Vec3::NEG_X, Vec3::new(1.0e5, 0.9e5, 0.8e5)),
        ];
        let seen = Seen::new(Atmosphere::EARTH, &lights, &viewpoint(1.7));
        assert_eq!(seen.sun, Some(3), "past the brighter that shines no way");
        assert_eq!(seen.illuminance(), 1.0e5);
        assert_eq!(seen.tint, Vec3::new(1.0, 0.9, 0.8));
        let before = lights.clone();
        seen.dim(&mut lights);
        let through = transmittance(&Atmosphere::EARTH, GROUND + 0.01, 1.0);
        assert!(
            lights[3].color.abs_diff_eq(before[3].color * through, 1.0),
            "{:?}",
            lights[3]
        );
        assert_eq!(lights[..3], before[..3]);
        assert_eq!(lights[4], before[4]);
        // Low, dimmer and redder; set, out.
        let low = Seen::new(Atmosphere::EARTH, &[lights[4]], &viewpoint(1.7)).sunlight;
        assert!(
            low.x < through.x && low.z / low.x < through.z / through.x,
            "{low}"
        );
        let set = directional(Vec3::new(1.0, 0.1, 0.0), Vec3::ONE);
        let night = Seen::new(Atmosphere::EARTH, &[set], &viewpoint(1.7));
        assert_eq!(night.sunlight, Vec3::ZERO);
        assert!(night.set() && !seen.set(), "set at night alone");
        // Without a sun, none.
        let dark = Seen::new(Atmosphere::EARTH, &[], &viewpoint(1.7));
        assert_eq!((dark.sun, dark.illuminance()), (None, 0.0));
    }

    /// The sun's light fades over its disk's height as it sets, rather than going out at once.
    #[test]
    fn a_setting_sun_fades() {
        let earth = Atmosphere::EARTH;
        let half = earth.sun_size / 2.0;
        let at = |angle: f32| sunlight(&earth, GROUND + 0.01, libm::sinf(angle)).x;
        let share = GROUND / (GROUND + 0.01);
        let horizon = -libm::asinf((1.0 - share * share).sqrt());
        let [above, middle, below] = [half, 0.0, -half].map(|offset| at(horizon + offset));
        assert!(
            above > middle && middle > below && below == 0.0,
            "{above} {middle} {below}"
        );
        assert!(middle > 0.2 * above, "{middle} of {above}");
    }

    /// The sky's light is prefiltered again once the sun turns past 1° high in the sky, and past
    /// less the lower it is, the viewer rises past 100 m, the sun's color or the atmosphere
    /// changes, but not as the sun brightens.
    #[test]
    fn the_skys_light_is_prefiltered_again_as_the_sun_turns() {
        let sun = |angle: f32, color: Vec3| {
            let way = Vec3::new(libm::cosf(angle), -libm::sinf(angle), 0.0);
            directional(way, color)
        };
        let lit = |light: Light, height: f32, atmosphere: Atmosphere| {
            Seen::new(atmosphere, &[light], &viewpoint(height)).lit()
        };
        let earth = Atmosphere::EARTH;
        let held = lit(sun(0.5, Vec3::ONE), 10.0, earth);
        let degree = to_radians(1.0);
        assert!(lit(sun(0.5 + 0.9 * degree, Vec3::ONE), 10.0, earth).near(&held));
        assert!(lit(sun(0.5, Vec3::splat(5.0)), 10.0, earth).near(&held));
        assert!(lit(sun(0.5, Vec3::ONE), 90.0, earth).near(&held));
        assert!(!lit(sun(0.5 + 1.1 * degree, Vec3::ONE), 10.0, earth).near(&held));
        assert!(!lit(sun(0.5, Vec3::ONE), 120.0, earth).near(&held));
        assert!(!lit(sun(0.5, Vec3::new(1.0, 0.5, 0.5)), 10.0, earth).near(&held));
        let hazier = Atmosphere { mie: 8.0, ..earth };
        assert!(!lit(sun(0.5, Vec3::ONE), 10.0, hazier).near(&held));
        // Lower, as the sky's light changes faster, sooner: a hundredth of a degree at the horizon
        // and below, and about 0.074° 10° up; but past twilight, at night, a degree again.
        for (elevation, most) in [(0.0, 0.01), (-5.0, 0.01), (10.0, 0.0739), (-30.0, 1.0)] {
            let from = to_radians(elevation);
            let held = lit(sun(from, Vec3::ONE), 10.0, earth);
            // Turning down, so that the lower of the two is where it started.
            let turned = |by: f32| lit(sun(from - to_radians(by), Vec3::ONE), 10.0, earth);
            assert!(turned(0.97 * most).near(&held), "{elevation}°");
            assert!(!turned(1.03 * most).near(&held), "{elevation}°");
        }
    }
}
