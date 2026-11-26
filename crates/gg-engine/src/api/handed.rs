//! Input the tool API hands the main window, for egui as well as the layers: `gg input` reaches
//! the debug overlays' bar and panels as a person's clicks do.

use gg_api::{Button, InputEvent, Scroll};

/// What the tool API handed since egui's last pass, as egui hears it, and whether egui keeps each
/// event from the layers, as a window's events are kept ([`hand`](Self::hand)).
#[derive(Default)]
pub(crate) struct Handed {
    /// The passes' egui, from the first on; `None` while frames run no pass.
    egui: Option<egui::Context>,
    /// For the next pass, in points.
    events: Vec<egui::Event>,
    /// Where the handed cursor is, in points: egui hears it only at its next pass.
    cursor: Option<egui::Pos2>,
    /// The modifier keys the handed keys hold down.
    modifiers: egui::Modifiers,
}

impl Handed {
    /// Queues `given` for egui's next pass, a cursor at `origin` in the window as the layers get
    /// it. Returns whether egui keeps it from the layers:
    ///
    /// - a press or a scroll over one of egui's areas but the background, or while egui drags;
    /// - a key's press or text while an egui widget has the keys.
    ///
    /// Releases and cursor moves reach the layers either way, as a window's do.
    pub(crate) fn hand(&mut self, given: &InputEvent, origin: [u32; 2]) -> bool {
        let Some(egui) = &self.egui else {
            return false;
        };
        let points = egui.pixels_per_point();
        let over = |at: Option<egui::Pos2>| {
            let layer = at.and_then(|at| egui.layer_id_at(at));
            egui.egui_is_using_pointer()
                || layer.is_some_and(|layer| layer.order != egui::Order::Background)
        };
        let keys = egui.egui_wants_keyboard_input();
        let (event, keeps) = match *given {
            InputEvent::Cursor([x, y]) => {
                let at = egui::pos2(
                    ((x + f64::from(origin[0])) as f32) / points,
                    ((y + f64::from(origin[1])) as f32) / points,
                );
                self.cursor = Some(at);
                (egui::Event::PointerMoved(at), false)
            }
            InputEvent::CursorLeft => {
                self.cursor = None;
                (egui::Event::PointerGone, false)
            }
            InputEvent::Button { button, pressed } => {
                let (Some(button), Some(pos)) = (pointer_button(button), self.cursor) else {
                    return false;
                };
                let event = egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers: self.modifiers,
                };
                (event, pressed && over(self.cursor))
            }
            InputEvent::Scroll(scroll) => {
                // Both say which way the content moves as winit does.
                let (unit, delta) = match scroll {
                    Scroll::Lines([x, y]) => (egui::MouseWheelUnit::Line, egui::vec2(x, y)),
                    Scroll::Pixels([x, y]) => {
                        let delta = egui::vec2(x as f32, y as f32) / points;
                        (egui::MouseWheelUnit::Point, delta)
                    }
                };
                let event = egui::Event::MouseWheel {
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: self.modifiers,
                };
                (event, over(self.cursor))
            }
            InputEvent::Key {
                ref key,
                pressed,
                repeat,
            } => {
                self.hold(key, pressed);
                let Some(egui_key) = egui::Key::from_name(key) else {
                    return pressed && keys;
                };
                let event = egui::Event::Key {
                    key: egui_key,
                    physical_key: Some(egui_key),
                    pressed,
                    repeat,
                    modifiers: self.modifiers,
                };
                (event, pressed && keys)
            }
            InputEvent::Text(ref text) => (egui::Event::Text(text.clone()), keys),
            InputEvent::Focused(focused) => (egui::Event::WindowFocused(focused), false),
            // The server makes waits, aims and actions into ticks, a cursor and a binding's events
            // first. egui hears no gamepad.
            InputEvent::CursorEntered
            | InputEvent::Motion(_)
            | InputEvent::Wait(_)
            | InputEvent::Aim(_)
            | InputEvent::Action { .. }
            | InputEvent::PadButton { .. }
            | InputEvent::PadStick { .. }
            | InputEvent::PadTrigger { .. } => return false,
        };
        self.events.push(event);
        keeps
    }

    /// Adds what was handed since the last pass to `input`, `egui`'s next.
    pub(crate) fn take(&mut self, egui: &egui::Context, input: &mut egui::RawInput) {
        if self.egui.is_none() {
            self.egui = Some(egui.clone());
        }
        input.events.append(&mut self.events);
    }

    /// For a frame that runs no pass: nothing handed waits for one.
    pub(crate) fn no_pass(&mut self) {
        self.egui = None;
        self.events.clear();
    }

    /// Notes a modifier key `key` going down or up.
    fn hold(&mut self, key: &str, pressed: bool) {
        let held = match key {
            "ShiftLeft" | "ShiftRight" => &mut self.modifiers.shift,
            "ControlLeft" | "ControlRight" => &mut self.modifiers.ctrl,
            "AltLeft" | "AltRight" => &mut self.modifiers.alt,
            "MetaLeft" | "MetaRight" | "SuperLeft" | "SuperRight" => &mut self.modifiers.mac_cmd,
            _ => return,
        };
        *held = pressed;
        let m = &mut self.modifiers;
        m.command = if cfg!(target_os = "macos") {
            m.mac_cmd
        } else {
            m.ctrl
        };
    }
}

fn pointer_button(button: Button) -> Option<egui::PointerButton> {
    Some(match button {
        Button::Left => egui::PointerButton::Primary,
        Button::Right => egui::PointerButton::Secondary,
        Button::Middle => egui::PointerButton::Middle,
        Button::Back => egui::PointerButton::Extra1,
        Button::Forward => egui::PointerButton::Extra2,
        Button::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `egui` with a button at its top left, and the input `handed` gave it.
    fn pass(egui: &egui::Context, handed: &mut Handed) -> bool {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        handed.take(egui, &mut input);
        let mut clicked = false;
        let mut output = egui.run_ui(input, |root| {
            egui::Area::new(egui::Id::unique("bar"))
                .fixed_pos(egui::pos2(0.0, 0.0))
                .show(root, |ui| clicked = ui.button("Menu").clicked());
        });
        output.textures_delta.clear();
        clicked
    }

    fn click(at: [f64; 2]) -> [InputEvent; 3] {
        [
            InputEvent::Cursor(at),
            InputEvent::Button {
                button: Button::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: Button::Left,
                pressed: false,
            },
        ]
    }

    #[test]
    fn egui_hears_a_handed_click_and_keeps_it_from_the_layers() {
        let egui = egui::Context::default();
        let mut handed = Handed::default();
        // No pass yet: egui keeps nothing, and hears nothing.
        assert!(!handed.hand(&click([5.0, 5.0])[1], [0, 0]));
        // An area shows from its second pass, sized in its first.
        assert!(!pass(&egui, &mut handed));
        assert!(!pass(&egui, &mut handed));
        let kept = click([10.0, 8.0]).map(|event| handed.hand(&event, [0, 0]));
        assert_eq!(kept, [false, true, false], "egui keeps the press alone");
        assert!(pass(&egui, &mut handed), "the button heard the click");
        let kept = click([300.0, 200.0]).map(|event| handed.hand(&event, [0, 0]));
        assert_eq!(
            kept, [false; 3],
            "a click off egui's areas reaches the layers"
        );
        assert!(!pass(&egui, &mut handed));
    }
}
