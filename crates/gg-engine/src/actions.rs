//! Input actions: what a game does, named by the game and bound to keys, mouse buttons and
//! gamepads. A layer feeds them its events and queues their changes as commands.

use std::fmt::Debug;

use gg_core::math::Vec2;
use gg_core::{Error, Result};

use crate::{Event, Key, MouseButton, Pad, PadButton, Stick, Trigger};

/// What a binding reads: a key (by position), a mouse button, or any gamepad's button, stick
/// direction (0 to 1) or trigger pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Key(Key),
    Button(MouseButton),
    PadButton(PadButton),
    PadStick(Stick, Toward),
    PadTrigger(Trigger),
}

impl Source {
    /// Whether it can be part of the way.
    fn analog(self) -> bool {
        matches!(self, Self::PadStick(..) | Self::PadTrigger(_))
    }
}

/// A stick direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toward {
    Left,
    Right,
    Down,
    Up,
}

/// How an action reads its sources: as a button, an axis or a pair of axes. An action has one
/// kind, and all its bindings are of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Held while its source is. A stick or trigger's travel can't be one: it would press and
    /// release as it wavered about a point. Use [`PadButton::LeftTrigger`] or
    /// [`PadButton::RightTrigger`] for a trigger's press.
    Button(Source),
    /// 1 while `positive` is held, -1 while `negative` is, 0 while both or neither; or as far as a
    /// stick or trigger goes.
    Axis { negative: Source, positive: Source },
    /// An axis across and one up, each as negative then positive source: the arrows are
    /// `x: [ArrowLeft, ArrowRight], y: [ArrowDown, ArrowUp]`.
    Axis2 { x: [Source; 2], y: [Source; 2] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Button,
    Axis,
    Axis2,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Button => "a button",
            Self::Axis => "an axis",
            Self::Axis2 => "a pair of axes",
        }
    }
}

impl Binding {
    /// W, A, S and D (by position) as a pair of axes: A and D across, S and W up.
    pub const WASD: Self = Self::Axis2 {
        x: [Source::Key(Key::KeyA), Source::Key(Key::KeyD)],
        y: [Source::Key(Key::KeyS), Source::Key(Key::KeyW)],
    };

    /// A stick as a pair of axes.
    pub const fn stick(stick: Stick) -> Self {
        Self::Axis2 {
            x: [
                Source::PadStick(stick, Toward::Left),
                Source::PadStick(stick, Toward::Right),
            ],
            y: [
                Source::PadStick(stick, Toward::Down),
                Source::PadStick(stick, Toward::Up),
            ],
        }
    }

    fn kind(self) -> Kind {
        match self {
            Self::Button(_) => Kind::Button,
            Self::Axis { .. } => Kind::Axis,
            Self::Axis2 { .. } => Kind::Axis2,
        }
    }

    fn sources(self) -> impl Iterator<Item = Source> {
        let (sources, count) = match self {
            Self::Button(source) => ([source; 4], 1),
            Self::Axis { negative, positive } => ([negative, positive, negative, positive], 2),
            Self::Axis2 { x, y } => ([x[0], x[1], y[0], y[1]], 4),
        };
        sources.into_iter().take(count)
    }

    /// What the `held` sources pressed since press `since` read, summed over pads: a button's 1
    /// or 0, or an axis's value, in `x`.
    fn value(self, held: &[Held], since: u32) -> Vec2 {
        let down = |source| {
            let pressed = held.iter().filter(|h| h.source == source && h.at >= since);
            pressed.map(|h| h.level).sum::<f32>()
        };
        let axis = |[negative, positive]: [Source; 2]| down(positive) - down(negative);
        match self {
            Self::Button(source) => Vec2::new(down(source), 0.0),
            Self::Axis { negative, positive } => Vec2::new(axis([negative, positive]), 0.0),
            Self::Axis2 { x, y } => Vec2::new(axis(x), axis(y)),
        }
    }
}

/// A change of an action's value, from [`Actions::event`], for the layer to queue as a command its
/// next tick applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change<A> {
    /// A button went down.
    Pressed(A),
    /// A button went up.
    Released(A),
    /// An axis's new value, from -1 to 1.
    Axis(A, f32),
    /// A pair of axes' new value, of length at most 1.
    Axis2(A, Vec2),
}

/// A source away from rest.
#[derive(Debug, Clone, Copy)]
struct Held {
    source: Source,
    /// The gamepad it's on; `None` for keyboard and mouse.
    pad: Option<Pad>,
    /// How far it goes, above 0: 1 for a key or button.
    level: f32,
    /// How many bindings existed as it left rest (its press).
    at: u32,
}

/// A layer's actions, of its own type (such as an enum of what its game does), and what does them.
///
/// The layer binds them at start, feeds [`event`](Self::event) each event its
/// [`Layer::event`](crate::Layer::event) gets, and queues the changes as commands, since a tick
/// goes by its commands alone. Values are for what's presented, such as the camera. Unlike
/// [`Input`](crate::Input), actions don't see a press a layer above handled.
///
/// Bindings add up, and so do pads: a button is held while any source is; axes add, then stop at
/// 1, so a key pair cancels and a pair of axes goes as far diagonally as straight. A source is
/// held from its press (or a stick or trigger leaving rest) to its release. So a release without
/// its press (it went to the UI) changes nothing, nor do a held key's repeats: a key already down
/// as focus arrives counts from its next press. Losing focus lets go of everything, since not
/// every platform reports releases while unfocused.
#[derive(Debug)]
pub struct Actions<A> {
    /// Each action, its kind, and its value as of the last change given.
    actions: Vec<(A, Kind, Vec2)>,
    /// Each binding and how many were made before it: it counts presses from there.
    bindings: Vec<(A, Binding, u32)>,
    /// The bound sources held: a list, as few are. Room for every source on one device, so
    /// feeding an event allocates only when a second gamepad first holds more.
    held: Vec<Held>,
    /// How many bindings have been made.
    made: u32,
}

impl<A> Default for Actions<A> {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            bindings: Vec::new(),
            held: Vec::new(),
            made: 0,
        }
    }
}

impl<A: Copy + Eq + Debug> Actions<A> {
    /// Binds `action` to `binding`, beside its others. A source already held for another action
    /// counts for this one from its next press.
    ///
    /// Fails, binding nothing, if:
    /// - the action is bound as another kind,
    /// - a button reads a stick or trigger's travel,
    /// - an axis's negative source is its positive (they'd only cancel).
    pub fn bind(&mut self, action: A, binding: Binding) -> Result<()> {
        let kind = binding.kind();
        let bound = self.actions.iter().find(|(a, ..)| *a == action);
        if let Some(&(_, bound, _)) = bound
            && bound != kind
        {
            return Err(Error::new(
                "engine.bad_binding",
                format!(
                    "action {action:?} is {}, so it can't be bound as {}: {binding:?}",
                    bound.name(),
                    kind.name()
                ),
            ));
        }
        if let Binding::Button(source) = binding
            && source.analog()
        {
            return Err(Error::new(
                "engine.bad_binding",
                format!(
                    "action {action:?} is a button, so it can't read {source:?}, which goes part \
                     of the way; a trigger's press is PadButton::LeftTrigger or RightTrigger"
                ),
            ));
        }
        let pairs = match binding {
            Binding::Button(_) => [None, None],
            Binding::Axis { negative, positive } => [Some([negative, positive]), None],
            Binding::Axis2 { x, y } => [Some(x), Some(y)],
        };
        if let Some([source, _]) = pairs.into_iter().flatten().find(|[a, b]| a == b) {
            return Err(Error::new(
                "engine.bad_binding",
                format!(
                    "action {action:?} can't read {source:?} as both the negative and the positive \
                     of {}",
                    kind.name()
                ),
            ));
        }
        if bound.is_none() {
            self.actions.push((action, kind, Vec2::ZERO));
        }
        self.made += 1;
        self.bindings.push((action, binding, self.made));
        let sources = self.sources();
        let distinct = sources
            .enumerate()
            .filter(|&(i, source)| self.sources().position(|s| s == source) == Some(i))
            .count();
        self.held.reserve(distinct.saturating_sub(self.held.len()));
        Ok(())
    }

    /// Removes `action`'s bindings and value, so it can be bound again. No change is given: a held
    /// action reads as at rest.
    pub fn unbind(&mut self, action: A) {
        self.actions.retain(|(a, ..)| *a != action);
        self.bindings.retain(|(a, ..)| *a != action);
        let mut held = std::mem::take(&mut self.held);
        held.retain(|held| self.binds(held.source));
        self.held = held;
    }

    /// Feeds an event, giving `changed` each action value it changes, in the order the actions were
    /// bound (one rebound after [`unbind`](Self::unbind) comes last).
    pub fn event(&mut self, event: &Event<'_>, changed: impl FnMut(Change<A>)) {
        let level = |pressed| f32::from(u8::from(pressed));
        let moved = match *event {
            Event::Key {
                key,
                pressed,
                repeat: false,
            } => self.hold(Source::Key(key), None, level(pressed)),
            Event::MouseButton { button, pressed } => {
                self.hold(Source::Button(button), None, level(pressed))
            }
            Event::PadButton {
                pad,
                button,
                pressed,
            } => self.hold(Source::PadButton(button), Some(pad), level(pressed)),
            Event::PadStick { pad, stick, value } => [
                (Toward::Left, -value.x),
                (Toward::Right, value.x),
                (Toward::Down, -value.y),
                (Toward::Up, value.y),
            ]
            .into_iter()
            .fold(false, |moved, (toward, way)| {
                let way = if way > 0.0 { way } else { 0.0 };
                self.hold(Source::PadStick(stick, toward), Some(pad), way) | moved
            }),
            Event::PadTrigger {
                pad,
                trigger,
                value,
            } => self.hold(Source::PadTrigger(trigger), Some(pad), value),
            Event::Focused(false) => {
                self.held.clear();
                true
            }
            _ => false,
        };
        if moved {
            self.changes(changed);
        }
    }

    /// Whether `action` is away from rest: a button held, an axis not 0.
    pub fn held(&self, action: A) -> bool {
        self.value(action) != Vec2::ZERO
    }

    /// An axis's value, -1 to 1; a button's 1 or 0; a pair of axes' across.
    pub fn axis(&self, action: A) -> f32 {
        self.value(action).x
    }

    /// A pair of axes' value, of length at most 1; another kind's as its `x`.
    pub fn axis2(&self, action: A) -> Vec2 {
        self.value(action)
    }

    /// `action`'s value; at rest if unbound.
    fn value(&self, action: A) -> Vec2 {
        let found = self.actions.iter().find(|(a, ..)| *a == action);
        found.map_or(Vec2::ZERO, |&(_, _, value)| value)
    }

    /// Every binding's sources, once per binding.
    fn sources(&self) -> impl Iterator<Item = Source> + '_ {
        let bindings = self.bindings.iter();
        bindings.flat_map(|&(_, binding, _)| binding.sources())
    }

    fn binds(&self, source: Source) -> bool {
        self.sources().any(|bound| bound == source)
    }

    /// Holds `source` on `pad` at `level`, or lets go at 0. Returns whether that changed it.
    /// Leaving rest is its press: bindings made since count it from the next one.
    fn hold(&mut self, source: Source, pad: Option<Pad>, level: f32) -> bool {
        if !self.binds(source) {
            return false;
        }
        let at = self
            .held
            .iter()
            .position(|held| held.source == source && held.pad == pad);
        match (level > 0.0, at) {
            (true, None) => self.held.push(Held {
                source,
                pad,
                level,
                at: self.made,
            }),
            (true, Some(at)) if self.held[at].level != level => self.held[at].level = level,
            (false, Some(at)) => {
                self.held.swap_remove(at);
            }
            _ => return false,
        }
        true
    }

    /// Recomputes each action's value from the held sources, giving `changed` each that changed.
    fn changes(&mut self, mut changed: impl FnMut(Change<A>)) {
        let Self {
            actions,
            bindings,
            held,
            ..
        } = self;
        for (action, kind, value) in actions {
            let bound = bindings.iter().filter(|(a, ..)| *a == *action);
            let sum: Vec2 = bound
                .map(|&(_, binding, since)| binding.value(held, since))
                .sum();
            let now = sum.clamp(Vec2::NEG_ONE, Vec2::ONE).clamp_length_max(1.0);
            if now == *value {
                continue;
            }
            *value = now;
            changed(match kind {
                Kind::Button if now.x > 0.0 => Change::Pressed(*action),
                Kind::Button => Change::Released(*action),
                Kind::Axis => Change::Axis(*action, now.x),
                Kind::Axis2 => Change::Axis2(*action, now),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Act {
        Jump,
        Walk,
        Move,
    }

    fn key(key: Key, pressed: bool) -> Event<'static> {
        Event::Key {
            key,
            pressed,
            repeat: false,
        }
    }

    fn repeat(key: Key) -> Event<'static> {
        Event::Key {
            key,
            pressed: true,
            repeat: true,
        }
    }

    const NONE: [Change<Act>; 0] = [];

    fn pair(negative: Key, positive: Key) -> [Source; 2] {
        [Source::Key(negative), Source::Key(positive)]
    }

    fn feed(actions: &mut Actions<Act>, events: &[Event<'_>]) -> Vec<Change<Act>> {
        let mut changes = Vec::new();
        for event in events {
            actions.event(event, |change| changes.push(change));
        }
        changes
    }

    #[test]
    fn a_button_presses_once_and_releases_once_whichever_of_its_sources_hold_it() {
        let mut actions = Actions::default();
        let jump = [Source::Key(Key::Space), Source::Button(MouseButton::Left)];
        for source in jump {
            actions
                .bind(Act::Jump, Binding::Button(source))
                .expect("bound");
        }
        // A tap between frames gives both.
        let tap = [key(Key::Space, true), key(Key::Space, false)];
        let tapped = [Change::Pressed(Act::Jump), Change::Released(Act::Jump)];
        assert_eq!(feed(&mut actions, &tap), tapped);
        // A release without its press, a repeat of a key not held and an unbound key give none.
        let none = [
            key(Key::Space, false),
            repeat(Key::Space),
            key(Key::KeyW, true),
        ];
        assert_eq!(feed(&mut actions, &none), NONE);
        assert!(!actions.held(Act::Jump));
        let left = |pressed| Event::MouseButton {
            button: MouseButton::Left,
            pressed,
        };
        let held = [key(Key::Space, true), repeat(Key::Space), left(true)];
        assert_eq!(feed(&mut actions, &held), [Change::Pressed(Act::Jump)]);
        assert_eq!(feed(&mut actions, &[key(Key::Space, false)]), NONE);
        assert!(actions.held(Act::Jump));
        assert_eq!(
            feed(&mut actions, &[left(false)]),
            [Change::Released(Act::Jump)]
        );
    }

    #[test]
    fn axes_add_up_and_stop_at_one() {
        let mut actions = Actions::default();
        let walk = Binding::Axis {
            negative: Source::Key(Key::KeyA),
            positive: Source::Key(Key::KeyD),
        };
        actions.bind(Act::Walk, walk).expect("bound");
        for (x, y) in [
            (
                pair(Key::ArrowLeft, Key::ArrowRight),
                pair(Key::ArrowDown, Key::ArrowUp),
            ),
            (pair(Key::KeyA, Key::KeyD), pair(Key::KeyS, Key::KeyW)),
        ] {
            actions
                .bind(Act::Move, Binding::Axis2 { x, y })
                .expect("bound");
        }
        // One key moves each action bound to it, in binding order.
        let walked = [
            Change::Axis(Act::Walk, 1.0),
            Change::Axis2(Act::Move, Vec2::X),
        ];
        assert_eq!(feed(&mut actions, &[key(Key::KeyD, true)]), walked);
        // The other key of the pair cancels it.
        let still = [
            Change::Axis(Act::Walk, 0.0),
            Change::Axis2(Act::Move, Vec2::ZERO),
        ];
        assert_eq!(feed(&mut actions, &[key(Key::KeyA, true)]), still);
        feed(&mut actions, &[key(Key::KeyA, false)]);
        // A second binding the same way adds nothing past 1.
        assert_eq!(feed(&mut actions, &[key(Key::ArrowRight, true)]), NONE);
        // Diagonal reaches as far as straight.
        let changes = feed(&mut actions, &[key(Key::ArrowUp, true)]);
        let [Change::Axis2(Act::Move, along)] = changes[..] else {
            panic!("{changes:?}");
        };
        assert!(
            along.x == along.y && (along.length() - 1.0).abs() < 1e-6,
            "{along}"
        );
        assert_eq!(
            (actions.axis(Act::Walk), actions.axis2(Act::Move)),
            (1.0, along)
        );
    }

    #[test]
    fn losing_focus_lets_go_of_everything_and_a_key_held_through_it_counts_from_its_next_press() {
        let mut actions = Actions::default();
        let walk = Binding::Axis {
            negative: Source::Key(Key::KeyA),
            positive: Source::Key(Key::KeyD),
        };
        actions
            .bind(Act::Jump, Binding::Button(Source::Key(Key::Space)))
            .expect("bound");
        actions.bind(Act::Walk, walk).expect("bound");
        feed(&mut actions, &[key(Key::Space, true), key(Key::KeyD, true)]);
        let let_go = [Change::Released(Act::Jump), Change::Axis(Act::Walk, 0.0)];
        assert_eq!(feed(&mut actions, &[Event::Focused(false)]), let_go);
        // Space, still down as focus returns, repeats unheld.
        let back = [
            Event::Focused(true),
            repeat(Key::Space),
            key(Key::Space, false),
        ];
        assert_eq!(feed(&mut actions, &back), NONE);
        let pressed = feed(&mut actions, &[key(Key::Space, true)]);
        assert_eq!(pressed, [Change::Pressed(Act::Jump)]);
    }

    #[test]
    fn a_source_held_as_it_is_bound_counts_from_its_next_press() {
        let mut actions = Actions::default();
        let walk = Binding::Axis {
            negative: Source::Key(Key::KeyA),
            positive: Source::Key(Key::KeyD),
        };
        actions
            .bind(Act::Jump, Binding::Button(Source::Key(Key::Space)))
            .expect("bound");
        actions.bind(Act::Walk, walk).expect("bound");
        feed(&mut actions, &[key(Key::Space, true)]);
        // Move, bound to Space while held for Jump, isn't held until its next press.
        let button = Binding::Button(Source::Key(Key::Space));
        actions.bind(Act::Move, button).expect("bound");
        let walked = feed(&mut actions, &[key(Key::KeyD, true)]);
        assert_eq!(walked, [Change::Axis(Act::Walk, 1.0)]);
        let let_go = feed(&mut actions, &[key(Key::Space, false)]);
        assert_eq!(let_go, [Change::Released(Act::Jump)]);
        let both = [Change::Pressed(Act::Jump), Change::Pressed(Act::Move)];
        assert_eq!(feed(&mut actions, &[key(Key::Space, true)]), both);
    }

    fn stick(pad: usize, value: Vec2) -> Event<'static> {
        Event::PadStick {
            pad: Pad(pad),
            stick: Stick::Left,
            value,
        }
    }

    fn trigger(trigger: Trigger, value: f32) -> Event<'static> {
        Event::PadTrigger {
            pad: Pad(0),
            trigger,
            value,
        }
    }

    fn south(pad: usize, pressed: bool) -> Event<'static> {
        Event::PadButton {
            pad: Pad(pad),
            button: PadButton::South,
            pressed,
        }
    }

    #[test]
    fn a_stick_and_a_trigger_go_as_far_as_they_go_adding_up_with_keys_and_every_pad() {
        let mut actions = Actions::default();
        let arrows = Binding::Axis2 {
            x: pair(Key::ArrowLeft, Key::ArrowRight),
            y: pair(Key::ArrowDown, Key::ArrowUp),
        };
        let triggers = Binding::Axis {
            negative: Source::PadTrigger(Trigger::Left),
            positive: Source::PadTrigger(Trigger::Right),
        };
        for (action, binding) in [
            (Act::Move, arrows),
            (Act::Move, Binding::stick(Stick::Left)),
            (Act::Walk, triggers),
        ] {
            actions.bind(action, binding).expect("bound");
        }
        let moved = |at| [Change::Axis2(Act::Move, at)];
        let half = Vec2::new(0.5, 0.0);
        assert_eq!(feed(&mut actions, &[stick(0, half)]), moved(half));
        // An arrow the same way reaches 1; another pad's stick adds too.
        assert_eq!(
            feed(&mut actions, &[key(Key::ArrowRight, true)]),
            moved(Vec2::X)
        );
        let back = [key(Key::ArrowRight, false), stick(1, Vec2::new(0.25, 0.0))];
        let changes = feed(&mut actions, &back);
        assert_eq!(changes, [moved(half)[0], moved(Vec2::new(0.75, 0.0))[0]]);
        let along = Vec2::new(-0.25, 0.5);
        let rest = [stick(1, Vec2::ZERO), stick(0, along)];
        assert_eq!(feed(&mut actions, &rest), [moved(half)[0], moved(along)[0]]);
        assert_eq!(
            feed(&mut actions, &[stick(0, Vec2::ZERO)]),
            moved(Vec2::ZERO)
        );
        let walked = [
            trigger(Trigger::Right, 0.5),
            trigger(Trigger::Left, 0.5),
            trigger(Trigger::Right, 0.0),
        ];
        let walked_so = [
            Change::Axis(Act::Walk, 0.5),
            Change::Axis(Act::Walk, 0.0),
            Change::Axis(Act::Walk, -0.5),
        ];
        assert_eq!(feed(&mut actions, &walked), walked_so);
    }

    #[test]
    fn a_button_held_on_two_pads_is_held_until_both_let_go() {
        let mut actions = Actions::default();
        let jump = Binding::Button(Source::PadButton(PadButton::South));
        actions.bind(Act::Jump, jump).expect("bound");
        assert_eq!(
            feed(&mut actions, &[south(0, true)]),
            [Change::Pressed(Act::Jump)]
        );
        assert_eq!(feed(&mut actions, &[south(1, true), south(0, false)]), NONE);
        // A release from a pad that already let go, or never pressed, changes nothing.
        assert_eq!(
            feed(&mut actions, &[south(0, false), south(2, false)]),
            NONE
        );
        assert_eq!(
            feed(&mut actions, &[south(1, false)]),
            [Change::Released(Act::Jump)]
        );
    }

    #[test]
    fn a_stick_away_from_rest_as_it_is_bound_counts_once_it_has_rested() {
        let mut actions = Actions::default();
        let right = Binding::Axis {
            negative: Source::PadStick(Stick::Left, Toward::Left),
            positive: Source::PadStick(Stick::Left, Toward::Right),
        };
        actions.bind(Act::Walk, right).expect("bound");
        let half = Vec2::new(0.5, 0.0);
        feed(&mut actions, &[stick(0, half)]);
        actions
            .bind(Act::Move, Binding::stick(Stick::Left))
            .expect("bound");
        // Move doesn't see the stick until it has been back at rest.
        let further = feed(&mut actions, &[stick(0, Vec2::new(0.75, 0.0))]);
        assert_eq!(further, [Change::Axis(Act::Walk, 0.75)]);
        let changes = feed(&mut actions, &[stick(0, Vec2::ZERO), stick(0, half)]);
        let both = [
            Change::Axis(Act::Walk, 0.0),
            Change::Axis(Act::Walk, 0.5),
            Change::Axis2(Act::Move, half),
        ];
        assert_eq!(changes, both);
    }

    #[test]
    fn a_binding_of_another_kind_or_of_an_axis_to_one_source_is_refused_binding_nothing() {
        let mut actions = Actions::default();
        let [space, a, d, w] = [Key::Space, Key::KeyA, Key::KeyD, Key::KeyW].map(Source::Key);
        actions
            .bind(Act::Jump, Binding::Button(space))
            .expect("bound");
        for (action, binding) in [
            (
                Act::Jump,
                Binding::Axis {
                    negative: a,
                    positive: d,
                },
            ),
            (
                Act::Walk,
                Binding::Axis {
                    negative: space,
                    positive: space,
                },
            ),
            (
                Act::Move,
                Binding::Axis2 {
                    x: [a, d],
                    y: [w, w],
                },
            ),
            // Sticks and triggers go part of the way.
            (
                Act::Walk,
                Binding::Button(Source::PadStick(Stick::Left, Toward::Up)),
            ),
            (
                Act::Move,
                Binding::Button(Source::PadTrigger(Trigger::Left)),
            ),
        ] {
            let refused = actions.bind(action, binding).expect_err("refused");
            assert_eq!(refused.code(), "engine.bad_binding", "{refused}");
        }
        // Jump is still Space's button; the others may be any kind.
        let held = [
            key(Key::KeyA, true),
            key(Key::KeyD, true),
            key(Key::Space, true),
        ];
        assert_eq!(feed(&mut actions, &held), [Change::Pressed(Act::Jump)]);
        for action in [Act::Walk, Act::Move] {
            actions.bind(action, Binding::Button(w)).expect("bound");
        }
    }
}
