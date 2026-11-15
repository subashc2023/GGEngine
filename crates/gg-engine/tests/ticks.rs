//! Steady-state ticks allocate nothing, on any thread: a layer's ticks that apply its commands,
//! run a system on the job system's threads and another on the ticks' thread alone, and hash the
//! world, as a recorded run's do, once their first ticks have run, with scripts that despawn their
//! own entities and spawn others in their place among them, and a Luau script that moves its
//! entities and counts its ticks in its own component, sprites' flipbooks and tweens of their
//! fields, looping and going back and forth, skinned foxes' state machines, blending a walk and a
//! run along a speed that the layer sets for some and a Luau script for the rest, and cutting from
//! a walk to a hop at a trigger and back, and agents walking to and fro over a navmesh, each
//! planning a path round a wall's end at each end of its way, turning its corners as each comes in
//! sight, and steering round the others coming the other way; before the job system's threads
//! start,
//! as in a run on one thread, and after, once each thread has waited for jobs and slept, as it
//! allocates the first time it does; and ticks that despawn entities and spawn others in their
//! place, as a game's bullets and particles come and go.
//!
//! It runs in one process with a main of its own, so that no other test's threads run beside it,
//! and counts every allocation the process makes while the ticks run, whichever thread makes it,
//! keeping where the first was made.

// For the allocator that counts, in `common`; see its SAFETY comments.
#![allow(unsafe_code)]

mod common;

use std::collections::VecDeque;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{fs, process, thread};

use gg_engine::math::{DVec3, Quat, Vec3};
use gg_engine::reflect::Value;
use gg_engine::{
    AnimationParam, AnimationState, AnimationStates, BlendPoint, BodyType, BoxCollider3D,
    Component, Context, Curve, Entity, Flipbook, Layer, Model, NavAgent, NavMeshVolume,
    ParamCondition, ParamTest, ParamValue, Playback, RigidBody3D, Script, Scripts, Sprite,
    StateMotion, StateTransition, Tick, Transform, Tween, World, run_scripts, step_animation,
    step_navigation,
};

use crate::common::Counted;

/// The ticks run before counting, on one thread and again on four, as a run's first ticks size
/// what the world and the layer keep, and the pool's what its threads make as they first wait.
const WARM_UP: u64 = 64;
/// The ticks counted.
const COUNTED: u64 = 1_000;
/// The parents, each with a child, that churn, and how many of them each tick replaces.
const SUBTREES: usize = 2_000;
const REPLACED: usize = 20;
/// The ticks of churn counted: far more than the world's map of IDs takes, at this population,
/// to run out of room, so that one that grew at a time its keys pick would grow among them.
const CHURNED: u64 = 20_000;

/// The allocations the process makes while `f` runs, on every thread, and where the first was made.
fn allocations(f: impl FnOnce()) -> Counted {
    common::start();
    f();
    common::stop()
}

#[derive(Clone, Component)]
struct Velocity(Vec3);

/// On a third of the movers, so that they're two tables.
#[derive(Clone, Component)]
struct Heavy;

/// The scripted entities, of which each tick's scripts replace a `LIFETIME`th.
const EPHEMERA: u32 = 256;
const LIFETIME: u32 = 64;

/// A script that drifts its entity, and once its ticks are up despawns it and spawns another in its
/// place.
#[derive(Clone, Component)]
struct Ephemeral {
    left: u32,
}

impl Script for Ephemeral {
    fn tick(entity: Entity, world: &mut World, _: Tick) {
        let Some(ephemeral) = world.get_mut::<Self>(entity) else {
            return;
        };
        ephemeral.left -= 1;
        if ephemeral.left > 0 {
            if let Some(transform) = world.get_mut::<Transform>(entity) {
                transform.translation.x += 1.0;
            }
            return;
        }
        world.despawn(entity).expect("alive");
        world.spawn((Transform::default(), Self { left: LIFETIME }));
    }
}

/// The entities a Luau script moves.
const DRIFTERS: u32 = 256;

/// The sprites that flipbooks play and tweens ease.
const ANIMATED: u32 = 256;

/// The agents walking to and fro, half from each end.
const WALKERS: u32 = 24;

/// A static box of `size` around `at`, which the navmesh is baked from.
fn solid(world: &mut World, at: Vec3, size: Vec3) {
    let placed = Transform {
        translation: at.as_dvec3(),
        ..Transform::default()
    };
    let body = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let boxed = BoxCollider3D {
        size,
        ..BoxCollider3D::default()
    };
    world.spawn((placed, body, boxed));
}

/// Moves its entities along x, and counts its ticks in its own component.
const DRIFT: &str = r#"
local Transform = gg.component("Transform")
local Drift = gg.declare("Drift", { speed = 1, ticks = 0 })

function Drift.tick(entity, dt)
    local x = entity[Drift.speed] * dt
    entity[Transform.translation] = entity[Transform.translation] + vector.create(x, 0, 0)
    entity[Drift.ticks] = entity[Drift.ticks] + 1
end

return Drift
"#;

/// The foxes whose state machines blend Walk and Run along a speed, half of them the layer's, which
/// it sets at each tick, and half a Luau script's; and those that cut from a walk to a hop, Run
/// played once, at a trigger the layer pulls every `HOPS` ticks, and back once it's done.
const BLENDING: u32 = 8;
const CUTTING: u32 = 4;
const HOPS: u64 = 48;

/// Sets its entity's state machine's speed at each tick, between a walk's and a run's.
const PACE: &str = r#"
local Pace = gg.declare("Pace", { time = 0 })

function Pace.tick(entity, dt)
    local time = entity[Pace.time] + dt
    entity[Pace.time] = time
    gg.set_param(entity, "speed", 3.3 + 1.5 * math.sin(time))
end

return Pace
"#;

fn param(name: &str, value: ParamValue) -> AnimationParam {
    AnimationParam {
        name: name.to_owned(),
        value,
    }
}

/// A state of `name` that plays `motion`, at `speed`, as `playback` says.
fn state(name: &str, motion: StateMotion, speed: f32, playback: Playback) -> AnimationState {
    AnimationState {
        name: name.to_owned(),
        motion,
        speed,
        playback,
    }
}

/// A transition from `from`, or from any state, to `to`, cutting, on `conditions`, after `exit` of
/// its state's motion if that's given.
fn cut(
    from: Option<&str>,
    to: &str,
    conditions: Vec<ParamCondition>,
    exit: Option<f32>,
) -> StateTransition {
    StateTransition {
        from: from.map(str::to_owned),
        to: to.to_owned(),
        conditions,
        exit_time: exit,
        fade: 0.0,
    }
}

/// A fox's machine that blends Walk and Run along its speed, and stays.
fn blending() -> AnimationStates {
    let points = [("Walk", 1.6), ("Run", 5.0)].map(|(clip, at)| BlendPoint {
        clip: clip.to_owned(),
        at,
    });
    let blend = StateMotion::Blend {
        param: "speed".to_owned(),
        points: points.into(),
    };
    AnimationStates {
        params: vec![param("speed", ParamValue::Float(3.0))],
        states: vec![state("Move", blend, 1.0, Playback::Loop)],
        ..AnimationStates::default()
    }
}

/// A fox's machine that walks, and hops at its trigger, from any state, and walks again once the
/// hop's done.
fn cutting() -> AnimationStates {
    let clip = |name: &str| StateMotion::Clip(name.to_owned());
    let hop = ParamCondition {
        param: "hop".to_owned(),
        test: ParamTest::True,
    };
    AnimationStates {
        params: vec![param("hop", ParamValue::Trigger(false))],
        states: vec![
            state("Walk", clip("Walk"), 1.0, Playback::Loop),
            state("Hop", clip("Run"), 2.0, Playback::Once),
        ],
        transitions: vec![
            cut(None, "Hop", vec![hop], None),
            cut(Some("Hop"), "Walk", Vec::new(), Some(1.0)),
        ],
        ..AnimationStates::default()
    }
}

enum Command {
    Speed(f32),
}

/// The rows of `Movers`' system that ran on the threads the job system started rather than on the
/// ticks' thread.
static ON_WORKERS: AtomicU64 = AtomicU64::new(0);

/// Whether the calling thread is one the job system started, which the ticks' thread isn't.
fn on_a_worker() -> bool {
    thread::current()
        .name()
        .is_some_and(|name| name.starts_with("gg-job-"))
}

/// 5,000 movers, in tables of six chunks between them, which each tick moves at its speed on the
/// job system's threads, and whose heavy ones it turns back on the ticks' thread alone;
/// `EPHEMERA` scripted entities, and `DRIFTERS` that a Luau script moves, whose scripts it runs;
/// `ANIMATED` sprites, whose animation it steps; the Fox, `BLENDING` and `CUTTING` times, the first
/// half of the blending ones and the cutting ones driven by its tick; and `WALKERS` agents on a
/// floor between pillars, which its navigation steps, each sent back the way it came as it arrives.
struct Movers {
    commands: Vec<Command>,
    speed: f32,
    scripts: Scripts,
    fox: Option<Model>,
    blending: Vec<Entity>,
    cutting: Vec<Entity>,
    /// How many times an agent has been sent back the way it came, and the most corners of a path
    /// one walked.
    turned: u64,
    corners: usize,
}

impl Layer for Movers {
    fn load(&mut self, cx: &mut Context) {
        for (name, script) in [("drift", DRIFT), ("pace", PACE)] {
            let file = format!("gg-ticks-{name}-{}.luau", process::id());
            let path = std::env::temp_dir().join(file);
            fs::write(&path, script).expect("the script is written");
            let loaded = self.scripts.load(cx, &path);
            let _ = fs::remove_file(&path);
            loaded.unwrap_or_else(|error| panic!("{error:#}"));
        }
        let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox/Fox.gltf");
        let fox = cx.add_model("fox", fox);
        self.fox = Some(fox.unwrap_or_else(|error| panic!("{error:#}")));
    }

    fn attach(&mut self, cx: &mut Context) {
        let world = cx.world_mut();
        for i in 0..5_000 {
            let velocity = Velocity(Vec3::new(i as f32, 1.0, 0.0));
            if i % 3 == 0 {
                world.spawn((Transform::default(), velocity, Heavy));
            } else {
                world.spawn((Transform::default(), velocity));
            }
        }
        for i in 0..EPHEMERA {
            let left = i % LIFETIME + 1;
            world.spawn((Transform::default(), Ephemeral { left }));
        }
        for _ in 0..DRIFTERS {
            let drifter = world.spawn(Transform::default());
            world.add_default(drifter, "Drift").expect("a drifter");
        }
        for i in 0..ANIMATED {
            let flipbook = Flipbook {
                columns: 4,
                rows: 4,
                frames: 16,
                rate: 12.0,
                playback: Playback::Loop,
                time: i as f32 / 64.0,
                ..Flipbook::default()
            };
            let animated = world.spawn((Transform::default(), Sprite::default(), flipbook));
            let turned = Quat::from_rotation_z(1.0);
            let tweens = [
                Tween {
                    playback: Playback::PingPong,
                    ..Tween::new("Transform", "scale.x", 1.0, 2.0, 0.5)
                },
                Tween {
                    curve: Curve::Elastic,
                    playback: Playback::Loop,
                    ..Tween::new("Sprite", "color.w", 1.0, 0.0, 0.75)
                },
                Tween {
                    playback: Playback::PingPong,
                    ..Tween::new("Transform", "rotation", Quat::IDENTITY, turned, 1.0)
                },
            ];
            for tween in tweens {
                world.tween(animated, tween).expect("fields of their kinds");
            }
        }
        let fox = self.fox.expect("loaded");
        for i in 0..BLENDING + CUTTING {
            let root = fox.spawn(world, Transform::default());
            if i < BLENDING {
                world.insert(root, blending()).expect("alive");
                if i < BLENDING / 2 {
                    self.blending.push(root);
                } else {
                    world.add_default(root, "Pace").expect("a pacer");
                }
            } else {
                world.insert(root, cutting()).expect("alive");
                self.cutting.push(root);
            }
        }
        solid(world, Vec3::new(0.0, -0.5, 0.0), Vec3::new(14.0, 1.0, 14.0));
        for (x, z) in [(-5.5, -5.5), (5.5, -5.5), (-5.5, 5.5), (5.5, 5.5)] {
            solid(world, Vec3::new(x, 1.0, z), Vec3::new(1.5, 2.0, 1.5));
        }
        // Between the ends, a gap past each of its own, so that every path turns its corners.
        solid(world, Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.4, 2.0, 8.0));
        let volume = NavMeshVolume {
            size: Vec3::new(16.0, 4.0, 16.0),
            ..NavMeshVolume::default()
        };
        world.spawn((Transform::default(), volume));
        for i in 0..WALKERS {
            let z = (i / 2) as f32 * 0.4 - 2.2;
            let (from, to) = if i % 2 == 0 { (-3.0, 3.0) } else { (3.0, -3.0) };
            let placed = Transform {
                translation: DVec3::new(from, 0.0, f64::from(z)),
                ..Transform::default()
            };
            let agent = NavAgent {
                destination: Some(Vec3::new(to, 0.0, z)),
                speed: 4.0,
                radius: 0.3,
                ..NavAgent::default()
            };
            world.spawn((placed, agent));
        }
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        for command in self.commands.drain(..) {
            match command {
                Command::Speed(speed) => self.speed = speed,
            }
        }
        let speed = self.speed;
        world
            .query_mut::<(&Velocity, &mut Transform)>()
            .par_for_each(|(velocity, transform)| {
                transform.translation += (velocity.0 * speed).as_dvec3();
                if on_a_worker() {
                    ON_WORKERS.fetch_add(1, Ordering::Relaxed);
                }
            });
        for (_, velocity) in world.query_mut::<(&Heavy, &mut Velocity)>() {
            velocity.0 = -velocity.0;
        }
        run_scripts::<Ephemeral>(world, tick);
        let between = (tick.number % 128) as f32 / 128.0;
        for &fox in &self.blending {
            let set = world.set_param(fox, "speed", 2.0 + 2.5 * between);
            set.expect("a float");
        }
        if tick.number.is_multiple_of(HOPS) {
            for &fox in &self.cutting {
                world.set_param(fox, "hop", true).expect("a trigger");
            }
        }
        self.scripts.run(world);
        step_animation(world);
        for agent in world.query_mut::<&mut NavAgent>() {
            if let (true, Some(to)) = (agent.arrived, agent.destination) {
                agent.destination = Some(Vec3::new(-to.x, to.y, to.z));
                self.turned += 1;
            }
            self.corners = self.corners.max(agent.corners.len());
        }
        step_navigation(world);
    }
}

/// Runs `count` ticks from tick `number`, queuing a command every 8th, and hashing the world
/// after each.
fn ticks(movers: &mut Movers, cx: &mut Context, number: &mut u64, count: u64) {
    for _ in 0..count {
        if number.is_multiple_of(8) {
            let speed = (*number % 5) as f32;
            movers.commands.push(Command::Speed(speed));
        }
        movers.tick(cx.world_mut(), Tick { number: *number });
        black_box(cx.world().hash());
        *number += 1;
    }
}

/// Parents, each with a child, of which each tick despawns the oldest `REPLACED` and spawns as many
/// anew in their place.
struct Churn {
    roots: VecDeque<Entity>,
}

impl Churn {
    fn new(world: &mut World) -> Self {
        let mut churn = Self {
            roots: VecDeque::with_capacity(SUBTREES),
        };
        for _ in 0..SUBTREES {
            churn.spawn(world);
        }
        churn
    }

    fn spawn(&mut self, world: &mut World) {
        let root = world.spawn(Transform::default());
        let child = world.spawn(Transform::default());
        world.set_parent(child, Some(root)).expect("both alive");
        self.roots.push_back(root);
    }

    fn tick(&mut self, world: &mut World) {
        for _ in 0..REPLACED {
            let oldest = self.roots.pop_front().expect("a full pool");
            world.despawn(oldest).expect("alive");
            self.spawn(world);
        }
    }
}

/// Has the ticks' thread wait 100 ms, with no job left to take, for rows another thread holds: long
/// enough for it to find none and sleep, as it does only when a wait drags on, which ticks this
/// quick can put off for a long while, though a run's ticks come to one at some point. The pool's
/// other threads sleep at the end of every tick. The ticks' thread holds its rows until another
/// thread has taken one, since rows this quick it could otherwise finish before a sleeping thread
/// woke to take any, as on one or two cores.
fn sleep_the_ticks_thread(world: &mut World) {
    let held = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(10);
    world.query_mut::<&Velocity>().par_for_each(|_| {
        if !on_a_worker() {
            while !held.load(Ordering::Relaxed) && Instant::now() < deadline {
                thread::yield_now();
            }
        } else if !held.swap(true, Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
    });
    assert!(
        held.into_inner(),
        "no row ran on the job system's other threads in 10 s"
    );
}

/// Checks that what the job system's other threads allocate is counted: that a pass whose rows
/// allocate once each on those threads counts as many allocations as such rows at least. The
/// ticks' thread holds its rows until one has run on another thread, since rows this quick it could
/// otherwise finish before a sleeping thread woke to take any, as on one or two cores.
fn workers_are_counted(world: &mut World) {
    let elsewhere = AtomicU64::new(0);
    let deadline = Instant::now() + Duration::from_secs(10);
    let counted = allocations(|| {
        world.query_mut::<&Velocity>().par_for_each(|_| {
            if on_a_worker() {
                elsewhere.fetch_add(1, Ordering::Relaxed);
                black_box(Box::new(0u8));
            } else {
                while elsewhere.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                    thread::yield_now();
                }
            }
        });
    })
    .allocations;
    let elsewhere = elsewhere.into_inner();
    assert!(
        elsewhere > 0,
        "no row ran on the job system's other threads in 10 s"
    );
    assert!(
        counted >= elsewhere,
        "counted {counted} allocations of the {elsewhere} rows that allocated on the job system's \
         other threads: the counter misses those threads"
    );
}

fn main() {
    if gg_engine::profile::ENABLED {
        panic!(
            "Tracy's client allocates as it records; run the allocation test without the tracy \
             feature"
        );
    }
    let mut cx = Context::seeded(0x5eed);
    let mut movers = Movers {
        commands: Vec::new(),
        speed: 1.0,
        scripts: Scripts::new().expect("Luau starts"),
        fox: None,
        blending: Vec::new(),
        cutting: Vec::new(),
        turned: 0,
        corners: 0,
    };
    movers.load(&mut cx);
    movers.attach(&mut cx);
    let mut number = 0;
    for threads in [1, 4] {
        if threads > 1 {
            let threads = NonZeroUsize::new(threads).expect("nonzero");
            gg_engine::start_jobs(threads).expect("the job system's threads start");
            sleep_the_ticks_thread(cx.world_mut());
        }
        ticks(&mut movers, &mut cx, &mut number, WARM_UP);
        let on = if threads == 1 {
            "one thread"
        } else {
            "four threads"
        };
        ON_WORKERS.store(0, Ordering::Relaxed);
        let turned = movers.turned;
        movers.corners = 0;
        let counted = allocations(|| ticks(&mut movers, &mut cx, &mut number, COUNTED));
        assert_eq!(
            counted.allocations, 0,
            "{COUNTED} ticks on {on} allocated {counted}"
        );
        // So that the count covers paths planned at the ticks it counts, round the wall's ends.
        assert!(
            movers.turned > turned,
            "no agent was sent back in the {COUNTED} ticks on {on}"
        );
        assert!(
            movers.corners >= 3,
            "the paths of the {COUNTED} ticks on {on} had at most {} corners",
            movers.corners
        );
        let on_workers = ON_WORKERS.load(Ordering::Relaxed);
        if threads > 1 {
            // So that the count covers ticks whose thread waited for jobs the others took.
            assert!(
                on_workers > 0,
                "no row of the {COUNTED} ticks on {on} ran on the job system's other threads"
            );
        }
        println!(
            "{COUNTED} ticks on {on} allocated nothing, {on_workers} of their rows on the job \
             system's other threads"
        );
    }
    // The Luau script ran at every tick, its counts say.
    let world = cx.world();
    let drifter = world.entities(Some("Drift")).expect("declared").next();
    let ran = world.get_field(drifter.expect("a drifter"), "Drift", "ticks");
    assert_eq!(ran.expect("its count"), Value::F64(number as f64));
    // The foxes blended, paced by the layer and by their script, and hopped as they went.
    let machines = world.query::<&AnimationStates>();
    let states: Vec<&str> = machines.map(|machine| machine.state.as_str()).collect();
    let count = |state| states.iter().filter(|&&of| of == state).count();
    // The cutting ones 16 ticks into a hop, of the 38 that Run's 1.16 seconds take at twice its
    // speed.
    assert_eq!(number % HOPS, 16);
    let counted = (count("Move"), count("Hop"));
    assert_eq!(counted, (BLENDING as usize, CUTTING as usize), "{states:?}");
    let pacer = world.entities(Some("Pace")).expect("declared").next();
    let paced = world.get_field(pacer.expect("a pacer"), "Pace", "time");
    let seconds = number as f64 / 64.0;
    assert_eq!(paced.expect("its time"), Value::F64(seconds));
    workers_are_counted(cx.world_mut());
    println!("allocations on the job system's other threads count");

    // A turn of the pool first, as a run's first ticks of churn take what its tables keep.
    let mut churn = Churn::new(cx.world_mut());
    for _ in 0..SUBTREES / REPLACED {
        churn.tick(cx.world_mut());
    }
    let counted = allocations(|| {
        for _ in 0..CHURNED {
            churn.tick(cx.world_mut());
        }
    });
    assert_eq!(
        counted.allocations, 0,
        "{CHURNED} ticks that each replaced {REPLACED} of {SUBTREES} parents and their children \
         allocated {counted}"
    );
    println!(
        "{CHURNED} ticks that each replaced {REPLACED} of {SUBTREES} parents and their children \
         allocated nothing"
    );
}
