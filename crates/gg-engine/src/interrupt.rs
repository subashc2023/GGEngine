//! Ctrl+C ends a headless run or a server between frames, as `--frames` does, not the process at
//! once: via the console's handler on Windows, `SIGINT`'s on Unix. A second Ctrl+C before the run
//! ends ends the process, as the OS would.
//!
//! Under `--supervised`, stdin's end also ends any run but a replay, windowed too. A supervisor
//! holding stdin (such as `gg run`) closes it however it ends, even killed, so no game it ran
//! (possibly embedded out of sight) outlives it.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use tracing::warn;

/// Set once Ctrl+C has been pressed.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Set once stdin has ended, under `--supervised`.
static ORPHANED: AtomicBool = AtomicBool::new(false);

/// Reads stdin to its end on a thread, then sets [`orphaned`] and calls `wake` (to wake a loop
/// waiting for events). Warns `engine.supervised` if the thread can't start.
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

/// Makes Ctrl+C set [`interrupted`] instead of ending the process. Warns `engine.interrupt` if the
/// OS refuses.
pub(crate) fn watch() {
    #[cfg(target_os = "windows")]
    // SAFETY: `on_control` is a valid handler routine. It only touches an atomic, so any thread
    // may call it.
    let watched = unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_control), 1) != 0
    };
    #[cfg(unix)]
    // SAFETY: `on_interrupt` does only what a signal handler may: it touches an atomic and calls
    // the async-signal-safe `_exit`.
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

/// Takes the first Ctrl+C or Ctrl+Break. The second, and other events (a window closing), go to
/// the OS's handler, which ends the process.
#[cfg(target_os = "windows")]
unsafe extern "system" fn on_control(event: u32) -> windows_sys::Win32::Foundation::BOOL {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};

    let taken = matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT)
        && !INTERRUPTED.swap(true, Ordering::Relaxed);
    i32::from(taken)
}

/// Takes the first `SIGINT`. The second ends the process with the status a shell reports for it.
#[cfg(unix)]
extern "C" fn on_interrupt(_: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::Relaxed) {
        // SAFETY: `_exit` is async-signal-safe and runs no destructors or handlers.
        unsafe { libc::_exit(130) }
    }
}
