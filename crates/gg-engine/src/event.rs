//! The engine's events, converted from winit's and gilrs's so apps and games never see their types.

use gg_core::math::Vec2;
use winit::event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// Something that happened to the window, or came from the keyboard, mouse or a gamepad.
/// Positions and sizes are in physical pixels; [`Window::scale`](crate::Window::scale) converts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event<'a> {
    /// The user asked to close the window. Unless a layer handles it, the app exits.
    CloseRequested,
    /// The drawable area changed size. A minimized window gets [`Event::Minimized`], not size 0.
    Resized { width: u32, height: u32 },
    /// The window has no area to draw into (minimized). Rendering pauses until
    /// [`Event::Restored`].
    Minimized,
    /// The window has an area again, after [`Event::Minimized`]: rendering resumes.
    Restored,
    /// The window gained or lost keyboard focus.
    Focused(bool),
    /// Physical pixels per logical pixel changed, as when the window moves to another display.
    ScaleChanged(f64),
    /// A key went down or up. `repeat` marks a held key's repeated presses. A layer can get a
    /// release without its press: the press went to the UI, or the key was already down as the
    /// window gained focus.
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
    },
    /// Text a key press typed, after its [`Event::Key`]. Backspace, Enter and Tab type none. IME
    /// composition isn't carried.
    Text(&'a str),
    /// Clipboard text pasted by Ctrl+V (Cmd+V on macOS), after the key's [`Event::Key`], in a
    /// windowed run where the clipboard holds text. A focused game text field takes its first line
    /// at its caret, as it takes typing.
    Paste(&'a str),
    /// The cursor moved within the window, to `x` and `y` from its top left.
    CursorMoved { x: f64, y: f64 },
    /// The cursor came into the window.
    CursorEntered,
    /// The cursor went out of the window.
    CursorLeft,
    /// The mouse moved `x` and `y` in its own units, positive right and down. Raw motion: without
    /// the OS's acceleration where the platform reports it so (as on Windows), and past the
    /// window's and screen's edges, for looking about with the cursor grabbed
    /// ([`Context::set_cursor_grabbed`](crate::Context::set_cursor_grabbed)). It comes beside
    /// [`Event::CursorMoved`] while the window has focus, and in a dockspace while the view has
    /// the keys.
    MouseMotion { x: f64, y: f64 },
    /// A mouse button went down or up. As with keys, a release can come without its press.
    MouseButton { button: MouseButton, pressed: bool },
    /// The mouse wheel or a touchpad scrolled.
    Scroll(Scroll),
    /// A gamepad's button went down or up. A trigger is a button too: pressed at 0.75 of its
    /// travel, released at 0.65 (about 0.81 and 0.69 after the dead zone an
    /// [`Event::PadTrigger`]'s value goes through). As with keys, a release can come without its
    /// press.
    PadButton {
        pad: Pad,
        button: PadButton,
        pressed: bool,
    },
    /// A gamepad's stick moved: right and up positive, length at most 1, exactly zero within its
    /// dead zone.
    PadStick { pad: Pad, stick: Stick, value: Vec2 },
    /// A gamepad's trigger moved: 0 at rest and within its dead zone, 1 all the way.
    PadTrigger {
        pad: Pad,
        trigger: Trigger,
        value: f32,
    },
}

/// A gamepad, by its number for the run. One that reconnects may come back as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pad(pub usize);

/// A gamepad's button, by position as keys are: `South` is Xbox's A and PlayStation's Cross,
/// `Select` View and Create, `Start` Menu and Options. Shoulders are the bumpers above the
/// triggers; thumbs are the sticks pressed in. Windows doesn't report `Mode`.
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

/// A gamepad's stick, as [`Event::PadStick`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stick {
    Left,
    Right,
}

/// A gamepad's trigger, as [`Event::PadTrigger`] names it; as a button, [`PadButton::LeftTrigger`]
/// or [`PadButton::RightTrigger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Left,
    Right,
}

/// A mouse button: `Back` and `Forward` are the side buttons; `Other` is one beyond them, by
/// winit's number, which differs by platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// A scroll's distance: lines from a mouse wheel, pixels from a touchpad. Positive `y` scrolls
/// up, positive `x` left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scroll {
    Lines { x: f32, y: f32 },
    Pixels { x: f64, y: f64 },
}

impl Scroll {
    /// A wheel line in points, for touchpad pixel scrolls: 40, as egui counts it on the desktop
    /// (`InputOptions::line_scroll_speed`).
    pub const POINTS_PER_LINE: f64 = 40.0;

    /// Lines scrolled up. A touchpad's pixels convert through `scale` pixels a point and
    /// [`POINTS_PER_LINE`](Self::POINTS_PER_LINE).
    pub fn lines(self, scale: f64) -> f32 {
        match self {
            Self::Lines { y, .. } => y,
            Self::Pixels { y, .. } => (y / scale / Self::POINTS_PER_LINE) as f32,
        }
    }
}

/// Turns a winit event into the engine's, calling `emit` for each; a key press that types text
/// gives two. Unused events and unidentified keys give none. The caller handles minimizing and
/// restoring, as it tracks the window's area.
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
            // winit makes up presses for keys held as the window gains focus (Alt after Alt+Tab)
            // that weren't pressed in it; egui ignores them too. Its made-up releases on losing
            // focus stay, so no key is left held.
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
            // winit gives Backspace, Enter, Escape and Tab control characters as text.
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

/// The engine's event for a winit device event: the mouse's raw motion. The rest come as window
/// events.
pub(crate) fn device(event: &DeviceEvent) -> Option<Event<'static>> {
    match *event {
        DeviceEvent::MouseMotion { delta: (x, y) } => Some(Event::MouseMotion { x, y }),
        _ => None,
    }
}

/// Defines [`Key`] and its conversion from winit's `KeyCode` from one list, so they can't drift.
macro_rules! keys {
    ($($name:ident)*) => {
        /// A key by position, not label: the key labeled `Q` on a US keyboard is `KeyQ` on a French
        /// one too, where it types `A`. Names are the W3C UI Events `code` values.
        /// [`Event::Text`] carries what a key types.
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

            /// Its name as the tool API's `input` uses: the W3C `code` value, such as `KeyW`,
            /// except `SuperLeft` and `SuperRight` (winit's names for `MetaLeft` and `MetaRight`).
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($name),)*
                }
            }

            /// The key named `name` by [`name`](Self::name) or its W3C `code` value (`MetaLeft` and
            /// `MetaRight` included); `None` for no key's name.
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
