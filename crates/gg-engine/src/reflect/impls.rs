//! Reflection of the types fields hold: `bool`, the integers of fixed sizes, `f32`, `f64` and
//! `String`; glam's vectors and quaternion; entities, textures, models and fonts; and options,
//! lists and arrays of any reflected type. `usize` and `isize` aren't, since their size differs by
//! platform, and a value saved or sent must mean the same everywhere.

use std::any::Any;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};

use super::{
    Angle, Color, Digest, Error, Ranged, Reflect, Result, Type, Types, Value, within, wrong_kind,
};
use crate::{Entity, Font, Model, Prefab, Texture};

/// `set_value` as all of them have it, and no parts.
macro_rules! whole {
    () => {
        fn set_value(&mut self, value: &Value) -> Result<()> {
            *self = self.with_value(value)?;
            Ok(())
        }

        fn part(&self, _: u32) -> Option<&dyn Reflect> {
            None
        }

        fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
            None
        }
    };
}

macro_rules! integers {
    ($($T:ident $signed:literal $Variant:ident $digest:ident,)*) => {$(
        impl Reflect for $T {
            fn describe(_: &mut Types) -> Result<Type> {
                Ok(Type::Int { bits: $T::BITS as u8, signed: $signed })
            }

            fn to_value(&self) -> Value {
                Value::$Variant((*self).into())
            }

            fn from_value(value: &Value) -> Result<Self> {
                let fits = match *value {
                    Value::Int(n) => Self::try_from(n).map_err(|_| n.to_string()),
                    Value::UInt(n) => Self::try_from(n).map_err(|_| n.to_string()),
                    ref other => return Err(wrong_kind("Int or UInt", other)),
                };
                fits.map_err(|n| {
                    Error::new(
                        "reflect.out_of_range",
                        format!(
                            "{n} doesn't fit {}, from {} to {}",
                            stringify!($T),
                            $T::MIN,
                            $T::MAX
                        ),
                    )
                })
            }

            fn with_value(&self, value: &Value) -> Result<Self> {
                Self::from_value(value)
            }

            fn digest(&self, to: &mut Digest<'_>) {
                to.$digest((*self).into());
            }

            fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}

            whole!();
        }

        impl Ranged for $T {}
    )*};
}

integers! {
    i8 true Int int, i16 true Int int, i32 true Int int, i64 true Int int,
    u8 false UInt word, u16 false UInt word, u32 false UInt word, u64 false UInt word,
}

/// Types whose values are one variant of [`Value`], taken as they are: such as `bool`, by
/// `Value::Bool`, which reflection describes as `Type::Bool`.
macro_rules! plain {
    ($($T:ty => $Variant:ident $digest:ident,)*) => {$(
        impl Reflect for $T {
            fn describe(_: &mut Types) -> Result<Type> {
                Ok(Type::$Variant)
            }

            fn to_value(&self) -> Value {
                Value::$Variant(*self)
            }

            fn from_value(value: &Value) -> Result<Self> {
                match value {
                    Value::$Variant(value) => Ok(*value),
                    other => Err(wrong_kind(stringify!($Variant), other)),
                }
            }

            fn with_value(&self, value: &Value) -> Result<Self> {
                Self::from_value(value)
            }

            fn digest(&self, to: &mut Digest<'_>) {
                to.$digest(*self);
            }

            /// Of these, an entity alone holds one: itself.
            fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity)) {
                let any: &mut dyn Any = self;
                if let Some(entity) = any.downcast_mut::<Entity>() {
                    each(entity);
                }
            }

            whole!();
        }
    )*};
}

plain! {
    bool => Bool bool,
    f32 => F32 f32,
    f64 => F64 f64,
    Entity => Entity entity,
    Texture => Texture texture,
    Model => Model model,
    Font => Font font,
    Prefab => Prefab prefab,
}

impl Reflect for String {
    fn describe(_: &mut Types) -> Result<Type> {
        Ok(Type::String)
    }

    fn to_value(&self) -> Value {
        Value::String(self.clone())
    }

    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::String(value) => Ok(value.clone()),
            other => Err(wrong_kind("String", other)),
        }
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn digest(&self, to: &mut Digest<'_>) {
        to.bytes(self.as_bytes());
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}

    whole!();
}

impl Ranged for f32 {}
impl Ranged for f64 {}
impl Angle for f32 {}

/// Vectors, whose components, each a `$S`, are their parts, digested as `$S`s are.
macro_rules! vectors {
    ($($T:ident: $S:ident,)*) => {$(
        impl Reflect for $T {
            fn describe(_: &mut Types) -> Result<Type> {
                Ok(Type::$T)
            }

            fn to_value(&self) -> Value {
                Value::$T(*self)
            }

            fn from_value(value: &Value) -> Result<Self> {
                match value {
                    Value::$T(value) => Ok(*value),
                    other => Err(wrong_kind(stringify!($T), other)),
                }
            }

            fn with_value(&self, value: &Value) -> Result<Self> {
                Self::from_value(value)
            }

            fn set_value(&mut self, value: &Value) -> Result<()> {
                *self = self.with_value(value)?;
                Ok(())
            }

            fn part(&self, index: u32) -> Option<&dyn Reflect> {
                let components: &[$S] = self.as_ref();
                Some(components.get(index as usize)?)
            }

            fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
                let components: &mut [$S] = self.as_mut();
                Some(components.get_mut(index as usize)?)
            }

            fn digest(&self, to: &mut Digest<'_>) {
                let components: &[$S] = self.as_ref();
                for &component in components {
                    to.$S(component);
                }
            }

            fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
        }

        impl Ranged for $T {}
    )*};
}

vectors! {
    Vec2: f32,
    Vec3: f32,
    Vec4: f32,
    DVec3: f64,
}

impl Color for Vec3 {}
impl Color for Vec4 {}

/// Set whole, and never to one that isn't a unit, so that it has no parts.
impl Reflect for Quat {
    fn describe(_: &mut Types) -> Result<Type> {
        Ok(Type::Quat)
    }

    fn to_value(&self) -> Value {
        Value::Quat(*self)
    }

    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Quat(q) if q.is_normalized() => Ok(*q),
            Value::Quat(q) => Err(Error::new(
                "reflect.not_normalized",
                format!(
                    "a rotation is a unit quaternion, but {q} is {} long",
                    q.length()
                ),
            )),
            other => Err(wrong_kind("Quat", other)),
        }
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    /// x, y, z, then w.
    fn digest(&self, to: &mut Digest<'_>) {
        for component in self.to_array() {
            to.f32(component);
        }
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}

    whole!();
}

/// Set whole: `None` and `Some` aren't parts.
impl<T: Reflect> Reflect for Option<T> {
    fn describe(types: &mut Types) -> Result<Type> {
        Ok(Type::Option(Box::new(T::describe(types)?)))
    }

    fn to_value(&self) -> Value {
        Value::Option(self.as_ref().map(|value| Box::new(value.to_value())))
    }

    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Option(None) => Ok(None),
            Value::Option(Some(value)) => T::from_value(value).map(Some),
            other => Err(wrong_kind("Option", other)),
        }
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        match (self, value) {
            (Some(current), Value::Option(Some(value))) => current.with_value(value).map(Some),
            _ => Self::from_value(value),
        }
    }

    fn digest(&self, to: &mut Digest<'_>) {
        match self {
            None => to.word(0),
            Some(value) => {
                to.word(1);
                value.digest(to);
            }
        }
    }

    fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity)) {
        if let Some(value) = self {
            value.entities_mut(each);
        }
    }

    whole!();
}

impl<T: Ranged> Ranged for Option<T> {}
impl<T: Angle> Angle for Option<T> {}
impl<T: Color> Color for Option<T> {}

/// The elements of a value of [`Value::List`], or the error that it isn't one.
fn elements(value: &Value) -> Result<&[Value]> {
    match value {
        Value::List(items) => Ok(items),
        other => Err(wrong_kind("List", other)),
    }
}

fn index(i: usize) -> String {
    format!("[{i}]")
}

/// Which of `current`'s elements each of `items`, a list set in its place, is: the one whose
/// opaque state it keeps, by its index, or `None` for one that's new. A list of the same length
/// keeps each in its place. Otherwise, as an element taken out, put in or replaced leaves the
/// rest as they were, those alike at the start, by their values, which read what's opaque as
/// [`Value::Opaque`], are the same ones, and so are those alike at the end, after the same; those
/// between keep their places from the first of them, as a set of one of them changed in place
/// does, and those past the elements between are new. So an element taken out takes its state
/// with it, rather than leaving it to the one after; of elements alike, whose values can't tell
/// them apart, the first keep theirs. Neither the Inspector, which sets an element in its place,
/// nor the tool API, which sets values whole, knows more of which element a set took out, so the
/// limits [`Reflect::with_value`] gives hold.
fn kept_from<T: Reflect>(current: &[T], items: &[Value]) -> impl Fn(usize) -> Option<usize> {
    let (old, new) = (current.len(), items.len());
    let (mut start, mut end) = (old.min(new), 0);
    if old != new {
        let same = |at: usize, item: &Value| current[at].to_value() == *item;
        start = (0..old.min(new))
            .find(|&i| !same(i, &items[i]))
            .unwrap_or(old.min(new));
        let tail = old.min(new) - start;
        end = (0..tail)
            .find(|&i| !same(old - 1 - i, &items[new - 1 - i]))
            .unwrap_or(tail);
    }
    move |i| {
        if i < start {
            Some(i)
        } else if i >= new - end {
            Some(i + old - new)
        } else {
            Some(i).filter(|&i| i < old - end)
        }
    }
}

/// Its elements are its parts.
impl<T: Reflect> Reflect for Vec<T> {
    fn describe(types: &mut Types) -> Result<Type> {
        Ok(Type::List(Box::new(T::describe(types)?)))
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(Reflect::to_value).collect())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let items = elements(value)?.iter().enumerate();
        let made = items.map(|(i, item)| T::from_value(item).map_err(|e| within(e, &index(i))));
        made.collect()
    }

    // Each element keeps what's opaque of the one it is, as `kept_from` finds it.
    fn with_value(&self, value: &Value) -> Result<Self> {
        let items = elements(value)?;
        let from = kept_from(self, items);
        let kept = items.iter().enumerate().map(|(i, item)| {
            match from(i).and_then(|at| self.get(at)) {
                Some(current) => current.with_value(item),
                None => T::from_value(item),
            }
            .map_err(|e| within(e, &index(i)))
        });
        kept.collect()
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        Some(self.get(index as usize)?)
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        Some(self.get_mut(index as usize)?)
    }

    fn digest(&self, to: &mut Digest<'_>) {
        to.word(self.len() as u64);
        for item in self {
            item.digest(to);
        }
    }

    fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity)) {
        for item in self {
            item.entities_mut(each);
        }
    }
}

impl<T: Ranged> Ranged for Vec<T> {}
impl<T: Angle> Angle for Vec<T> {}
impl<T: Color> Color for Vec<T> {}

/// Its elements are its parts, and a value of it has `N` of them.
impl<T: Reflect, const N: usize> Reflect for [T; N] {
    fn describe(types: &mut Types) -> Result<Type> {
        let len = const {
            assert!(
                N <= u32::MAX as usize,
                "reflected arrays have fewer than 2^32 elements"
            );
            N as u32
        };
        Ok(Type::Array(Box::new(T::describe(types)?), len))
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(Reflect::to_value).collect())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let items = sized::<N>(value)?.iter().enumerate();
        let made = items.map(|(i, item)| T::from_value(item).map_err(|e| within(e, &index(i))));
        array(made.collect::<Result<_>>()?)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let items = self.iter().zip(sized::<N>(value)?).enumerate();
        let kept = items
            .map(|(i, (current, item))| current.with_value(item).map_err(|e| within(e, &index(i))));
        array(kept.collect::<Result<_>>()?)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        Some(self.get(index as usize)?)
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        Some(self.get_mut(index as usize)?)
    }

    /// Its elements, without their number, which its type says.
    fn digest(&self, to: &mut Digest<'_>) {
        for item in self {
            item.digest(to);
        }
    }

    fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity)) {
        for item in self {
            item.entities_mut(each);
        }
    }
}

impl<T: Ranged, const N: usize> Ranged for [T; N] {}
impl<T: Angle, const N: usize> Angle for [T; N] {}
impl<T: Color, const N: usize> Color for [T; N] {}

/// The elements of `value`, a list of `N` of them.
fn sized<const N: usize>(value: &Value) -> Result<&[Value]> {
    let items = elements(value)?;
    if items.len() != N {
        return Err(Error::new(
            "reflect.wrong_length",
            format!("an array of {N} is given {} elements", items.len()),
        ));
    }
    Ok(items)
}

/// `items`, `N` of them, as an array.
fn array<T, const N: usize>(items: Vec<T>) -> Result<[T; N]> {
    let len = items.len();
    items.try_into().map_err(|_| {
        Error::new(
            "reflect.wrong_length",
            format!("an array of {N} is given {len} elements"),
        )
    })
}
