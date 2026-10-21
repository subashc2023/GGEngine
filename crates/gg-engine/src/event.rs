//! The engine's events, converted from winit's so that apps and games never see winit's types.

use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// Something that happened to the window or came from the keyboard or mouse. Positions and
/// sizes are in physical pixels; [`Window::scale`](crate::Window::scale) converts them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event<'a> {
    /// The user asked to close the window. Unless a layer handles it, the app exits.
    CloseRequested,
    /// The window's drawable area changed size. A minimized window gets [`Event::Minimized`]
    /// instead of a size of zero.
    Resized {
        width: u32,
        height: u32,
    },
    /// The window was minimized, or otherwise left without an area to draw into. Rendering
    /// pauses until [`Event::Restored`].
    Minimized,
    Restored,
    /// The window gained or lost keyboard focus.
    Focused(bool),
    /// Physical pixels per logical pixel changed, as when the window moves to another display.
    ScaleChanged(f64),
    /// A key went down or up. `repeat` marks the presses a held key repeats. A layer can get a
    /// release without its press, when the press went to the UI.
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
    },
    /// Text that a key press typed, which follows its [`Event::Key`]. Keys such as Backspace,
    /// Enter and Tab type none.
    Text(&'a str),
    /// The cursor moved within the window, to `x` and `y` from its top left.
    CursorMoved {
        x: f64,
        y: f64,
    },
    CursorEntered,
    CursorLeft,
    /// A mouse button went down or up. As with keys, a release can come without its press.
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    /// The mouse wheel or a touchpad scrolled.
    Scroll(Scroll),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// How far a scroll went: in lines from a mouse wheel, or in pixels from a touchpad. Positive
/// `y` scrolls up, and positive `x` scrolls left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scroll {
    Lines { x: f32, y: f32 },
    Pixels { x: f64, y: f64 },
}

/// Turns one of winit's events into the engine's, calling `emit` for each; a key press that
/// types text gives two. Events the engine doesn't use, and keys the platform couldn't
/// identify, give none. Minimizing and restoring are left to the caller, which tracks the
/// window's area.
pub(crate) fn convert<'a>(event: &'a WindowEvent, mut emit: impl FnMut(Event<'a>)) {
    match event {
        WindowEvent::CloseRequested => emit(Event::CloseRequested),
        WindowEvent::Focused(focused) => emit(Event::Focused(*focused)),
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            emit(Event::ScaleChanged(*scale_factor));
        }
        WindowEvent::KeyboardInput { event, .. } => {
            let PhysicalKey::Code(code) = event.physical_key else {
                return;
            };
            let Some(key) = Key::from_winit(code) else {
                return;
            };
            let pressed = event.state == ElementState::Pressed;
            emit(Event::Key {
                key,
                pressed,
                repeat: event.repeat,
            });
            // winit gives Backspace, Enter, Escape and Tab their control characters as text.
            if pressed
                && let Some(text) = &event.text
                && !text.chars().any(char::is_control)
            {
                emit(Event::Text(text));
            }
        }
        WindowEvent::CursorMoved { position, .. } => emit(Event::CursorMoved {
            x: position.x,
            y: position.y,
        }),
        WindowEvent::CursorEntered { .. } => emit(Event::CursorEntered),
        WindowEvent::CursorLeft { .. } => emit(Event::CursorLeft),
        WindowEvent::MouseInput { state, button, .. } => {
            use winit::event::MouseButton as Winit;
            let button = match *button {
                Winit::Left => MouseButton::Left,
                Winit::Right => MouseButton::Right,
                Winit::Middle => MouseButton::Middle,
                Winit::Back => MouseButton::Back,
                Winit::Forward => MouseButton::Forward,
                Winit::Other(other) => MouseButton::Other(other),
            };
            emit(Event::MouseButton {
                button,
                pressed: *state == ElementState::Pressed,
            });
        }
        WindowEvent::MouseWheel { delta, .. } => emit(Event::Scroll(match *delta {
            MouseScrollDelta::LineDelta(x, y) => Scroll::Lines { x, y },
            MouseScrollDelta::PixelDelta(position) => Scroll::Pixels {
                x: position.x,
                y: position.y,
            },
        })),
        _ => {}
    }
}

/// Defines [`Key`] and its conversion from winit's `KeyCode` from one list of names, so the
/// two can't drift apart.
macro_rules! keys {
    ($($name:ident)*) => {
        /// A key, by its position rather than its label: the key named for `Q` on a US
        /// keyboard is `KeyQ` on a French one too, where it types `A`. The names are the W3C
        /// UI Events `code` values. [`Event::Text`] carries what a key types.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Key {
            $($name,)*
        }

        impl Key {
            /// `None` for keys winit adds after this list was written.
            fn from_winit(code: KeyCode) -> Option<Self> {
                match code {
                    $(KeyCode::$name => Some(Self::$name),)*
                    _ => None,
                }
            }
        }
    };
}

// winit 0.30's `KeyCode`, in its order.
keys! {
    Backquote Backslash BracketLeft BracketRight Comma
    Digit0 Digit1 Digit2 Digit3 Digit4 Digit5 Digit6 Digit7 Digit8 Digit9
    Equal IntlBackslash IntlRo IntlYen
    KeyA KeyB KeyC KeyD KeyE KeyF KeyG KeyH KeyI KeyJ KeyK KeyL KeyM
    KeyN KeyO KeyP KeyQ KeyR KeyS KeyT KeyU KeyV KeyW KeyX KeyY KeyZ
    Minus Period Quote Semicolon Slash
    AltLeft AltRight Backspace CapsLock ContextMenu ControlLeft ControlRight Enter
    SuperLeft SuperRight ShiftLeft ShiftRight Space Tab
    Convert KanaMode Lang1 Lang2 Lang3 Lang4 Lang5 NonConvert
    Delete End Help Home Insert PageDown PageUp
    ArrowDown ArrowLeft ArrowRight ArrowUp
    NumLock Numpad0 Numpad1 Numpad2 Numpad3 Numpad4 Numpad5 Numpad6 Numpad7 Numpad8 Numpad9
    NumpadAdd NumpadBackspace NumpadClear NumpadClearEntry NumpadComma NumpadDecimal
    NumpadDivide NumpadEnter NumpadEqual NumpadHash NumpadMemoryAdd NumpadMemoryClear
    NumpadMemoryRecall NumpadMemoryStore NumpadMemorySubtract NumpadMultiply NumpadParenLeft
    NumpadParenRight NumpadStar NumpadSubtract
    Escape Fn FnLock PrintScreen ScrollLock Pause
    BrowserBack BrowserFavorites BrowserForward BrowserHome BrowserRefresh BrowserSearch
    BrowserStop Eject LaunchApp1 LaunchApp2 LaunchMail
    MediaPlayPause MediaSelect MediaStop MediaTrackNext MediaTrackPrevious
    Power Sleep AudioVolumeDown AudioVolumeMute AudioVolumeUp WakeUp
    Meta Hyper Turbo Abort Resume Suspend Again Copy Cut Find Open Paste Props Select Undo
    Hiragana Katakana
    F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 F13 F14 F15 F16 F17 F18 F19 F20
    F21 F22 F23 F24 F25 F26 F27 F28 F29 F30 F31 F32 F33 F34 F35
}
