//! Steady-state ticks allocate nothing, on any thread: a layer's ticks that apply its commands,
//! run a system on the job system's threads and another on the ticks' thread alone, and hash the
//! world, as a recorded run's do, once their first ticks have run, with scripts that despawn their
//! own entities and spawn others in their place among them, and a Luau script that moves its
//! entities and counts its ticks in its own component; before the job system's threads start,
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{fs, process, thread};

use gg_engine::math::Vec3;
use gg_engine::reflect::Value;
use gg_engine::{
    Component, Context, Entity, Layer, Script, Scripts, Tick, Transform, World, run_scripts,
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
/// job system's threads, and whose heavy ones it turns back on the ticks' thread alone; and
/// `EPHEMERA` scripted entities, and `DRIFTERS` that a Luau script moves, whose scripts it runs.
struct Movers {
    commands: Vec<Command>,
    speed: f32,
    scripts: Scripts,
}

impl Layer for Movers {
    fn load(&mut self, cx: &mut Context) {
        let path = std::env::temp_dir().join(format!("gg-ticks-{}.luau", process::id()));
        fs::write(&path, DRIFT).expect("the script is written");
        let loaded = self.scripts.load(cx, &path);
        let _ = fs::remove_file(&path);
        loaded.unwrap_or_else(|error| panic!("{error:#}"));
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
                transform.translation += velocity.0 * speed;
                if on_a_worker() {
                    ON_WORKERS.fetch_add(1, Ordering::Relaxed);
                }
            });
        for (_, velocity) in world.query_mut::<(&Heavy, &mut Velocity)>() {
            velocity.0 = -velocity.0;
        }
        run_scripts::<Ephemeral>(world, tick);
        self.scripts.run(world);
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
        let counted = allocations(|| ticks(&mut movers, &mut cx, &mut number, COUNTED));
        assert_eq!(
            counted.allocations, 0,
            "{COUNTED} ticks on {on} allocated {counted}"
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
