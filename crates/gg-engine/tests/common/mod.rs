//! The allocation tests' allocator: System's, counting every allocation the process makes through
//! Rust's allocator between [`start`] and [`stop`], on any thread, and keeping where the first was
//! made.

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

/// System's allocator, counting allocations while `COUNTING` is set.
struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
/// The first counted allocation's backtrace; its frames say which thread made it.
static FIRST: Mutex<Option<Backtrace>> = Mutex::new(None);

thread_local! {
    /// Set while the thread counts an allocation, since capturing a backtrace allocates too.
    static IN_COUNT: Cell<bool> = const { Cell::new(false) };
}

fn count() {
    if !COUNTING.load(Ordering::Relaxed) || IN_COUNT.with(|in_count| in_count.replace(true)) {
        return;
    }
    // No backtrace while panicking (the panic hook captures one too), and no `thread::current`:
    // std aborts if its own first allocation comes here. The backtrace's frames say which thread.
    if ALLOCATIONS.fetch_add(1, Ordering::Relaxed) == 0 && !thread::panicking() {
        let first = Backtrace::force_capture();
        if let Ok(mut kept) = FIRST.lock() {
            kept.get_or_insert(first);
        }
    }
    IN_COUNT.with(|in_count| in_count.set(false));
}

// SAFETY: each method passes what it's given to System's, so it keeps System's contract; counting
// touches atomics and a thread-local flag, and allocates only through this allocator again, which
// the flag keeps from counting.
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

/// Counts from none.
pub fn start() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    *FIRST.lock().expect("no thread panicked holding it") = None;
    COUNTING.store(true, Ordering::SeqCst);
}

/// Stops counting and returns the count.
pub fn stop() -> Counted {
    COUNTING.store(false, Ordering::SeqCst);
    Counted {
        allocations: ALLOCATIONS.load(Ordering::SeqCst),
        first: FIRST.lock().expect("no thread panicked holding it").take(),
    }
}

/// The allocations counted between [`start`] and [`stop`], and the first's backtrace unless it was
/// made while panicking.
pub struct Counted {
    pub allocations: u64,
    first: Option<Backtrace>,
}

/// For a failure message: how many times, and where the first was made.
impl fmt::Display for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} times; the first, from", self.allocations)?;
        match &self.first {
            Some(first) => write!(f, "{first}"),
            None => f.write_str("none kept"),
        }
    }
}
