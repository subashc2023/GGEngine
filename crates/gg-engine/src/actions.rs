//! Input actions: what a game does, named by the game, bound to the keys and mouse buttons that do
//! it, which a layer feeds the events it's sent and queues the changes of as its commands.

use std::fmt::Debug;

use gg_core::math::Vec2;
use gg_core::{Error, Result};

use crate::{Event, Key, MouseButton};

/// What a binding reads: a key, by its position, or a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Key(Key),
    Button(MouseButton),
}

/// How an action reads its sources, which makes it a button, an axis or a pair of axes: an action
/// is of one kind, which all of its bindings are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Held while its source is.
    Button(Source),
    /// 1 while `positive` is held, -1 while `negative` is, and 0 while both or neither are.
    Axis { negative: Source, positive: Source },
    /// An axis across and one up, each its negative source then its positive: the arrows are
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

    /// What it reads while the sources in `held` are, of those pressed since press `since`: a
    /// button's 1 or 0, or an axis's value, in `x`.
    fn value(self, held: &[(Source, u32)], since: u32) -> Vec2 {
        let pressed = |source| held.iter().any(|&(s, at)| s == source && at >= since);
        let down = |source| f32::from(u8::from(pressed(source)));
        let axis = |[negative, positive]: [Source; 2]| down(positive) - down(negative);
        match self {
            Self::Button(source) => Vec2::new(down(source), 0.0),
            Self::Axis { negative, positive } => Vec2::new(axis([negative, positive]), 0.0),
            Self::Axis2 { x, y } => Vec2::new(axis(x), axis(y)),
        }
    }
}

/// A change of an action's value, which [`Actions::event`] gives: for the layer to queue as a
/// command of its own, which its next tick applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change<A> {
    /// A button went down.
    Pressed(A),
    /// A button went up.
    Released(A),
    /// An axis's new value, from -1 to 1.
    Axis(A, f32),
    /// A pair of axes' new value, no longer than 1.
    Axis2(A, Vec2),
}

/// A layer's actions, of a type of its own, such as an enum of what its game does, and what does
/// them. The layer binds them as it starts, feeds [`event`](Self::event) each event its
/// [`Layer::event`](crate::Layer::event) is sent, and queues the changes it gives as its commands,
/// since a tick goes by its commands alone; their values are for what's presented, such as the
/// camera. Fed by the layer, they follow the events the layer is sent: unlike
/// [`Input`](crate::Input), they don't see a press that a layer above it handled.
///
/// An action's bindings add up: a button is held while any of its sources is, and axes add, then
/// stop at 1, so that a key pair's two keys cancel, and a pair of axes goes as far diagonally as
/// straight. A source is held from a press to its release, so a release without its press, as when
/// the press went to the UI, changes nothing, and nor do a held key's repeats, so that a key
/// already down as the window gains focus counts from its next press. Losing focus lets go of
/// everything, since not every platform reports the releases that come while the window is out of
/// focus.
#[derive(Debug)]
pub struct Actions<A> {
    /// Each action, its kind, and its value as of the last change given.
    actions: Vec<(A, Kind, Vec2)>,
    /// Each binding, and how many were made before it, which it counts presses from.
    bindings: Vec<(A, Binding, u32)>,
    /// The bound sources held, each with how many bindings had been made as it was pressed: a list
    /// rather than a set, since few are held at once, with room for all of them, so that feeding an
    /// event never allocates.
    held: Vec<(Source, u32)>,
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
    /// Binds `action` to `binding`, beside its other bindings. A source held as it's bound, for
    /// another action, counts for this one from its next press.
    ///
    /// Fails, binding nothing, if the action is bound as another kind, or if an axis's negative
    /// source is its positive, which would only cancel.
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

    /// Takes `action`'s bindings away, so that it can be bound again, and its value with them,
    /// without a change: a held one reads as at rest.
    pub fn unbind(&mut self, action: A) {
        self.actions.retain(|(a, ..)| *a != action);
        self.bindings.retain(|(a, ..)| *a != action);
        let mut held = std::mem::take(&mut self.held);
        held.retain(|&(source, _)| self.binds(source));
        self.held = held;
    }

    /// Feeds an event the layer was sent, giving `changed` each change of an action's value that it
    /// makes, in the order the actions were bound, one bound again after [`unbind`](Self::unbind)
    /// coming after the others.
    pub fn event(&mut self, event: &Event<'_>, changed: impl FnMut(Change<A>)) {
        let (source, pressed) = match *event {
            Event::Key {
                key,
                pressed,
                repeat: false,
            } => (Source::Key(key), pressed),
            Event::MouseButton { button, pressed } => (Source::Button(button), pressed),
            Event::Focused(false) => {
                self.held.clear();
                self.changes(changed);
                return;
            }
            _ => return,
        };
        if !self.binds(source) {
            return;
        }
        let at = self.held.iter().position(|&(held, _)| held == source);
        match (pressed, at) {
            (true, None) => self.held.push((source, self.made)),
            (false, Some(at)) => {
                self.held.swap_remove(at);
            }
            _ => return,
        }
        self.changes(changed);
    }

    /// Whether `action` is away from rest: a button held, or an axis anywhere but 0.
    pub fn held(&self, action: A) -> bool {
        self.value(action) != Vec2::ZERO
    }

    /// An axis's value, from -1 to 1; a button's, 1 while it's held and 0 otherwise; or a pair of
    /// axes' across.
    pub fn axis(&self, action: A) -> f32 {
        self.value(action).x
    }

    /// A pair of axes' value, no longer than 1; or another action's as its `x`.
    pub fn axis2(&self, action: A) -> Vec2 {
        self.value(action)
    }

    /// `action`'s value, at rest if it isn't bound.
    fn value(&self, action: A) -> Vec2 {
        let found = self.actions.iter().find(|(a, ..)| *a == action);
        found.map_or(Vec2::ZERO, |&(_, _, value)| value)
    }

    /// The sources of every binding, as often as they're bound.
    fn sources(&self) -> impl Iterator<Item = Source> + '_ {
        let bindings = self.bindings.iter();
        bindings.flat_map(|&(_, binding, _)| binding.sources())
    }

    fn binds(&self, source: Source) -> bool {
        self.sources().any(|bound| bound == source)
    }

    /// Works out each action's value from the sources held, giving `changed` each that changed.
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

    /// The changes `events` make, fed in order.
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
        // A tap between two frames gives both.
        let tap = [key(Key::Space, true), key(Key::Space, false)];
        let tapped = [Change::Pressed(Act::Jump), Change::Released(Act::Jump)];
        assert_eq!(feed(&mut actions, &tap), tapped);
        // A release without its press, a repeat of a key not held, and a key not bound give none.
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
        // One key moves each action it's bound to, in the order they were bound.
        let walked = [
            Change::Axis(Act::Walk, 1.0),
            Change::Axis2(Act::Move, Vec2::X),
        ];
        assert_eq!(feed(&mut actions, &[key(Key::KeyD, true)]), walked);
        // Its pair's other key cancels it.
        let still = [
            Change::Axis(Act::Walk, 0.0),
            Change::Axis2(Act::Move, Vec2::ZERO),
        ];
        assert_eq!(feed(&mut actions, &[key(Key::KeyA, true)]), still);
        feed(&mut actions, &[key(Key::KeyA, false)]);
        // A second binding that goes the same way goes no further.
        assert_eq!(feed(&mut actions, &[key(Key::ArrowRight, true)]), NONE);
        // Diagonally, as far as straight.
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
        // Space, still down as focus comes back, repeats, which doesn't hold it.
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
        // Move, bound to Space as it's held for Jump, isn't held until its next press, whatever
        // else changes meanwhile.
        let button = Binding::Button(Source::Key(Key::Space));
        actions.bind(Act::Move, button).expect("bound");
        let walked = feed(&mut actions, &[key(Key::KeyD, true)]);
        assert_eq!(walked, [Change::Axis(Act::Walk, 1.0)]);
        let let_go = feed(&mut actions, &[key(Key::Space, false)]);
        assert_eq!(let_go, [Change::Released(Act::Jump)]);
        let both = [Change::Pressed(Act::Jump), Change::Pressed(Act::Move)];
        assert_eq!(feed(&mut actions, &[key(Key::Space, true)]), both);
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
        ] {
            let refused = actions.bind(action, binding).expect_err("refused");
            assert_eq!(refused.code(), "engine.bad_binding", "{refused}");
        }
        // Jump is a button still, of Space alone, and the others are free to be any kind.
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
