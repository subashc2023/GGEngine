//! Gamepads, through gilrs: the only code that names it. What the layers see of a pad goes through
//! `Pads`, which knows nothing of gilrs: its dead zones, a stick's and a trigger's moves as one
//! event, and the focus rule.

use std::mem;
use std::time::{Duration, Instant};

use gg_core::math::Vec2;
use gilrs::ev::filter::axis_dpad_to_button;
use gilrs::{Axis, Button, EventType, Filter, GamepadId, Gilrs, GilrsBuilder};
use tracing::{info, warn};

use crate::{Event, Pad, PadButton, Stick, Trigger};

/// How far from rest a stick or a trigger has to go to count, and how far counts as all the way,
/// for a worn stick to still reach it: Microsoft's dead zone for the pads Windows.Gaming.Input
/// reads, which gilrs gives them too.
const DEAD_ZONE: f32 = 0.1;
const FULL: f32 = 0.9;
/// How long after the main window gains focus what the pads do starts to count: long enough for
/// gilrs's threads, which read the pads every 8 ms on Windows.Gaming.Input and every 10 on XInput,
/// to have read them in focus, since Windows gives pads to the focused window alone, so that what's
/// held as focus comes back waits for its next press. A backend slower than that to read them
/// again would have it count at once.
const SETTLE: Duration = Duration::from_millis(40);

/// The gamepads of a windowed run. Headless runs and replays have none.
#[derive(Default)]
pub(crate) struct Gamepads {
    /// `None` when gilrs couldn't start.
    gilrs: Option<Gilrs>,
    pads: Pads,
}

impl Gamepads {
    /// Starts gilrs, or logs why it couldn't and goes on without pads; `focused` is whether the
    /// main window has focus.
    pub(crate) fn open(focused: bool) -> Self {
        let _zone = gg_core::zone!();
        // Without gilrs's filters, whose dead zone differs by backend, since the engine applies its
        // own; and without its state kept as events come, since the D-pad's filter that `poll` runs
        // reads that state, which `update` then has to write after it. No force feedback, which
        // would start a thread of its own.
        let built = GilrsBuilder::new()
            .with_default_filters(false)
            .set_update_state(false)
            .with_force_feedback(false)
            .build();
        let gilrs = match built {
            Ok(gilrs) => gilrs,
            Err(error) => {
                warn!(
                    code = "engine.gamepads",
                    "Gamepads are off, since gilrs couldn't start: {error}"
                );
                return Self::default();
            }
        };
        // The pads gilrs found as it started, which some backends report as connected too.
        for (id, pad) in gilrs.gamepads() {
            info!(
                pad = usize::from(id),
                name = pad.name(),
                "Gamepad connected"
            );
        }
        Self {
            gilrs: Some(gilrs),
            pads: Pads {
                counts_from: focused.then(Instant::now),
                counting: focused,
                pads: Vec::new(),
            },
        }
    }

    /// Gives `emit` what the pads did since the last poll, as the layers see it.
    pub(crate) fn poll(&mut self, mut emit: impl FnMut(Event<'static>)) {
        let Some(gilrs) = &mut self.gilrs else {
            return;
        };
        let _zone = gg_core::zone!();
        // A D-pad that reports as a hat, as on Linux and macOS, becomes its four buttons.
        while let Some(event) = gilrs.next_event().filter_ev(&axis_dpad_to_button, gilrs) {
            if event.is_dropped() {
                continue;
            }
            gilrs.update(&event);
            if let Some(reading) = reading(gilrs, event.id, event.event) {
                self.pads.read(reading, &mut emit);
            }
        }
        gilrs.inc();
        self.pads.moved(Instant::now(), &mut emit);
    }

    /// Tells the pads that the main window gained or lost focus, before the layers are told.
    pub(crate) fn focus(&mut self, focused: bool, mut emit: impl FnMut(Event<'static>)) {
        self.pads.focus(focused, Instant::now(), &mut emit);
    }

    /// For the main window restored from being minimized, while no frame polled the pads.
    pub(crate) fn restored(&mut self, mut emit: impl FnMut(Event<'static>)) {
        self.pads.restored(Instant::now(), &mut emit);
    }
}

/// One of gilrs's events in the engine's names, or `None` for what the engine doesn't name.
fn reading(gilrs: &Gilrs, id: GamepadId, event: EventType) -> Option<Reading> {
    let pad = Pad(usize::from(id));
    Some(match event {
        EventType::ButtonPressed(button, _) => Reading::Button(pad, pad_button(button)?, true),
        EventType::ButtonReleased(button, _) => Reading::Button(pad, pad_button(button)?, false),
        // A digital button's changes repeat its presses and releases.
        EventType::ButtonChanged(button, value, _) => match pad_button(button)? {
            PadButton::LeftTrigger => Reading::Trigger(pad, Trigger::Left, value),
            PadButton::RightTrigger => Reading::Trigger(pad, Trigger::Right, value),
            _ => return None,
        },
        EventType::AxisChanged(axis, value, _) => {
            let (stick, axis) = stick_axis(axis)?;
            Reading::Stick(pad, stick, axis, value)
        }
        EventType::Connected => {
            // `connected_gamepad` rather than `gamepad`, which can panic (gilrs #206).
            let connected = gilrs.connected_gamepad(id);
            let name = connected.as_ref().map(|pad| pad.name());
            info!(pad = pad.0, name, "Gamepad connected");
            Reading::Connected(pad)
        }
        EventType::Disconnected => {
            info!(pad = pad.0, "Gamepad disconnected");
            Reading::Disconnected(pad)
        }
        _ => return None,
    })
}

/// gilrs's names for the buttons, whose `LeftTrigger` is the bumper and `LeftTrigger2` the trigger.
/// C and Z, which few pads have, have no name.
fn pad_button(button: Button) -> Option<PadButton> {
    Some(match button {
        Button::South => PadButton::South,
        Button::East => PadButton::East,
        Button::North => PadButton::North,
        Button::West => PadButton::West,
        Button::LeftTrigger => PadButton::LeftShoulder,
        Button::RightTrigger => PadButton::RightShoulder,
        Button::LeftTrigger2 => PadButton::LeftTrigger,
        Button::RightTrigger2 => PadButton::RightTrigger,
        Button::Select => PadButton::Select,
        Button::Start => PadButton::Start,
        Button::Mode => PadButton::Mode,
        Button::LeftThumb => PadButton::LeftThumb,
        Button::RightThumb => PadButton::RightThumb,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        Button::C | Button::Z | Button::Unknown => return None,
    })
}

/// A stick's axis, 0 across and 1 up. A D-pad left as a hat, and the Z axes some Linux drivers
/// report the triggers as (gilrs #200), have none.
fn stick_axis(axis: Axis) -> Option<(Stick, usize)> {
    Some(match axis {
        Axis::LeftStickX => (Stick::Left, 0),
        Axis::LeftStickY => (Stick::Left, 1),
        Axis::RightStickX => (Stick::Right, 0),
        Axis::RightStickY => (Stick::Right, 1),
        Axis::LeftZ | Axis::RightZ | Axis::DPadX | Axis::DPadY | Axis::Unknown => return None,
    })
}

/// A pad's reading, in the engine's names.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reading {
    Button(Pad, PadButton, bool),
    /// One of a stick's two axes, 0 across and 1 up, from -1 to 1.
    Stick(Pad, Stick, usize, f32),
    Trigger(Pad, Trigger, f32),
    Connected(Pad),
    Disconnected(Pad),
}

/// What the layers see of the pads: each reading through its dead zone, a stick's and a trigger's
/// moves at their last reading, once a poll and before each of its pad's button events, so that a
/// press sees the moves that came before it; and nothing out of focus, on every platform, as
/// Windows gives pads to the focused window alone where Linux and macOS give them to any.
#[derive(Debug, Default)]
struct Pads {
    /// From when what the pads do counts: `SETTLE` after the main window gained focus, or `None`
    /// while it hasn't.
    counts_from: Option<Instant>,
    /// Whether it counts, as of the last poll.
    counting: bool,
    /// Each pad's controls, by its number, from its first reading.
    pads: Vec<Controls>,
}

/// A pad's buttons, sticks and triggers, in their orders.
#[derive(Debug, Default, Clone)]
struct Controls {
    buttons: [Control; PadButton::ALL.len()],
    sticks: [Control; 2],
    triggers: [Control; 2],
}

impl Controls {
    fn each(&mut self) -> impl Iterator<Item = &mut Control> {
        let Self {
            buttons,
            sticks,
            triggers,
        } = self;
        buttons.iter_mut().chain(sticks).chain(triggers)
    }
}

/// One of a pad's buttons, sticks or triggers: a button's 0 or 1 in `x`, or a trigger's travel, or
/// a stick's place.
#[derive(Debug, Default, Clone, Copy)]
struct Control {
    /// As the pad last read, before the dead zone.
    read: Vec2,
    /// As the layers last saw it.
    shown: Vec2,
    /// Away from rest as what the pads do began to count again, so not seen until it has rested.
    muted: bool,
}

impl Control {
    /// Shows the layers the control as it is, giving that if it changed: at rest while what the
    /// pads do doesn't count, and while it's muted.
    fn show(&mut self, counting: bool) -> Option<Vec2> {
        let value = dead_zone(self.read);
        self.muted &= value != Vec2::ZERO;
        let seen = if counting && !self.muted {
            value
        } else {
            Vec2::ZERO
        };
        (seen != self.shown).then(|| {
            self.shown = seen;
            seen
        })
    }
}

/// A reading through the dead zone, which is round, so that a stick near an axis doesn't snap onto
/// it, and scaled, so that it goes from exactly 0 at its edge to 1 at `FULL`, and stops there. A
/// trigger's travel is across, and a button's 0 or 1 stays as it is.
fn dead_zone(read: Vec2) -> Vec2 {
    let length = read.length();
    if length <= DEAD_ZONE {
        return Vec2::ZERO;
    }
    let scaled = (length - DEAD_ZONE) / (FULL - DEAD_ZONE);
    (read * (scaled / length)).clamp_length_max(1.0)
}

impl Pads {
    fn read(&mut self, reading: Reading, emit: &mut impl FnMut(Event<'static>)) {
        match reading {
            Reading::Button(pad, button, pressed) => {
                self.moved_on(pad, emit);
                let counting = self.counting;
                let control = &mut self.controls(pad).buttons[button as usize];
                control.read = Vec2::new(f32::from(u8::from(pressed)), 0.0);
                if let Some(seen) = control.show(counting) {
                    let pressed = seen.x > 0.0;
                    emit(Event::PadButton {
                        pad,
                        button,
                        pressed,
                    });
                }
            }
            Reading::Stick(pad, stick, axis, value) => {
                self.controls(pad).sticks[stick as usize].read[axis] = value;
            }
            Reading::Trigger(pad, trigger, value) => {
                self.controls(pad).triggers[trigger as usize].read.x = value;
            }
            // gilrs starts a pad connected again afresh, even one it had.
            Reading::Connected(pad) | Reading::Disconnected(pad) => {
                self.let_go(pad, emit);
                *self.controls(pad) = Controls::default();
            }
        }
    }

    /// Ends a poll at `now`: gives the layers each stick and trigger that moved since they last saw
    /// it, at its last reading, once what the pads do counts. As it starts to count, what's away
    /// from rest is muted, and counts once it has rested, as a key counts from its next press.
    fn moved(&mut self, now: Instant, emit: &mut impl FnMut(Event<'static>)) {
        if !self.counting && self.counts_from.is_some_and(|from| now >= from) {
            self.counting = true;
            for control in self.pads.iter_mut().flat_map(Controls::each) {
                control.muted = dead_zone(control.read) != Vec2::ZERO;
            }
        }
        for pad in 0..self.pads.len() {
            self.moved_on(Pad(pad), emit);
        }
    }

    fn moved_on(&mut self, pad: Pad, emit: &mut impl FnMut(Event<'static>)) {
        let counting = self.counting;
        let Some(controls) = self.pads.get_mut(pad.0) else {
            return;
        };
        for (stick, control) in [Stick::Left, Stick::Right]
            .into_iter()
            .zip(&mut controls.sticks)
        {
            if let Some(value) = control.show(counting) {
                emit(Event::PadStick { pad, stick, value });
            }
        }
        let triggers = [Trigger::Left, Trigger::Right].into_iter();
        for (trigger, control) in triggers.zip(&mut controls.triggers) {
            if let Some(value) = control.show(counting) {
                let value = value.x;
                emit(Event::PadTrigger {
                    pad,
                    trigger,
                    value,
                });
            }
        }
    }

    /// Lets go of whatever the layers saw of `pad` away from rest.
    fn let_go(&mut self, pad: Pad, emit: &mut impl FnMut(Event<'static>)) {
        let Some(controls) = self.pads.get_mut(pad.0) else {
            return;
        };
        let rest = |control: &mut Control| mem::take(&mut control.shown) != Vec2::ZERO;
        for (button, control) in PadButton::ALL.into_iter().zip(&mut controls.buttons) {
            if rest(control) {
                emit(Event::PadButton {
                    pad,
                    button,
                    pressed: false,
                });
            }
        }
        for (stick, control) in [Stick::Left, Stick::Right]
            .into_iter()
            .zip(&mut controls.sticks)
        {
            if rest(control) {
                let value = Vec2::ZERO;
                emit(Event::PadStick { pad, stick, value });
            }
        }
        let triggers = [Trigger::Left, Trigger::Right].into_iter();
        for (trigger, control) in triggers.zip(&mut controls.triggers) {
            if rest(control) {
                let value = 0.0;
                emit(Event::PadTrigger {
                    pad,
                    trigger,
                    value,
                });
            }
        }
    }

    /// Losing focus lets go of everything, and nothing counts until `SETTLE` after it's gained
    /// again, at `now`; meanwhile what the pads did out of focus, which gilrs may still hold, and
    /// what they do just after, read and count for nothing.
    fn focus(&mut self, focused: bool, now: Instant, emit: &mut impl FnMut(Event<'static>)) {
        if focused {
            self.counts_from.get_or_insert(now + SETTLE);
            return;
        }
        self.counts_from = None;
        self.counting = false;
        for pad in 0..self.pads.len() {
            self.let_go(Pad(pad), emit);
        }
    }

    /// For the main window restored from being minimized, at `now`, after frames that didn't poll
    /// the pads: what they did meanwhile counts for nothing, as though out of focus.
    fn restored(&mut self, now: Instant, emit: &mut impl FnMut(Event<'static>)) {
        if self.counts_from.is_some() {
            self.focus(false, now, emit);
            self.focus(true, now, emit);
        }
    }

    /// `pad`'s controls, which it has from its first reading.
    fn controls(&mut self, pad: Pad) -> &mut Controls {
        if self.pads.len() <= pad.0 {
            self.pads.resize_with(pad.0 + 1, Controls::default);
        }
        &mut self.pads[pad.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Pad = Pad(0);
    const B: Pad = Pad(1);
    const NONE: [Event<'static>; 0] = [];

    fn near(read: Vec2, value: Vec2) -> bool {
        (read - value).length() < 1e-6
    }

    /// Pads in focus since `t`.
    fn pads(t: Instant) -> Pads {
        Pads {
            counts_from: Some(t),
            counting: true,
            pads: Vec::new(),
        }
    }

    /// The events `readings` give, read in one poll that ends at `at`.
    fn poll(pads: &mut Pads, at: Instant, readings: &[Reading]) -> Vec<Event<'static>> {
        let mut events = Vec::new();
        let mut emit = |event| events.push(event);
        for &reading in readings {
            pads.read(reading, &mut emit);
        }
        pads.moved(at, &mut emit);
        events
    }

    fn focus(pads: &mut Pads, focused: bool, at: Instant) -> Vec<Event<'static>> {
        let mut events = Vec::new();
        pads.focus(focused, at, &mut |event| events.push(event));
        events
    }

    fn stick(pad: Pad, stick: Stick, value: Vec2) -> Event<'static> {
        Event::PadStick { pad, stick, value }
    }

    fn button(pad: Pad, button: PadButton, pressed: bool) -> Event<'static> {
        Event::PadButton {
            pad,
            button,
            pressed,
        }
    }

    #[test]
    fn the_dead_zone_is_round_and_scaled_from_its_edge_to_all_the_way() {
        // Round: (0.07, 0.07) is less than 0.1 from rest.
        assert_eq!(dead_zone(Vec2::new(0.07, 0.07)), Vec2::ZERO);
        assert_eq!(dead_zone(Vec2::new(0.0, -DEAD_ZONE)), Vec2::ZERO);
        // A quarter and half of the way from its edge to all the way, along an axis or across.
        assert!(near(dead_zone(Vec2::new(0.0, 0.3)), Vec2::new(0.0, 0.25)));
        assert!(near(dead_zone(Vec2::new(0.3, -0.4)), Vec2::new(0.3, -0.4)));
        for full in [Vec2::new(0.0, FULL), Vec2::new(-0.95, 0.0)] {
            let read = dead_zone(full);
            assert!(near(read, full.normalize()), "{read}");
        }
        // A square gate's corner goes as far as straight.
        let corner = dead_zone(Vec2::ONE);
        assert!(near(corner, Vec2::ONE.normalize()), "{corner}");
        // A button's press stays a press.
        assert_eq!(dead_zone(Vec2::X), Vec2::X);
    }

    #[test]
    fn a_poll_gives_a_stick_once_at_its_last_reading_and_its_moves_before_a_press() {
        let t = Instant::now();
        let mut pads = pads(t);
        let read = [
            Reading::Stick(A, Stick::Left, 0, 0.3),
            Reading::Stick(A, Stick::Left, 0, 0.5),
            Reading::Stick(A, Stick::Left, 1, 0.5),
            Reading::Trigger(A, Trigger::Right, 0.5),
            Reading::Button(A, PadButton::South, true),
            Reading::Stick(A, Stick::Right, 1, -1.0),
            Reading::Stick(B, Stick::Left, 1, 0.05),
        ];
        let diagonal = dead_zone(Vec2::new(0.5, 0.5));
        let trigger = Event::PadTrigger {
            pad: A,
            trigger: Trigger::Right,
            value: dead_zone(Vec2::new(0.5, 0.0)).x,
        };
        assert_eq!(
            poll(&mut pads, t, &read),
            [
                stick(A, Stick::Left, diagonal),
                trigger,
                button(A, PadButton::South, true),
                stick(A, Stick::Right, Vec2::NEG_Y),
            ]
        );
        // What hasn't moved, or moves only within the dead zone, gives nothing.
        let still = [
            Reading::Button(A, PadButton::South, true),
            Reading::Stick(A, Stick::Right, 1, -1.0),
            Reading::Stick(B, Stick::Left, 1, -0.05),
        ];
        assert_eq!(poll(&mut pads, t, &still), NONE);
    }

    #[test]
    fn out_of_focus_nothing_counts_and_what_is_held_as_focus_returns_counts_once_it_has_rested() {
        let t = Instant::now();
        let mut pads = pads(t);
        let held = [
            Reading::Stick(A, Stick::Left, 0, 1.0),
            Reading::Button(A, PadButton::South, true),
        ];
        poll(&mut pads, t, &held);
        // Losing focus lets go, before the layers hear of it.
        assert_eq!(
            focus(&mut pads, false, t),
            [
                button(A, PadButton::South, false),
                stick(A, Stick::Left, Vec2::ZERO),
            ]
        );
        let away = [
            Reading::Button(A, PadButton::East, true),
            Reading::Button(A, PadButton::South, false),
            Reading::Button(A, PadButton::South, true),
            Reading::Stick(A, Stick::Left, 0, 0.5),
        ];
        assert_eq!(poll(&mut pads, t, &away), NONE);
        // Just after focus comes back nothing counts either, as gilrs reads the pads again: a frame
        // polls before it has, then West, pressed while away, is read.
        let back = t + Duration::from_secs(1);
        assert_eq!(focus(&mut pads, true, back), NONE);
        assert_eq!(poll(&mut pads, back, &[]), NONE);
        let late = [Reading::Button(A, PadButton::West, true)];
        assert_eq!(poll(&mut pads, back + SETTLE / 2, &late), NONE);
        // Then what's away from rest stays muted while it's held, and as it moves.
        let moving = [
            Reading::Stick(A, Stick::Left, 0, 0.8),
            Reading::Button(A, PadButton::East, false),
        ];
        assert_eq!(poll(&mut pads, back + SETTLE, &moving), NONE);
        // Back at rest, and away again: seen.
        let again = [
            Reading::Stick(A, Stick::Left, 0, 0.0),
            Reading::Button(A, PadButton::South, false),
            Reading::Button(A, PadButton::South, true),
            Reading::Button(A, PadButton::West, false),
            Reading::Button(A, PadButton::West, true),
            Reading::Stick(A, Stick::Left, 0, -1.0),
        ];
        assert_eq!(
            poll(&mut pads, back + SETTLE, &again),
            [
                button(A, PadButton::South, true),
                button(A, PadButton::West, true),
                stick(A, Stick::Left, Vec2::NEG_X),
            ]
        );
    }

    #[test]
    fn restored_from_minimized_what_the_pads_did_meanwhile_counts_for_nothing() {
        let t = Instant::now();
        let mut pads = pads(t);
        poll(&mut pads, t, &[Reading::Button(A, PadButton::South, true)]);
        let mut let_go = Vec::new();
        pads.restored(t, &mut |event| let_go.push(event));
        assert_eq!(let_go, [button(A, PadButton::South, false)]);
        // What the frames that didn't poll would have read, and what's held as it's read.
        let meanwhile = [
            Reading::Button(A, PadButton::South, false),
            Reading::Button(A, PadButton::North, true),
        ];
        assert_eq!(poll(&mut pads, t, &meanwhile), NONE);
        assert_eq!(poll(&mut pads, t + SETTLE, &[]), NONE);
        let again = [
            Reading::Button(A, PadButton::North, false),
            Reading::Button(A, PadButton::North, true),
        ];
        assert_eq!(
            poll(&mut pads, t + SETTLE, &again),
            [button(A, PadButton::North, true)]
        );
    }

    #[test]
    fn a_pad_that_disconnects_or_connects_again_lets_go_of_what_it_held_alone() {
        let t = Instant::now();
        let mut pads = pads(t);
        let held = [
            Reading::Button(A, PadButton::DPadUp, true),
            Reading::Trigger(A, Trigger::Left, 1.0),
            Reading::Button(B, PadButton::South, true),
        ];
        poll(&mut pads, t, &held);
        let let_go = [
            button(A, PadButton::DPadUp, false),
            Event::PadTrigger {
                pad: A,
                trigger: Trigger::Left,
                value: 0.0,
            },
        ];
        assert_eq!(poll(&mut pads, t, &[Reading::Disconnected(A)]), let_go);
        // Back, and held again, and connected again over it, as gilrs restarts a pad it had.
        poll(&mut pads, t, &[Reading::Connected(A)]);
        poll(&mut pads, t, &held[..2]);
        assert_eq!(poll(&mut pads, t, &[Reading::Connected(A)]), let_go);
        // B holds South still.
        let released = [Reading::Button(B, PadButton::South, false)];
        assert_eq!(
            poll(&mut pads, t, &released),
            [button(B, PadButton::South, false)]
        );
    }

    #[test]
    fn gilrs_names_map_to_the_engines_by_position() {
        let names = [
            (Button::LeftTrigger, Some(PadButton::LeftShoulder)),
            (Button::RightTrigger, Some(PadButton::RightShoulder)),
            (Button::LeftTrigger2, Some(PadButton::LeftTrigger)),
            (Button::RightTrigger2, Some(PadButton::RightTrigger)),
            (Button::South, Some(PadButton::South)),
            (Button::DPadLeft, Some(PadButton::DPadLeft)),
            (Button::C, None),
            (Button::Unknown, None),
        ];
        for (button, name) in names {
            assert_eq!(pad_button(button), name, "{button:?}");
        }
        assert_eq!(stick_axis(Axis::RightStickY), Some((Stick::Right, 1)));
        assert_eq!(stick_axis(Axis::LeftZ), None);
        for (i, button) in PadButton::ALL.into_iter().enumerate() {
            assert_eq!(button as usize, i, "{button:?}");
        }
    }
}
