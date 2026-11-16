//! The view's input: what each event does, and what of it goes to the game.

use std::mem;
use std::ops::ControlFlow;

use gg_api::InputEvent;
use gg_engine::{Event, Key, MouseButton, Scroll, Viewport};

use crate::handles::Reading;

use super::{Game, Probing, along, picked};

impl Game {
    /// Does what `event` does, which the view got, as the editor reads the window, `reading`, with
    /// the view at `view`: in Play, it goes to the game, in the view's pixels, and to the view's
    /// tools only if it lets go of what's held, so that no click picks, and no wheel or button
    /// turns the view to the editor's camera; otherwise the tools take it, the game getting
    /// nothing, and a press off the handles asks the game for a pick. In every mode, neither gets
    /// a key of the File menu's shortcuts, the key at N, O or S's position, as [`shortcuts_key`]
    /// says. `Break` for what the tools took.
    pub(super) fn route(
        &mut self,
        event: &Event<'_>,
        reading: &Reading,
        view: Option<Viewport>,
    ) -> ControlFlow<()> {
        let origin = view.map_or([0, 0], |view| view.origin);
        let through = self.through();
        // Escape lets go of the cursor grabbed for the game, whatever the game does with it, as a
        // game may not let go itself, and ends Play's capture; a press in the view grabs it again,
        // if the game asks. The game gets both, as its own window would give them.
        let escape = matches!(
            *event,
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            }
        );
        if through && escape {
            self.escaped = true;
            self.captured = false;
        }
        if through && matches!(*event, Event::MouseButton { pressed: true, .. }) {
            self.escaped = false;
        }
        let shortcut = shortcuts_key(event, reading.command);
        let handled = if shortcut || (through && !lets_go(event)) {
            ControlFlow::Continue(())
        } else {
            let along = along(&self.view.link);
            self.view
                .take(event, reading, picked(&self.view.link), &along)
        };
        if through
            && !shortcut
            && self.talker.is_some()
            && let Some(input) = forwarded(event, origin)
        {
            self.queue(input);
        }
        if let Some((at, toggles)) = self.view.picking.take() {
            // Within the active pick's model, so that a click on a model picked goes into it.
            self.view.asked = self.ask_pick(at, origin, self.view.link.picked());
            self.view.toggles = toggles;
        }
        // Escape closes the Probe tool's numbers, and a probe asked shows its own in their place.
        if escape {
            self.probing = None;
        }
        if let Some(at) = self.view.probing.take()
            && let Some(talker) = &self.talker
        {
            let ticket = self.tickets;
            self.tickets += 1;
            talker.probe(ticket, in_view(at, origin));
            self.probing = Some(Probing {
                ticket,
                at,
                answer: None,
            });
        }
        self.tell(reading.cursor, view);
        handled
    }

    /// Tells the game the view's input started or stopped going to it, as
    /// [`through`](Self::through) says, if it did since it was last told: as it stops, the game
    /// loses the focus, letting go of what's held; as it starts, it gets the cursor, at `cursor`,
    /// in the main window's pixels, if it's over the view, at `view`.
    pub(super) fn tell(&mut self, cursor: Option<[f64; 2]>, view: Option<Viewport>) {
        let through = self.through();
        if through == mem::replace(&mut self.told, through) || self.talker.is_none() {
            return;
        }
        if !through {
            self.queue(InputEvent::Focused(false));
            return;
        }
        if let (Some(at), Some(view)) = (cursor, view) {
            let [x, y] = in_view(at, view.origin);
            let [width, height] = view.size.map(f64::from);
            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                self.queue(InputEvent::Cursor([x, y]));
            }
        }
    }

    /// Queues `input` for the game's next `input`, a move of the cursor taking the place of one
    /// queued just before it, and the mouse's raw motion adding to the motion queued just before
    /// it: so that while an `input` waits for its answer, as while a script has stopped the game,
    /// what's queued grows with presses, as a key's, rather than with every move of the mouse. The
    /// game gets what's queued then in one frame, as a game's window gets what its OS queued while
    /// its frames waited.
    fn queue(&mut self, input: InputEvent) {
        match (self.input.last_mut(), input) {
            (Some(InputEvent::Cursor(last)), InputEvent::Cursor(at)) => *last = at,
            (Some(InputEvent::Motion([x, y])), InputEvent::Motion([by_x, by_y])) => {
                *x += by_x;
                *y += by_y;
            }
            (_, input) => self.input.push(input),
        }
    }
}

/// Whether `event` is a press, or a repeat, of a key of the File menu's shortcuts, the key at N, O
/// or S's position, whatever the layout calls it, as the menu matches it, with `command` held,
/// Ctrl, or Cmd on macOS, which the menu takes from egui: neither the game nor the view's tools
/// get it. Its release does, which changes nothing without its press.
fn shortcuts_key(event: &Event<'_>, command: bool) -> bool {
    let key = matches!(
        *event,
        Event::Key {
            key: Key::KeyN | Key::KeyO | Key::KeyS,
            pressed: true,
            ..
        }
    );
    command && key
}

/// Whether `event` lets go of what's held: a key's or a button's release, or the loss of the
/// focus, as the engine hands each past a layer that takes it.
fn lets_go(event: &Event<'_>) -> bool {
    match *event {
        Event::Key { pressed, .. } | Event::MouseButton { pressed, .. } => !pressed,
        Event::Focused(focused) => !focused,
        _ => false,
    }
}

/// `at`, in the main window's pixels, in the game's view's, whose top left is at `origin`.
pub(super) fn in_view(at: [f64; 2], origin: [u32; 2]) -> [f64; 2] {
    [at[0] - f64::from(origin[0]), at[1] - f64::from(origin[1])]
}

/// The event of the tool API's `input` that the game gets for `event`, which the view got, the
/// cursor's position taken from the main window's pixels to the view's, whose top left is at
/// `origin`; `None` for what the game reads itself, or doesn't get.
pub(super) fn forwarded(event: &Event<'_>, origin: [u32; 2]) -> Option<InputEvent> {
    let event = match *event {
        Event::Key {
            key,
            pressed,
            repeat,
        } => InputEvent::Key {
            key: key.name().to_owned(),
            pressed,
            repeat,
        },
        Event::Text(text) => InputEvent::Text(text.to_owned()),
        Event::CursorMoved { x, y } => InputEvent::Cursor(in_view([x, y], origin)),
        Event::CursorEntered => InputEvent::CursorEntered,
        Event::CursorLeft => InputEvent::CursorLeft,
        Event::MouseMotion { x, y } => InputEvent::Motion([x, y]),
        Event::MouseButton { button, pressed } => InputEvent::Button {
            button: match button {
                MouseButton::Left => gg_api::Button::Left,
                MouseButton::Right => gg_api::Button::Right,
                MouseButton::Middle => gg_api::Button::Middle,
                MouseButton::Back => gg_api::Button::Back,
                MouseButton::Forward => gg_api::Button::Forward,
                MouseButton::Other(number) => gg_api::Button::Other(number),
            },
            pressed,
        },
        Event::Scroll(Scroll::Lines { x, y }) => InputEvent::Scroll(gg_api::Scroll::Lines([x, y])),
        Event::Scroll(Scroll::Pixels { x, y }) => {
            InputEvent::Scroll(gg_api::Scroll::Pixels([x, y]))
        }
        Event::Focused(focused) => InputEvent::Focused(focused),
        _ => return None,
    };
    Some(event)
}
