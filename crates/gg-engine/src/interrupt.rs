//! Ctrl+C, which ends a headless run or a server between frames, as it ends at `--frames`, rather
//! than the process at once: through the console's handler on Windows, and `SIGINT`'s on Unix. A
//! second before the run has ended ends the process, as the OS would.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

/// Set once Ctrl+C has been pressed.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

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
