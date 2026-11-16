//! The File menu's items and their shortcuts, and what the editor shows for them in front of its
//! UI: the prompt that asks whether to save the open scene's unsaved edits first, and a message
//! that says why a scene couldn't be opened or saved.

use gg_engine::egui::{self, Button, Id, KeyboardShortcut, Modal};

use crate::shortcuts::{Does, SHORTCUTS};

/// The widest the prompt and a message get, in points, so that a long error wraps.
const WIDTH: f32 = 400.0;

/// An item of the File menu, which its shortcut chooses too; Quit, below them, has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Has the game empty the open scene, which is untitled until it's saved.
    New,
    /// Picks a scene, which the game opens in place of the open one.
    Open,
    /// Has the game save the open scene to its file, or, without one, as Save As does.
    Save,
    /// Picks a file, which the game saves the open scene to, and which is then its file.
    SaveAs,
}

impl Item {
    /// The items, in the menu's order.
    pub const MENU: [Self; 4] = [Self::New, Self::Open, Self::Save, Self::SaveAs];

    /// What the menu calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Open => "Open...",
            Self::Save => "Save",
            Self::SaveAs => "Save As...",
        }
    }

    /// The keys that choose it, as the menu names them: N, O or S with Ctrl, or Cmd on macOS, and
    /// for Save As, S with Shift too; each the key at that position, as [`shortcut`] says, and as
    /// [`SHORTCUTS`] has them.
    pub fn shortcut(self) -> KeyboardShortcut {
        let shortcut = SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.does == Does::File(self));
        shortcut.expect("each item has a shortcut").egui()
    }
}

/// Which item of the File menu a shortcut pressed among `events`, a frame's input to egui,
/// chooses, if one does: the first's. A shortcut's key is the one at N, O or S's position, as the
/// engine's keys are positions, whatever the layout calls it, so that the editor's view takes out
/// the same keys of what the game gets. Takes out the shortcuts' presses and their repeats, which
/// choose nothing, so that one held down saves, or shows a dialog, once.
pub fn shortcut(events: &mut Vec<egui::Event>) -> Option<Item> {
    // In the table's order, Save As's before Save's, since a shortcut matches with Shift held as
    // well.
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

/// Shows the prompt that asks whether to save the open scene's unsaved edits first, which `title`
/// asks and `text` says more of, in front of the UI, which it keeps clicks from: with Save, while
/// `saves`, Don't Save and Cancel, which Escape or a click outside it chooses too. Returns what was
/// chosen this frame, if anything was.
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

/// Why a scene couldn't be opened, saved or made, which the editor shows until it's dismissed.
#[derive(Debug)]
pub struct Message {
    /// What couldn't be done, to which file, such as "Can't save arena.ron".
    pub title: String,
    /// Why: the refusal's code and message.
    pub text: String,
}

impl Message {
    /// Shows it in front of the UI, which it keeps clicks from; returns whether it was dismissed
    /// this frame: by its OK, Escape or a click outside it.
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

    /// `key` pressed with `modifiers`, as egui gets it, or repeating as it's held, if `repeat`.
    fn pressed(key: egui::Key, modifiers: Modifiers, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    /// Each of the File menu's shortcuts chooses its item, Ctrl+Shift+S Save As rather than Save,
    /// and is taken from egui's input; a repeat chooses nothing, and is taken too; and keys without
    /// Ctrl, or Cmd on macOS, or another's shortcut, are left as they were.
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

    /// A shortcut goes by where its key is, as the engine's keys do: the key at S's chooses Save
    /// whatever the layout calls it, and one the layout calls S elsewhere, as Colemak's at D's,
    /// chooses nothing, and is left as it was.
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

    /// A message shows until it's dismissed, by Escape or a click outside it; the prompt, until
    /// it's answered, which they cancel.
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
