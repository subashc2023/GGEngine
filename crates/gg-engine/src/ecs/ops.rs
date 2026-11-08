//! Operations: the authoring changes people and agents make to a world, which name its entities by
//! their IDs. Each, applied, gives what undoes it, its inverse: another operation, or, for a delete
//! and a remove, what they took out, typed, so that what's opaque comes back too. A delete's keeps
//! its entities' slots and IDs, so that undoing it gives each back under its handle, and every
//! reference to it is whole again; a create's takes out the entity it made alone, keeping it the
//! same way. A set's keeps the component it changed too, whole and typed, as it was, so that rolling
//! back an edit refused partway puts back exactly what the world held, which a set of the old value
//! would refuse where code had stored what a set refuses. Only the [`Log`](super::Log) holds
//! inverses, and it releases what they keep.

use std::collections::HashSet;
use std::rc::Rc;
use std::{fmt, mem};

use gg_core::{Error, Result};

use super::nodes;
use super::prefab::Instance;
use super::scene::{Expanding, NO_HINTS, Template};
use super::table::{AnyColumn, ComponentId, Row};
use super::{Entity, EntityId, Parent, World};
use crate::reflect::{self, Ident, Reflect, Value};
use crate::{ModelNode, ModelRoot, PrefabRoot};

/// An authoring change to a world, by a person or an agent, which a [`Log`](super::Log) applies,
/// and undoes. It names entities by their IDs, and carries values as reflection's, whose entities
/// are handles, as [`World::set_field`] takes one.
///
/// Applying one checks everything before it changes anything, so that a refusal changes nothing.
/// Each is refused with `ecs.no_entity` for an entity it changes, or a parent it gives, that no
/// living entity has. A create, an add and a remove are refused with `reflect.unknown_type` for a
/// component type that isn't registered, and with `reflect.read_only` for [`Parent`], which a
/// create's parent and a reparent give. A create is refused with `reflect.duplicate_component` for
/// a component it gives twice, and an add with `reflect.has_component` for one the entity has;
/// both as `set_field` refuses a value, and as making the component from it on its type's
/// [`base`](crate::Component::base) is, such as with `reflect.opaque` for a type that's opaque as
/// a whole, as [`MeshDraw`](crate::MeshDraw) is. A create is refused with `ecs.id_taken` for an ID
/// a living entity has, and a duplicate for one of its copies' IDs that a living entity has, or a
/// delete keeps for its undo, and with `ops.bad_copies` for copies' IDs given other than one each;
/// a remove with `reflect.missing_component`; a set as `set_field` refuses it, but for a
/// `PrefabRoot`, which it changes, as below; and a reparent as [`World::set_parent`] refuses it.
///
/// An instance of a prefab, as [`PrefabRoot`] says, is one edit with its root: a create of a root
/// with a `PrefabRoot`, or an add or a set of one, spawns its prefab's entities, which its undo
/// takes out, and a remove or a set takes out those it had, with the components its prefab gave the
/// root, which its undo puts back as they were, each instance put back taking what its prefab's
/// file changed since in a development build. Each is refused with `ecs.prefab_entity` for an
/// entity an instance's prefab gives that it would delete, move, unlink, or give, change or take
/// away a `PrefabRoot` of, and for a component a prefab gives that a remove would take away; as the
/// prefab's template refuses to load, as a scene that names it would, such as with
/// `scene.prefab_cycle`; and with `ecs.id_taken` for an ID of the instance's entities that a living
/// entity has, or, for a duplicate's copy, a delete keeps. A duplicate is refused with
/// `ops.bad_copies` for an ID it's given that an instance's copy gives one of its entities, mixed
/// from the ID its root's copy is given. An unlink is refused with `ecs.no_prefab` for an entity
/// without a `PrefabRoot`, and a revert for one no instance gives, or a component its prefab
/// doesn't give it, or as a set of the prefab's value is.
///
/// A create, an add or a set that makes a component's [exclusive](crate::Component::exclusive)
/// field true on its entity makes it false on every other entity with the component, and its
/// inverse makes theirs true again; a duplicate's copies have it false.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Spawns an entity with `components`, each a type's name and its whole value, made as a
    /// scene's is, its opaque parts given as [`Value::Opaque`]: the last of `parent`'s children,
    /// or a root. Its ID is `entity`, or one drawn if that's `None`, which applying it writes in.
    Create {
        entity: Option<EntityId>,
        parent: Option<EntityId>,
        components: Vec<(Ident, Value)>,
    },
    /// Spawns a copy of `entity` and of each of its descendants, with their components as they are,
    /// typed, so that what's opaque comes too, but each exclusive field false, and each reference
    /// to one of them to its copy, as a prefab's are: the copy of `entity` the last of its parent's
    /// children, or a root, and each descendant's under its parent's copy, in the order of the
    /// originals. The copies' IDs are `copies`, `entity`'s copy's first, each parent's before its
    /// children's; or ones drawn if it's empty, which applying it writes in: all but the copies of
    /// models' nodes, which are their roots' copies' nodes, their IDs mixed from those.
    Duplicate {
        entity: EntityId,
        copies: Vec<EntityId>,
    },
    /// Despawns `entity` and its descendants.
    Delete { entity: EntityId },
    /// Gives `entity` `component`, which it hasn't, made from `value` as a scene's is.
    Add {
        entity: EntityId,
        component: Ident,
        value: Value,
    },
    /// Takes `entity`'s `component` away.
    Remove { entity: EntityId, component: Ident },
    /// Sets the field at `path` of `entity`'s `component` to `value`, as `set_field` does.
    Set {
        entity: EntityId,
        component: Ident,
        path: String,
        value: Value,
    },
    /// Makes `entity` the last of `parent`'s children, or a root, as `set_parent` does.
    Reparent {
        entity: EntityId,
        parent: Option<EntityId>,
    },
    /// Unlinks the instance whose root is `entity` from its prefab, as [`PrefabRoot`] says: takes
    /// its `PrefabRoot` away, and its entities stay as they are, under their IDs, its own from then
    /// on.
    Unlink { entity: EntityId },
    /// Sets the field at `path` of `entity`'s `component`, or the whole component for an empty
    /// path, to what the prefab of the instance that gives it has there, as `set_field` does: the
    /// instance's root's, but its `Transform`, which is its own; or the entity's, that the
    /// outermost instance whose prefab gives it gives.
    Revert {
        entity: EntityId,
        component: Ident,
        path: String,
    },
}

impl Operation {
    /// The entity it changes or creates, a duplicate's copy of its entity; `None` for a create or a
    /// duplicate given no IDs, until it's applied.
    pub(crate) fn entity(&self) -> Option<EntityId> {
        match self {
            Self::Create { entity, .. } => *entity,
            Self::Duplicate { copies, .. } => copies.first().copied(),
            Self::Delete { entity }
            | Self::Add { entity, .. }
            | Self::Remove { entity, .. }
            | Self::Set { entity, .. }
            | Self::Reparent { entity, .. }
            | Self::Unlink { entity }
            | Self::Revert { entity, .. } => Some(*entity),
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
            Self::Duplicate { entity, .. } => write!(f, "duplicate {entity}"),
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
            Self::Unlink { entity } => write!(f, "unlink {entity}"),
            Self::Revert {
                entity,
                component,
                path,
            } => write!(f, "revert `{}` of {entity}", reflect::join(component, path)),
        }
    }
}

/// What undoes an operation applied to a world, which [`World::undo`] applies, and
/// [`World::roll_back`] too. It's its world's own, since its values hold the world's handles and its
/// rows its component types, and a world restored from a snapshot is another, which may have given
/// those handles out again; a delete's, and an undone create's, keeps its entities' slots and IDs
/// until it's applied or [released](World::release), and a set's its component until it's
/// [settled](Self::settle), applied or released.
pub(super) struct Inverse {
    /// Its world's serial.
    world: u64,
    undo: Undo,
}

enum Undo {
    Apply(Operation),
    /// Applies `set`, as `Apply` does, a set of the field's value before, checked as any set is;
    /// `kept` is the component, whole, as it was before, in a one-row column of its own, which a
    /// [roll back](World::roll_back) puts back unchecked, until it's settled.
    Set {
        set: Operation,
        kept: Option<Box<dyn AnyColumn>>,
    },
    /// Gives `entity` back the component a remove took, in a one-row column of its own.
    Put {
        entity: EntityId,
        component: ComponentId,
        column: Box<dyn AnyColumn>,
    },
    /// Takes out the entities a create or a duplicate made, their IDs, the root's first, keeping
    /// their slots and IDs as a delete does, if the root's subtree is them alone: what the log
    /// gave them has been undone before it, so any other is the game's, which a delete would take
    /// with it.
    Uncreate(Vec<EntityId>),
    /// Puts back the entities a delete took, in the order it took them: children before their
    /// parents, each parent's first to last, and the root last; or, if `created`, those a create or
    /// a duplicate made, which undoing it took out.
    Restore {
        taken: Vec<Taken>,
        created: bool,
        /// The instances among them, as [`World::kept_instances`] found them, each of which takes
        /// what its prefab's file changed while it was out as it's put back.
        #[cfg(feature = "dev")]
        instances: Vec<(EntityId, Rc<Template>)>,
    },
    /// Applies `undo`, then sets the exclusive fields `excluded` names to `to`: true again, to
    /// undo an operation that made them false as it made another entity's true, and false, to
    /// redo it.
    Exclusive {
        undo: Box<Undo>,
        excluded: Vec<Excluded>,
        to: bool,
    },
    /// Takes out of instance root `entity` what its prefab gave it, the components `given` and
    /// the entities `spawned` under it, with what's under them, and its `PrefabRoot`, keeping
    /// their slots and IDs as a delete does; and puts back what `put` keeps, which an add, a set
    /// or a remove of its `PrefabRoot` took out: undoing it, its prefab's entities as they were.
    Relink {
        entity: EntityId,
        put: Linked,
        given: Vec<ComponentId>,
        spawned: Vec<EntityId>,
    },
    /// Gives instance root `entity` back the `PrefabRoot` an unlink took, in a one-row column of
    /// its own.
    Link {
        entity: EntityId,
        column: Box<dyn AnyColumn>,
    },
}

/// What a change of an instance's root's prefab took out of it, which [`Undo::Relink`] puts back:
/// its `PrefabRoot`, if it had one, in a one-row column; the components its prefab gave it; and the
/// entities its prefab gave under it, each top one's subtree in the order a delete takes it.
struct Linked {
    link: Option<Box<dyn AnyColumn>>,
    row: Row,
    taken: Vec<Taken>,
    /// Its instance and those among the entities, as [`World::kept_instances`] found them, each of
    /// which takes what its prefab's file changed while it was out as it's put back.
    #[cfg(feature = "dev")]
    instances: Vec<(EntityId, Rc<Template>)>,
}

/// An entity's exclusive field, `field` of `component`, which an operation made false as it made
/// another entity's true.
#[derive(Clone)]
struct Excluded {
    entity: EntityId,
    component: Ident,
    field: &'static str,
}

/// An entity a delete, or a create undone, took out of the world.
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
        // So that the IDs drawn name what a create or a duplicate made: `exclude` finds it by
        // them, as the Inspector's pick does, and the log's and menus' names give them.
        match (&mut *operation, &undo) {
            (Operation::Create { entity, .. }, Undo::Uncreate(made)) => {
                *entity = made.first().copied()
            }
            (Operation::Duplicate { copies, .. }, Undo::Uncreate(made)) => copies.clone_from(made),
            _ => {}
        }
        let undo = self.exclude(operation, undo);
        Ok(Inverse {
            world: self.serial,
            undo,
        })
    }

    /// Applies `inverse`, undoing its operation, and leaves in its place what undoes that, which
    /// redoes it. Refused, it changes neither the world nor `inverse`: with `ops.other_world` for
    /// another world's inverse, as one from before the world was restored is; as its operation is
    /// refused, if it's one; as it takes out what a create made, with `ecs.no_entity` for the
    /// entity gone since, and `ecs.has_children` for children it has since, which the create didn't
    /// make; and, as it gives back what a remove or a delete took, with `ecs.no_entity` for an
    /// entity gone since, or a root's parent gone, `reflect.has_component` for a component the
    /// entity has since, and `ecs.id_taken` for an ID a living entity has since, such as one a
    /// scene loaded; and, as it sets an exclusive field of another entity's again, with
    /// `ecs.no_entity` for the entity gone since, and `reflect.missing_component` for the
    /// component taken away since.
    pub(super) fn undo(&mut self, inverse: &mut Inverse) -> Result<()> {
        let _zone = gg_core::zone!();
        self.check_world(inverse)?;
        self.revert(&mut inverse.undo, false)
    }

    /// Applies `inverse` as [`undo`](Self::undo) does, but puts a set's component back whole, as
    /// its inverse kept it, without checking it again: for an operation applied, undone or redone
    /// just before, with nothing changing the world since but those after it rolled back, which it
    /// then puts back exactly as it was, though code had stored in it what a set refuses, such as a
    /// rotation that's no longer a unit. It's refused only where the world has changed since, as
    /// `undo` is; and a set's inverse [settled](Inverse::settle) sets its field back as `undo` does,
    /// checked.
    pub(super) fn roll_back(&mut self, inverse: &mut Inverse) -> Result<()> {
        let _zone = gg_core::zone!();
        self.check_world(inverse)?;
        self.revert(&mut inverse.undo, true)
    }

    /// Fails with `ops.other_world` if `inverse` isn't this world's.
    fn check_world(&self, inverse: &Inverse) -> Result<()> {
        if inverse.world != self.serial {
            return Err(Error::new(
                "ops.other_world",
                "it's another world's, or from before the world was restored: what undoes an \
                 operation undoes it in the world it was applied to",
            ));
        }
        Ok(())
    }

    /// Applies `undo`, and leaves in its place what undoes that; refused, as
    /// [`undo`](Self::undo) says, it changes neither the world nor `undo`. If `exact`, a set's
    /// component goes back whole, as [`roll_back`](Self::roll_back) says.
    fn revert(&mut self, undo: &mut Undo, exact: bool) -> Result<()> {
        if let Undo::Exclusive {
            undo: inner,
            excluded,
            to,
        } = undo
        {
            // Checked first, so that a refusal changes nothing.
            for excluded in &*excluded {
                self.can_reset(excluded, *to)?;
            }
            self.revert(inner, exact)?;
            for Excluded {
                entity,
                component,
                field,
            } in &*excluded
            {
                let entity = self.entity(*entity).expect("checked alive");
                let set = self.set_field(entity, component, field, &Value::Bool(*to));
                set.expect("checked: a `bool` of a component the entity has");
            }
            *to = !*to;
            return Ok(());
        }
        if exact
            && let Undo::Set {
                set,
                kept: Some(kept),
            } = undo
        {
            return self.put_back(set, &mut **kept);
        }
        if let Undo::Relink { .. } = undo {
            return self.relink_back(undo);
        }
        let redo = match &*undo {
            Undo::Exclusive { .. } | Undo::Relink { .. } => unreachable!("reverted above"),
            Undo::Link { entity, .. } => {
                let id = self
                    .component::<PrefabRoot>()
                    .expect("a `PrefabRoot` was taken");
                self.can_put(*entity, id)?;
                Undo::Apply(Operation::Unlink { entity: *entity })
            }
            Undo::Apply(operation) | Undo::Set { set: operation, .. } => self.applied(operation)?,
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
            Undo::Uncreate(made) => {
                let root = self.can_uncreate(made)?;
                Undo::Restore {
                    // Found before they're taken out.
                    #[cfg(feature = "dev")]
                    instances: self.kept_instances(root),
                    taken: self.vacate_subtree(root),
                    created: true,
                }
            }
            Undo::Restore { taken, created, .. } => {
                self.can_restore(taken)?;
                let root = taken.last().expect("a delete takes its entity");
                if *created {
                    // Once they're back, as `made` finds them.
                    Undo::Uncreate(Vec::new())
                } else {
                    Undo::Apply(Operation::Delete { entity: root.id })
                }
            }
        };
        // Checked, so that what the inverse kept goes back, which can't fail.
        match mem::replace(undo, redo) {
            Undo::Exclusive { .. } | Undo::Relink { .. } => unreachable!("reverted above"),
            Undo::Apply(_) | Undo::Set { .. } | Undo::Uncreate(_) => {}
            Undo::Put {
                entity,
                component,
                column,
            } => {
                let entity = self.entity(entity).expect("checked alive");
                let at = self.location(entity).expect("alive");
                self.give(entity, at, component, column);
                self.fit_root(entity, component);
            }
            Undo::Link { entity, column } => {
                let entity = self.entity(entity).expect("checked alive");
                let at = self.location(entity).expect("alive");
                let id = self
                    .component::<PrefabRoot>()
                    .expect("a `PrefabRoot` was taken");
                self.give(entity, at, id, column);
            }
            Undo::Restore {
                taken,
                created,
                #[cfg(feature = "dev")]
                instances,
            } => {
                let root = taken.last().expect("a delete takes its entity").entity;
                self.restore(taken);
                #[cfg(feature = "dev")]
                self.refit_kept(&instances);
                if created {
                    *undo = Undo::Uncreate(self.made(root));
                }
            }
        }
        Ok(())
    }

    /// The IDs of what a create or a duplicate made, whose root is `root`, which is alive, as its
    /// undo takes them out: the root's first, then those in its subtree but models' nodes, which go
    /// with their roots, and the entities instances in it give, which go with theirs.
    fn made(&self, root: Entity) -> Vec<EntityId> {
        let claims = self
            .claims(&[root], &mut Expanding::default())
            .unwrap_or_default();
        let subtree = self.subtree(root).into_iter();
        let ids = subtree.map(|(entity, _)| (entity, self.id(entity).expect("alive")));
        let made = ids.filter(|&(entity, id)| {
            entity == root || (self.get::<ModelNode>(entity).is_none() && !claims.has(id))
        });
        made.map(|(_, id)| id).collect()
    }

    /// Puts back the component `set`, a set's inverse, changed, whole, as `kept` holds it, which
    /// then holds the one it had; and turns `set` into what sets the field again, with its value
    /// before: as `set` would, but exactly as the world held it, unchecked. A model's root has its
    /// model's nodes again, as a set that changes its model gives them.
    ///
    /// Fails, changing nothing, with `ecs.no_entity` for an entity gone since, and as
    /// [`get_field`](Self::get_field) does for a component taken away since, or a field gone.
    fn put_back(&mut self, set: &mut Operation, kept: &mut dyn AnyColumn) -> Result<()> {
        let Operation::Set {
            entity,
            component,
            path,
            value,
        } = set
        else {
            unreachable!("a set's inverse is a set")
        };
        let handle = self.living(*entity)?;
        let now = self.get_field(handle, component, path)?;
        let at = self.location(handle).expect("alive");
        let id = self.named(component).expect("its field was read");
        let tick = self.change_tick;
        let table = &mut self.tables[at.table as usize];
        let column = table.position(id).expect("its field was read");
        (table.row_mut(column, at.row as usize, tick)).swap_row(at.row as usize, kept);
        if Some(id) == self.component::<ModelRoot>() && now != *value {
            self.despawn_nodes(handle);
            self.fit_nodes(handle, None);
        }
        *value = now;
        Ok(())
    }

    /// Drops `inverse`, which won't be applied: if it's this world's, the slots a delete's keeps
    /// are freed, and their IDs let go, so that its entities are gone for good.
    pub(super) fn release(&mut self, inverse: Inverse) {
        if inverse.world != self.serial {
            return;
        }
        self.let_go(inverse.undo);
    }

    /// Takes into `inverse`, a set's, what undoes `later`, a later set of the same field that joins
    /// its gesture: the exclusive fields `later` made false, which undoing the gesture then makes
    /// true again with those `inverse` names; the rest of `later` is let go, since what undoes the
    /// first set undoes it too.
    pub(super) fn join(&mut self, inverse: &mut Inverse, later: Inverse) {
        let Inverse { world, undo } = later;
        let Undo::Exclusive { undo, excluded, .. } = undo else {
            self.release(Inverse { world, undo });
            return;
        };
        self.release(Inverse { world, undo: *undo });
        match &mut inverse.undo {
            Undo::Exclusive {
                excluded: before, ..
            } => before.extend(excluded),
            first => {
                *first = Undo::Exclusive {
                    undo: Box::new(mem::replace(first, Undo::Uncreate(Vec::new()))),
                    excluded,
                    to: true,
                }
            }
        }
    }

    /// Frees the slots `undo` keeps, and lets their IDs go.
    fn let_go(&mut self, undo: Undo) {
        match undo {
            Undo::Restore { taken, .. }
            | Undo::Relink {
                put: Linked { taken, .. },
                ..
            } => {
                for taken in taken {
                    self.free_slot(taken.entity.index);
                    self.free_id(taken.id);
                }
            }
            Undo::Exclusive { undo, .. } => self.let_go(*undo),
            Undo::Apply(_)
            | Undo::Set { .. }
            | Undo::Put { .. }
            | Undo::Uncreate(_)
            | Undo::Link { .. } => {}
        }
    }

    /// What undoes `operation`, applied, given `undo`, which undoes it but for its exclusive
    /// fields: each it made true on its entity is made false here on every other entity, which
    /// what's returned makes true again.
    fn exclude(&mut self, operation: &Operation, undo: Undo) -> Undo {
        // Each component it gave or set, and the path it set: a set reaches a field at the field's
        // own path, or whole with what's around it.
        let written: Vec<(&str, &str)> = match operation {
            Operation::Create { components, .. } => {
                components.iter().map(|(name, _)| (&**name, "")).collect()
            }
            Operation::Add { component, .. } => vec![(component, "")],
            Operation::Set {
                component, path, ..
            }
            | Operation::Revert {
                component, path, ..
            } => vec![(component, path)],
            // A duplicate's copies have their exclusive fields false.
            Operation::Duplicate { .. }
            | Operation::Delete { .. }
            | Operation::Remove { .. }
            | Operation::Reparent { .. }
            | Operation::Unlink { .. } => {
                return undo;
            }
        };
        let id = operation.entity().expect("an applied create has its ID");
        let entity = self.entity(id).expect("applied: alive");
        let mut excluded = Vec::new();
        for (component, path) in written {
            let of = self.named(component).expect("applied: registered");
            let Some(field) = self.components.exclusive(of) else {
                continue;
            };
            let true_on = |entity| {
                let value = self.get_field(entity, component, field);
                matches!(value.expect("a field of its own"), Value::Bool(true))
            };
            if !(path.is_empty() || path == field) || !true_on(entity) {
                continue;
            }
            let others = self.entities(Some(component)).expect("registered");
            let others: Vec<Entity> = others
                .filter(|&other| other != entity && true_on(other))
                .collect();
            for other in others {
                let set = self.set_field(other, component, field, &Value::Bool(false));
                set.expect("a `bool` of a component it has");
                excluded.push(Excluded {
                    entity: self.id(other).expect("alive"),
                    component: component.to_owned().into(),
                    field,
                });
            }
        }
        if excluded.is_empty() {
            return undo;
        }
        Undo::Exclusive {
            undo: Box::new(undo),
            excluded,
            to: true,
        }
    }

    /// Whether `excluded`'s field can be set to `to` again: its entity is alive, with the
    /// component.
    fn can_reset(&self, excluded: &Excluded, to: bool) -> Result<()> {
        let Excluded {
            entity,
            component,
            field,
        } = excluded;
        let refused = |error: Error| {
            let at = reflect::join(component, field);
            let message = format!("can't set `{at}` of {entity} to {to} again: {error}");
            error.with_message(message)
        };
        let handle = self.living(*entity).map_err(refused)?;
        let at = self.location(handle).expect("alive");
        let id = self.named(component).map_err(refused)?;
        self.column(at, id, component).map_err(refused)?;
        Ok(())
    }

    /// Applies `operation`, and returns what undoes it.
    fn applied(&mut self, operation: &Operation) -> Result<Undo> {
        let refused = |error: Error| {
            let message = format!("can't {operation}: {error}");
            error.with_message(message)
        };
        // A model's nodes are the model's, which the world changes as their root does, and which
        // take no children of their own.
        let touched = match operation {
            Operation::Create { parent, .. } => [*parent, None],
            Operation::Reparent { entity, parent } => [Some(*entity), *parent],
            Operation::Duplicate { entity, .. }
            | Operation::Delete { entity }
            | Operation::Add { entity, .. }
            | Operation::Remove { entity, .. }
            | Operation::Set { entity, .. }
            | Operation::Unlink { entity }
            | Operation::Revert { entity, .. } => [Some(*entity), None],
        };
        for touched in touched
            .into_iter()
            .flatten()
            .filter_map(|id| self.entity(id))
        {
            self.not_a_node(touched).map_err(refused)?;
        }
        // An instance's prefab's entities and components, which tools change but take none of
        // away.
        self.prefabs_kept(operation).map_err(refused)?;
        let linked = self.component::<PrefabRoot>();
        let link_name = linked.map(|id| self.components.name(id).to_owned());
        let links = |name: &str| link_name.as_deref() == Some(name);
        Ok(match operation {
            Operation::Create {
                entity,
                parent,
                components,
            } => {
                let template = self.created_template(components).map_err(refused)?;
                let made = self.create(*entity, *parent, components, template.as_deref());
                let id = made.map_err(refused)?;
                let made = self.entity(id).expect("made");
                // Its prefab's entities and its model's nodes, which its undo takes out with it,
                // whichever it has then.
                if let Some(template) = template {
                    let ids = template.ids(id).expect("checked as it was made");
                    self.spawn_instance(made, &Instance { template, ids });
                }
                self.fit_nodes(made, None);
                Undo::Uncreate(vec![id])
            }
            Operation::Duplicate { entity, copies } => {
                Undo::Uncreate(self.duplicate(*entity, copies).map_err(refused)?)
            }
            Operation::Delete { entity } => {
                let root = self.living(*entity).map_err(refused)?;
                Undo::Restore {
                    // Found before they're taken out.
                    #[cfg(feature = "dev")]
                    instances: self.kept_instances(root),
                    taken: self.vacate_subtree(root),
                    created: false,
                }
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
                if links(component) {
                    return self.relink(handle, Some(column)).map_err(refused);
                }
                self.give(handle, at, id, column);
                self.fit_root(handle, id);
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
                if links(component) {
                    return self.relink(handle, None).map_err(refused);
                }
                let column = self.take_out(handle, at, id);
                self.fit_root(handle, id);
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
                // Whole, before anything changes, for a roll back.
                let kept = self.copy_of(handle, component);
                // A link too, which the relink below spawns the instance of.
                let old = self.write_field(handle, component, path, value, true)?;
                let undo = Undo::Set {
                    set: Operation::Set {
                        entity: *entity,
                        component: component.clone(),
                        path: path.clone(),
                        value: old,
                    },
                    kept,
                };
                let (
                    Undo::Set {
                        kept: Some(kept), ..
                    },
                    true,
                ) = (&undo, links(component))
                else {
                    return Ok(undo);
                };
                // Another prefab: the one the world held goes back, for the relink to take out.
                let id = linked.expect("a link's");
                let set = self.copy_of(handle, component).expect("set");
                if set.value(0) == kept.value(0) {
                    return Ok(undo);
                }
                let Undo::Set {
                    kept: Some(mut kept),
                    ..
                } = undo
                else {
                    unreachable!("matched above");
                };
                self.swap_component(handle, id, &mut *kept);
                return self.relink(handle, Some(set)).map_err(refused);
            }
            Operation::Unlink { entity } => {
                let handle = self.living(*entity).map_err(refused)?;
                let at = self.location(handle).expect("alive");
                let id = linked.filter(|&id| self.tables[at.table as usize].has(id));
                let Some(id) = id else {
                    return Err(refused(Error::new(
                        "ecs.no_prefab",
                        "it has no `PrefabRoot`: it's no instance's root",
                    )));
                };
                Undo::Link {
                    entity: *entity,
                    column: self.take_out(handle, at, id),
                }
            }
            Operation::Revert {
                entity,
                component,
                path,
            } => {
                let handle = self.living(*entity).map_err(refused)?;
                let value = self.prefab_value(handle, component, path);
                let value = value.map_err(refused)?;
                self.applied(&Operation::Set {
                    entity: *entity,
                    component: component.clone(),
                    path: path.clone(),
                    value,
                })?
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

    /// A copy of `entity`'s component of type `name`, in a one-row column of its own; `None` if it
    /// has none.
    fn copy_of(&self, entity: Entity, name: &str) -> Option<Box<dyn AnyColumn>> {
        let at = self.location(entity)?;
        let table = &self.tables[at.table as usize];
        let column = table.position(self.components.named(name)?)?;
        Some(table.column_at(column).clone_row(at.row as usize))
    }

    /// Fits `entity`'s nodes to its [`ModelRoot`], as [`fit_nodes`](Self::fit_nodes) does, if
    /// `component`, which it was given or lost, is that.
    fn fit_root(&mut self, entity: Entity, component: ComponentId) {
        if self.component::<ModelRoot>() == Some(component) {
            self.fit_nodes(entity, None);
        }
    }

    /// Fails with `ecs.prefab_entity` for `operation` if it would take away what an instance's
    /// prefab gives: delete or move one of the entities it gives, or unlink one, or give one a
    /// `PrefabRoot`, change it, or take it away, as those of instances inside another's are the
    /// other's prefab's; or remove a component the prefab gives one of its entities, or its root.
    fn prefabs_kept(&self, operation: &Operation) -> Result<()> {
        let linked = self.component::<PrefabRoot>();
        let links = |name: &str| linked.is_some() && self.components.named(name) == linked;
        let (id, component) = match operation {
            Operation::Delete { entity }
            | Operation::Reparent { entity, .. }
            | Operation::Unlink { entity } => (*entity, None),
            Operation::Add {
                entity, component, ..
            }
            | Operation::Set {
                entity, component, ..
            } if links(component) => (*entity, None),
            Operation::Remove { entity, component } if links(component) => (*entity, None),
            Operation::Remove { entity, component } => (*entity, Some(component)),
            _ => return Ok(()),
        };
        // One no entity has, its operation refuses.
        let Some(entity) = self.entity(id) else {
            return Ok(());
        };
        let Some(component) = component else {
            return self.not_a_prefabs(entity);
        };
        let Some(of) = self.components.named(component) else {
            return Ok(());
        };
        let mut expanding = Expanding::default();
        let (root, instance, part) = match self.claimer(entity, &mut expanding)? {
            Some(claimed) => claimed,
            None => match self.instance(entity, &mut expanding)? {
                Some(instance) => (entity, instance, 0),
                None => return Ok(()),
            },
        };
        if instance.components(self, part).contains(&of) {
            return Err(self.prefabs_own(entity, root, &format!("its `{component}`")));
        }
        Ok(())
    }

    /// The template of the prefab a create's `components` link, if they give a `PrefabRoot`.
    ///
    /// Fails as the `PrefabRoot`'s value does, and as the template does.
    fn created_template(&self, components: &[(Ident, Value)]) -> Result<Option<Rc<Template>>> {
        let Some(linked) = self.component::<PrefabRoot>() else {
            return Ok(None);
        };
        let name = self.components.name(linked);
        let Some((_, value)) = components.iter().find(|(given, _)| given == name) else {
            return Ok(None);
        };
        let PrefabRoot(prefab) = PrefabRoot::from_value(value)?;
        Expanding::default().template(self, prefab).map(Some)
    }

    /// Changes `root`'s prefab to the one `link`, a one-row column of a `PrefabRoot`, names, or
    /// takes its prefab away without one: takes out its `PrefabRoot`, the components its prefab
    /// gave it, but its `Transform`, and the entities its prefab gave under it, with what's under
    /// them, keeping their slots and IDs as a delete does; then gives it `link`, and its prefab's
    /// instance, as [`spawn_instance`](Self::spawn_instance) does. Returns what undoes it.
    ///
    /// Fails, changing nothing, as the instances' templates do, and with `ecs.id_taken` for an ID
    /// an entity of the new instance takes that a living entity has, but one taken out.
    fn relink(&mut self, root: Entity, link: Option<Box<dyn AnyColumn>>) -> Result<Undo> {
        let id = self.id(root).expect("alive");
        let mut expanding = Expanding::default();
        let old = self.instance(root, &mut expanding)?;
        let new = match &link {
            Some(link) => {
                let PrefabRoot(prefab) = PrefabRoot::from_value(&link.value(0))?;
                let template = expanding.template(self, prefab)?;
                let ids = template.ids(id)?;
                Some(Instance { template, ids })
            }
            None => None,
        };
        let tops = self.linked_tops(root, old.as_ref());
        if let Some(new) = &new {
            let leaving = self.subtree_ids(&tops);
            let mut ids = new.ids[1..].iter();
            let taken = ids.find(|&id| self.by_id.contains_key(id) && !leaving.contains(id));
            if let Some(taken) = taken {
                return Err(Error::new(
                    "ecs.id_taken",
                    format!(
                        "entity {taken}, which its prefab gives it, is in the world already, as \
                         an entity of its own under it"
                    ),
                ));
            }
        }
        let given = old.map_or_else(Vec::new, |old| old.components(self, 0));
        let put = self.take_link(root, &given, &tops);
        let (given, spawned) = match (link, new) {
            (Some(link), Some(new)) => {
                let at = self.location(root).expect("alive");
                self.give(
                    root,
                    at,
                    self.component::<PrefabRoot>().expect("linked"),
                    link,
                );
                self.spawn_instance(root, &new)
            }
            _ => (Vec::new(), Vec::new()),
        };
        Ok(Undo::Relink {
            entity: id,
            put,
            given,
            spawned,
        })
    }

    /// Applies `undo`, an [`Undo::Relink`], and leaves in its place what undoes that; refused, it
    /// changes neither the world nor `undo`: with `ecs.no_entity` for the root gone since,
    /// `ecs.id_taken` for an ID of what goes back that a living entity has since, but those taken
    /// out, and `reflect.has_component` for a component that goes back that the root has since.
    fn relink_back(&mut self, undo: &mut Undo) -> Result<()> {
        let Undo::Relink {
            entity,
            put,
            given,
            spawned,
        } = undo
        else {
            unreachable!("a relink's");
        };
        let root = self.living(*entity)?;
        let tops = spawned.iter().filter_map(|&id| self.entity(id));
        let tops: Vec<Entity> = tops
            .filter(|&top| self.parent(top).expect("alive") == Some(root))
            .collect();
        let leaving = self.subtree_ids(&tops);
        let refused = |error: Error| {
            let message = format!("can't put entity {entity}'s prefab back: {error}");
            error.with_message(message)
        };
        let mut back = put.taken.iter().map(|taken| taken.id);
        if let Some(taken) = back.find(|id| self.by_id.contains_key(id) && !leaving.contains(id)) {
            return Err(refused(Error::new(
                "ecs.id_taken",
                format!("entity {taken} is in the world again"),
            )));
        }
        let at = self.location(root).expect("alive");
        for &(component, _) in &put.row {
            if self.tables[at.table as usize].has(component) && !given.contains(&component) {
                let name = self.components.name(component);
                return Err(refused(Error::new(
                    "reflect.has_component",
                    format!("it has a `{name}` again"),
                )));
            }
        }
        let current = self.take_link(root, given, &tops);
        // The root's own, which what goes back doesn't give it.
        let at = self.location(root).expect("alive");
        let own: Vec<ComponentId> = self.tables[at.table as usize].components().to_vec();
        let own_children: HashSet<Entity> = self.children(root).expect("alive").collect();
        let Linked {
            link,
            row,
            taken,
            #[cfg(feature = "dev")]
            instances,
        } = mem::replace(put, current);
        let model = self.component::<ModelRoot>();
        let fits = row.iter().any(|&(component, _)| Some(component) == model);
        for (component, column) in row {
            let at = self.location(root).expect("alive");
            self.give(root, at, component, column);
        }
        self.restore(taken);
        if let Some(link) = link {
            let at = self.location(root).expect("alive");
            self.give(
                root,
                at,
                self.component::<PrefabRoot>().expect("linked"),
                link,
            );
        }
        if fits {
            self.fit_nodes(root, None);
        }
        #[cfg(feature = "dev")]
        self.refit_kept(&instances);
        // What its prefab gives it now, which undoing this takes out: what went back, as its
        // prefab's file changed it while it was out.
        let link_type = self.component::<PrefabRoot>();
        let at = self.location(root).expect("alive");
        let types = self.tables[at.table as usize].components().iter().copied();
        let types =
            types.filter(|&component| !own.contains(&component) && Some(component) != link_type);
        *given = types.collect();
        // But its model's nodes, which come and go with its `ModelRoot`.
        let tops = self.children(root).expect("alive");
        let tops = tops
            .filter(|&top| !own_children.contains(&top) && self.get::<ModelNode>(top).is_none());
        *spawned = tops.map(|top| self.id(top).expect("alive")).collect();
        Ok(())
    }

    /// `root`'s children that `instance`, its instance, if it has one, gives.
    fn linked_tops(&self, root: Entity, instance: Option<&Instance>) -> Vec<Entity> {
        let Some(instance) = instance else {
            return Vec::new();
        };
        let given: HashSet<EntityId> = instance.ids[1..].iter().copied().collect();
        let children = self.children(root).expect("alive");
        let tops = children.filter(|&child| given.contains(&self.id(child).expect("alive")));
        tops.collect()
    }

    /// The IDs of `tops`, which are alive, and their descendants.
    fn subtree_ids(&self, tops: &[Entity]) -> HashSet<EntityId> {
        let subtrees = tops.iter().flat_map(|&top| self.subtree(top));
        subtrees
            .map(|(entity, _)| self.id(entity).expect("alive"))
            .collect()
    }

    /// Takes out of `root` its `PrefabRoot`, the components of the types `given` that it has, and
    /// `tops`, which are under it, with what's under them, keeping their slots and IDs as a delete
    /// does; and its model's nodes, if its `ModelRoot` goes. Returns what it took out, as an
    /// [`Undo::Relink`] keeps it.
    fn take_link(&mut self, root: Entity, given: &[ComponentId], tops: &[Entity]) -> Linked {
        // Found before they're taken out: those of the root and among what's taken alone.
        #[cfg(feature = "dev")]
        let mut instances = self.kept_instances(root);
        #[cfg(feature = "dev")]
        {
            let root_id = self.id(root).expect("alive");
            let leaving = self.subtree_ids(tops);
            instances.retain(|(id, _)| *id == root_id || leaving.contains(id));
        }
        let take = |world: &mut World, component: ComponentId| {
            let at = world.location(root).expect("alive");
            let has = world.tables[at.table as usize].has(component);
            has.then(|| world.take_out(root, at, component))
        };
        let link = self.component::<PrefabRoot>().and_then(|id| take(self, id));
        let mut row = Vec::with_capacity(given.len());
        for &component in given {
            if let Some(column) = take(self, component) {
                row.push((component, column));
            }
        }
        let mut taken = Vec::new();
        for &top in tops {
            taken.extend(self.vacate_subtree(top));
        }
        let model = self.component::<ModelRoot>();
        if row.iter().any(|&(component, _)| Some(component) == model) {
            self.fit_nodes(root, None);
        }
        Linked {
            link,
            row,
            taken,
            #[cfg(feature = "dev")]
            instances,
        }
    }

    /// Swaps `entity`'s component `id` with the one `column`, a one-row column of its type, holds,
    /// unchecked, as a roll back puts back what a set kept.
    fn swap_component(&mut self, entity: Entity, id: ComponentId, column: &mut dyn AnyColumn) {
        let at = self.location(entity).expect("alive");
        let tick = self.change_tick;
        let table = &mut self.tables[at.table as usize];
        let position = table.position(id).expect("the entity has the component");
        (table.row_mut(position, at.row as usize, tick)).swap_row(at.row as usize, column);
    }

    /// Spawns the entity a create makes, and returns its ID: one that leaves free the IDs the
    /// entities of its instance of `template`, if it links a prefab, take.
    fn create(
        &mut self,
        id: Option<EntityId>,
        parent: Option<EntityId>,
        components: &[(Ident, Value)],
        template: Option<&Template>,
    ) -> Result<EntityId> {
        if let Some(id) = id
            && self.by_id.contains_key(&id)
        {
            return Err(Error::new(
                "ecs.id_taken",
                format!("entity {id} is in the world already"),
            ));
        }
        if let (Some(id), Some(template)) = (id, template)
            && let Some(taken) = template.ids(id)?[1..]
                .iter()
                .find(|id| self.by_id.contains_key(id))
        {
            return Err(Error::new(
                "ecs.id_taken",
                format!(
                    "entity {taken}, which its prefab gives it, is in the world already: give it \
                     another ID"
                ),
            ));
        }
        let parent = parent.map(|parent| self.living(parent)).transpose()?;
        let mut row = Vec::with_capacity(components.len() + 1);
        for (name, value) in components {
            let component = self.writable_type(name)?;
            if row.iter().any(|&(other, _)| other == component) {
                return Err(Error::new(
                    "reflect.duplicate_component",
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
        let id = match (id, template) {
            (Some(id), _) => {
                self.find_by_id(id, entity);
                id
            }
            (None, None) => self.draw_id(entity, &[]),
            // Drawn until its instance's IDs are free too, as a model's nodes' are.
            (None, Some(template)) => loop {
                let id = self.draw_id(entity, &[]);
                let ids = template.ids(id).ok();
                if ids.is_some_and(|ids| ids[1..].iter().all(|&id| !self.reserved(id))) {
                    break id;
                }
                self.by_id.remove(&id);
                self.stream.reseed();
            },
        };
        self.place(entity, id, row);
        if let Some(parent) = parent {
            self.append(parent.index, entity.index);
        }
        Ok(id)
    }

    /// Takes `root`, which is alive, and its descendants out of the world, keeping their slots and
    /// IDs, in the order [`take_apart`](Self::take_apart) gives them: children before their
    /// parents, and the root last.
    fn vacate_subtree(&mut self, root: Entity) -> Vec<Taken> {
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
        taken
    }

    /// The root of what a create or a duplicate made, `made`, the root's ID first, if undoing it
    /// can take them out again: the root is alive, and its subtree is them alone, every one of
    /// them, without what the create or the duplicate didn't make, but models' nodes and the
    /// entities instances give, which go with their roots, whichever their models and prefabs have
    /// now.
    fn can_uncreate(&self, made: &[EntityId]) -> Result<Entity> {
        let root = self.living(made[0])?;
        let claims = self.claims(&[root], &mut Expanding::default())?;
        let subtree = self.subtree(root);
        let under = subtree[1..].iter().filter(|&&(entity, _)| {
            let id = self.id(entity).expect("alive");
            self.get::<ModelNode>(entity).is_none() && !claims.has(id)
        });
        let under: Vec<Entity> = under.map(|&(entity, _)| entity).collect();
        let mut ours = made[1..].to_vec();
        ours.sort_unstable();
        let theirs = under.iter().map(|&entity| self.id(entity).expect("alive"));
        if let Some(other) = theirs.clone().find(|id| ours.binary_search(id).is_err()) {
            let child = self.living(other).and_then(|other| self.parent(other));
            let place = if child.is_ok_and(|parent| parent == Some(root)) {
                "its child"
            } else {
                "under it"
            };
            return Err(Error::new(
                "ecs.has_children",
                format!(
                    "entity {other} is {place}, which wasn't made with it: an undo takes only what \
                     was made"
                ),
            ));
        }
        if under.len() != ours.len() {
            let mut found: Vec<EntityId> = theirs.collect();
            found.sort_unstable();
            let gone = ours.iter().find(|id| found.binary_search(id).is_err());
            let gone = gone.expect("fewer under it, all of them made");
            return Err(Error::new(
                "ecs.no_entity",
                format!("entity {gone}, which it made, isn't under it any more"),
            ));
        }
        Ok(root)
    }

    /// Spawns a copy of `entity`'s subtree, as [`Operation::Duplicate`] says, under the IDs `ids`,
    /// or drawn if it's empty; and returns the copies' IDs, `entity`'s copy's first: all but the
    /// copies of models' nodes, which are their roots' copies' nodes, their IDs mixed from those.
    fn duplicate(&mut self, entity: EntityId, ids: &[EntityId]) -> Result<Vec<EntityId>> {
        let root = self.living(entity)?;
        let originals = self.subtree(root);
        let is_node: Vec<bool> = (originals.iter())
            .map(|&(original, _)| self.get::<ModelNode>(original).is_some())
            .collect();
        // Each original an instance in the subtree gives, by the instance and its part, whose copy
        // the instance's copy gives, its ID mixed from the copy's root's.
        let claims = self.claims(&[root], &mut Expanding::default())?;
        let claimed: Vec<Option<(usize, usize)>> = (originals.iter())
            .map(|&(original, _)| claims.of(self.id(original).expect("alive")))
            .collect();
        let mixes: Vec<bool> = (is_node.iter().zip(&claimed))
            .map(|(&node, claimed)| node || claimed.is_some())
            .collect();
        let root_at: Vec<usize> = (claims.instances.iter())
            .map(|(of, _)| {
                let at = originals.iter().position(|(original, _)| original == of);
                at.expect("an instance in the subtree")
            })
            .collect();
        let mut copied_ids: Vec<Option<Vec<EntityId>>> = vec![None; claims.instances.len()];
        let named = mixes.iter().filter(|&&mixed| !mixed).count();
        if !ids.is_empty() {
            if ids.len() != named {
                return Err(Error::new(
                    "ops.bad_copies",
                    format!(
                        "it's given {} IDs for {named} copies, one for each of the entity and its \
                         descendants but its models' nodes and the entities its instances' \
                         prefabs give",
                        ids.len(),
                    ),
                ));
            }
            for (given, id) in ids.iter().enumerate() {
                if self.reserved(*id) {
                    return Err(Error::new(
                        "ecs.id_taken",
                        format!("entity {id} is in the world already, or a delete keeps its ID"),
                    ));
                }
                if ids[..given].contains(id) {
                    return Err(Error::new(
                        "ops.bad_copies",
                        format!("it's given ID {id} twice, but each copy has its own"),
                    ));
                }
            }
            // The IDs each instance's copy gives its entities, mixed from its root's copy's, which
            // no other copy is given, and which are free.
            let mut given = ids.iter();
            let given: Vec<Option<EntityId>> = (mixes.iter())
                .map(|&mixed| (!mixed).then(|| *given.next().expect("one for each")))
                .collect();
            for (instance, &at) in root_at.iter().enumerate() {
                let Some(copy_root) = given[at] else {
                    continue;
                };
                let template = &claims.instances[instance].1.template;
                for &id in &template.ids(copy_root)?[1..] {
                    if ids.contains(&id) {
                        return Err(Error::new(
                            "ops.bad_copies",
                            format!(
                                "it's given ID {id}, which the copy of the instance whose root is \
                                 entity {} gives an entity of its prefab, as its ID {copy_root} \
                                 mixes",
                                self.name_of(originals[at].0)
                            ),
                        ));
                    }
                    if self.reserved(id) {
                        return Err(Error::new(
                            "ecs.id_taken",
                            format!(
                                "entity {id}, which the copy of the instance whose root is entity \
                                 {} gives an entity of its prefab, as its ID {copy_root} mixes, is \
                                 in the world already, or a delete keeps its ID: give the copy \
                                 another ID",
                                self.name_of(originals[at].0)
                            ),
                        ));
                    }
                }
            }
        }
        let parent_id = self.component::<Parent>();
        let parent_id = parent_id.expect("every world has registered `Parent`");
        // Each original's components, copied before anything changes, but its `Parent`, which its
        // copy's is.
        let rows: Vec<Row> = originals
            .iter()
            .map(|&(original, _)| {
                let at = self.location(original).expect("alive");
                let table = &self.tables[at.table as usize];
                let columns = table.components().iter().zip(table.columns());
                let columns = columns.filter(|&(&component, _)| component != parent_id);
                let copied = columns
                    .map(|(&component, column)| (component, column.clone_row(at.row as usize)));
                copied.collect()
            })
            .collect();
        let root_parent = self.parent(root).expect("alive");
        let mut made: Vec<(Entity, EntityId)> = Vec::with_capacity(rows.len());
        let mut given = ids.iter().copied();
        for (at, (mut row, &(_, under))) in rows.into_iter().zip(&originals).enumerate() {
            let parent = under.map_or(root_parent, |under| Some(made[under].0));
            if let Some(parent) = parent {
                let column: Box<dyn AnyColumn> = Box::new(vec![Parent(parent)]);
                row.push((parent_id, column));
                row.sort_unstable_by_key(|&(component, _)| component);
            }
            // A model's node's copy is its root's copy's node, its ID mixed from the copy's, or
            // drawn where that's taken or given: a given ID goes to the copy it's given for alone.
            let part = self
                .get::<ModelNode>(originals[at].0)
                .map(|&ModelNode(part)| part);
            let mixed = part.and_then(|part| {
                let mut up = under;
                while let Some(above) = up {
                    if self.get::<ModelRoot>(originals[above].0).is_some() {
                        let id = nodes::node_id(made[above].1, part as usize);
                        return id.filter(|&id| !self.reserved(id) && !ids.contains(&id));
                    }
                    up = originals[above].1;
                }
                None
            });
            // An instance's entity's copy is its root's copy's, its ID mixed from the copy's, which
            // the checks above, or the draw of the root's copy, leave free; drawn only where
            // another copy took it since, by chance.
            let linked = claimed[at].and_then(|(instance, part)| {
                let copy_root = made[root_at[instance]].1;
                let template = &claims.instances[instance].1.template;
                let copied = copied_ids[instance]
                    .get_or_insert_with(|| template.ids(copy_root).unwrap_or_default());
                let id = copied.get(part).copied();
                id.filter(|&id| !self.reserved(id) && !ids.contains(&id))
            });
            let id = match (is_node[at], claimed[at]) {
                (true, _) => mixed,
                (false, Some(_)) => linked,
                (false, None) => given.next(),
            };
            let copy = self.take_slot();
            let instance = root_at.iter().position(|&root| root == at);
            let id = match (id, instance) {
                (Some(id), _) => {
                    self.find_by_id(id, copy);
                    id
                }
                (None, None) => self.draw_id(copy, ids),
                // An instance's root's copy, drawn until the IDs its instance's copy gives are free
                // too, as a create's is.
                (None, Some(instance)) => loop {
                    let id = self.draw_id(copy, ids);
                    let template = &claims.instances[instance].1.template;
                    let mixed = template.ids(id).ok();
                    let free =
                        |mixed: Vec<EntityId>| mixed[1..].iter().all(|&id| !self.reserved(id));
                    if mixed.is_some_and(free) {
                        break id;
                    }
                    self.by_id.remove(&id);
                    self.stream.reseed();
                },
            };
            self.place(copy, id, row);
            if let Some(parent) = parent {
                self.append(parent.index, copy.index);
            }
            made.push((copy, id));
        }
        // References among the originals are to their copies, as a prefab's are, pointed there
        // typed, so that the rest stays exactly as it was copied, though a set would refuse what
        // code stored in it; and exclusive fields are false, as the copies are placed.
        let originals: Vec<Entity> = originals
            .into_iter()
            .map(|(original, _)| original)
            .collect();
        let copies: Vec<Entity> = made.iter().map(|&(copy, _)| copy).collect();
        let mut retarget = |entity: &mut Entity| {
            if let Some(at) = originals.iter().position(|original| original == entity) {
                *entity = copies[at];
            }
        };
        for &copy in &copies {
            let at = self.location(copy).expect("alive");
            let (row, tick) = (at.row as usize, self.change_tick);
            let table = &mut self.tables[at.table as usize];
            for position in 0..table.components().len() {
                if table.components()[position] != parent_id {
                    (table.row_mut(position, row, tick)).entities_mut(row, &mut retarget);
                }
            }
            let components = self.tables[at.table as usize].components().to_vec();
            for component in components.into_iter().filter(|&id| id != parent_id) {
                let name = self.components.name(component).to_owned();
                if let Some(field) = self.components.exclusive(component) {
                    let set = self.set_field(copy, &name, field, &Value::Bool(false));
                    set.expect("an exclusive field is a `bool` of a component it has");
                }
            }
        }
        let named = made.into_iter().zip(mixes).filter(|&(_, mixed)| !mixed);
        Ok(named.map(|((_, id), _)| id).collect())
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
                meta.location.is_none()
                    && meta.generation == taken.entity.generation
                    && self.kept.contains_key(&taken.id),
                "a slot a delete vacated is freed, and its ID let go, only with its inverse"
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

    /// Puts the entities a delete took, `taken`, back, each into its slot under its handle and ID.
    fn restore(&mut self, taken: Vec<Taken>) {
        let mut links = Vec::with_capacity(taken.len());
        for Taken {
            entity,
            id,
            parent,
            row,
        } in taken
        {
            self.free_id(id);
            self.find_by_id(id, entity);
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

    /// The id of the component type named `name`, which isn't [`Parent`] or [`ModelNode`].
    fn writable_type(&self, name: &str) -> Result<ComponentId> {
        let id = self.named(name)?;
        if Some(id) == self.component::<Parent>() {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps `Parent`, so that the hierarchy stays whole: a create's parent \
                 and a reparent give it",
            ));
        }
        if Some(id) == self.component::<ModelNode>() {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps `ModelNode`, which a model's nodes have as it spawns them under \
                 the model's root: a `ModelRoot` gives them",
            ));
        }
        Ok(id)
    }

    /// Component `id`, of the type named `name`, made from `value` as a scene's is, in a one-row
    /// column; checked as `set_field` checks a value.
    pub(super) fn build(
        &self,
        id: ComponentId,
        name: &str,
        value: &Value,
    ) -> Result<Box<dyn AnyColumn>> {
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

impl Inverse {
    /// Lets go of what only a [roll back](World::roll_back) reads, a set's component kept whole,
    /// once nothing will roll its operation back.
    pub(super) fn settle(&mut self) {
        fn settle(undo: &mut Undo) {
            match undo {
                Undo::Set { kept, .. } => *kept = None,
                Undo::Exclusive { undo, .. } => settle(undo),
                Undo::Apply(_)
                | Undo::Put { .. }
                | Undo::Uncreate(_)
                | Undo::Restore { .. }
                | Undo::Relink { .. }
                | Undo::Link { .. } => {}
            }
        }
        settle(&mut self.undo);
    }

    /// A copy of it, its world's still, as a copy of its log takes: it keeps what this keeps, as
    /// its own, so that once [rebound](Self::rebind) to a copy of the world taken while this kept
    /// it, it undoes there as this does in the world.
    #[cfg(any(feature = "dev", test))]
    pub(super) fn copied(&self) -> Inverse {
        fn copied(undo: &Undo) -> Undo {
            match undo {
                Undo::Apply(operation) => Undo::Apply(operation.clone()),
                Undo::Set { set, kept } => Undo::Set {
                    set: set.clone(),
                    kept: kept.as_ref().map(|kept| kept.cloned()),
                },
                Undo::Put {
                    entity,
                    component,
                    column,
                } => Undo::Put {
                    entity: *entity,
                    component: *component,
                    column: column.cloned(),
                },
                Undo::Uncreate(made) => Undo::Uncreate(made.clone()),
                Undo::Restore {
                    taken,
                    created,
                    #[cfg(feature = "dev")]
                    instances,
                } => Undo::Restore {
                    taken: copied_taken(taken),
                    created: *created,
                    #[cfg(feature = "dev")]
                    instances: instances.clone(),
                },
                Undo::Relink {
                    entity,
                    put,
                    given,
                    spawned,
                } => Undo::Relink {
                    entity: *entity,
                    put: Linked {
                        link: put.link.as_ref().map(|link| link.cloned()),
                        row: copied_row(&put.row),
                        taken: copied_taken(&put.taken),
                        #[cfg(feature = "dev")]
                        instances: put.instances.clone(),
                    },
                    given: given.clone(),
                    spawned: spawned.clone(),
                },
                Undo::Link { entity, column } => Undo::Link {
                    entity: *entity,
                    column: column.cloned(),
                },
                Undo::Exclusive { undo, excluded, to } => Undo::Exclusive {
                    undo: Box::new(copied(undo)),
                    excluded: excluded.clone(),
                    to: *to,
                },
            }
        }
        fn copied_row(row: &Row) -> Row {
            let row = row.iter();
            row.map(|(id, column)| (*id, column.cloned())).collect()
        }
        fn copied_taken(taken: &[Taken]) -> Vec<Taken> {
            let taken = taken.iter().map(|taken| Taken {
                row: copied_row(&taken.row),
                ..*taken
            });
            taken.collect()
        }
        Inverse {
            world: self.world,
            undo: copied(&self.undo),
        }
    }

    /// Has it be the world's whose serial is `to`, if it's `from`'s, as [`copied`](Self::copied)
    /// says.
    #[cfg(any(feature = "dev", test))]
    pub(super) fn rebind(&mut self, from: u64, to: u64) {
        if self.world == from {
            self.world = to;
        }
    }
}

#[cfg(test)]
impl Inverse {
    /// How many slots it keeps.
    pub(super) fn kept(&self) -> usize {
        fn kept(undo: &Undo) -> usize {
            match undo {
                Undo::Restore { taken, .. }
                | Undo::Relink {
                    put: Linked { taken, .. },
                    ..
                } => taken.len(),
                Undo::Exclusive { undo, .. } => kept(undo),
                Undo::Apply(_)
                | Undo::Set { .. }
                | Undo::Put { .. }
                | Undo::Uncreate(_)
                | Undo::Link { .. } => 0,
            }
        }
        kept(&self.undo)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use gg_core::math::{Quat, Vec3};
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
        world
            .assets_mut()
            .add_texture(texture, "own.png".into(), "own.png".into());
        let at = |x: f32| Transform {
            translation: Vec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let r = world.spawn(at(0.0));
        let sprite = Sprite {
            texture: Some(texture),
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

    fn named(name: &'static str, value: Value) -> (Ident, Value) {
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
            Operation::Duplicate {
                entity: a,
                copies: Vec::new(),
            },
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
        assert_eq!(sprite.texture, Some(x.texture));
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.handle, Handle(99));
        assert_eq!(x.world.get::<Sealed>(x.g), Some(&Sealed(5)));
    }

    /// A duplicate copies an entity and its descendants, typed, with what's opaque, each under an
    /// ID of its own, written into it: the entity's copy the last of its parent's children, each
    /// other copy under its parent's, in the order of the originals; a reference to one of them
    /// points at its copy, and one to an entity outside stays. Undoing it is refused once one of
    /// its copies has a child it didn't make, and once one is gone, changing nothing.
    #[test]
    fn a_duplicate_copies_a_subtree_typed_under_ids_of_its_own() {
        let mut x = fixture();
        // `f`, which targets `e`, under `a` with it; `g`, opaque as a whole, too.
        x.world.set_parent(x.f, Some(x.a)).expect("no cycle");
        x.world.set_parent(x.g, Some(x.f)).expect("no cycle");
        let mut duplicate = Operation::Duplicate {
            entity: x.id(x.a),
            copies: Vec::new(),
        };
        let mut inverse = x.world.apply(&mut duplicate).expect("applied");
        let Operation::Duplicate { copies, .. } = &duplicate else {
            unreachable!("applied as it was")
        };
        let copied = copies.iter().map(|&id| x.world.entity(id).expect("alive"));
        let copied: Vec<Entity> = copied.collect();
        // Each parent before its children: `a`, then `b`, `c`, `d` and `f`, then `e` and `g`.
        let [a, b, c, d, f, e, g] = copied[..] else {
            panic!("seven copies: {copies:?}");
        };
        assert_eq!(children(&x.world, x.r), [x.a, x.s, a]);
        assert_eq!(children(&x.world, a), [b, c, d, f]);
        assert_eq!(children(&x.world, b), [e]);
        assert_eq!(children(&x.world, f), [g]);
        for (copy, original) in [(a, x.a), (b, x.b), (c, x.c), (d, x.d), (e, x.e), (f, x.f)] {
            let at = x.world.get::<Transform>(copy);
            assert_eq!(at, x.world.get::<Transform>(original));
            assert_ne!(x.world.id(copy), Some(x.id(original)));
        }
        let sprite = x.world.get::<Sprite>(a).expect("copied");
        assert_eq!(sprite.texture, Some(x.texture), "opaque");
        assert_eq!(x.world.get::<Sealed>(g), Some(&Sealed(5)));
        let all = x.world.get::<Everything>(f).expect("copied");
        assert_eq!(all.target, Some(e), "its copy's");
        assert_eq!(all.handle, Handle(99));
        let all = x.world.get::<Everything>(x.f).expect("kept");
        assert_eq!(all.target, Some(x.e));
        check_ids(&x.world);
        check_links(&x.world);
        // Another target, outside what's copied, stays.
        let mut again = Operation::Duplicate {
            entity: x.id(x.f),
            copies: Vec::new(),
        };
        let redo = x.world.apply(&mut again).expect("applied");
        let again = x.world.entity(again.entity().expect("written in"));
        let all = x.world.get::<Everything>(again.expect("alive"));
        assert_eq!(all.expect("copied").target, Some(x.e));
        x.world.release(redo);
        // A child it didn't make, then one of its copies gone: refused, changing nothing.
        let extra = x.world.spawn(Transform::IDENTITY);
        x.world.set_parent(extra, Some(c)).expect("no cycle");
        for (code, case) in [
            ("ecs.has_children", "a child it didn't make"),
            ("ecs.no_entity", "a copy gone"),
        ] {
            if code == "ecs.no_entity" {
                x.world.despawn(extra).expect("alive");
                x.world.despawn(d).expect("alive");
            }
            let before = sorted(&x.world);
            let refused = x.world.undo(&mut inverse).expect_err(case);
            assert_eq!(refused.code(), code, "{refused}");
            assert!(same_snapshots(&sorted(&x.world), &before), "{case}");
        }
    }

    /// Refers to entities each way a value holds one, beside what a set refuses: a count above its
    /// max, and a rotation that isn't a unit, in a variant with an entity.
    #[derive(Clone, Debug, PartialEq, crate::Component)]
    struct Links {
        to: Option<Entity>,
        all: Vec<Entity>,
        tie: Tie,
        #[reflect(max = 9)]
        count: u8,
    }

    #[derive(Clone, Debug, PartialEq, crate::Reflect)]
    enum Tie {
        Turned { to: Entity, turn: Quat },
    }

    /// A duplicate points its copies' references to the originals at their copies, wherever a
    /// value holds them, without setting their components again: the rest of each is copied bit
    /// for bit, what a set refuses among it, as code may store it.
    #[test]
    fn a_duplicate_points_references_at_the_copies_keeping_what_a_set_refuses() {
        let mut x = fixture();
        let drifted = Quat::from_xyzw(-0.0, 0.0, 0.6, 0.9);
        let links = Links {
            to: Some(x.e),
            all: vec![x.c, x.r],
            tie: Tie::Turned {
                to: x.d,
                turn: drifted,
            },
            count: 200,
        };
        x.world.insert(x.b, links.clone()).expect("alive");
        let mut duplicate = Operation::Duplicate {
            entity: x.id(x.a),
            copies: Vec::new(),
        };
        x.world.apply(&mut duplicate).expect("applied");
        let Operation::Duplicate { copies, .. } = &duplicate else {
            unreachable!("applied as it was")
        };
        let copied = copies.iter().map(|&id| x.world.entity(id).expect("alive"));
        let [_, b, c, d, e] = copied.collect::<Vec<Entity>>()[..] else {
            panic!("five copies: {copies:?}");
        };
        let copy = x.world.get::<Links>(b).expect("copied");
        let expected = Links {
            to: Some(e),
            all: vec![c, x.r],
            tie: Tie::Turned {
                to: d,
                turn: drifted,
            },
            count: 200,
        };
        assert_eq!(copy, &expected, "the outside one kept");
        let bits = |turn: Quat| turn.to_array().map(f32::to_bits);
        let Tie::Turned { turn, .. } = copy.tie;
        assert_eq!(bits(turn), bits(drifted));
        let original = x.world.get::<Links>(x.b);
        assert_eq!(original, Some(&links), "the original's kept");
        check_ids(&x.world);
        check_links(&x.world);
    }

    /// A duplicate's copies of an exclusive field are false, so that it stays true on one entity at
    /// most: the original's.
    #[test]
    fn a_duplicates_exclusive_fields_are_false() {
        let (mut world, [lit, ..], _) = lamps();
        let mut duplicate = Operation::Duplicate {
            entity: world.id(lit).expect("alive"),
            copies: Vec::new(),
        };
        world.apply(&mut duplicate).expect("applied");
        let copy = world.entity(duplicate.entity().expect("written in"));
        let copy = world.get::<Lamp>(copy.expect("alive")).expect("copied");
        assert_eq!(
            copy,
            &Lamp {
                lit: false,
                watts: 40
            }
        );
        assert!(world.get::<Lamp>(lit).expect("kept").lit);
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
        assert_eq!(sprite.texture, Some(x.texture));
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
        let unknown = fresh();
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
                "reflect.duplicate_component",
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
            (
                Operation::Duplicate {
                    entity: unknown,
                    copies: Vec::new(),
                },
                "ecs.no_entity",
            ),
            // `a`'s subtree is five.
            (
                Operation::Duplicate {
                    entity: a,
                    copies: vec![fresh()],
                },
                "ops.bad_copies",
            ),
            (
                Operation::Duplicate {
                    entity: s,
                    copies: vec![e],
                },
                "ecs.id_taken",
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

    /// An ID no entity of the fixture's has.
    fn fresh() -> EntityId {
        "0123456789abcdef".parse().expect("an ID")
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
        let cases: [Case; 7] = [
            (
                "a set, then a despawn",
                |x| set(x, "count", Value::UInt(1)),
                |x| x.world.despawn(x.f).expect("alive"),
                "ecs.no_entity",
            ),
            (
                "a create, then a child given to what it made",
                |_| Operation::Create {
                    entity: Some(fresh()),
                    parent: None,
                    components: Vec::new(),
                },
                |x| {
                    let made = x.world.entity(fresh()).expect("created");
                    x.world.set_parent(x.g, Some(made)).expect("no cycle");
                },
                "ecs.has_children",
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

    /// A lamp, of which operations keep one lit at most.
    #[derive(Clone, Debug, PartialEq, crate::Reflect)]
    pub(in crate::ecs) struct Lamp {
        lit: bool,
        watts: u32,
    }

    impl crate::Component for Lamp {
        fn exclusive() -> Option<&'static str> {
            Some("lit")
        }
    }

    impl crate::Writable for Lamp {}

    /// Three lamps, the first two lit by code, and an entity without one.
    pub(in crate::ecs) fn lamps() -> (World, [Entity; 3], Entity) {
        let mut world = World::seeded(0x1a3b);
        let lamps = [true, true, false].map(|lit| world.spawn(Lamp { lit, watts: 40 }));
        let bare = world.spawn(());
        (world, lamps, bare)
    }

    /// Whether each of `lamps` is lit.
    pub(in crate::ecs) fn lit(world: &World, lamps: [Entity; 3]) -> [bool; 3] {
        lamps.map(|lamp| world.get::<Lamp>(lamp).expect("a lamp").lit)
    }

    /// A create, an add or a set that lights a lamp puts every other out, and undoes and redoes
    /// exactly, as one operation; one that puts a lamp out, or sets another field, puts none out.
    #[test]
    fn an_operation_that_makes_an_exclusive_field_true_makes_it_false_on_every_other_entity() {
        let (mut world, lamps, bare) = lamps();
        let [a, _, c] = lamps.map(|lamp| world.id(lamp).expect("alive"));
        let bare = world.id(bare).expect("alive");
        let lamp = Lamp {
            lit: true,
            watts: 75,
        };
        let set = |entity, path: &str, value| Operation::Set {
            entity,
            component: "Lamp".into(),
            path: path.to_owned(),
            value,
        };
        for (operation, after) in [
            (set(c, "lit", Value::Bool(true)), [false, false, true]),
            (set(c, "", lamp.to_value()), [false, false, true]),
            (set(a, "lit", Value::Bool(true)), [true, false, false]),
            (
                Operation::Create {
                    entity: None,
                    parent: None,
                    components: vec![named("Lamp", lamp.to_value())],
                },
                [false, false, false],
            ),
            (
                Operation::Add {
                    entity: bare,
                    component: "Lamp".into(),
                    value: lamp.to_value(),
                },
                [false, false, false],
            ),
            (set(a, "lit", Value::Bool(false)), [false, true, false]),
            (set(c, "watts", Value::UInt(60)), [true, true, false]),
        ] {
            let said = operation.to_string();
            let mut redo = round_trip(&mut world, operation);
            assert_eq!(lit(&world, lamps), [true, true, false], "{said}");
            world.undo(&mut redo).expect("redone");
            assert_eq!(lit(&world, lamps), after, "{said}");
            world.undo(&mut redo).expect("undone");
            world.release(redo);
        }
    }

    /// An undo that would light a lamp again that's gone, or that's no lamp, since is refused,
    /// changing nothing.
    #[test]
    fn an_undo_that_would_set_an_exclusive_field_again_where_it_cant_is_refused_whole() {
        type Case = (&'static str, fn(&mut World, Entity), &'static str);
        let cases: [Case; 2] = [
            (
                "a despawn",
                |world, lamp| world.despawn(lamp).expect("alive"),
                "ecs.no_entity",
            ),
            (
                "a remove",
                |world, lamp| assert!(world.remove::<Lamp>(lamp).expect("alive").is_some()),
                "reflect.missing_component",
            ),
        ];
        for (case, change, code) in cases {
            let (mut world, lamps, _) = lamps();
            let mut light = Operation::Set {
                entity: world.id(lamps[2]).expect("alive"),
                component: "Lamp".into(),
                path: "lit".to_owned(),
                value: Value::Bool(true),
            };
            let mut inverse = world.apply(&mut light).expect("lit");
            change(&mut world, lamps[0]);
            let (before, version) = (sorted(&world), world.version());
            // Twice, since a refusal leaves the inverse as it was too.
            for _ in 0..2 {
                let error = world.undo(&mut inverse).expect_err(case);
                assert_eq!(error.code(), code, "{case}: {error}");
                assert!(same_snapshots(&sorted(&world), &before), "{case}");
                assert_eq!(world.version(), version, "{case}");
            }
            world.release(inverse);
        }
    }

    /// Naming a field it hasn't its exclusive one.
    #[derive(Clone, Debug, PartialEq, crate::Reflect)]
    struct Unlit {
        watts: u32,
    }

    impl crate::Component for Unlit {
        fn exclusive() -> Option<&'static str> {
            Some("lit")
        }
    }

    /// Naming a field that isn't a `bool` its exclusive one.
    #[derive(Clone, Debug, PartialEq, crate::Reflect)]
    struct Dimmed {
        lit: u32,
    }

    impl crate::Component for Dimmed {
        fn exclusive() -> Option<&'static str> {
            Some("lit")
        }
    }

    #[test]
    fn a_components_exclusive_field_is_a_bool_of_its_own() {
        let mut world = World::default();
        let described = world.types().iter().len();
        for registered in [world.register::<Unlit>(), world.register::<Dimmed>()] {
            let error = registered.expect_err("refused");
            assert_eq!(error.code(), "ecs.bad_exclusive", "{error}");
        }
        assert_eq!(world.component_types().count(), 1, "`Parent` alone");
        // Nor are their descriptions kept, which would take their names.
        assert_eq!(world.types().iter().len(), described);
        world.register::<Lamp>().expect("registered");
    }
}
