//! The main thread's wait for the render thread, on Windows, answering what other threads send
//! its windows meanwhile. Vulkan's swapchain calls (acquire, present) may send the window's
//! thread a message and wait for the answer, so a main thread that only waited could deadlock
//! with a render thread waiting for it. The Vulkan spec's WSI chapter warns the window's thread
//! must keep answering
//! (<https://docs.vulkan.org/spec/latest/chapters/VK_KHR_surface/wsi.html>).
//!
//! The answers run winit's window procedure inside the waiting frame, which is safe: winit holds
//! back the events they make until the frame's handler returns. A script's stop also takes
//! posted messages off the queue, as [`pump`] says.

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

/// Like [`Receiver::recv`], but each 1 ms timeout answers the messages other threads sent the
/// calling thread's windows; the rest wait for the event loop. A timed wait ends on Windows' timer
/// tick, about 16 ms unless the process asks for less, so an answer can take that long; a
/// received value wakes the wait at once.
pub(crate) fn recv<T>(from: &Receiver<T>) -> Result<T, RecvError> {
    loop {
        match from.recv_timeout(Duration::from_millis(1)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Disconnected) => return Err(RecvError),
            Err(RecvTimeoutError::Timeout) => answer_sent_messages(),
        }
    }
}

/// Answers sent messages; takes nothing off the queue.
fn answer_sent_messages() {
    // SAFETY: an empty message for `PeekMessageW` to fill.
    let mut msg: MSG = unsafe { mem::zeroed() };
    // SAFETY: a plain call for any of the thread's windows; it dispatches the messages sent to
    // them and, with `PM_NOREMOVE`, leaves the queue as it was.
    unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE) };
}

/// Answers sent messages and dispatches up to `most` posted ones, as a modal loop does. So while
/// a script's stop waits (see `luau::Debugger`), Windows takes the windows as responsive instead
/// of marking them hung after five seconds, as it does a thread answering sent messages alone.
/// winit holds back the events they make until the frame's handler returns.
///
/// A repaint, which comes once nothing else is queued, stays queued for the event loop: winit
/// asks for another as it can't draw meanwhile, so it would come again at once. A quit is posted
/// again for the event loop. Only development builds debug scripts.
#[cfg(feature = "dev")]
pub(crate) fn pump(most: usize) {
    for _ in 0..most {
        // SAFETY: an empty message for `PeekMessageW` to fill.
        let mut msg: MSG = unsafe { mem::zeroed() };
        // SAFETY: a plain call for any of the thread's windows; it dispatches the messages sent to
        // them and takes the next posted one off the queue, except a repaint, which stays.
        if unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) } == 0 || msg.message == WM_PAINT {
            return;
        }
        if msg.message == WM_QUIT {
            // SAFETY: a plain call, posting the quit again with its exit code.
            unsafe { PostQuitMessage(msg.wParam as i32) };
            return;
        }
        // SAFETY: the message `PeekMessageW` filled, dispatched to its window as a message loop
        // does.
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
        // SAFETY: a hidden window of this thread, of a class with Windows' default procedure; the
        // class may be registered already.
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
            // SAFETY: a no-op message to another thread's window, which has 10 s to answer.
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
