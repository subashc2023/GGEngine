//! The main thread's wait for the render thread, on Windows, which answers what other threads send
//! the main thread's windows meanwhile. Vulkan's swapchain calls, acquiring and presenting among
//! them, may send the window's thread a message and wait for its answer, so a main thread that only
//! waited could wait for a render thread waiting for it: the Vulkan spec's WSI chapter warns that
//! the window's thread must keep answering
//! (<https://docs.vulkan.org/spec/latest/chapters/VK_KHR_surface/wsi.html>).
//!
//! The answers run winit's window procedure inside the frame that waits, which is safe: winit holds
//! back the events they make until the frame's handler returns. A script's stop waits longer, and
//! takes what's posted to the windows off the queue too, as [`pump`] says.

use std::mem;
use std::sync::mpsc::{Receiver, RecvError, RecvTimeoutError};
use std::time::Duration;

#[cfg(feature = "dev")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PM_REMOVE, PostQuitMessage, TranslateMessage, WM_PAINT, WM_QUIT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MSG, PM_NOREMOVE, PM_QS_SENDMESSAGE, PeekMessageW,
};

/// Waits for what `from` receives, as [`Receiver::recv`] does, and each time a millisecond's wait
/// for it times out, answers the messages other threads have sent the calling thread's windows:
/// those alone, so that the rest wait for the event loop, as they would. A timed wait ends on
/// Windows' timer tick, about 16 ms unless the process asks for less, so an answer can take that
/// long; what's received wakes the wait at once.
pub(crate) fn recv<T>(from: &Receiver<T>) -> Result<T, RecvError> {
    loop {
        match from.recv_timeout(Duration::from_millis(1)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Disconnected) => return Err(RecvError),
            Err(RecvTimeoutError::Timeout) => answer_sent_messages(),
        }
    }
}

/// Answers the messages other threads have sent the calling thread's windows, taking nothing off
/// its queue.
fn answer_sent_messages() {
    // SAFETY: an empty message for `PeekMessageW` to fill.
    let mut msg: MSG = unsafe { mem::zeroed() };
    // SAFETY: a plain call for any of the thread's windows, which dispatches the messages sent to
    // them and, with `PM_NOREMOVE`, leaves the queue as it was.
    unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE) };
}

/// Answers the messages other threads have sent the calling thread's windows, and dispatches those
/// posted to them, at most `most`, as a modal loop does, so that Windows takes the windows for ones
/// that respond while a script's stop waits, as `luau::Debugger` says, rather than marking them
/// hung after five seconds, as it does a thread that answers sent messages alone. winit holds back
/// the events they make until the frame's handler returns. A repaint, which comes once nothing else
/// is queued, stays queued for the event loop: winit asks for another as it can't draw meanwhile,
/// which would come again at once. A quit is posted again, for the event loop. Development builds
/// alone debug scripts.
#[cfg(feature = "dev")]
pub(crate) fn pump(most: usize) {
    for _ in 0..most {
        // SAFETY: an empty message for `PeekMessageW` to fill.
        let mut msg: MSG = unsafe { mem::zeroed() };
        // SAFETY: a plain call for any of the thread's windows, which dispatches the messages sent
        // to them and takes the next posted to them off the queue, but a repaint, which stays.
        if unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) } == 0 || msg.message == WM_PAINT {
            return;
        }
        if msg.message == WM_QUIT {
            // SAFETY: a plain call, posting the quit taken again, with its exit code.
            unsafe { PostQuitMessage(msg.wParam as i32) };
            return;
        }
        // SAFETY: the message `PeekMessageW` filled, to the window it's for, as a message loop
        // dispatches it.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::mpsc;
    use std::thread;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SMTO_BLOCK,
        SendMessageTimeoutW, WM_NULL, WNDCLASSW,
    };

    use super::*;

    #[test]
    fn a_wait_answers_messages_sent_to_the_thread_s_windows() {
        let class: Vec<u16> = "gg windows wait test\0".encode_utf16().collect();
        // SAFETY: a hidden window of this thread, of a class whose procedure is Windows' default;
        // the class may be registered already.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(ptr::null());
            RegisterClassW(&WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: ptr::null(),
                lpszClassName: class.as_ptr(),
            });
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                100,
                100,
                0,
                0,
                instance,
                ptr::null(),
            )
        };
        assert_ne!(hwnd, 0);
        let (answered, answers) = mpsc::sync_channel(1);
        // As a driver's swapchain call does from the render thread.
        let sender = thread::spawn(move || {
            let mut result = 0;
            // SAFETY: a message that does nothing, to another thread's window, which has 10 s to
            // answer it.
            let sent = unsafe {
                SendMessageTimeoutW(hwnd, WM_NULL, 0, 0, SMTO_BLOCK, 10_000, &mut result)
            };
            answered.send(sent != 0).unwrap();
        });
        assert_eq!(
            recv(&answers),
            Ok(true),
            "the message wasn't answered while this thread waited"
        );
        sender.join().unwrap();
        // SAFETY: the window is this thread's.
        unsafe { DestroyWindow(hwnd) };
    }
}
