//! Particles: emitters, which are components of the world, and their particles, which belong to the
//! frames. Each frame emits what the world's time owes each emitter since the frame before;
//! `gg-renderer` writes and draws it. So no hash, snapshot, scene, recording or rollback holds a
//! particle, as none holds the frames' copies of where entities were.

use std::collections::HashMap;

use gg_core::math::{DVec3, Mat4, Vec3, Vec4};
use gg_renderer::{Births, Bursts, Emission, MAX_PARTICLES, ParticleShape, Particles};

use crate::{Component, EntityId, Reflect, Texture, Tick};

/// Where an emitter's particles are born, in its entity's space, so they turn and scale with it,
/// and which way they go. Without a rotation they go up, along +Y, like a fountain or fire.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum EmitterShape {
    /// At its entity's origin, any way.
    Point,
    /// Anywhere within `radius` of its origin, away from it.
    Sphere { radius: f32 },
    /// Anywhere in the box of `half_extents` round its origin, along +Y.
    Box { half_extents: Vec3 },
    /// On the disc of `radius` round its origin, in its XZ plane, any way within `angle` of +Y:
    /// the cone's half angle, in radians.
    Cone {
        #[reflect(degrees)]
        angle: f32,
        radius: f32,
    },
}

impl Default for EmitterShape {
    /// A cone 15° about +Y, from a disc a tenth of a unit across.
    fn default() -> Self {
        Self::Cone {
            angle: gg_core::math::to_radians(15.0),
            radius: 0.05,
        }
    }
}

/// How an emitter's particles go over what's behind them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum ParticleBlend {
    /// By their alpha, like smoke, dust and water.
    #[default]
    Alpha,
    /// Adding their light, by their alpha, like fire, sparks and glows: a color past 1 is brighter.
    Additive,
}

/// Emits particles from where its entity draws (ancestors carry it).
///
/// Only the emitter is in the world, as an ordinary component: saved in scenes, editable, read by
/// the world hash. The frames own the particles.
///
/// - Births: `rate` a second, evenly. Particle `n` is born `n / rate` seconds of world time after
///   the emitter first draws, at its position then, interpolated between the frames around it.
/// - Bursts: `burst` particles at once each time a tick raises `bursts`.
/// - Motion is a closed form of age, so frames between birth and now don't matter: birth velocity
///   under `acceleration` and `drag`, and size, color, turn and cell by age over lifetime.
/// - World time stops while the simulation is paused or stopped, and so do particles. Headless
///   frames draw the same particles every run.
/// - Each particle's randomness hashes its emitter's ID and its number.
///
/// A frame that doesn't draw the emitter (its entity is gone) lets go of its particles. So does
/// world time going back more than a tick, as the tool API's `stop` does; births then restart.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct ParticleEmitter {
    /// Particles born a second: 0 for bursts alone.
    #[reflect(min = 0.0, default = 20.0)]
    pub rate: f32,
    /// Particles per burst, at most [`MAX_PARTICLES`].
    pub burst: u32,
    /// How many times it has burst. A game raises it in a tick to burst: each frame bursts once
    /// per increase, born where the latest tick left the emitter, at its end. A rollback or
    /// correction that reruns the tick bursts once; one that lowers the count bursts none. A
    /// spawned emitter with a raised count bursts on its first draw.
    pub bursts: u32,
    /// Least and most lifetime in seconds; each particle gets a random value between.
    #[reflect(min = 0.0, default = LIFETIME)]
    pub lifetime: [f32; 2],
    /// Least and most birth speed along the shape's direction, in world units a second, whatever
    /// the entity's scale.
    #[reflect(default = SPEED)]
    pub speed: [f32; 2],
    /// Where particles are born and which way they go.
    pub shape: EmitterShape,
    /// Acceleration in world units a second squared: gravity is (0, -9.81, 0); fire rises.
    pub acceleration: Vec3,
    /// The share of velocity lost a second, by linear drag: 0 for none.
    #[reflect(min = 0.0)]
    pub drag: f32,
    /// Size across, in world units, at birth and at death, changing over its life.
    #[reflect(min = 0.0, default = SIZE)]
    pub size: [f32; 2],
    /// Linear RGBA at birth and at death, alpha not multiplied in, changing over its life. Past 1
    /// is light, like fire.
    #[reflect(color, default = COLOR)]
    pub color: [Vec4; 2],
    /// Least and most turn a second, counterclockwise as seen; each particle gets a random value
    /// between, from a random starting turn.
    #[reflect(degrees)]
    pub spin: [f32; 2],
    /// What a particle shows, tinted by its color. None is a soft disc, brightest at its middle.
    pub texture: Option<Texture>,
    /// Cells across and down the texture, cut evenly. A particle plays through them over its
    /// life, row by row from the top left. `[1, 1]` is the whole texture.
    #[reflect(min = 1, default = CELLS)]
    pub cells: [u32; 2],
    /// Whether particles go over what's behind them by their alpha, or add their light.
    pub blend: ParticleBlend,
}

/// The default emitter's lifetimes, speeds, sizes, colors and cells; the fields' defaults too.
const LIFETIME: [f32; 2] = [1.0, 2.0];
const SPEED: [f32; 2] = [1.0, 2.0];
const SIZE: [f32; 2] = [0.2, 0.05];
const COLOR: [Vec4; 2] = [Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)];
const CELLS: [u32; 2] = [1, 1];

impl Default for ParticleEmitter {
    /// 20 particles a second: white discs fading out as they shrink, up from a small cone, living
    /// a second or two.
    fn default() -> Self {
        Self {
            rate: 20.0,
            burst: 0,
            bursts: 0,
            lifetime: LIFETIME,
            speed: SPEED,
            shape: EmitterShape::default(),
            acceleration: Vec3::ZERO,
            drag: 0.0,
            size: SIZE,
            color: COLOR,
            spin: [0.0; 2],
            texture: None,
            cells: CELLS,
            blend: ParticleBlend::Alpha,
        }
    }
}

impl ParticleEmitter {
    /// What the renderer takes of it, beside its frame's births and bursts.
    fn emission(&self) -> Emission {
        let shape = match self.shape {
            EmitterShape::Point => ParticleShape::Point,
            EmitterShape::Sphere { radius } => ParticleShape::Sphere { radius },
            EmitterShape::Box { half_extents } => ParticleShape::Box { half_extents },
            EmitterShape::Cone { angle, radius } => ParticleShape::Cone { angle, radius },
        };
        Emission {
            shape,
            speed: self.speed,
            lifetime: self.lifetime,
            spin: self.spin,
            acceleration: self.acceleration,
            drag: self.drag,
            size: self.size,
            color: self.color,
            texture: self.texture,
            cells: self.cells,
            additive: self.blend == ParticleBlend::Additive,
        }
    }
}

/// The world's time for a frame. `ticked` is ticks run; the latest's end is where bursts and an
/// emitter's first birth are born. `drawn` is the frame's time in ticks, `alpha` through the
/// latest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Clock {
    pub(crate) ticked: u64,
    pub(crate) drawn: f64,
}

impl Clock {
    /// The time at `alpha` through the latest of `ticks` ticks; 0 before any.
    pub(crate) fn new(ticks: u64, alpha: f32) -> Self {
        Self {
            ticked: ticks,
            drawn: (ticks as f64 - 1.0 + f64::from(alpha)).max(0.0),
        }
    }
}

/// The frames' state of emitters by entity ID, outside the world. Dropped the frame after one that
/// doesn't draw the emitter.
#[derive(Default)]
pub(crate) struct Emitters {
    by_id: HashMap<EntityId, Emitting>,
    /// Frames begun; each emitter's `seen` is one's number.
    frame: u64,
    /// The last frame's origin; each `from` is relative to it.
    origin: DVec3,
}

/// What a frame emits an emitter's particles from.
#[derive(Clone, Copy, Debug)]
struct Emitting {
    seen: u64,
    /// Births at `rate` started at `start` world ticks, numbered from `base`: birth `n` is at
    /// `start + (n - base) * every(rate)`.
    start: f64,
    base: u64,
    rate: f32,
    /// Births so far: the next's number.
    emitted: u64,
    /// Bursts count the frame before saw; 0 before any frame drew it.
    bursts: u32,
    /// Where the frame before drew it, and its world time in ticks.
    from: Mat4,
    then: f64,
}

impl Emitters {
    /// Begins a frame relative to `origin`: drops emitters the last frame didn't draw and moves
    /// the rest's last positions into `origin`'s space.
    pub(crate) fn begin(&mut self, origin: DVec3) {
        let last = self.frame;
        self.by_id.retain(|_, emitting| emitting.seen == last);
        self.frame += 1;
        let moved = (self.origin - origin).as_vec3();
        self.origin = origin;
        if moved != Vec3::ZERO {
            let moved = Mat4::from_translation(moved);
            for emitting in self.by_id.values_mut() {
                emitting.from = moved * emitting.from;
            }
        }
    }

    /// This frame's particles of `emitter` (entity `id`), per [`ParticleEmitter`]. `drawn` is its
    /// draw position, `ticked` where the latest tick left it. Births due by `clock` since the
    /// last frame, placed between the two draws; the oldest drop past the most alive at once.
    /// Bursts, one per rise of its count, born at the latest tick's end.
    pub(crate) fn emit(
        &mut self,
        id: EntityId,
        emitter: &ParticleEmitter,
        drawn: Mat4,
        ticked: Mat4,
        clock: Clock,
    ) -> Particles {
        let (now, latest) = (clock.drawn, clock.ticked as f64);
        // NaN and less than 0 count as none.
        let rate = if emitter.rate > 0.0 && emitter.rate.is_finite() {
            emitter.rate
        } else {
            0.0
        };
        let size = emitter.burst.min(MAX_PARTICLES);
        let lifetime = emitter.lifetime[0].max(emitter.lifetime[1]).max(0.0);
        let alive = (f64::from(rate) * f64::from(lifetime)).ceil();
        // Saturating: infinity exceeds any count.
        let most = (alive as u32).saturating_add(size);
        let started = Emitting {
            seen: self.frame,
            start: latest,
            base: 0,
            rate,
            emitted: 0,
            bursts: 0,
            from: drawn,
            then: now,
        };
        let mut fresh = false;
        let emitting = self.by_id.entry(id).or_insert_with(|| {
            fresh = true;
            started
        });
        // The world's time went back further than the tick a pause's end draws back by, as the
        // tool API's `stop` does: births start again as if new; the bursts count is kept.
        if !fresh && now < emitting.then - 1.0 {
            fresh = true;
            *emitting = Emitting {
                bursts: emitting.bursts,
                ..started
            };
        }
        emitting.seen = self.frame;
        // A rate changed: births go on at the new rate from where the next was due at the old, or
        // from the latest tick's end if the old was none.
        if rate.to_bits() != emitting.rate.to_bits() {
            let old = emitting.rate;
            let since = (emitting.emitted - emitting.base) as f64;
            emitting.start = if old > 0.0 {
                emitting.start + since * every(old)
            } else {
                latest
            };
            emitting.base = emitting.emitted;
            emitting.rate = rate;
        }
        let limit = most.min(MAX_PARTICLES);
        let births = births(emitting, now, drawn, limit);
        let bursts = bursts(emitting, emitter.bursts, size, latest, ticked, limit);
        emitting.from = drawn;
        emitting.then = now;
        Particles {
            id: id.get(),
            fresh,
            most,
            now,
            tick: Tick::DT.as_secs_f32(),
            births,
            bursts,
            emission: emitter.emission(),
        }
    }
}

/// The ticks between births at `rate` births a second, more than 0.
fn every(rate: f32) -> f64 {
    1.0 / (f64::from(rate) * Tick::DT.as_secs_f64())
}

/// `emitting`'s births due by world time `now`, placed between the last frame's draw and `drawn`.
/// Births past `limit` drop, oldest first.
fn births(emitting: &mut Emitting, now: f64, drawn: Mat4, limit: u32) -> Births {
    let none = Births::none(drawn);
    if emitting.rate == 0.0 || now < emitting.start {
        return none;
    }
    let every = every(emitting.rate);
    // Births from `base` on due at or before now; saturating cast.
    let due = ((now - emitting.start) / every).floor() + 1.0;
    let end = emitting.base.saturating_add(due as u64);
    if end <= emitting.emitted {
        return none;
    }
    let first = emitting.emitted.max(end - u64::from(limit).min(end));
    emitting.emitted = end;
    let time = emitting.start + (first - emitting.base) as f64 * every;
    // Where between the frames each is born.
    let span = now - emitting.then;
    let (along, step) = match span > 0.0 {
        true => (
            ((time - emitting.then) / span) as f32,
            (every / span) as f32,
        ),
        false => (1.0, 0.0),
    };
    Births {
        first,
        count: (end - first) as u32,
        time,
        every,
        from: emitting.from,
        to: drawn,
        along,
        step,
    }
}

/// One burst of `size` per rise of the count to `counter`, born at `latest` (the latest tick's
/// end) at `ticked`. The first particles past `limit` drop. A lowered count bursts nothing.
fn bursts(
    emitting: &mut Emitting,
    counter: u32,
    size: u32,
    latest: f64,
    ticked: Mat4,
    limit: u32,
) -> Bursts {
    let first = emitting.bursts;
    let rise = counter.wrapping_sub(first);
    emitting.bursts = counter;
    if rise == 0 || rise > u32::MAX / 2 || size == 0 {
        return Bursts::none(ticked);
    }
    let dropped = (u64::from(rise) * u64::from(size)).saturating_sub(u64::from(limit));
    let whole = (dropped / u64::from(size)) as u32;
    Bursts {
        first: first.wrapping_add(whole),
        count: rise - whole,
        size,
        skip: (dropped % u64::from(size)) as u32,
        time: latest,
        at: ticked,
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::DVec3;

    use super::*;
    use crate::draw::{self, Scratch};
    use crate::font::Fonts;
    use crate::interpolation::Past;
    use crate::placed::Placed;
    use crate::{Transform, World};
    use gg_renderer::{Draws, Shaders, Textures};

    /// An emitter of `rate` particles a second, each living a second.
    fn at_rate(rate: f32) -> ParticleEmitter {
        ParticleEmitter {
            rate,
            lifetime: [1.0, 1.0],
            ..ParticleEmitter::default()
        }
    }

    /// Each birth's number and tick time from `Emitters::emit` over `frames` (ticks run, time).
    fn born(emitter: &ParticleEmitter, frames: &[(u64, f64)]) -> Vec<(u64, f64)> {
        let mut emitters = Emitters::default();
        let id = EntityId::new(7).expect("not 0");
        let mut born = Vec::new();
        for &(ticked, drawn) in frames {
            emitters.begin(DVec3::ZERO);
            let clock = Clock { ticked, drawn };
            let particles = emitters.emit(id, emitter, Mat4::IDENTITY, Mat4::IDENTITY, clock);
            let births = particles.births;
            let times = (0..births.count).map(|i| {
                let n = births.first + u64::from(i);
                (n, births.time + f64::from(i) * births.every)
            });
            born.extend(times);
        }
        born
    }

    /// At 16 a second (a birth every 4 of 64 ticks a second), birth `n` is born `4n` ticks after
    /// the latest tick's end at first draw, in the first frame reaching it, whatever the frames.
    /// Births past the most alive at once drop, a long frame's oldest.
    #[test]
    fn births_are_due_by_the_frames_time_whatever_the_frames() {
        let emitter = at_rate(16.0);
        // First drawn after 10 ticks, at 9.5: the first birth is due at 10.
        let even: Vec<_> = (0..40)
            .map(|frame| (10 + frame, 9.5 + frame as f64))
            .collect();
        let uneven = [
            (10, 9.5),
            (11, 10.0),
            (14, 13.9),
            (20, 19.0),
            (30, 29.5),
            (49, 48.5),
        ];
        let want: Vec<_> = (0..10).map(|n| (n, 10.0 + 4.0 * n as f64)).collect();
        assert_eq!(born(&emitter, &even), want);
        assert_eq!(born(&emitter, &uneven), want);
        // 16 alive at most, of a second's life: a frame 200 ticks on has the newest 16 alone.
        let late = born(&emitter, &[(10, 10.0), (210, 210.0)]);
        let newest: Vec<_> = (35..51).map(|n| (n, 10.0 + 4.0 * n as f64)).collect();
        assert_eq!(late[1..], newest);
    }

    /// A changed rate continues from the next birth's old due time. Time going back a tick, as a
    /// pause's end draws it, holds births until it passes them; further back restarts them.
    #[test]
    fn rates_go_on_and_time_back_holds_them() {
        let mut emitters = Emitters::default();
        let id = EntityId::new(7).expect("not 0");
        let mut at = |emitter: &ParticleEmitter, ticked: u64, drawn: f64| {
            emitters.begin(DVec3::ZERO);
            let clock = Clock { ticked, drawn };
            emitters.emit(id, emitter, Mat4::IDENTITY, Mat4::IDENTITY, clock)
        };
        let first = at(&at_rate(16.0), 0, 0.0);
        assert!(first.fresh);
        assert_eq!((first.births.count, first.births.time), (1, 0.0));
        // Births at 0 and 4 so far; at 32 a second from 8, the next due, every 2 ticks.
        assert_eq!(at(&at_rate(16.0), 5, 5.0).births.count, 1);
        let faster = at(&at_rate(32.0), 12, 12.0).births;
        assert_eq!((faster.first, faster.count, faster.time), (2, 3, 8.0));
        assert_eq!(faster.every, 2.0);
        // Back to 11.5, as a resumed pause draws: nothing until past 12.
        assert_eq!(at(&at_rate(32.0), 12, 11.5).births.count, 0);
        let past = at(&at_rate(32.0), 14, 14.0);
        assert!(!past.fresh);
        assert_eq!((past.births.first, past.births.count), (5, 1));
        // Back 10 ticks, as `stop` takes it: started again, at the latest tick's end.
        let again = at(&at_rate(32.0), 5, 4.0);
        assert!(again.fresh);
        assert_eq!(again.births.count, 0);
        assert_eq!(at(&at_rate(32.0), 5, 5.0).births.first, 0);
    }

    /// A still emitter when the origin moves births between two draws that are both relative to
    /// the new origin, so at its place, not along a line from its old-origin position.
    #[test]
    fn births_come_from_where_the_emitter_was_as_the_origin_moves() {
        let mut emitters = Emitters::default();
        let id = EntityId::new(7).expect("not 0");
        let emitter = at_rate(16.0);
        let stands = DVec3::new(1_000_000.25, 2.0, -999_999.5);
        let mut frame = |origin: DVec3, ticked: u64| {
            emitters.begin(origin);
            let drawn = Mat4::from_translation((stands - origin).as_vec3());
            let clock = Clock {
                ticked,
                drawn: ticked as f64,
            };
            emitters.emit(id, &emitter, drawn, drawn, clock).births
        };
        let first = DVec3::new(1_000_448.0, 0.0, -1_000_448.0);
        frame(first, 0);
        let moved = frame(first + DVec3::new(1024.0, 0.0, 0.0), 8);
        assert_eq!(moved.count, 2);
        assert_eq!(moved.from, moved.to);
    }

    /// The particles `world`'s emitters give the frame drawn after `ticks` ticks.
    fn drawn(world: &World, ticks: u64, scratch: &mut Scratch) -> Vec<Particles> {
        let mut draws = Draws::default();
        let past = Past::default();
        draw::draw_world(
            world,
            &past,
            &Placed::of(world, &past, 1.0),
            (1.0, DVec3::ZERO),
            ticks,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            scratch,
        );
        draws.particles().to_vec()
    }

    /// Raising `bursts` in a tick bursts once in the next frame, at the tick's end where it left
    /// the emitter. A rollback rerunning the tick, or a correction lowering the count, bursts no
    /// more; raising it again does. A spawned emitter with a raised count bursts on first draw:
    /// the newest bursts that fit beside its births.
    #[test]
    fn a_burst_is_born_once_however_often_its_tick_runs() {
        let mut world = World::default();
        let emitter = ParticleEmitter {
            rate: 0.0,
            burst: 12,
            ..ParticleEmitter::default()
        };
        let at = Transform {
            translation: DVec3::new(1.0, 2.0, 0.0),
            ..Transform::IDENTITY
        };
        let entity = world.spawn((at, emitter));
        let mut scratch = Scratch::default();
        let bursts = |particles: &[Particles]| {
            let bursts = particles[0].bursts;
            (bursts.first, bursts.count, bursts.particles())
        };
        assert_eq!(bursts(&drawn(&world, 3, &mut scratch)), (0, 0, 0));
        let before = world.clone();
        // The tick, which raises it, and moves the emitter.
        let tick = |world: &mut World| {
            world
                .get_mut::<ParticleEmitter>(entity)
                .expect("alive")
                .bursts += 1;
            world
                .get_mut::<Transform>(entity)
                .expect("alive")
                .translation
                .x = 5.0;
        };
        tick(&mut world);
        let particles = drawn(&world, 4, &mut scratch);
        assert_eq!(bursts(&particles), (0, 1, 12));
        let burst = particles[0].bursts;
        assert_eq!((burst.time, burst.at.w_axis.x), (4.0, 5.0));
        // Rolled back and run again: the same count, so no burst.
        world.clone_from(&before);
        tick(&mut world);
        assert_eq!(bursts(&drawn(&world, 4, &mut scratch)), (0, 0, 0));
        // Corrected: the tick didn't burst after all.
        world.clone_from(&before);
        assert_eq!(bursts(&drawn(&world, 4, &mut scratch)), (0, 0, 0));
        tick(&mut world);
        assert_eq!(bursts(&drawn(&world, 5, &mut scratch)), (0, 1, 12));
        // Spawned with two bursts counted; room for one beside its births, so the newest.
        let counted = ParticleEmitter {
            bursts: 2,
            ..emitter
        };
        world.spawn((at, counted));
        let particles = drawn(&world, 6, &mut scratch);
        let spawned = particles.iter().find(|particles| particles.fresh);
        let spawned = spawned.expect("the new emitter's");
        assert_eq!(bursts(std::slice::from_ref(spawned)), (1, 1, 12));
    }

    /// A paused world's time stands still: its frames bear no new particles and draw existing ones
    /// at their ages by that time. A step bears a tick's.
    #[test]
    fn a_paused_world_freezes_its_particles() {
        let mut world = World::default();
        world.spawn((Transform::IDENTITY, at_rate(64.0)));
        let mut scratch = Scratch::default();
        let first = drawn(&world, 20, &mut scratch);
        assert_eq!((first[0].births.count, first[0].now), (1, 20.0));
        for _ in 0..10 {
            let paused = drawn(&world, 20, &mut scratch);
            assert_eq!((paused[0].births.count, paused[0].now), (0, 20.0));
        }
        let stepped = drawn(&world, 21, &mut scratch);
        assert_eq!((stepped[0].births.count, stepped[0].now), (1, 21.0));
    }
}
