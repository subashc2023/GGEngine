//! The world as tools reach it, without its components' Rust types: component types by name,
//! fields by path, values as reflection's [`Value`]s. The inspector is built on these, the world's
//! IDs and its hierarchy, and so will the tool API be.

use std::any::TypeId;
use std::slice;

use gg_core::{Error, Result};

use super::declared::{self, Declared, Kind};
use super::id::Named;
use super::table::{AnyColumn, ComponentId, Table};
use super::{Component, Entity, Location, Parent, World};
use crate::reflect::{self, Ident, Reflect, Resolved, Shape, Type, Types, Value};
use crate::{ModelNode, ModelRoot, PrefabRoot};

impl World {
    /// Registers component type `T` with reflection, if it hasn't registered. A type also
    /// registers when an entity first has one; call this to list a type before then.
    ///
    /// Fails with:
    ///
    /// - `ecs.registered_late` once the world has loaded mods ([`Mods`](crate::Mods)).
    /// - `reflect.name_taken`: another type has its name.
    /// - `reflect.unnamed`: it isn't described as a struct or enum.
    /// - `reflect.bad_default`: a field's default is outside its bounds, or opaque.
    /// - `ecs.bad_exclusive`: it names an exclusive field that isn't its own `bool`
    ///   ([`Component::exclusive`]).
    pub fn register<T: Component>(&mut self) -> Result<()> {
        self.components.register::<T>().map(drop)
    }

    /// Refuses, from now on, a Rust type that hasn't registered, as mods load in the world.
    pub(crate) fn mods_loaded(&mut self) {
        self.components.mods_loaded = true;
    }

    /// The descriptions of the component types, and of the types of their fields.
    pub fn types(&self) -> &Types {
        self.components.types()
    }

    /// The component types' names, in the order they registered.
    pub fn component_types(&self) -> impl ExactSizeIterator<Item = &str> {
        self.components.names()
    }

    /// The default of component type `component`: [`Types::default_value`], with what's opaque
    /// from [`Component::base`], so it works as an [`Operation::Add`](crate::Operation::Add)
    /// value. It's the same each call, unless a script redeclares the type.
    ///
    /// Fails with `reflect.unknown_type` if no component type is named `component`; with
    /// `default_value`'s errors, such as `reflect.no_default` for an entity; and with an add's,
    /// such as `reflect.opaque` for a type opaque as a whole without a `base`, like
    /// [`MeshDraw`](crate::MeshDraw). Every error names the component and the failing path.
    pub fn default_of(&self, component: &str) -> Result<Value> {
        let id = self.named(component)?;
        let about = |error: Error| {
            let (at, what) = reflect::located(&error);
            let message = format!("can't make a default `{at}`: {what}");
            error.with_message(message)
        };
        let ty = self.components.ty(id);
        let value = self.types().default_value(&ty, &reflect::Meta::default());
        let value = value.map_err(|error| about(reflect::within(error, component)))?;
        self.build(id, component, &value).map_err(about)?;
        Ok(value)
    }

    /// The living entities in query order, table by table; with `with`, only those with that
    /// component. Skipping skips whole tables: a page costs a step per table plus one per entity.
    ///
    /// Fails with `reflect.unknown_type` if no component type is named `with`.
    pub fn entities(&self, with: Option<&str>) -> Result<Entities<'_>> {
        let with = with.map(|name| self.named(name)).transpose()?;
        Ok(self.listing(with, None))
    }

    /// The living entities without a [`Parent`], ordered and skipped like
    /// [`entities`](Self::entities).
    pub fn roots(&self) -> Entities<'_> {
        self.listing(None, Some(self.parent_type()))
    }

    /// `entity`'s place among the [`roots`](Self::roots), found in a step per table; `None` if it
    /// isn't alive or is a child.
    pub fn root_index(&self, entity: Entity) -> Option<usize> {
        let at = self.location(entity)?;
        let parent = Some(self.parent_type());
        let table = at.table as usize;
        if !has(&self.tables[table], None, parent) {
            return None;
        }
        let roots = self.tables[..table].iter();
        let roots = roots.filter(|table| has(table, None, parent));
        let before: usize = roots.map(|table| table.entities().len()).sum();
        Some(before + at.row as usize)
    }

    /// The names of `entity`'s components' types, in the order they registered.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn components_of(&self, entity: Entity) -> Result<impl ExactSizeIterator<Item = &str>> {
        let at = self.locate(entity)?;
        let ids = self.tables[at.table as usize].components().iter();
        Ok(ids.map(|&id| self.components.name(id)))
    }

    /// The value at `path` in `entity`'s `component`, such as `translation.x` or `rect[1]`; an
    /// empty path is the whole component. A path is a field name, then `.name` for a struct's
    /// field, `.x` to `.w` for a vector's component, `[i]` for an array's or list's element. Paths
    /// don't go into enums, options or quaternions, which are set whole. What's opaque reads as
    /// [`Value::Opaque`].
    ///
    /// Fails with `ecs.no_entity` if `entity` isn't alive; `reflect.unknown_type` if no component
    /// type is named `component`; `reflect.missing_component` if `entity` lacks it;
    /// `reflect.bad_path` for an unreadable path; `reflect.no_field` for a path to no field. Every
    /// error names the entity (by ID while alive), the component and the path.
    pub fn get_field(&self, entity: Entity, component: &str, path: &str) -> Result<Value> {
        let name = self.name_of(entity);
        let about = |error: Error| about(error, "get", name, component, path);
        let (id, at) = self.find(entity, component).map_err(about)?;
        let column = self.column(at, id, component).map_err(about)?;
        let types = self.components.types();
        let to = reflect::resolve(types, &self.components.ty(id), path).map_err(about)?;
        let column = self.tables[at.table as usize].column_at(column);
        let row = at.row as usize;
        if to.ty.is_opaque(types) {
            reach(column, row, opaque_steps(&to)).map_err(about)?;
            return Ok(Value::Opaque);
        }
        Ok(match reach(column, row, &to.steps).map_err(about)? {
            Some(part) => part.to_value(),
            None => column.value(row),
        })
    }

    /// Sets the field at `path` in `entity`'s `component` and returns the old value, which sets it
    /// back (unless code stored one the checks below refuse, such as a number outside its bounds).
    /// Paths are as in [`get_field`](Self::get_field).
    ///
    /// All of `value` is checked before anything changes, so a refusal changes nothing: its kinds,
    /// its fields' bounds (a non-finite number is outside them), and unit length for a rotation.
    /// What's opaque is set only to [`Value::Opaque`], which keeps it as is.
    ///
    /// [`Parent`], [`ModelNode`] and [`PrefabRoot`] aren't set by name: the world keeps them.
    /// [`World::set_parent`] changes a `Parent`; the tool API's operations change an instance's
    /// link. A [`ModelRoot`] given another model gets that model's nodes in place of its own.
    ///
    /// Fails like `get_field`, and with `reflect.read_only` for `Parent`, `ModelNode` and
    /// `PrefabRoot` at any path, whether or not the entity has one; `reflect.out_of_range` for a
    /// number outside its field's bounds or its type's; and [`Reflect::with_value`]'s errors for a
    /// value that doesn't fit. Every error names the entity (by ID while alive), the component and
    /// the path down to the part refused.
    pub fn set_field(
        &mut self,
        entity: Entity,
        component: &str,
        path: &str,
        value: &Value,
    ) -> Result<Value> {
        self.write_field(entity, component, path, value, false)
    }

    /// [`set_field`](Self::set_field), but sets a [`PrefabRoot`] too if `relinks`, as operations
    /// and refits do; they respawn its instance's entities themselves.
    pub(super) fn write_field(
        &mut self,
        entity: Entity,
        component: &str,
        path: &str,
        value: &Value,
        relinks: bool,
    ) -> Result<Value> {
        // Named before the closure, which would still borrow the world at `row_mut`.
        let name = self.name_of(entity);
        let about = |error: Error| about(error, "set", name, component, path);
        let (id, at) = self.find(entity, component).map_err(about)?;
        if !(relinks && Some(id) == self.components.get(TypeId::of::<PrefabRoot>())) {
            self.settable(id).map_err(about)?;
        }
        let column = self.column(at, id, component).map_err(about)?;
        let types = self.components.types();
        let to = reflect::resolve(types, &self.components.ty(id), path).map_err(about)?;
        let row = at.row as usize;
        if to.ty.is_opaque(types) {
            let column = self.tables[at.table as usize].column_at(column);
            reach(column, row, opaque_steps(&to)).map_err(about)?;
            return match value {
                Value::Opaque => Ok(Value::Opaque),
                other => Err(about(reflect::not_opaque(other))),
            };
        }
        reflect::check(types, &to.ty, &to.meta, value).map_err(about)?;
        let tick = self.change_tick;
        let column = self.tables[at.table as usize].row_mut(column, row, tick);
        let old = match to.steps.split_first() {
            None => {
                let old = column.value(row);
                column.set_value(row, value).map_err(about)?;
                old
            }
            Some((&first, rest)) => {
                let field = column.part_mut(row, first).expect(FIELD);
                let field = reflect::reach_mut(field, rest).map_err(about)?;
                let old = field.to_value();
                field.set_value(value).map_err(about)?;
                old
            }
        };
        // Another model's nodes in place of its own.
        if Some(id) == self.components.get(TypeId::of::<ModelRoot>()) && old != *value {
            self.despawn_nodes(entity);
            self.fit_nodes(entity, None);
        }
        Ok(old)
    }

    /// Fails with `reflect.read_only` for [`Parent`], [`ModelNode`] and [`PrefabRoot`]; see
    /// [`set_field`](Self::set_field).
    fn settable(&self, id: ComponentId) -> Result<()> {
        if Some(id) == self.components.get(TypeId::of::<Parent>()) {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps it, so that the hierarchy stays whole; `World::set_parent` \
                 changes it",
            ));
        }
        if Some(id) == self.components.get(TypeId::of::<ModelNode>()) {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps it, as it spawns a model's nodes under the model's root",
            ));
        }
        if Some(id) == self.components.get(TypeId::of::<PrefabRoot>()) {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps it, as it spawns the prefab's entities under the instance's root; \
                 the tool API's operations change it",
            ));
        }
        Ok(())
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

    /// Gives `entity` a `component` at its [default](Self::default_of). This is how an entity gets
    /// a script to run: the component type the script declares, by the name
    /// [`Scripts::load`](crate::Scripts::load) returns.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive; `reflect.has_component` if it has one
    /// already; and `default_of`'s errors, such as `reflect.unknown_type`.
    pub fn add_default(&mut self, entity: Entity, component: &str) -> Result<()> {
        let value = self.default_of(component)?;
        let id = self.named(component)?;
        let at = self.locate(entity)?;
        if self.tables[at.table as usize].has(id) {
            return Err(Error::new(
                "reflect.has_component",
                format!(
                    "entity {} has a `{component}` already",
                    self.name_of(entity)
                ),
            ));
        }
        let column = self.build(id, component, &value)?;
        self.give(entity, at, id, column);
        Ok(())
    }

    /// Declares a script's component type; see [`Components::declare`].
    ///
    /// [`Components::declare`]: super::table::Components::declare
    pub(crate) fn declare(
        &mut self,
        name: &str,
        doc: &str,
        fields: Vec<(Ident, Kind, Value)>,
    ) -> Result<ComponentId> {
        self.components.declare(name, doc, fields)
    }

    /// Redeclares script type `id` ([`Components::redeclare`]) and migrates each of its components
    /// to the new fields ([`declared::migrate`]).
    ///
    /// Fails like `redeclare`, changing nothing.
    ///
    /// [`Components::redeclare`]: super::table::Components::redeclare
    pub(crate) fn redeclare(
        &mut self,
        id: ComponentId,
        doc: &str,
        fields: Vec<(Ident, Kind, Value)>,
    ) -> Result<()> {
        let declared = self.components.redeclare(id, doc, fields)?;
        let tick = self.change_tick;
        for table in &mut self.tables {
            if let Some(at) = table.position(id) {
                declared::migrate(table.column_at_mut(at, tick), &declared);
            }
        }
        Ok(())
    }

    /// See [`Components::revision`].
    ///
    /// [`Components::revision`]: super::table::Components::revision
    pub(crate) fn types_revision(&self) -> u64 {
        self.components.revision()
    }

    pub(crate) fn component_named(&self, name: &str) -> Option<ComponentId> {
        self.components.named(name)
    }

    /// The names and kinds of declared type `id`'s fields; `None` if it's a Rust type.
    pub(crate) fn declared(&self, id: ComponentId) -> Option<&Declared> {
        self.components.declared(id).map(|declared| &**declared)
    }

    /// The fields of component type `id`; none if it isn't a struct with named fields.
    pub(crate) fn fields_of(&self, id: ComponentId) -> &[reflect::Field] {
        let Type::Named(index) = self.components.ty(id) else {
            return &[];
        };
        match &self.types().info(index).shape {
            Shape::Struct {
                fields,
                tuple: false,
            } => fields,
            _ => &[],
        }
    }

    /// The doc of component type `id`; a declared type's names its script.
    pub(crate) fn doc_of(&self, id: ComponentId) -> &str {
        match self.components.ty(id) {
            Type::Named(index) => &self.types().info(index).doc,
            _ => "",
        }
    }

    /// Field `index` of `entity`'s component `id`, a struct with named fields. Doesn't allocate:
    /// scripts read fields every tick.
    ///
    /// Fails with `ecs.no_entity` if `entity` isn't alive, `reflect.missing_component` if it lacks
    /// the component.
    pub(crate) fn field_at(
        &self,
        entity: Entity,
        id: ComponentId,
        index: u32,
    ) -> Result<&dyn Reflect> {
        let (table, row) = self.row_of(entity, id)?;
        let table = &self.tables[table];
        let column = table.column_at(table.position(id).expect(HAS));
        Ok(column.part(row, index).expect(FIELD))
    }

    /// Field `index` of `entity`'s component `id`, a struct with named fields, to be set to
    /// `value`, checked like [`set_field`](Self::set_field). Doesn't allocate unless it's refused:
    /// scripts set fields every tick. To avoid allocating a `Value`, `value` is `None` for an
    /// entity or none (no bounds apply), set as an `Option<Entity>`; for an option's field, a
    /// value of the inner type stands for `Some` of it, checked as the option's would be.
    ///
    /// Fails like [`field_at`](Self::field_at), and with `reflect.read_only` for `Parent`,
    /// `ModelNode` and `PrefabRoot`; `reflect.out_of_range` for a number outside its field's bounds
    /// (non-finite is outside).
    pub(crate) fn field_to_set(
        &mut self,
        entity: Entity,
        id: ComponentId,
        index: u32,
        value: Option<&Value>,
    ) -> Result<&mut dyn Reflect> {
        self.settable(id)?;
        let (table, row) = self.row_of(entity, id)?;
        if let Some(value) = value {
            let field = &self.fields_of(id)[index as usize];
            let ty = match (&field.ty, value) {
                (Type::Option(inner), value) if !matches!(value, Value::Option(_)) => &**inner,
                (ty, _) => ty,
            };
            reflect::check(self.components.types(), ty, &field.meta, value)?;
        }
        let tick = self.change_tick;
        let table = &mut self.tables[table];
        let column = table.row_mut(table.position(id).expect(HAS), row, tick);
        Ok(column.part_mut(row, index).expect(FIELD))
    }

    fn row_of(&self, entity: Entity, id: ComponentId) -> Result<(usize, usize)> {
        let at = self.location(entity).ok_or_else(|| {
            Error::new(
                "ecs.no_entity",
                "the entity isn't in the world: it was despawned, or is another world's",
            )
        })?;
        if !self.tables[at.table as usize].has(id) {
            return Err(Error::new(
                "reflect.missing_component",
                format!("the entity has no `{}`", self.components.name(id)),
            ));
        }
        Ok((at.table as usize, at.row as usize))
    }

    pub(crate) fn has(&self, entity: Entity, id: ComponentId) -> bool {
        self.location(entity)
            .is_some_and(|at| self.tables[at.table as usize].has(id))
    }

    /// Fills `list` (the world's [kept list](World::take_list)) with the entities with component
    /// `id`, in query order. Doesn't allocate once the list has grown.
    pub(crate) fn list_with(&self, id: ComponentId, list: &mut Vec<Entity>) {
        list.clear();
        let tables = self.tables.iter().filter(|table| table.has(id));
        list.extend(tables.flat_map(|table| table.entities()));
    }

    fn find(&self, entity: Entity, component: &str) -> Result<(ComponentId, Location)> {
        let at = self.location(entity).ok_or_else(|| {
            Error::new(
                "ecs.no_entity",
                "the entity isn't in the world: it was despawned, or is another world's",
            )
        })?;
        Ok((self.named(component)?, at))
    }

    pub(super) fn column(&self, at: Location, id: ComponentId, component: &str) -> Result<usize> {
        let table = &self.tables[at.table as usize];
        let Some(column) = table.position(id) else {
            let names = table
                .components()
                .iter()
                .map(|&id| self.components.name(id));
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

    fn parent_type(&self) -> ComponentId {
        let id = self.component::<Parent>();
        id.expect("every world has registered `Parent`")
    }

    fn listing(&self, with: Option<ComponentId>, without: Option<ComponentId>) -> Entities<'_> {
        let len = self
            .tables
            .iter()
            .filter(|table| has(table, with, without))
            .map(|table| table.entities().len())
            .sum();
        Entities {
            tables: self.tables.iter(),
            with,
            without,
            rows: slice::Iter::default(),
            len,
        }
    }
}

/// `error` from `doing` (getting, setting) the field at `path` of `entity`'s `component`, naming
/// them and the whole path down to where it happened.
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

const FIELD: &str = "a resolved path's first step is a field of the component";

const HAS: &str = "the table has the component";

/// The part of `row` of `column` that the resolved path `steps` leads to; `None` for the whole row,
/// which a declared type has no Rust value for.
///
/// Fails with `reflect.no_field` at an index past a list's end.
fn reach<'a>(
    column: &'a dyn AnyColumn,
    row: usize,
    steps: &[u32],
) -> Result<Option<&'a dyn Reflect>> {
    let Some((&first, rest)) = steps.split_first() else {
        return Ok(None);
    };
    let field = column.part(row, first).expect(FIELD);
    reflect::reach(field, rest).map(Some)
}

/// The steps to the opaque part `to` leads to: an opaque field isn't a part, so its struct is.
fn opaque_steps(to: &Resolved) -> &[u32] {
    match to.ty {
        Type::Opaque => to.steps.split_last().map_or(&[][..], |(_, before)| before),
        _ => &to.steps,
    }
}

fn has(table: &Table, with: Option<ComponentId>, without: Option<ComponentId>) -> bool {
    with.is_none_or(|id| table.has(id)) && without.is_none_or(|id| !table.has(id))
}

/// The entities [`World::entities`] and [`World::roots`] list, table by table.
pub struct Entities<'w> {
    tables: slice::Iter<'w, Table>,
    with: Option<ComponentId>,
    without: Option<ComponentId>,
    rows: slice::Iter<'w, Entity>,
    len: usize,
}

impl Entities<'_> {
    /// Moves to the next listed table; whether there was one.
    fn next_table(&mut self) -> bool {
        let (with, without) = (self.with, self.without);
        match self.tables.find(|table| has(table, with, without)) {
            Some(table) => {
                self.rows = table.entities().iter();
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

    /// Skips whole tables: a step per table.
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
    use gg_core::math::{DVec3, Quat, Vec2, Vec4};
    use gg_renderer::{Mipmaps, Textures};

    use super::*;
    use crate::reflect::tests::{
        Everything, Form, Handle, Inner, WithOpaque, everything, same, same_snapshots, snapshot,
    };
    use crate::{MeshDraw, Reflect, Sampler, Sprite, Transform};

    fn named(name: &str, value: Value) -> (reflect::Ident, Value) {
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
        assert_eq!(x.expect("a field"), Value::F64(0.0));
        let old = world.set_field(entity, "Transform", "translation.x", &Value::F64(2.5));
        assert_eq!(old.expect("set"), Value::F64(0.0));
        let transform = world.get::<Transform>(entity).expect("a transform");
        assert_eq!(transform.translation, DVec3::new(2.5, 0.0, 0.0));

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

    /// A sprite's texture reads as its handle, set whole or alone; none is the white texture.
    #[test]
    fn a_sprite_reads_and_sets_its_texture() {
        let texture = Textures::default().add("first", [1, 1], &[0; 4], Mipmaps::None);
        let texture = texture.expect("a texture");
        let mut world = World::default();
        let sprite = Sprite {
            texture: Some(texture),
            ..Sprite::default()
        };
        let entity = world.spawn(sprite);
        let Value::Struct(mut fields) = world.get_field(entity, "Sprite", "").expect("a sprite")
        else {
            panic!("a sprite is a struct");
        };
        let shown = Value::Option(Some(Box::new(Value::Texture(texture))));
        assert_eq!(fields[1], named("texture", shown.clone()));
        fields[0].1 = Value::Vec4(Vec4::new(0.5, 0.5, 0.5, 1.0));
        world
            .set_field(entity, "Sprite", "", &Value::Struct(fields))
            .expect("set");
        let set = world.get::<Sprite>(entity).expect("a sprite");
        assert_eq!(
            (set.texture, set.color),
            (Some(texture), Vec4::new(0.5, 0.5, 0.5, 1.0))
        );
        let white = world.set_field(entity, "Sprite", "texture", &Value::Option(None));
        assert_eq!(white.expect("set"), shown);
        let set = world.get::<Sprite>(entity).expect("a sprite");
        assert_eq!(set.texture, None);
        world
            .set_field(entity, "Sprite", "texture", &shown)
            .expect("set");
        let sampler = Value::Variant("NearestRepeat".into(), Vec::new());
        world
            .set_field(entity, "Sprite", "sampler", &sampler)
            .expect("set");
        let set = world.get::<Sprite>(entity).expect("a sprite");
        assert_eq!(
            (set.texture, set.sampler),
            (Some(texture), Sampler::NearestRepeat)
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
                "reflect.wrong_kind",
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
    fn a_type_without_a_name_of_its_own_is_refused() {
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
        // Nor does a type take another's description and name, whichever registers first.
        for _ in 0..2 {
            let error = world
                .register::<Posing<Transform>>()
                .expect_err("another's description");
            assert_eq!(error.code(), "reflect.name_taken");
            let message = error.to_string();
            // Posing's own name holds Transform's path, but only the owner's ends at a backtick.
            assert!(
                message.contains("by_name::tests::Posing")
                    && message.contains("transform::Transform`"),
                "{message}"
            );
            world.register::<Transform>().expect("registered");
        }
        // Nor does a type describe itself as something other than a struct or an enum, which
        // reflection names.
        let error = world
            .register::<Posing<u32>>()
            .expect_err("a description without a name");
        assert_eq!(error.code(), "reflect.unnamed", "{error}");
        let names: Vec<&str> = world.component_types().collect();
        assert_eq!(names, ["Parent", "Health", "Armor", "Transform"]);
    }

    /// Describes itself as what it holds, as a type that passes everything to its field might.
    #[derive(Clone)]
    struct Posing<T>(T);

    impl<T: Reflect> Reflect for Posing<T> {
        fn describe(types: &mut Types) -> Result<reflect::Type> {
            T::describe(types)
        }

        fn to_value(&self) -> Value {
            self.0.to_value()
        }

        fn from_value(value: &Value) -> Result<Self> {
            T::from_value(value).map(Self)
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

        fn digest(&self, to: &mut reflect::Digest<'_>) {
            self.0.digest(to);
        }

        fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity)) {
            self.0.entities_mut(each);
        }
    }

    impl<T: Reflect + Clone> Component for Posing<T> {}
    impl<T: Reflect + Clone> crate::Writable for Posing<T> {}

    /// Tools read a child's parent by name, but only the world sets it, even if the entity has
    /// none.
    #[test]
    fn a_parent_is_read_by_name_but_not_set() {
        let mut world = World::default();
        let [parent, child, root] = [0.0, 1.0, 2.0].map(|x| {
            let at = Transform {
                translation: DVec3::splat(x),
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

    /// Panics unless `world`'s roots are its parentless living entities in query order, each at
    /// its place when skipped to, and `gone` and the children aren't among them.
    fn check_roots(world: &World, gone: &[Entity]) {
        let living: Vec<Entity> = world.query::<Entity>().collect();
        let expected: Vec<Entity> = living
            .iter()
            .copied()
            .filter(|&entity| world.get::<Parent>(entity).is_none())
            .collect();
        assert_eq!(world.roots().len(), expected.len());
        assert_eq!(world.roots().collect::<Vec<_>>(), expected);
        for first in 0..=expected.len() {
            let mut roots = world.roots();
            assert_eq!(roots.nth(first), expected.get(first).copied(), "{first}");
            assert_eq!(roots.len(), expected.len().saturating_sub(first + 1));
        }
        for entity in living {
            let at = expected.iter().position(|&root| root == entity);
            assert_eq!(world.root_index(entity), at, "{entity}");
        }
        for &gone in gone {
            assert_eq!(world.root_index(gone), None, "{gone}");
        }
    }

    #[test]
    fn roots_are_listed_and_placed_a_table_at_a_time() {
        let mut world = World::default();
        check_roots(&world, &[]);
        // Tables of A, of A's children, which sit between tables of roots, of A and B, and of B.
        let a: Vec<Entity> = (0..4).map(|_| world.spawn(A)).collect();
        world.set_parent(a[1], Some(a[0])).expect("no cycle");
        let both: Vec<Entity> = (0..3).map(|i| world.spawn((A, B(i)))).collect();
        let b: Vec<Entity> = (0..2).map(|i| world.spawn(B(i))).collect();
        world.set_parent(b[1], Some(both[2])).expect("no cycle");
        check_roots(&world, &[]);
        assert_eq!(world.parent(b[1]).expect("alive"), Some(both[2]));
        // A root despawned from the middle of its table, whose last row takes its place.
        world.despawn(both[0]).expect("alive");
        check_roots(&world, &[both[0]]);
        // A root moved to another table's end, and a child made a root again.
        world.insert(a[2], B(9)).expect("alive");
        world.set_parent(a[1], None).expect("a root");
        check_roots(&world, &[both[0]]);
        world.remove::<B>(a[2]).expect("alive");
        check_roots(&world, &[both[0]]);
        // A parent despawned takes its child with it.
        world.despawn(both[2]).expect("alive");
        check_roots(&world, &[both[0], both[2], b[1]]);
        assert_eq!(world.parent(a[0]).expect("alive"), None);
        let error = world.parent(b[1]).expect_err("despawned");
        assert_eq!(error.code(), "ecs.no_entity");
    }
}
