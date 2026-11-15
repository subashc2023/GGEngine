//! Animation state machines: a model's root's states, each a clip or clips blended along a
//! parameter, and the transitions between them on its parameters' values, which drive its
//! animator's plays at the ticks.

use std::fmt;
use std::mem;
use std::sync::Arc;

use gg_core::{Error, Result};

use super::Playback;
use super::animator::{Animator, Play, named};
use super::skeleton::Clip;
use crate::{Component, Entity, EntityId, ModelRoot, Reflect, Tick, World, reflect};

/// A state machine of the clips a model's root plays, beside its [`Animator`], which it drives at
/// the ticks [`step_animation`](super::step_animation) runs in, before the animator steps: its
/// parameters, which code, scripts and tools set; its states, each a clip, or clips blended along a
/// parameter; and its transitions from state to state on its parameters' values. Each tick, the
/// transitions from its state, and from any state, are checked in their order, and the first that
/// holds fires, the animator cross-fading to the state it enters as [`World::play`] has it do; then
/// the state's plays take the weights its motion gives them, and every other play eases to 0 and
/// leaves, so that a state of a lone clip, once its fade is done, plays it exactly. Its state and
/// the seconds it has been in it are fields of its own, and its plays' times its animator's, so
/// that a snapshot, a replay or a joining peer goes on exactly where it was. An entity without a
/// [`ModelRoot`] steps none, and one without an animator is given one.
///
/// Entering a state, the play of each clip it plays that the animator holds eases to its weight
/// from where it is, keeping its time, as `World::play` has it, unless the state plays once, which
/// starts each of its clips afresh, from its end if the state plays backward, the plays of them the
/// animator held easing out with the rest; a clip the animator hasn't joins from its start, at 0,
/// or at its weight for a fade of 0. A blend's clips keep in step, each at the same fraction of its
/// length, the heaviest held one's, the first in the blend's order of those as heavy, each played
/// at the speed that brings it round in the blend's length: its clips' lengths weighted as the blend
/// weights them, over the weights of those its model has of some length, so that a clip it hasn't,
/// or one of no length, as a still pose, speeds up none of the others. So a walk and a run blend
/// into a jog whose feet agree.
///
/// What it names and hasn't is warned of once a world, by name: a state (`animation.no_state`),
/// which, named as its own state, it leaves for its first, and to which no transition fires; a
/// parameter (`animation.no_param`), whose conditions don't hold, and along which a blend blends as
/// at 0; a test or a blend of a parameter of another kind (`animation.wrong_param`), likewise; and a
/// clip its model hasn't (`animation.no_clip`), which it leaves out.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub struct AnimationStates {
    /// The values that transitions' conditions test and blends blend along, each by its name:
    /// code sets them with [`World::set_param`], scripts with `gg.set_param`, and tools through
    /// these fields.
    pub params: Vec<AnimationParam>,
    /// The states, each by its name.
    pub states: Vec<AnimationState>,
    /// The transitions, in the order they're checked.
    pub transitions: Vec<StateTransition>,
    /// The state it's in, by its name. Empty, as a scene starts it, it enters the first at its first
    /// tick; set to another, as by a tool, it cuts to that one's clips at the next, its time going
    /// on from what it's given.
    pub state: String,
    /// The seconds it has been in its state, which an exit time measures.
    pub time: f32,
}

/// A parameter of an [`AnimationStates`]: its name and its value.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct AnimationParam {
    /// Its name, which conditions, blends and setters name it by.
    pub name: String,
    /// Its value, whose kind a value set to it keeps.
    pub value: ParamValue,
}

/// The value of an [`AnimationParam`], and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum ParamValue {
    /// A number, such as a speed, which a blend blends along, and which a condition tests is above
    /// or below a value.
    Float(f32),
    /// A flag that stays as it's set.
    Bool(bool),
    /// A flag that a transition that fires on it, as it's set, consumes, setting it back.
    Trigger(bool),
}

impl From<f32> for ParamValue {
    fn from(x: f32) -> Self {
        Self::Float(x)
    }
}

impl From<bool> for ParamValue {
    /// A bool, which sets a bool or a trigger alike.
    fn from(set: bool) -> Self {
        Self::Bool(set)
    }
}

impl ParamValue {
    /// The name of its variant.
    fn kind(self) -> &'static str {
        match self {
            Self::Float(_) => "Float",
            Self::Bool(_) => "Bool",
            Self::Trigger(_) => "Trigger",
        }
    }
}

/// A state of an [`AnimationStates`]: its name and what it plays, how fast and how it goes on at
/// its end.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct AnimationState {
    /// Its name, which the machine's `state` and transitions name it by.
    pub name: String,
    /// The clip it plays, or the clips it blends.
    pub motion: StateMotion,
    /// How fast it plays: 1 as its clips go, 2 twice as fast, and backward below 0.
    #[reflect(default = 1.0)]
    pub speed: f32,
    /// Whether its clips play once and hold their last pose, loop, or go back and forth; played
    /// once, each starts afresh as the state is entered, from its end if its speed is below 0.
    #[reflect(default = Playback::Loop)]
    pub playback: Playback,
}

/// What an [`AnimationState`] plays.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub enum StateMotion {
    /// One clip, by its name among its model's.
    Clip(String),
    /// Clips blended along the float parameter `param`, each at its point: the two nearest the
    /// parameter's value either side weighted by how near it is to each, the clip at a point it's
    /// at, or past every point the nearest, alone.
    Blend {
        param: String,
        points: Vec<BlendPoint>,
    },
}

/// A clip of a blend, at its point along the blend's parameter.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct BlendPoint {
    /// The clip, by its name among its model's.
    pub clip: String,
    /// The parameter's value at which it plays alone.
    pub at: f32,
}

/// A transition of an [`AnimationStates`] from a state, or from any, to another, once its
/// conditions all hold.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct StateTransition {
    /// The state it leaves, by its name; `None`, any but the one it enters.
    pub from: Option<String>,
    /// The state it enters, by its name.
    pub to: String,
    /// What must all hold of the parameters for it to fire: none, and it fires as soon as its exit
    /// time allows. It consumes each trigger it tests is set as it fires.
    pub conditions: Vec<ParamCondition>,
    /// How much of its motion the state it leaves must have played first, if any: its seconds in
    /// the state times the size of its speed, over its clip's length, or over its blend's, as its
    /// weights are, as [`AnimationStates`] says; past 1 for more than a round of a loop.
    pub exit_time: Option<f32>,
    /// The seconds the animator takes to cross-fade from the state it leaves to the one it enters,
    /// as [`World::play`] fades: at 0, it cuts.
    #[reflect(min = 0.0)]
    pub fade: f32,
}

/// What a [`StateTransition`] tests of a parameter.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct ParamCondition {
    /// The parameter, by its name.
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
    /// A bool that's true, or a trigger that's set, which the transition consumes as it fires.
    True,
    /// A bool that's false, or a trigger that isn't set.
    False,
}

impl ParamTest {
    /// What it tests, as a warning says it.
    fn tests(self) -> &'static str {
        match self {
            Self::Above(_) | Self::Below(_) => "against a number, as a float's value is",
            Self::True | Self::False => "as a flag, as a bool or a trigger is",
        }
    }
}

impl AnimationStates {
    /// The value of its parameter `name`, if it has one.
    pub fn param(&self, name: &str) -> Option<ParamValue> {
        let mut params = self.params.iter();
        params
            .find(|param| param.name == name)
            .map(|param| param.value)
    }

    /// Sets its parameter `name` to `value`, keeping its kind: a float to a number, and a bool or a
    /// trigger to a bool, a trigger set staying set until a transition that fires on it consumes
    /// it, or it's set back.
    ///
    /// Fails with `animation.no_param`, naming its parameters, if it has none of that name; and with
    /// `animation.wrong_param` for a value of another kind than the parameter's.
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

    /// The index of its state named `name`.
    fn position(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|state| state.name == name)
    }

    /// Steps it a tick of `dt` seconds, `plays` its animator's, of a model whose clips are `clips`:
    /// fires the first of its transitions that holds, then sets the plays for the state it's in,
    /// as [`AnimationStates`] says, warning of what it names and hasn't through `warnings`.
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

    /// The first of its transitions that holds in state `at`, as [`AnimationStates`] says, having
    /// consumed the triggers it tests: the state it enters and the seconds its fade takes.
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

    /// Whether `condition` holds of its parameters, warning of a parameter it hasn't, or of another
    /// kind than its test's, which it doesn't hold of.
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

    /// The value of the float parameter `param` that a blend blends along, warning of a parameter
    /// it hasn't or of another kind, which it takes as 0, if `warnings` are given.
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

    /// How much of its motion state `at` has played, as an exit time measures it: without end for
    /// a motion of no length.
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

    /// Sets `plays` for state `at`, which a transition of a fade of `entered` seconds entered at
    /// this tick, if it did: each play of a clip of the state's to the weight its motion gives it,
    /// with the state's speed and playback, a blend's in step, and a fade that's done once the
    /// state's time is past it; and each other play to a weight of 0, as [`AnimationStates`] says.
    /// A tick that enters no state, nor brings back a blend's clip that left at a weight of 0,
    /// allocates nothing; one that does allocates the name of each clip it adds to the plays, but
    /// where it takes the place of a play that leaves at the tick, as each play a cut leaves does,
    /// and the plays' room, past what they held before.
    fn drive(
        &self,
        at: usize,
        clips: &[Clip],
        plays: &mut Vec<Play>,
        entered: Option<f32>,
        warnings: &mut Warnings,
    ) {
        let state = &self.states[at];
        // Played once, a state's clips start afresh as it's entered, from their ends backward.
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
        // Each of its clips, with the weight its motion gives it, and its clip, if its model has it.
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
        // Where a blend's clips are, as a fraction of their lengths: where the heaviest held is, the
        // first in the blend's order of those as heavy; or where a clip starts.
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
                    // Once the state has been in for as long as its fade, its weight is its target.
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

/// The points of `points` that `x` falls between and how far it is from the first to the second,
/// from 0 to 1: the nearest at or below it and the nearest at or above, the first of those at the
/// same point; or, at a point, or past every point, the nearest alone, at 0. A value that isn't a
/// number, or points that aren't, give the first point alone.
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

/// The weight point `i` of a blend takes where its value falls `between` points, as [`between`]
/// gives it.
fn weight(between: Option<(usize, usize, f32)>, i: usize) -> f32 {
    match between {
        Some((low, _, t)) if i == low => 1.0 - t,
        Some((_, high, t)) if i == high => t,
        _ => 0.0,
    }
}

/// The length of a blend of `points` whose value falls `between` them, as [`between`] gives it, of a
/// model whose clips are `clips`: the lengths of the clips its model has of some length, weighted as
/// the blend weights them over those clips' weights alone, so that a clip its model hasn't, or of no
/// length, has no share of it, and speeds up none of the others; 0 if no clip with weight has a
/// length.
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

/// The index of the last play of `plays` of clip `name`, the newest.
fn last_of(plays: &[Play], name: &str) -> Option<usize> {
    plays.iter().rposition(|play| play.clip == name)
}

/// Whether `play` leaves its animator at this tick's step, eased to 0, as its fields stand: one
/// that eases toward no weight from none, or at once.
fn leaves(play: &Play) -> bool {
    let kept = play.target > 0.0 || (play.fade > 0.0 && play.weight > 0.0);
    !kept
}

/// Adds `play`, as a play of clip `name`, after `plays`: in the room of a play that leaves at this
/// tick's step, its clip's name's among it, if there is one, which can't show at the step, being at
/// a weight of 0, so that what's sampled is as if it had been added after them all.
fn add(plays: &mut Vec<Play>, name: &str, play: Play) {
    let leaving = plays.iter().position(leaves);
    let mut clip = leaving.map_or_else(String::new, |at| plays.remove(at).clip);
    clip.clear();
    clip.push_str(name);
    plays.push(Play { clip, ..play });
}

/// What warns of a state machine's names that it hasn't, each once a world: the world, which keeps
/// what it has warned of, and the machine's entity's ID.
struct Warnings<'w> {
    world: &'w mut World,
    id: EntityId,
}

/// What a world's state machines have named and hadn't, and been warned of: by their entities' IDs,
/// the warnings' codes and the names, so that each warns once rather than at every tick, as few as
/// a world's machines' mistakes, searched in turn. Snapshots copy it, so that a world restored from
/// one warns as the one it was taken from would.
#[derive(Clone, Debug, Default)]
struct Warned(Vec<(EntityId, &'static str, String)>);

impl Warnings<'_> {
    /// Warns, with `code`, that the machine `what`, of `name`, unless it has before.
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

/// Steps each state machine of `world` a tick, [`Tick::DT`], as [`AnimationStates`] says, in the
/// order queries find their entities: those of models' roots alone, each given an animator if it
/// has none.
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
        // Taken out of the world while it steps, and put back, as the animator's plays are.
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
    /// [`AnimationStates::set_param`] does: a number to a float, and a bool to a bool or a trigger,
    /// as `world.set_param(fox, "speed", 2.5)` and `world.set_param(fox, "pounce", true)` do.
    ///
    /// Fails with `ecs.no_entity` if `entity` isn't alive, `reflect.missing_component` if it has no
    /// state machine, and as `AnimationStates::set_param` does.
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

    /// A state of `name` that plays `clip`, as its `playback` says, as the clip goes.
    fn state(name: &str, clip: &str, playback: Playback) -> AnimationState {
        AnimationState {
            name: name.to_owned(),
            motion: StateMotion::Clip(clip.to_owned()),
            speed: 1.0,
            playback,
        }
    }

    /// A state of `name` that blends `points`, each a clip and its point, along `param`, looping.
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

    /// A transition from `from`, or from any state, to `to`, cutting, once `conditions` hold, each a
    /// parameter and its test.
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

    /// `transition`, fading over `fade` seconds, after `exit` of its state, if that's given.
    fn faded(fade: f32, exit: Option<f32>, transition: StateTransition) -> StateTransition {
        StateTransition {
            fade,
            exit_time: exit,
            ..transition
        }
    }

    /// A root of the animator's tests' model, with `machine`, and its context.
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

    /// A rest that slides, a move that blends a slide and a lift by speed, and a swing played once
    /// at a trigger from any state, which goes back to moving once it's done.
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

    /// A transition fires at the first tick that finds its conditions all hold, once its state has
    /// been entered, on each kind of test: a float above or below a value, a bool true or false, and
    /// a trigger set or not; and not while one doesn't, the other aside.
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

    /// A trigger fires a transition that tests it once, which consumes it, so that the next tick
    /// stays; one set in the state a transition from any state enters waits, as such a transition
    /// skips the state it enters, until the state is left; and tools set it through its fields.
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
        // So that the move it goes back to stays.
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
        // Swing's second of its clip, played, at the 64th tick in it; then the waiting trigger.
        run(world, 52);
        assert_eq!(machine_of(world, root).state, "Swing");
        run(world, 1);
        assert_eq!(machine_of(world, root).state, "Move");
        run(world, 1);
        let machine = machine_of(world, root);
        assert_eq!(machine.state, "Swing");
        assert_eq!(machine.param("go"), Some(Trigger(false)));
    }

    /// A transition with an exit time waits until its state has played that share of its motion, at
    /// its speed; and cross-fades as `World::play` does, the play of the state it leaves easing out
    /// as the state it enters eases in, which then plays alone, its fade done.
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
        // Half of Lift's two seconds, at twice its speed: half a second, 32 ticks after the first.
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

    /// A blend weights the two clips whose points its parameter falls between by how near it is to
    /// each, and plays each at the speed that brings it round in their weighted length, keeping
    /// them at the same fraction of their lengths; past its last point it plays that one's clip
    /// alone, and a clip that comes back joins where the others are.
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
        // A length of 1.25 seconds, Slide's one second and Lift's two weighted.
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

    /// A clip of a blend that its model hasn't, or one of no length, as a still pose, has no share
    /// of the blend's length: the clip with a length plays at its own speed, however little of the
    /// blend it weighs, and an exit time measures its length.
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
            // Half of Lift's two seconds, at its own speed: a second, 64 ticks after the first.
            run(world, 63);
            assert_eq!(machine_of(world, root).state, "Move", "{still}");
            run(world, 1);
            assert_eq!(machine_of(world, root).state, "Rest", "{still}");
        }
    }

    /// A state played once backward starts its clip at its end and plays it to its start, its first
    /// pose, which it reaches as its exit time, at the size of its speed, comes.
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
        // Slide's root goes from 0 to 2 along X over its second.
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

    /// A blend entered while the animator holds its clips at phases apart, as one fading out has
    /// drifted, takes its phase from the heaviest, which goes on where it was, the others brought
    /// to it.
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
        // Halfway round Lift's two seconds, on by a tick of the blend's 1.5.
        let phase = 0.5 + 1.0 / 64.0 / 1.5;
        let plays = plays_of(world, root);
        let phases = [plays[0].time, plays[1].time / 2.0];
        for of in phases {
            assert!((of - phase).abs() < 1e-6, "{phases:?}, not {phase}");
        }
    }

    /// A state, a parameter or a clip it names and hasn't, and a test of a parameter of another
    /// kind, are warned of once each, however many ticks find them: a state it's in that it hasn't
    /// leaves for its first, and a transition to one never fires; and code that sets a parameter it
    /// hasn't, or to another kind, or of an entity without one, is refused.
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
        // A tool puts it in a state that blends a clip its model hasn't along a parameter it
        // hasn't: what it played leaves, and the clip is left out.
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

    /// A world restored from a snapshot taken as its state machine cross-fades goes on as the world
    /// it was taken from did, tick for tick: the machine in the state it was in, as long, mid-fade,
    /// with its parameters as they were.
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

    /// The Fox's states, as the showcase's: surveying as it stands, walking and running blended by
    /// its speed, and looking about once at a trigger, from any state, before it goes on.
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

    /// The world's hash after four of Khronos's skinned Fox, each driven by its state machine
    /// through its states for eight seconds, from standing to a run and back, a blend's clips kept
    /// in step, and a look at a trigger from the move, each fox a little after the one before, is
    /// the same on every platform, as lockstep peers and replays need: a reference CI checks on
    /// every platform it runs. A difference on some platforms only is a bug in determinism, not a
    /// value to change here.
    #[test]
    fn state_machines_hash_as_their_reference_says() {
        let logged = crate::testing::logged();
        // The Fox's files without its ID file, under a root of their own: the hash reads the
        // model's name, which is then its path from the root, with `dev` and without, wherever
        // the checkout is.
        let scratch = crate::dirs::tests::Scratch::new("states-hash");
        std::fs::create_dir_all(&scratch.0).expect("a directory");
        let files = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox");
        for file in ["Fox.gltf", "Fox.bin", "Texture.png"] {
            std::fs::copy(files.join(file), scratch.0.join(file)).expect("copied");
        }
        let mut cx = Context::seeded(7).root(scratch.0.clone());
        let model = (cx.add_model("fox", scratch.0.join("Fox.gltf"))).expect("the Fox reads");
        // A development build's scan warns of the files without ID files (`asset.no_id`).
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
                // Up toward a run and back down to standing, 16 ticks apart.
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
