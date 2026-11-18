//! The path tracer: a reference for the light the view shades, which [`Show::PathTraced`] shows on
//! a device with ray queries. `shaders/trace.slang` says what it computes; this keeps what it
//! traces and plans its pass.
//!
//! - A BLAS per drawable model mesh's level 0, the raster's own triangles up close, built by the
//!   first frame that traces the mesh, and kept until the mesh goes or its buffers change.
//! - A TLAS over the opaque and masked model instances, the scene's and the frame's draws', beside a
//!   table of what hits on each read. Both are written, and the TLAS built, as they change.
//! - Each pixel's light, summed unexposed over its paths in a graph buffer frames keep by name: up
//!   to [`PER_FRAME`] paths a pixel a frame, until the samples asked for, after which the image
//!   holds.
//! - Tracing starts over as what it reads changes: the viewpoint or the view's size, the
//!   instances, the lights, the sky, the exposure, a mesh, texture or material coming, changing or
//!   going, or a frame that didn't trace.
//!
//! The instances are gathered again in each frame that traces, which costs the CPU what the scene
//! and the frame's draws hold: it's a reference mode, and a frame that doesn't trace costs nothing.
//!
//! [`Show::PathTraced`]: crate::Show::PathTraced

use std::sync::Arc;

use bytemuck::{NoUninit, Pod, Zeroable};
use gg_core::math::{Mat4, Vec3};
use gg_core::{Result, label};
use gg_gpu::{
    Blas, BufferId, ComputePass, ComputePipeline, Device, Frame, GpuBuffer, Graph, ImageId, PassId,
    Tlas, TlasInstance, Triangles,
};

use crate::mesh::{FrameData, MeshPass, TracedInstance};
use crate::packed::HEADER_VERTICES;
use crate::{Draws, PackedVertex, halves};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/trace.spv"));

/// A workgroup's side, in pixels: `GROUP` in `shaders/trace.slang`.
const GROUP: u32 = 8;

/// The most paths a pixel takes in a frame.
const PER_FRAME: u32 = 4;

/// What hits on an instance read: `Traced` in `shaders/trace.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct Traced {
    vertices: [u32; 2],
    indices: [u32; 2],
    material: u32,
    unused: u32,
}

/// `Trace` in `shaders/trace.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct TraceData {
    world_from_clip: [f32; 16],
    tlas: [u32; 2],
    size: [u32; 2],
    taken: u32,
    samples: u32,
    target: u32,
    unused: u32,
}

/// `TracePush` in `shaders/trace.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct TracePush {
    trace: u64,
    frame: u64,
    traced: u64,
    sums: u64,
}

/// What a frame that traces reads besides its instances, by which it starts over or goes on.
pub(crate) struct Seen<'a> {
    pub(crate) number: u64,
    pub(crate) size: [u32; 2],
    /// Unjittered.
    pub(crate) clip_from_world: Mat4,
    /// The lights, as shaders read them.
    pub(crate) lights: &'a [u8],
    /// The luminance a sky texel of 1 stands for, if there's a sky, and whether its light was
    /// prefiltered anew in this frame.
    pub(crate) sky: Option<f32>,
    pub(crate) prefiltered: bool,
    pub(crate) exposure: f32,
    /// As [`MeshPass::uploaded`] says.
    pub(crate) uploaded: bool,
    /// The paths a pixel takes in all: at least one.
    pub(crate) samples: u32,
}

/// The tracer's pass in a frame's graph: the view's size, the light it sums into and the colors
/// it writes, which the view shows.
pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    sums: BufferId,
    shown: ImageId,
}

impl Plan {
    pub(crate) fn shown(&self) -> ImageId {
        self.shown
    }
}

pub(crate) struct TracePass {
    device: Arc<Device>,
    pipeline: ComputePipeline,
    tlas: Tlas,
    /// By mesh handle: the address of the vertices it was built from, and its BLAS.
    blases: Vec<Option<(u64, Blas)>>,
    /// The instances the frame traces, kept so frames don't allocate.
    found: Vec<TracedInstance>,
    /// The TLAS's instances and what hits on each read, as last written, and the next frame's.
    instances: Vec<TlasInstance>,
    traced: Vec<Traced>,
    next_instances: Vec<TlasInstance>,
    next_traced: Vec<Traced>,
    /// Their tables, and how many rows each holds.
    tables: Option<(GpuBuffer, GpuBuffer, usize)>,
    /// What the last frame that traced read besides its instances.
    number: u64,
    size: [u32; 2],
    clip_from_world: Mat4,
    lights: Vec<u8>,
    sky: Option<f32>,
    exposure: f32,
    /// The paths each pixel took before the frame being recorded, and those it takes.
    taken: u32,
    taking: u32,
}

impl TracePass {
    /// # Panics
    ///
    /// If the device has no ray queries.
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            device: device.clone(),
            pipeline: ComputePipeline::new(device, "path trace", SPIRV, c"trace_main")?,
            tlas: Tlas::new(device, "traced instances")?,
            blases: Vec::new(),
            found: Vec::new(),
            instances: Vec::new(),
            traced: Vec::new(),
            next_instances: Vec::new(),
            next_traced: Vec::new(),
            tables: None,
            number: 0,
            size: [0; 2],
            clip_from_world: Mat4::ZERO,
            lights: Vec::new(),
            sky: None,
            exposure: 0.0,
            taken: 0,
            taking: 0,
        })
    }

    /// How many paths each pixel has taken, as of the last frame recorded.
    pub(crate) fn taken(&self) -> u32 {
        self.taken
    }

    /// Declares in `graph` the pass tracing the view, of `size` pixels, which samples `sky`, the
    /// sky's light if the frame prefilters it.
    pub(crate) fn plan(
        graph: &mut Graph,
        size: [u32; 2],
        sky: impl Iterator<Item = ImageId>,
    ) -> Plan {
        let shown = graph.hdr("path traced", size);
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 16;
        let sums = graph.buffer("path traced light", bytes);
        let pass = sky.fold(graph.compute(label!("path trace")), ComputePass::sample);
        Plan {
            pass: pass.write_buffer(sums).overwrite(shown).add(),
            size,
            sums,
            shown,
        }
    }

    /// Readies the frame's tracing, before its passes: builds the BLASes of meshes newly drawn,
    /// writes the instances and builds the TLAS if they changed, and starts over if what it reads
    /// did, as `seen` says.
    pub(crate) fn prepare(
        &mut self,
        frame: &mut Frame<'_>,
        meshes: &MeshPass,
        draws: &Draws,
        seen: &Seen<'_>,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        meshes.traced_instances(draws, &mut self.found);
        // A mesh gone or streamed anew has another BLAS made when next traced.
        for (index, held) in self.blases.iter_mut().enumerate() {
            let address = meshes
                .traced_mesh(index as u32)
                .map(|mesh| mesh.vertices.address());
            if held
                .as_ref()
                .is_some_and(|&(built, _)| Some(built) != address)
            {
                *held = None;
            }
        }
        self.next_instances.clear();
        self.next_traced.clear();
        for found in &self.found {
            let Some(mesh) = meshes.traced_mesh(found.mesh) else {
                continue;
            };
            if mesh.triangles == 0 || mesh.positions.count == 0 {
                continue;
            }
            let index = found.mesh as usize;
            if self.blases.len() <= index {
                self.blases.resize_with(index + 1, || None);
            }
            let first_index = u64::from(mesh.first_index) * 4;
            if self.blases[index].is_none() {
                let stride = size_of::<PackedVertex>() as u64;
                let triangles = Triangles {
                    vertices: mesh.vertices,
                    first_vertex: HEADER_VERTICES as u64 * stride,
                    vertex_stride: stride,
                    vertex_count: mesh.positions.count,
                    indices: mesh.indices,
                    first_index,
                    triangles: mesh.triangles,
                    opaque: true,
                };
                let blas = frame.build_blas(&triangles, "traced mesh")?;
                self.blases[index] = Some((mesh.vertices.address(), blas));
            }
            let (_, blas) = self.blases[index].as_ref().expect("just built");
            let row = self.next_traced.len() as u32;
            // Its BLAS holds the counts read as UNORM (`Triangles`): to the mesh's space, as
            // `unpack` takes them, then to the world.
            let mesh_from_counts = Mat4::from_translation(mesh.positions.offset)
                * Mat4::from_scale(Vec3::splat(mesh.positions.step * 65535.0));
            let [x, y, z, w] = (found.transform * mesh_from_counts).to_cols_array_2d();
            let transform = [0, 1, 2].map(|r| [x[r], y[r], z[r], w[r]]);
            let faces = if found.both_faces {
                TlasInstance::BOTH_FACES
            } else {
                0
            };
            let mask = if found.masked {
                TlasInstance::NOT_OPAQUE
            } else {
                0
            };
            self.next_instances.push(TlasInstance {
                transform,
                index_and_mask: row | 0xFF << 24,
                offset_and_flags: faces | mask,
                blas: blas.address(),
            });
            self.next_traced.push(Traced {
                vertices: halves(mesh.vertices.address()),
                indices: halves(mesh.indices.address() + first_index),
                material: found.material,
                unused: 0,
            });
        }
        let changed = bytemuck::cast_slice::<_, u8>(&self.next_instances)
            != bytemuck::cast_slice::<_, u8>(&self.instances)
            || self.next_traced != self.traced
            || self.tables.is_none();
        if changed {
            std::mem::swap(&mut self.instances, &mut self.next_instances);
            std::mem::swap(&mut self.traced, &mut self.next_traced);
            self.write_tables(frame)?;
        }
        let restart = changed
            || seen.number != self.number + 1
            || seen.size != self.size
            || seen.clip_from_world != self.clip_from_world
            || seen.lights != &self.lights[..]
            || seen.sky != self.sky
            || seen.prefiltered
            || seen.exposure != self.exposure
            || seen.uploaded;
        if restart {
            self.taken = 0;
            self.size = seen.size;
            self.clip_from_world = seen.clip_from_world;
            self.lights.clear();
            self.lights.extend_from_slice(seen.lights);
            self.sky = seen.sky;
            self.exposure = seen.exposure;
        }
        self.number = seen.number;
        self.taking = PER_FRAME.min(seen.samples.max(1).saturating_sub(self.taken));
        Ok(())
    }

    /// Writes the instances and what hits on them read into their tables, made anew as they
    /// outgrow them, and builds the TLAS over them. A table holds a row however few there are, as
    /// builds take only buffers frames have written.
    fn write_tables(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        let count = self.instances.len();
        if self
            .tables
            .as_ref()
            .is_none_or(|&(.., room)| room < count.max(1))
        {
            let device = &self.device;
            let room = count.max(1).next_power_of_two();
            let bytes = |row: usize| (room * row) as u64;
            let instances =
                GpuBuffer::new(device, bytes(size_of::<TlasInstance>()), "traced instances")?;
            let traced = GpuBuffer::new(device, bytes(size_of::<Traced>()), "traced hits")?;
            self.tables = Some((instances, traced, room));
        }
        let (instances, traced, _) = self.tables.as_mut().expect("just made");
        if count == 0 {
            frame.write_buffer(instances, 0, &[0; size_of::<TlasInstance>()])?;
            frame.write_buffer(traced, 0, &[0; size_of::<Traced>()])?;
        } else {
            frame.write_buffer(instances, 0, bytemuck::cast_slice(&self.instances))?;
            frame.write_buffer(traced, 0, bytemuck::cast_slice(&self.traced))?;
        }
        frame.build_tlas(&mut self.tlas, instances, 0, count as u32)
    }

    /// Records `plan`'s pass, which reads the frame's data `frame_data` gives, from the indices of
    /// the images it samples.
    pub(crate) fn record(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &Plan,
        frame_data: impl FnOnce(&dyn Fn(ImageId) -> u32) -> FrameData,
    ) -> Result<()> {
        let mut pass = frame.compute(plan.pass);
        let shared = frame_data(&|image| pass.sampled(image));
        let shared = pass.upload(bytemuck::bytes_of(&shared))?.address();
        let trace = TraceData {
            world_from_clip: self.clip_from_world.inverse().to_cols_array(),
            tlas: halves(self.tlas.address()),
            size: plan.size,
            taken: self.taken,
            samples: self.taking,
            target: pass.storage(plan.shown, 0),
            unused: 0,
        };
        let trace = pass.upload(bytemuck::bytes_of(&trace))?.address();
        let (_, traced, _) = self.tables.as_ref().expect("written as the frame began");
        let push = TracePush {
            trace,
            frame: shared,
            traced: traced.address(),
            sums: pass.address(plan.sums),
        };
        pass.set_pipeline(&self.pipeline);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
        self.taken += self.taking;
        Ok(())
    }
}
