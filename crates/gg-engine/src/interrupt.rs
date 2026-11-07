//! Ctrl+C, which ends a headless run or a server between frames, as it ends at `--frames`, rather
//! than the process at once: through the console's handler on Windows, and `SIGINT`'s on Unix. A
//! second before the run has ended ends the process, as the OS would.
//!
//! And, under `--supervised`, stdin's end, which ends any run but a replay so, windowed too: the
//! supervisor that holds stdin, such as `gg run`, closes it as it ends, however it ends, being
//! killed too, so that no game it ran, which it may embed out of sight, outlives it.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use tracing::warn;

/// Set once Ctrl+C has been pressed.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Set once stdin has ended, under `--supervised`.
static ORPHANED: AtomicBool = AtomicBool::new(false);

/// Has a thread of its own read stdin to its end, then set [`orphaned`] and call `wake`, which
/// wakes a loop that waits for events; warns `engine.supervised` if the thread can't start.
pub(crate) fn watch_stdin(wake: impl FnOnce() + Send + 'static) {
    let watching = thread::Builder::new()
        .name("gg-stdin".to_owned())
        .spawn(move || {
            // A supervisor writes nothing, and a read that breaks is as good as the end.
            let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
            ORPHANED.store(true, Ordering::Relaxed);
            wake();
        });
    if let Err(error) = watching {
        warn!(
            code = "engine.supervised",
            "Can't watch stdin, so the run outlives its supervisor: {error}"
        );
    }
}

/// Whether stdin has ended since [`watch_stdin`].
pub(crate) fn orphaned() -> bool {
    ORPHANED.load(Ordering::Relaxed)
}

/// Has Ctrl+C set [`interrupted`] from now on, rather than end the process; warns
/// `engine.interrupt` if the OS won't have it.
pub(crate) fn watch() {
    #[cfg(target_os = "windows")]
    // SAFETY: `on_control` is a handler routine as `SetConsoleCtrlHandler` takes one, which only
    // touches an atomic, on whichever thread the OS calls it.
    let watched = unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_control), 1) != 0
    };
    #[cfg(unix)]
    // SAFETY: `on_interrupt` does only what a signal's handler may: it touches an atomic, and calls
    // `_exit`, which is async-signal-safe.
    let watched = unsafe {
        let handler = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler) != libc::SIG_ERR
    };
    if !watched {
        warn!(
            code = "engine.interrupt",
            "Can't take Ctrl+C, which ends the process at once, without the run's end"
        );
    }
}

/// Whether Ctrl+C has been pressed since [`watch`].
pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// The console's handler: takes the first Ctrl+C or Ctrl+Break, and leaves the second, and any
/// other event, as a window's closing, to the OS's, which ends the process.
#[cfg(target_os = "windows")]
unsafe extern "system" fn on_control(event: u32) -> windows_sys::Win32::Foundation::BOOL {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};

    let taken = matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT)
        && !INTERRUPTED.swap(true, Ordering::Relaxed);
    i32::from(taken)
}

/// `SIGINT`'s handler: takes the first, and ends the process at the second, with the status a
/// shell gives a process `SIGINT` ended.
#[cfg(unix)]
extern "C" fn on_interrupt(_: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::Relaxed) {
        // SAFETY: `_exit` is async-signal-safe, and ends the process without running anything of
        // its own.
        unsafe { libc::_exit(130) }
    }
}
