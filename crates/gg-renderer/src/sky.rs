//! The sky: an equirectangular image of the world around, shown behind it and lighting it. Compute
//! passes prefilter it in the first frame that shows an image:
//! - into a cube, each level averaging the one before;
//! - from that, Lambert's diffuse light, and GGX's specular at each step of roughness;
//! - once, the split sum's table.
//!
//! Its sun lights once, as [`Sky`] says. The [`atmosphere`]'s sky is prefiltered alike, from a cube
//! its sky table makes without the sun's disk.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3};
use gg_core::{Result, label};
use gg_gpu::{
    Blend, BufferId, ComputePipeline, Device, Format, Frame, Graph, GraphicsDesc, ImageId, Pass,
    PassId, Pipeline, StorageImage, Target,
};

use crate::atmosphere::{self, Lit};
use crate::{Light, LightKind, Texture, Viewpoint, taa};

static PREFILTER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prefilter.spv"));
static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv"));

/// The cube's most and fewest texels a side. It takes a quarter of the sky's width, to the next
/// power of two: about the image's texels across, round its middle. The view draws the sky from its
/// image, so the cube is for prefiltering alone, and capped at 512 (about 17 MB).
const MOST_CUBE: u32 = 512;
const LEAST_CUBE: u32 = 8;

/// The cube's most levels: its full chain at [`MOST_CUBE`].
const CUBE_LEVELS: u32 = MOST_CUBE.ilog2() + 1;

/// The specular light's most texels a side at its first level, roughness 0: the cube's, up to this.
/// Its last level, roughness 1, has this many, or the first's if fewer.
const MOST_SPECULAR: u32 = 256;
const LEAST_SPECULAR: u32 = 8;

/// Texels a side of the diffuse light's cube and of the split sum's table.
const IRRADIANCE: u32 = 32;
const TABLE: u32 = 128;

/// Samples per texel of an image sky's specular light, diffuse light and table. The atmosphere's
/// take [`atmosphere::SAMPLES`].
const SPECULAR_SAMPLES: u32 = 512;
const IRRADIANCE_SAMPLES: u32 = 512;
const TABLE_SAMPLES: u32 = 512;

/// Threads across and down per workgroup: `prefilter.slang`'s `numthreads`.
const GROUP: u32 = 8;

/// The buffer an image sky's prefiltering finds its sun into, and its size: `Sun` in
/// `shaders/prefilter.slang`.
const SUN: &str = "sky sun";
const SUN_BYTES: u64 = 48;

/// Whether a directional light among `lights` shines, standing for an image sky's sun.
pub(crate) fn stood_for(lights: &[Light]) -> bool {
    lights.iter().any(|light| {
        light.kind == LightKind::Directional
            && light.color.cmpgt(Vec3::ZERO).any()
            && light.direction.try_normalize().is_some()
    })
}

/// The sky a frame shows, as [`RenderData::sky`](crate::RenderData::sky) gives it.
///
/// Its sun lights once: a shining directional light stands for it, and casts the shadows the sky's
/// light can't. The sky's light then leaves out the sun, found by contrast: its brightest texel, if
/// at least 100 times the median of the sky 6° to 12° round it, and within 4° of that, what's past
/// the median. Without such a light, the sky lights by all of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    /// Its equirectangular image of the world around: the center column toward +X, a quarter across
    /// toward -Z, three quarters toward +Z, the top row straight up, as Khronos's glTF Sample
    /// Renderer and three.js map one. A color texture, of high dynamic range if
    /// [`Texels::Hdr`](crate::Texels::Hdr) or [`Texels::Bc6h`](crate::Texels::Bc6h). The view shows
    /// and lights by it as its slot shows it, not while it shows a stand-in.
    pub texture: Texture,
    /// The luminance in cd/m² a texel of 1 stands for: the image's colors are multiplied by it
    /// before exposure, as lit shading multiplies light in lux and candela.
    pub luminance: f32,
}

/// What a frame prefilters the sky's light from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Source {
    /// An image sky, its texture shown for the `shown`th time, `size` texels across and down; its
    /// light `leaves` out its sun if a directional light stands for it, as [`stood_for`] says.
    Image {
        sky: Sky,
        size: [u32; 2],
        shown: u64,
        leaves: bool,
    },
    /// The atmosphere, as the frame lights and sees it.
    Atmosphere(Lit),
}

impl Source {
    /// Whether light prefiltered from `held` stands for this: the same showing of the same image,
    /// leaving out its sun alike, or an atmosphere [near](Lit::near) it.
    fn held_by(&self, held: &Source) -> bool {
        match (self, held) {
            (
                Source::Image {
                    sky, shown, leaves, ..
                },
                Source::Image {
                    sky: was,
                    shown: then,
                    leaves: left,
                    ..
                },
            ) => sky.texture.index == was.texture.index && shown == then && leaves == left,
            (Source::Atmosphere(lit), Source::Atmosphere(was)) => lit.near(was),
            _ => false,
        }
    }

    /// The sky texture the view shows: an image sky's, or 0 for the atmosphere, which the view
    /// draws otherwise.
    fn texture(&self) -> u32 {
        match self {
            Source::Image { sky, .. } => sky.texture.index,
            Source::Atmosphere(_) => 0,
        }
    }
}

/// The sky's part of the frame's data: `Frame`'s fields from `sky` on in `shaders/mesh.slang`,
/// field for field. All 0 in a frame without a sky.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct SkyData {
    pub(crate) sky: u32,
    /// The slots of the diffuse and specular light's cubes.
    pub(crate) irradiance: u32,
    pub(crate) specular: u32,
    /// 0 for a frame without a sky.
    pub(crate) specular_levels: u32,
    /// The split sum table's slot.
    pub(crate) brdf: u32,
    /// As [`Sky::luminance`].
    pub(crate) luminance: f32,
}

/// `Push` in `shaders/prefilter.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct PrefilterPush {
    sun: u64,
    source_size: [u32; 2],
    source: u32,
    target: u32,
    size: u32,
    roughness: f32,
    samples: u32,
    leaves: u32,
}

/// `Push` in `shaders/sky.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct DrawPush {
    world_from_clip: gg_core::math::Mat4,
    sky: u32,
    scale: f32,
}

/// The pipelines that prefilter the sky and draw it behind the world.
pub(crate) struct SkyPass {
    sun: ComputePipeline,
    cube: ComputePipeline,
    downsample: ComputePipeline,
    irradiance: ComputePipeline,
    specular: ComputePipeline,
    table: ComputePipeline,
    draw: Pipeline,
}

impl SkyPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let compute = |name, entry| ComputePipeline::new(device, name, PREFILTER, entry);
        Ok(Self {
            sun: compute("sky sun", c"sun_main")?,
            cube: compute("sky cube", c"cube_main")?,
            downsample: compute("sky levels", c"downsample_main")?,
            irradiance: compute("sky irradiance", c"irradiance_main")?,
            specular: compute("sky specular", c"specular_main")?,
            table: compute("split sum", c"brdf_main")?,
            draw: Pipeline::graphics(
                device,
                &GraphicsDesc {
                    name: "sky",
                    spirv: SPIRV,
                    vertex: c"vertex_main",
                    mesh: None,
                    fragment: Some(c"fragment_main"),
                    format: Format::HDR.with_ids().with_motion().with_depth(),
                    // Drawn first, at depth 0, which the cleared depth passes, writing no depth or
                    // motion.
                    blend: Blend::Opaque,
                    depth_write: false,
                    ids_write: true,
                    depth_bias: 0.0,
                    depth_clamp: false,
                },
            )?,
        })
    }

    /// Records `plan`'s passes that find an image sky's sun and turn the sky into the cube's first
    /// level, the next of `frame`'s graph.
    ///
    /// # Panics
    ///
    /// - The plan's sky isn't an image.
    pub(crate) fn image_to_cube(&self, frame: &mut Frame<'_>, plan: &Plan) {
        let (
            Source::Image {
                sky, size, leaves, ..
            },
            Some((sun, find)),
        ) = (plan.source, plan.sun)
        else {
            panic!("an image sky is turned into a cube, and the plan's isn't one");
        };
        let mut pass = frame.compute(find);
        pass.set_pipeline(&self.sun);
        let push = PrefilterPush {
            sun: pass.address(sun),
            source_size: size,
            source: sky.texture.index,
            target: 0,
            size: 0,
            roughness: 0.0,
            samples: 0,
            leaves: 0,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([1, 1, 1]);
        drop(pass);
        let face = plan.cube_face;
        let mut pass = frame.compute(plan.to_cube);
        pass.set_pipeline(&self.cube);
        let push = PrefilterPush {
            sun: pass.address(sun),
            source_size: size,
            source: sky.texture.index,
            target: pass.storage(plan.images[0], 0),
            size: face,
            roughness: 0.0,
            samples: 0,
            leaves: u32::from(leaves),
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch([face.div_ceil(GROUP), face.div_ceil(GROUP), 6]);
    }

    /// Records `plan`'s passes after the cube's first level, the next of `frame`'s graph:
    /// prefiltering the cube into the plan's light images.
    pub(crate) fn prefilter(&self, frame: &mut Frame<'_>, plan: &Plan) {
        let groups = |side: u32, layers: u32| [side.div_ceil(GROUP), side.div_ceil(GROUP), layers];
        let [cube, specular, irradiance, table] = plan.images;
        let face = plan.cube_face;
        for (level, &levels) in (1..).zip(plan.levels.iter().flatten()) {
            let mut pass = frame.compute(levels);
            pass.set_pipeline(&self.downsample);
            let size = face >> level;
            let push = PrefilterPush {
                sun: 0,
                source_size: [face; 2],
                source: pass.storage(cube, level - 1),
                target: pass.storage(cube, level),
                size,
                roughness: 0.0,
                samples: 0,
                leaves: 0,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(size, 6));
        }
        let mut pass = frame.compute(plan.light);
        let sampled = pass.sampled(cube);
        pass.set_pipeline(&self.specular);
        let last = plan.specular_levels - 1;
        for level in 0..plan.specular_levels {
            let size = plan.specular_face >> level;
            // A step of roughness each, from 0 at the first to 1 at the last.
            let roughness = if last == 0 {
                0.0
            } else {
                level as f32 / last as f32
            };
            let push = PrefilterPush {
                sun: 0,
                source_size: [face; 2],
                source: sampled,
                target: pass.storage(specular, level),
                size,
                roughness,
                samples: plan.samples[0],
                leaves: 0,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(size, 6));
        }
        pass.set_pipeline(&self.irradiance);
        let push = PrefilterPush {
            sun: 0,
            source_size: [face; 2],
            source: sampled,
            target: pass.storage(irradiance, 0),
            size: IRRADIANCE,
            roughness: 0.0,
            samples: plan.samples[1],
            leaves: 0,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(IRRADIANCE, 6));
        drop(pass);
        if let Some(written) = plan.table {
            let mut pass = frame.compute(written);
            pass.set_pipeline(&self.table);
            let push = PrefilterPush {
                sun: 0,
                source_size: [0; 2],
                source: 0,
                target: pass.storage(table, 0),
                size: TABLE,
                roughness: 0.0,
                samples: TABLE_SAMPLES,
                leaves: 0,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(TABLE, 1));
        }
    }

    /// Draws the sky, its texture `sky` times `scale`, behind everything the pass draws after it,
    /// over the whole target. It's seen from `viewpoint` at `aspect` (width over height), offset by
    /// antialiasing's `jitter` in normalized coordinates.
    ///
    /// # Panics
    ///
    /// - The pass isn't into an image of [`Format::HDR`] with IDs, motion and a depth image.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewpoint: &Viewpoint,
        aspect: f32,
        jitter: Vec2,
        sky: u32,
        scale: f32,
    ) {
        pass.begin_label(c"sky");
        pass.set_pipeline(&self.draw);
        let push = DrawPush {
            // Back from where the offset drew each pixel's way.
            world_from_clip: viewpoint.ways_from_clip(aspect) * taa::jitter(-jitter),
            sky,
            scale,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
        pass.end_label();
    }
}

/// What a frame that prefilters the sky runs, as [`SkyLight::plan`] adds it to the graph.
pub(crate) struct Plan {
    source: Source,
    cube_face: u32,
    /// Samples per texel of the specular light and the diffuse light.
    samples: [u32; 2],
    specular_face: u32,
    specular_levels: u32,
    /// The cube, the specular light, the diffuse light and the table, among the frame's targets.
    images: [ImageId; 4],
    /// For an image sky, the buffer its sun is found into and the pass that finds it.
    sun: Option<(BufferId, PassId)>,
    to_cube: PassId,
    /// A pass per level after the cube's first, as many as it has, so a frame that prefilters
    /// again, as the atmosphere's sun turns, allocates nothing.
    levels: [Option<PassId>; CUBE_LEVELS as usize - 1],
    /// The light from all around.
    light: PassId,
    /// The table, in the first frame that prefilters.
    table: Option<PassId>,
}

impl Plan {
    /// The images the view's pass samples to light by: the specular light, the diffuse light and
    /// the table.
    pub(crate) fn sampled(&self) -> [ImageId; 3] {
        let [_, specular, irradiance, table] = self.images;
        [specular, irradiance, table]
    }

    /// The sky's part of the frame's data for a frame that prefilters as planned. `sampled` gives
    /// the light's sampled indices; a texel of 1 stands for `luminance`.
    pub(crate) fn data(&self, sampled: impl Fn(ImageId) -> u32, luminance: f32) -> SkyData {
        let [specular, irradiance, table] = self.sampled();
        SkyData {
            sky: self.source.texture(),
            irradiance: sampled(irradiance),
            specular: sampled(specular),
            specular_levels: self.specular_levels,
            brdf: sampled(table),
            luminance,
        }
    }

    /// The pass that makes the cube's first level, the cube, and its side.
    pub(crate) fn to_cube(&self) -> (PassId, ImageId, u32) {
        (self.to_cube, self.images[0], self.cube_face)
    }
}

/// The sky's prefiltered light, in storage images that prefiltering frames write as targets and
/// later frames sample: the cube, the light from all around, and the split sum's table, made once.
#[derive(Default)]
pub(crate) struct SkyLight {
    cube: Option<StorageImage>,
    specular: Option<StorageImage>,
    irradiance: Option<StorageImage>,
    table: Option<StorageImage>,
    tabled: bool,
    /// What the light was prefiltered from, once a frame has.
    from: Option<Source>,
}

impl SkyLight {
    /// Whether the light holds what `source` stands for, as [`Source::held_by`] says.
    pub(crate) fn holds(&self, source: &Source) -> bool {
        (self.from.as_ref()).is_some_and(|held| source.held_by(held))
    }

    /// The sky's part of the frame's data for a frame that doesn't prefilter `source`, a texel of 1
    /// standing for `luminance`. `None` unless the light holds `source` and is left to sample.
    pub(crate) fn data(&self, source: &Source, luminance: f32) -> Option<SkyData> {
        if !self.holds(source) {
            return None;
        }
        let specular = self.specular.as_ref()?;
        Some(SkyData {
            sky: source.texture(),
            irradiance: self.irradiance.as_ref()?.sampled()?,
            specular: specular.sampled()?,
            specular_levels: specular.levels(),
            brdf: self.table.as_ref()?.sampled()?,
            luminance,
        })
    }

    /// Makes what a frame needs to prefilter `source`, and adds the passes to `graph`: first, for
    /// an image sky, the pass finding its sun into a buffer the cube's first level reads. Their
    /// images are the frame's targets from `first` on, in [`SkyLight::targets`]' order:
    /// - the cube, sized as [`MOST_CUBE`] says, or [`atmosphere::CUBE`] for the atmosphere, with
    ///   its full chain of levels. Its first level's pass samples `sampled`, if given: the
    ///   atmosphere's sky table;
    /// - the specular light, a level per step of roughness;
    /// - the diffuse light;
    /// - the table, if no frame has written it.
    pub(crate) fn plan(
        &mut self,
        device: &Arc<Device>,
        graph: &mut Graph,
        first: usize,
        source: Source,
        sampled: Option<ImageId>,
    ) -> Result<Plan> {
        let face = match source {
            Source::Image { size, .. } => (size[0] / 4)
                .max(1)
                .next_power_of_two()
                .clamp(LEAST_CUBE, MOST_CUBE),
            Source::Atmosphere(_) => atmosphere::CUBE,
        };
        let samples = match source {
            Source::Image { .. } => [SPECULAR_SAMPLES, IRRADIANCE_SAMPLES],
            Source::Atmosphere(_) => [atmosphere::SAMPLES; 2],
        };
        let cube_levels = face.ilog2() + 1;
        let specular_face = face.min(MOST_SPECULAR);
        let specular_levels = (specular_face / LEAST_SPECULAR.min(specular_face)).ilog2() + 1;
        let sides = |image: &Option<StorageImage>| image.as_ref().map(StorageImage::size);
        if sides(&self.cube) != Some([face; 2]) {
            let cube = StorageImage::new(device, [face; 2], cube_levels, true, "sky cube")?;
            self.cube = Some(cube);
        }
        if sides(&self.specular) != Some([specular_face; 2]) {
            let specular = [specular_face; 2];
            let made = StorageImage::new(device, specular, specular_levels, true, "sky specular")?;
            self.specular = Some(made);
            self.from = None;
        }
        if self.irradiance.is_none() {
            let made = StorageImage::new(device, [IRRADIANCE; 2], 1, true, "sky irradiance")?;
            self.irradiance = Some(made);
        }
        if self.table.is_none() {
            let made = StorageImage::new(device, [TABLE; 2], 1, false, "split sum")?;
            self.table = Some(made);
        }
        let images = [0, 1, 2, 3].map(|i| graph.target(first + i));
        let [cube, specular, irradiance, table] = images;
        let sun = matches!(source, Source::Image { .. }).then(|| {
            let sun = graph.buffer(SUN, SUN_BYTES);
            (
                sun,
                graph.compute(label!("sky sun")).write_buffer(sun).add(),
            )
        });
        let to_cube = graph.compute(label!("sky cube")).write(cube);
        let to_cube = match sampled {
            Some(image) => to_cube.sample(image),
            None => to_cube,
        };
        let to_cube = match sun {
            Some((sun, _)) => to_cube.read_buffer(sun),
            None => to_cube,
        }
        .add();
        let levels = std::array::from_fn(|level| {
            let below = level as u32 + 1 < cube_levels;
            below.then(|| graph.compute(label!("sky levels")).write(cube).add())
        });
        let light = (graph.compute(label!("sky light")))
            .sample(cube)
            .write(specular)
            .write(irradiance)
            .add();
        let table = (!self.tabled).then(|| graph.compute(label!("split sum")).write(table).add());
        Ok(Plan {
            source,
            cube_face: face,
            samples,
            specular_face,
            specular_levels,
            images,
            sun,
            to_cube,
            levels,
            light,
            table,
        })
    }

    /// The images a frame that prefilters takes as targets, in [`SkyLight::plan`]'s order: the
    /// cube, the specular light, the diffuse light and the table.
    ///
    /// # Panics
    ///
    /// - No plan has made them.
    pub(crate) fn targets(&mut self) -> impl Iterator<Item = Target<'_>> {
        let made = "a plan makes the sky's images";
        [
            self.cube.as_mut().expect(made),
            self.specular.as_mut().expect(made),
            self.irradiance.as_mut().expect(made),
            self.table.as_mut().expect(made),
        ]
        .into_iter()
        .map(Target::Storage)
    }

    /// Notes that a frame has prefiltered by `plan`. An image sky's cube is dropped, retiring once
    /// the GPU is done; the atmosphere's, a megabyte, is kept for frames that prefilter again as
    /// its sun turns.
    pub(crate) fn prefiltered(&mut self, plan: &Plan) {
        self.from = Some(plan.source);
        self.tabled |= plan.table.is_some();
        if let Source::Image { .. } = plan.source {
            self.cube = None;
        }
    }
}
