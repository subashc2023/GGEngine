//! Particles: emitters, components of the world, and their particles, which are the frames'. Each
//! frame emits what the world's time as it draws owes each emitter since the frame before, which
//! `gg-renderer` writes and draws, so that no hash, snapshot, scene, recording or rollback holds a
//! particle, as none holds the frames' copies of where the world's entities were.

use std::collections::HashMap;

use gg_core::math::{Mat4, Vec3, Vec4};
use gg_renderer::{Births, Bursts, Emission, MAX_PARTICLES, ParticleShape, Particles};

use crate::{Component, EntityId, Reflect, Texture, Tick};

/// Where an emitter's particles are born, in its entity's space, so that they turn and scale with
/// it, and which way they go: an emitter without a rotation sends them up, along its +Y, as a
/// fountain and a fire do.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum EmitterShape {
    /// At its entity's origin, any way.
    Point,
    /// Anywhere within `radius` of its origin, away from it.
    Sphere { radius: f32 },
    /// Anywhere in the box of `half_extents` round its origin, along its +Y.
    Box { half_extents: Vec3 },
    /// On the disc of `radius` round its origin, in its XZ plane, any way within `angle` of its
    /// +Y: the cone's half angle, in radians.
    Cone {
        #[reflect(degrees)]
        angle: f32,
        radius: f32,
    },
}

impl Default for EmitterShape {
    /// A cone 15° about its +Y, from a disc a tenth of a unit across.
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
    /// By their alpha, as smoke, dust and water do.
    #[default]
    Alpha,
    /// Adding their light, by their alpha, as fire, sparks and glows do: a color past 1 is
    /// brighter.
    Additive,
}

/// Particles, born where its entity draws, ancestors carrying it, which the frames own: the
/// world holds the emitter alone, which scenes save, the Inspector and tools edit, and the world's
/// hash reads, as any component. `rate` particles a second are born evenly, particle `n` at
/// `n / rate` seconds of the world's time after the emitter first draws, each where the emitter was
/// then, between where the frames around its birth drew it; and a burst of `burst` at once each
/// time a tick raises `bursts`. Each particle's motion is a closed form of its age, its velocity
/// at birth carried on under `acceleration` and `drag`, and its size, color, turn and cell by its
/// age over its lifetime, so that it draws alike whatever the frames between its birth and now:
/// the world's time stops while the simulation is paused or stopped, and so do its particles, and
/// headless frames draw the same particles every run. Each particle's randomness is a hash of its
/// emitter's ID and its number. A frame that doesn't draw the emitter, as one after its entity
/// goes, lets go of its particles; and so does the world's time going back more than a tick, as
/// the tool API's `stop` takes it, which starts its births again.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct ParticleEmitter {
    /// Particles born a second, evenly: 0 for none but its bursts.
    #[reflect(min = 0.0, default = 20.0)]
    pub rate: f32,
    /// The particles each of its bursts gives, at most [`MAX_PARTICLES`].
    pub burst: u32,
    /// How many times it has burst, which a game raises at a tick to burst: each frame bursts once
    /// for each it went up by since the frame before, every burst born where the latest tick left
    /// the emitter, as that tick ends. So a rollback or a server's correction that runs the tick
    /// again bursts once, and one that takes it back down bursts no more; and an emitter spawned
    /// with its count raised bursts as it first draws.
    pub bursts: u32,
    /// The least and the most seconds a particle lives, each a share between them at random.
    #[reflect(min = 0.0, default = LIFETIME)]
    pub lifetime: [f32; 2],
    /// The least and the most speed a particle is born with, along the way its shape gives it, in
    /// the world's units a second, whatever its entity's scale.
    #[reflect(default = SPEED)]
    pub speed: [f32; 2],
    /// Where particles are born and which way they go.
    pub shape: EmitterShape,
    /// What every particle accelerates by, in the world's units a second squared: gravity's
    /// (0, -9.81, 0), or a fire's rising.
    pub acceleration: Vec3,
    /// The share of its velocity a particle loses a second, by linear drag: 0 for none.
    #[reflect(min = 0.0)]
    pub drag: f32,
    /// A particle's size across, in the world, at its birth and at its death, as it goes from one
    /// to the other over its life.
    #[reflect(min = 0.0, default = SIZE)]
    pub size: [f32; 2],
    /// A particle's color at its birth and at its death, in linear RGBA, alpha not multiplied in, as
    /// it goes from one to the other over its life: past 1 for light, as fire's.
    #[reflect(color, default = COLOR)]
    pub color: [Vec4; 2],
    /// The least and the most a particle turns a second, counterclockwise as seen, each a share
    /// between them at random, from a turn at random.
    #[reflect(degrees)]
    pub spin: [f32; 2],
    /// What a particle shows, tinted by its color, by its path: none for a soft disc, brightest at
    /// its middle.
    pub texture: Option<Texture>,
    /// The cells across and down its texture is cut into, evenly, which a particle plays through,
    /// row by row from the top left, over its life: `[1, 1]` for the whole texture.
    #[reflect(min = 1, default = CELLS)]
    pub cells: [u32; 2],
    /// Whether particles go over what's behind them by their alpha, or add their light.
    pub blend: ParticleBlend,
}

/// The default emitter's lifetimes, speeds, sizes, colors and cells, which its description gives
/// as its fields' defaults too.
const LIFETIME: [f32; 2] = [1.0, 2.0];
const SPEED: [f32; 2] = [1.0, 2.0];
const SIZE: [f32; 2] = [0.2, 0.05];
const COLOR: [Vec4; 2] = [Vec4::ONE, Vec4::new(1.0, 1.0, 1.0, 0.0)];
const CELLS: [u32; 2] = [1, 1];

impl Default for ParticleEmitter {
    /// 20 particles a second, white discs fading out as they shrink, up from a small cone, living a
    /// second or two.
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

/// The world's time as a frame draws it: the ticks it has run, the latest's end, where a burst and
/// an emitter's first birth are born; and how far the frame draws the world, in ticks, `alpha` of
/// the way through the latest, as `Stepper::time` has it in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Clock {
    pub(crate) ticked: u64,
    pub(crate) drawn: f64,
}

impl Clock {
    /// The time a frame `alpha` of the way through the latest of `ticks` ticks draws at: 0 before
    /// any tick.
    pub(crate) fn new(ticks: u64, alpha: f32) -> Self {
        Self {
            ticked: ticks,
            drawn: (ticks as f64 - 1.0 + f64::from(alpha)).max(0.0),
        }
    }
}

/// The frames' state of the world's emitters, by their entities' IDs, kept from frame to frame
/// outside the world, as the frames' copies of where entities were are: what each frame emits
/// from. An emitter's is let go of the frame after one that doesn't draw it.
#[derive(Default)]
pub(crate) struct Emitters {
    by_id: HashMap<EntityId, Emitting>,
    /// How many frames have begun, which each emitter's `seen` is the number of one of.
    frame: u64,
}

/// What a frame emits an emitter's particles from.
#[derive(Clone, Copy, Debug)]
struct Emitting {
    /// The last frame that drew it.
    seen: u64,
    /// When its births at its rate started, in the world's ticks, and the number of the first
    /// born then: birth `n`, from `base` on, is born at `start + (n - base) * every(rate)`.
    start: f64,
    base: u64,
    rate: f32,
    /// How many births it has had: the next's number.
    emitted: u64,
    /// Its count of bursts as the frame before saw it, 0 before any frame drew it.
    bursts: u32,
    /// Where the frame before drew it, and the world's time it drew at, in ticks.
    from: Mat4,
    then: f64,
}

impl Emitters {
    /// Begins a frame's emitting: lets go of what the frame before didn't draw.
    pub(crate) fn begin(&mut self) {
        let last = self.frame;
        self.by_id.retain(|_, emitting| emitting.seen == last);
        self.frame += 1;
    }

    /// The particles of `emitter`, of entity `id`, this frame, which draws it where `drawn` places
    /// it, its transform as the latest tick left it `ticked`, at the world's time `clock` says, as
    /// [`ParticleEmitter`] says: its births since the frame before drew it, due by the frame's
    /// time, each where the emitter was between where the frame before drew it and where this one
    /// does, those past the most it has alive at once dropped, the oldest; and its bursts, as many
    /// as its count went up by since, born as the latest tick ends, where it left the emitter.
    pub(crate) fn emit(
        &mut self,
        id: EntityId,
        emitter: &ParticleEmitter,
        drawn: Mat4,
        ticked: Mat4,
        clock: Clock,
    ) -> Particles {
        let (now, latest) = (clock.drawn, clock.ticked as f64);
        // What isn't a number, or is less than 0, is none.
        let rate = if emitter.rate > 0.0 && emitter.rate.is_finite() {
            emitter.rate
        } else {
            0.0
        };
        let size = emitter.burst.min(MAX_PARTICLES);
        let lifetime = emitter.lifetime[0].max(emitter.lifetime[1]).max(0.0);
        let alive = (f64::from(rate) * f64::from(lifetime)).ceil();
        // A saturating cast, which takes the infinite past any count.
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
        // tool API's `stop` takes it: its births start again, as though it had just come, its
        // count of bursts kept.
        if !fresh && now < emitting.then - 1.0 {
            fresh = true;
            *emitting = Emitting {
                bursts: emitting.bursts,
                ..started
            };
        }
        emitting.seen = self.frame;
        // A rate changed: births go on at the new from where the next was due at the old, or, from
        // none, from the latest tick's end.
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

/// The ticks between births at `rate`, births a second, of more than 0.
fn every(rate: f32) -> f64 {
    1.0 / (f64::from(rate) * Tick::DT.as_secs_f64())
}

/// `emitting`'s births due by `now`, of the world's time, since those before, as the frame before
/// left them: each born where the emitter was between where that frame drew it and `drawn`, of
/// those past `limit`, the oldest dropped.
fn births(emitting: &mut Emitting, now: f64, drawn: Mat4, limit: u32) -> Births {
    let none = Births::none(drawn);
    if emitting.rate == 0.0 || now < emitting.start {
        return none;
    }
    let every = every(emitting.rate);
    // Births from `base` on whose times are at or before now; a saturating cast.
    let due = ((now - emitting.start) / every).floor() + 1.0;
    let end = emitting.base.saturating_add(due as u64);
    if end <= emitting.emitted {
        return none;
    }
    let first = emitting.emitted.max(end - u64::from(limit).min(end));
    emitting.emitted = end;
    let time = emitting.start + (first - emitting.base) as f64 * every;
    // How far between the frames each is born.
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

/// `emitting`'s bursts of `size` each, as many as its count went up by to `counter` since the frame
/// before saw it, born at `latest`, the latest tick's end, where `ticked` places the emitter, the
/// first of those past `limit` dropped. A count taken down bursts nothing.
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

    /// Each birth's number and time, in ticks, of every frame of `emitter` drawn at each of
    /// `drawn`, the world's times, at `ticked` ticks run each, as `Emitters::emit` gives them.
    fn born(emitter: &ParticleEmitter, frames: &[(u64, f64)]) -> Vec<(u64, f64)> {
        let mut emitters = Emitters::default();
        let id = EntityId::new(7).expect("not 0");
        let mut born = Vec::new();
        for &(ticked, drawn) in frames {
            emitters.begin();
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

    /// At 16 a second, a birth every 4 ticks of 64 a second: birth `n` is born at the latest tick's
    /// end as the emitter first drew, plus `4n` ticks, in the first frame whose time reaches it,
    /// whichever frames draw and however many ticks each runs; and those past the most it has
    /// alive at once, a long frame's oldest, are dropped.
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

    /// A rate that changes goes on from where the next birth was due at the old one; and the
    /// world's time going back a tick, as a pause's end draws it, holds births back until it's
    /// past them, where going back further starts the births again.
    #[test]
    fn rates_go_on_and_time_back_holds_them() {
        let mut emitters = Emitters::default();
        let id = EntityId::new(7).expect("not 0");
        let mut at = |emitter: &ParticleEmitter, ticked: u64, drawn: f64| {
            emitters.begin();
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

    /// What `world`'s emitters give the frame drawn after `ticks` ticks, its draws' particles.
    fn drawn(world: &World, ticks: u64, scratch: &mut Scratch) -> Vec<Particles> {
        let mut draws = Draws::default();
        let past = Past::default();
        draw::draw_world(
            world,
            &past,
            &Placed::of(world, &past, 1.0),
            1.0,
            ticks,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            scratch,
        );
        draws.particles().to_vec()
    }

    /// A tick that raises an emitter's count of bursts bursts it once in the frame after, born as
    /// the tick ends where it left the emitter; a rollback that restores the world from before the
    /// tick and runs it again bursts no more, nor does a correction that takes the count back
    /// down, after which raising it again bursts again. An emitter spawned with its count raised
    /// bursts as it's first drawn, of as many bursts as fit beside its births, the newest.
    #[test]
    fn a_burst_is_born_once_however_often_its_tick_runs() {
        let mut world = World::default();
        let emitter = ParticleEmitter {
            rate: 0.0,
            burst: 12,
            ..ParticleEmitter::default()
        };
        let at = Transform {
            translation: Vec3::new(1.0, 2.0, 0.0),
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
        // Spawned with two bursts counted, of which it holds the newest alone, as it has room for
        // a burst beside its births.
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

    /// A paused world's frames, drawn at its time, which stands still, bear no particles and draw
    /// those it has at their ages, as the time each frame gives says; a step bears a tick's.
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
