//! Particles: the particles of the frame's emitters, which belong to the frames rather than the
//! world. Each particle is its birth record, in a pool on the GPU that lasts from frame to frame,
//! each emitter's in a ring of its own there, from which its age alone gives where it is and how it
//! looks, as `shaders/particles.slang` draws it. So a frame writes only its emitters' births, in a
//! compute pass before the view's passes, as `shaders/emit.slang` writes them, and draws every slot
//! of each emitter's ring, among the frame's draws that blend, in the emitter's place.

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

/// The most particles the pool holds, all its emitters' together: 2^20, 32 MiB of birth records.
pub const MAX_PARTICLES: u32 = 1 << 20;

/// The fewest slots a ring has, so that emitters of a few particles don't cut the pool fine.
const LEAST_RING: u32 = 64;

/// The fewest slots the pool has, once it has any.
const LEAST_POOL: u32 = 1 << 12;

/// The orders of the pool's blocks, the powers of two of their slots, from a slot to the whole
/// pool at its largest.
const ORDERS: usize = MAX_PARTICLES.trailing_zeros() as usize + 1;

/// The bytes of a birth record, `Particle` in `shaders/particle.slang`.
const RECORD: u64 = 32;

/// The threads of a workgroup of `shaders/emit.slang`, a slot or a word each.
const GROUP: u32 = 64;

/// The most workgroups a dispatch takes across, as every device takes: a pool of more words grows
/// in several dispatches.
const MOST_GROUPS: u32 = 65_535;

/// `DISC` in `shaders/particle.slang`: a soft disc in place of a texture.
const DISC: u32 = u32::MAX;

/// The pool's two names, which it takes by turns as it grows, so that the frame that grows it
/// declares both, and copies the one into the other.
const POOLS: [&str; 2] = ["particle pool a", "particle pool b"];

/// Where an emitter's particles are born, in its own space, and which way they go: an emitter
/// without a rotation sends them up, along its +Y, as a fountain and a fire do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleShape {
    /// At its origin, any way.
    Point,
    /// Anywhere within `radius` of its origin, away from it.
    Sphere { radius: f32 },
    /// Anywhere in the box of `half_extents` round its origin, along its +Y.
    Box { half_extents: Vec3 },
    /// On the disc of `radius` round its origin, in its XZ plane, any way within `angle` of its +Y,
    /// in radians: the cone's half angle.
    Cone { angle: f32, radius: f32 },
}

/// How an emitter's particles are born, move and look: the parts of a [`Particles`] that are the
/// emitter's own, rather than its frame's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emission {
    pub shape: ParticleShape,
    /// The least and the most speed a particle is born with, along the way its shape gives it, in
    /// the world's units a second, whatever the emitter's scale: each takes a share between them at
    /// random, as it does of its lifetime and its spin.
    pub speed: [f32; 2],
    /// The least and the most seconds a particle lives; one of no more than 0 is never drawn.
    pub lifetime: [f32; 2],
    /// The least and the most a particle turns a second, in radians, counterclockwise as seen.
    pub spin: [f32; 2],
    /// What every particle accelerates by, in the world's units a second squared: gravity, or a
    /// fire's rising.
    pub acceleration: Vec3,
    /// The share of its velocity a particle loses a second, by linear drag, of dv/dt = acceleration
    /// - drag * v: 0 for none. Less than 0 is 0.
    pub drag: f32,
    /// A particle's size across, in the world, at its birth and at its death, as it goes from one
    /// to the other over its life.
    pub size: [f32; 2],
    /// A particle's color at its birth and at its death, linear RGBA, alpha not multiplied in, as it
    /// goes from one to the other over its life: past 1 for light, as fire's.
    pub color: [Vec4; 2],
    /// What a particle shows, tinted by its color: the texture, sampled as stored by
    /// [`Sampler::LinearClamp`], or `None` for a soft disc, brightest at its middle.
    pub texture: Option<Texture>,
    /// The cells across and down the texture is cut into, evenly, which a particle plays through,
    /// row by row from the top left, over its life: `[1, 1]` for the whole texture. A side of 0 is
    /// 1.
    pub cells: [u32; 2],
    /// Whether particles add their light to what's behind them, as fire and sparks do, rather than
    /// going over it by their alpha.
    pub additive: bool,
}

/// An emitter's births in a frame: `count` particles, numbered on from `first` among the emitter's,
/// the first born at `time`, of the world's time, in ticks, each next `every` ticks after, and each
/// where the emitter was at its birth: the first `along` of the way from `from`, the emitter's
/// transform the frame before, to `to`, its transform this frame, each next `step` further.
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
    /// None, of an emitter that's at `at`.
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

/// An emitter's bursts in a frame: `count` bursts of `size` particles each, numbered on from `first`,
/// but for the first `skip` particles of the first, all born at `time`, of the world's time, in
/// ticks, where `at` places the emitter.
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
    /// None, of an emitter that's at `at`.
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

    /// How many particles they are.
    pub fn particles(&self) -> u64 {
        (u64::from(self.count) * u64::from(self.size)).saturating_sub(u64::from(self.skip))
    }
}

/// An emitter's particles in a frame, which the frame draws where [`Draws::push_particles`] puts
/// them: plain data, which the engine gives for each emitter each frame, of the world's time as the
/// frame draws it. The renderer keeps the emitter's particles from frame to frame by its `id`, in a
/// ring of slots of its own, as long as each frame draws it; each particle's randomness is a hash
/// of `id` and its number, or its burst's and its own within the burst. Its particles write `id`
/// where they show, which picks read back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particles {
    pub id: u64,
    /// Whether its ring starts again empty this frame, as the engine starts an emitter again.
    pub fresh: bool,
    /// The most particles it has alive at once, which its ring holds: its births a second times
    /// their most lifetime, and a burst. A frame's births past what its ring holds, the oldest,
    /// aren't drawn. One of more than [`MAX_PARTICLES`] draws the newest of them alone, warning
    /// (`renderer.particles_full`).
    pub most: u32,
    /// The world's time the frame draws, in ticks, from which each particle's age is taken.
    pub now: f64,
    /// The seconds a tick takes.
    pub tick: f32,
    pub births: Births,
    pub bursts: Bursts,
    pub emission: Emission,
}

/// A block of the pool's slots: the first, and its order, the power of two of the slots it has.
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

/// An emitter's ring: its emitter's ID, its block of slots, the slot its next birth goes to, which
/// holds its oldest particle, and whether a draw of the frame being planned names it.
#[derive(Clone, Copy, Debug)]
struct Ring {
    id: u64,
    block: Block,
    next: u32,
    seen: bool,
}

/// What a frame writes and draws of an emitter: its draw's index among the frame's particles, and
/// its ring's block; where its births go, after the particles it keeps of the ring it had, as a ring
/// made or grown starts, which writes every slot; how many births it writes, after the oldest it
/// drops, and burst particles, and how many more of those it skips, the first; and the slot that
/// holds its oldest particle once they're written, which draws first.
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
    /// The slots it writes: every slot of a ring made or grown, or its births.
    fn writes(&self) -> u32 {
        match self.rebuilt {
            Some(_) => self.block.slots(),
            None => self.births + self.bursts,
        }
    }
}

/// The particles a ring made or grown keeps of the ring it had: that ring's block, its slot, from
/// its first, that holds the first kept, and how many it keeps, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kept {
    block: Block,
    first: u32,
    count: u32,
}

impl Kept {
    /// None.
    const NONE: Self = Self {
        block: Block {
            offset: 0,
            order: 0,
        },
        first: 0,
        count: 0,
    };
}

/// The pool a frame writes and draws: its slots, which of [`POOLS`] names it, and, if it grew in the
/// frame, the slots it had before, 0 for a pool made in the frame, which holds nothing to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pool {
    slots: u32,
    name: usize,
    grown_from: Option<u32>,
}

/// The pool's slots and the emitters' rings in it, planned on the CPU, kept from frame to frame:
/// the pool, of a power of two slots, cut into blocks of powers of two, a buddy allocator's, each
/// ring a block, the pool doubling as rings need more room, to [`MAX_PARTICLES`]. A block freed in a
/// frame is free from the next, so that what a frame copies out of a ring it lets go of isn't
/// written in the same pass by a ring that takes its place.
#[derive(Default)]
struct Rings {
    /// By their emitters' IDs.
    rings: Vec<Ring>,
    /// The offsets of the free blocks of each order.
    free: [Vec<u32>; ORDERS],
    /// The blocks let go of in the frame being planned, free from the next.
    freed: Vec<Block>,
    /// The pool's slots, 0 for none.
    slots: u32,
    /// Which of [`POOLS`] names it.
    name: usize,
    /// The slots it had before the frame being planned grew it, if it did.
    grown_from: Option<u32>,
    /// What the frame being planned writes and draws, in the order of its draws.
    planned: Vec<Planned>,
    /// The emitters warned of having no room, which aren't again while they're drawn without it.
    warned: Vec<u64>,
}

impl Rings {
    /// Plans a frame of `particles`, the frame's draws': each emitter's ring, the one it had, if
    /// it's in the frame before's draws and still has room, made, as a new emitter's is, or grown,
    /// keeping its particles, as one of more particles needs, the pool growing as they need; the
    /// rings of emitters it doesn't draw let go of. Returns the pool, if it has any particles.
    fn plan(&mut self, particles: &[Particles]) -> Option<Pool> {
        let _zone = gg_core::zone!("plan particles");
        // By index, keeping the list's memory.
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

    /// Plans `drawn`, the frame's draw of `index`: `None` where it has no ring, as one that doesn't
    /// fit, or an emitter drawn twice in the frame, whose second draw draws nothing.
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
        // The bursts, born after the births, are the newest, so the births are dropped first, the
        // oldest; and of bursts the ring can't hold, the first.
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

    /// Warns once that `drawn` doesn't fit, as `why` says.
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

    /// A block of `order`, the pool doubled as it must be to have one, up to [`MAX_PARTICLES`];
    /// `None` past that.
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
            // A frame that grows a pool it had takes the other name for it, once, however many
            // times it doubles it, and copies the pool it had into it.
            if self.grown_from.is_none() {
                self.grown_from = Some(self.slots);
                if self.slots > 0 {
                    self.name ^= 1;
                }
            }
            // The pool's new half, or the whole of a pool made now, a block free.
            let (offset, free) = match self.slots {
                0 => (0, grown),
                slots => (slots, slots),
            };
            self.free[free.trailing_zeros() as usize].push(offset);
            self.slots = grown;
        }
    }

    /// A free block of `order`, the smallest free that's large enough cut down to it, its other
    /// halves free.
    fn take(&mut self, order: u32) -> Option<Block> {
        let found = (order as usize..ORDERS).find(|&k| !self.free[k].is_empty())?;
        let offset = self.free[found].pop()?;
        for k in (order..found as u32).rev() {
            self.free[k as usize].push(offset + (1 << k));
        }
        Some(Block { offset, order })
    }

    /// Frees `block`, joining it with its buddy, and theirs with its own, while they're free.
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

    /// Forgets every ring, and the pool, as a frame that doesn't write it leaves it: the graph lets
    /// go of a buffer a frame doesn't declare.
    fn forget(&mut self) {
        self.rings.clear();
        self.free.iter_mut().for_each(Vec::clear);
        self.freed.clear();
        self.planned.clear();
        self.slots = 0;
        self.grown_from = None;
    }
}

/// The slots of a ring for `most` particles: the power of two at or past it, from [`LEAST_RING`] to
/// [`MAX_PARTICLES`].
fn ring_slots(most: u32) -> u32 {
    most.clamp(LEAST_RING, MAX_PARTICLES).next_power_of_two()
}

/// What `shaders/emit.slang` pushes: `EmitPush` there.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct EmitPush {
    pool: u64,
    emitter: u64,
    from: u64,
    count: u32,
    first: u32,
}

/// What `shaders/particles.slang` pushes: `DrawPush` there.
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
    /// The address of the frame's data, a [`FrameData`](crate::mesh::FrameData), whose fog the
    /// particles are fogged by.
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

/// `Emitter` in `shaders/particle.slang`: an emitter's part of a frame, its look first.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct GpuEmitter {
    look: GpuLook,
    emission: GpuEmission,
}

/// `ticks`, of the world's time, as the shaders take a time: the low 32 bits of its whole ticks,
/// which wrap alike as the shaders subtract them, and how far it is into the next.
fn split(ticks: f64) -> (u32, f32) {
    let whole = ticks.floor();
    // A saturating cast, then the low bits.
    ((whole as u64) as u32, (ticks - whole) as f32)
}

/// What the shaders read of `drawn`'s frame, as `planned` writes and draws it.
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
    // The births it writes, after the oldest it drops.
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
            // What isn't a number, or is less than 0, is none.
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

/// What a frame's particles take in its graph: the pool, the pass that writes their births, and, in
/// a frame that grows the pool, the pool before and the pass that copies it.
pub(crate) struct ParticlePlan {
    pub(crate) pool: BufferId,
    emit: PassId,
    grow: Option<(BufferId, PassId, u32)>,
}

impl ParticlePlan {
    /// `pass`, reading the pool, as a pass that draws the frame's particles does.
    pub(crate) fn drawn<'g>(&self, pass: RenderPass<'g>) -> RenderPass<'g> {
        pass.read_buffer(self.pool)
    }
}

/// Where a frame's particles are: the pool's address, and that of its emitters' parts of the frame,
/// one after another, in the order they're planned.
#[derive(Clone, Copy)]
pub(crate) struct Emitted {
    pool: u64,
    emitters: u64,
}

/// The particle passes' pipelines, the pool's rings, and what a frame planned, kept from frame to
/// frame so that a frame allocates nothing once they've grown.
pub(crate) struct ParticlePass {
    emit: ComputePipeline,
    grow: ComputePipeline,
    draw: Pipeline,
    rings: Rings,
    /// Each planned emitter's part of the frame, as the shaders read it.
    emitters: Vec<GpuEmitter>,
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
                // Over what's behind them by their alpha, premultiplied, which writing no alpha
                // makes adding their light; testing depth without writing it or motion.
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
            draw,
            rings: Rings::default(),
            emitters: Vec::new(),
        })
    }

    /// Declares in `graph` what `draws`' particles take, if it has any, as [`Rings::plan`] plans
    /// their rings: the pool, the pass that copies it into the larger one that takes its place, in
    /// a frame that grows it, and the pass that writes the births, on the graphics queue, before
    /// the passes that draw.
    pub(crate) fn plan(&mut self, graph: &mut Graph, draws: &Draws) -> Option<ParticlePlan> {
        let pool = self.rings.plan(&draws.particles)?;
        let bytes = |slots: u32| u64::from(slots) * RECORD;
        let from = (pool.grown_from.filter(|&slots| slots > 0))
            .map(|slots| (graph.buffer(POOLS[pool.name ^ 1], bytes(slots)), slots));
        let buffer = graph.buffer(POOLS[pool.name], bytes(pool.slots));
        // What no pass of the frame wrote is read as a pass that writes it, as the graph has it.
        let grow = from.map(|(from, slots)| {
            let grow = graph.compute(label!("particle pool growth"));
            let pass = grow.write_buffer(from).write_buffer(buffer).add();
            (from, pass, slots)
        });
        let emit = (graph.compute(label!("particles")))
            .write_buffer(buffer)
            .add();
        Some(ParticlePlan {
            pool: buffer,
            emit,
            grow,
        })
    }

    /// Forgets every ring, as a frame that doesn't render the view, or renders nothing, leaves
    /// them: the next frame's particles start in new rings, without the particles before.
    pub(crate) fn forget(&mut self) {
        self.rings.forget();
    }

    /// Records `plan`'s passes, the next of the frame's: the pool copied into the one that takes
    /// its place, in a frame that grows it, and the births written into each ring, of `draws`'
    /// particles.
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
            };
            pass.push_constants(bytemuck::bytes_of(&push));
            // At most `MAX_PARTICLES` slots, of 16,384 workgroups.
            pass.dispatch([count.div_ceil(GROUP), 1, 1]);
        }
        Ok(Emitted { pool, emitters })
    }

    /// What the view's passes draw the frame's particles by, once [`record`](Self::record) has
    /// written them where `emitted` says, each facing the view along `right` and `up`.
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

/// What the view's passes draw a frame's particles by, as `MeshPass::draw` draws each emitter's in
/// its place among the frame's draws.
#[derive(Clone, Copy)]
pub(crate) struct ParticleDraws<'a> {
    pipeline: &'a Pipeline,
    planned: &'a [Planned],
    emitted: Emitted,
    right: Vec3,
    up: Vec3,
}

impl ParticleDraws<'_> {
    /// Draws the frame's particles of `index` among its draws', every slot of their ring an
    /// instance of the quad, whose indices are `quad`, as `clip_from_world` takes the world to clip
    /// space, if they have a ring, fogged by the fog of the frame's data at the address `frame`.
    /// Returns what it drew.
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

    /// An emitter's particles of up to `most` alive at once, with `births` births and `bursts`
    /// burst particles this frame.
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

    /// Each planned emitter's ID's draw index, block, kept particles and oldest slot.
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

    /// A new emitter's ring is made, every slot written, of the least slots or the power of two at
    /// or past its most particles; the next frame writes its births after the last's, round its
    /// ring, its oldest drawing first; a frame's births past its ring's room drop the oldest, before
    /// any burst; and an emitter that needs more grows its ring, keeping its particles, oldest
    /// first, the newest of them where the births leave too little room, while the pool grows from
    /// its least by doubling.
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
        // Grown to 256 slots for 200, of 150 births: its 64 kept, oldest first, from its slot 3,
        // before them, in a block past the first's, which is free again only from the next frame.
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
        // One started afresh keeps nothing, in its ring as it was.
        let fresh = Particles {
            fresh: true,
            ..particles(7, 200, 1, 0)
        };
        rings.plan(&[fresh]);
        assert_eq!(planned(&rings), [(0, grown, made, 1)]);
        // More births than the ring had kept, of the newest of them alone.
        rings.plan(&[particles(7, 200, 0, 0)]);
        rings.plan(&[particles(7, 400, 400, 0)]);
        let regrown = rings.planned[0];
        assert_eq!(regrown.block.slots(), 512);
        let kept = regrown.rebuilt.expect("grown");
        assert_eq!((kept.count, kept.first, regrown.drawn_from), (112, 145, 0));
        // A ring past what the pool has doubles it, and doubles it again, the pool taking the other
        // name once, and copying the pool it had into it.
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

    /// The pool holds `MAX_PARTICLES` across its rings: an emitter that doesn't fit beside the
    /// rest draws nothing, warned of once while it's drawn without room, and has a ring once the
    /// rings of emitters no longer drawn are free, the frame after; one of more than the pool holds
    /// is warned of too, and draws what fits.
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
        // More than the pool holds: warned of, drawing what fits once the rest are gone.
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

    /// The buddies of blocks freed join again, so that a pool cut fine by many small rings has its
    /// whole room for a large one once they're gone: a large ring in the frame they go doubles the
    /// pool, as what they free is free only from the next frame, and another in the frame after
    /// takes their room, joined whole.
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
