//! Steady-state ticks allocate nothing, on any thread: a layer's ticks that apply its commands,
//! run a system on the job system's threads and another on the ticks' thread alone, and hash the
//! world, as a recorded run's do, once their first ticks have run; before the job system's threads
//! start, as in a run on one thread, and after, from the first tick after, since starting them
//! makes what they keep.
//!
//! It runs in one process with a main of its own, so that no other test's threads run beside it,
//! and counts every allocation the process makes while the ticks run, whichever thread makes it.

// For the allocator that counts; see the SAFETY comments.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use gg_engine::math::Vec3;
use gg_engine::{Component, Context, Layer, Tick, Transform, World};

/// The ticks run before counting, as a run's first ticks size what the world and the layer keep.
const WARM_UP: u64 = 64;
/// The ticks counted.
const COUNTED: u64 = 1_000;

/// System's allocator, which counts the allocations made while `COUNTING` is set.
struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

fn count() {
    if COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: each method passes what it's given to System's, so it keeps System's contract; counting
// touches only atomics, which don't allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `alloc`'s contract, which is System's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `alloc_zeroed`'s contract, which is System's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        // SAFETY: the caller keeps `realloc`'s contract: `ptr` is System's, from `layout`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller keeps `dealloc`'s contract: `ptr` is System's, from `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The allocations the process makes while `f` runs, on every thread.
fn allocations(f: impl FnOnce()) -> u64 {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    COUNTING.store(true, Ordering::SeqCst);
    f();
    COUNTING.store(false, Ordering::SeqCst);
    ALLOCATIONS.load(Ordering::SeqCst)
}

#[derive(Clone, Component)]
struct Velocity(Vec3);

/// On a third of the movers, so that they're two tables.
#[derive(Clone, Component)]
struct Heavy;

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
/// job system's threads, and whose heavy ones it turns back on the ticks' thread alone.
struct Movers {
    commands: Vec<Command>,
    speed: f32,
}

impl Layer for Movers {
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
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
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

/// Checks that what the job system's other threads allocate is counted: that a pass whose rows
/// allocate once each on those threads counts as many allocations as such rows at least, trying
/// again until some rows run there.
fn workers_are_counted(world: &mut World) {
    for _ in 0..100 {
        let elsewhere = AtomicU64::new(0);
        let counted = allocations(|| {
            world.query_mut::<&Velocity>().par_for_each(|_| {
                if on_a_worker() {
                    elsewhere.fetch_add(1, Ordering::Relaxed);
                    black_box(Box::new(0u8));
                }
            });
        });
        let elsewhere = elsewhere.into_inner();
        if elsewhere > 0 {
            assert!(
                counted >= elsewhere,
                "counted {counted} allocations of the {elsewhere} rows that allocated on the job \
                 system's other threads: the counter misses those threads"
            );
            return;
        }
    }
    panic!("no row ran on the job system's other threads in 100 passes");
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
    };
    movers.attach(&mut cx);
    let mut number = 0;
    ticks(&mut movers, &mut cx, &mut number, WARM_UP);
    for threads in [1, 4] {
        if threads > 1 {
            let threads = NonZeroUsize::new(threads).expect("nonzero");
            gg_engine::start_jobs(threads).expect("the job system's threads start");
        }
        let on = if threads == 1 {
            "one thread"
        } else {
            "four threads"
        };
        ON_WORKERS.store(0, Ordering::Relaxed);
        let counted = allocations(|| ticks(&mut movers, &mut cx, &mut number, COUNTED));
        assert_eq!(
            counted, 0,
            "{COUNTED} ticks on {on} allocated {counted} times"
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
    workers_are_counted(cx.world_mut());
    println!("allocations on the job system's other threads count");
}
