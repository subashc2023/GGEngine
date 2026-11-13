//! Tweens: components' fields eased from one value to another over a time.

use std::f32::consts::PI;
use std::mem;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4, libm};
use gg_core::{Error, Result};
use tracing::warn;

use super::Playback;
use crate::ecs::ComponentId;
use crate::reflect::{self, Value};
use crate::{Component, Entity, Reflect, Tick, World};

/// Fields of its entity's components eased from one value to another, each a [`Tween`], at the
/// ticks [`step_animation`](super::step_animation) runs in, in the order they're listed: each sets
/// its field as its time says, but where the field holds that already, so that a tween at rest
/// leaves its component's rows unmarked. One played once leaves the list as it ends, its field at
/// its end; one that can't set its field, as of a component its entity hasn't, or a value its
/// field refuses, as one past its bounds that a curve overshooting reaches, is warned of and left
/// out (`tween.failed`).
///
/// Scenes list them, and code and scripts start them: [`World::tween`], and scripts' `gg.tween`,
/// which give an entity its `Tweens` if it has none, and take the place of a tween of the same
/// field.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub struct Tweens(pub Vec<Tween>);

/// A field eased from `from` to `to` over `seconds`, as its `curve` and its `ease` shape the way,
/// and as its `playback` says once it reaches `to`: the field of its entity's `component` named
/// `field`, such as `Transform`'s `translation`; or one of a vector field's components, such as
/// `translation.y`, or a `Sprite`'s `color.w`, its alpha. `from` and `to` hold what the field
/// does: a number, a vector of its size, or a rotation, which turns the shortest way at a steady
/// rate; a number for a vector's component. A field of another kind refuses it.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct Tween {
    /// The component type's name, as scenes name it.
    pub component: String,
    /// The field's name, or the name and a vector's component, as `translation.y`.
    pub field: String,
    /// Where the field starts, of the kind it holds.
    pub from: TweenValue,
    /// Where the field ends, of the kind it holds.
    pub to: TweenValue,
    /// The seconds from `from` to `to`: at 0, it's at `to` at once.
    #[reflect(min = 0.0, default = 1.0)]
    pub seconds: f32,
    /// The shape of the way from `from` to `to`.
    pub curve: Curve,
    /// Which end of the way the curve shapes: its start, its end, or both.
    pub ease: Ease,
    /// Whether it plays once, leaving the list at its end, loops, or goes back and forth.
    pub playback: Playback,
    /// The seconds it has played, which each tick moves on: brought back within a round as it
    /// loops. Negative, it waits that long before it starts, leaving its field as it is.
    pub time: f32,
}

impl Tween {
    /// A tween of `component`'s `field` from `from` to `to` over `seconds`, along a straight line,
    /// played once from now; the rest is set as its fields are, as `Tween { curve: Curve::Back,
    /// ..Tween::new(..) }`.
    pub fn new(
        component: &str,
        field: &str,
        from: impl Into<TweenValue>,
        to: impl Into<TweenValue>,
        seconds: f32,
    ) -> Self {
        Self {
            component: component.to_owned(),
            field: field.to_owned(),
            from: from.into(),
            to: to.into(),
            seconds,
            curve: Curve::Linear,
            ease: Ease::InOut,
            playback: Playback::Once,
            time: 0.0,
        }
    }

    /// How far it is from `from` to `to` at its time, from 0 to 1, as its playback says, bringing
    /// its time back within a round as it loops; and whether it has ended, as one played once does.
    fn progress(&mut self) -> (f32, bool) {
        let seconds = self.seconds;
        let once = self.playback == Playback::Once;
        if seconds.is_nan() || seconds <= 0.0 {
            return (1.0, once);
        }
        match self.playback {
            Playback::Once if self.time >= seconds => (1.0, true),
            Playback::Once => (self.time / seconds, false),
            Playback::Loop => {
                if self.time >= seconds {
                    self.time %= seconds;
                }
                (self.time / seconds, false)
            }
            Playback::PingPong => {
                let round = 2.0 * seconds;
                if self.time >= round {
                    self.time %= round;
                }
                let at = self.time / seconds;
                (if at > 1.0 { 2.0 - at } else { at }, false)
            }
        }
    }

    /// Moves its time on by `dt` seconds, and sets its field of `entity`'s component as its time
    /// says; returns whether it has ended.
    fn step(&mut self, world: &mut World, entity: Entity, dt: f32) -> Result<bool> {
        self.time += dt;
        if self.time < 0.0 {
            return Ok(false);
        }
        let (at, ended) = self.progress();
        let target = Target::find(world, &self.component, &self.field)?;
        let eased = self.curve.eased(self.ease, at);
        target.set(world, entity, (&self.from, &self.to), eased)?;
        Ok(ended)
    }
}

/// What a [`Tween`] eases between: a value of the kind its field holds.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum TweenValue {
    /// For a field of an `f32` or an `f64`, as a script's numbers are, or a vector's component.
    Number(f64),
    Vec2(Vec2),
    /// For an `f32` vector or an `f64` one, as an entity's translation is: in `f64`s, so that a
    /// tween far from the origin ends where it's told.
    Vec3(DVec3),
    Vec4(Vec4),
    /// For a rotation, a unit quaternion.
    Quat(Quat),
}

impl From<f32> for TweenValue {
    fn from(x: f32) -> Self {
        Self::Number(x.into())
    }
}

impl From<f64> for TweenValue {
    fn from(x: f64) -> Self {
        Self::Number(x)
    }
}

impl From<Vec2> for TweenValue {
    fn from(v: Vec2) -> Self {
        Self::Vec2(v)
    }
}

impl From<Vec3> for TweenValue {
    fn from(v: Vec3) -> Self {
        Self::Vec3(v.as_dvec3())
    }
}

impl From<DVec3> for TweenValue {
    fn from(v: DVec3) -> Self {
        Self::Vec3(v)
    }
}

impl From<Vec4> for TweenValue {
    fn from(v: Vec4) -> Self {
        Self::Vec4(v)
    }
}

impl From<Quat> for TweenValue {
    fn from(q: Quat) -> Self {
        Self::Quat(q)
    }
}

impl TweenValue {
    /// `value`, of a field a tween can ease, as a tween's: `None` for one of another kind.
    pub fn of(value: &Value) -> Option<Self> {
        Some(match *value {
            Value::F32(x) => Self::Number(x.into()),
            Value::F64(x) => Self::Number(x),
            Value::Vec2(v) => Self::Vec2(v),
            Value::Vec3(v) => Self::Vec3(v.as_dvec3()),
            Value::DVec3(v) => Self::Vec3(v),
            Value::Vec4(v) => Self::Vec4(v),
            Value::Quat(q) => Self::Quat(q),
            _ => return None,
        })
    }

    /// The name of its variant.
    fn kind(&self) -> &'static str {
        match self {
            Self::Number(_) => "Number",
            Self::Vec2(_) => "Vec2",
            Self::Vec3(_) => "Vec3",
            Self::Vec4(_) => "Vec4",
            Self::Quat(_) => "Quat",
        }
    }
}

/// The shape of a [`Tween`]'s way from its start to its end, which its [`Ease`] says which end of
/// is shaped: the curves Robert Penner named, as most tools do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Curve {
    /// At a steady rate, whatever its ease.
    #[default]
    Linear,
    /// A quarter of a sine wave's: gently.
    Sine,
    /// As the square of the time.
    Quad,
    /// As its cube: sharper.
    Cubic,
    /// Going back a little past its start before it goes, or on a little past its end before it
    /// comes back to it.
    Back,
    /// Springing about its start or its end, past both.
    Elastic,
    /// Bouncing on its start or its end, as a ball dropped does.
    Bounce,
}

/// Which end of a [`Tween`]'s way its [`Curve`] shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Ease {
    /// Its start: it sets off slowly.
    In,
    /// Its end: it comes to it slowly.
    Out,
    /// Both, the first half as `In`, the second as `Out`.
    #[default]
    InOut,
}

impl Curve {
    /// How far along a tween along this curve, eased at `ease`, is at `at`, from 0 at its start to
    /// 1 at its end: 0 at 0 and before, 1 at 1 and past, and between as the curve says, which a
    /// curve that overshoots, as `Back`, `Elastic` and `Bounce` do, takes past 0 or 1.
    pub fn eased(self, ease: Ease, at: f32) -> f32 {
        if at.is_nan() || at <= 0.0 {
            return 0.0;
        }
        if at >= 1.0 {
            return 1.0;
        }
        match ease {
            Ease::In => self.ease_in(at),
            Ease::Out => 1.0 - self.ease_in(1.0 - at),
            Ease::InOut if at < 0.5 => self.ease_in(2.0 * at) / 2.0,
            Ease::InOut => 1.0 - self.ease_in(2.0 - 2.0 * at) / 2.0,
        }
    }

    /// The curve eased in, at `at`, from 0 to 1: as easings.net gives each.
    fn ease_in(self, at: f32) -> f32 {
        match self {
            Self::Linear => at,
            Self::Sine => 1.0 - libm::cosf(at * PI / 2.0),
            Self::Quad => at * at,
            Self::Cubic => at * at * at,
            Self::Back => {
                const C1: f32 = 1.70158;
                (C1 + 1.0) * at * at * at - C1 * at * at
            }
            Self::Elastic => {
                let wave = libm::sinf((10.0 * at - 10.75) * (2.0 * PI / 3.0));
                -libm::exp2f(10.0 * at - 10.0) * wave
            }
            Self::Bounce => 1.0 - bounce_out(1.0 - at),
        }
    }
}

/// A ball's bounces coming to rest at 1, at `at`, from 0 to 1.
fn bounce_out(at: f32) -> f32 {
    const N: f32 = 7.5625;
    const D: f32 = 2.75;
    let (from, up) = if at < 1.0 / D {
        (0.0, 0.0)
    } else if at < 2.0 / D {
        (1.5 / D, 0.75)
    } else if at < 2.5 / D {
        (2.25 / D, 0.9375)
    } else {
        (2.625 / D, 0.984_375)
    };
    let at = at - from;
    N * at * at + up
}

/// Where a tween's field is: its component's type, the field's index in it, and the component of
/// it, if the tween eases one of a vector's.
struct Target {
    id: ComponentId,
    index: u32,
    part: Option<usize>,
}

impl Target {
    /// Finds the field `field` of component type `component` in `world`, as [`Tween`] names it:
    /// without allocating, but to fail.
    ///
    /// Fails with `reflect.unknown_type` if there's no such component type, and `reflect.no_field`
    /// if it has no such field, or the field no such component.
    fn find(world: &World, component: &str, field: &str) -> Result<Self> {
        let Some(id) = world.component_named(component) else {
            return Err(Error::new(
                "reflect.unknown_type",
                format!("there's no component type `{component}`"),
            ));
        };
        let (name, part) = match field.split_once('.') {
            Some((name, part)) => (name, Some(part)),
            None => (field, None),
        };
        let fields = world.fields_of(id);
        let Some(index) = fields.iter().position(|of| of.name == name) else {
            let names = reflect::list(fields.iter().map(|of| &of.name));
            return Err(Error::new(
                "reflect.no_field",
                format!("`{component}` has no field `{name}`; its fields are {names}"),
            ));
        };
        let part = match part {
            None => None,
            Some(part) => {
                let size = match fields[index].ty {
                    reflect::Type::Vec2 => 2,
                    reflect::Type::Vec3 | reflect::Type::DVec3 => 3,
                    reflect::Type::Vec4 => 4,
                    _ => 0,
                };
                let at = reflect::COMPONENTS[..size]
                    .iter()
                    .position(|&of| of == part);
                Some(at.ok_or_else(|| {
                    Error::new(
                        "reflect.no_field",
                        format!(
                            "`{component}.{name}` has no component `{part}`: a tween eases one of \
                             a vector's, `x` to `w`, as many as it has"
                        ),
                    )
                })?)
            }
        };
        let index = u32::try_from(index).expect("a type's fields number fewer than u32's");
        Ok(Self { id, index, part })
    }

    /// Sets the field of `entity`'s component to what's `eased` of the way `between` its ends, a
    /// tween's, but where it holds that already: without allocating, but to fail.
    ///
    /// Fails as [`World::field_to_set`] does, as for an entity that hasn't the component, or a
    /// value outside the field's bounds; and with `reflect.wrong_kind` for ends of another kind
    /// than the field holds.
    fn set(
        &self,
        world: &mut World,
        entity: Entity,
        between: (&TweenValue, &TweenValue),
        eased: f32,
    ) -> Result<()> {
        let held = world.field_at(entity, self.id, self.index)?.to_value();
        let value = eased_value(&held, self.part, between, eased)?;
        if value != held {
            let field = world.field_to_set(entity, self.id, self.index, Some(&value))?;
            field.set_value(&value)?;
        }
        Ok(())
    }
}

/// The value a field that holds `held` takes `eased` of the way `between` a tween's ends, `part`
/// of it alone if it's a vector's component that the tween eases: each component `eased` of the way
/// along a straight line, which is at either end exactly at 0 and 1, and a rotation along the
/// shortest arc, at a steady rate.
///
/// Fails with `reflect.wrong_kind` for ends of another kind than the field holds.
fn eased_value(
    held: &Value,
    part: Option<usize>,
    (from, to): (&TweenValue, &TweenValue),
    eased: f32,
) -> Result<Value> {
    let (t, t64) = (eased, f64::from(eased));
    Ok(match (*from, *to, held, part) {
        (TweenValue::Number(a), TweenValue::Number(b), _, _) => {
            let x = a * (1.0 - t64) + b * t64;
            match (held, part) {
                (Value::F32(_), None) => Value::F32(x as f32),
                (Value::F64(_), None) => Value::F64(x),
                (&Value::Vec2(mut v), Some(i)) => {
                    v[i] = x as f32;
                    Value::Vec2(v)
                }
                (&Value::Vec3(mut v), Some(i)) => {
                    v[i] = x as f32;
                    Value::Vec3(v)
                }
                (&Value::Vec4(mut v), Some(i)) => {
                    v[i] = x as f32;
                    Value::Vec4(v)
                }
                (&Value::DVec3(mut v), Some(i)) => {
                    v[i] = x;
                    Value::DVec3(v)
                }
                _ => return Err(unlike(held, part, from, to)),
            }
        }
        (TweenValue::Vec2(a), TweenValue::Vec2(b), Value::Vec2(_), None) => {
            Value::Vec2(a * (1.0 - t) + b * t)
        }
        (TweenValue::Vec3(a), TweenValue::Vec3(b), Value::Vec3(_), None) => {
            Value::Vec3((a * (1.0 - t64) + b * t64).as_vec3())
        }
        (TweenValue::Vec3(a), TweenValue::Vec3(b), Value::DVec3(_), None) => {
            Value::DVec3(a * (1.0 - t64) + b * t64)
        }
        (TweenValue::Vec4(a), TweenValue::Vec4(b), Value::Vec4(_), None) => {
            Value::Vec4(a * (1.0 - t) + b * t)
        }
        (TweenValue::Quat(a), TweenValue::Quat(b), Value::Quat(_), None) => Value::Quat(match t {
            0.0 => a,
            1.0 => b,
            t => a.slerp(b, t).normalize(),
        }),
        _ => return Err(unlike(held, part, from, to)),
    })
}

/// The error that a tween from `from` to `to` can't ease a field that holds `held`, `part` of it
/// if it's a vector's component that the tween eases.
fn unlike(held: &Value, part: Option<usize>, from: &TweenValue, to: &TweenValue) -> Error {
    let holds = match part {
        Some(_) => "a number, one of a vector's components",
        None => reflect::kind(held),
    };
    Error::new(
        "reflect.wrong_kind",
        format!(
            "the tween goes from a {} to a {}, where the field holds {holds}: a tween's ends are \
             a Number for a number, a Vec2, a Vec3 or a Vec4 for a vector of its size, and a Quat \
             for a rotation",
            from.kind(),
            to.kind()
        ),
    )
}

impl World {
    /// Starts `tween` on `entity`, in its [`Tweens`]: in place of one of the same field, if it has
    /// one; at the end of the list, if not; and in a `Tweens` given it, if it has none.
    ///
    /// Fails with `ecs.no_entity` if `entity` isn't alive; as the tween's first step would, as
    /// [`Tweens`] says: with `reflect.unknown_type`, `reflect.missing_component` or
    /// `reflect.no_field` for a field it hasn't, and `reflect.wrong_kind` for ends of another kind
    /// than the field holds; with `reflect.out_of_range` for seconds that are negative or aren't a
    /// number, as a scene's are refused; and starting nothing.
    pub fn tween(&mut self, entity: Entity, tween: Tween) -> Result<()> {
        if tween.seconds.is_nan() || tween.seconds < 0.0 {
            return Err(Error::new(
                "reflect.out_of_range",
                format!("a tween's seconds are 0 or more, not {}", tween.seconds),
            ));
        }
        let target = Target::find(self, &tween.component, &tween.field)?;
        let held = self.field_at(entity, target.id, target.index)?.to_value();
        eased_value(&held, target.part, (&tween.from, &tween.to), 0.0)?;
        let Some(Tweens(tweens)) = self.get_mut::<Tweens>(entity) else {
            return self.insert(entity, Tweens(vec![tween]));
        };
        let same = |of: &Tween| of.component == tween.component && of.field == tween.field;
        match tweens.iter_mut().find(|of| same(of)) {
            Some(of) => *of = tween,
            None => tweens.push(tween),
        }
        Ok(())
    }
}

/// Moves each tween's time on a tick, and sets its field as its time says, in the order queries
/// find their entities, and each entity's tweens in their order; leaving out those that end, and
/// those that fail, warned of.
pub(super) fn step(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let mut entities = world.take_list();
    entities.extend(world.query::<(Entity, &Tweens)>().map(|(entity, _)| entity));
    for &entity in &entities {
        // Taken out of the world while they set its fields, and put back.
        let mut tweens = match world.get_mut::<Tweens>(entity) {
            Some(Tweens(tweens)) => mem::take(tweens),
            None => continue,
        };
        tweens.retain_mut(|tween| match tween.step(world, entity, dt) {
            Ok(ended) => !ended,
            Err(error) => {
                let id = world.id(entity).expect("listed alive");
                let (component, field) = (&tween.component, &tween.field);
                warn!(
                    code = "tween.failed",
                    entity = %id,
                    component,
                    field,
                    "Entity {id}'s tween of `{component}.{field}` stops: {error:#}"
                );
                false
            }
        });
        if let Some(Tweens(left)) = world.get_mut::<Tweens>(entity) {
            *left = tweens;
        }
    }
    world.keep_list(entities);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Sprite, Transform};

    /// Steps `world`'s tweens `ticks` times.
    fn run(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            step(world);
        }
    }

    fn transform(world: &World, entity: Entity) -> Transform {
        *world.get::<Transform>(entity).expect("placed")
    }

    /// A tween played once moves its field steadily from its start to its end, exactly, over its
    /// seconds, and then leaves its entity's list, the field at its end; one waiting leaves its
    /// field as it is until its time comes.
    #[test]
    fn a_tween_moves_its_field_to_its_end_and_leaves() {
        let mut world = World::default();
        let entity = world.spawn(Transform::IDENTITY);
        let tween = Tween::new(
            "Transform",
            "translation",
            Vec3::ZERO,
            Vec3::new(4.0, 2.0, 0.0),
            1.0,
        );
        world.tween(entity, tween).expect("a field of a vector");
        run(&mut world, 32);
        let half = transform(&world, entity).translation;
        assert_eq!(half, DVec3::new(2.0, 1.0, 0.0));
        run(&mut world, 32);
        assert_eq!(
            transform(&world, entity).translation,
            DVec3::new(4.0, 2.0, 0.0)
        );
        assert_eq!(world.get::<Tweens>(entity), Some(&Tweens(Vec::new())));
        let waiting = Tween {
            time: -0.5,
            ..Tween::new("Transform", "translation.y", 0.0, 1.0, 0.5)
        };
        world.tween(entity, waiting).expect("a vector's component");
        run(&mut world, 31);
        assert_eq!(transform(&world, entity).translation.y, 2.0, "waiting");
        run(&mut world, 17);
        let moved = transform(&world, entity).translation;
        assert_eq!(moved, DVec3::new(4.0, 0.5, 0.0), "the rest left as it was");
    }

    /// A tween of a translation 1,000 km out moves it in `f64`: halfway between two points 20 cm
    /// apart, to the micrometer, where `f32` would be 6 mm off.
    #[test]
    fn a_far_translation_tweens_in_f64() {
        let mut world = World::default();
        let entity = world.spawn(Transform::IDENTITY);
        let [from, to] = [0.3, 0.5].map(|x| DVec3::new(1.0e6 + x, -1.0e6, 0.0));
        let tween = Tween::new("Transform", "translation", from, to, 1.0);
        world.tween(entity, tween).expect("a field of a vector");
        run(&mut world, 32);
        let half = transform(&world, entity).translation;
        let want = DVec3::new(1.0e6 + 0.4, -1.0e6, 0.0);
        assert!(half.abs_diff_eq(want, 1e-6), "{half}");
    }

    /// A loop starts again at its end, a ping-pong comes back, and both stay within a round; a
    /// rotation turns the shortest way, and a sprite's alpha fades alone.
    #[test]
    fn loops_ping_pongs_rotations_and_components_go_as_they_say() {
        let mut world = World::default();
        let entity = world.spawn((Transform::IDENTITY, Sprite::default()));
        let turn = Quat::from_rotation_y(PI / 2.0);
        let tweens = [
            Tween {
                playback: Playback::Loop,
                ..Tween::new("Transform", "scale.x", 1.0, 2.0, 0.5)
            },
            Tween {
                playback: Playback::PingPong,
                ..Tween::new("Sprite", "color.w", 1.0, 0.0, 0.5)
            },
            Tween::new("Transform", "rotation", Quat::IDENTITY, turn, 1.0),
        ];
        for tween in tweens {
            world.tween(entity, tween).expect("fields of their kinds");
        }
        run(&mut world, 16);
        assert_eq!(transform(&world, entity).scale.x, 1.5);
        let alpha = |world: &World| world.get::<Sprite>(entity).expect("a sprite").color;
        assert_eq!(alpha(&world), Vec4::new(1.0, 1.0, 1.0, 0.5));
        // A quarter of the way, a quarter of the turn.
        let quarter = Quat::from_rotation_y(PI / 8.0);
        assert!(transform(&world, entity).rotation.angle_between(quarter) < 1e-3);
        run(&mut world, 24);
        // 40 ticks, 0.625 seconds: the loop a quarter into its second round, the ping-pong a
        // quarter of the way back.
        assert_eq!(transform(&world, entity).scale.x, 1.25);
        assert_eq!(alpha(&world).w, 0.25);
        run(&mut world, 64 * 10);
        let tweens = &world.get::<Tweens>(entity).expect("tweens").0;
        assert_eq!(tweens.len(), 2, "the rotation's ended");
        assert!(tweens.iter().all(|tween| (0.0..1.0).contains(&tween.time)));
        assert_eq!(
            transform(&world, entity).rotation,
            turn,
            "at its end exactly"
        );
    }

    /// Each curve starts at 0 and ends at 1 exactly, eased at either end or both; `Back` goes back
    /// past its start, and the rest within them, `Bounce` and `Elastic` but at their far ends.
    #[test]
    fn curves_start_at_0_end_at_1_and_overshoot_where_they_should() {
        let curves = [
            Curve::Linear,
            Curve::Sine,
            Curve::Quad,
            Curve::Cubic,
            Curve::Back,
            Curve::Elastic,
            Curve::Bounce,
        ];
        for curve in curves {
            for ease in [Ease::In, Ease::Out, Ease::InOut] {
                assert_eq!(curve.eased(ease, 0.0), 0.0, "{curve:?} {ease:?}");
                assert_eq!(curve.eased(ease, 1.0), 1.0, "{curve:?} {ease:?}");
                assert_eq!(curve.eased(ease, f32::NAN), 0.0, "{curve:?} {ease:?}");
                let middle = curve.eased(ease, 0.5);
                assert!(
                    (-0.5..=1.5).contains(&middle),
                    "{curve:?} {ease:?}: {middle}"
                );
            }
        }
        assert_eq!(Curve::Linear.eased(Ease::In, 0.25), 0.25);
        assert_eq!(Curve::Quad.eased(Ease::In, 0.5), 0.25);
        assert_eq!(Curve::Quad.eased(Ease::Out, 0.5), 0.75);
        assert_eq!(Curve::Cubic.eased(Ease::InOut, 0.25), 0.0625);
        assert!(Curve::Back.eased(Ease::In, 0.2) < 0.0);
        assert!(Curve::Back.eased(Ease::Out, 0.8) > 1.0);
        assert!(Curve::Elastic.eased(Ease::Out, 0.1) > 1.0);
        let bounced = (1..100).map(|i| Curve::Bounce.eased(Ease::Out, i as f32 / 100.0));
        assert!(bounced.clone().all(|at| (0.0..=1.0).contains(&at)));
        // It lands three times before it rests.
        let bounced: Vec<f32> = bounced.collect();
        let landings = bounced.windows(3).filter(|w| w[1] > w[0] && w[1] > w[2]);
        assert_eq!(landings.count(), 3);
    }

    /// A tween of a field it can't set is refused as it starts, and one that comes to fail as it
    /// steps, as of a component its entity loses, leaves the list, warned of, the others going on;
    /// and one of a field already where it would set it leaves the component's rows unmarked.
    #[test]
    fn tweens_that_cant_set_their_fields_are_refused_or_leave() {
        let mut world = World::default();
        let entity = world.spawn((Transform::IDENTITY, Sprite::default()));
        let refused = |tween: Tween, code: &str| {
            let mut world = World::default();
            let entity = world.spawn((Transform::IDENTITY, Sprite::default()));
            let error = world.tween(entity, tween).expect_err("refused");
            assert_eq!(error.code(), code, "{error:#}");
        };
        refused(
            Tween::new("Nothing", "x", 0.0, 1.0, 1.0),
            "reflect.unknown_type",
        );
        refused(
            Tween::new("Transform", "size", 0.0, 1.0, 1.0),
            "reflect.no_field",
        );
        refused(
            Tween::new("Transform", "scale.q", 0.0, 1.0, 1.0),
            "reflect.no_field",
        );
        refused(
            Tween::new("Transform", "rotation.x", 0.0, 1.0, 1.0),
            "reflect.no_field",
        );
        refused(
            Tween::new("Transform", "scale", 0.0, 1.0, 1.0),
            "reflect.wrong_kind",
        );
        refused(
            Tween::new("Sprite", "sampler", 0.0, 1.0, 1.0),
            "reflect.wrong_kind",
        );
        let starts = [
            Tween::new("Sprite", "color.x", 1.0, 0.0, 1.0),
            Tween::new("Transform", "translation.x", 0.0, 1.0, 1.0),
        ];
        for tween in starts {
            world.tween(entity, tween).expect("fields it has");
        }
        run(&mut world, 4);
        world.remove::<Sprite>(entity).expect("alive");
        run(&mut world, 4);
        let tweens = &world.get::<Tweens>(entity).expect("tweens").0;
        assert_eq!(tweens.len(), 1);
        assert_eq!(tweens[0].field, "translation.x");
        assert_eq!(transform(&world, entity).translation.x, 0.125);
        // A tween of the same field takes the other's place.
        let again = Tween::new("Transform", "translation.x", 5.0, 5.0, 1.0);
        world.tween(entity, again).expect("a field it has");
        assert_eq!(world.get::<Tweens>(entity).expect("tweens").0.len(), 1);
        run(&mut world, 1);
        let marked = |world: &World| {
            let id = world.component::<Transform>().expect("registered");
            let tables = world.tables().iter();
            let mut rows = tables.filter_map(|table| {
                let row = table.entities().iter().position(|&of| of == entity)?;
                Some(table.changed(id).expect("it has one").row(row))
            });
            rows.next().expect("alive")
        };
        let at = marked(&world);
        run(&mut world, 4);
        assert_eq!(marked(&world), at, "at rest");
    }
}
