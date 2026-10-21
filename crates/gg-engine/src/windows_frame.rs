//! Windows' own frame around a window whose title bar the UI draws
//! ([`Chrome::InWindowsFrame`](crate::chrome::Chrome::InWindowsFrame)): DWM's border and shadow;
//! the bands that resize it, just outside the window and along the top of its inside, as on any
//! window; the title bar's free space as the OS's own title bar, which moves the window with
//! snapping and maximizes it at a double-click, and where a right-click opens the system menu;
//! and the maximize button as the OS's, over which Windows 11 shows Snap Layouts.
//!
//! winit has no hook for the messages that takes, so this subclasses the window procedure.
//!
//! winit takes a window without decorations to have no frame, and sizes the whole window as the
//! inside it means. The frame grows back around that inside wherever winit sizes the window this
//! way: as it opens, and as its scale changes. An inner size asked of winit after that comes out
//! a frame smaller; nothing in the engine asks for one.

use std::cell::Cell;
use std::mem;
use std::rc::Rc;

use egui::{Pos2, pos2};
use gg_core::{Error, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetWindowRect, HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IsIconic, IsZoomed, MINMAXINFO, PostMessageW, SC_MAXIMIZE, SC_RESTORE,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    WM_DPICHANGED, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONUP,
    WM_SYSCOMMAND,
};
use winit::dpi::PhysicalPosition;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::chrome::{Caption, Look};

/// Windows' frame around one window, through which the UI and the frame tell each other where
/// the title bar's parts are, and how the OS says the maximize button looks.
pub(crate) struct Frame(Rc<State>);

/// What the window procedure keeps between messages.
#[derive(Default)]
struct State {
    /// The parts of the title bar that are the OS's, in the inside's pixels.
    caption: Cell<Option<Caption>>,
    /// How thick the frame's borders are, in pixels, as of its last layout, which the band along
    /// the top of the inside is as deep as.
    border: Cell<i32>,
    /// Whether the pointer is over the maximize button, and pressed it there.
    over_maximize: Cell<bool>,
    pressed: Cell<bool>,
    /// Whether the window has asked to be told when the pointer leaves its frame.
    tracking: Cell<bool>,
    /// Where the title bar's free space was right-clicked, in the inside's pixels.
    window_menu: Cell<Option<POINT>>,
}

impl Frame {
    /// Gives `window`, which winit opened without decorations, Windows' own frame. The window
    /// keeps its corner, and its inside the size winit gave the whole window.
    pub(crate) fn install(window: &Window) -> Result<Self> {
        let installed = match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => install(handle.hwnd.get()),
            _ => None,
        };
        installed.ok_or_else(|| {
            Error::new(
                "engine.window_frame",
                "can't give the window Windows' own frame",
            )
        })
    }

    /// Tells the frame where the title bar's parts that are the OS's are, in the window's points,
    /// `ppp` pixels each.
    pub(crate) fn set_caption(&self, caption: Option<Caption>, ppp: f32) {
        self.0.caption.set(caption.map(|caption| Caption {
            drag: caption.drag.map(|rect| rect * ppp),
            maximize: caption.maximize * ppp,
        }));
    }

    /// How the OS says the maximize button looks.
    pub(crate) fn maximize(&self) -> Look {
        match (self.0.over_maximize.get(), self.0.pressed.get()) {
            (true, true) => Look::Pressed,
            (true, false) => Look::Hovered,
            (false, _) => Look::Idle,
        }
    }

    /// Where the title bar's free space was right-clicked since this was last asked, in the
    /// inside's pixels, for the OS's window menu, which Windows' default doesn't open for a title
    /// bar inside the window.
    pub(crate) fn take_window_menu(&self) -> Option<PhysicalPosition<i32>> {
        let point = self.0.window_menu.take()?;
        Some(PhysicalPosition::new(point.x, point.y))
    }
}

/// Subclasses window `hwnd`, one of this thread's.
fn install(hwnd: HWND) -> Option<Frame> {
    let state = Rc::new(State::default());
    let data = Rc::into_raw(Rc::clone(&state));
    // SAFETY: `hwnd` is a window of this thread, which calls `procedure` on this thread alone,
    // with `data`: a reference the window holds until `procedure` releases it as the window is
    // destroyed.
    if unsafe { SetWindowSubclass(hwnd, Some(procedure), 0, data as usize) } == 0 {
        // SAFETY: the window didn't take the reference, which nothing else holds.
        drop(unsafe { Rc::from_raw(data) });
        return None;
    }
    // SAFETY: `hwnd` is a window; this lays out its frame anew, which `procedure` now does.
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    fit(hwnd);
    Some(Frame(state))
}

/// The window procedure in front of winit's.
///
/// # Safety
///
/// Windows calls it for window `hwnd`, with `data` from [`install`].
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: the window holds a reference to `data`, released only below, as the window goes.
    let state = unsafe { &*(data as *const State) };
    let next = || {
        // SAFETY: this passes the message on, from within the subclass procedure.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    };
    match message {
        WM_NCCALCSIZE => {
            // The first of the rects `lparam` points to is the window's, and becomes its inside:
            // within Windows' own frame, then back up to the window's top, past the title bar
            // and the top border, which the band along the top of the inside stands in for. A
            // maximized window hangs its borders past the monitor's edges, the top one included.
            let rect = lparam as *mut RECT;
            // SAFETY: `lparam` points to a rect for this message, which Windows' default lays out
            // the frame in.
            let (rect, before) = unsafe {
                let before = *rect;
                DefWindowProcW(hwnd, message, wparam, lparam);
                (&mut *rect, before)
            };
            let border = rect.left - before.left;
            // SAFETY: plain queries of a window.
            let (minimized, maximized) = unsafe { (IsIconic(hwnd) != 0, IsZoomed(hwnd) != 0) };
            if !minimized {
                state.border.set(border);
            }
            rect.top = before.top + if maximized { border } else { 0 };
            0
        }
        WM_NCHITTEST => {
            // Windows' borders, outside the inside.
            let hit = next();
            if hit != HTCLIENT as LRESULT {
                return hit;
            }
            let (_, inside) = rects(hwnd);
            let point = inside_point(hwnd, lparam);
            // SAFETY: a plain query of a window.
            let maximized = unsafe { IsZoomed(hwnd) != 0 };
            let pos = pos2(point.x as f32 + 0.5, point.y as f32 + 0.5);
            let band = if maximized { 0 } else { state.border.get() };
            hit_test(pos, inside.right as f32, band as f32, state.caption.get()) as LRESULT
        }
        WM_NCMOUSEMOVE => {
            let over = wparam == HTMAXBUTTON as WPARAM;
            state.over_maximize.set(over);
            state.pressed.set(state.pressed.get() && over);
            if !state.tracking.replace(true) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: `track` asks for `WM_NCMOUSELEAVE` once the pointer leaves the frame.
                unsafe { TrackMouseEvent(&mut track) };
            }
            next()
        }
        // Windows' default would draw a button of its own over the window's, and press that.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam == HTMAXBUTTON as WPARAM => {
            state.pressed.set(true);
            0
        }
        WM_NCLBUTTONUP if wparam == HTMAXBUTTON as WPARAM => {
            if state.pressed.replace(false) {
                // SAFETY: plain calls on a window, which take effect after this message.
                unsafe {
                    let command = if IsZoomed(hwnd) != 0 {
                        SC_RESTORE
                    } else {
                        SC_MAXIMIZE
                    };
                    PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
                }
            }
            0
        }
        WM_NCMOUSELEAVE => {
            state.over_maximize.set(false);
            state.pressed.set(false);
            state.tracking.set(false);
            next()
        }
        // Windows' default opens the system menu only for a title bar of its own, so winit does.
        WM_NCRBUTTONUP if wparam == HTCAPTION as WPARAM => {
            state.window_menu.set(Some(inside_point(hwnd, lparam)));
            0
        }
        WM_GETMINMAXINFO => {
            // winit bounds the inside's size as the whole window's.
            let result = next();
            let border = state.border.get();
            // SAFETY: `lparam` points to the window's bounds for this message.
            let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            for size in [&mut info.ptMinTrackSize, &mut info.ptMaxTrackSize] {
                size.x += 2 * border;
                size.y += border;
            }
            result
        }
        WM_DPICHANGED => {
            // winit sizes the window for its new scale, which it does at every change of scale.
            let result = next();
            fit(hwnd);
            result
        }
        WM_NCDESTROY => {
            // SAFETY: the subclass goes with the window, and no other call of `procedure` holds
            // the window's reference to `data`, which goes with it.
            unsafe {
                RemoveWindowSubclass(hwnd, Some(procedure), 0);
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                drop(Rc::from_raw(data as *const State));
                result
            }
        }
        _ => next(),
    }
}

/// What's at `pos` in a window's inside, `width` pixels wide, as Windows' hit test names it: the
/// band along the top that resizes the window, `band` pixels deep, which resizes it diagonally as
/// far in from either end, then the title bar's parts that are the OS's.
fn hit_test(pos: Pos2, width: f32, band: f32, caption: Option<Caption>) -> u32 {
    if pos.y < band {
        return if pos.x < band {
            HTTOPLEFT
        } else if pos.x > width - band {
            HTTOPRIGHT
        } else {
            HTTOP
        };
    }
    match caption {
        Some(caption) if caption.maximize.contains(pos) => HTMAXBUTTON,
        Some(caption) if caption.drag.iter().any(|rect| rect.contains(pos)) => HTCAPTION,
        _ => HTCLIENT,
    }
}

/// Grows window `hwnd`'s frame back around its inside, which winit sized as the whole window:
/// the window keeps its corner, and its inside the window's size. Not while the window is
/// minimized or maximized, when the OS sizes it.
fn fit(hwnd: HWND) {
    // SAFETY: plain queries of a window.
    if unsafe { IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 } {
        return;
    }
    let (window, inside) = rects(hwnd);
    let (width, height) = (window.right - window.left, window.bottom - window.top);
    // SAFETY: a plain call on a window.
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            2 * width - inside.right,
            2 * height - inside.bottom,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// The point on the desktop that a non-client mouse message's `lparam` holds, in window `hwnd`'s
/// inside.
fn inside_point(hwnd: HWND, lparam: LPARAM) -> POINT {
    let mut point = POINT {
        x: i32::from(lparam as u16 as i16),
        y: i32::from((lparam >> 16) as u16 as i16),
    };
    // SAFETY: a plain query of a window.
    unsafe { ScreenToClient(hwnd, &mut point) };
    point
}

/// Window `hwnd`'s rect on the desktop, and its inside's, from the inside's corner, in pixels.
fn rects(hwnd: HWND) -> (RECT, RECT) {
    let empty = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let (mut window, mut inside) = (empty, empty);
    // SAFETY: each fills the rect it's given, or leaves it empty for a window that's gone.
    unsafe {
        GetWindowRect(hwnd, &mut window);
        GetClientRect(hwnd, &mut inside);
    }
    (window, inside)
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use egui::Rect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HTBOTTOM, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, MSG, PM_REMOVE,
        PeekMessageW, RegisterClassW, SendMessageW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    use super::*;

    /// A title bar 600 pixels wide and 40 high, whose maximize button is 50 pixels from its
    /// right.
    fn caption() -> Caption {
        Caption {
            drag: [
                Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 40.0)),
                Rect::from_min_max(pos2(200.0, 0.0), pos2(450.0, 40.0)),
            ],
            maximize: Rect::from_min_max(pos2(500.0, 0.0), pos2(550.0, 40.0)),
        }
    }

    #[test]
    fn the_top_resizes_the_window_and_the_title_bar_s_free_space_and_maximize_button_are_the_os_s()
    {
        let hit = |x, y, band| hit_test(pos2(x, y), 600.0, band, Some(caption()));
        assert_eq!(hit(300.0, 3.0, 10.0), HTTOP);
        assert_eq!(hit(4.0, 3.0, 10.0), HTTOPLEFT);
        assert_eq!(hit(595.0, 3.0, 10.0), HTTOPRIGHT);
        assert_eq!(hit(300.0, 20.0, 10.0), HTCAPTION);
        assert_eq!(hit(4.0, 20.0, 10.0), HTCAPTION);
        assert_eq!(hit(525.0, 20.0, 10.0), HTMAXBUTTON);
        assert_eq!(hit(100.0, 20.0, 10.0), HTCLIENT);
        assert_eq!(hit(300.0, 60.0, 10.0), HTCLIENT);
        // Maximized, a window has no band.
        assert_eq!(hit(300.0, 3.0, 0.0), HTCAPTION);
        assert_eq!(hit(525.0, 3.0, 0.0), HTMAXBUTTON);
        // While the UI takes the whole bar, only the band is the OS's.
        let hit = |x, y| hit_test(pos2(x, y), 600.0, 10.0, None);
        assert_eq!(hit(300.0, 3.0), HTTOP);
        assert_eq!(hit(300.0, 20.0), HTCLIENT);
    }

    /// Sends `message` to window `hwnd`, with the point (`x`, `y`) on the desktop.
    fn send(hwnd: HWND, message: u32, wparam: u32, x: i32, y: i32) -> u32 {
        let lparam = ((y as u16 as u32) << 16 | x as u16 as u32) as LPARAM;
        // SAFETY: a message to a window of this thread, whose point fits in `lparam`.
        unsafe { SendMessageW(hwnd, message, wparam as WPARAM, lparam) as u32 }
    }

    #[test]
    fn a_window_keeps_its_inside_and_windows_frame_goes_around_it() {
        let class: Vec<u16> = "gg window frame test\0".encode_utf16().collect();
        // SAFETY: a hidden window, of a class whose procedure is Windows' default, styled as
        // winit styles a window; the class may be registered already.
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
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                640,
                480,
                0,
                0,
                instance,
                ptr::null(),
            )
        };
        assert_ne!(hwnd, 0);
        let frame = install(hwnd).expect("the window takes the subclass");

        // The inside is the size the whole window was, with the window's corner where it was,
        // and borders left, right and below it but none above.
        let (window, inside) = rects(hwnd);
        assert_eq!((inside.right, inside.bottom), (640, 480));
        assert_eq!((window.left, window.top), (100, 100));
        let border = window.right - window.left - 640;
        assert!(border > 0 && border % 2 == 0, "{border}");
        let border = border / 2;
        assert_eq!(window.bottom - window.top, 480 + border);
        let mut info: MINMAXINFO = MINMAXINFO {
            ptReserved: POINT { x: 0, y: 0 },
            ptMaxSize: POINT { x: 0, y: 0 },
            ptMaxPosition: POINT { x: 0, y: 0 },
            ptMinTrackSize: POINT { x: 500, y: 300 },
            ptMaxTrackSize: POINT { x: 900, y: 700 },
        };
        // SAFETY: the window's bounds to fill, as Windows asks for them.
        unsafe { SendMessageW(hwnd, WM_GETMINMAXINFO, 0, &mut info as *mut _ as LPARAM) };
        let min = info.ptMinTrackSize;
        assert_eq!((min.x, min.y), (500 + 2 * border, 300 + border));

        // The inside's corner on the desktop, and points from it.
        let (x, y) = (100 + border, 100);
        let hit = |dx, dy| send(hwnd, WM_NCHITTEST, 0, x + dx, y + dy);
        assert_eq!(hit(-border / 2, 240), HTLEFT);
        assert_eq!(hit(640 + border / 2, 240), HTRIGHT);
        assert_eq!(hit(320, 480 + border / 2), HTBOTTOM);
        assert_eq!(hit(640 + border / 2, 480 + border / 2), HTBOTTOMRIGHT);
        assert_eq!(hit(320, 1), HTTOP);
        assert_eq!(hit(1, 1), HTTOPLEFT);
        assert_eq!(hit(320, 20), HTCLIENT);
        frame.set_caption(Some(caption()), 1.0);
        assert_eq!(hit(320, 20), HTCAPTION);
        assert_eq!(hit(525, 20), HTMAXBUTTON);
        assert_eq!(hit(320, 1), HTTOP);

        // The maximize button looks as the OS says, and a click on it maximizes the window.
        let button = |message| send(hwnd, message, HTMAXBUTTON, x + 525, y + 20);
        button(WM_NCMOUSEMOVE);
        assert_eq!(frame.maximize(), Look::Hovered);
        assert_eq!(button(WM_NCLBUTTONDOWN), 0);
        assert_eq!(frame.maximize(), Look::Pressed);
        button(WM_NCLBUTTONUP);
        assert_eq!(frame.maximize(), Look::Hovered);
        // SAFETY: an empty message for `PeekMessageW` to fill.
        let mut msg: MSG = unsafe { mem::zeroed() };
        // SAFETY: takes the posted command off this thread's queue, so that nothing maximizes
        // the window.
        let posted =
            unsafe { PeekMessageW(&mut msg, hwnd, WM_SYSCOMMAND, WM_SYSCOMMAND, PM_REMOVE) };
        assert_ne!(posted, 0);
        assert_eq!(msg.wParam, SC_MAXIMIZE as WPARAM);
        // Pressed, then let go elsewhere, it doesn't.
        button(WM_NCLBUTTONDOWN);
        send(hwnd, WM_NCMOUSEMOVE, HTCAPTION, x + 320, y + 20);
        assert_eq!(frame.maximize(), Look::Idle);
        button(WM_NCLBUTTONUP);
        // SAFETY: as above.
        let posted =
            unsafe { PeekMessageW(&mut msg, hwnd, WM_SYSCOMMAND, WM_SYSCOMMAND, PM_REMOVE) };
        assert_eq!(posted, 0);
        button(WM_NCMOUSEMOVE);
        send(hwnd, WM_NCMOUSELEAVE, 0, 0, 0);
        assert_eq!(frame.maximize(), Look::Idle);

        // A right-click on the free space asks for the window menu, where it was let go.
        assert_eq!(frame.take_window_menu(), None);
        send(hwnd, WM_NCRBUTTONUP, HTCAPTION, x + 320, y + 20);
        assert_eq!(
            frame.take_window_menu(),
            Some(PhysicalPosition::new(320, 20))
        );
        assert_eq!(frame.take_window_menu(), None);

        // The window lets go of what it shares with the frame as it goes.
        // SAFETY: the window is this thread's.
        unsafe { DestroyWindow(hwnd) };
        assert_eq!(Rc::strong_count(&frame.0), 1);
    }
}
