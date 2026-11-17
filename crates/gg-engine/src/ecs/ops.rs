//! Operations: authoring changes by people and agents, naming entities by ID. Applying one gives
//! its inverse: another operation or, for a delete and a remove, what they took out, typed, so
//! opaque parts come back too.
//!
//! - A delete's inverse keeps its entities' slots and IDs, so undoing it restores each handle
//!   and every reference.
//! - A create's takes out only the entity it made, keeping it the same way.
//! - A set's keeps the whole typed component, so rolling back a partly refused edit restores
//!   exactly what the world held, even what a set of the old value would refuse.
//!
//! Only the [`Log`](super::Log) holds inverses, and it releases what they keep.

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
use crate::{ModelNode, ModelRoot, Prefab, PrefabRoot};

/// An authoring change to a world, by a person or an agent, which a [`Log`](super::Log) applies and
/// undoes.
///
/// It names entities by ID and carries reflection's values, whose entities are handles, as
/// [`World::set_field`] takes one.
///
/// Applying one checks everything first, so a refusal changes nothing. Refusals:
/// - `ecs.no_entity`: an entity it changes, or a parent it gives, isn't alive.
/// - Create, add, remove: `reflect.unknown_type` for an unregistered component type;
///   `reflect.read_only` for [`Parent`] (a create's parent and a reparent give it).
/// - Create: `reflect.duplicate_component` for a component given twice; `ecs.id_taken` for an ID
///   a living entity has.
/// - Add: `reflect.has_component` for one the entity has.
/// - Create and add also fail like `set_field` on a value, and like building the component on
///   its type's [`base`](crate::Component::base), such as `reflect.opaque` for a type opaque as a
///   whole ([`MeshDraw`](crate::MeshDraw)).
/// - Duplicate: `ecs.id_taken` for a copy's ID a living entity has or a delete keeps for its
///   undo; `ops.bad_copies` for copies' IDs not given one each.
/// - Remove: `reflect.missing_component`.
/// - Set: like `set_field`, except for a `PrefabRoot` (below).
/// - Reparent: like [`World::set_parent`].
///
/// An instance of a prefab ([`PrefabRoot`]) is one edit with its root. Creating a root with a
/// `PrefabRoot`, or adding or setting one, spawns the prefab's entities; undo takes them out.
/// A remove or a set takes out those it had and the components its prefab gave the root; undo
/// puts them back as they were, each instance taking its prefab file's changes since in a
/// development build. Further refusals:
/// - `ecs.prefab_entity`: an entity an instance's prefab gives that the operation would delete,
///   move, unlink, or give, change or remove a `PrefabRoot` of; or a component a prefab gives
///   that a remove would take away.
/// - The prefab's template won't load, as for a scene naming it, such as `scene.prefab_cycle`.
/// - `ecs.id_taken`: an ID of the instance's entities that a living entity has or, for a
///   duplicate's copy, a delete keeps.
/// - Duplicate: `ops.bad_copies` for a given ID that an instance's copy gives one of its
///   entities, mixed from its root copy's ID.
/// - Unlink: `ecs.no_prefab` for an entity without a `PrefabRoot`.
/// - Revert: an entity no instance gives, a component its prefab doesn't give it, or as a set of
///   the prefab's value.
/// - Link: `reflect.has_component` for an entity with a `PrefabRoot`; `ecs.prefab_missing` for
///   an entity or component the prefab gives that the subtree lacks under the IDs it mixes;
///   `ecs.prefab_parent` for one under another parent than the prefab's; `ecs.prefab_entity` for
///   one with a `PrefabRoot` the prefab doesn't give it.
///
/// A create, add or set that makes a component's [exclusive](crate::Component::exclusive) field
/// true makes it false on every other entity with the component; the inverse restores theirs.
/// A duplicate's copies have it false.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Spawns an entity with `components` (type name and whole value each, built as a scene's
    /// are, opaque parts as [`Value::Opaque`]), last of `parent`'s children or a root. Its ID is
    /// `entity`, or drawn if `None` (applying writes it in).
    Create {
        entity: Option<EntityId>,
        parent: Option<EntityId>,
        components: Vec<(Ident, Value)>,
    },
    /// Spawns a copy of `entity` and each descendant, components as they are, typed (opaque
    /// parts come too), except exclusive fields are false and references among the originals
    /// point to their copies, as a prefab's do. `entity`'s copy is the last of its parent's
    /// children or a root; each descendant's is under its parent's copy, in the originals'
    /// order. Copies' IDs are `copies`, `entity`'s first, parents before children, or drawn if
    /// empty (applying writes them in). Models' nodes' copies take none: they're their roots'
    /// copies' nodes, IDs mixed from those.
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
    /// Unlinks the instance rooted at `entity` from its prefab ([`PrefabRoot`]): removes its
    /// `PrefabRoot`; its entities keep their IDs and become its own.
    Unlink { entity: EntityId },
    /// Links `entity` to `prefab`, as an unlink's undo does: gives it a `PrefabRoot` and spawns
    /// nothing, as its subtree already holds the prefab's entities under the IDs the instance
    /// mixes, each with the components the prefab gives it (as an unlink leaves them and a save
    /// as a prefab makes them).
    Link { entity: EntityId, prefab: Prefab },
    /// Sets the field at `path` of `entity`'s `component` (all of it for an empty path) to the
    /// prefab's value, like `set_field`: from the instance's root's, except its own `Transform`;
    /// or the entity's, from the outermost instance whose prefab gives it.
    Revert {
        entity: EntityId,
        component: Ident,
        path: String,
    },
}

impl Operation {
    /// The entity it changes or creates (a duplicate's copy); `None` for a create or duplicate
    /// without IDs, until applied.
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
            | Self::Link { entity, .. }
            | Self::Revert { entity, .. } => Some(*entity),
        }
    }
}

/// The operation in words, as menus and diagnostics name it, without its values: `set
/// `Transform.scale` of 2b7e151628aed2a6`.
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
            Self::Link { entity, .. } => write!(f, "link {entity}"),
            Self::Revert {
                entity,
                component,
                path,
            } => write!(f, "revert `{}` of {entity}", reflect::join(component, path)),
        }
    }
}

/// What undoes an applied operation; [`World::undo`] and [`World::roll_back`] apply it. It
/// belongs to its world: its values hold that world's handles and component types, and a world
/// restored from a snapshot is another, which may have reissued them. A delete's, and an undone
/// create's, keeps its entities' slots and IDs until applied or [released](World::release); a
/// set's keeps its component until [settled](Self::settle), applied or released.
pub(super) struct Inverse {
    /// Its world's serial.
    world: u64,
    undo: Undo,
}

enum Undo {
    Apply(Operation),
    /// Applies `set`, the field's old value, checked like any set. `kept` is the whole old
    /// component in a one-row column, which a [roll back](World::roll_back) restores unchecked
    /// until settled. `before` is the same, kept past settling where the old value held opaque
    /// parts the set may have taken out (an element or variant): an undo refused for making
    /// what's opaque restores the field from it ([`World::set_back_from`]).
    Set {
        set: Operation,
        kept: Option<Box<dyn AnyColumn>>,
        before: Option<Box<dyn AnyColumn>>,
    },
    /// Gives `entity` back the component a remove took, in a one-row column of its own.
    Put {
        entity: EntityId,
        component: ComponentId,
        column: Box<dyn AnyColumn>,
    },
    /// Takes out the entities a create or duplicate made (IDs, root's first), keeping their
    /// slots and IDs like a delete, if the root's subtree is only them: the log undid what it
    /// gave them first, so anything else is the game's, which a delete would take too.
    Uncreate(Vec<EntityId>),
    /// Puts back the entities a delete took, in its order (children before parents, each
    /// parent's first to last, root last), or, if `created`, those a create or duplicate made
    /// that undoing took out.
    Restore {
        taken: Vec<Taken>,
        created: bool,
        /// The instances among them ([`World::kept_instances`]); each takes its prefab file's
        /// changes made while it was out.
        #[cfg(feature = "dev")]
        instances: Vec<(EntityId, Rc<Template>)>,
    },
    /// Applies `undo`, then sets the exclusive fields `excluded` to `to`: true to undo an
    /// operation that made them false for another entity's true, false to redo it.
    Exclusive {
        undo: Box<Undo>,
        excluded: Vec<Excluded>,
        to: bool,
    },
    /// Takes out of instance root `entity` what its prefab gave it (components `given`, entities
    /// `spawned` under it with their subtrees, its `PrefabRoot`), keeping slots and IDs like a
    /// delete, and puts back what `put` keeps, taken out by an add, set or remove of its
    /// `PrefabRoot`: undoing it restores its prefab's entities as they were.
    Relink {
        entity: EntityId,
        put: Linked,
        given: Vec<ComponentId>,
        spawned: Vec<EntityId>,
    },
    /// Gives instance root `entity` back the `PrefabRoot` an unlink took (a one-row column),
    /// checked like a link of its prefab, since the world may have changed; a
    /// [roll back](World::roll_back) skips the check, as it finds the world as the unlink left it.
    Link {
        entity: EntityId,
        column: Box<dyn AnyColumn>,
    },
}

/// What a prefab change took out of an instance's root, which [`Undo::Relink`] puts back: its
/// `PrefabRoot` if any (a one-row column), the components its prefab gave it, and the entities
/// its prefab gave under it (each top one's subtree in a delete's order).
struct Linked {
    link: Option<Box<dyn AnyColumn>>,
    row: Row,
    taken: Vec<Taken>,
    /// Its instance and those among the entities ([`World::kept_instances`]); each takes its
    /// prefab file's changes made while it was out.
    #[cfg(feature = "dev")]
    instances: Vec<(EntityId, Rc<Template>)>,
}

/// An entity's exclusive field made false when an operation made another entity's true.
#[derive(Clone)]
struct Excluded {
    entity: EntityId,
    component: Ident,
    field: &'static str,
}

/// An entity a delete, or a create undone, took out of the world.
struct Taken {
    /// Its handle, valid again on return: its slot is kept.
    entity: Entity,
    id: EntityId,
    parent: Option<Entity>,
    /// Its components, its `Parent` among them.
    row: Row,
}

impl World {
    /// Applies `operation` and returns its inverse; a create without an ID gets the one it
    /// drew. Refused, it changes nothing ([`Operation`]).
    pub(super) fn apply(&mut self, operation: &mut Operation) -> Result<Inverse> {
        let _zone = gg_core::zone!();
        let undo = self.applied(operation)?;
        // Written in so `exclude`, the Inspector's pick, and the log's and menus' names find
        // what a create or duplicate made.
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

    /// The sets, run ahead of a delete `operation`, that clear to `None` each field
    /// [`Component::cleared`](super::Component::cleared) names, on a surviving entity naming a
    /// deleted one, with the ID each named. Ordered by entity (query order), component type
    /// (registration order), then field. None for other operations or a delete of no living
    /// entity (refused).
    pub(super) fn clears(&self, operation: &Operation) -> Vec<(Operation, EntityId)> {
        let Operation::Delete { entity } = operation else {
            return Vec::new();
        };
        let Some(root) = self.entity(*entity) else {
            return Vec::new();
        };
        let mut sets = Vec::new();
        let mut gone: Option<HashSet<Entity>> = None;
        for (id, fields) in self.components.cleared() {
            let name = self.components.name(id);
            let Ok(holding) = self.entities(Some(name)) else {
                continue;
            };
            for holder in holding {
                let gone = gone.get_or_insert_with(|| {
                    let subtree = self.subtree(root).into_iter();
                    subtree.map(|(entity, _)| entity).collect()
                });
                if gone.contains(&holder) {
                    continue;
                }
                for &field in fields {
                    let named = self.get_field(holder, name, field);
                    let Ok(Value::Option(Some(named))) = named else {
                        continue;
                    };
                    if let Value::Entity(named) = *named
                        && gone.contains(&named)
                    {
                        let set = Operation::Set {
                            entity: self.id(holder).expect("alive"),
                            component: name.to_owned().into(),
                            path: field.to_owned(),
                            value: Value::Option(None),
                        };
                        sets.push((set, self.id(named).expect("alive, to be deleted")));
                    }
                }
            }
        }
        sets
    }

    /// Applies `inverse`, undoing its operation, and leaves in its place what redoes it.
    /// Refused, it changes neither the world nor `inverse`. Refusals:
    /// - `ops.other_world`: another world's inverse, such as one from before a restore.
    /// - Its operation, if it is one, as that is refused.
    /// - A link (an unlink's inverse): like a link, such as `ecs.prefab_missing` for a prefab
    ///   entity gone since.
    /// - Taking out what a create made: `ecs.no_entity` for the entity gone since;
    ///   `ecs.has_children` for children since that the create didn't make.
    /// - Giving back what a remove or delete took: `ecs.no_entity` for an entity gone since or a
    ///   root's parent gone; `reflect.has_component` for a component the entity has since;
    ///   `ecs.id_taken` for an ID a living entity has since (a scene load, say).
    /// - Resetting another entity's exclusive field: `ecs.no_entity` for the entity gone since;
    ///   `reflect.missing_component` for the component removed since.
    pub(super) fn undo(&mut self, inverse: &mut Inverse) -> Result<()> {
        let _zone = gg_core::zone!();
        self.check_world(inverse)?;
        self.revert(&mut inverse.undo, false)
    }

    /// Like [`undo`](Self::undo), but puts a set's kept component back whole and unchecked. For
    /// an operation applied, undone or redone just before, with only later ones rolled back
    /// since: restores exactly what was there, even what a set refuses (a rotation no longer a
    /// unit). Refused only where the world has changed since. A [settled](Inverse::settle) set
    /// inverse sets its field back, checked, as `undo` does.
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

    /// Applies `undo` and leaves its reverse in place. Refused, it changes neither the world nor
    /// `undo` ([`undo`](Self::undo)). If `exact`, a set's component goes back whole
    /// ([`roll_back`](Self::roll_back)).
    fn revert(&mut self, undo: &mut Undo, exact: bool) -> Result<()> {
        if let Undo::Exclusive {
            undo: inner,
            excluded,
            to,
        } = undo
        {
            // Checked first, so a refusal changes nothing.
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
                ..
            } = undo
        {
            return self.put_back(set, &mut **kept);
        }
        if let Undo::Relink { .. } = undo {
            return self.relink_back(undo);
        }
        // Checked like a link: the subtree may differ from the prefab's since the unlink (another
        // author took away an entity).
        if !exact && let Undo::Link { entity, column } = undo {
            let &PrefabRoot(prefab) = column.vec::<PrefabRoot>().first().expect("an unlink's");
            let link = Operation::Link {
                entity: *entity,
                prefab,
            };
            *undo = self.applied(&link)?;
            return Ok(());
        }
        let redo = match &*undo {
            Undo::Exclusive { .. } | Undo::Relink { .. } => unreachable!("reverted above"),
            // Rolled back, as the unlink left it.
            Undo::Link { entity, .. } => {
                let id = self
                    .component::<PrefabRoot>()
                    .expect("a `PrefabRoot` was taken");
                self.can_put(*entity, id)?;
                Undo::Apply(Operation::Unlink { entity: *entity })
            }
            Undo::Apply(operation) => self.applied(operation)?,
            Undo::Set { set, before, .. } => match self.applied(set) {
                Err(error) if error.code() == "reflect.opaque" => match before {
                    Some(before) => self.set_back_from(set, &**before)?,
                    None => return Err(error),
                },
                applied => applied?,
            },
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
        // Checked: what the inverse kept goes back, which can't fail.
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

    /// The IDs a create or duplicate made under live `root`, for its undo: the root's first,
    /// then its subtree's, except models' nodes and instances' entities, which go with their
    /// roots.
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

    /// Puts back the whole component `kept` holds, which `set` (a set's inverse) changed, and
    /// swaps `kept` for the current one; `set` becomes the redo with the old value. Exact and
    /// unchecked. A model's root gets its nodes again, as a set changing its model gives them.
    ///
    /// Fails, changing nothing, with `ecs.no_entity` for an entity gone since, and like
    /// [`get_field`](Self::get_field) for a component removed or a field gone.
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

    /// The redo of `set` (a set's inverse) where [`undo`](Self::undo) was refused for making
    /// what's opaque (a set took out a list element or switched an enum variant). The field at
    /// `set`'s path is swapped back from `before`, the pre-set component, opaque parts included;
    /// the rest of the component stays as it is.
    ///
    /// Fails, changing nothing, like [`get_field`](Self::get_field), and where the field's
    /// hints now refuse its value.
    fn set_back_from(&mut self, set: &Operation, before: &dyn AnyColumn) -> Result<Undo> {
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
        let id = self.named(component).expect("its field was read");
        let types = self.components.types();
        let to = reflect::resolve(types, &self.components.ty(id), path)?;
        reflect::check(types, &to.ty, &to.meta, value)?;
        let kept = self.copy_of(handle, component).expect("its field was read");
        let mut set_back = before.cloned();
        let at = self.location(handle).expect("alive");
        let tick = self.change_tick;
        let table = &mut self.tables[at.table as usize];
        let column = table.position(id).expect("its field was read");
        let row = at.row as usize;
        let column = table.row_mut(column, row, tick);
        match to.steps.split_first() {
            None => column.swap_row(row, &mut *set_back),
            Some((&first, rest)) => {
                let field = column.part_mut(row, first).expect("its field was read");
                let field = reflect::reach_mut(field, rest)?;
                let held = set_back.part_mut(0, first).expect("a field of its type");
                let held = reflect::reach_mut(held, rest).expect("where its value was read");
                let swapped = field.swap_reflected(held);
                assert!(
                    swapped,
                    "a field and the same field of a copy are of one type"
                );
            }
        }
        if Some(id) == self.component::<ModelRoot>() && now != *value {
            self.despawn_nodes(handle);
            self.fit_nodes(handle, None);
        }
        let before = drops_opaque(value, &now).then(|| kept.cloned());
        Ok(Undo::Set {
            set: Operation::Set {
                entity: *entity,
                component: component.clone(),
                path: path.clone(),
                value: now,
            },
            kept: Some(kept),
            before,
        })
    }

    /// Drops `inverse`, never to be applied: if this world's, the slots it keeps are freed and
    /// their IDs let go, so its entities are gone for good.
    pub(super) fn release(&mut self, inverse: Inverse) {
        if inverse.world != self.serial {
            return;
        }
        self.let_go(inverse.undo);
    }

    /// Merges `later` (a later set of the same field joining its gesture) into `inverse` (a
    /// set's): the exclusive fields `later` made false join those `inverse` names, so undoing
    /// the gesture makes them true. The rest of `later` is let go; the first set's inverse
    /// covers it.
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

    /// The inverse of applied `operation`, given `undo` (its inverse minus exclusive fields):
    /// each exclusive field it made true is made false here on every other entity, and the
    /// result sets them true again.
    fn exclude(&mut self, operation: &Operation, undo: Undo) -> Undo {
        // Each component it gave or set, and the path: a set reaches a field by its own path or
        // whole.
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
            | Operation::Unlink { .. }
            | Operation::Link { .. } => {
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

    /// Whether `excluded`'s field can be set again: its entity is alive with the component.
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

    fn applied(&mut self, operation: &Operation) -> Result<Undo> {
        let refused = |error: Error| {
            let message = format!("can't {operation}: {error}");
            error.with_message(message)
        };
        // A model's nodes belong to the model: they change with their root and take no children.
        let touched = match operation {
            Operation::Create { parent, .. } => [*parent, None],
            Operation::Reparent { entity, parent } => [Some(*entity), *parent],
            Operation::Duplicate { entity, .. }
            | Operation::Delete { entity }
            | Operation::Add { entity, .. }
            | Operation::Remove { entity, .. }
            | Operation::Set { entity, .. }
            | Operation::Unlink { entity }
            | Operation::Link { entity, .. }
            | Operation::Revert { entity, .. } => [Some(*entity), None],
        };
        for touched in touched
            .into_iter()
            .flatten()
            .filter_map(|id| self.entity(id))
        {
            self.not_a_node(touched).map_err(refused)?;
        }
        // An instance's prefab's entities and components: tools change but never take them away.
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
                // Its undo takes out its prefab's entities and model's nodes with it.
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
                // A link too: the relink below spawns its instance.
                let old = self.write_field(handle, component, path, value, true)?;
                let before = drops_opaque(&old, value)
                    .then(|| kept.as_ref().map(|kept| kept.cloned()))
                    .flatten();
                let undo = Undo::Set {
                    set: Operation::Set {
                        entity: *entity,
                        component: component.clone(),
                        path: path.clone(),
                        value: old,
                    },
                    kept,
                    before,
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
                // Another prefab: the old one goes back for the relink to take out.
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
            Operation::Link { entity, prefab } => {
                let handle = self.living(*entity).map_err(refused)?;
                self.link(handle, *prefab).map_err(refused)?;
                Undo::Apply(Operation::Unlink { entity: *entity })
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

    fn copy_of(&self, entity: Entity, name: &str) -> Option<Box<dyn AnyColumn>> {
        let at = self.location(entity)?;
        let table = &self.tables[at.table as usize];
        let column = table.position(self.components.named(name)?)?;
        Some(table.column_at(column).clone_row(at.row as usize))
    }

    /// [`fit_nodes`](Self::fit_nodes) for `entity` if `component`, given or lost, is its
    /// [`ModelRoot`].
    fn fit_root(&mut self, entity: Entity, component: ComponentId) {
        if self.component::<ModelRoot>() == Some(component) {
            self.fit_nodes(entity, None);
        }
    }

    /// Spawns an entity with `components` (type name and whole value each), last of `parent`'s
    /// children or a root, as a create does, and returns it. A `PrefabRoot` spawns its prefab's
    /// instance under it, the root's given components replacing the prefab's; a `ModelRoot`
    /// spawns its model's nodes. No log keeps it (a script's `gg.spawn`). Its ID is drawn and
    /// slot taken once nothing can refuse it, like [`World::spawn`].
    ///
    /// Fails, changing nothing, like a create: `ecs.no_entity` for a dead parent,
    /// `ecs.model_node` for a model's node parent.
    pub(crate) fn spawn_named(
        &mut self,
        parent: Option<Entity>,
        components: &[(Ident, Value)],
    ) -> Result<Entity> {
        let parent = match parent {
            Some(parent) => {
                self.locate(parent)?;
                self.not_a_node(parent)?;
                self.id(parent)
            }
            None => None,
        };
        let template = self.created_template(components)?;
        let id = self.create(None, parent, components, template.as_deref())?;
        let made = self.entity(id).expect("made");
        if let Some(template) = template {
            let ids = template.ids(id).expect("checked as it was made");
            self.spawn_instance(made, &Instance { template, ids });
        }
        self.fit_nodes(made, None);
        Ok(made)
    }

    /// An add kept by no log (a script's `gg.add`): gives `entity` `component`, built from
    /// `value` like a scene's.
    ///
    /// Fails, changing nothing, with `ecs.no_entity` for a dead `entity`, like an add, and like
    /// [`unlinked_type`](Self::unlinked_type) for its type.
    pub(crate) fn add_named(
        &mut self,
        entity: Entity,
        component: &str,
        value: &Value,
    ) -> Result<()> {
        let at = self.locate(entity)?;
        let id = self.unlinked_type(component)?;
        if self.tables[at.table as usize].has(id) {
            return Err(Error::new(
                "reflect.has_component",
                format!("it has a `{component}` already"),
            ));
        }
        let column = self.build(id, component, value)?;
        self.give(entity, at, id, column);
        Ok(())
    }

    /// A remove kept by no log (a script's `gg.remove`): takes `entity`'s `component` away.
    ///
    /// Fails, changing nothing, with `ecs.no_entity` for a dead `entity`, like a remove (such as
    /// `reflect.missing_component`), and like [`unlinked_type`](Self::unlinked_type) for its type.
    pub(crate) fn remove_named(&mut self, entity: Entity, component: &str) -> Result<()> {
        let at = self.locate(entity)?;
        let id = self.unlinked_type(component)?;
        self.column(at, id, component)?;
        drop(self.take_out(entity, at, id));
        Ok(())
    }

    /// The id of the type `name` a script may give and take away: not [`Parent`] or
    /// [`ModelNode`] ([`writable_type`](Self::writable_type)), nor [`PrefabRoot`] or
    /// [`ModelRoot`] (the world spawns and despawns entities under them).
    ///
    /// Fails with `reflect.unknown_type` for an unknown type and `reflect.read_only` for those.
    pub(crate) fn unlinked_type(&self, name: &str) -> Result<ComponentId> {
        let id = self.writable_type(name)?;
        if Some(id) == self.component::<PrefabRoot>() {
            return Err(Error::new(
                "reflect.read_only",
                "the world keeps `PrefabRoot`, spawning its prefab's entities under the instance's \
                 root: a spawn of the prefab gives it",
            ));
        }
        if Some(id) == self.component::<ModelRoot>() {
            return Err(Error::new(
                "reflect.read_only",
                "the world spawns a `ModelRoot`'s model's nodes under it, and despawns them with \
                 it: a script gives and takes away none",
            ));
        }
        Ok(id)
    }

    /// Fails with `ecs.prefab_entity` if `operation` would take away what an instance's prefab
    /// gives: deleting, moving or unlinking its entities; adding, changing or removing a
    /// `PrefabRoot` on one (nested instances' belong to the outer prefab); or removing a
    /// component the prefab gives an entity or the root.
    fn prefabs_kept(&self, operation: &Operation) -> Result<()> {
        let linked = self.component::<PrefabRoot>();
        let links = |name: &str| linked.is_some() && self.components.named(name) == linked;
        let (id, component) = match operation {
            Operation::Delete { entity }
            | Operation::Reparent { entity, .. }
            | Operation::Unlink { entity }
            | Operation::Link { entity, .. } => (*entity, None),
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
    /// Fails like the `PrefabRoot`'s value and the template.
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

    /// Changes `root`'s prefab to the one `link` (a one-row `PrefabRoot` column) names, or
    /// removes it without one. Takes out its `PrefabRoot`, the components its prefab gave it
    /// except `Transform`, and the entities its prefab gave under it with their subtrees,
    /// keeping slots and IDs like a delete; then gives it `link` and its prefab's instance
    /// ([`spawn_instance`](Self::spawn_instance)). Returns the inverse.
    ///
    /// Fails, changing nothing, like the instances' templates, and with `ecs.id_taken` for an ID
    /// of the new instance's entities that a living entity has (not one taken out).
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

    /// Applies `undo`, an [`Undo::Relink`], leaving its reverse in place. Refused, it changes
    /// neither the world nor `undo`: `ecs.no_entity` for the root gone since; `ecs.id_taken` for
    /// an ID going back that a living entity has since (not those taken out);
    /// `reflect.has_component` for a component going back that the root has since.
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
        // The root's own: what goes back doesn't give them.
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
        // What its prefab gives it now (undoing takes it out): what went back, plus its prefab
        // file's changes while out.
        let link_type = self.component::<PrefabRoot>();
        let at = self.location(root).expect("alive");
        let types = self.tables[at.table as usize].components().iter().copied();
        let types =
            types.filter(|&component| !own.contains(&component) && Some(component) != link_type);
        *given = types.collect();
        // Not its model's nodes, which come and go with its `ModelRoot`.
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

    /// The IDs of the live `tops` and their descendants.
    fn subtree_ids(&self, tops: &[Entity]) -> HashSet<EntityId> {
        let subtrees = tops.iter().flat_map(|&top| self.subtree(top));
        subtrees
            .map(|(entity, _)| self.id(entity).expect("alive"))
            .collect()
    }

    /// Takes out of `root` its `PrefabRoot`, its components of the types `given`, and `tops`
    /// (under it) with their subtrees, keeping slots and IDs like a delete; and its model's
    /// nodes if its `ModelRoot` goes. Returns what it took, for an [`Undo::Relink`].
    fn take_link(&mut self, root: Entity, given: &[ComponentId], tops: &[Entity]) -> Linked {
        // Found before they're taken out: only the root's and those among what's taken.
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

    /// Swaps `entity`'s component `id` with `column`'s one row, unchecked (a roll back).
    fn swap_component(&mut self, entity: Entity, id: ComponentId, column: &mut dyn AnyColumn) {
        let at = self.location(entity).expect("alive");
        let tick = self.change_tick;
        let table = &mut self.tables[at.table as usize];
        let position = table.position(id).expect("the entity has the component");
        (table.row_mut(position, at.row as usize, tick)).swap_row(at.row as usize, column);
    }

    /// Spawns a create's entity and returns its ID, which leaves free the IDs its `template`
    /// instance (if it links a prefab) takes.
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
        // Only once nothing can refuse: a refusal draws no ID.
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

    /// Takes live `root` and its descendants out, keeping slots and IDs, in
    /// [`take_apart`](Self::take_apart)'s order: children before parents, root last.
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

    /// The root of what a create or duplicate made (`made`, root's ID first), if undo can take
    /// them out: the root is alive and its subtree is exactly them, except models' nodes and
    /// instances' entities, which go with their roots whatever their models and prefabs have now.
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

    /// Copies `entity`'s subtree ([`Operation::Duplicate`]) under `ids` (drawn if empty) and
    /// returns the copies' IDs, `entity`'s first, omitting models' nodes' copies.
    fn duplicate(&mut self, entity: EntityId, ids: &[EntityId]) -> Result<Vec<EntityId>> {
        let root = self.living(entity)?;
        let originals = self.subtree(root);
        let is_node: Vec<bool> = (originals.iter())
            .map(|&(original, _)| self.get::<ModelNode>(original).is_some())
            .collect();
        // Originals an instance in the subtree gives, by instance and part: the instance's copy
        // gives their copies, IDs mixed from the copy's root's.
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
            // IDs each instance's copy gives its entities, mixed from its root copy's: none
            // given to another copy, all free.
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
        // Each original's components, copied before anything changes, except its `Parent`
        // (its copy's is its own).
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
            // A node's copy is its root copy's node, ID mixed from the root's, or drawn if that's
            // taken or given: a given ID goes only to the copy it's for.
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
            // An instance entity's copy has an ID mixed from its root copy's, free by the checks
            // above or the root copy's draw; drawn only if another copy took it by chance.
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
                // An instance root's copy: drawn until its instance copy's IDs are free, as in a
                // create.
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
        // References among the originals point to their copies, typed, so the rest stays as
        // copied even where a set would refuse it; exclusive fields are false.
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

    /// Puts `taken` back, each in its slot under its handle and ID.
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
        // In the order the delete took them: each parent's children are as they were and the
        // root is the last of its parent's.
        for (parent, child) in links {
            self.append(parent, child);
        }
        // A value naming a put-back entity names a living one again, with no mark since nothing
        // touched its column (a row's text, `Dead` while out, is its ID again). So mark every
        // row, as a rewind does, for what skips tables unmarked since, like a server's states.
        let tick = self.change_tick;
        for table in &mut self.tables {
            table.mark_all(tick);
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
    /// column, checked as `set_field` checks a value.
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

    /// A copy for a log copy; still this world's. It keeps what this keeps, so once
    /// [rebound](Self::rebind) to a world copy taken meanwhile it undoes there as this does here.
    #[cfg(any(feature = "dev", test))]
    pub(super) fn copied(&self) -> Inverse {
        fn copied(undo: &Undo) -> Undo {
            match undo {
                Undo::Apply(operation) => Undo::Apply(operation.clone()),
                Undo::Set { set, kept, before } => Undo::Set {
                    set: set.clone(),
                    kept: kept.as_ref().map(|kept| kept.cloned()),
                    before: before.as_ref().map(|before| before.cloned()),
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

    /// Makes it the world's with serial `to` if it's `from`'s, as [`copied`](Self::copied) says.
    #[cfg(any(feature = "dev", test))]
    pub(super) fn rebind(&mut self, from: u64, to: u64) {
        if self.world == from {
            self.world = to;
        }
    }
}

/// Whether `value` holds [`Value::Opaque`] anywhere, which a set of it may take out.
fn holds_opaque(value: &Value) -> bool {
    match value {
        Value::Opaque => true,
        Value::Option(Some(value)) => holds_opaque(value),
        Value::List(items) => items.iter().any(holds_opaque),
        Value::Struct(fields) | Value::Variant(_, fields) => {
            fields.iter().any(|(_, value)| holds_opaque(value))
        }
        _ => false,
    }
}

/// Whether setting `old` to `new` may take out something opaque, which a set's undo can't
/// remake: removing or adding a list element, switching an enum variant, making an option
/// `None`, or giving another kind of value, over something holding opaque parts. Struct fields
/// go by name.
fn drops_opaque(old: &Value, new: &Value) -> bool {
    match (old, new) {
        (Value::List(old), Value::List(new)) if old.len() == new.len() => {
            old.iter().zip(new).any(|(old, new)| drops_opaque(old, new))
        }
        (Value::Struct(old), Value::Struct(new)) => fields_drop_opaque(old, new),
        (Value::Variant(was, old), Value::Variant(is, new)) if was == is => {
            fields_drop_opaque(old, new)
        }
        (Value::Option(Some(old)), Value::Option(Some(new))) => drops_opaque(old, new),
        (Value::Opaque, Value::Opaque) => false,
        (old, _) => holds_opaque(old),
    }
}

/// Whether setting fields `old` to `new`, by name, may take out something opaque, as
/// [`drops_opaque`] says.
fn fields_drop_opaque(old: &[(Ident, Value)], new: &[(Ident, Value)]) -> bool {
    old.iter().any(
        |(name, old)| match new.iter().find(|(given, _)| given == name) {
            Some((_, new)) => drops_opaque(old, new),
            None => holds_opaque(old),
        },
    )
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

    /// Whether a set's inverse keeps the component as it was before, for its undo.
    pub(super) fn keeps_before(&self) -> bool {
        matches!(
            &self.undo,
            Undo::Set {
                before: Some(_),
                ..
            }
        )
    }
}

#[cfg(test)]
pub(super) mod tests;
