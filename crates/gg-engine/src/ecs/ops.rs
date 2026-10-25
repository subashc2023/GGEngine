//! Operations: the authoring changes people and agents make to a world, which name its entities by
//! their IDs. Each, applied, gives what undoes it, its inverse: another operation, or, for a delete
//! and a remove, what they took out, typed, so that what's opaque comes back too. A delete's keeps
//! its entities' slots, so that undoing it gives each back under its handle, and every reference to
//! it is whole again. Only the [`Log`](super::Log) holds inverses, and it releases what they keep.

use std::{fmt, mem};

use gg_core::{Error, Result};

use super::scene::NO_HINTS;
use super::table::{AnyColumn, ComponentId, Row};
use super::{Entity, EntityId, Parent, World};
use crate::reflect::{self, Name, Value};

/// An authoring change to a world, by a person or an agent, which a [`Log`](super::Log) applies,
/// and undoes. It names entities by their IDs, and carries values as reflection's, whose entities
/// are handles, as [`World::set_field`] takes one.
///
/// Applying one checks everything before it changes anything, so that a refusal changes nothing.
/// Each is refused with `ecs.no_entity` for an entity it changes, or a parent it gives, that no
/// living entity has. A create, an add and a remove are refused with `reflect.unknown_type` for a
/// component type that isn't registered, and with `reflect.read_only` for [`Parent`], which a
/// create's parent and a reparent give. A create and an add are refused with
/// `reflect.has_component` for a component the entity has, or a create gives twice; as `set_field`
/// refuses a value; and as making the component from it on its type's
/// [`base`](crate::Component::base) is, such as with `reflect.opaque` for a type that's opaque as
/// a whole, as [`MeshDraw`](crate::MeshDraw) is. A create is refused with `ecs.id_taken` for an ID
/// a living entity has; a remove with `reflect.missing_component`; a set as `set_field` refuses
/// it; and a reparent as [`World::set_parent`] refuses it.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Spawns an entity with `components`, each a type's name and its whole value, made as a
    /// scene's is, its opaque parts given as [`Value::Opaque`]: the last of `parent`'s children,
    /// or a root. Its ID is `entity`, or one drawn if that's `None`, which applying it writes in.
    Create {
        entity: Option<EntityId>,
        parent: Option<EntityId>,
        components: Vec<(Name, Value)>,
    },
    /// Despawns `entity` and its descendants.
    Delete { entity: EntityId },
    /// Gives `entity` `component`, which it hasn't, made from `value` as a scene's is.
    Add {
        entity: EntityId,
        component: Name,
        value: Value,
    },
    /// Takes `entity`'s `component` away.
    Remove { entity: EntityId, component: Name },
    /// Sets the field at `path` of `entity`'s `component` to `value`, as `set_field` does.
    Set {
        entity: EntityId,
        component: Name,
        path: String,
        value: Value,
    },
    /// Makes `entity` the last of `parent`'s children, or a root, as `set_parent` does.
    Reparent {
        entity: EntityId,
        parent: Option<EntityId>,
    },
}

impl Operation {
    /// The entity it changes or creates; `None` for a create given no ID, until it's applied.
    pub(super) fn entity(&self) -> Option<EntityId> {
        match self {
            Self::Create { entity, .. } => *entity,
            Self::Delete { entity }
            | Self::Add { entity, .. }
            | Self::Remove { entity, .. }
            | Self::Set { entity, .. }
            | Self::Reparent { entity, .. } => Some(*entity),
        }
    }
}

/// The operation in words, as menus and diagnostics name it, such as `set
/// `Transform.scale` of 2b7e151628aed2a6`; without its values.
impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create {
                entity: Some(entity),
                ..
            } => write!(f, "create {entity}"),
            Self::Create { entity: None, .. } => f.write_str("create an entity"),
            Self::Delete { entity } => write!(f, "delete {entity}"),
            Self::Add {
                entity, component, ..
            } => write!(f, "add `{component}` to {entity}"),
            Self::Remove { entity, component } => write!(f, "remove `{component}` from {entity}"),
            Self::Set {
                entity,
                component,
                path,
                ..
            } => write!(f, "set `{}` of {entity}", reflect::join(component, path)),
            Self::Reparent {
                entity,
                parent: Some(parent),
            } => write!(f, "make {entity} a child of {parent}"),
            Self::Reparent {
                entity,
                parent: None,
            } => write!(f, "make {entity} a root"),
        }
    }
}

/// What undoes an operation applied to a world, which [`World::undo`] applies. It's its world's
/// own, since its values hold the world's handles and its rows its component types, and a world
/// restored from a snapshot is another, which may have given those handles out again; a delete's
/// keeps its entities' slots until it's applied or [released](World::release).
pub(super) struct Inverse {
    /// Its world's serial.
    world: u64,
    undo: Undo,
}

enum Undo {
    Apply(Operation),
    /// Gives `entity` back the component a remove took, in a one-row column of its own.
    Put {
        entity: EntityId,
        component: ComponentId,
        column: Box<dyn AnyColumn>,
    },
    /// Puts back the entities a delete took, in the order it took them: children before their
    /// parents, each parent's first to last, and the root last.
    Restore(Vec<Taken>),
}

/// An entity a delete took out of the world.
struct Taken {
    /// Its handle, which it has again as it comes back: its slot is kept for it.
    entity: Entity,
    id: EntityId,
    parent: Option<Entity>,
    /// Its components, its `Parent` among them.
    row: Row,
}

impl World {
    /// Applies `operation`, and returns its inverse; a create given no ID is given the one it
    /// drew. Refused, it changes nothing, as [`Operation`] says.
    pub(super) fn apply(&mut self, operation: &mut Operation) -> Result<Inverse> {
        let _zone = gg_core::zone!();
        let undo = self.applied(operation)?;
        // So that the log holds the ID drawn, which a replay creates the entity with.
        if let (Operation::Create { entity, .. }, Undo::Apply(Operation::Delete { entity: id })) =
            (operation, &undo)
        {
            *entity = Some(*id);
        }
        Ok(Inverse {
            world: self.serial,
            undo,
        })
    }

    /// Applies `inverse`, undoing its operation, and leaves in its place what undoes that, which
    /// redoes it. Refused, it changes neither the world nor `inverse`: with `ops.other_world` for
    /// another world's inverse, as one from before the world was restored is; as its operation is
    /// refused, if it's one; and, as it gives back what a remove or a delete took, with
    /// `ecs.no_entity` for an entity gone since, or a root's parent gone, `reflect.has_component`
    /// for a component the entity has since, and `ecs.id_taken` for an ID a living entity has
    /// since, such as one a scene loaded.
    pub(super) fn undo(&mut self, inverse: &mut Inverse) -> Result<()> {
        let _zone = gg_core::zone!();
        if inverse.world != self.serial {
            return Err(Error::new(
                "ops.other_world",
                "it's another world's, or from before the world was restored: what undoes an \
                 operation undoes it in the world it was applied to",
            ));
        }
        let redo = match &inverse.undo {
            Undo::Apply(operation) => self.applied(operation)?,
            Undo::Put {
                entity, component, ..
            } => {
                self.can_put(*entity, *component)?;
                let name = self.components.name(*component);
                Undo::Apply(Operation::Remove {
                    entity: *entity,
                    component: name.to_owned().into(),
                })
            }
            Undo::Restore(taken) => {
                self.can_restore(taken)?;
                let root = taken.last().expect("a delete takes its entity");
                Undo::Apply(Operation::Delete { entity: root.id })
            }
        };
        // Checked, so that what the inverse kept goes back, which can't fail.
        match mem::replace(&mut inverse.undo, redo) {
            Undo::Apply(_) => {}
            Undo::Put {
                entity,
                component,
                column,
            } => {
                let entity = self.entity(entity).expect("checked alive");
                let at = self.location(entity).expect("alive");
                self.give(entity, at, component, column);
            }
            Undo::Restore(taken) => self.restore(taken),
        }
        Ok(())
    }

    /// Drops `inverse`, which won't be applied: if it's this world's, the slots a delete's keeps
    /// are freed, so that its entities are gone for good.
    pub(super) fn release(&mut self, inverse: Inverse) {
        if inverse.world != self.serial {
            return;
        }
        if let Undo::Restore(taken) = inverse.undo {
            for taken in taken {
                self.free_slot(taken.entity.index);
            }
        }
    }

    /// Applies `operation`, and returns what undoes it.
    fn applied(&mut self, operation: &Operation) -> Result<Undo> {
        let refused = |error: Error| {
            let message = format!("can't {operation}: {error}");
            error.with_message(message)
        };
        Ok(match operation {
            Operation::Create {
                entity,
                parent,
                components,
            } => {
                let id = self.create(*entity, *parent, components).map_err(refused)?;
                Undo::Apply(Operation::Delete { entity: id })
            }
            Operation::Delete { entity } => {
                let root = self.living(*entity).map_err(refused)?;
                let mut taken = Vec::new();
                self.take_apart(root, |world, entity, parent| {
                    let id = world.id(entity).expect("alive");
                    let row = world.vacate(entity.index);
                    taken.push(Taken {
                        entity,
                        id,
                        parent,
                        row,
                    });
                });
                Undo::Restore(taken)
            }
            Operation::Add {
                entity,
                component,
                value,
            } => {
                let handle = self.living(*entity).map_err(refused)?;
                let at = self.location(handle).expect("alive");
                let id = self.writable_type(component).map_err(refused)?;
                if self.tables[at.table as usize].has(id) {
                    return Err(refused(Error::new(
                        "reflect.has_component",
                        format!(
                            "it has a `{component}` already, which a set of the whole of it \
                             changes"
                        ),
                    )));
                }
                let column = self.build(id, component, value).map_err(refused)?;
                self.give(handle, at, id, column);
                Undo::Apply(Operation::Remove {
                    entity: *entity,
                    component: component.clone(),
                })
            }
            Operation::Remove { entity, component } => {
                let handle = self.living(*entity).map_err(refused)?;
                let at = self.location(handle).expect("alive");
                let id = self.writable_type(component).map_err(refused)?;
                self.column(at, id, component).map_err(refused)?;
                let column = self.take_out(handle, at, id);
                Undo::Put {
                    entity: *entity,
                    component: id,
                    column,
                }
            }
            Operation::Set {
                entity,
                component,
                path,
                value,
            } => {
                let handle = self.living(*entity).map_err(refused)?;
                let old = self.set_field(handle, component, path, value)?;
                Undo::Apply(Operation::Set {
                    entity: *entity,
                    component: component.clone(),
                    path: path.clone(),
                    value: old,
                })
            }
            Operation::Reparent { entity, parent } => {
                let child = self.living(*entity).map_err(refused)?;
                let parent = parent.map(|parent| self.living(parent));
                let parent = parent.transpose().map_err(refused)?;
                let old = self.set_parent(child, parent)?;
                let old = old.map(|old| self.id(old).expect("a parent is alive"));
                Undo::Apply(Operation::Reparent {
                    entity: *entity,
                    parent: old,
                })
            }
        })
    }

    /// Spawns the entity a create makes, and returns its ID.
    fn create(
        &mut self,
        id: Option<EntityId>,
        parent: Option<EntityId>,
        components: &[(Name, Value)],
    ) -> Result<EntityId> {
        if let Some(id) = id
            && self.by_id.contains_key(&id)
        {
            return Err(Error::new(
                "ecs.id_taken",
                format!("entity {id} is in the world already"),
            ));
        }
        let parent = parent.map(|parent| self.living(parent)).transpose()?;
        let mut row = Vec::with_capacity(components.len() + 1);
        for (name, value) in components {
            let component = self.writable_type(name)?;
            if row.iter().any(|&(other, _)| other == component) {
                return Err(Error::new(
                    "reflect.has_component",
                    format!("it's given `{name}` twice, but an entity has one of each component"),
                ));
            }
            row.push((component, self.build(component, name, value)?));
        }
        if let Some(parent) = parent {
            let id = self.component::<Parent>();
            let id = id.expect("every world has registered `Parent`");
            let column: Box<dyn AnyColumn> = Box::new(vec![Parent(parent)]);
            row.push((id, column));
        }
        row.sort_unstable_by_key(|&(component, _)| component);
        // Only once nothing can refuse, so that a refusal draws no ID.
        let entity = self.take_slot();
        let id = match id {
            Some(id) => {
                self.by_id.insert(id, entity);
                id
            }
            None => self.draw_id(entity),
        };
        self.place(entity, id, row);
        if let Some(parent) = parent {
            self.append(parent.index, entity.index);
        }
        Ok(id)
    }

    /// Whether the component a remove took, `component`, can go back on `entity`.
    fn can_put(&self, entity: EntityId, component: ComponentId) -> Result<()> {
        let name = self.components.name(component);
        let refused = |error: Error| {
            let message = format!("can't give {entity} its `{name}` back: {error}");
            error.with_message(message)
        };
        let handle = self.living(entity).map_err(refused)?;
        let at = self.location(handle).expect("alive");
        if self.tables[at.table as usize].has(component) {
            return Err(refused(Error::new(
                "reflect.has_component",
                format!("it has a `{name}` again"),
            )));
        }
        Ok(())
    }

    /// Whether the entities a delete took, `taken`, can come back.
    fn can_restore(&self, taken: &[Taken]) -> Result<()> {
        let root = taken.last().expect("a delete takes its entity");
        let refused = |error: Error| {
            let message = format!("can't put entity {} back: {error}", root.id);
            error.with_message(message)
        };
        for taken in taken {
            let meta = &self.meta[taken.entity.index as usize];
            assert!(
                meta.location.is_none() && meta.generation == taken.entity.generation,
                "a slot a delete vacated is freed only with its inverse"
            );
            if self.by_id.contains_key(&taken.id) {
                return Err(refused(Error::new(
                    "ecs.id_taken",
                    format!("entity {} is in the world again", taken.id),
                )));
            }
        }
        if let Some(parent) = root.parent
            && !self.contains(parent)
        {
            return Err(refused(Error::new(
                "ecs.no_entity",
                format!("its parent {parent} isn't in the world"),
            )));
        }
        Ok(())
    }

    /// Puts the entities a delete took, `taken`, back, each into its slot under its handle.
    fn restore(&mut self, taken: Vec<Taken>) {
        let mut links = Vec::with_capacity(taken.len());
        for Taken {
            entity,
            id,
            parent,
            row,
        } in taken
        {
            self.by_id.insert(id, entity);
            self.place(entity, id, row);
            if let Some(parent) = parent {
                links.push((parent.index, entity.index));
            }
        }
        // In the order the delete took them, so that each parent's children are as they were, and
        // the root is the last of its parent's.
        for (parent, child) in links {
            self.append(parent, child);
        }
    }

    /// The living entity whose ID is `id`.
    fn living(&self, id: EntityId) -> Result<Entity> {
        self.entity(id)
            .ok_or_else(|| Error::new("ecs.no_entity", format!("no entity has ID {id}")))
    }

    /// The id of the component type named `name`, which isn't [`Parent`].
    fn writable_type(&self, name: &str) -> Result<ComponentId> {
        let id = self.named(name)?;
        if Some(id) == self.component::<Parent>() {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps `Parent`, so that the hierarchy stays whole: a create's parent \
                 and a reparent give it",
            ));
        }
        Ok(id)
    }

    /// Component `id`, of the type named `name`, made from `value` as a scene's is, in a one-row
    /// column; checked as `set_field` checks a value.
    fn build(&self, id: ComponentId, name: &str, value: &Value) -> Result<Box<dyn AnyColumn>> {
        let at = |error: Error| {
            let (inner, what) = reflect::located(&error);
            let message = format!("at `{}`: {what}", reflect::join(name, &inner));
            error.with_message(message)
        };
        let ty = self.components.ty(id);
        reflect::check(self.components.types(), &ty, &NO_HINTS, value).map_err(at)?;
        let mut column = self.components.new_column(id);
        column.push_value(value).map_err(at)?;
        Ok(column)
    }
}

#[cfg(test)]
impl Inverse {
    /// How many slots it keeps.
    pub(super) fn kept(&self) -> usize {
        match &self.undo {
            Undo::Restore(taken) => taken.len(),
            Undo::Apply(_) | Undo::Put { .. } => 0,
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use gg_core::math::Vec3;
    use gg_renderer::{Mipmaps, Texture, Textures};

    use super::*;
    use crate::ecs::hierarchy::tests::check_links;
    use crate::ecs::id::tests::check_ids;
    use crate::ecs::scene::tests::unchanged;
    use crate::reflect::tests::{Everything, Handle, everything, same_snapshots, snapshot};
    use crate::{Reflect, Sprite, Transform};

    /// Opaque as a whole, as a [`MeshDraw`](crate::MeshDraw) is: a world keeps one, but no value
    /// makes one.
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    #[reflect(opaque)]
    pub(in crate::ecs) struct Sealed(pub(in crate::ecs) u32);

    /// A seeded world's entities, the same each time it's made: `r`, a root, whose children are
    /// `a` and then `s`; `a`, with a sprite of a texture of its own, whose children are `b`, `c`
    /// and `d`; `e`, `b`'s child; `f`, a root whose `Everything` targets `e` and keeps
    /// `Handle(99)`; and `g`, a root that's `Sealed` alone.
    pub(in crate::ecs) struct Fixture {
        pub(in crate::ecs) world: World,
        pub(in crate::ecs) r: Entity,
        pub(in crate::ecs) a: Entity,
        pub(in crate::ecs) s: Entity,
        pub(in crate::ecs) b: Entity,
        pub(in crate::ecs) c: Entity,
        pub(in crate::ecs) d: Entity,
        pub(in crate::ecs) e: Entity,
        pub(in crate::ecs) f: Entity,
        pub(in crate::ecs) g: Entity,
        pub(in crate::ecs) texture: Texture,
        /// Each entity's ID, which a dead one's handle no longer gives.
        ids: Vec<(Entity, EntityId)>,
    }

    impl Fixture {
        /// The ID of `entity`, one of the fixture's, alive or not.
        pub(in crate::ecs) fn id(&self, entity: Entity) -> EntityId {
            let found = self.ids.iter().find(|&&(other, _)| other == entity);
            found.expect("one of the fixture's").1
        }
    }

    pub(in crate::ecs) fn fixture() -> Fixture {
        let mut world = World::seeded(0x0905);
        let registered = [
            world.register::<Transform>(),
            world.register::<Sprite>(),
            world.register::<Everything>(),
            world.register::<Sealed>(),
        ];
        registered
            .into_iter()
            .for_each(|done| done.expect("registered"));
        let texture = Textures::default().add("own", [1, 1], &[255, 0, 0, 255], Mipmaps::None);
        let texture = texture.expect("a texture");
        let at = |x: f32| Transform {
            translation: Vec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let r = world.spawn(at(0.0));
        let sprite = Sprite {
            texture,
            ..Sprite::default()
        };
        let a = world.spawn((at(1.0), sprite));
        let s = world.spawn(at(2.0));
        let [b, c, d, e] = [3.0, 4.0, 5.0, 6.0].map(|x| world.spawn(at(x)));
        for (child, parent) in [(a, r), (s, r), (b, a), (c, a), (d, a), (e, b)] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let f = world.spawn(Everything {
            target: Some(e),
            handle: Handle(99),
            ..everything()
        });
        let g = world.spawn(Sealed(5));
        let ids =
            [r, a, s, b, c, d, e, f, g].map(|entity| (entity, world.id(entity).expect("alive")));
        Fixture {
            world,
            r,
            a,
            s,
            b,
            c,
            d,
            e,
            f,
            g,
            texture,
            ids: ids.to_vec(),
        }
    }

    /// Every component of every entity of `world`, by entity and then by component, whichever
    /// tables they're in.
    pub(in crate::ecs) fn sorted(world: &World) -> Vec<(Entity, EntityId, String, Value)> {
        let mut all = snapshot(world);
        all.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
        all
    }

    fn children(world: &World, entity: Entity) -> Vec<Entity> {
        world.children(entity).expect("alive").collect()
    }

    fn named(name: &'static str, value: Value) -> (Name, Value) {
        (name.into(), value)
    }

    /// Applies `operation`, then undoes it, redoes it and undoes it again, checking the world
    /// against what it was before it and after it, each entity by its handle; and returns what
    /// redoes it.
    fn round_trip(world: &mut World, mut operation: Operation) -> Inverse {
        let said = operation.to_string();
        let before = sorted(world);
        let inverse = world.apply(&mut operation);
        let mut inverse = inverse.unwrap_or_else(|error| panic!("{error}"));
        let after = sorted(world);
        for (step, expected) in [
            ("undone", &before),
            ("redone", &after),
            ("undone again", &before),
        ] {
            let stepped = world.undo(&mut inverse);
            stepped.unwrap_or_else(|error| panic!("{said} {step}: {error}"));
            assert!(same_snapshots(&sorted(world), expected), "{said} {step}");
            check_ids(world);
            check_links(world);
        }
        inverse
    }

    #[test]
    fn each_operation_is_undone_and_redone_exactly() {
        let mut x = fixture();
        let [a, b, e, f, g] = [x.a, x.b, x.e, x.f, x.g].map(|entity| x.id(entity));
        let target = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
        let held = Everything {
            target: Some(x.c),
            count: 1,
            ..everything()
        };
        let held = held.to_value();
        for operation in [
            Operation::Create {
                entity: None,
                parent: Some(b),
                components: vec![
                    named("Transform", Transform::IDENTITY.to_value()),
                    named("Sprite", Sprite::default().to_value()),
                    named("Everything", held.clone()),
                ],
            },
            Operation::Delete { entity: a },
            Operation::Add {
                entity: g,
                component: "Transform".into(),
                value: Transform::IDENTITY.to_value(),
            },
            Operation::Remove {
                entity: a,
                component: "Sprite".into(),
            },
            Operation::Set {
                entity: f,
                component: "Everything".into(),
                path: "target".to_owned(),
                value: target(x.c),
            },
            Operation::Set {
                entity: f,
                component: "Everything".into(),
                path: String::new(),
                value: held,
            },
            Operation::Reparent {
                entity: e,
                parent: Some(f),
            },
            Operation::Reparent {
                entity: a,
                parent: None,
            },
        ] {
            let redo = round_trip(&mut x.world, operation);
            x.world.release(redo);
        }
        // What's opaque, which no value reads, came back too.
        let sprite = x.world.get::<Sprite>(x.a).expect("a sprite");
        assert_eq!(sprite.texture, x.texture);
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.handle, Handle(99));
        assert_eq!(x.world.get::<Sealed>(x.g), Some(&Sealed(5)));
    }

    #[test]
    fn a_delete_undone_gives_back_each_entity_under_its_handle() {
        let mut x = fixture();
        let taken = [x.e, x.b, x.c, x.d, x.a];
        let mut delete = Operation::Delete { entity: x.id(x.a) };
        let mut inverse = x.world.apply(&mut delete).expect("deleted");
        assert_eq!(inverse.kept(), taken.len());
        for gone in taken {
            assert!(!x.world.contains(gone), "{gone}");
            assert_eq!(x.world.entity(x.id(gone)), None, "{gone}");
        }
        assert_eq!(children(&x.world, x.r), [x.s]);
        // A reference into it dangles, as after a despawn, so the world doesn't save.
        let roots = [x.r, x.f, x.g];
        let error = x.world.save_scene(&roots).expect_err("a reference dangles");
        assert_eq!(error.code(), "ecs.no_entity", "{error}");
        // Kept, its slots are taken by none of the entities spawned meanwhile.
        let spawned: Vec<Entity> = (0..taken.len()).map(|_| x.world.spawn(())).collect();
        let elsewhere = |new: &Entity| taken.iter().all(|gone| gone.index != new.index);
        assert!(spawned.iter().all(elsewhere), "{spawned:?}");
        for new in spawned {
            x.world.despawn(new).expect("alive");
        }
        x.world.undo(&mut inverse).expect("undone");
        for gone in taken {
            assert_eq!(x.world.entity(x.id(gone)), Some(gone), "{gone}");
        }
        assert_eq!(children(&x.world, x.r), [x.s, x.a], "the root last");
        assert_eq!(children(&x.world, x.a), [x.b, x.c, x.d]);
        assert_eq!(children(&x.world, x.b), [x.e]);
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.target, Some(x.e));
        x.world.save_scene(&roots).expect("whole again");
        let sprite = x.world.get::<Sprite>(x.a).expect("a sprite");
        assert_eq!(sprite.texture, x.texture);
        check_ids(&x.world);
        check_links(&x.world);
        // Redone and dropped, its entities are gone for good, their slots free under their next
        // generation.
        x.world.undo(&mut inverse).expect("redone");
        x.world.release(inverse);
        let spawned: Vec<Entity> = (0..taken.len()).map(|_| x.world.spawn(())).collect();
        for gone in taken {
            assert!(!x.world.contains(gone), "{gone}");
            let generation = gone.generation.checked_add(1).expect("a generation to go");
            let next = Entity {
                index: gone.index,
                generation,
            };
            assert!(spawned.contains(&next), "{gone}'s slot: {spawned:?}");
        }
        check_ids(&x.world);
    }

    #[test]
    fn an_add_builds_on_the_types_base() {
        let mut x = fixture();
        let mut add = Operation::Add {
            entity: x.id(x.s),
            component: "Sprite".into(),
            value: Sprite::default().to_value(),
        };
        x.world.apply(&mut add).expect("added");
        let sprite = x.world.get::<Sprite>(x.s).expect("a sprite");
        assert_eq!(
            sprite.texture,
            Sprite::default().texture,
            "a scene's sprite's"
        );
    }

    #[test]
    fn a_refusal_changes_nothing() {
        let x = fixture();
        let [r, a, s, b, e, f] = [x.r, x.a, x.s, x.b, x.e, x.f].map(|entity| x.id(entity));
        let unknown: EntityId = "0123456789abcdef".parse().expect("an ID");
        let transform = || named("Transform", Transform::IDENTITY.to_value());
        let create = |components| Operation::Create {
            entity: None,
            parent: None,
            components,
        };
        let too_many = Everything {
            count: 200,
            ..everything()
        };
        let set = |entity, path: &str, value| Operation::Set {
            entity,
            component: "Everything".into(),
            path: path.to_owned(),
            value,
        };
        let cases = [
            (Operation::Delete { entity: unknown }, "ecs.no_entity"),
            (
                Operation::Create {
                    entity: Some(a),
                    parent: None,
                    components: Vec::new(),
                },
                "ecs.id_taken",
            ),
            (
                Operation::Create {
                    entity: None,
                    parent: Some(unknown),
                    components: vec![transform()],
                },
                "ecs.no_entity",
            ),
            (
                create(vec![
                    transform(),
                    named("Velocity", Value::Struct(Vec::new())),
                ]),
                "reflect.unknown_type",
            ),
            (
                create(vec![transform(), named("Parent", Parent(x.r).to_value())]),
                "reflect.read_only",
            ),
            (
                create(vec![transform(), transform()]),
                "reflect.has_component",
            ),
            (
                create(vec![transform(), named("Everything", too_many.to_value())]),
                "reflect.out_of_range",
            ),
            (
                create(vec![transform(), named("Sealed", Value::Opaque)]),
                "reflect.opaque",
            ),
            (
                Operation::Add {
                    entity: a,
                    component: "Transform".into(),
                    value: transform().1,
                },
                "reflect.has_component",
            ),
            (
                Operation::Add {
                    entity: s,
                    component: "Sealed".into(),
                    value: Value::Opaque,
                },
                "reflect.opaque",
            ),
            (
                Operation::Remove {
                    entity: s,
                    component: "Sprite".into(),
                },
                "reflect.missing_component",
            ),
            (
                Operation::Remove {
                    entity: b,
                    component: "Parent".into(),
                },
                "reflect.read_only",
            ),
            (set(f, "count", Value::UInt(200)), "reflect.out_of_range"),
            (set(unknown, "count", Value::UInt(1)), "ecs.no_entity"),
            (
                Operation::Reparent {
                    entity: r,
                    parent: Some(e),
                },
                "ecs.parent_cycle",
            ),
            (
                Operation::Reparent {
                    entity: e,
                    parent: Some(unknown),
                },
                "ecs.no_entity",
            ),
        ];
        for (mut operation, code) in cases {
            let (mut world, mut twin) = (fixture().world, fixture().world);
            let said = operation.to_string();
            let refused = world.apply(&mut operation).map(|_| ());
            let error = refused.expect_err(&said);
            assert_eq!(error.code(), code, "{said}: {error}");
            assert_eq!(operation.to_string(), said, "an ID written in");
            unchanged(&mut world, &mut twin, &said);
        }
        // Refused, a create says what it would have made, and where.
        let mut unknown = create(vec![
            transform(),
            named("Velocity", Value::Struct(Vec::new())),
        ]);
        let error = fixture().world.apply(&mut unknown).map(|_| ());
        let error = error.expect_err("an unknown type");
        let says = "can't create an entity: there's no component type `Velocity`";
        assert!(error.to_string().starts_with(says), "{error}");
        let mut opaque = create(vec![named("Sealed", Value::Opaque)]);
        let error = fixture().world.apply(&mut opaque).map(|_| ());
        let error = error.expect_err("an opaque type");
        let says = "can't create an entity: at `Sealed`: ";
        assert!(error.to_string().starts_with(says), "{error}");
    }

    /// A set of the field at `path` of `f`'s `Everything`.
    fn set(x: &Fixture, path: &str, value: Value) -> Operation {
        Operation::Set {
            entity: x.id(x.f),
            component: "Everything".into(),
            path: path.to_owned(),
            value,
        }
    }

    #[test]
    fn an_undo_the_world_has_moved_past_is_refused_whole() {
        // Each an operation, a change code makes after it, and the code that refuses its undo.
        type Case = (
            &'static str,
            fn(&Fixture) -> Operation,
            fn(&mut Fixture),
            &'static str,
        );
        let cases: [Case; 6] = [
            (
                "a set, then a despawn",
                |x| set(x, "count", Value::UInt(1)),
                |x| x.world.despawn(x.f).expect("alive"),
                "ecs.no_entity",
            ),
            (
                "a remove, then an insert",
                |x| Operation::Remove {
                    entity: x.id(x.a),
                    component: "Sprite".into(),
                },
                |x| x.world.insert(x.a, Sprite::default()).expect("alive"),
                "reflect.has_component",
            ),
            (
                "a delete, then a load of a descendant's ID",
                |x| Operation::Delete { entity: x.id(x.a) },
                |x| {
                    let id = x.id(x.e);
                    let text = format!("(\n    entities: {{\n        \"{id}\": (),\n    }},\n)\n");
                    x.world.load_scene(&text).expect("loaded");
                },
                "ecs.id_taken",
            ),
            (
                "a delete, then a despawn of its parent",
                |x| Operation::Delete { entity: x.id(x.c) },
                |x| x.world.despawn(x.a).expect("alive"),
                "ecs.no_entity",
            ),
            (
                "a reparent, then one its undo would cycle",
                |x| Operation::Reparent {
                    entity: x.id(x.e),
                    parent: Some(x.id(x.f)),
                },
                |x| {
                    x.world.set_parent(x.b, Some(x.e)).expect("no cycle");
                },
                "ecs.parent_cycle",
            ),
            (
                "a set of a list's element, then the list shortened",
                |x| set(x, "list[1]", Value::UInt(10)),
                |x| {
                    let all = x.world.get_mut::<Everything>(x.f).expect("everything");
                    all.list.truncate(1);
                },
                "reflect.no_field",
            ),
        ];
        for (case, operation, change, code) in cases {
            let mut x = fixture();
            let mut operation = operation(&x);
            let inverse = x.world.apply(&mut operation);
            let mut inverse = inverse.unwrap_or_else(|error| panic!("{case}: {error}"));
            change(&mut x);
            let (before, version) = (sorted(&x.world), x.world.version());
            // Twice, since a refusal leaves the inverse as it was too.
            for _ in 0..2 {
                let error = x.world.undo(&mut inverse).expect_err(case);
                assert_eq!(error.code(), code, "{case}: {error}");
                assert!(same_snapshots(&sorted(&x.world), &before), "{case}");
                assert_eq!(x.world.version(), version, "{case}");
            }
            x.world.release(inverse);
        }
    }

    #[test]
    fn an_inverse_is_its_worlds_own() {
        let mut x = fixture();
        let mut delete = Operation::Delete { entity: x.id(x.a) };
        let mut inverse = x.world.apply(&mut delete).expect("deleted");
        let mut other = fixture();
        let before = sorted(&other.world);
        let error = other.world.undo(&mut inverse).expect_err("another world's");
        assert_eq!(error.code(), "ops.other_world", "{error}");
        // Dropped there, it frees none of that world's slots.
        other.world.release(inverse);
        assert!(same_snapshots(&sorted(&other.world), &before));
        let next = other.world.spawn(());
        assert!(
            other
                .ids
                .iter()
                .all(|(entity, _)| entity.index != next.index)
        );
        check_ids(&other.world);
    }
}
