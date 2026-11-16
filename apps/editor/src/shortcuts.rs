//! The editor's keyboard shortcuts, in one table that the File menu, its labels and the view read.

use gg_engine::Key;
use gg_engine::egui::{self, KeyboardShortcut, Modifiers};

use crate::camera::Tool;
use crate::file::Item;

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    /// Chooses the File menu's item, whichever panel has the keys, as [`file::shortcut`] matches
    /// it.
    ///
    /// [`file::shortcut`]: crate::file::shortcut
    File(Item),
    /// Picks the view's tool.
    Tool(Tool),
    /// Flips the space of Move's and Rotate's axes between the entity's own and the world's.
    FlipSpace,
    /// Frames what's picked, as the Entities panel asks too.
    Frame,
}

/// A shortcut: its key, the one at that position whatever the layout calls it, as the engine's
/// keys are positions; whether it takes the command key, Ctrl or Cmd on macOS, and Shift; and what
/// it does.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    pub key: Key,
    pub command: bool,
    pub shift: bool,
    pub does: Does,
}

/// The editor's shortcuts. Save As's comes before Save's, as a File menu's shortcut matches with
/// Shift held as well. The view's take no command key, which leaves them to the menu's.
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

/// A File menu's shortcut, `key` with the command key, and with Shift if `shift`, choosing `item`.
const fn file(key: Key, shift: bool, item: Item) -> Shortcut {
    Shortcut {
        key,
        command: true,
        shift,
        does: Does::File(item),
    }
}

/// A view's shortcut, `key` alone, doing `does`.
const fn view(key: Key, does: Does) -> Shortcut {
    Shortcut {
        key,
        command: false,
        shift: false,
        does,
    }
}

impl Shortcut {
    /// It as egui has it, for the File menu, which matches its shortcuts in egui's input and names
    /// them in its labels.
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

/// egui's key at `key`'s position, for a key [`SHORTCUTS`] has.
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
