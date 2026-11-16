//! The editor's keyboard shortcuts: one table for the File menu, its labels and the view.

use gg_engine::Key;
use gg_engine::egui::{self, KeyboardShortcut, Modifiers};

use crate::camera::Tool;
use crate::file::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    /// Chooses a File menu item, whichever panel has the keys ([`file::shortcut`]).
    ///
    /// [`file::shortcut`]: crate::file::shortcut
    File(Item),
    Tool(Tool),
    /// Flips Move's and Rotate's axes between the entity's own and the world's.
    FlipSpace,
    /// Frames what's picked, as the Entities panel asks too.
    Frame,
}

/// A shortcut. Its key is a position, whatever the layout calls it, as the engine's keys are.
/// `command` is Ctrl, or Cmd on macOS.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    pub key: Key,
    pub command: bool,
    pub shift: bool,
    pub does: Does,
}

/// The editor's shortcuts. Save As's comes before Save's, as a File menu shortcut matches with
/// Shift held too. The view's take no command key, leaving those to the menu.
pub const SHORTCUTS: [Shortcut; 10] = [
    file(Key::KeyS, true, Item::SaveAs),
    file(Key::KeyS, false, Item::Save),
    file(Key::KeyN, false, Item::New),
    file(Key::KeyO, false, Item::Open),
    view(Key::KeyQ, Does::Tool(Tool::Select)),
    view(Key::KeyW, Does::Tool(Tool::Move)),
    view(Key::KeyE, Does::Tool(Tool::Rotate)),
    view(Key::KeyR, Does::Tool(Tool::Scale)),
    view(Key::KeyX, Does::FlipSpace),
    view(Key::KeyF, Does::Frame),
];

const fn file(key: Key, shift: bool, item: Item) -> Shortcut {
    Shortcut {
        key,
        command: true,
        shift,
        does: Does::File(item),
    }
}

const fn view(key: Key, does: Does) -> Shortcut {
    Shortcut {
        key,
        command: false,
        shift: false,
        does,
    }
}

impl Shortcut {
    /// As egui's, which the File menu matches and names in its labels.
    pub fn egui(self) -> KeyboardShortcut {
        let mut modifiers = Modifiers::NONE;
        if self.command {
            modifiers = modifiers.plus(Modifiers::COMMAND);
        }
        if self.shift {
            modifiers = modifiers.plus(Modifiers::SHIFT);
        }
        KeyboardShortcut::new(modifiers, egui_key(self.key))
    }
}

/// egui's key at `key`'s position.
///
/// # Panics
///
/// - If `key` isn't one of [`SHORTCUTS`]' keys.
fn egui_key(key: Key) -> egui::Key {
    match key {
        Key::KeyN => egui::Key::N,
        Key::KeyO => egui::Key::O,
        Key::KeyS => egui::Key::S,
        Key::KeyQ => egui::Key::Q,
        Key::KeyW => egui::Key::W,
        Key::KeyE => egui::Key::E,
        Key::KeyR => egui::Key::R,
        Key::KeyX => egui::Key::X,
        Key::KeyF => egui::Key::F,
        _ => unreachable!("a key of the shortcuts' table, which this names each of"),
    }
}
