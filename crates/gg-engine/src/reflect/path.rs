//! Paths, such as `rect[1].y`: where one leads in a reflected type's description, and the part of
//! a value it reaches, which `World::get_field` and `World::set_field` read and set.

use std::fmt;

use super::{
    COMPONENTS, Error, Meta, Reflect, Result, Shape, Show, Type, Types, Value, list, what_is,
};

/// Where a path leads in a type: the parts to take, from the outside in, the type it reaches, and
/// the hints of the field it's in, whose bounds its components and elements inherit.
#[derive(Debug)]
pub(crate) struct Resolved {
    pub(crate) steps: Vec<u32>,
    pub(crate) ty: Type,
    pub(crate) meta: Meta,
}

/// Where `path` leads in a value of `root`: a field's name, then `.name` for a struct's field,
/// `.x` to `.w` for a vector's component, and `[i]` for an array's or a list's element, such as
/// `rect[1].y`. An empty path is the whole value.
///
/// Fails with `reflect.bad_path` for a path it can't read, and `reflect.no_field` for a step to
/// a part there isn't, saying what's there: enums, options and quaternions are set whole, so a
/// step into one is one. An index past a list's end only fails as the list is reached.
pub(crate) fn resolve(types: &Types, root: &Type, path: &str) -> Result<Resolved> {
    let mut to = Resolved {
        steps: Vec::new(),
        ty: root.clone(),
        meta: Meta::default(),
    };
    let bad =
        |what: String| Error::new("reflect.bad_path", format!("`{path}` isn't a path: {what}"));
    let mut rest = path;
    while !rest.is_empty() {
        let walked = &path[..path.len() - rest.len()];
        let step = if let Some(index) = rest.strip_prefix('[') {
            let Some((digits, after)) = index.split_once(']') else {
                return Err(bad("a `[` has no `]`".to_owned()));
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad(format!("`[{digits}]` isn't an index, such as `[0]`")));
            }
            let Ok(index) = digits.parse() else {
                return Err(bad(format!(
                    "`[{digits}]` is past the last index, {}",
                    u32::MAX
                )));
            };
            rest = after;
            Step::Index(index)
        } else {
            let name = if walked.is_empty() {
                rest
            } else {
                rest.strip_prefix('.').ok_or_else(|| {
                    bad(format!(
                        "`{rest}` follows `{walked}` without a `.` or a `[`"
                    ))
                })?
            };
            let end = name.find(['.', '[']).unwrap_or(name.len());
            let (name, after) = name.split_at(end);
            if name.is_empty() {
                return Err(bad("a step names no field".to_owned()));
            }
            // Any identifier's: those that aren't ASCII are left to the lookup, as Rust's
            // identifiers take so many characters.
            if !name
                .chars()
                .all(|c| !c.is_ascii() || c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(bad(format!("`{name}` isn't a field's name")));
            }
            rest = after;
            Step::Field(name)
        };
        to.take(types, step, walked)?;
    }
    Ok(to)
}

/// One step of a path.
#[derive(Clone, Copy)]
enum Step<'a> {
    Field(&'a str),
    Index(u32),
}

impl fmt::Display for Step<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(name) => write!(f, "{name}"),
            Self::Index(index) => write!(f, "[{index}]"),
        }
    }
}

impl Resolved {
    /// Takes `step` from where it's reached, `walked` along the path.
    fn take(&mut self, types: &Types, step: Step<'_>, walked: &str) -> Result<()> {
        let none = |what: String| Err(Error::new("reflect.no_field", what));
        let here = |types: &Types, ty: &Type| match walked {
            "" => what_is(types, ty),
            walked => format!("`{walked}` is {}", what_is(types, ty)),
        };
        let (components, component) = match self.ty {
            Type::Vec2 => (2, Type::F32),
            Type::Vec3 => (3, Type::F32),
            Type::Vec4 => (4, Type::F32),
            Type::DVec3 => (3, Type::F64),
            _ => (0, Type::F32),
        };
        match (&self.ty, step) {
            (Type::Named(index), Step::Field(name)) => {
                let info = types.info(*index);
                let Shape::Struct { fields, .. } = &info.shape else {
                    return none(format!("{}, set whole", here(types, &self.ty)));
                };
                let Some(i) = fields.iter().position(|field| field.name == name) else {
                    let names = list(fields.iter().map(|field| &field.name));
                    return none(format!(
                        "{}, which has no field `{name}`; its fields are {names}",
                        here(types, &self.ty)
                    ));
                };
                self.steps.push(i as u32);
                self.ty = fields[i].ty.clone();
                self.meta = fields[i].meta.clone();
            }
            (_, Step::Field(name)) if components > 0 => {
                let names = &COMPONENTS[..components];
                let Some(i) = names.iter().position(|&component| component == name) else {
                    return none(format!(
                        "{}, whose components are {}",
                        here(types, &self.ty),
                        list(names)
                    ));
                };
                self.steps.push(i as u32);
                self.ty = component;
                self.meta.show = Show::Plain;
            }
            (Type::Array(_, len), Step::Index(index)) if index >= *len => {
                let last = match len {
                    0 => "none".to_owned(),
                    len => format!("`[0]` to `[{}]`", len - 1),
                };
                return none(format!(
                    "{}, whose elements are {last}",
                    here(types, &self.ty)
                ));
            }
            (Type::Array(inner, _) | Type::List(inner), Step::Index(index)) => {
                self.steps.push(index);
                self.ty = (**inner).clone();
            }
            (Type::Option(_) | Type::Quat, _) => {
                return none(format!("{}, set whole", here(types, &self.ty)));
            }
            _ => {
                return none(format!("{}, which has no `{step}`", here(types, &self.ty)));
            }
        }
        Ok(())
    }
}

/// The part of `root` that `steps`, a resolved path's, lead to.
///
/// Fails with `reflect.no_field` at an index past a list's end.
pub(crate) fn reach<'a>(root: &'a dyn Reflect, steps: &[u32]) -> Result<&'a dyn Reflect> {
    let mut at = root;
    for &step in steps {
        at = at.part(step).ok_or_else(|| past_end(at, step))?;
    }
    Ok(at)
}

pub(crate) fn reach_mut<'a>(
    root: &'a mut dyn Reflect,
    steps: &[u32],
) -> Result<&'a mut dyn Reflect> {
    let mut at = root;
    for &step in steps {
        // Asked first, since the borrow checker keeps `at` borrowed in the branch where
        // `part_mut` returns nothing.
        if at.part(step).is_none() {
            return Err(past_end(at, step));
        }
        at = at.part_mut(step).expect("the part is there");
    }
    Ok(at)
}

/// The error that `list` has no element `index`.
fn past_end(list: &dyn Reflect, index: u32) -> Error {
    let len = match list.to_value() {
        Value::List(items) => items.len(),
        _ => 0,
    };
    Error::new(
        "reflect.no_field",
        format!("the list has no element [{index}]: it has {len}"),
    )
}
