//! What of the main window's input reaches the layers in a dockspace, whose game's view is a panel
//! among egui's: the rule windowed runs route that input by.

use tracing::debug;

use crate::dock::Under;
use crate::{Event, MouseButton};

/// What of the main window's input reaches the layers in a dockspace, whose game's view is a panel
/// among egui's: the mouse's presses and its wheel over the view, and the drags those start; and
/// keys, as [`keys`](Self::keys) says. Modifier keys reach them wherever they're pressed, while
/// none of egui's widgets has focus, so that one pressed over a panel and held onto the view counts
/// there; and releases reach them wherever they come, so that none is left holding what it saw
/// pressed.
pub(crate) struct ViewInput {
    /// Whether the view has the keys, as it does from the start.
    focused: bool,
    /// The mouse buttons pressed over the view and not yet let go, a bit each, as [`bit`] gives.
    held: u8,
    /// The cursor's last move, if the layers didn't get it, in physical pixels.
    withheld: Option<(f64, f64)>,
    /// Whether the cursor is in the window: from a move in it until it leaves.
    inside: bool,
}

/// What of an event reaches the layers.
#[derive(Debug, PartialEq)]
pub(crate) enum Reach {
    Nothing,
    Event,
    /// The cursor's move to `x` and `y`, which they didn't get, then the event: a press, which a
    /// drag then starts from.
    MoveThen {
        x: f64,
        y: f64,
    },
}

impl ViewInput {
    pub(crate) fn new() -> Self {
        Self {
            focused: true,
            held: 0,
            withheld: None,
            inside: false,
        }
    }

    /// Whether keys reach the layers, their presses, repeats and text, and Tab, which goes past
    /// egui to them alone then: while none of egui's widgets has focus, no modal of egui's is
    /// open, which keeps them whether the view has them or the cursor, and the view shows
    /// (`shown`); and either the view has the keys or the cursor is in the window over the view or
    /// the UI over it, as `under` says, while egui drags none of its own widgets, as a press there
    /// needs too.
    pub(crate) fn keys(&self, egui: &egui::Context, under: Under, shown: bool) -> bool {
        let free = !egui.egui_wants_keyboard_input()
            && egui.memory(|memory| memory.top_modal_layer()).is_none();
        let hovered = self.inside
            && matches!(under, Under::View | Under::Overlay)
            && (self.held != 0 || !egui.egui_is_using_pointer());
        free && shown && (self.focused || hovered)
    }

    /// What of `event` reaches the layers, as egui keeps it if `consumed`, with the cursor over
    /// what `under` says, as the view, which `shown` says shows. Releases always reach them, and a
    /// modifier key's press does unless egui keeps it, as it keeps keys while one of its widgets has
    /// focus. A press over the view, or over the UI over it, gives it focus and takes egui's.
    pub(crate) fn route(
        &mut self,
        egui: &egui::Context,
        event: &Event<'_>,
        consumed: bool,
        under: Under,
        shown: bool,
    ) -> Reach {
        let reach = |reaches: bool| {
            if reaches {
                Reach::Event
            } else {
                Reach::Nothing
            }
        };
        match *event {
            Event::MouseButton {
                button,
                pressed: true,
            } => match under {
                // egui uses the pointer for the view's own widget while a button is held there, and
                // otherwise for one of its own, such as a splitter dragged over the view.
                Under::View if self.held != 0 || !egui.egui_is_using_pointer() => {
                    self.held |= bit(button);
                    // egui keeps a text field's focus until a click elsewhere, which a drag isn't.
                    egui.memory_mut(|memory| memory.stop_text_input());
                    self.focus(true);
                    match self.withheld.take() {
                        Some((x, y)) => Reach::MoveThen { x, y },
                        None => Reach::Event,
                    }
                }
                // The UI over the view is egui's, but leaves the view the keys.
                Under::Overlay => {
                    egui.memory_mut(|memory| memory.stop_text_input());
                    self.focus(true);
                    Reach::Nothing
                }
                _ => {
                    self.focus(false);
                    Reach::Nothing
                }
            },
            Event::MouseButton {
                button,
                pressed: false,
            } => {
                self.held &= !bit(button);
                Reach::Event
            }
            Event::Scroll(_) => {
                reach(self.held != 0 || (under == Under::View && !egui.egui_is_using_pointer()))
            }
            // As keys do, or while a drag the view started goes on.
            Event::MouseMotion { .. } => reach(self.held != 0 || self.keys(egui, under, shown)),
            // egui uses the pointer while a button pressed over the view is held.
            Event::CursorMoved { x, y } => {
                self.inside = true;
                let reaches = !consumed || self.held != 0;
                self.withheld = (!reaches).then_some((x, y));
                reach(reaches)
            }
            Event::CursorLeft => {
                self.inside = false;
                reach(!consumed)
            }
            Event::Key {
                key, pressed: true, ..
            } if modifier(key) => reach(!consumed),
            Event::Key { pressed: true, .. } | Event::Text(_) => {
                reach(!consumed && self.keys(egui, under, shown))
            }
            Event::Key { pressed: false, .. } => Reach::Event,
            Event::Focused(false) => {
                self.held = 0;
                Reach::Event
            }
            _ => reach(!consumed),
        }
    }

    /// Whether the view has the keys, as it does from the start, and from a press over it, or the
    /// UI over it, until a press elsewhere.
    pub(crate) fn focused(&self) -> bool {
        self.focused
    }

    /// Whether the cursor is in the window: from a move in it until it leaves.
    pub(crate) fn inside(&self) -> bool {
        self.inside
    }

    fn focus(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            debug!(focused, "View focus");
        }
    }
}

/// Whether `key` is a modifier: Shift, Ctrl, Alt or Super, on either side.
fn modifier(key: crate::Key) -> bool {
    use crate::Key;
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// `button`'s bit among those [`ViewInput`] holds; none for a button without a name.
fn bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
        MouseButton::Other(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Id, PointerButton, RawInput, Rect, Sense, pos2, vec2};

    use super::*;
    use crate::{Key, Scroll};

    /// egui, after a pass that had a widget pressed if `pressing`, so that egui uses the pointer.
    fn context(pressing: bool) -> egui::Context {
        let egui = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        let at = pos2(50.0, 50.0);
        let press = egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        // The widget is in egui's hit test from the pass after it's drawn.
        for events in [vec![egui::Event::PointerMoved(at)], vec![press]] {
            let events = if pressing { events } else { Vec::new() };
            let input = RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            };
            let mut output = egui.run_ui(input, |ui| {
                ui.interact(screen, Id::unique("widget"), Sense::drag());
            });
            output.textures_delta.clear();
        }
        assert_eq!(egui.egui_is_using_pointer(), pressing);
        egui
    }

    fn button(button: MouseButton, pressed: bool) -> Event<'static> {
        Event::MouseButton { button, pressed }
    }

    fn key(pressed: bool) -> Event<'static> {
        Event::Key {
            key: Key::KeyW,
            pressed,
            repeat: false,
        }
    }

    const WHEEL: Event<'static> = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
    const MOVE: Event<'static> = Event::CursorMoved { x: 10.0, y: 20.0 };

    #[test]
    fn the_view_has_the_keys_from_the_start_and_from_a_press_on_it() {
        let egui = context(false);
        let mut input = ViewInput::new();
        // Before it first shows, keys reach nothing.
        assert_eq!(
            input.route(&egui, &key(true), false, Under::Other, false),
            Reach::Nothing
        );
        for event in [key(true), Event::Text("w"), key(false)] {
            assert_eq!(
                input.route(&egui, &event, false, Under::Other, true),
                Reach::Event
            );
        }
        // A press elsewhere takes the keys away, but not their releases.
        let elsewhere = button(MouseButton::Left, true);
        assert_eq!(
            input.route(&egui, &elsewhere, true, Under::Other, true),
            Reach::Nothing
        );
        assert!(!input.keys(&egui, Under::Other, true));
        for (event, reach) in [
            (key(true), Reach::Nothing),
            (Event::Text("w"), Reach::Nothing),
            (key(false), Reach::Event),
        ] {
            assert_eq!(input.route(&egui, &event, false, Under::Other, true), reach);
        }
        // A press on the view, which egui counts as its own, gives them back.
        let press = button(MouseButton::Left, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        assert!(input.keys(&egui, Under::Other, true));
        // Not those egui keeps, as while one of its widgets has focus.
        assert_eq!(
            input.route(&egui, &key(true), true, Under::View, true),
            Reach::Nothing
        );
    }

    #[test]
    fn a_press_on_the_view_takes_egui_s_focus() {
        let egui = context(false);
        egui.memory_mut(|memory| memory.request_focus(Id::unique("text")));
        assert!(egui.egui_wants_keyboard_input());
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        assert!(!egui.egui_wants_keyboard_input());
    }

    #[test]
    fn a_button_held_from_the_view_brings_the_layers_moves_and_the_wheel() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let right = |pressed| button(MouseButton::Right, pressed);
        assert_eq!(
            input.route(&egui, &right(true), true, Under::View, true),
            Reach::Event
        );
        // egui uses the pointer for the view's widget now, and the drag goes past the view.
        let using = context(true);
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&using, &event, true, Under::Other, true),
                Reach::Event
            );
        }
        // Another button pressed over the view joins in.
        let left = |pressed| button(MouseButton::Left, pressed);
        assert_eq!(
            input.route(&using, &left(true), true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &right(false), true, Under::Other, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &MOVE, true, Under::Other, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&using, &left(false), true, Under::Other, true),
            Reach::Event
        );
        // Let go, egui's moves and wheel are egui's.
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&using, &event, true, Under::View, true),
                Reach::Nothing
            );
        }
        // The layers get the move they missed before the next press, which a drag starts from.
        let next = input.route(&egui, &right(true), true, Under::View, true);
        assert_eq!(next, Reach::MoveThen { x: 10.0, y: 20.0 });
        // A move they get leaves none to miss.
        input.route(&egui, &right(false), false, Under::View, true);
        input.route(&egui, &MOVE, false, Under::View, true);
        assert_eq!(
            input.route(&egui, &right(true), true, Under::View, true),
            Reach::Event
        );
    }

    #[test]
    fn a_press_over_the_view_while_egui_drags_one_of_its_own_is_egui_s() {
        let using = context(true);
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&using, &press, true, Under::View, true),
            Reach::Nothing
        );
        assert!(!input.keys(&using, Under::Other, true));
        assert_eq!(
            input.route(&using, &WHEEL, true, Under::View, true),
            Reach::Nothing
        );
    }

    /// The mouse's raw motion reaches the layers as keys do, while the view has them, as it does
    /// grabbing the cursor, and while a drag the view started goes on.
    #[test]
    fn the_mouses_motion_reaches_the_layers_while_the_view_has_the_keys_or_a_drag() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let motion = Event::MouseMotion { x: 3.0, y: -2.0 };
        assert_eq!(
            input.route(&egui, &motion, false, Under::Other, true),
            Reach::Event
        );
        assert!(input.focused());
        let left = |pressed| button(MouseButton::Left, pressed);
        input.route(&egui, &left(true), true, Under::Other, true);
        assert!(!input.focused());
        assert_eq!(
            input.route(&egui, &motion, false, Under::Other, true),
            Reach::Nothing
        );
        let right = |pressed| button(MouseButton::Right, pressed);
        input.route(&egui, &right(true), true, Under::View, true);
        let using = context(true);
        assert_eq!(
            input.route(&using, &motion, false, Under::Other, true),
            Reach::Event
        );
        // Nor while the view doesn't show.
        input.route(&egui, &right(false), false, Under::View, true);
        assert_eq!(
            input.route(&egui, &motion, false, Under::View, false),
            Reach::Nothing
        );
    }

    #[test]
    fn the_wheel_over_the_view_reaches_the_layers_and_elsewhere_doesn_t() {
        let egui = context(false);
        let mut input = ViewInput::new();
        assert_eq!(
            input.route(&egui, &WHEEL, true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &WHEEL, true, Under::Other, true),
            Reach::Nothing
        );
    }

    #[test]
    fn a_press_on_ui_over_the_view_gives_it_the_keys_and_reaches_egui_alone() {
        let egui = context(false);
        egui.memory_mut(|memory| memory.request_focus(Id::unique("text")));
        let mut input = ViewInput::new();
        let left = |pressed| button(MouseButton::Left, pressed);
        assert_eq!(
            input.route(&egui, &left(true), true, Under::Other, true),
            Reach::Nothing
        );
        assert!(!input.focused);
        assert_eq!(
            input.route(&egui, &left(true), true, Under::Overlay, true),
            Reach::Nothing
        );
        assert!(input.keys(&egui, Under::Other, true));
        assert!(!egui.egui_wants_keyboard_input());
        // Its release reaches the layers, as every release does, but it holds nothing.
        assert_eq!(
            input.route(&egui, &left(false), true, Under::Overlay, true),
            Reach::Event
        );
        for event in [MOVE, WHEEL] {
            assert_eq!(
                input.route(&egui, &event, true, Under::Overlay, true),
                Reach::Nothing
            );
        }
    }

    #[test]
    fn losing_the_window_s_focus_lets_go_of_what_s_held() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let press = button(MouseButton::Right, true);
        assert_eq!(
            input.route(&egui, &press, true, Under::View, true),
            Reach::Event
        );
        let lost = Event::Focused(false);
        assert_eq!(
            input.route(&egui, &lost, false, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &MOVE, true, Under::View, true),
            Reach::Nothing
        );
        // The view keeps the keys.
        assert!(input.keys(&egui, Under::Other, true));
    }

    #[test]
    fn buttons_without_a_name_reach_the_layers_but_hold_nothing() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let other = button(MouseButton::Other(7), true);
        assert_eq!(
            input.route(&egui, &other, true, Under::View, true),
            Reach::Event
        );
        assert_eq!(
            input.route(&egui, &MOVE, true, Under::View, true),
            Reach::Nothing
        );
    }

    /// A view whose keys a press elsewhere took, with the cursor moved into the window.
    fn unfocused(egui: &egui::Context) -> ViewInput {
        let mut input = ViewInput::new();
        let elsewhere = button(MouseButton::Left, true);
        assert_eq!(
            input.route(egui, &elsewhere, true, Under::Other, true),
            Reach::Nothing
        );
        input.route(egui, &MOVE, false, Under::Other, true);
        assert!(!input.focused);
        input
    }

    /// Once a press on a panel has taken the view's focus, keys follow the cursor: they reach the
    /// layers while it's over the view or the toolbar over it, their repeats and text too; but not
    /// while it's over a panel, nor while the view doesn't show. Hovering gives the view no focus.
    #[test]
    fn keys_follow_the_cursor_onto_the_view_and_the_ui_over_it() {
        let egui = context(false);
        let mut input = unfocused(&egui);
        let repeat = Event::Key {
            key: Key::KeyW,
            pressed: true,
            repeat: true,
        };
        for under in [Under::View, Under::Overlay] {
            assert!(input.keys(&egui, under, true), "{under:?}");
            for event in [key(true), repeat, Event::Text("w")] {
                let reached = input.route(&egui, &event, false, under, true);
                assert_eq!(reached, Reach::Event, "{event:?} over {under:?}");
            }
            assert!(!input.keys(&egui, under, false), "{under:?}, hidden");
        }
        assert!(!input.keys(&egui, Under::Other, true));
        for event in [key(true), repeat, Event::Text("w")] {
            let reached = input.route(&egui, &event, false, Under::Other, true);
            assert_eq!(reached, Reach::Nothing, "{event:?} over a panel");
        }
        assert!(!input.focused);
    }

    /// While a text field has focus, keys are its, wherever the cursor is and whether or not the
    /// view has focus.
    #[test]
    fn keys_reach_nothing_while_a_text_field_has_focus() {
        let egui = context(false);
        egui.memory_mut(|memory| memory.request_focus(Id::unique("text")));
        let hovering = unfocused(&egui);
        let mut focused = ViewInput::new();
        focused.route(&egui, &MOVE, false, Under::View, true);
        for input in [hovering, focused] {
            let mut input = input;
            for under in [Under::View, Under::Overlay, Under::Other] {
                assert!(!input.keys(&egui, under, true), "{under:?}");
                // egui-winit says egui keeps them.
                for event in [key(true), Event::Text("w")] {
                    let reached = input.route(&egui, &event, true, under, true);
                    assert_eq!(reached, Reach::Nothing, "{event:?} over {under:?}");
                }
            }
        }
    }

    /// egui after a pass that showed a modal, which keeps it open in the passes after.
    fn showing_a_modal() -> egui::Context {
        let egui = egui::Context::default();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0))),
            ..RawInput::default()
        };
        let mut output = egui.run_ui(input, |ui| {
            egui::Modal::new(Id::unique("modal")).show(ui.ctx(), |ui| ui.label("A message"));
        });
        output.textures_delta.clear();
        assert!(egui.memory(|memory| memory.top_modal_layer()).is_some());
        egui
    }

    /// Under a modal, keys reach nothing, though the view has focus or the cursor is over it; but
    /// modifiers do, while none of egui's widgets has focus.
    #[test]
    fn keys_reach_nothing_under_a_modal() {
        let egui = showing_a_modal();
        assert!(!egui.egui_wants_keyboard_input());
        let mut focused = ViewInput::new();
        focused.route(&egui, &MOVE, false, Under::View, true);
        for mut input in [unfocused(&egui), focused] {
            assert!(!input.keys(&egui, Under::View, true));
            let reached = input.route(&egui, &key(true), false, Under::View, true);
            assert_eq!(reached, Reach::Nothing);
            let alt = Event::Key {
                key: Key::AltLeft,
                pressed: true,
                repeat: false,
            };
            assert_eq!(
                input.route(&egui, &alt, false, Under::View, true),
                Reach::Event
            );
        }
    }

    /// The cursor counts over the view only from a move in the window until it leaves it, where
    /// it was last over the view: keys don't follow it out of the window.
    #[test]
    fn keys_dont_follow_the_cursor_out_of_the_window() {
        let egui = context(false);
        let mut input = ViewInput::new();
        let elsewhere = button(MouseButton::Left, true);
        input.route(&egui, &elsewhere, true, Under::Other, true);
        assert!(!input.keys(&egui, Under::View, true), "before a move");
        input.route(&egui, &MOVE, false, Under::View, true);
        assert!(input.keys(&egui, Under::View, true));
        let left = input.route(&egui, &Event::CursorLeft, false, Under::View, true);
        assert_eq!(left, Reach::Event);
        assert!(!input.keys(&egui, Under::View, true), "after it left");
        assert_eq!(
            input.route(&egui, &key(true), false, Under::View, true),
            Reach::Nothing
        );
    }

    /// While egui drags one of its own widgets across the view, such as a splitter, the cursor
    /// over the view gives it no keys, as it gives the layers no press.
    #[test]
    fn keys_dont_follow_a_drag_of_egui_s_own_across_the_view() {
        let egui = context(false);
        let using = context(true);
        let mut input = unfocused(&egui);
        assert!(!input.keys(&using, Under::View, true));
        assert_eq!(
            input.route(&using, &key(true), false, Under::View, true),
            Reach::Nothing
        );
        assert!(input.keys(&egui, Under::View, true), "once it's let go");
    }

    /// Modifiers pressed over a panel reach the layers, so that one held onto the view counts
    /// there, but not while a widget of egui's has them; other keys there don't.
    #[test]
    fn modifiers_pressed_over_a_panel_reach_the_layers() {
        let egui = context(false);
        let mut input = unfocused(&egui);
        for modifier in [
            Key::ShiftLeft,
            Key::ShiftRight,
            Key::ControlLeft,
            Key::ControlRight,
            Key::AltLeft,
            Key::AltRight,
            Key::SuperLeft,
            Key::SuperRight,
        ] {
            let pressed = Event::Key {
                key: modifier,
                pressed: true,
                repeat: false,
            };
            for shown in [true, false] {
                let reached = input.route(&egui, &pressed, false, Under::Other, shown);
                assert_eq!(reached, Reach::Event, "{modifier:?}, shown {shown}");
            }
            let kept = input.route(&egui, &pressed, true, Under::Other, true);
            assert_eq!(kept, Reach::Nothing, "{modifier:?} egui keeps");
        }
        assert_eq!(
            input.route(&egui, &key(true), false, Under::Other, true),
            Reach::Nothing
        );
    }
}
