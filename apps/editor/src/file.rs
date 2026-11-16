//! The File menu's items and shortcuts, and what the editor shows for them over its UI: the prompt
//! to save unsaved edits first, and a message saying why a scene couldn't be opened or saved.

use gg_engine::egui::{self, Button, Id, KeyboardShortcut, Modal};

use crate::shortcuts::{Does, SHORTCUTS};

/// The widest the prompt and a message get, in points, so a long error wraps.
const WIDTH: f32 = 400.0;

/// A File menu item, which its shortcut chooses too. Quit, below them, has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Empties the open scene, untitled until saved.
    New,
    /// Opens a picked scene in place of the open one.
    Open,
    /// Saves the open scene to its file, or without one, as Save As.
    Save,
    /// Saves the open scene to a picked file, which becomes its file.
    SaveAs,
}

impl Item {
    /// In the menu's order.
    pub const MENU: [Self; 4] = [Self::New, Self::Open, Self::Save, Self::SaveAs];

    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Open => "Open...",
            Self::Save => "Save",
            Self::SaveAs => "Save As...",
        }
    }

    /// Its keys from [`SHORTCUTS`], as the menu names them.
    pub fn shortcut(self) -> KeyboardShortcut {
        let shortcut = SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.does == Does::File(self));
        shortcut.expect("each item has a shortcut").egui()
    }
}

/// The File menu item that the first shortcut pressed among `events`, a frame's egui input,
/// chooses.
///
/// A shortcut's key is a position, whatever the layout calls it, so the view takes the same keys
/// out of what the game gets. Takes out the shortcuts' presses and their repeats, which choose
/// nothing, so one held down acts once.
pub fn shortcut(events: &mut Vec<egui::Event>) -> Option<Item> {
    // The table's order matters: Save As's comes first.
    let shortcuts = SHORTCUTS.iter().filter_map(|shortcut| match shortcut.does {
        Does::File(item) => Some(item),
        _ => None,
    });
    let mut chosen = None;
    events.retain(|event| {
        let &egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat,
            modifiers,
        } = event
        else {
            return true;
        };
        // egui-winit gives the position, unless egui has no key for it.
        let key = physical_key.unwrap_or(key);
        let matched = shortcuts.clone().find(|item| {
            let shortcut = item.shortcut();
            key == shortcut.logical_key && modifiers.matches_logically(shortcut.modifiers)
        });
        let Some(item) = matched else {
            return true;
        };
        if !repeat {
            chosen = chosen.or(Some(item));
        }
        false
    });
    chosen
}

/// How the prompt was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Save the open scene first, then go on.
    Save,
    /// Go on, discarding its unsaved edits.
    DontSave,
    /// Go back, doing nothing.
    Cancel,
}

/// Shows the save-first prompt, `title` and `text`, blocking clicks to the UI. Its buttons: Save
/// (if `saves`), Don't Save and Cancel, which Escape or a click outside also chooses. Returns this
/// frame's choice.
pub fn prompt(egui: &egui::Context, title: &str, text: &str, saves: bool) -> Option<Choice> {
    let shown = Modal::new(Id::unique("editor prompt")).show(egui, |ui| {
        ui.set_max_width(WIDTH);
        ui.heading(title);
        ui.label(text);
        let buttons = ui.horizontal(|ui| {
            let save = ui.add_enabled(saves, Button::new("Save"));
            let save =
                save.on_disabled_hover_text("The game is restarting: none serves to save it");
            let buttons = [
                (save, Choice::Save),
                (ui.button("Don't Save"), Choice::DontSave),
                (ui.button("Cancel"), Choice::Cancel),
            ];
            let clicked = buttons.into_iter().find(|(button, _)| button.clicked());
            clicked.map(|(_, choice)| choice)
        });
        buttons.inner
    });
    let cancelled = shown.should_close().then_some(Choice::Cancel);
    shown.inner.or(cancelled)
}

/// Why a scene couldn't be opened, saved or made, shown until dismissed.
#[derive(Debug)]
pub struct Message {
    /// What failed, on which file, such as "Can't save arena.ron".
    pub title: String,
    /// Why: the refusal's code and message.
    pub text: String,
}

impl Message {
    /// Shows it, blocking clicks to the UI. Returns whether its OK, Escape or a click outside
    /// dismissed it this frame.
    pub fn show(&self, egui: &egui::Context) -> bool {
        let shown = Modal::new(Id::unique("editor message")).show(egui, |ui| {
            ui.set_max_width(WIDTH);
            ui.heading(self.title.as_str());
            ui.label(self.text.as_str());
            ui.button("OK").clicked()
        });
        shown.inner || shown.should_close()
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::egui::Modifiers;

    use super::*;

    /// `key` pressed with `modifiers`, or held and repeating if `repeat`.
    fn pressed(key: egui::Key, modifiers: Modifiers, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    /// Each File menu shortcut chooses its item, Ctrl+Shift+S Save As rather than Save, and is
    /// taken from egui's input. A repeat chooses nothing and is taken too. Keys without the command
    /// key, and other shortcuts, are left.
    #[test]
    fn the_shortcuts_choose_the_file_menus_items() {
        // The command key as egui-winit sends it: with Ctrl on Windows and Linux, Cmd on macOS.
        for key in [Modifiers::CTRL, Modifiers::MAC_CMD] {
            let command = key.plus(Modifiers::COMMAND);
            let command_shift = command.plus(Modifiers::SHIFT);
            for (key, modifiers, item) in [
                (egui::Key::N, command, Item::New),
                (egui::Key::O, command, Item::Open),
                (egui::Key::S, command, Item::Save),
                (egui::Key::S, command_shift, Item::SaveAs),
            ] {
                let mut events = vec![pressed(key, modifiers, false)];
                assert_eq!(shortcut(&mut events), Some(item));
                assert_eq!(events, []);
                let mut events = vec![pressed(key, modifiers, true)];
                assert_eq!(shortcut(&mut events), None, "{item:?} repeating");
                assert_eq!(events, []);
            }
            // S alone and with Shift, and the Inspector's Undo.
            let others = vec![
                pressed(egui::Key::S, Modifiers::NONE, false),
                pressed(egui::Key::S, Modifiers::SHIFT, false),
                pressed(egui::Key::Z, command, false),
            ];
            let mut events = others.clone();
            assert_eq!(shortcut(&mut events), None);
            assert_eq!(events, others);
        }
    }

    /// A shortcut goes by its key's position: the key at S's chooses Save whatever the layout calls
    /// it. The key a layout calls S elsewhere, as Colemak's at D's, chooses nothing and is left.
    #[test]
    fn the_shortcuts_go_by_where_their_keys_are() {
        let command = Modifiers::CTRL.plus(Modifiers::COMMAND);
        let at = |key, physical| egui::Event::Key {
            key,
            physical_key: Some(physical),
            pressed: true,
            repeat: false,
            modifiers: command,
        };
        let mut events = vec![at(egui::Key::R, egui::Key::S)];
        assert_eq!(shortcut(&mut events), Some(Item::Save));
        assert_eq!(events, []);
        let elsewhere = vec![at(egui::Key::S, egui::Key::D)];
        let mut events = elsewhere.clone();
        assert_eq!(shortcut(&mut events), None);
        assert_eq!(events, elsewhere);
    }

    /// A message shows until Escape or a click outside dismisses it; the prompt until it's
    /// answered, which those cancel.
    #[test]
    fn a_message_and_the_prompt_show_until_theyre_dismissed() {
        let message = Message {
            title: "Can't save arena.ron".into(),
            text: "why".into(),
        };
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let frame = |events: Vec<egui::Event>, prompting: bool| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let mut shown = None;
            let mut output = ctx.run_ui(input, |ui| {
                shown = Some(if prompting {
                    prompt(ui.ctx(), "Save arena.ron?", "Or not", true) == Some(Choice::Cancel)
                } else {
                    message.show(ui.ctx())
                });
            });
            // As the renderer would have.
            output.textures_delta.clear();
            shown.expect("shown")
        };
        // A corner, outside them, which show in the middle.
        let corner = egui::pos2(4.0, 4.0);
        let click = |pressed| egui::Event::PointerButton {
            pos: corner,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let escape = vec![pressed(egui::Key::Escape, Modifiers::NONE, false)];
        let outside = vec![egui::Event::PointerMoved(corner), click(true), click(false)];
        for prompting in [false, true] {
            for dismissal in [escape.clone(), outside.clone()] {
                assert!(!frame(Vec::new(), prompting), "{dismissal:?}");
                // A frame each, as egui gets them.
                let mut frames = dismissal.into_iter();
                let dismissed = frames.any(|event| frame(vec![event], prompting));
                assert!(dismissed, "prompting: {prompting}");
            }
        }
    }
}
