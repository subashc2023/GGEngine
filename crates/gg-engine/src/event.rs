//! The engine's events, converted from winit's and gilrs's so that apps and games never see their
//! types.

use gg_core::math::Vec2;
use winit::event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// Something that happened to the window or came from the keyboard, the mouse or a gamepad.
/// Positions and sizes are in physical pixels; [`Window::scale`](crate::Window::scale) converts
/// them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event<'a> {
    /// The user asked to close the window. Unless a layer handles it, the app exits.
    CloseRequested,
    /// The window's drawable area changed size. A minimized window gets [`Event::Minimized`]
    /// instead of a size of zero.
    Resized { width: u32, height: u32 },
    /// The window was minimized, or otherwise left without an area to draw into. Rendering
    /// pauses until [`Event::Restored`].
    Minimized,
    /// The window has an area to draw into again, after [`Event::Minimized`]: rendering resumes.
    Restored,
    /// The window gained or lost keyboard focus.
    Focused(bool),
    /// Physical pixels per logical pixel changed, as when the window moves to another display.
    ScaleChanged(f64),
    /// A key went down or up. `repeat` marks the presses a held key repeats. A layer can get a
    /// release without its press, when the press went to the UI, or when the key was already
    /// down as the window gained focus.
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
    },
    /// Text that a key press typed, which follows its [`Event::Key`]. Keys such as Backspace,
    /// Enter and Tab type none. An IME's composition doesn't come as text: the engine's events
    /// carry none of it.
    Text(&'a str),
    /// The text on the OS's clipboard, which Ctrl+V, or Cmd+V on macOS, pasted: it follows the
    /// key's [`Event::Key`], in a windowed run, where the clipboard holds text. A game's text field
    /// with the focus takes its first line at its caret, as it takes what's typed.
    Paste(&'a str),
    /// The cursor moved within the window, to `x` and `y` from its top left.
    CursorMoved { x: f64, y: f64 },
    /// The cursor came into the window.
    CursorEntered,
    /// The cursor went out of the window.
    CursorLeft,
    /// The mouse moved `x` and `y` in its own units, positive right and down as the cursor goes:
    /// its raw motion, before the OS moves the cursor by it, without the OS's acceleration where
    /// the platform reports it without, as on Windows, and on past the window's and the screen's
    /// edges, for looking about with the cursor grabbed, as
    /// [`Context::set_cursor_grabbed`](crate::Context::set_cursor_grabbed) has it. It comes while
    /// the window has focus, beside [`Event::CursorMoved`], with or without the cursor grabbed,
    /// and in a dockspace while the view has the keys.
    MouseMotion { x: f64, y: f64 },
    /// A mouse button went down or up. As with keys, a release can come without its press.
    MouseButton { button: MouseButton, pressed: bool },
    /// The mouse wheel or a touchpad scrolled.
    Scroll(Scroll),
    /// A gamepad's button went down or up. A trigger is a button too, pressed at 0.75 of its travel
    /// and released at 0.65, before the dead zone that an [`Event::PadTrigger`]'s value goes
    /// through, which reads them as about 0.81 and 0.69. As with keys, a release can come without
    /// its press.
    PadButton {
        pad: Pad,
        button: PadButton,
        pressed: bool,
    },
    /// A gamepad's stick moved: right and up are positive, it's no longer than 1, and it's exactly
    /// zero within its dead zone.
    PadStick { pad: Pad, stick: Stick, value: Vec2 },
    /// A gamepad's trigger moved: from 0, at rest and within its dead zone, to 1 all the way.
    PadTrigger {
        pad: Pad,
        trigger: Trigger,
        value: f32,
    },
}

/// A gamepad, by its number for the run. One that disconnects and connects again may come back as
/// another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pad(pub usize);

/// A gamepad's button, by its position, as keys are: `South` is A on an Xbox pad and Cross on a
/// PlayStation one, `Select` View and Create, and `Start` Menu and Options. The shoulders are the
/// bumpers above the triggers, and the thumbs are the sticks pressed in. Windows doesn't report
/// `Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadButton {
    South,
    East,
    North,
    West,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl PadButton {
    /// Every button, in order.
    pub(crate) const ALL: [Self; 17] = [
        Self::South,
        Self::East,
        Self::North,
        Self::West,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::Select,
        Self::Start,
        Self::Mode,
        Self::LeftThumb,
        Self::RightThumb,
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
    ];
}

/// A gamepad's stick, which [`Event::PadStick`] says has moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stick {
    Left,
    Right,
}

/// A gamepad's trigger, which [`Event::PadTrigger`] says has moved; as a button, it's
/// [`PadButton::LeftTrigger`] or [`PadButton::RightTrigger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Left,
    Right,
}

/// A mouse button: `Back` and `Forward` are the side buttons, and `Other` one beyond them, by
/// winit's number for it, which differs from one platform to another.
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

impl Scroll {
    /// What a line of the wheel is in points, for scrolls that a touchpad reports in pixels: 40, as
    /// egui counts it on the desktop (`InputOptions::line_scroll_speed`).
    pub const POINTS_PER_LINE: f64 = 40.0;

    /// How many lines it scrolled up, a touchpad's pixels counted in a window of `scale` pixels a
    /// point, as [`POINTS_PER_LINE`](Self::POINTS_PER_LINE) says.
    pub fn lines(self, scale: f64) -> f32 {
        match self {
            Self::Lines { y, .. } => y,
            Self::Pixels { y, .. } => (y / scale / Self::POINTS_PER_LINE) as f32,
        }
    }
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
        WindowEvent::KeyboardInput {
            event,
            is_synthetic,
            ..
        } => {
            // winit makes up presses for the keys held as the window gains focus, such as Alt
            // after Alt+Tab, which weren't pressed in it; egui ignores them too. Its made-up
            // releases as the window loses focus stay, so that no key is left held.
            if *is_synthetic && event.state == ElementState::Pressed {
                return;
            }
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

/// The engine's event for one of winit's device events: the mouse's raw motion, and none for the
/// rest, which windows' events give.
pub(crate) fn device(event: &DeviceEvent) -> Option<Event<'static>> {
    match *event {
        DeviceEvent::MouseMotion { delta: (x, y) } => Some(Event::MouseMotion { x, y }),
        _ => None,
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

            /// Its name, as the tool API's `input` names keys: its W3C UI Events `code` value, such
            /// as `KeyW`, but for `SuperLeft` and `SuperRight`, winit's names for `MetaLeft` and
            /// `MetaRight`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($name),)*
                }
            }

            /// The key named `name`, as [`name`](Self::name) names it, or by its W3C `code` value,
            /// `MetaLeft` and `MetaRight` among them; `None` for a name that's no key's.
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    "MetaLeft" => Some(Self::SuperLeft),
                    "MetaRight" => Some(Self::SuperRight),
                    $(stringify!($name) => Some(Self::$name),)*
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
