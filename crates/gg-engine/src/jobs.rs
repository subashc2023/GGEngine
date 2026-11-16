//! The job system: rayon-core's thread pool, which a tick's systems hand rows to through
//! [`join`]. The ticks' thread is the pool's first, so it runs jobs while it waits. Outside the
//! pool (tests' threads, a one-thread run) jobs run in turn on the calling thread.

use std::num::NonZeroUsize;

use gg_core::{Error, Result};

/// Starts a pool of `threads` threads, the calling thread (which runs the ticks) the first; with
/// one thread, starts none. Fails with `engine.threads` if the threads can't start, or a pool was
/// started before in the process.
///
/// Each thread allocates the first time it looks for a job to steal (crossbeam-epoch records it),
/// and on macOS the first time it sleeps for want of one (std makes the mutex and condition
/// variable then): the other threads in the first ticks, the calling thread at the first tick
/// that keeps it waiting long, at any time. rayon-core doesn't say when a thread has slept, so
/// nothing here can do it sooner.
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
    Ok(())
}

/// Runs `a` and `b`. On a pool thread, `b` is offered to the others while this one runs `a`, and
/// runs here after `a` if none took it. On any other thread, `a` then `b` run on it, in that order.
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
        // Not rayon-core's own pool, which a call from outside every pool would start.
        (a(), b())
    }
}

/// Runs `f` on a pool of `threads` threads of its own, for tests of multi-threaded simulation. A
/// pool thread runs `f`, joining its jobs as the ticks' thread does. Each call starts threads and
/// allocates.
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

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
