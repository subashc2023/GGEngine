//! The job system: rayon-core's thread pool, which a tick's systems hand rows to through
//! `join`. The ticks' thread is the pool's first, so it runs jobs while it waits. Outside the
//! pool (tests' threads, a one-thread run) jobs run in turn on the calling thread.

use std::num::NonZeroUsize;
use std::thread;
use std::time::Duration;

use gg_core::{Error, Result};

/// Starts a pool of `threads` threads, the calling thread (which runs the ticks) the first.
///
/// With one thread, it starts none. Fails with `engine.threads` if the threads can't start, or a
/// pool was started before in the process.
///
/// Returns with every thread warm, so steady frames allocate nothing. Each thread allocates the
/// first time it looks for a job to steal (crossbeam-epoch records it), and on macOS the first
/// time it sleeps for want of one (std makes the mutex and condition variable then).
/// Both happen here, before it returns.
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

/// Makes each of the pool's threads steal and then sleep once.
///
/// The calling thread, the pool's first, runs its share of a broadcast and waits for the others,
/// which sleep in theirs. A waiting thread sleeps after 33 rounds of looking
/// (`ROUNDS_UNTIL_SLEEPING` in rayon-core), well within the others' wait. Once they finish they idle the same
/// way, so a second wait lets them sleep.
fn warm_up() {
    let _zone = gg_core::zone!();
    const BUSY: Duration = Duration::from_millis(10);
    rayon_core::broadcast(|context| {
        // Index 0 is the calling thread, taken over by `use_current_thread`.
        if context.index() != 0 {
            thread::sleep(BUSY);
        }
    });
    thread::sleep(BUSY);
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

/// `f` of each of `items`, in their order: halves offered to the pool's other threads, down to an
/// item a job. Outside a pool, in turn on the calling thread.
pub fn in_order<T: Send, R: Send>(mut items: Vec<T>, f: &(impl Fn(T) -> R + Sync)) -> Vec<R> {
    if items.len() < 2 {
        return items.into_iter().map(f).collect();
    }
    let rest = items.split_off(items.len() / 2);
    let (mut done, rest) = join(|| in_order(items, f), || in_order(rest, f));
    done.extend(rest);
    done
}

/// Whether the calling thread is a pool's.
///
/// There [`join`] offers work to the pool's other threads, and runs theirs while it waits.
pub(crate) fn on_pool() -> bool {
    rayon_core::current_thread_index().is_some()
}

/// Offers `f` to the pool's other threads and returns at once. Outside a pool, as [`join`] says,
/// does nothing.
pub(crate) fn spawn(f: impl FnOnce() + Send + 'static) {
    if rayon_core::current_thread_index().is_some() {
        rayon_core::spawn(f);
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
