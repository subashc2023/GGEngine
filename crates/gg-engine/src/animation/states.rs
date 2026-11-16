//! Animation state machines: states (a clip, or clips blended along a parameter) and transitions
//! on parameters' values, which drive a model root's [`Animator`] plays.

use std::fmt;
use std::mem;
use std::sync::Arc;

use gg_core::{Error, Result};

use super::Playback;
use super::animator::{Animator, Play, named};
use super::skeleton::Clip;
use crate::{Component, Entity, EntityId, ModelRoot, Reflect, Tick, World, reflect};

/// A state machine driving a model root's [`Animator`], before the animator steps in
/// [`step_animation`](super::step_animation).
///
/// Parameters (set by code, scripts and tools) and transitions on them move between states, each a
/// clip or clips blended along a parameter. Each tick the transitions from the current state and
/// from any state are checked in order and the first that holds fires, the animator cross-fading
/// as [`World::play`] does. Then the state's plays take its motion's weights and every other play
/// eases to 0 and leaves, so a lone clip plays exactly once its fade is done.
///
/// The state and its seconds are fields here and the plays' times the animator's, so a snapshot,
/// replay or joining peer continues exactly. An entity without a [`ModelRoot`] steps none; one
/// without an animator is given one.
///
/// Entering a state, each play of its clips the animator holds eases to its weight from where it
/// is, keeping its time, unless the state plays once: that starts each clip afresh (from its end if
/// backward) and held plays ease out with the rest. A clip the animator lacks joins from its start
/// at weight 0, or at its weight for a fade of 0.
///
/// A blend's clips keep in step at the same fraction of their lengths: the heaviest held clip's
/// (the first in blend order among equals). Each plays at the speed that brings it round in the
/// blend's length, their lengths weighted as the blend weights them over the clips its model has
/// of some length. A missing clip or one of no length (a still pose) speeds up no other. So a walk
/// and a run blend into a jog whose feet agree.
///
/// Names it lacks are warned of once per world, by name:
/// - a state (`animation.no_state`): named as its own state, it leaves for the first; no
///   transition to it fires;
/// - a parameter (`animation.no_param`): its conditions don't hold and a blend along it blends as
///   at 0;
/// - a test or blend on a parameter of another kind (`animation.wrong_param`): likewise;
/// - a clip its model lacks (`animation.no_clip`): left out.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub struct AnimationStates {
    /// The values transitions test and blends blend along, by name. Code sets them with
    /// [`World::set_param`], scripts with `gg.set_param`, tools through these fields.
    pub params: Vec<AnimationParam>,
    /// The states, by name.
    pub states: Vec<AnimationState>,
    /// The transitions, in the order checked.
    pub transitions: Vec<StateTransition>,
    /// The current state's name. Empty, as a scene starts it, it enters the first at its first
    /// tick; set to another (by a tool, say), it cuts to that state's clips at the next tick, its
    /// time continuing from what it's given.
    pub state: String,
    /// Seconds in the current state, which an exit time measures.
    pub time: f32,
}

/// A parameter of an [`AnimationStates`]: a name and a value.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct AnimationParam {
    /// The name conditions, blends and setters use.
    pub name: String,
    /// Its value; a value set to it keeps its kind.
    pub value: ParamValue,
}

/// The value of an [`AnimationParam`], and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum ParamValue {
    /// A number, such as a speed: a blend blends along it, a condition tests it above or below a
    /// value.
    Float(f32),
    /// A flag that stays as set.
    Bool(bool),
    /// A flag that a transition firing on it consumes, setting it back.
    Trigger(bool),
}

impl From<f32> for ParamValue {
    fn from(x: f32) -> Self {
        Self::Float(x)
    }
}

impl From<bool> for ParamValue {
    /// A bool sets a bool or a trigger alike.
    fn from(set: bool) -> Self {
        Self::Bool(set)
    }
}

impl ParamValue {
    fn kind(self) -> &'static str {
        match self {
            Self::Float(_) => "Float",
            Self::Bool(_) => "Bool",
            Self::Trigger(_) => "Trigger",
        }
    }
}

/// A state of an [`AnimationStates`]: what it plays, how fast, and what happens at the end.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct AnimationState {
    /// The name the machine's `state` and transitions use.
    pub name: String,
    /// The clip it plays, or the clips it blends.
    pub motion: StateMotion,
    /// Playback rate: 1 as its clips go, 2 twice as fast, backward below 0.
    #[reflect(default = 1.0)]
    pub speed: f32,
    /// Its clips play once and hold the last pose, loop, or go back and forth. Played once, each
    /// starts afresh as the state is entered, from its end if `speed` is below 0.
    #[reflect(default = Playback::Loop)]
    pub playback: Playback,
}

/// What an [`AnimationState`] plays.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub enum StateMotion {
    /// One clip, by its name among its model's.
    Clip(String),
    /// Clips blended along the float parameter `param`, each at its point. The two points nearest
    /// the value, either side, weigh by nearness; at a point, or past every point, the nearest
    /// clip plays alone.
    Blend {
        param: String,
        points: Vec<BlendPoint>,
    },
}

/// A clip of a blend, at its point along the blend's parameter.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct BlendPoint {
    /// The clip's name among its model's.
    pub clip: String,
    /// The parameter's value at which it plays alone.
    pub at: f32,
}

/// A transition of an [`AnimationStates`] from a state, or any, to another, once all its
/// conditions hold.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct StateTransition {
    /// The state it leaves, by name. `None`: any but the one it enters.
    pub from: Option<String>,
    /// The state it enters, by name.
    pub to: String,
    /// All must hold for it to fire. With none, it fires as soon as its exit time allows. Firing
    /// consumes each trigger tested as set.
    pub conditions: Vec<ParamCondition>,
    /// How much of its motion the state it leaves must have played first: seconds in the state
    /// times the size of its speed, over the clip's length or the blend's (weighted as
    /// [`AnimationStates`] says). Past 1 is more than a loop's round.
    pub exit_time: Option<f32>,
    /// Seconds the animator takes to cross-fade from the state it leaves, as [`World::play`] does.
    /// At 0 it cuts.
    #[reflect(min = 0.0)]
    pub fade: f32,
}

/// What a [`StateTransition`] tests of a parameter.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct ParamCondition {
    /// The parameter's name.
    pub param: String,
    /// What must hold of its value.
    pub test: ParamTest,
}

/// What must hold of a parameter's value for a [`ParamCondition`] to hold.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum ParamTest {
    /// A float above this.
    Above(f32),
    /// A float below this.
    Below(f32),
    /// A bool that's true, or a trigger that's set (the transition consumes it as it fires).
    True,
    /// A bool that's false, or a trigger that isn't set.
    False,
}

impl ParamTest {
    /// What it tests, for a warning.
    fn tests(self) -> &'static str {
        match self {
            Self::Above(_) | Self::Below(_) => "against a number, as a float's value is",
            Self::True | Self::False => "as a flag, as a bool or a trigger is",
        }
    }
}

impl AnimationStates {
    /// The value of parameter `name`, if any.
    pub fn param(&self, name: &str) -> Option<ParamValue> {
        let mut params = self.params.iter();
        params
            .find(|param| param.name == name)
            .map(|param| param.value)
    }

    /// Sets parameter `name` to `value`, keeping its kind: a float takes a number, a bool or
    /// trigger takes a bool. A trigger set stays set until a transition firing on it consumes it,
    /// or it's set back.
    ///
    /// # Errors
    ///
    /// - `animation.no_param`, naming its parameters: no parameter `name`.
    /// - `animation.wrong_param`: `value` is of another kind than the parameter's.
    pub fn set_param(&mut self, name: &str, value: impl Into<ParamValue>) -> Result<()> {
        let value = value.into();
        let Some(param) = self.params.iter_mut().find(|param| param.name == name) else {
            let names = reflect::list(self.params.iter().map(|param| &param.name));
            return Err(Error::new(
                "animation.no_param",
                format!("the state machine has no parameter {name:?}: its parameters are {names}"),
            ));
        };
        param.value = match (param.value, value) {
            (ParamValue::Float(_), ParamValue::Float(x)) => ParamValue::Float(x),
            (ParamValue::Bool(_), ParamValue::Bool(set) | ParamValue::Trigger(set)) => {
                ParamValue::Bool(set)
            }
            (ParamValue::Trigger(_), ParamValue::Bool(set) | ParamValue::Trigger(set)) => {
                ParamValue::Trigger(set)
            }
            (held, given) => {
                let takes = match held {
                    ParamValue::Float(_) => "a number",
                    ParamValue::Bool(_) | ParamValue::Trigger(_) => "a bool",
                };
                return Err(Error::new(
                    "animation.wrong_param",
                    format!(
                        "the state machine's parameter {name:?} is a {}, set to {takes}, not a {}",
                        held.kind(),
                        given.kind()
                    ),
                ));
            }
        };
        Ok(())
    }

    /// The index of state `name`.
    fn position(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|state| state.name == name)
    }

    /// Steps `dt` seconds over the animator's `plays` of a model with `clips`: fires the first
    /// transition that holds, then sets the plays for the state, as [`AnimationStates`] says.
    /// Missing names go to `warnings`.
    fn step(&mut self, clips: &[Clip], plays: &mut Vec<Play>, dt: f32, warnings: &mut Warnings) {
        if self.states.is_empty() {
            return;
        }
        let (at, entered) = match self.position(&self.state) {
            Some(at) => match self.fired(at, clips, warnings) {
                Some((to, fade)) => (to, Some(fade)),
                None => (at, None),
            },
            None => {
                if !self.state.is_empty() {
                    let first = &self.states[0].name;
                    warnings.once(
                        "animation.no_state",
                        &self.state,
                        format_args!(
                            "is in state {:?}, which it hasn't: it enters its first, {first:?}",
                            self.state
                        ),
                    );
                }
                (0, Some(0.0))
            }
        };
        if entered.is_some() {
            self.state.clone_from(&self.states[at].name);
            self.time = 0.0;
        }
        self.drive(at, clips, plays, entered, warnings);
        self.time += dt;
    }

    /// The first transition that holds in state `at`, its triggers consumed: the state it enters
    /// and its fade's seconds.
    fn fired(
        &mut self,
        at: usize,
        clips: &[Clip],
        warnings: &mut Warnings,
    ) -> Option<(usize, f32)> {
        let name = &self.states[at].name;
        let mut fired = None;
        for (i, transition) in self.transitions.iter().enumerate() {
            match &transition.from {
                Some(from) if from != name => {
                    if self.position(from).is_none() {
                        warnings.once(
                            "animation.no_state",
                            from,
                            format_args!("has a transition from state {from:?}, which it hasn't"),
                        );
                    }
                    continue;
                }
                None if transition.to == *name => continue,
                Some(_) | None => {}
            }
            let to = &transition.to;
            let Some(to) = self.position(to) else {
                warnings.once(
                    "animation.no_state",
                    to,
                    format_args!(
                        "has a transition to state {to:?}, which it hasn't: it never fires"
                    ),
                );
                continue;
            };
            if (transition.exit_time).is_some_and(|exit| self.played(at, clips) < exit) {
                continue;
            }
            let mut conditions = transition.conditions.iter();
            if conditions.all(|condition| self.holds(condition, warnings)) {
                fired = Some((i, to));
                break;
            }
        }
        let (i, to) = fired?;
        let transition = &self.transitions[i];
        for condition in &transition.conditions {
            let param = (self.params.iter_mut()).find(|param| param.name == condition.param);
            if condition.test == ParamTest::True
                && let Some(AnimationParam {
                    value: ParamValue::Trigger(set),
                    ..
                }) = param
            {
                *set = false;
            }
        }
        Some((to, transition.fade))
    }

    /// Whether `condition` holds. A missing parameter, or one of another kind than the test's, is
    /// warned of and the condition doesn't hold.
    fn holds(&self, condition: &ParamCondition, warnings: &mut Warnings) -> bool {
        let name = &condition.param;
        let Some(value) = self.param(name) else {
            warnings.once(
                "animation.no_param",
                name,
                format_args!("tests parameter {name:?}, which it hasn't: the test doesn't hold"),
            );
            return false;
        };
        match (condition.test, value) {
            (ParamTest::Above(x), ParamValue::Float(value)) => value > x,
            (ParamTest::Below(x), ParamValue::Float(value)) => value < x,
            (ParamTest::True, ParamValue::Bool(set) | ParamValue::Trigger(set)) => set,
            (ParamTest::False, ParamValue::Bool(set) | ParamValue::Trigger(set)) => !set,
            (test, value) => {
                warnings.once(
                    "animation.wrong_param",
                    name,
                    format_args!(
                        "tests parameter {name:?}, a {}, {}: the test doesn't hold",
                        value.kind(),
                        test.tests()
                    ),
                );
                false
            }
        }
    }

    /// The float parameter `param` a blend blends along. A missing one or one of another kind
    /// counts as 0, warned of if `warnings` is given.
    fn along(&self, param: &str, warnings: Option<&mut Warnings>) -> f32 {
        let held = match self.param(param) {
            Some(ParamValue::Float(x)) => return x,
            held => held,
        };
        match (warnings, held) {
            (Some(warnings), Some(value)) => warnings.once(
                "animation.wrong_param",
                param,
                format_args!(
                    "blends along parameter {param:?}, a {}, where a blend takes a float: it \
                     blends as at 0",
                    value.kind()
                ),
            ),
            (Some(warnings), None) => warnings.once(
                "animation.no_param",
                param,
                format_args!(
                    "blends along parameter {param:?}, which it hasn't: it blends as at 0"
                ),
            ),
            (None, _) => {}
        }
        0.0
    }

    /// How much of state `at`'s motion has played, as an exit time measures it: infinite for a
    /// motion of no length.
    fn played(&self, at: usize, clips: &[Clip]) -> f32 {
        let state = &self.states[at];
        let length = match &state.motion {
            StateMotion::Clip(clip) => named(clips, clip).map_or(0.0, |clip| clip.duration),
            StateMotion::Blend { param, points } => {
                let between = between(points, self.along(param, None));
                blend_length(points, between, clips)
            }
        };
        if length > 0.0 {
            self.time * state.speed.abs() / length
        } else {
            f32::INFINITY
        }
    }

    /// Sets `plays` for state `at`, entered this tick with a fade of `entered` seconds, if at all.
    /// Each play of the state's clips takes its motion's weight, the state's speed and playback, a
    /// blend's step, and a fade that's done once the state's time passes it; every other play goes
    /// to weight 0.
    ///
    /// A tick that enters no state and brings back no blend clip that left at weight 0 allocates
    /// nothing. Otherwise it allocates each added clip's name and the plays' room beyond what they
    /// held, except where an added play replaces one leaving this tick (as a cut leaves each).
    fn drive(
        &self,
        at: usize,
        clips: &[Clip],
        plays: &mut Vec<Play>,
        entered: Option<f32>,
        warnings: &mut Warnings,
    ) {
        let state = &self.states[at];
        // Played once, a state's clips start afresh on entry, from their ends if backward.
        let once = state.playback == Playback::Once;
        let fresh = entered.is_some() && once;
        let start = if once && state.speed < 0.0 { 1.0 } else { 0.0 };
        let (count, between, length) = match &state.motion {
            StateMotion::Clip(_) => (1, None, None),
            StateMotion::Blend { param, points } => {
                let between = between(points, self.along(param, Some(&mut *warnings)));
                let length = blend_length(points, between, clips);
                (points.len(), between, Some(length))
            }
        };
        let blended = length.is_some();
        // Each clip's name, its motion's weight, and the clip if the model has it.
        let clip_of = |i: usize| {
            let (name, weight) = match &state.motion {
                StateMotion::Clip(clip) => (clip.as_str(), 1.0),
                StateMotion::Blend { points, .. } => (points[i].clip.as_str(), weight(between, i)),
            };
            (name, weight, named(clips, name))
        };
        for j in 0..plays.len() {
            match entered {
                Some(fade) => {
                    plays[j].target = 0.0;
                    plays[j].fade = fade;
                }
                None => {
                    let name = plays[j].clip.as_str();
                    let last = !plays[j + 1..].iter().any(|later| later.clip == name);
                    let ours =
                        (0..count).any(|i| matches!(clip_of(i), (of, _, Some(_)) if of == name));
                    if !(last && ours) {
                        plays[j].target = 0.0;
                    }
                }
            }
        }
        // Where a blend's clips are, as a fraction of their lengths: the heaviest held one's (the
        // first in blend order among equals), or a clip's start.
        let mut heaviest: Option<(f32, f32)> = None;
        if blended && !fresh {
            for i in 0..count {
                let (name, _, clip) = clip_of(i);
                let (Some(clip), Some(held)) = (clip, last_of(plays, name)) else {
                    continue;
                };
                let play = &plays[held];
                if clip.duration > 0.0 && heaviest.is_none_or(|(weight, _)| play.weight > weight) {
                    heaviest = Some((play.weight, play.time / clip.duration));
                }
            }
        }
        let phase = heaviest.map_or(start, |(_, phase)| phase);
        for i in 0..count {
            let (name, weight, clip) = clip_of(i);
            let Some(clip) = clip else {
                warnings.once(
                    "animation.no_clip",
                    name,
                    format_args!(
                        "plays clip {name:?} in state {:?}, which its model hasn't: it's left out",
                        state.name
                    ),
                );
                continue;
            };
            let speed = match length {
                Some(length) if length > 0.0 => state.speed * clip.duration / length,
                _ => state.speed,
            };
            let time = phase * clip.duration;
            match (!fresh).then(|| last_of(plays, name)).flatten() {
                Some(held) => {
                    let play = &mut plays[held];
                    play.target = weight;
                    // Once the state has lasted its fade, the weight is the target.
                    if entered.is_none() && self.time >= play.fade {
                        play.fade = 0.0;
                    }
                    play.speed = speed;
                    play.playback = state.playback;
                    if blended {
                        play.time = time;
                    }
                }
                None if weight > 0.0 => {
                    let fade = entered.unwrap_or(0.0);
                    let play = Play {
                        clip: String::new(),
                        time,
                        speed,
                        playback: state.playback,
                        weight: if fade > 0.0 { 0.0 } else { weight },
                        target: weight,
                        fade,
                    };
                    add(plays, name, play);
                }
                None => {}
            }
        }
    }
}

/// The points `x` falls between, and how far it is from the first to the second, 0 to 1: the
/// nearest at or below and the nearest at or above (the first, at the same point). At a point, or
/// past every point, the nearest alone, at 0. A NaN value or points give the first point alone.
fn between(points: &[BlendPoint], x: f32) -> Option<(usize, usize, f32)> {
    let (mut below, mut above): (Option<usize>, Option<usize>) = (None, None);
    for (i, point) in points.iter().enumerate() {
        if point.at <= x && below.is_none_or(|below| point.at > points[below].at) {
            below = Some(i);
        }
        if point.at >= x && above.is_none_or(|above| point.at < points[above].at) {
            above = Some(i);
        }
    }
    match (below, above) {
        (Some(below), Some(above)) if points[above].at > points[below].at => {
            let (low, high) = (points[below].at, points[above].at);
            Some((below, above, (x - low) / (high - low)))
        }
        (Some(only), _) | (None, Some(only)) => Some((only, only, 0.0)),
        (None, None) => (!points.is_empty()).then_some((0, 0, 0.0)),
    }
}

/// The weight of blend point `i` where its value falls [`between`] points.
fn weight(between: Option<(usize, usize, f32)>, i: usize) -> f32 {
    match between {
        Some((low, _, t)) if i == low => 1.0 - t,
        Some((_, high, t)) if i == high => t,
        _ => 0.0,
    }
}

/// The length of a blend of `points` whose value falls [`between`] them, in a model with `clips`:
/// the lengths of its clips of some length, weighted as the blend weights them over those clips'
/// weights alone, so a missing clip or one of no length has no share and speeds up no other. 0 if
/// no weighted clip has a length.
fn blend_length(
    points: &[BlendPoint],
    between: Option<(usize, usize, f32)>,
    clips: &[Clip],
) -> f32 {
    let (mut weighted, mut total) = (0.0, 0.0);
    for (i, point) in points.iter().enumerate() {
        let weight = weight(between, i);
        let duration = named(clips, &point.clip).map_or(0.0, |clip| clip.duration);
        if weight > 0.0 && duration > 0.0 {
            weighted += weight * duration;
            total += weight;
        }
    }
    if total > 0.0 { weighted / total } else { 0.0 }
}

/// The index of the newest (last) play of clip `name`.
fn last_of(plays: &[Play], name: &str) -> Option<usize> {
    plays.iter().rposition(|play| play.clip == name)
}

/// Whether `play` leaves its animator at this tick's step as its fields stand: its target is 0
/// and it has no weight or no fade.
fn leaves(play: &Play) -> bool {
    let kept = play.target > 0.0 || (play.fade > 0.0 && play.weight > 0.0);
    !kept
}

/// Adds `play`, as a play of clip `name`, after `plays`, reusing the name's allocation of a play
/// that leaves this tick, if any. That play is at weight 0 and can't show, so sampling is as if
/// the new one were added after them all.
fn add(plays: &mut Vec<Play>, name: &str, play: Play) {
    let leaving = plays.iter().position(leaves);
    let mut clip = leaving.map_or_else(String::new, |at| plays.remove(at).clip);
    clip.clear();
    clip.push_str(name);
    plays.push(Play { clip, ..play });
}

/// Warns of a machine's missing names, once per world: the world (which keeps what it warned of)
/// and the machine's entity.
struct Warnings<'w> {
    world: &'w mut World,
    id: EntityId,
}

/// What a world's machines have been warned of, by entity ID, code and name, so each warns once
/// rather than every tick. As few as a world's machines' mistakes, searched in turn. Snapshots
/// copy it, so a restored world warns as the original would.
#[derive(Clone, Debug, Default)]
struct Warned(Vec<(EntityId, &'static str, String)>);

impl Warnings<'_> {
    /// Warns with `code` that the machine `what`, about `name`, unless it has before.
    fn once(&mut self, code: &'static str, name: &str, what: fmt::Arguments<'_>) {
        let id = self.id;
        if self.world.resource::<Warned>().is_none() {
            self.world.insert_resource(Warned::default());
        }
        let Some(Warned(warned)) = self.world.resource_mut::<Warned>() else {
            return;
        };
        let before = |&(of, warned, ref about): &(EntityId, &str, String)| {
            of == id && warned == code && about == name
        };
        if warned.iter().any(before) {
            return;
        }
        warned.push((id, code, name.to_owned()));
        tracing::warn!(code, entity = %id, name, "Entity {id}'s state machine {what}");
    }
}

/// Steps each state machine of a model's root one [`Tick::DT`] in query order, giving each root an
/// animator if it has none.
pub(super) fn step(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let mut roots = world.take_list();
    let machines = world.query::<(Entity, &AnimationStates, &ModelRoot)>();
    roots.extend(machines.map(|(root, _, _)| root));
    for &root in &roots {
        let (Some(&ModelRoot(model)), Some(id)) = (world.get::<ModelRoot>(root), world.id(root))
        else {
            continue;
        };
        let Some(clips) = world
            .assets()
            .nodes(model)
            .map(|nodes| Arc::clone(&nodes.clips))
        else {
            continue;
        };
        if world.get::<Animator>(root).is_none() && world.insert(root, Animator::default()).is_err()
        {
            continue;
        }
        // Taken out while it steps, then put back, as the animator's plays are.
        let machine = world.get_mut::<AnimationStates>(root);
        let mut machine = mem::take(machine.expect("listed with one"));
        let Animator(plays) = world.get_mut::<Animator>(root).expect("given one");
        let mut plays = mem::take(plays);
        machine.step(&clips, &mut plays, dt, &mut Warnings { world, id });
        world.get_mut::<Animator>(root).expect("given one").0 = plays;
        *world
            .get_mut::<AnimationStates>(root)
            .expect("listed with one") = machine;
    }
    world.keep_list(roots);
}

impl World {
    /// Sets parameter `name` of `entity`'s [`AnimationStates`] to `value`, as
    /// [`AnimationStates::set_param`] does: `world.set_param(fox, "speed", 2.5)` for a float,
    /// `world.set_param(fox, "pounce", true)` for a bool or trigger.
    ///
    /// # Errors
    ///
    /// - `ecs.no_entity`: `entity` isn't alive.
    /// - `reflect.missing_component`: it has no state machine.
    /// - Those of `AnimationStates::set_param`.
    pub fn set_param(
        &mut self,
        entity: Entity,
        name: &str,
        value: impl Into<ParamValue>,
    ) -> Result<()> {
        let Some(id) = self.id(entity) else {
            return Err(Error::new(
                "ecs.no_entity",
                "the entity isn't in the world: it was despawned, or is another world's",
            ));
        };
        let Some(machine) = self.get_mut::<AnimationStates>(entity) else {
            return Err(Error::new(
                "reflect.missing_component",
                format!("entity {id} has no `AnimationStates`, whose parameters it would set"),
            ));
        };
        machine.set_param(name, value).map_err(|error| {
            let message = format!("entity {id}: {error}");
            error.with_message(message)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ParamTest::{Above, Below, False, True};
    use ParamValue::{Bool, Float, Trigger};
    use Playback::{Loop, Once};
    use gg_core::math::DVec3;

    use super::*;
    use crate::animation::animator::tests::{at, clips, node, spawned, weights};
    use crate::animation::step_animation;
    use crate::model::tests::added;
    use crate::reflect::Value;
    use crate::{Context, Model, Transform};

    /// A state `name` playing `clip` at its own speed.
    fn state(name: &str, clip: &str, playback: Playback) -> AnimationState {
        AnimationState {
            name: name.to_owned(),
            motion: StateMotion::Clip(clip.to_owned()),
            speed: 1.0,
            playback,
        }
    }

    /// A looping state `name` blending `points` (clip, point) along `param`.
    fn blend(name: &str, param: &str, points: &[(&str, f32)]) -> AnimationState {
        let points = points.iter().map(|&(clip, at)| BlendPoint {
            clip: clip.to_owned(),
            at,
        });
        AnimationState {
            name: name.to_owned(),
            motion: StateMotion::Blend {
                param: param.to_owned(),
                points: points.collect(),
            },
            speed: 1.0,
            playback: Loop,
        }
    }

    fn param(name: &str, value: ParamValue) -> AnimationParam {
        AnimationParam {
            name: name.to_owned(),
            value,
        }
    }

    /// A cutting transition from `from` (any state if `None`) to `to` once `conditions` (parameter,
    /// test) hold.
    fn transition(
        from: Option<&str>,
        to: &str,
        conditions: &[(&str, ParamTest)],
    ) -> StateTransition {
        let conditions = conditions.iter().map(|&(param, test)| ParamCondition {
            param: param.to_owned(),
            test,
        });
        StateTransition {
            from: from.map(str::to_owned),
            to: to.to_owned(),
            conditions: conditions.collect(),
            exit_time: None,
            fade: 0.0,
        }
    }

    /// `transition` fading over `fade` seconds, after `exit` of its state if given.
    fn faded(fade: f32, exit: Option<f32>, transition: StateTransition) -> StateTransition {
        StateTransition {
            fade,
            exit_time: exit,
            ..transition
        }
    }

    /// A root of the animator tests' model with `machine`, and its context.
    fn driven(machine: AnimationStates) -> (Context, Entity) {
        let (mut cx, root) = spawned();
        cx.world_mut().insert(root, machine).expect("alive");
        (cx, root)
    }

    fn machine_of(world: &World, root: Entity) -> &AnimationStates {
        world.get::<AnimationStates>(root).expect("a state machine")
    }

    fn plays_of(world: &World, root: Entity) -> &[Play] {
        &world.get::<Animator>(root).expect("an animator").0
    }

    fn run(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            step_animation(world);
        }
    }

    /// A rest that slides, a move blending slide and lift by speed, and a swing played once at a
    /// trigger from any state, then back to moving.
    fn moving() -> AnimationStates {
        AnimationStates {
            params: vec![param("speed", Float(0.0)), param("go", Trigger(false))],
            states: vec![
                state("Rest", "Slide", Loop),
                blend("Move", "speed", &[("Slide", 1.0), ("Lift", 3.0)]),
                state("Swing", "Swing", Once),
            ],
            transitions: vec![
                faded(
                    0.25,
                    None,
                    transition(Some("Rest"), "Move", &[("speed", Above(0.5))]),
                ),
                faded(
                    0.25,
                    None,
                    transition(Some("Move"), "Rest", &[("speed", Below(0.5))]),
                ),
                faded(0.2, None, transition(None, "Swing", &[("go", True)])),
                faded(0.25, Some(1.0), transition(Some("Swing"), "Move", &[])),
            ],
            ..AnimationStates::default()
        }
    }

    /// A transition fires at the first tick after entry that finds all conditions hold, on each
    /// kind of test, and not while one doesn't.
    #[test]
    fn a_transition_fires_once_its_conditions_all_hold() {
        let cases = [
            (Float(1.0), Above(0.5), true),
            (Float(0.5), Above(0.5), false),
            (Float(0.2), Below(0.5), true),
            (Float(1.0), Below(0.5), false),
            (Bool(true), True, true),
            (Bool(false), True, false),
            (Bool(false), False, true),
            (Bool(true), False, false),
            (Trigger(true), True, true),
            (Trigger(false), True, false),
            (Trigger(false), False, true),
        ];
        for (on, value, test, fires) in (cases.iter().map(|&(v, t, f)| (true, v, t, f))).chain([(
            false,
            Float(1.0),
            Above(0.5),
            false,
        )]) {
            let machine = AnimationStates {
                params: vec![param("p", value), param("on", Bool(on))],
                states: vec![state("A", "Slide", Loop), state("B", "Lift", Loop)],
                transitions: vec![transition(Some("A"), "B", &[("on", True), ("p", test)])],
                ..AnimationStates::default()
            };
            let (mut cx, root) = driven(machine);
            run(cx.world_mut(), 1);
            assert_eq!(machine_of(cx.world(), root).state, "A", "entered first");
            run(cx.world_mut(), 1);
            let world = cx.world();
            let (state, clip) = if fires { ("B", "Lift") } else { ("A", "Slide") };
            let what = format!("{value:?} {test:?}, on {on}");
            assert_eq!(machine_of(world, root).state, state, "{what}");
            assert_eq!(weights(world, root), [(clip.to_owned(), 1.0)], "{what}");
        }
    }

    /// A trigger fires a transition testing it once and is consumed, so the next tick stays. One
    /// set in the state a from-any transition enters waits until that state is left, as such a
    /// transition skips the state it enters. Tools set it through its fields.
    #[test]
    fn a_trigger_fires_once_and_is_consumed() {
        let (mut cx, root) = driven(moving());
        let world = cx.world_mut();
        run(world, 1);
        let pull = Value::Variant("Trigger".into(), vec![("0".into(), Value::Bool(true))]);
        world
            .set_field(root, "AnimationStates", "params[1].value", &pull)
            .expect("a trigger, set as tools set it");
        run(world, 1);
        let machine = machine_of(world, root);
        assert_eq!(machine.state, "Swing");
        assert_eq!(machine.param("go"), Some(Trigger(false)), "consumed");
        // So the move it returns to stays.
        world.set_param(root, "speed", 2.0).expect("a float");
        run(world, 10);
        let machine = machine_of(world, root);
        assert_eq!(
            (machine.state.as_str(), machine.time),
            ("Swing", 11.0 / 64.0)
        );
        world.set_param(root, "go", true).expect("a trigger");
        run(world, 1);
        let machine = machine_of(world, root);
        assert_eq!(
            (machine.state.as_str(), machine.time),
            ("Swing", 12.0 / 64.0)
        );
        assert_eq!(machine.param("go"), Some(Trigger(true)), "waiting");
        // Swing's one-second clip is played at its 64th tick; then the waiting trigger.
        run(world, 52);
        assert_eq!(machine_of(world, root).state, "Swing");
        run(world, 1);
        assert_eq!(machine_of(world, root).state, "Move");
        run(world, 1);
        let machine = machine_of(world, root);
        assert_eq!(machine.state, "Swing");
        assert_eq!(machine.param("go"), Some(Trigger(false)));
    }

    /// An exit time waits until the state has played that share of its motion, at its speed; the
    /// transition cross-fades as `World::play` does, the entered state then playing alone.
    #[test]
    fn an_exit_time_waits_for_its_share_and_a_fade_cross_fades() {
        let lift = AnimationState {
            speed: 2.0,
            ..state("A", "Lift", Loop)
        };
        let machine = AnimationStates {
            states: vec![lift, state("B", "Slide", Loop)],
            transitions: vec![faded(0.25, Some(0.5), transition(Some("A"), "B", &[]))],
            ..AnimationStates::default()
        };
        let (mut cx, root) = driven(machine);
        let world = cx.world_mut();
        // Half of Lift's two seconds at double speed: 0.5 s, 32 ticks after the first.
        run(world, 32);
        assert_eq!(machine_of(world, root).state, "A");
        assert_eq!(plays_of(world, root)[0].speed, 2.0);
        run(world, 1);
        assert_eq!(machine_of(world, root).state, "B");
        let eased = [
            ("Lift".to_owned(), 1.0 - 4.0 / 64.0),
            ("Slide".to_owned(), 4.0 / 64.0),
        ];
        assert_eq!(weights(world, root), eased);
        run(world, 15);
        assert_eq!(weights(world, root), [("Slide".to_owned(), 1.0)]);
        assert_eq!(plays_of(world, root)[0].fade, 0.25);
        run(world, 1);
        assert_eq!(plays_of(world, root)[0].fade, 0.0, "its fade done");
    }

    /// A blend weights the two clips its parameter falls between by nearness, at speeds that keep
    /// them at the same fraction of their lengths. Past the last point only that clip plays; a
    /// clip that comes back joins where the others are.
    #[test]
    fn a_blend_weights_its_nearest_clips_and_keeps_them_in_step() {
        let machine = AnimationStates {
            params: vec![param("speed", Float(1.5))],
            states: vec![blend(
                "Move",
                "speed",
                &[("Slide", 1.0), ("Lift", 3.0), ("Swing", 5.0)],
            )],
            ..AnimationStates::default()
        };
        let (mut cx, root) = driven(machine);
        let world = cx.world_mut();
        run(world, 1);
        let blended = [("Slide".to_owned(), 0.75), ("Lift".to_owned(), 0.25)];
        assert_eq!(weights(world, root), blended);
        // Length 1.25 s: Slide's 1 s and Lift's 2 s, weighted.
        let speeds: Vec<f32> = plays_of(world, root)
            .iter()
            .map(|play| play.speed)
            .collect();
        assert_eq!(speeds, [0.8, 1.6]);
        let in_step = |world: &World| {
            let plays = plays_of(world, root);
            let (first, second) = (&plays[0], &plays[1]);
            let lengths = |play: &Play| if play.clip == "Lift" { 2.0 } else { 1.0 };
            let phases = [first.time / lengths(first), second.time / lengths(second)];
            assert!((phases[0] - phases[1]).abs() < 1e-6, "{phases:?}");
            assert!(phases[0] > 0.0, "played: {phases:?}");
        };
        run(world, 100);
        in_step(world);
        world.set_param(root, "speed", 6.0).expect("a float");
        run(world, 1);
        assert_eq!(weights(world, root), [("Swing".to_owned(), 1.0)]);
        world.set_param(root, "speed", 4.0).expect("a float");
        run(world, 1);
        let joined = [("Swing".to_owned(), 0.5), ("Lift".to_owned(), 0.5)];
        assert_eq!(weights(world, root), joined);
        in_step(world);
    }

    /// A blend clip its model lacks, or of no length (a still pose), has no share of the blend's
    /// length: the other plays at its own speed, and an exit time measures its length.
    #[test]
    fn a_blend_s_length_leaves_out_clips_without_one() {
        for still in ["Pose", "Tango"] {
            let mut cx = Context::seeded(1);
            let mut imported = clips();
            imported.clips.push(Clip {
                name: "Pose".to_owned(),
                duration: 0.0,
                channels: Vec::new(),
            });
            let (model, _) = added(&mut cx, "m", imported);
            let world = cx.world_mut();
            let root = Model::spawn(model, world, Transform::IDENTITY);
            let machine = AnimationStates {
                params: vec![param("speed", Float(0.2))],
                states: vec![
                    blend("Move", "speed", &[(still, 0.0), ("Lift", 2.0)]),
                    state("Rest", "Slide", Loop),
                ],
                transitions: vec![faded(0.0, Some(0.5), transition(Some("Move"), "Rest", &[]))],
                ..AnimationStates::default()
            };
            world.insert(root, machine).expect("alive");
            run(world, 1);
            let plays = plays_of(world, root);
            let lift = plays.iter().find(|play| play.clip == "Lift").expect("Lift");
            assert_eq!((lift.weight, lift.speed), (0.1, 1.0), "{still}");
            // Half of Lift's two seconds at its own speed: 1 s, 64 ticks after the first.
            run(world, 63);
            assert_eq!(machine_of(world, root).state, "Move", "{still}");
            run(world, 1);
            assert_eq!(machine_of(world, root).state, "Rest", "{still}");
        }
    }

    /// A state played once backward starts its clip at the end and plays to the first pose,
    /// reached as its exit time (at the size of its speed) comes.
    #[test]
    fn a_one_shot_played_backward_goes_from_its_end_to_its_start() {
        let back = AnimationState {
            speed: -1.0,
            ..state("Back", "Slide", Once)
        };
        let machine = AnimationStates {
            states: vec![back, state("Rest", "Swing", Loop)],
            transitions: vec![faded(0.0, Some(1.0), transition(Some("Back"), "Rest", &[]))],
            ..AnimationStates::default()
        };
        let (mut cx, root) = driven(machine);
        let world = cx.world_mut();
        // Slide's root goes 0 to 2 along X in a second.
        let x = |world: &World| at(world, node(world, root, 0)).translation.x;
        run(world, 1);
        assert_eq!(x(world), 2.0 * 63.0 / 64.0, "from its end");
        run(world, 31);
        assert_eq!(x(world), 1.0);
        run(world, 32);
        let machine = machine_of(world, root);
        assert_eq!((machine.state.as_str(), x(world)), ("Back", 0.0));
        run(world, 1);
        assert_eq!(machine_of(world, root).state, "Rest");
    }

    /// A blend entered while the animator holds its clips at different phases (as a fading-out
    /// one drifts) takes the heaviest's phase: it goes on where it was, the others brought to it.
    #[test]
    fn a_blend_takes_its_phase_from_its_heaviest_held_clip() {
        let machine = AnimationStates {
            params: vec![param("speed", Float(2.0))],
            states: vec![blend("Move", "speed", &[("Slide", 1.0), ("Lift", 3.0)])],
            ..AnimationStates::default()
        };
        let (mut cx, root) = driven(machine);
        let world = cx.world_mut();
        let held = |clip, time, weight| Play {
            time,
            weight,
            target: weight,
            ..Play::new(clip)
        };
        let plays = vec![held("Slide", 0.1, 0.2), held("Lift", 1.0, 0.8)];
        world.insert(root, Animator(plays)).expect("alive");
        run(world, 1);
        // Halfway round Lift's two seconds, plus a tick at the blend's 1.5.
        let phase = 0.5 + 1.0 / 64.0 / 1.5;
        let plays = plays_of(world, root);
        let phases = [plays[0].time, plays[1].time / 2.0];
        for of in phases {
            assert!((of - phase).abs() < 1e-6, "{phases:?}, not {phase}");
        }
    }

    /// A missing state, parameter or clip, and a test of a parameter of another kind, are each
    /// warned of once however many ticks find them. A current state it lacks leaves for the first,
    /// and a transition to one never fires. Code that sets a missing parameter, one of another
    /// kind, or one of an entity without a machine, is refused.
    #[test]
    fn what_it_names_and_hasnt_is_warned_of_once() {
        let logged = crate::testing::logged();
        let machine = AnimationStates {
            params: vec![param("flag", Bool(true))],
            states: vec![
                state("Rest", "Slide", Loop),
                blend("Dance", "tempo", &[("Tango", 0.0)]),
            ],
            transitions: vec![
                transition(Some("Rest"), "Gone", &[]),
                transition(Some("Rest"), "Dance", &[("missing", Above(0.0))]),
                transition(Some("Rest"), "Dance", &[("flag", Above(0.0))]),
                transition(Some("Nowhere"), "Dance", &[]),
            ],
            state: "Lost".to_owned(),
            ..AnimationStates::default()
        };
        let (mut cx, root) = driven(machine);
        let world = cx.world_mut();
        run(world, 10);
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let warned = [
            "animation.no_state",
            "animation.no_state",
            "animation.no_param",
            "animation.wrong_param",
            "animation.no_state",
        ];
        assert_eq!(codes, warned);
        assert_eq!(machine_of(world, root).state, "Rest");
        assert_eq!(weights(world, root), [("Slide".to_owned(), 1.0)]);
        // A tool enters a state blending a missing clip along a missing parameter: what played
        // leaves, and the clip is left out.
        let dance = Value::String("Dance".to_owned());
        let set = world.set_field(root, "AnimationStates", "state", &dance);
        set.expect("a string");
        run(world, 4);
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert_eq!(codes, ["animation.no_param", "animation.no_clip"]);
        assert!(weights(world, root).is_empty());
        let refused = |result: Result<()>| result.expect_err("refused").code();
        assert_eq!(
            refused(world.set_param(root, "missing", 1.0)),
            "animation.no_param"
        );
        assert_eq!(
            refused(world.set_param(root, "flag", 1.0)),
            "animation.wrong_param"
        );
        let bare = world.spawn(Transform::IDENTITY);
        assert_eq!(
            refused(world.set_param(bare, "flag", true)),
            "reflect.missing_component"
        );
    }

    /// A world restored from a snapshot taken mid-cross-fade goes on as the original did, tick for
    /// tick: same state, time, fade and parameters.
    #[test]
    fn a_state_machine_restored_from_a_snapshot_goes_on_exactly() {
        let (mut cx, root) = driven(moving());
        let world = cx.world_mut();
        world.set_param(root, "speed", 2.0).expect("a float");
        run(world, 40);
        world.set_param(root, "go", true).expect("a trigger");
        run(world, 3);
        assert_eq!(machine_of(world, root).state, "Swing");
        assert_eq!(
            plays_of(world, root).len(),
            3,
            "Swing fading in over a blend"
        );
        let snapshot = world.clone();
        let go_on = |world: &mut World| -> Vec<u64> {
            (0..120)
                .map(|tick| {
                    if tick == 90 {
                        world.set_param(root, "speed", 0.25).expect("a float");
                    }
                    step_animation(world);
                    world.hash().content
                })
                .collect()
        };
        let went = go_on(world);
        assert_eq!(machine_of(world, root).state, "Rest", "it went on");
        world.clone_from(&snapshot);
        assert_eq!(go_on(world), went);
    }

    /// The showcase's Fox states: surveying while standing, walking and running blended by speed,
    /// and a look about at a trigger from any state before going on.
    fn fox() -> AnimationStates {
        let look = AnimationState {
            speed: 1.5,
            ..state("Look", "Survey", Once)
        };
        AnimationStates {
            params: vec![param("speed", Float(0.0)), param("look", Trigger(false))],
            states: vec![
                state("Idle", "Survey", Loop),
                blend("Move", "speed", &[("Walk", 1.6), ("Run", 5.0)]),
                look,
            ],
            transitions: vec![
                faded(
                    0.25,
                    None,
                    transition(Some("Idle"), "Move", &[("speed", Above(0.2))]),
                ),
                faded(
                    0.3,
                    None,
                    transition(Some("Move"), "Idle", &[("speed", Below(0.2))]),
                ),
                faded(0.2, None, transition(None, "Look", &[("look", True)])),
                faded(
                    0.3,
                    Some(0.9),
                    transition(Some("Look"), "Move", &[("speed", Above(0.2))]),
                ),
                faded(0.3, Some(0.9), transition(Some("Look"), "Idle", &[])),
            ],
            ..AnimationStates::default()
        }
    }

    /// The world's hash after eight seconds of four skinned Khronos Foxes, each a little behind
    /// the one before, driven from standing to a run and back (blend clips in step, a look at a
    /// trigger from the move): a reference CI checks on every platform, as lockstep peers and
    /// replays need. A difference on some platforms only is a determinism bug, not a value to
    /// change.
    #[test]
    fn state_machines_hash_as_their_reference_says() {
        let logged = crate::testing::logged();
        // The Fox's files without its ID file, under their own root: the hash reads the model's
        // name, then its path from the root, with `dev` and without, wherever the checkout is.
        let scratch = crate::dirs::tests::Scratch::new("states-hash");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let files = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
        for file in ["Fox.gltf", "Fox.bin", "Texture.png"] {
            std::fs::copy(files.join(file), scratch.0.join(file)).expect("copied");
        }
        let mut cx = Context::seeded(7).root(scratch.0.clone());
        let model = (cx.add_model("fox", scratch.0.join("Fox.gltf"))).expect("the Fox reads");
        // A development build's scan warns of files without ID files (`asset.no_id`).
        let _ = logged.codes.try_iter().count();
        let world = cx.world_mut();
        let foxes: Vec<Entity> = (0..4)
            .map(|i| {
                let at = Transform {
                    translation: DVec3::new(f64::from(2.0 * i as f32), 0.0, 0.0),
                    ..Transform::IDENTITY
                };
                let root = model.spawn(world, at);
                world.insert(root, fox()).expect("alive");
                root
            })
            .collect();
        let mut looked = [false; 4];
        for tick in 0..512 {
            for (i, &root) in foxes.iter().enumerate() {
                // Up toward a run and back to standing, each fox 16 ticks behind.
                let seconds = (tick as f32 - 16.0 * i as f32) / 64.0;
                let speed = (1.5 * seconds).min(1.5 * (7.0 - seconds)).max(0.0);
                world.set_param(root, "speed", speed).expect("a float");
                if tick == 100 + 60 * i {
                    world.set_param(root, "look", true).expect("a trigger");
                }
            }
            step_animation(world);
            for (looked, &root) in looked.iter_mut().zip(&foxes) {
                *looked |= machine_of(world, root).state == "Look";
            }
        }
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.is_empty(), "every name found: {codes:?}");
        assert_eq!(looked, [true; 4], "each looked about");
        let states: Vec<&str> = (foxes.iter())
            .map(|&root| machine_of(world, root).state.as_str())
            .collect();
        assert_eq!(states, ["Idle"; 4], "each stands again");
        let content = world.hash().content;
        assert_eq!(content, 0x31aa_474f_5d5e_f2dd, "{content:#018x}");
    }
}
