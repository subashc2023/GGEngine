//! Particles of the frame's emitters, owned by the frames, not the world.
//!
//! Each particle is a birth record in a GPU pool kept across frames, in its emitter's ring. Its age
//! alone gives where it is and how it looks. So a frame writes only births, in a compute pass
//! before the view's (`shaders/emit.slang`), and draws every ring slot among the blended draws
//! (`shaders/particles.slang`). They draw before TAA's resolve and write no motion, so the resolve
//! takes what's behind them; their soft edges shimmer little under its jitter.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Vec3, Vec4};
use gg_core::{Result, label};
use gg_gpu::{
    Blend, BufferId, ComputePipeline, Device, Faces, Format, Frame, GpuBuffer, Graph, GraphicsDesc,
    Pass, PassId, Pipeline, RenderPass, Sampler,
};
use tracing::warn;

use crate::{DrawCounts, Draws, Texture};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/particles.spv"));
static EMIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/emit.spv"));

/// The pool's capacity, all emitters together: 2^20, 32 MiB of birth records.
pub const MAX_PARTICLES: u32 = 1 << 20;

/// So small emitters don't cut the pool fine.
const LEAST_RING: u32 = 64;

/// Once the pool has any slots.
const LEAST_POOL: u32 = 1 << 12;

/// Block orders (log2 of slots), from a slot to the largest pool.
const ORDERS: usize = MAX_PARTICLES.trailing_zeros() as usize + 1;

/// Bytes of `Particle` in `shaders/particle.slang`.
const RECORD: u64 = 32;

/// Threads of a `shaders/emit.slang` workgroup, a slot or a word each.
const GROUP: u32 = 64;

/// The most workgroups across a dispatch that every device takes. Growing a larger pool takes
/// several dispatches.
const MOST_GROUPS: u32 = 65_535;

/// `DISC` in `shaders/particle.slang`: a soft disc in place of a texture.
const DISC: u32 = u32::MAX;

/// The pool's names, alternating as it grows, so the frame that grows it declares both and copies
/// one into the other.
const POOLS: [&str; 2] = ["particle pool a", "particle pool b"];

/// Where particles are born in the emitter's space, and which way they go. Unrotated, they go up
/// +Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleShape {
    /// At its origin, any way.
    Point,
    /// Within `radius`, outward.
    Sphere { radius: f32 },
    /// In the box, along +Y.
    Box { half_extents: Vec3 },
    /// On the disc of `radius` in the XZ plane, within `angle` of +Y (the cone's half angle, in
    /// radians).
    Cone { angle: f32, radius: f32 },
}

/// The emitter's own part of [`Particles`]: how they're born, move and look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emission {
    pub shape: ParticleShape,
    /// Least and most speed at birth, in world units a second, whatever the emitter's scale. Each
    /// particle picks at random between them, as for lifetime and spin.
    pub speed: [f32; 2],
    /// Least and most seconds alive. 0 or less is never drawn.
    pub lifetime: [f32; 2],
    /// Least and most radians a second, counterclockwise as seen.
    pub spin: [f32; 2],
    /// In world units a second squared.
    pub acceleration: Vec3,
    /// Linear drag, per second: dv/dt = acceleration - drag * v. 0 or less for none.
    pub drag: f32,
    /// Size across in the world at birth and at death.
    pub size: [f32; 2],
    /// Color at birth and at death: linear RGBA, alpha not premultiplied, past 1 for light.
    pub color: [Vec4; 2],
    /// Tinted by `color`, sampled as stored by [`Sampler::LinearClamp`]. `None` is a soft disc.
    pub texture: Option<Texture>,
    /// Even cells across and down, played row by row from the top left over a life. `[1, 1]` is the
    /// whole texture; a side of 0 is 1.
    pub cells: [u32; 2],
    /// Adds light to what's behind, rather than blending by alpha.
    pub additive: bool,
}

/// An emitter's births in a frame: `count` particles numbered from `first`.
///
/// The first is born at `time` (world ticks), each next `every` ticks later, at `along` of the way from `from` (last
/// frame's transform) to `to` (this frame's), each next `step` further.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Births {
    pub first: u64,
    pub count: u32,
    pub time: f64,
    pub every: f64,
    pub from: Mat4,
    pub to: Mat4,
    pub along: f32,
    pub step: f32,
}

impl Births {
    /// None, of an emitter at `at`.
    pub fn none(at: Mat4) -> Self {
        Self {
            first: 0,
            count: 0,
            time: 0.0,
            every: 0.0,
            from: at,
            to: at,
            along: 1.0,
            step: 0.0,
        }
    }
}

/// An emitter's bursts in a frame: `count` bursts of `size`, numbered from `first`, less the first
/// `skip` particles. All are born at `time` (world ticks), where `at` places the emitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bursts {
    pub first: u32,
    pub count: u32,
    pub size: u32,
    pub skip: u32,
    pub time: f64,
    pub at: Mat4,
}

impl Bursts {
    /// None, of an emitter at `at`.
    pub fn none(at: Mat4) -> Self {
        Self {
            first: 0,
            count: 0,
            size: 0,
            skip: 0,
            time: 0.0,
            at,
        }
    }

    pub fn particles(&self) -> u64 {
        (u64::from(self.count) * u64::from(self.size)).saturating_sub(u64::from(self.skip))
    }
}

/// An emitter's particles in a frame, drawn where [`Draws::push_particles`] puts them.
///
/// The renderer keeps them across frames by `id`, in a ring of their own, while each frame draws them. A
/// particle's randomness hashes `id` with its number (or its burst's and its own). They write `id`
/// for picks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particles {
    pub id: u64,
    /// Whether its ring starts empty this frame, as when the engine restarts an emitter.
    pub fresh: bool,
    /// The most alive at once, its ring's size: births a second times the most lifetime, plus a
    /// burst. A frame's oldest births past that aren't drawn. Past [`MAX_PARTICLES`], the newest
    /// draw alone, with a warning (`renderer.particles_full`).
    pub most: u32,
    /// World ticks the frame draws; ages are taken from it.
    pub now: f64,
    /// Seconds per tick.
    pub tick: f32,
    pub births: Births,
    pub bursts: Bursts,
    pub emission: Emission,
}

/// A block of pool slots: its first, and its order (log2 of its slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block {
    offset: u32,
    order: u32,
}

impl Block {
    fn slots(self) -> u32 {
        1 << self.order
    }
}

/// An emitter's ring. `next` takes the next birth and holds the oldest particle; `seen` means a
/// draw of the frame being planned names it.
#[derive(Clone, Copy, Debug)]
struct Ring {
    id: u64,
    block: Block,
    next: u32,
    seen: bool,
}

/// A frame's work for an emitter:
///
/// - `index`: its draw's index among the frame's particles.
/// - `write_at`: where births go, after what a ring made or grown (`rebuilt`) keeps; such a ring
///   writes every slot.
/// - `dropped`: the oldest births not written.
/// - `burst_skip`: how many more of the first burst it skips.
/// - `drawn_from`: the oldest slot once written, drawn first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Planned {
    index: usize,
    block: Block,
    write_at: u32,
    rebuilt: Option<Kept>,
    births: u32,
    dropped: u32,
    bursts: u32,
    burst_skip: u32,
    drawn_from: u32,
}

impl Planned {
    /// Every slot of a ring made or grown, else its births and bursts.
    fn writes(&self) -> u32 {
        match self.rebuilt {
            Some(_) => self.block.slots(),
            None => self.births + self.bursts,
        }
    }
}

/// What a ring made or grown keeps of its old one: the old block, the first kept slot (from its
/// start), and how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kept {
    block: Block,
    first: u32,
    count: u32,
}

impl Kept {
    const NONE: Self = Self {
        block: Block {
            offset: 0,
            order: 0,
        },
        first: 0,
        count: 0,
    };
}

/// The pool a frame writes and draws. `name` indexes [`POOLS`]. `grown_from` is the slots before it
/// grew in the frame; 0 for a new pool, with nothing to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pool {
    slots: u32,
    name: usize,
    grown_from: Option<u32>,
}

/// The rings, planned on the CPU and kept across frames. A buddy allocator: the pool is a power of
/// two slots, cut into power-of-two blocks, one per ring, doubling up to [`MAX_PARTICLES`]. A freed
/// block is free from the next frame, so a ring taking its place can't overwrite what the frame
/// copies out of it.
#[derive(Default)]
struct Rings {
    /// By emitter ID.
    rings: Vec<Ring>,
    /// Offsets of free blocks, per order.
    free: [Vec<u32>; ORDERS],
    /// Freed this frame; free from the next.
    freed: Vec<Block>,
    /// 0 for none.
    slots: u32,
    /// Indexes [`POOLS`].
    name: usize,
    /// The slots before this frame grew it.
    grown_from: Option<u32>,
    /// This frame's work, in draw order.
    planned: Vec<Planned>,
    /// Emitters warned of having no room, not warned again while drawn without it.
    warned: Vec<u64>,
}

impl Rings {
    /// Plans a frame's `particles`. An emitter drawn last frame keeps its ring if it fits;
    /// otherwise its ring is made or grown, keeping its particles, and the pool grows as needed.
    /// Undrawn emitters' rings go. Returns the pool, if any.
    fn plan(&mut self, particles: &[Particles]) -> Option<Pool> {
        let _zone = gg_core::zone!("plan particles");
        for i in 0..self.freed.len() {
            let block = self.freed[i];
            self.release(block);
        }
        self.freed.clear();
        self.planned.clear();
        self.grown_from = None;
        if particles.is_empty() {
            self.forget();
            return None;
        }
        for ring in &mut self.rings {
            ring.seen = false;
        }
        for (index, drawn) in particles.iter().enumerate() {
            if let Some(planned) = self.ring(index, drawn) {
                self.planned.push(planned);
            }
        }
        let freed = &mut self.freed;
        self.rings.retain(|ring| {
            if !ring.seen {
                freed.push(ring.block);
            }
            ring.seen
        });
        self.warned
            .retain(|id| particles.iter().any(|drawn| drawn.id == *id));
        Some(Pool {
            slots: self.slots,
            name: self.name,
            grown_from: self.grown_from,
        })
    }

    /// `None` where `drawn` has no ring: it doesn't fit, or it's its emitter's second draw this
    /// frame.
    fn ring(&mut self, index: usize, drawn: &Particles) -> Option<Planned> {
        let order = ring_slots(drawn.most).trailing_zeros();
        let fits = drawn.most <= MAX_PARTICLES;
        if !fits {
            let why = "more than the pool holds, so it draws the newest that fit";
            self.full(drawn, why);
        }
        let found = self.rings.binary_search_by_key(&drawn.id, |ring| ring.id);
        let (block, kept, write_at) = match found {
            Ok(at) if self.rings[at].seen => return None,
            Ok(at) => {
                let ring = &mut self.rings[at];
                ring.seen = true;
                let ring = *ring;
                let kept = (!drawn.fresh).then_some(Kept {
                    block: ring.block,
                    first: ring.next,
                    count: ring.block.slots(),
                });
                match (order > ring.block.order).then(|| self.allocate(order)) {
                    Some(Some(block)) => {
                        self.freed.push(ring.block);
                        self.rings[at].block = block;
                        if fits {
                            self.warned.retain(|&id| id != drawn.id);
                        }
                        (block, Some(kept.unwrap_or(Kept::NONE)), 0)
                    }
                    grown => {
                        if grown.is_some() {
                            let why = "more room than the pool has left, so it keeps the ring it \
                                       had";
                            self.full(drawn, why);
                        }
                        match kept {
                            Some(_) => (ring.block, None, ring.next),
                            None => (ring.block, Some(Kept::NONE), 0),
                        }
                    }
                }
            }
            Err(at) => {
                let Some(block) = self.allocate(order) else {
                    self.full(
                        drawn,
                        "a ring the pool has no room for, so it draws nothing",
                    );
                    return None;
                };
                let ring = Ring {
                    id: drawn.id,
                    block,
                    next: 0,
                    seen: true,
                };
                self.rings.insert(at, ring);
                if fits {
                    self.warned.retain(|&id| id != drawn.id);
                }
                (block, Some(Kept::NONE), 0)
            }
        };
        let slots = block.slots();
        // Bursts are newest, so the oldest births drop first, then the first bursts.
        let all_bursts = drawn.bursts.particles();
        let bursts = all_bursts.min(u64::from(slots));
        let burst_skip = u64::from(drawn.bursts.skip) + (all_bursts - bursts);
        let bursts = bursts as u32;
        let births = drawn.births.count.min(slots - bursts);
        let written = births + bursts;
        let (rebuilt, drawn_from) = match kept {
            Some(kept) => {
                let count = kept.count.min(slots - written);
                let first = (kept.first + (kept.count - count)) % kept.block.slots();
                let kept = Kept {
                    first,
                    count,
                    ..kept
                };
                (Some(kept), (count + written) % slots)
            }
            None => (None, (write_at + written) % slots),
        };
        let ring = self.rings.binary_search_by_key(&drawn.id, |ring| ring.id);
        self.rings[ring.expect("planned above")].next = drawn_from;
        Some(Planned {
            index,
            block,
            write_at,
            rebuilt,
            births,
            dropped: drawn.births.count - births,
            bursts,
            burst_skip: u32::try_from(burst_skip).unwrap_or(u32::MAX),
            drawn_from,
        })
    }

    /// Warns once that `drawn` doesn't fit.
    fn full(&mut self, drawn: &Particles, why: &str) {
        if self.warned.contains(&drawn.id) {
            return;
        }
        self.warned.push(drawn.id);
        warn!(
            code = "renderer.particles_full",
            emitter = drawn.id,
            most = drawn.most,
            pool = MAX_PARTICLES,
            "Emitter {:#018x}, of up to {} particles alive at once, needs {why}: the pool holds {} \
             for every emitter",
            drawn.id,
            drawn.most,
            MAX_PARTICLES,
        );
    }

    /// Doubles the pool as needed, up to [`MAX_PARTICLES`]; `None` past that.
    fn allocate(&mut self, order: u32) -> Option<Block> {
        loop {
            if let Some(block) = self.take(order) {
                return Some(block);
            }
            let grown = match self.slots {
                0 => LEAST_POOL.max(1 << order),
                slots => slots * 2,
            };
            if grown > MAX_PARTICLES {
                return None;
            }
            // Growing a pool switches its name once, however often it doubles, and copies the old
            // pool.
            if self.grown_from.is_none() {
                self.grown_from = Some(self.slots);
                if self.slots > 0 {
                    self.name ^= 1;
                }
            }
            // The new half, or a new pool, is free.
            let (offset, free) = match self.slots {
                0 => (0, grown),
                slots => (slots, slots),
            };
            self.free[free.trailing_zeros() as usize].push(offset);
            self.slots = grown;
        }
    }

    /// The smallest free block large enough, halved down to `order`, the other halves freed.
    fn take(&mut self, order: u32) -> Option<Block> {
        let found = (order as usize..ORDERS).find(|&k| !self.free[k].is_empty())?;
        let offset = self.free[found].pop()?;
        for k in (order..found as u32).rev() {
            self.free[k as usize].push(offset + (1 << k));
        }
        Some(Block { offset, order })
    }

    /// Frees `block`, merging it with its buddy while both are free.
    fn release(&mut self, mut block: Block) {
        let whole = self.slots.trailing_zeros();
        while block.order < whole {
            let buddy = block.offset ^ block.slots();
            let free = &mut self.free[block.order as usize];
            let Some(at) = free.iter().position(|&offset| offset == buddy) else {
                break;
            };
            free.swap_remove(at);
            block = Block {
                offset: block.offset.min(buddy),
                order: block.order + 1,
            };
        }
        self.free[block.order as usize].push(block.offset);
    }

    /// Forgets every ring and the pool, for a frame that doesn't declare it: the graph lets go of
    /// such a buffer.
    fn forget(&mut self) {
        self.rings.clear();
        self.free.iter_mut().for_each(Vec::clear);
        self.freed.clear();
        self.planned.clear();
        self.slots = 0;
        self.grown_from = None;
    }
}

/// The power of two at or past `most`, from [`LEAST_RING`] to [`MAX_PARTICLES`].
fn ring_slots(most: u32) -> u32 {
    most.clamp(LEAST_RING, MAX_PARTICLES).next_power_of_two()
}

/// `EmitPush` in `shaders/emit.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct EmitPush {
    pool: u64,
    emitter: u64,
    from: u64,
    count: u32,
    first: u32,
    shift: [f32; 3],
    unused: u32,
}

/// `DrawPush` in `shaders/particles.slang`.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct DrawPush {
    clip_from_world: Mat4,
    pool: u64,
    look: u64,
    right: Vec3,
    unused_right: f32,
    up: Vec3,
    unused_up: f32,
    /// The frame's [`FrameData`](crate::mesh::FrameData), whose fog the particles take.
    frame: u64,
}

/// `Look` in `shaders/particle.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct GpuLook {
    color_birth: Vec4,
    color_death: Vec4,
    acceleration: Vec3,
    drag: f32,
    size: [f32; 2],
    id: [u32; 2],
    cells: [u32; 2],
    texture: u32,
    sampler: u32,
    additive: u32,
    offset: u32,
    capacity: u32,
    drawn_from: u32,
    now_tick: u32,
    now_fraction: f32,
    tick_seconds: f32,
    unused: u32,
}

/// `Emission` in `shaders/particle.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct GpuEmission {
    from: Mat4,
    to: Mat4,
    burst_at: Mat4,
    shape: Vec4,
    speed: [f32; 2],
    lifetime: [f32; 2],
    spin: [f32; 2],
    first: [u32; 2],
    births: u32,
    birth_tick: u32,
    birth_fraction: f32,
    every: f32,
    along: f32,
    step: f32,
    bursts: u32,
    burst_skip: u32,
    first_burst: u32,
    burst_size: u32,
    burst_tick: u32,
    burst_fraction: f32,
    shape_kind: u32,
    write_at: u32,
    rebuild: u32,
    kept: u32,
    old_offset: u32,
    old_capacity: u32,
    old_first: u32,
    unused: u32,
}

/// `Emitter` in `shaders/particle.slang`: an emitter's part of a frame, look first.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct GpuEmitter {
    look: GpuLook,
    emission: GpuEmission,
}

/// World ticks as the shaders take them: the low 32 bits of the whole ticks (they wrap alike when
/// subtracted), and the fraction.
fn split(ticks: f64) -> (u32, f32) {
    let whole = ticks.floor();
    // A saturating cast, then the low bits.
    ((whole as u64) as u32, (ticks - whole) as f32)
}

fn gpu_emitter(drawn: &Particles, planned: &Planned) -> GpuEmitter {
    let Particles {
        id,
        now,
        tick,
        births,
        bursts,
        emission,
        ..
    } = drawn;
    let (now_tick, now_fraction) = split(*now);
    let (shape_kind, shape) = match emission.shape {
        ParticleShape::Point => (0, Vec4::ZERO),
        ParticleShape::Sphere { radius } => (1, Vec4::new(radius, 0.0, 0.0, 0.0)),
        ParticleShape::Box { half_extents } => (2, half_extents.extend(0.0)),
        ParticleShape::Cone { angle, radius } => (3, Vec4::new(angle, radius, 0.0, 0.0)),
    };
    let dropped = planned.dropped;
    let first = births.first.wrapping_add(u64::from(dropped));
    let (birth_tick, birth_fraction) = split(births.time + f64::from(dropped) * births.every);
    let (burst_tick, burst_fraction) = split(bursts.time);
    let kept = planned.rebuilt.unwrap_or(Kept::NONE);
    let ring = planned.block;
    GpuEmitter {
        look: GpuLook {
            color_birth: emission.color[0],
            color_death: emission.color[1],
            acceleration: emission.acceleration,
            // NaN or less than 0 is none.
            drag: if emission.drag > 0.0 {
                emission.drag
            } else {
                0.0
            },
            size: emission.size,
            id: crate::halves(*id),
            cells: emission.cells.map(|cells| cells.max(1)),
            texture: emission.texture.map_or(DISC, Texture::index),
            sampler: Sampler::LinearClamp.index(),
            additive: u32::from(emission.additive),
            offset: ring.offset,
            capacity: ring.slots(),
            drawn_from: planned.drawn_from,
            now_tick,
            now_fraction,
            tick_seconds: *tick,
            unused: 0,
        },
        emission: GpuEmission {
            from: births.from,
            to: births.to,
            burst_at: bursts.at,
            shape,
            speed: emission.speed,
            lifetime: emission.lifetime,
            spin: emission.spin,
            first: crate::halves(first),
            births: planned.births,
            birth_tick,
            birth_fraction,
            every: births.every as f32,
            along: births.along + dropped as f32 * births.step,
            step: births.step,
            bursts: planned.bursts,
            burst_skip: planned.burst_skip,
            first_burst: bursts.first,
            burst_size: bursts.size,
            burst_tick,
            burst_fraction,
            shape_kind,
            write_at: planned.write_at,
            rebuild: u32::from(planned.rebuilt.is_some()),
            kept: kept.count,
            old_offset: kept.block.offset,
            old_capacity: kept.block.slots(),
            old_first: kept.first,
            unused: 0,
        },
    }
}

/// What a frame's particles declare in its graph: the pool and the births' pass, plus the old pool
/// and the copy's pass when it grows.
pub(crate) struct ParticlePlan {
    pub(crate) pool: BufferId,
    emit: PassId,
    grow: Option<(BufferId, PassId, u32)>,
    /// The pass moving particles with the origin, by how far, and the pool's slots.
    moved: Option<(PassId, Vec3, u32)>,
}

impl ParticlePlan {
    /// Has `pass` read the pool.
    pub(crate) fn drawn<'g>(&self, pass: RenderPass<'g>) -> RenderPass<'g> {
        pass.read_buffer(self.pool)
    }
}

/// The pool's address, and that of its emitters' frame parts, in planned order.
#[derive(Clone, Copy)]
pub(crate) struct Emitted {
    pool: u64,
    emitters: u64,
}

/// The particle pipelines, rings and plan, kept across frames so a frame allocates nothing once
/// they've grown.
pub(crate) struct ParticlePass {
    emit: ComputePipeline,
    grow: ComputePipeline,
    moved: ComputePipeline,
    draw: Pipeline,
    rings: Rings,
    emitters: Vec<GpuEmitter>,
    /// How far the origin moved since particles were last drawn. The next frame drawing them moves
    /// them by it.
    shift: Vec3,
}

impl ParticlePass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        let draw = Pipeline::graphics(
            device,
            &GraphicsDesc {
                name: "particles",
                spirv: SPIRV,
                vertex: c"vertex_main",
                mesh: None,
                fragment: Some(c"fragment_main"),
                format: Format::HDR.with_ids().with_motion().with_depth(),
                // Premultiplied alpha; writing alpha 0 adds light. They test depth, but write
                // neither it nor motion.
                blend: Blend::PremultipliedAlpha,
                depth_write: false,
                ids_write: true,
                depth_bias: 0.0,
                depth_clamp: false,
            },
        )?;
        Ok(Self {
            emit: ComputePipeline::new(device, "particle emission", EMIT, c"emit_main")?,
            grow: ComputePipeline::new(device, "particle pool growth", EMIT, c"grow_main")?,
            moved: ComputePipeline::new(device, "particle pool moved", EMIT, c"moved_main")?,
            draw,
            rings: Rings::default(),
            emitters: Vec::new(),
            shift: Vec3::ZERO,
        })
    }

    /// Moves live particles into a frame whose origin is `by` further on, before the next births:
    /// `p` becomes `p - by`, as [`RenderData::origin`](crate::RenderData::origin) says.
    pub(crate) fn moved(&mut self, by: Vec3) {
        self.shift += by;
    }

    /// Declares in `graph` what `draws`' particles take, if any: the pool, the copy into a grown
    /// pool, and the births' pass, on the graphics queue, before the drawing passes.
    pub(crate) fn plan(&mut self, graph: &mut Graph, draws: &Draws) -> Option<ParticlePlan> {
        // A frame without particles lets them go, and forgets the shift.
        let shift = std::mem::replace(&mut self.shift, Vec3::ZERO);
        let pool = self.rings.plan(&draws.particles)?;
        let bytes = |slots: u32| u64::from(slots) * RECORD;
        let from = (pool.grown_from.filter(|&slots| slots > 0))
            .map(|slots| (graph.buffer(POOLS[pool.name ^ 1], bytes(slots)), slots));
        let buffer = graph.buffer(POOLS[pool.name], bytes(pool.slots));
        // The graph has a pass read what no earlier pass wrote by declaring a write.
        let grow = from.map(|(from, slots)| {
            let grow = graph.compute(label!("particle pool growth"));
            let pass = grow.write_buffer(from).write_buffer(buffer).add();
            (from, pass, slots)
        });
        let moved = (shift != Vec3::ZERO).then(|| {
            let moved = graph.compute(label!("particles moved"));
            (moved.write_buffer(buffer).add(), shift, pool.slots)
        });
        let emit = (graph.compute(label!("particles")))
            .write_buffer(buffer)
            .add();
        Some(ParticlePlan {
            pool: buffer,
            emit,
            grow,
            moved,
        })
    }

    /// Forgets every ring, for a frame that doesn't render the view, or renders nothing. The next
    /// particles start in new rings.
    pub(crate) fn forget(&mut self) {
        self.rings.forget();
    }

    /// Records `plan`'s passes: the grown pool's copy, then each ring's births.
    pub(crate) fn record(
        &mut self,
        frame: &mut Frame<'_>,
        plan: &ParticlePlan,
        draws: &Draws,
    ) -> Result<Emitted> {
        let _zone = gg_core::zone!("emit particles");
        self.emitters.clear();
        let planned = &self.rings.planned;
        (self.emitters).extend(
            (planned.iter()).map(|planned| gpu_emitter(&draws.particles[planned.index], planned)),
        );
        if let Some((from, pass, slots)) = plan.grow {
            let mut pass = frame.compute(pass);
            let (from, pool) = (pass.address(from), pass.address(plan.pool));
            pass.set_pipeline(&self.grow);
            let words = slots * (RECORD / 4) as u32;
            let mut first = 0;
            while first < words {
                let groups = (words - first).div_ceil(GROUP).min(MOST_GROUPS);
                let push = EmitPush {
                    pool,
                    emitter: 0,
                    from,
                    count: words,
                    first,
                    shift: [0.0; 3],
                    unused: 0,
                };
                pass.push_constants(bytemuck::bytes_of(&push));
                pass.dispatch([groups, 1, 1]);
                first += groups * GROUP;
            }
        }
        if let Some((pass, by, slots)) = plan.moved {
            let mut pass = frame.compute(pass);
            let pool = pass.address(plan.pool);
            pass.set_pipeline(&self.moved);
            let mut first = 0;
            while first < slots {
                let groups = (slots - first).div_ceil(GROUP).min(MOST_GROUPS);
                let push = EmitPush {
                    pool,
                    emitter: 0,
                    from: 0,
                    count: slots,
                    first,
                    shift: by.to_array(),
                    unused: 0,
                };
                pass.push_constants(bytemuck::bytes_of(&push));
                pass.dispatch([groups, 1, 1]);
                first += groups * GROUP;
            }
        }
        let mut pass = frame.compute(plan.emit);
        let pool = pass.address(plan.pool);
        let emitters = match self.emitters.as_slice() {
            [] => 0,
            emitters => pass.upload(bytemuck::cast_slice(emitters))?.address(),
        };
        pass.set_pipeline(&self.emit);
        for (i, planned) in self.rings.planned.iter().enumerate() {
            let count = planned.writes();
            if count == 0 {
                continue;
            }
            let push = EmitPush {
                pool,
                emitter: emitters + (i * size_of::<GpuEmitter>()) as u64,
                from: 0,
                count,
                first: 0,
                shift: [0.0; 3],
                unused: 0,
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            // At most `MAX_PARTICLES` slots: 16,384 workgroups.
            pass.dispatch([count.div_ceil(GROUP), 1, 1]);
        }
        Ok(Emitted { pool, emitters })
    }

    /// For the view's passes, once [`record`](Self::record) has written the particles where
    /// `emitted` says. They face the view along `right` and `up`.
    pub(crate) fn draws(&self, emitted: Emitted, right: Vec3, up: Vec3) -> ParticleDraws<'_> {
        ParticleDraws {
            pipeline: &self.draw,
            planned: &self.rings.planned,
            emitted,
            right,
            up,
        }
    }
}

/// What `MeshPass::draw` draws each emitter's particles by, in its place among the frame's draws.
#[derive(Clone, Copy)]
pub(crate) struct ParticleDraws<'a> {
    pipeline: &'a Pipeline,
    planned: &'a [Planned],
    emitted: Emitted,
    right: Vec3,
    up: Vec3,
}

impl ParticleDraws<'_> {
    /// Draws the particles of the frame's draw `index`, if they have a ring: an instance of the
    /// quad (indices `quad`) per slot, fogged by the frame data at `frame`. Returns what it drew.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        quad: &GpuBuffer,
        index: usize,
        clip_from_world: Mat4,
        frame: u64,
    ) -> DrawCounts {
        let Ok(at) = (self.planned).binary_search_by_key(&index, |planned| planned.index) else {
            return DrawCounts::default();
        };
        let slots = self.planned[at].block.slots();
        pass.set_pipeline(self.pipeline);
        pass.set_faces(Faces::Both);
        let push = DrawPush {
            clip_from_world,
            pool: self.emitted.pool,
            look: self.emitted.emitters + (at * size_of::<GpuEmitter>()) as u64,
            right: self.right,
            unused_right: 0.0,
            up: self.up,
            unused_up: 0.0,
            frame,
        };
        pass.push_constants(bytemuck::bytes_of(&push));
        pass.draw_indexed(quad, 0, 6, 0, slots);
        DrawCounts {
            draws: 1,
            instances: slots as usize,
            triangles: 2 * slots as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn particles(id: u64, most: u32, births: u32, bursts: u32) -> Particles {
        Particles {
            id,
            fresh: false,
            most,
            now: 0.0,
            tick: 1.0 / 64.0,
            births: Births {
                count: births,
                ..Births::none(Mat4::IDENTITY)
            },
            bursts: Bursts {
                count: bursts.min(1),
                size: bursts,
                ..Bursts::none(Mat4::IDENTITY)
            },
            emission: Emission {
                shape: ParticleShape::Point,
                speed: [1.0; 2],
                lifetime: [1.0; 2],
                spin: [0.0; 2],
                acceleration: Vec3::ZERO,
                drag: 0.0,
                size: [0.1; 2],
                color: [Vec4::ONE; 2],
                texture: None,
                cells: [1, 1],
                additive: false,
            },
        }
    }

    /// Each planned emitter's draw index, block, kept particles and oldest slot.
    fn planned(rings: &Rings) -> Vec<(usize, Block, Option<Kept>, u32)> {
        let planned = |planned: &Planned| {
            let Planned {
                index,
                block,
                rebuilt,
                drawn_from,
                ..
            } = *planned;
            (index, block, rebuilt, drawn_from)
        };
        rings.planned.iter().map(planned).collect()
    }

    /// A new ring is made whole: the least slots, or the power of two at or past its most. Next
    /// frame's births follow the last's round the ring, oldest drawn first. Births past its room
    /// drop the oldest, before any burst. An emitter needing more grows its ring, keeping its
    /// particles oldest first (the newest, where births leave too little room). The pool doubles
    /// from its least.
    #[test]
    fn rings_hold_their_emitters_particles_from_frame_to_frame() {
        let mut rings = Rings::default();
        let pool = rings.plan(&[particles(7, 10, 3, 0), particles(9, 100, 0, 0)]);
        let made = Some(Kept::NONE);
        let first = Block {
            offset: 0,
            order: 6,
        };
        let second = Block {
            offset: 128,
            order: 7,
        };
        assert_eq!(planned(&rings), [(0, first, made, 3), (1, second, made, 0)]);
        let made_now = Some(0);
        let least = Pool {
            slots: LEAST_POOL,
            name: 0,
            grown_from: made_now,
        };
        assert_eq!(pool, Some(least));
        // 70 births into 64 slots, from slot 3: the oldest 6 dropped, ending where they started.
        rings.plan(&[particles(7, 10, 70, 0)]);
        let births = rings.planned[0];
        assert_eq!((births.births, births.dropped, births.write_at), (64, 6, 3));
        assert_eq!((births.rebuilt, births.drawn_from), (None, 3));
        // Bursts go before births, which then drop.
        rings.plan(&[particles(7, 10, 5, 62)]);
        let both = rings.planned[0];
        assert_eq!((both.births, both.bursts, both.dropped), (2, 62, 3));
        // Grown to 256 slots for 200 with 150 births: the 64 kept go first, oldest first from slot
        // 3, in a block past the first's, which is free again only from the next frame.
        rings.plan(&[particles(7, 200, 150, 0)]);
        let grown = Block {
            offset: 256,
            order: 8,
        };
        let kept = Kept {
            block: first,
            first: 3,
            count: 64,
        };
        assert_eq!(planned(&rings), [(0, grown, Some(kept), 214)]);
        assert_eq!(rings.freed, [first]);
        // A fresh one keeps nothing, in the same ring.
        let fresh = Particles {
            fresh: true,
            ..particles(7, 200, 1, 0)
        };
        rings.plan(&[fresh]);
        assert_eq!(planned(&rings), [(0, grown, made, 1)]);
        // More births than the ring had kept: only the newest of them kept.
        rings.plan(&[particles(7, 200, 0, 0)]);
        rings.plan(&[particles(7, 400, 400, 0)]);
        let regrown = rings.planned[0];
        assert_eq!(regrown.block.slots(), 512);
        let kept = regrown.rebuilt.expect("grown");
        assert_eq!((kept.count, kept.first, regrown.drawn_from), (112, 145, 0));
        // Past the pool's room: it doubles twice, switching its name once and copying the old pool.
        let pool = rings.plan(&[particles(7, 400, 0, 0), particles(8, 5000, 0, 0)]);
        let doubled = Pool {
            slots: 4 * LEAST_POOL,
            name: 1,
            grown_from: Some(LEAST_POOL),
        };
        assert_eq!(pool, Some(doubled));
        let pool = rings.plan(&[particles(7, 400, 0, 0), particles(8, 5000, 0, 0)]);
        let kept = Pool {
            grown_from: None,
            ..doubled
        };
        assert_eq!(pool, Some(kept));
    }

    /// An emitter that doesn't fit beside the rest draws nothing, warned once while drawn without
    /// room. It gets a ring the frame after unused rings are freed. One larger than the pool warns
    /// too, and draws what fits.
    #[test]
    fn the_pool_holds_its_most_and_no_more() {
        let mut rings = Rings::default();
        let half = MAX_PARTICLES / 2;
        let both = [particles(1, half, 0, 0), particles(2, half, 0, 0)];
        let pool = rings.plan(&both).expect("a pool");
        assert_eq!((pool.slots, rings.planned.len()), (MAX_PARTICLES, 2));
        let three = [both[0], both[1], particles(3, 64, 0, 0)];
        rings.plan(&three);
        assert_eq!(rings.planned.len(), 2, "the third has no ring");
        assert_eq!(rings.warned, [3]);
        rings.plan(&three);
        assert_eq!(rings.warned, [3], "once");
        // The second goes; its ring is free from the frame after.
        let first_and_third = [both[0], three[2]];
        rings.plan(&first_and_third);
        assert_eq!(rings.planned.len(), 1);
        rings.plan(&first_and_third);
        let indices: Vec<_> = rings.planned.iter().map(|planned| planned.index).collect();
        assert_eq!(indices, [0, 1]);
        assert!(rings.warned.is_empty(), "it fits");
        // Larger than the pool: warned of, drawing what fits once the rest are gone.
        let huge = [particles(4, MAX_PARTICLES + 1, 0, 0)];
        rings.plan(&huge);
        assert_eq!(rings.warned, [4]);
        rings.plan(&huge);
        assert_eq!(rings.planned.len(), 1);
        assert_eq!(rings.planned[0].block.slots(), MAX_PARTICLES);
        // None drawn: the pool is let go of, every block free.
        assert_eq!(rings.plan(&[]), None);
        assert_eq!((rings.slots, rings.rings.len()), (0, 0));
    }

    /// A pool cut fine by small rings has its whole room for a large one once they're gone. In the
    /// frame they go, a large ring doubles the pool, as their blocks are free only from the next
    /// frame; one in the frame after takes their merged room.
    #[test]
    fn freed_blocks_join_their_buddies() {
        let mut rings = Rings::default();
        let small: Vec<_> = (0..64).map(|id| particles(id, 64, 0, 0)).collect();
        rings.plan(&small);
        assert_eq!(rings.slots, LEAST_POOL);
        let large = particles(100, LEAST_POOL, 0, 0);
        rings.plan(&[large]);
        assert_eq!(rings.slots, 2 * LEAST_POOL);
        rings.plan(&[large]);
        let free = |rings: &Rings| {
            (rings.free.iter().enumerate())
                .flat_map(|(order, free)| free.iter().map(move |&offset| (offset, order)))
                .collect::<Vec<_>>()
        };
        let order = LEAST_POOL.trailing_zeros() as usize;
        assert_eq!(free(&rings), [(0, order)]);
        rings.plan(&[large, particles(101, LEAST_POOL, 0, 0)]);
        assert_eq!(rings.slots, 2 * LEAST_POOL, "no larger");
        assert_eq!(rings.planned[1].block.offset, 0);
        assert!(free(&rings).is_empty());
    }
}
