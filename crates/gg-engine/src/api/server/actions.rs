//! The layers' actions as the tool API lists them, and an `input` action handed as the events of a
//! binding that gives its value, as a player's keys or stick would.

use gg_api::{ActionKind, ActionList, ActionValue, Failure, InputEvent};
use gg_core::math::Vec2;

use crate::layer::Stack;
use crate::{Binding, MouseButton, PadButton, Source, Stick, Toward, Trigger};

/// Each gamepad button as the engine and the protocol name it.
const PAD_BUTTONS: [(PadButton, gg_api::PadButton); 17] = [
    (PadButton::South, gg_api::PadButton::South),
    (PadButton::East, gg_api::PadButton::East),
    (PadButton::North, gg_api::PadButton::North),
    (PadButton::West, gg_api::PadButton::West),
    (PadButton::LeftShoulder, gg_api::PadButton::LeftShoulder),
    (PadButton::RightShoulder, gg_api::PadButton::RightShoulder),
    (PadButton::LeftTrigger, gg_api::PadButton::LeftTrigger),
    (PadButton::RightTrigger, gg_api::PadButton::RightTrigger),
    (PadButton::Select, gg_api::PadButton::Select),
    (PadButton::Start, gg_api::PadButton::Start),
    (PadButton::Mode, gg_api::PadButton::Mode),
    (PadButton::LeftThumb, gg_api::PadButton::LeftThumb),
    (PadButton::RightThumb, gg_api::PadButton::RightThumb),
    (PadButton::DPadUp, gg_api::PadButton::DPadUp),
    (PadButton::DPadDown, gg_api::PadButton::DPadDown),
    (PadButton::DPadLeft, gg_api::PadButton::DPadLeft),
    (PadButton::DPadRight, gg_api::PadButton::DPadRight),
];

/// What `actions` answers: the layers' actions, from the top layer down.
pub(super) fn list(stack: &Stack) -> ActionList {
    let mut actions: Vec<gg_api::Action> = Vec::new();
    for (layer, name, binding) in stack.actions() {
        let sources = binding.sources().map(api_source).collect();
        let listed =
            (actions.iter_mut()).find(|action| action.layer == layer && action.name == name);
        match listed {
            Some(action) => action.bindings.push(sources),
            None => actions.push(gg_api::Action {
                name,
                layer: layer.to_owned(),
                kind: kind(binding),
                bindings: vec![sources],
            }),
        }
    }
    ActionList { actions }
}

/// The events that set the action `name` to `value`, of the top layer that binds an action so
/// named: through the first binding that gives it, with the others let go of, so the action is
/// at `value` whatever the client set before. Releases come first, then presses.
///
/// Fails with:
/// - `api.no_action`: no layer binds an action so named;
/// - `api.bad_params`: a value of another kind than the action's, or that none of its bindings
///   gives, as a value between -1 and 1 that keys alone can't.
pub(super) fn events(
    stack: &Stack,
    name: &str,
    value: Option<ActionValue>,
) -> Result<Vec<InputEvent>, Failure> {
    let bound = stack.actions();
    let Some(&(layer, ..)) = bound.iter().find(|(_, action, _)| action == name) else {
        let mut names: Vec<&str> = Vec::new();
        for (_, action, _) in &bound {
            if !names.contains(&action.as_str()) {
                names.push(action);
            }
        }
        let have = match names.as_slice() {
            [] => "none: a layer lists its `Actions` through `Layer::actions`".to_owned(),
            names => names.join(", "),
        };
        let message = format!("no layer binds an action `{name}`; they bind {have}");
        return Err(Failure::new("api.no_action", message));
    };
    let bindings = (bound.iter())
        .filter(|&&(at, ref action, _)| at == layer && action == name)
        .map(|&(.., binding)| binding);
    let bindings: Vec<Binding> = bindings.collect();
    let in_range = |x: f32| (-1.0..=1.0).contains(&x);
    let wanted = match (kind(bindings[0]), value) {
        (ActionKind::Button, None) => Some(Vec2::X),
        (ActionKind::Button, Some(ActionValue::Axis(x))) if x == 0.0 || x == 1.0 => {
            Some(Vec2::new(x, 0.0))
        }
        (ActionKind::Axis, Some(ActionValue::Axis(x))) if in_range(x) => Some(Vec2::new(x, 0.0)),
        (ActionKind::Axis2, Some(ActionValue::Axis2([x, y]))) if in_range(x) && in_range(y) => {
            Some(Vec2::new(x, y))
        }
        _ => None,
    };
    let Some(wanted) = wanted else {
        let what = match kind(bindings[0]) {
            ActionKind::Button => "a button: its `value` is 1, pressed, the default, or 0",
            ActionKind::Axis => "an axis: its `value` is a number from -1 to 1",
            ActionKind::Axis2 => "a pair of axes: its `value` is `[x, y]`, each from -1 to 1",
        };
        let message = format!("`{name}` is {what}");
        return Err(Failure::new("api.bad_params", message));
    };
    let Some(chosen) = bindings.iter().position(|&b| levels(b, wanted).is_some()) else {
        let message = format!(
            "`{name}` is bound to keys and buttons alone, which give -1, 0 and 1, not {value}: \
             a value between needs a stick or trigger bound to it",
            value = match value {
                Some(ActionValue::Axis2([x, y])) => format!("[{x}, {y}]"),
                Some(ActionValue::Axis(x)) => x.to_string(),
                None => "1".to_owned(),
            }
        );
        return Err(Failure::new("api.bad_params", message));
    };
    // A source two bindings share goes as far as either takes it.
    let mut held: Vec<(Source, f32)> = Vec::new();
    for (at, &binding) in bindings.iter().enumerate() {
        let value = if at == chosen { wanted } else { Vec2::ZERO };
        let levels = levels(binding, value).expect("any binding gives rest");
        for (source, level) in levels {
            match held.iter_mut().find(|(held, _)| *held == source) {
                Some((_, most)) => *most = most.max(level),
                None => held.push((source, level)),
            }
        }
    }
    Ok(handed(&held))
}

/// Fails as [`events`] does at the first action among `events` the layers can't set so.
pub(super) fn check<'a>(
    stack: &Stack,
    events: impl IntoIterator<Item = &'a InputEvent>,
) -> Result<(), Failure> {
    for given in events {
        if let InputEvent::Action { name, value } = given {
            self::events(stack, name, *value)?;
        }
    }
    Ok(())
}

/// How far each of `binding`'s sources goes for it to give `value`, from 0 at rest. `None` where
/// a key or button would be held part of the way.
fn levels(binding: Binding, value: Vec2) -> Option<Vec<(Source, f32)>> {
    let way = |x: f32| [(-x).max(0.0), x.max(0.0)];
    let [left, right] = way(value.x);
    let [down, up] = way(value.y);
    let levels = match binding {
        Binding::Button(source) => vec![(source, value.x)],
        Binding::Axis { negative, positive } => vec![(negative, left), (positive, right)],
        Binding::Axis2 { x, y } => vec![(x[0], left), (x[1], right), (y[0], down), (y[1], up)],
    };
    let analog = |source| matches!(source, Source::PadStick(..) | Source::PadTrigger(_));
    let gives = |&(source, level): &(Source, f32)| analog(source) || level == 0.0 || level == 1.0;
    levels.iter().all(gives).then_some(levels)
}

/// The events that take each source to its level, releases first, on pad 0. A stick goes at most
/// 1 from rest, as keys held diagonally do once the game's `Actions` reads them.
fn handed(levels: &[(Source, f32)]) -> Vec<InputEvent> {
    let mut releases = Vec::new();
    let mut presses = Vec::new();
    let mut sticks: Vec<(Stick, Vec2)> = Vec::new();
    for &(source, level) in levels {
        let held = level > 0.0;
        let event = match source {
            Source::Key(key) => InputEvent::Key {
                key: key.name().to_owned(),
                pressed: held,
                repeat: false,
            },
            Source::Button(button) => InputEvent::Button {
                button: api_button(button),
                pressed: held,
            },
            Source::PadButton(button) => InputEvent::PadButton {
                pad: 0,
                button: api_pad_button(button),
                pressed: held,
            },
            Source::PadTrigger(trigger) => InputEvent::PadTrigger {
                pad: 0,
                trigger: api_trigger(trigger),
                value: level,
            },
            Source::PadStick(stick, toward) => {
                let along = match toward {
                    Toward::Left => Vec2::NEG_X,
                    Toward::Right => Vec2::X,
                    Toward::Down => Vec2::NEG_Y,
                    Toward::Up => Vec2::Y,
                };
                match sticks.iter_mut().find(|(at, _)| *at == stick) {
                    Some((_, at)) => *at += along * level,
                    None => sticks.push((stick, along * level)),
                }
                continue;
            }
        };
        if held { &mut presses } else { &mut releases }.push(event);
    }
    for (stick, at) in sticks {
        let at = at.clamp_length_max(1.0);
        let event = InputEvent::PadStick {
            pad: 0,
            stick: api_stick(stick),
            value: at.to_array(),
        };
        if at == Vec2::ZERO {
            &mut releases
        } else {
            &mut presses
        }
        .push(event);
    }
    releases.append(&mut presses);
    releases
}

/// The kind an action of `binding` is.
fn kind(binding: Binding) -> ActionKind {
    match binding {
        Binding::Button(_) => ActionKind::Button,
        Binding::Axis { .. } => ActionKind::Axis,
        Binding::Axis2 { .. } => ActionKind::Axis2,
    }
}

fn api_source(source: Source) -> gg_api::Source {
    match source {
        Source::Key(key) => gg_api::Source::Key(key.name().to_owned()),
        Source::Button(button) => gg_api::Source::Button(api_button(button)),
        Source::PadButton(button) => gg_api::Source::PadButton(api_pad_button(button)),
        Source::PadStick(stick, toward) => {
            let toward = match toward {
                Toward::Left => gg_api::Toward::Left,
                Toward::Right => gg_api::Toward::Right,
                Toward::Down => gg_api::Toward::Down,
                Toward::Up => gg_api::Toward::Up,
            };
            gg_api::Source::PadStick(api_stick(stick), toward)
        }
        Source::PadTrigger(trigger) => gg_api::Source::PadTrigger(api_trigger(trigger)),
    }
}

fn api_button(button: MouseButton) -> gg_api::Button {
    match button {
        MouseButton::Left => gg_api::Button::Left,
        MouseButton::Right => gg_api::Button::Right,
        MouseButton::Middle => gg_api::Button::Middle,
        MouseButton::Back => gg_api::Button::Back,
        MouseButton::Forward => gg_api::Button::Forward,
        MouseButton::Other(number) => gg_api::Button::Other(number),
    }
}

fn api_pad_button(button: PadButton) -> gg_api::PadButton {
    let found = PAD_BUTTONS.iter().find(|(engine, _)| *engine == button);
    found.expect("every button is listed").1
}

/// The engine's button `button` names.
pub(super) fn pad_button(button: gg_api::PadButton) -> PadButton {
    let found = PAD_BUTTONS.iter().find(|(_, api)| *api == button);
    found.expect("every button is listed").0
}

fn api_stick(stick: Stick) -> gg_api::Stick {
    match stick {
        Stick::Left => gg_api::Stick::Left,
        Stick::Right => gg_api::Stick::Right,
    }
}

/// The engine's stick `stick` names.
pub(super) fn stick(stick: gg_api::Stick) -> Stick {
    match stick {
        gg_api::Stick::Left => Stick::Left,
        gg_api::Stick::Right => Stick::Right,
    }
}

fn api_trigger(trigger: Trigger) -> gg_api::Trigger {
    match trigger {
        Trigger::Left => gg_api::Trigger::Left,
        Trigger::Right => gg_api::Trigger::Right,
    }
}

/// The engine's trigger `trigger` names.
pub(super) fn trigger(trigger: gg_api::Trigger) -> Trigger {
    match trigger {
        gg_api::Trigger::Left => Trigger::Left,
        gg_api::Trigger::Right => Trigger::Right,
    }
}
