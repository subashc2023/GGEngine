//! Input state that layers poll: which keys and mouse buttons are held, and where the cursor is.

use crate::{Event, Key, MouseButton};

/// What's held down and where the cursor is, as of the last event the layers were sent.
///
/// It follows those events, so a press the UI kept doesn't count, and a key pressed and released
/// between two frames never shows as held: layers that need every press take them from events.
/// Losing focus lets go of everything, since not every platform reports the releases that happen
/// while the window is out of focus; a key already down as the window regains focus counts from its
/// next press, or from its first repeat.
#[derive(Debug, Default)]
pub struct Input {
    /// Lists rather than sets, since few keys and buttons are held at once.
    keys: Vec<Key>,
    buttons: Vec<MouseButton>,
    cursor: Option<[f64; 2]>,
}

impl Input {
    /// Whether `key` is held.
    pub fn key_down(&self, key: Key) -> bool {
        self.keys.contains(&key)
    }

    /// Whether `button` is held.
    pub fn button_down(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button)
    }

    /// Where the cursor is, in physical pixels from the window's top left; `None` while it's
    /// outside the window, and in headless runs. A drag that leaves the window can go on
    /// reporting the cursor beyond its edges.
    pub fn cursor(&self) -> Option<[f64; 2]> {
        self.cursor
    }

    pub(crate) fn record(&mut self, event: &Event<'_>) {
        match *event {
            Event::Key { key, pressed, .. } => hold(&mut self.keys, key, pressed),
            Event::MouseButton { button, pressed } => hold(&mut self.buttons, button, pressed),
            Event::CursorMoved { x, y } => self.cursor = Some([x, y]),
            Event::CursorLeft => self.cursor = None,
            Event::Focused(false) => {
                self.keys.clear();
                self.buttons.clear();
            }
            _ => {}
        }
    }
}

/// Adds `item` to `held` when it's pressed and removes it when it's released. A held key's
/// repeats are presses too, and a release can come without its press.
fn hold<T: PartialEq>(held: &mut Vec<T>, item: T, pressed: bool) {
    let at = held.iter().position(|held| *held == item);
    match (pressed, at) {
        (true, None) => held.push(item),
        (false, Some(at)) => {
            held.swap_remove(at);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool, repeat: bool) -> Event<'static> {
        Event::Key {
            key,
            pressed,
            repeat,
        }
    }

    #[test]
    fn a_press_holds_until_its_release_however_often_it_repeats() {
        let mut input = Input::default();
        input.record(&key(Key::KeyW, false, false));
        assert!(!input.key_down(Key::KeyW));
        input.record(&key(Key::KeyW, true, false));
        input.record(&key(Key::KeyW, true, true));
        input.record(&key(Key::KeyW, true, true));
        input.record(&key(Key::ShiftLeft, true, false));
        assert!(input.key_down(Key::KeyW));
        input.record(&key(Key::KeyW, false, false));
        assert!(!input.key_down(Key::KeyW));
        assert!(input.key_down(Key::ShiftLeft));
    }

    #[test]
    fn losing_focus_lets_go_of_everything_but_the_cursor() {
        let mut input = Input::default();
        input.record(&key(Key::AltLeft, true, false));
        for button in [MouseButton::Left, MouseButton::Other(9)] {
            input.record(&Event::MouseButton {
                button,
                pressed: true,
            });
        }
        input.record(&Event::CursorMoved { x: 3.0, y: 4.0 });
        assert!(input.button_down(MouseButton::Other(9)));
        input.record(&Event::Focused(false));
        assert!(!input.key_down(Key::AltLeft));
        assert!(!input.button_down(MouseButton::Left));
        assert!(!input.button_down(MouseButton::Other(9)));
        assert_eq!(input.cursor(), Some([3.0, 4.0]));
    }

    #[test]
    fn the_cursor_is_where_it_last_moved_until_it_leaves() {
        let mut input = Input::default();
        assert_eq!(input.cursor(), None);
        input.record(&Event::CursorMoved { x: 10.5, y: 20.0 });
        assert_eq!(input.cursor(), Some([10.5, 20.0]));
        input.record(&Event::CursorLeft);
        assert_eq!(input.cursor(), None);
    }
}
