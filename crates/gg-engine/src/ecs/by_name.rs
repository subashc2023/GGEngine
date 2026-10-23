//! The world as tools reach it, without the Rust types of its components: component types by name,
//! their fields by path, and their values as reflection's [`Value`]s. The inspector is built on
//! these, the world's IDs and its hierarchy, and so will the tool API be.

use std::any::TypeId;
use std::slice;

use gg_core::{Error, Result};

use super::id::Named;
use super::table::{ComponentId, Table};
use super::{Component, Entity, Location, Parent, World};
use crate::reflect::{self, Types, Value};

impl World {
    /// Registers component type `T`, describing it to reflection, if it hasn't registered yet: a
    /// type registers the first time an entity has one too, so this is for those that tools should
    /// list before then.
    ///
    /// Fails with `reflect.name_taken` if another type has its name.
    pub fn register<T: Component>(&mut self) -> Result<()> {
        self.components.register::<T>().map(drop)
    }

    /// The descriptions of the component types, and of the types of their fields.
    pub fn types(&self) -> &Types {
        self.components.types()
    }

    /// The component types' names, in the order they registered.
    pub fn component_types(&self) -> impl ExactSizeIterator<Item = &str> {
        self.components.names()
    }

    /// The living entities, table by table, in the order queries find them; with `with`, those
    /// with that component. Skipping entities skips whole tables, so a page of them costs a step a
    /// table, and one for each entity on the page.
    ///
    /// Fails with `reflect.unknown_type` if no component type is named `with`.
    pub fn entities(&self, with: Option<&str>) -> Result<Entities<'_>> {
        let with = with.map(|name| self.named(name)).transpose()?;
        let len = self
            .tables
            .iter()
            .filter(|table| has(table, with))
            .map(|table| table.entities.len())
            .sum();
        Ok(Entities {
            tables: self.tables.iter(),
            with,
            rows: slice::Iter::default(),
            len,
        })
    }

    /// The names of `entity`'s components' types, in the order they registered.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn components_of(&self, entity: Entity) -> Result<impl ExactSizeIterator<Item = &str>> {
        let at = self.locate(entity)?;
        let ids = self.tables[at.table as usize].components.iter();
        Ok(ids.map(|&id| self.components.name(id)))
    }

    /// The value of the field at `path` of `entity`'s `component`: such as `translation.x`, or
    /// `rect[1]`; an empty path is the whole component. A path is a field's name, then `.name` for
    /// a field of a struct, `.x` to `.w` for a vector's component, and `[i]` for an array's or a
    /// list's element. Enums, options and quaternions are set whole, so a path doesn't go into
    /// them. What's opaque reads as [`Value::Opaque`].
    ///
    /// Fails with `ecs.no_entity` if `entity` isn't alive; `reflect.unknown_type` if no component
    /// type is named `component`; `reflect.missing_component` if `entity` hasn't one;
    /// `reflect.bad_path` for a path that can't be read; and `reflect.no_field` for one that leads
    /// to no field. Every error names the entity, by its ID while it's alive, the component and the
    /// path.
    pub fn get_field(&self, entity: Entity, component: &str, path: &str) -> Result<Value> {
        let name = self.name_of(entity);
        let about = |error: Error| about(error, "get", name, component, path);
        let (id, at) = self.find(entity, component).map_err(about)?;
        let column = self.column(at, id, component).map_err(about)?;
        let types = self.components.types();
        let to = reflect::resolve(types, &self.components.ty(id), path).map_err(about)?;
        let row = self.tables[at.table as usize].columns[column].row(at.row as usize);
        if to.ty.is_opaque(types) {
            reflect::reach_opaque(row, &to).map_err(about)?;
            return Ok(Value::Opaque);
        }
        Ok(reflect::reach(row, &to.steps).map_err(about)?.to_value())
    }

    /// Sets the field at `path` of `entity`'s `component` to `value`, and returns its old value,
    /// which sets it back, unless code stored one that the checks below refuse, such as a number
    /// outside its field's bounds. Paths are as [`get_field`](Self::get_field) reads them. `value`
    /// is checked against the component's description, all of it before anything changes, so that a
    /// value refused changes nothing: its kinds, the bounds of its fields, which it's outside of if
    /// a number isn't finite, and a rotation being a unit quaternion. What's opaque is set only to
    /// [`Value::Opaque`], which keeps it as it is. A [`Parent`] isn't set by name: the world keeps
    /// it, and [`World::set_parent`] changes it.
    ///
    /// Fails as `get_field` does; with `reflect.read_only` for `Parent`, at any path, whether the
    /// entity has one or not; with `reflect.out_of_range` for a number outside its field's
    /// bounds or its type's; and as [`Reflect::with_value`](crate::Reflect::with_value) does for
    /// a value that doesn't fit the field. Every error names the entity, by its ID while it's
    /// alive, the component and the path, down to the part refused.
    pub fn set_field(
        &mut self,
        entity: Entity,
        component: &str,
        path: &str,
        value: &Value,
    ) -> Result<Value> {
        // Named before the closure, which would otherwise still borrow the world at `row_mut`.
        let name = self.name_of(entity);
        let about = |error: Error| about(error, "set", name, component, path);
        let (id, at) = self.find(entity, component).map_err(about)?;
        if Some(id) == self.components.get(TypeId::of::<Parent>()) {
            return Err(about(Error::new(
                "reflect.read_only",
                "the world keeps it, so that the hierarchy stays whole; `World::set_parent` \
                 changes it",
            )));
        }
        let column = self.column(at, id, component).map_err(about)?;
        let types = self.components.types();
        let to = reflect::resolve(types, &self.components.ty(id), path).map_err(about)?;
        if to.ty.is_opaque(types) {
            let row = self.tables[at.table as usize].columns[column].row(at.row as usize);
            reflect::reach_opaque(row, &to).map_err(about)?;
            return match value {
                Value::Opaque => Ok(Value::Opaque),
                other => Err(about(reflect::not_opaque(other))),
            };
        }
        reflect::check(types, &to.ty, &to.meta, value).map_err(about)?;
        let row = self.tables[at.table as usize].columns[column].row_mut(at.row as usize);
        let field = reflect::reach_mut(row, &to.steps).map_err(about)?;
        let old = field.to_value();
        field.set_value(value).map_err(about)?;
        Ok(old)
    }

    /// The id of component type `name`.
    ///
    /// Fails with `reflect.unknown_type` if there's none.
    pub(super) fn named(&self, name: &str) -> Result<ComponentId> {
        self.components.named(name).ok_or_else(|| {
            let names = reflect::list(self.component_types());
            let what = match self.types().find(name) {
                Some(_) => format!("`{name}` is a type of fields, not a component"),
                None => format!("there's no component type `{name}`"),
            };
            Error::new(
                "reflect.unknown_type",
                format!("{what}; the component types are {names}"),
            )
        })
    }

    /// Where `entity` is, and the id of component type `component`.
    fn find(&self, entity: Entity, component: &str) -> Result<(ComponentId, Location)> {
        let at = self.location(entity).ok_or_else(|| {
            Error::new(
                "ecs.no_entity",
                "the entity isn't in the world: it was despawned, or is another world's",
            )
        })?;
        Ok((self.named(component)?, at))
    }

    /// Which column of the table at `at` holds the entity's `component`, whose id is `id`.
    fn column(&self, at: Location, id: ComponentId, component: &str) -> Result<usize> {
        let table = &self.tables[at.table as usize];
        let Some(column) = table.position(id) else {
            let names = table.components.iter().map(|&id| self.components.name(id));
            return Err(Error::new(
                "reflect.missing_component",
                format!(
                    "the entity has no `{component}`; it has {}",
                    reflect::list(names)
                ),
            ));
        };
        Ok(column)
    }
}

/// `error`, from `doing`, such as getting or setting, the field at `path` of `entity`'s
/// `component`, naming them and the whole path, down to the part where it happened.
pub(super) fn about(
    error: Error,
    doing: &str,
    entity: Named,
    component: &str,
    path: &str,
) -> Error {
    let (inner, what) = reflect::located(&error);
    let at = reflect::join(&reflect::join(component, path), &inner);
    let message = format!("can't {doing} `{at}` of entity {entity}: {what}");
    error.with_message(message)
}

/// Whether `table` has component `with`, if it's given.
fn has(table: &Table, with: Option<ComponentId>) -> bool {
    with.is_none_or(|id| table.has(id))
}

/// The entities [`World::entities`] lists, table by table.
pub struct Entities<'w> {
    /// The tables after the one it's in.
    tables: slice::Iter<'w, Table>,
    with: Option<ComponentId>,
    /// The rows left of the table it's in.
    rows: slice::Iter<'w, Entity>,
    /// The entities left.
    len: usize,
}

impl Entities<'_> {
    /// Moves to the next table that has the component, and returns whether there was one.
    fn next_table(&mut self) -> bool {
        let with = self.with;
        match self.tables.find(|table| has(table, with)) {
            Some(table) => {
                self.rows = table.entities.iter();
                true
            }
            None => false,
        }
    }
}

impl Iterator for Entities<'_> {
    type Item = Entity;

    fn next(&mut self) -> Option<Entity> {
        loop {
            if let Some(&entity) = self.rows.next() {
                self.len -= 1;
                return Some(entity);
            }
            if !self.next_table() {
                return None;
            }
        }
    }

    /// Skips whole tables, so that skipping costs a step a table.
    fn nth(&mut self, mut n: usize) -> Option<Entity> {
        loop {
            let left = self.rows.len();
            if n < left {
                self.len -= n + 1;
                return self.rows.nth(n).copied();
            }
            n -= left;
            self.len -= left;
            self.rows = slice::Iter::default();
            if !self.next_table() {
                return None;
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl ExactSizeIterator for Entities<'_> {}

#[cfg(test)]
mod tests {
    use gg_core::math::{Quat, Vec2, Vec3, Vec4};
    use gg_renderer::Textures;

    use super::*;
    use crate::reflect::tests::{
        Everything, Form, Handle, Inner, WithOpaque, everything, same, same_snapshots, snapshot,
    };
    use crate::{MeshDraw, Reflect, Sampler, Sprite, Transform};

    fn named(name: &str, value: Value) -> (reflect::Name, Value) {
        (name.to_owned().into(), value)
    }

    fn inner(level: i64, name: &str) -> Value {
        Value::Struct(vec![
            named("level", Value::Int(level)),
            named("name", Value::String(name.to_owned())),
        ])
    }

    #[test]
    fn fields_are_set_by_path_and_set_back_by_what_they_were() {
        let mut world = World::default();
        let entity = world.spawn((Transform::IDENTITY, everything()));
        let x = world.get_field(entity, "Transform", "translation.x");
        assert_eq!(x.expect("a field"), Value::F32(0.0));
        let old = world.set_field(entity, "Transform", "translation.x", &Value::F32(2.5));
        assert_eq!(old.expect("set"), Value::F32(0.0));
        let transform = world.get::<Transform>(entity).expect("a transform");
        assert_eq!(transform.translation, Vec3::new(2.5, 0.0, 0.0));

        let before = snapshot(&world);
        for (path, value) in [
            ("on", Value::Bool(false)),
            ("count", Value::UInt(100)),
            ("byte", Value::UInt(255)),
            ("weight", Value::F64(f64::from_bits(1))),
            ("angle", Value::F32(-0.0)),
            ("tint", Value::Vec4(Vec4::new(1.0, 0.0, 0.5, 0.25))),
            ("offset.y", Value::F32(9.0)),
            ("turn", Value::Quat(Quat::from_rotation_y(2.0))),
            (
                "target",
                Value::Option(Some(Box::new(Value::Entity(entity)))),
            ),
            ("inner.level", Value::Int(1)),
            ("inner.name", Value::String("second".to_owned())),
            ("list[1]", Value::UInt(10)),
            ("rect[1]", Value::Vec2(Vec2::new(-0.0, 7.0))),
            ("rect[0].x", Value::F32(0.5)),
            ("pair.1", Value::Int(i64::MIN)),
            ("unit", Value::Struct(Vec::new())),
            ("handle", Value::Opaque),
        ] {
            let old = world
                .set_field(entity, "Everything", path, &value)
                .unwrap_or_else(|error| panic!("{error}"));
            let now = world
                .get_field(entity, "Everything", path)
                .expect("a field");
            assert!(same(&now, &value), "{path}: {now:?}");
            let set_back = world
                .set_field(entity, "Everything", path, &old)
                .expect("set back");
            assert!(same(&set_back, &value), "{path}");
        }
        assert!(same_snapshots(&snapshot(&world), &before));
        assert_eq!(
            world.get::<Everything>(entity).expect("everything").handle,
            Handle(7)
        );
        // A signed value sets an unsigned field it fits, which reads as unsigned.
        let old = world.set_field(entity, "Everything", "byte", &Value::Int(255));
        assert_eq!(old.expect("set"), Value::UInt(7));
        let byte = world.get_field(entity, "Everything", "byte");
        assert_eq!(byte.expect("a field"), Value::UInt(255));
        world
            .set_field(entity, "Everything", "byte", &Value::UInt(7))
            .expect("set back");
        // Read whole, a component sets back whole.
        let whole = world
            .get_field(entity, "Everything", "")
            .expect("everything");
        let old = world
            .set_field(entity, "Everything", "", &whole)
            .expect("set");
        assert!(same(&old, &whole));
        assert_eq!(world.get::<Everything>(entity), Some(&everything()));
    }

    #[test]
    fn a_sprite_set_whole_keeps_its_texture() {
        let texture = Textures::default().add("first", [1, 1], &[0; 4]);
        let texture = texture.expect("a texture");
        let mut world = World::default();
        let sprite = Sprite {
            texture,
            ..Sprite::default()
        };
        let entity = world.spawn(sprite);
        let Value::Struct(mut fields) = world.get_field(entity, "Sprite", "").expect("a sprite")
        else {
            panic!("a sprite is a struct");
        };
        assert_eq!(fields[1], named("texture", Value::Opaque));
        fields[0].1 = Value::Vec4(Vec4::new(0.5, 0.5, 0.5, 1.0));
        world
            .set_field(entity, "Sprite", "", &Value::Struct(fields))
            .expect("set");
        let set = world.get::<Sprite>(entity).expect("a sprite");
        assert_eq!(
            (set.texture, set.color),
            (texture, Vec4::new(0.5, 0.5, 0.5, 1.0))
        );
        // Opaque, it reads and sets as such alone.
        let read = world.get_field(entity, "Sprite", "texture");
        assert_eq!(read.expect("a field"), Value::Opaque);
        let set = world.set_field(entity, "Sprite", "texture", &Value::Opaque);
        assert_eq!(set.expect("kept"), Value::Opaque);
        let sampler = Value::Variant("NearestRepeat".into(), Vec::new());
        world
            .set_field(entity, "Sprite", "sampler", &sampler)
            .expect("set");
        let set = world.get::<Sprite>(entity).expect("a sprite");
        assert_eq!(
            (set.texture, set.sampler),
            (texture, Sampler::NearestRepeat)
        );
    }

    #[test]
    fn options_lists_and_enums_are_set_whole() {
        let mut world = World::default();
        let entity = world.spawn(everything());
        let mut set = |path, value| {
            world
                .set_field(entity, "Everything", path, &value)
                .unwrap_or_else(|error| panic!("{error}"));
        };
        set("maybe", Value::Option(Some(Box::new(inner(1, "made")))));
        set("list", Value::List((1..=4).map(Value::UInt).collect()));
        let size = named("size", Value::Vec2(Vec2::ONE));
        set(
            "form",
            Value::Variant("Box".into(), vec![size, named("turn", Value::F32(1.0))]),
        );
        let all = world.get::<Everything>(entity).expect("everything");
        let made = Inner {
            level: 1,
            name: "made".to_owned(),
        };
        assert_eq!(all.maybe, Some(made));
        assert_eq!(all.list, [1, 2, 3, 4]);
        let form = Form::Box {
            size: Vec2::ONE,
            turn: 1.0,
        };
        assert_eq!(all.form, form);
        let mut set = |path, value| {
            world
                .set_field(entity, "Everything", path, &value)
                .unwrap_or_else(|error| panic!("{error}"));
        };
        set("maybe", Value::Option(None));
        set("list", Value::List(vec![Value::UInt(9)]));
        set(
            "form",
            Value::Variant("Circle".into(), vec![named("0", Value::F32(2.0))]),
        );
        let all = world.get::<Everything>(entity).expect("everything");
        assert_eq!(
            (&all.maybe, &all.list[..], &all.form),
            (&None, &[9][..], &Form::Circle(2.0))
        );
        // A variant with something opaque is kept, but not made.
        world
            .get_mut::<Everything>(entity)
            .expect("everything")
            .form = Form::Held(Handle(5));
        let held = Value::Variant("Held".into(), vec![named("0", Value::Opaque)]);
        world
            .set_field(entity, "Everything", "form", &held)
            .expect("kept");
        let all = world.get::<Everything>(entity).expect("everything");
        assert_eq!(all.form, Form::Held(Handle(5)));
    }

    #[test]
    fn a_value_refused_changes_nothing_and_says_where() {
        let mut world = World::default();
        let entity = world.spawn((Transform::IDENTITY, everything()));
        let sprite = world.spawn(Sprite::default());
        let dead = world.spawn(());
        world.despawn(dead).expect("alive");
        let quat = Value::Quat(Quat::from_xyzw(0.0, 0.0, 0.0, 2.0));
        let point = Value::Variant("Point".into(), Vec::new());
        let held = Value::Variant("Held".into(), vec![named("0", Value::Opaque)]);
        let circle = Value::Variant("Circle".into(), vec![named("0", Value::F32(-1.0))]);
        let level_only = Value::Struct(vec![named("level", Value::Int(0))]);
        let Value::Struct(mut extra) = inner(0, "x") else {
            unreachable!()
        };
        extra.push(named("extra", Value::Bool(true)));
        let mut twice = extra.clone();
        twice[2] = named("level", Value::Int(0));
        let too_far = inner(5, "x");
        let elements = Value::List(vec![Value::UInt(1), Value::UInt(11)]);
        let mut cases = vec![
            (
                entity,
                "Everything",
                "on",
                Value::UInt(1),
                "reflect.wrong_kind",
                "Everything.on",
            ),
            (
                entity,
                "Everything",
                "offset",
                Value::Opaque,
                "reflect.wrong_kind",
                "Everything.offset",
            ),
            (
                entity,
                "Everything",
                "count",
                Value::UInt(101),
                "reflect.out_of_range",
                "Everything.count",
            ),
            (
                entity,
                "Everything",
                "angle",
                Value::F32(f32::NAN),
                "reflect.out_of_range",
                "Everything.angle",
            ),
            (
                entity,
                "Everything",
                "byte",
                Value::UInt(300),
                "reflect.out_of_range",
                "Everything.byte",
            ),
            (
                entity,
                "Everything",
                "byte",
                Value::Int(-1),
                "reflect.out_of_range",
                "Everything.byte",
            ),
            (
                entity,
                "Everything",
                "turn",
                quat,
                "reflect.not_normalized",
                "Everything.turn",
            ),
            (
                sprite,
                "Sprite",
                "texture",
                Value::UInt(1),
                "reflect.opaque",
                "Sprite.texture",
            ),
            (
                entity,
                "Everything",
                "form",
                held,
                "reflect.opaque",
                "Everything.form.0",
            ),
            (
                entity,
                "Everything",
                "form",
                circle,
                "reflect.out_of_range",
                "Everything.form.0",
            ),
            (
                entity,
                "Everything",
                "handle",
                Value::UInt(1),
                "reflect.opaque",
                "Everything.handle",
            ),
            (
                entity,
                "Everything",
                "inner",
                level_only,
                "reflect.missing_field",
                "Everything.inner",
            ),
            (
                entity,
                "Everything",
                "inner",
                Value::Struct(extra),
                "reflect.no_field",
                "Everything.inner",
            ),
            (
                entity,
                "Everything",
                "inner",
                Value::Struct(twice),
                "reflect.duplicate_field",
                "Everything.inner",
            ),
            (
                entity,
                "Everything",
                "inner",
                too_far,
                "reflect.out_of_range",
                "Everything.inner.level",
            ),
            (
                entity,
                "Everything",
                "list",
                elements,
                "reflect.out_of_range",
                "Everything.list[1]",
            ),
            (
                entity,
                "Everything",
                "list[9]",
                Value::UInt(1),
                "reflect.no_field",
                "Everything.list[9]",
            ),
            (
                entity,
                "Everything",
                "rect",
                Value::List(vec![Value::Vec2(Vec2::ZERO)]),
                "reflect.wrong_length",
                "Everything.rect",
            ),
            (
                entity,
                "Everything",
                "form",
                Value::Variant("Square".into(), Vec::new()),
                "reflect.no_variant",
                "Everything.form",
            ),
            (
                entity,
                "Everything",
                "spin",
                Value::F32(0.0),
                "reflect.no_field",
                "Everything.spin",
            ),
            (
                entity,
                "Everything",
                "rect[",
                Value::F32(0.0),
                "reflect.bad_path",
                "Everything.rect[",
            ),
            (
                entity,
                "Nothing",
                "",
                point.clone(),
                "reflect.unknown_type",
                "Nothing",
            ),
            (
                entity,
                "Inner",
                "",
                inner(0, "x"),
                "reflect.unknown_type",
                "Inner",
            ),
            (
                entity,
                "Sprite",
                "",
                point.clone(),
                "reflect.missing_component",
                "Sprite",
            ),
            (dead, "Everything", "", point, "ecs.no_entity", "Everything"),
        ];
        // A whole component, refused at the part that's wrong.
        let Value::Struct(mut whole) = world
            .get_field(entity, "Everything", "")
            .expect("everything")
        else {
            unreachable!()
        };
        whole[1].1 = Value::UInt(200);
        cases.push((
            entity,
            "Everything",
            "",
            Value::Struct(whole),
            "reflect.out_of_range",
            "Everything.count",
        ));
        let before = snapshot(&world);
        for (at, component, path, value, code, location) in cases {
            let error = world
                .set_field(at, component, path, &value)
                .expect_err(path);
            let message = error.to_string();
            assert_eq!(error.code(), code, "{message}");
            // By its ID while it's alive, and by its handle once it isn't.
            let named = world
                .id(at)
                .map_or_else(|| at.to_string(), |id| id.to_string());
            let says = format!("can't set `{location}` of entity {named}: ");
            assert!(message.starts_with(&says), "{message}");
            assert!(same_snapshots(&snapshot(&world), &before), "{message}");
        }
        let all = world.get::<Everything>(entity).expect("everything");
        assert_eq!(all, &everything());
        // Reading names the same.
        let error = world
            .get_field(entity, "Everything", "list[2]")
            .expect_err("past the end");
        assert_eq!(error.code(), "reflect.no_field");
        let message = error.to_string();
        let id = world.id(entity).expect("alive");
        let says = format!("can't get `Everything.list[2]` of entity {id}: ");
        assert!(
            message.starts_with(&says) && message.ends_with("it has 2"),
            "{message}"
        );
        let error = world
            .entities(Some("Inner"))
            .err()
            .expect("a type of fields");
        assert_eq!(error.code(), "reflect.unknown_type");
        assert!(
            error
                .to_string()
                .contains("`Transform`, `Everything` and `Sprite`"),
            "{error}"
        );
    }

    /// Writes a struct, as a game's macro might, taking its derive from where it's called.
    macro_rules! written {
        ($(#[$meta:meta])* $name:ident) => {
            $(#[$meta])*
            struct $name {
                #[reflect(min = 0.0)]
                speed: f32,
                #[reflect(opaque)]
                handle: Handle,
            }
        };
    }

    /// A tuple struct the same way.
    macro_rules! written_tuple {
        ($(#[$meta:meta])* $name:ident) => {
            $(#[$meta])*
            struct $name(#[reflect(min = 0.0)] f32, #[reflect(opaque)] Handle);
        };
    }

    written!(
        #[derive(Clone, crate::Component)]
        Written
    );
    written_tuple!(
        #[derive(Clone, crate::Component)]
        WrittenTuple
    );

    #[test]
    fn types_a_macro_writes_are_reached_by_path() {
        let mut world = World::default();
        let written = Written {
            speed: 1.0,
            handle: Handle(2),
        };
        let entity = world.spawn((written, WrittenTuple(3.0, Handle(4))));
        for (component, path) in [("Written", "speed"), ("WrittenTuple", "0")] {
            let old = world.set_field(entity, component, path, &Value::F32(5.0));
            assert!(matches!(old, Ok(Value::F32(_))), "{old:?}");
            let error = world
                .set_field(entity, component, path, &Value::F32(-1.0))
                .expect_err("below its min");
            assert_eq!(error.code(), "reflect.out_of_range");
        }
        for (component, path) in [("Written", "handle"), ("WrittenTuple", "1")] {
            let kept = world.set_field(entity, component, path, &Value::Opaque);
            assert_eq!(kept.expect("kept"), Value::Opaque);
        }
        let written = world.get::<Written>(entity).expect("written");
        assert_eq!((written.speed, &written.handle), (5.0, &Handle(2)));
        let tuple = world.get::<WrittenTuple>(entity).expect("written");
        assert_eq!((tuple.0, &tuple.1), (5.0, &Handle(4)));
    }

    /// Lists of what holds something opaque, and of what's opaque.
    #[derive(Clone, crate::Component)]
    struct Held {
        items: Vec<WithOpaque>,
        meshes: Vec<MeshDraw>,
    }

    #[test]
    fn an_opaque_part_is_reached_as_any_other_and_kept_in_its_place() {
        let mut world = World::default();
        let items = (1..=3).map(|n| WithOpaque { handle: Handle(n) }).collect();
        let meshes = Vec::new();
        let entity = world.spawn(Held { items, meshes });
        // Past a list's end there's no field, opaque or not.
        for path in ["items[3].handle", "meshes[0]"] {
            let error = world.get_field(entity, "Held", path).expect_err(path);
            assert_eq!(error.code(), "reflect.no_field", "{error}");
            let error = world
                .set_field(entity, "Held", path, &Value::Opaque)
                .expect_err(path);
            assert_eq!(error.code(), "reflect.no_field", "{error}");
        }
        let read = world.get_field(entity, "Held", "items[2].handle");
        assert_eq!(read.expect("a field"), Value::Opaque);
        // What's opaque is kept by index: a list cut short keeps its first elements' own.
        let Value::List(mut items) = world.get_field(entity, "Held", "items").expect("a list")
        else {
            panic!("a list");
        };
        items.pop();
        world
            .set_field(entity, "Held", "items", &Value::List(items))
            .expect("set");
        let held = world.get::<Held>(entity).expect("held");
        let handles: Vec<u32> = held.items.iter().map(|item| item.handle.0).collect();
        assert_eq!(handles, [1, 2]);
    }

    mod first {
        #[derive(Clone, crate::Component)]
        pub(super) struct Health(pub(super) u32);
    }

    mod second {
        #[derive(Clone, crate::Component)]
        pub(super) struct Health(pub(super) f32);

        /// Renamed, so that it doesn't take another's name.
        #[derive(Clone, crate::Component)]
        #[reflect(name = "Armor")]
        pub(super) struct Health2(pub(super) f32);

        /// Named as the world's own component.
        #[derive(Clone, crate::Component)]
        pub(super) struct Parent;
    }

    #[test]
    fn a_type_named_as_another_is_refused() {
        let mut world = World::default();
        world.register::<first::Health>().expect("registered");
        let error = world
            .register::<second::Health>()
            .expect_err("a name taken");
        assert_eq!(error.code(), "reflect.name_taken");
        let message = error.to_string();
        assert!(
            message.contains("by_name::tests::first::Health")
                && message.contains("by_name::tests::second::Health"),
            "{message}"
        );
        world.register::<second::Health2>().expect("renamed");
        // Nor does a type take the name of the world's own, which it has from the start.
        let error = world
            .register::<second::Parent>()
            .expect_err("the world's own name");
        assert_eq!(error.code(), "reflect.name_taken");
        // Nor does a type take another's description, and with it its name, whichever registers
        // first.
        for _ in 0..2 {
            let error = world
                .register::<Posing>()
                .expect_err("another's description");
            assert_eq!(error.code(), "reflect.name_taken");
            let message = error.to_string();
            assert!(
                message.contains("by_name::tests::Posing")
                    && message.contains("transform::Transform"),
                "{message}"
            );
            world.register::<Transform>().expect("registered");
        }
        let names: Vec<&str> = world.component_types().collect();
        assert_eq!(names, ["Parent", "Health", "Armor", "Transform"]);
    }

    /// Describes itself as a transform, as a type that passes everything to its field might.
    #[derive(Clone)]
    struct Posing(Transform);

    impl Reflect for Posing {
        fn describe(types: &mut Types) -> Result<reflect::Type> {
            Transform::describe(types)
        }

        fn to_value(&self) -> Value {
            self.0.to_value()
        }

        fn from_value(value: &Value) -> Result<Self> {
            Transform::from_value(value).map(Self)
        }

        fn with_value(&self, value: &Value) -> Result<Self> {
            self.0.with_value(value).map(Self)
        }

        fn set_value(&mut self, value: &Value) -> Result<()> {
            self.0.set_value(value)
        }

        fn part(&self, index: u32) -> Option<&dyn Reflect> {
            self.0.part(index)
        }

        fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
            self.0.part_mut(index)
        }
    }

    impl Component for Posing {}
    impl crate::Writable for Posing {}

    /// Tools read a child's parent by name, but only the world sets it, whether the entity has one
    /// or not.
    #[test]
    fn a_parent_is_read_by_name_but_not_set() {
        let mut world = World::default();
        let [parent, child, root] = [0.0, 1.0, 2.0].map(|x| {
            let at = Transform {
                translation: Vec3::splat(x),
                ..Transform::IDENTITY
            };
            world.spawn(at)
        });
        // Refused from the start, before any entity has a parent.
        let early = world.set_field(root, "Parent", "0", &Value::Entity(parent));
        let early = early.expect_err("the world's to set");
        assert_eq!(early.code(), "reflect.read_only", "{early}");
        world.set_parent(child, Some(parent)).expect("no cycle");
        let read = world.get_field(child, "Parent", "0");
        assert_eq!(read.ok(), Some(Value::Entity(parent)));
        assert!(world.component_types().any(|name| name == "Parent"));
        let before = snapshot(&world);
        let whole = Value::Struct(vec![named("0", Value::Entity(root))]);
        for (entity, path, value) in [
            (child, "", &whole),
            (child, "0", &Value::Entity(root)),
            (root, "", &whole),
            (root, "0", &Value::Entity(child)),
        ] {
            let error = world.set_field(entity, "Parent", path, value);
            let error = error.expect_err("the world's to set");
            assert_eq!(error.code(), "reflect.read_only", "{error}");
            assert!(error.to_string().contains("World::set_parent"), "{error}");
        }
        assert!(same_snapshots(&snapshot(&world), &before));
    }

    #[test]
    #[should_panic(expected = "[reflect.name_taken] ")]
    fn spawning_a_type_named_as_another_panics() {
        let mut world = World::default();
        world.spawn(first::Health(1));
        world.spawn(second::Health(1.0));
    }

    #[derive(Clone, crate::Component)]
    struct A;

    #[derive(Clone, crate::Component)]
    struct B(u8);

    #[test]
    fn entities_are_listed_and_skipped_a_table_at_a_time() {
        let mut world = World::default();
        // Tables of A, of A and B, of none, of B and Transform, which is emptied, and of B, in
        // that order.
        for _ in 0..3 {
            world.spawn(A);
        }
        for i in 0..2 {
            world.spawn((A, B(i)));
        }
        world.spawn(());
        let gone = world.spawn((B(0), Transform::IDENTITY));
        for i in 0..4 {
            world.spawn(B(i));
        }
        world.despawn(gone).expect("alive");
        for with in [None, Some("A"), Some("B"), Some("Transform")] {
            let expected: Vec<Entity> = match with {
                None => world.query::<Entity>().collect(),
                Some("A") => world.query::<(Entity, &A)>().map(|(e, _)| e).collect(),
                Some("B") => world.query::<(Entity, &B)>().map(|(e, _)| e).collect(),
                _ => Vec::new(),
            };
            let listed = || world.entities(with).expect("a component type");
            assert_eq!(listed().len(), expected.len(), "{with:?}");
            assert_eq!(listed().collect::<Vec<_>>(), expected, "{with:?}");
            for first in 0..=expected.len() {
                let mut entities = listed();
                assert_eq!(
                    entities.nth(first),
                    expected.get(first).copied(),
                    "{with:?}"
                );
                let left = expected.len().saturating_sub(first + 1);
                assert_eq!(entities.len(), left, "{with:?} {first}");
                for second in 0..=left {
                    let mut entities = listed();
                    entities.nth(first);
                    let at = first + 1 + second;
                    assert_eq!(
                        entities.nth(second),
                        expected.get(at).copied(),
                        "{with:?} {at}"
                    );
                }
            }
        }
        let error = world.entities(Some("Nothing")).err().expect("no such type");
        assert_eq!(error.code(), "reflect.unknown_type");
        assert!(
            error.to_string().contains("`A`, `B` and `Transform`"),
            "{error}"
        );
    }
}
