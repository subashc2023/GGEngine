//! The job system: rayon-core's pool of threads, which a tick's systems hand their rows to through
//! [`join`], with the thread that runs the ticks as the pool's first thread, so that it works
//! through the jobs too as it waits for them. Outside the pool, as on tests' threads and in a run
//! on one thread, jobs run one after another on the thread that calls.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use gg_core::{Error, Result};

/// Starts the pool of `threads` threads, the calling thread, which runs the ticks, as its first;
/// with one thread, starts none. Fails with `engine.threads` if the threads can't start, or a pool
/// was started before in the process.
pub fn start(threads: NonZeroUsize) -> Result<()> {
    if threads.get() == 1 {
        return Ok(());
    }
    let _zone = gg_core::zone!();
    rayon_core::ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .use_current_thread()
        // Within the 15 bytes Linux keeps of a thread's name.
        .thread_name(|index| format!("gg-job-{index}"))
        .build_global()
        .map_err(|error| {
            Error::new(
                "engine.threads",
                format!("can't start {threads} threads for the jobs: {error}"),
            )
        })?;
    warm_up();
    Ok(())
}

/// Has the calling thread, the pool's first, wait once for a job another thread took, and every
/// thread find no job and sleep, so that what a thread allocates the first time it waits is
/// allocated now rather than at some tick: crossbeam-epoch's record of the thread, made as it first
/// looks for a job to steal, and, on macOS, the mutex and the condition variable it sleeps on,
/// which std's there allocate as they're first used. Those are rayon-core's internals, so the
/// `ticks` test checks that ticks allocate nothing after this.
fn warm_up() {
    let taken = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    // A pool whose threads never take the job runs it here once this passes.
    let deadline = Instant::now() + Duration::from_secs(1);
    rayon_core::join(
        || {
            while !taken.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::yield_now();
            }
            done.store(true, Ordering::Release);
        },
        || {
            taken.store(true, Ordering::Release);
            while !done.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::yield_now();
            }
            // Long enough for the waiting thread to find no job to steal and sleep.
            thread::sleep(Duration::from_millis(20));
        },
    );
    // The others slept while `b` held: long enough for the thread that ran it to sleep too.
    thread::sleep(Duration::from_millis(20));
}

/// Runs `a` and `b`: on a thread of the pool, `b` is offered to its other threads while this one
/// runs `a`, and runs here after `a` if none took it; on any other thread, `a` then `b` run on it,
/// in that order.
pub(crate) fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    if rayon_core::current_thread_index().is_some() {
        rayon_core::join(a, b)
    } else {
        // Not through rayon-core's own pool, which a call from outside every pool would start.
        (a(), b())
    }
}

/// Runs `f` on a pool of `threads` threads of its own, for tests that run the simulation on more
/// than one: a thread of it runs `f`, whose jobs it joins as the ticks' thread does. Each call
/// starts its threads, and allocates.
#[doc(hidden)]
pub fn on_threads<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    assert!(threads > 0, "a pool has a thread at least");
    rayon_core::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|index| format!("gg-job-{index}"))
        .build()
        .expect("a pool's threads start")
        .install(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_pool_join_runs_a_then_b_on_the_caller() {
        let caller = thread::current().id();
        let order = std::sync::Mutex::new(Vec::new());
        join(
            || order.lock().unwrap().push(("a", thread::current().id())),
            || order.lock().unwrap().push(("b", thread::current().id())),
        );
        assert_eq!(*order.lock().unwrap(), [("a", caller), ("b", caller)]);
    }

    #[test]
    fn in_a_pool_join_runs_a_and_b_on_two_threads() {
        let (a, b) = on_threads(2, || {
            let started = AtomicBool::new(false);
            let deadline = Instant::now() + Duration::from_secs(10);
            join(
                || {
                    // Only another thread can start `b` while this waits.
                    while !started.load(Ordering::Acquire) {
                        assert!(Instant::now() < deadline, "b never started elsewhere");
                        thread::yield_now();
                    }
                    thread::current().id()
                },
                || {
                    started.store(true, Ordering::Release);
                    thread::current().id()
                },
            )
        });
        assert_ne!(a, b);
    }
}
