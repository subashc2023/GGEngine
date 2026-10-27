//! The job system: rayon-core's pool of threads, which a tick's systems hand their rows to through
//! [`join`], with the thread that runs the ticks as the pool's first thread, so that it works
//! through the jobs too as it waits for them. Outside the pool, as on tests' threads and in a run
//! on one thread, jobs run one after another on the thread that calls.

use std::num::NonZeroUsize;

use gg_core::{Error, Result};

/// Starts the pool of `threads` threads, the calling thread, which runs the ticks, as its first;
/// with one thread, starts none. Fails with `engine.threads` if the threads can't start, or a pool
/// was started before in the process.
///
/// Each thread allocates the first time it looks for a job to steal, as crossbeam-epoch records
/// it, and on macOS the first time it sleeps for want of one, as std there makes the mutex and the
/// condition variable it sleeps on: the pool's other threads in its first ticks, and the calling
/// thread at the first tick that keeps it waiting long, which may come at any time. rayon-core
/// doesn't say when a thread has slept, so nothing here can make them sooner.
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
