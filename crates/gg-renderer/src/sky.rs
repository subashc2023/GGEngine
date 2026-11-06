//! The sky: an image of the world around, equirectangular, which the view shows behind the world
//! and which lights it from all around, prefiltered on the GPU by compute passes in the first
//! frame that renders the view once the image shows, and again whenever it shows another: into a
//! cube, whose levels each average the one before; from that, the light Lambert's diffuse takes,
//! and GGX's specular at each step of roughness; and, once, the split sum's table.
//!
//! A sky's sun lights once: in a frame with a directional light, which stands for it, casting
//! shadows where the sky's light can't, the sky's light keeps each of its texels to [`SUN`]'s
//! luminance, so that the sun lights through the directional light alone; in one without, the sky
//! lights by all of it.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::Vec3;
use gg_core::{Result, label};
use gg_gpu::{
    Blend, ComputePipeline, Device, Format, Frame, Graph, GraphicsDesc, ImageId, Pass, PassId,
    Pipeline, StorageImage, Target,
};

use crate::{Light, LightKind, Texture, Viewpoint};

static PREFILTER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prefilter.spv"));
static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv"));

/// The most texels a side of the cube a sky is turned into has, and the fewest: a quarter of the
/// sky's width, to the next power of two, which gives the cube about as many texels round its
/// middle as the image has across it. It's for prefiltering alone, as the view draws the sky from
/// its image, so a larger sky's is kept to 512, of about 17 MB, for the frame that prefilters it.
const MOST_CUBE: u32 = 512;
const LEAST_CUBE: u32 = 8;

/// The most texels a side of the specular light's first level has, its roughness 0: the cube's,
/// up to this. Its last level, of roughness 1, has this many, or its first's.
const MOST_SPECULAR: u32 = 256;
const LEAST_SPECULAR: u32 = 8;

/// The texels a side of the diffuse light's cube, and of the split sum's table, has.
const IRRADIANCE: u32 = 32;
const TABLE: u32 = 128;

/// How many samples each texel of the specular light, the diffuse light and the table takes.
const SPECULAR_SAMPLES: u32 = 512;
const IRRADIANCE_SAMPLES: u32 = 512;
const TABLE_SAMPLES: u32 = 512;

/// The threads across and down of each workgroup, as `prefilter.slang`'s `numthreads` has them.
const GROUP: u32 = 8;

/// The luminance, in candela per square meter, past which a sky's texels are its sun's: about a
/// hundredth of the sun's disk at noon, of some 1.6 × 10⁹, and past any cloud's. A sun low in its
/// sky, dimmed through more air, can fall short of it, and lights through the sky's light then as
/// well as through the directional light that stands for it.
const SUN: f32 = 1.0e7;

/// The luminance, in `sky`'s texels' units, that the sky's light keeps each of its texels to, in a
/// frame with `lights`: [`SUN`]'s, if a directional light shines, standing for the sky's sun, so
/// that the sun lights through it alone; and none, `f32::MAX`, if none does.
pub(crate) fn cutoff(sky: Sky, lights: &[Light]) -> f32 {
    let shines = |light: &Light| {
        light.kind == LightKind::Directional
            && light.color.cmpgt(Vec3::ZERO).any()
            && light.direction.try_normalize().is_some()
    };
    if sky.luminance > 0.0 && lights.iter().any(shines) {
        SUN / sky.luminance
    } else {
        f32::MAX
    }
}

/// The sky a frame shows, as [`RenderData::sky`](crate::RenderData::sky) gives it. Its sun lights
/// once: in a frame with a directional light that shines, which stands for it, the sky's light
/// keeps each of its texels to 10⁷ candela per square meter, leaving out the sun's disk, some
/// 1.6 × 10⁹ at noon, and lighting by the rest, as it shows; and lights by all of it in a frame
/// without.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    /// Its image of the world around, equirectangular: its center column the way toward +X, a
    /// quarter of the way across the way toward -Z, three quarters toward +Z, and its top row
    /// straight up, as Khronos's glTF Sample Renderer and three.js map one. A texture of colors,
    /// as one of [`Texels::Hdr`](crate::Texels::Hdr) or [`Texels::Bc6h`](crate::Texels::Bc6h) is
    /// of high dynamic range, which the view shows and lights by as its slot shows it, and not
    /// while it shows a stand-in.
    pub texture: Texture,
    /// The luminance, in candela per square meter, that a texel of 1 stands for: what the image's
    /// colors are multiplied by, before the exposure, as lit shading multiplies light, in lux and
    /// candela.
    pub luminance: f32,
}

/// The sky's part of the frame's data: `Frame`'s fields from `sky` on in `shaders/mesh.slang`,
/// field for field, all 0 in a frame without a sky.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, NoUninit)]
pub(crate) struct SkyData {
    /// The sky's texture.
    pub(crate) sky: u32,
    /// The slots of its light's cubes: the diffuse's and the specular's.
    pub(crate) irradiance: u32,
    pub(crate) specular: u32,
    /// How many levels the specular has; 0 for a frame without a sky.
    pub(crate) specular_levels: u32,
    /// The split sum's table's slot.
    pub(crate) brdf: u32,
    /// As [`Sky::luminance`] has it.
    pub(crate) luminance: f32,
}

/// What prefiltering pushes: `Push` in `shaders/prefilter.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct PrefilterPush {
    source_size: [u32; 2],
    source: u32,
    target: u32,
    size: u32,
    roughness: f32,
    samples: u32,
    cutoff: f32,
}

/// What the sky's draw pushes: `Push` in `shaders/sky.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct DrawPush {
    world_from_clip: gg_core::math::Mat4,
    sky: u32,
    scale: f32,
}

/// The sky's pipelines: those that prefilter it, and the one that draws it behind the world.
pub(crate) struct SkyPass {
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
                    format: Format::HDR.with_ids().with_depth(),
                    // Drawn first, at depth 0, which the cleared depth passes, writing none.
                    blend: Blend::Opaque,
                    depth_write: false,
                    depth_bias: 0.0,
                    depth_clamp: false,
                },
            )?,
        })
    }

    /// Records `plan`'s passes, the next of `frame`'s graph to record, which prefilter the sky into
    /// the light the plan's images hold.
    pub(crate) fn prefilter(&self, frame: &mut Frame<'_>, plan: &Plan) {
        let groups = |side: u32, layers: u32| [side.div_ceil(GROUP), side.div_ceil(GROUP), layers];
        let [cube, specular, irradiance, table] = plan.images;
        let face = plan.cube_face;
        let mut pass = frame.compute(plan.to_cube);
        pass.set_pipeline(&self.cube);
        let push = PrefilterPush {
            source_size: plan.sky_size,
            source: plan.sky,
            target: pass.storage(cube, 0),
            size: face,
            roughness: 0.0,
            samples: 0,
            cutoff: plan.cutoff,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(face, 6));
        drop(pass);
        for (level, &levels) in (1..).zip(&plan.levels) {
            let mut pass = frame.compute(levels);
            pass.set_pipeline(&self.downsample);
            let size = face >> level;
            let push = PrefilterPush {
                source_size: [face; 2],
                source: pass.storage(cube, level - 1),
                target: pass.storage(cube, level),
                size,
                roughness: 0.0,
                samples: 0,
                cutoff: f32::MAX,
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
            // A step of roughness each, from none at the first to 1 at the last.
            let roughness = if last == 0 {
                0.0
            } else {
                level as f32 / last as f32
            };
            let push = PrefilterPush {
                source_size: [face; 2],
                source: sampled,
                target: pass.storage(specular, level),
                size,
                roughness,
                samples: SPECULAR_SAMPLES,
                cutoff: f32::MAX,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(size, 6));
        }
        pass.set_pipeline(&self.irradiance);
        let push = PrefilterPush {
            source_size: [face; 2],
            source: sampled,
            target: pass.storage(irradiance, 0),
            size: IRRADIANCE,
            roughness: 0.0,
            samples: IRRADIANCE_SAMPLES,
            cutoff: f32::MAX,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.dispatch(groups(IRRADIANCE, 6));
        drop(pass);
        if let Some(written) = plan.table {
            let mut pass = frame.compute(written);
            pass.set_pipeline(&self.table);
            let push = PrefilterPush {
                source_size: [0; 2],
                source: 0,
                target: pass.storage(table, 0),
                size: TABLE,
                roughness: 0.0,
                samples: TABLE_SAMPLES,
                cutoff: f32::MAX,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            pass.dispatch(groups(TABLE, 1));
        }
    }

    /// Draws the sky behind everything the pass draws after it, over the whole of its target, as
    /// `viewpoint` sees it in a view `aspect` times as wide as it's tall: its texture, `sky`, times
    /// `scale`.
    ///
    /// # Panics
    ///
    /// If the pass isn't into an image of [`Format::HDR`] with IDs and a depth image.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        viewpoint: &Viewpoint,
        aspect: f32,
        sky: u32,
        scale: f32,
    ) {
        pass.begin_label(c"sky");
        pass.set_pipeline(&self.draw);
        let push = DrawPush {
            world_from_clip: viewpoint.ways_from_clip(aspect),
            sky,
            scale,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw(3, 1);
        pass.end_label();
    }
}

/// What a frame that prefilters the sky runs, as [`SkyLight::plan`] adds it to its graph.
pub(crate) struct Plan {
    /// The sky's texture, and its size.
    sky: u32,
    sky_size: [u32; 2],
    /// What the light keeps each of the sky's texels to, as [`cutoff`] gives it.
    cutoff: f32,
    /// The cube's side, and the specular light's first level's, and how many levels it has.
    cube_face: u32,
    specular_face: u32,
    specular_levels: u32,
    /// The cube, the specular light, the diffuse light and the table, among the frame's targets.
    images: [ImageId; 4],
    to_cube: PassId,
    /// Each of the cube's levels after its first.
    levels: Vec<PassId>,
    /// The light from all around.
    light: PassId,
    /// The table, in the first frame that prefilters.
    table: Option<PassId>,
}

impl Plan {
    /// Those of its images the view's pass samples, for its draws to light by: the specular light,
    /// the diffuse light and the table.
    pub(crate) fn sampled(&self) -> [ImageId; 3] {
        let [_, specular, irradiance, table] = self.images;
        [specular, irradiance, table]
    }

    /// The sky's part of the frame's data for a frame that prefilters the sky as this plans, of
    /// `sky`, its light sampled through the indices `sampled` gives, a pass's that samples it.
    pub(crate) fn data(&self, sampled: impl Fn(ImageId) -> u32, sky: Sky) -> SkyData {
        let [specular, irradiance, table] = self.sampled();
        SkyData {
            sky: sky.texture.index,
            irradiance: sampled(irradiance),
            specular: sampled(specular),
            specular_levels: self.specular_levels,
            brdf: sampled(table),
            luminance: sky.luminance,
        }
    }
}

/// The sky's light, prefiltered, in storage images that frames take as targets as they prefilter
/// it, and sample after: the cube a frame turns the sky into, made for the frame that prefilters,
/// which drops it once it's submitted; the light from all around; and the split sum's table, made
/// once.
#[derive(Default)]
pub(crate) struct SkyLight {
    cube: Option<StorageImage>,
    specular: Option<StorageImage>,
    irradiance: Option<StorageImage>,
    table: Option<StorageImage>,
    /// Whether a frame has written the table.
    tabled: bool,
    /// The texture, how many times its slot had come to show another image, and the bits of what
    /// each texel was kept to, as [`cutoff`] gives it, that the light holds, once a frame has
    /// prefiltered it.
    from: Option<(u32, u64, u32)>,
}

impl SkyLight {
    /// Whether the light holds `sky`'s texture as it's shown for the `shown`th time, each texel
    /// kept to `cutoff`.
    pub(crate) fn holds(&self, sky: Sky, shown: u64, cutoff: f32) -> bool {
        self.from == Some((sky.texture.index, shown, cutoff.to_bits()))
    }

    /// The sky's part of the frame's data for a frame that doesn't prefilter `sky`, its texture
    /// shown for the `shown`th time, each texel kept to `cutoff`, if the light holds it and the
    /// frames have left it to sample.
    pub(crate) fn data(&self, sky: Sky, shown: u64, cutoff: f32) -> Option<SkyData> {
        if !self.holds(sky, shown, cutoff) {
            return None;
        }
        let specular = self.specular.as_ref()?;
        Some(SkyData {
            sky: sky.texture.index,
            irradiance: self.irradiance.as_ref()?.sampled()?,
            specular: specular.sampled()?,
            specular_levels: specular.levels(),
            brdf: self.table.as_ref()?.sampled()?,
            luminance: sky.luminance,
        })
    }

    /// Makes what a frame needs to prefilter `sky`, `size` texels across and down, each texel kept
    /// to `cutoff`, and adds the passes that prefilter it to `graph`, its images the frame's
    /// targets from `first` on, in the order [`SkyLight::targets`] gives them: the cube, of
    /// [`MOST_CUBE`] texels a side at most, with its full chain of levels; the specular light, with
    /// a level for each step of roughness; the diffuse light; and the table, if no frame has
    /// written it.
    pub(crate) fn plan(
        &mut self,
        device: &Arc<Device>,
        graph: &mut Graph,
        first: usize,
        sky: Sky,
        size: [u32; 2],
        cutoff: f32,
    ) -> Result<Plan> {
        let face = (size[0] / 4)
            .max(1)
            .next_power_of_two()
            .clamp(LEAST_CUBE, MOST_CUBE);
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
        let to_cube = graph.compute(label!("sky cube")).write(cube).add();
        let levels = (1..cube_levels)
            .map(|_| graph.compute(label!("sky levels")).write(cube).add())
            .collect();
        let light = (graph.compute(label!("sky light")))
            .sample(cube)
            .write(specular)
            .write(irradiance)
            .add();
        let table = (!self.tabled).then(|| graph.compute(label!("split sum")).write(table).add());
        Ok(Plan {
            sky: sky.texture.index,
            sky_size: size,
            cutoff,
            cube_face: face,
            specular_face,
            specular_levels,
            images,
            to_cube,
            levels,
            light,
            table,
        })
    }

    /// The images a frame that prefilters takes as targets, in the order [`SkyLight::plan`] gave
    /// them their places: the cube, the specular light, the diffuse light and the table.
    ///
    /// # Panics
    ///
    /// If no plan has made them.
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

    /// Notes that a frame has prefiltered `sky`, its texture shown for the `shown`th time, into
    /// the light, by `plan`, and drops the cube, which retires until the GPU is done with it.
    pub(crate) fn prefiltered(&mut self, sky: Sky, shown: u64, plan: &Plan) {
        self.from = Some((sky.texture.index, shown, plan.cutoff.to_bits()));
        self.tabled |= plan.table.is_some();
        self.cube = None;
    }
}
