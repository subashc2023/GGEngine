//! The log of the edits applied to a world: what undo and redo step through.

use gg_core::{Error, Result};
use tracing::field::display;
use tracing::{debug, warn};

use super::World;
use super::ops::{Inverse, Operation};
use crate::reflect::Value;

/// The edits applied to a world, in order, each with its operations and what undoes them. An
/// edit's operations undo and redo together; a gesture's sets of one field (a drag) merge into
/// one edit.
///
/// An entry whose undo or redo puts entities back (a delete done, a create undone) keeps their
/// typed components, slots and IDs, which no draw gives out meanwhile, and gives each back under
/// its handle and ID. It does so until the entry is dropped by:
/// - an edit applied after an undo (drops what was undone);
/// - an undo or redo the world refuses;
/// - [`clear`](Self::clear).
///
/// A log dropped without `clear` leaves those slots unused for good, so whoever keeps one clears
/// it. A world restored from a snapshot, [cleared](World::clear), or with entities
/// [replaced](World::replace_scene) by a scene refuses older entries as another world's (a
/// restore leaves the slots kept at snapshot time unused for good). So whoever restores, clears
/// or replaces a world clears its log.
///
/// An entry applied before the world's scene changed ([`World::change_scene`]) may find other
/// entities under its IDs. It refuses to undo or redo (`ops.scene_changed`) and stays, keeping
/// what it kept, so it neither changes a scene entity nor frees a slot another peer's log holds.
///
/// The last entry done keeps each component its sets changed, whole, so a refused append rolls
/// its edit back exactly. It lets them go once another entry is done or it's undone.
#[derive(Default)]
pub struct Log {
    /// Applied and not undone, the first applied first.
    done: Vec<Entry>,
    /// Undone, the last undone last.
    undone: Vec<Entry>,
    /// Whether the last entry done is a gesture still going, which [`amend`](Self::amend) merges
    /// sets into.
    open: bool,
}

/// An edit: its operations as applied, and the inverses of each and of the sets a delete clears
/// with ([`Log::apply_all`]), in applied order. `of` maps each inverse to its operation's index,
/// so the Inspector's undo shows the operations the author gave.
struct Entry {
    operations: Vec<Operation>,
    inverses: Vec<Inverse>,
    of: Vec<usize>,
    /// The world's [`scene_changes`](World::scene_changes) when applied.
    scenes: u64,
}

impl Log {
    /// Applies `operation` and logs it as its own edit, dropping what was undone. A create
    /// without an ID is logged with the one it drew. Refused, it changes and logs nothing
    /// ([`Operation`]).
    pub fn apply(&mut self, world: &mut World, operation: Operation) -> Result<()> {
        self.apply_all(world, vec![operation])
    }

    /// Applies `operations` in order as one edit that undoes and redoes whole, and logs it. All
    /// or none: a refusal rolls back the earlier ones exactly, even where code had stored in a
    /// component a set changed what a set refuses, and logs nothing. No operations log nothing.
    ///
    /// Each delete first clears, with sets, the fields its components'
    /// [`cleared`](crate::Component::cleared) names on surviving entities that name an entity it
    /// deletes. The log keeps those sets with the delete, outside the operations it shows. A
    /// field whose entity the edit makes again under its ID (a scene file's relink of an
    /// instance) is set to it after the edit's operations apply, as a scene file names it.
    ///
    /// Fails like the first refused operation, naming it if there are others; a refused clearing
    /// set fails like the delete.
    pub fn apply_all(&mut self, world: &mut World, operations: Vec<Operation>) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        let count = operations.len();
        let mut inverses = Vec::with_capacity(count);
        let mut of = Vec::with_capacity(count);
        let mut applied: Vec<Operation> = Vec::with_capacity(count);
        // Each set a delete cleared with, the delete's index, and the ID of what it named.
        let mut cleared = Vec::new();
        for (i, mut operation) in operations.into_iter().enumerate() {
            for (mut set, named) in world.clears(&operation) {
                match world.apply(&mut set) {
                    Ok(inverse) => {
                        inverses.push(inverse);
                        of.push(i);
                        cleared.push((i, set, named));
                    }
                    Err(error) => {
                        roll_back(world, inverses);
                        return Err(refused(error, count, i, &operation, true));
                    }
                }
            }
            match world.apply(&mut operation) {
                Ok(inverse) => {
                    inverses.push(inverse);
                    of.push(i);
                    applied.push(operation);
                }
                Err(error) => {
                    roll_back(world, inverses);
                    return Err(refused(error, count, i, &operation, false));
                }
            }
        }
        for (i, set, named) in cleared {
            let Operation::Set {
                entity,
                component,
                path,
                ..
            } = set
            else {
                unreachable!("a clear is a set");
            };
            let (Some(holder), Some(remade)) = (world.entity(entity), world.entity(named)) else {
                continue;
            };
            let now = world.get_field(holder, &component, &path);
            if !matches!(now, Ok(Value::Option(None))) {
                continue;
            }
            let mut set = Operation::Set {
                entity,
                component,
                path,
                value: Value::Option(Some(Box::new(Value::Entity(remade)))),
            };
            match world.apply(&mut set) {
                Ok(inverse) => {
                    inverses.push(inverse);
                    of.push(i);
                }
                Err(error) => {
                    roll_back(world, inverses);
                    return Err(refused(error, count, i, &applied[i], true));
                }
            }
        }
        let operations = applied;
        for operation in &operations {
            debug!(
                code = "ops.applied",
                entity = operation.entity().map(display),
                operation = %operation,
                "Applied an operation"
            );
        }
        for dropped in self.undone.drain(..) {
            release(world, dropped);
        }
        self.push_done(Entry {
            operations,
            inverses,
            of,
            scenes: world.scene_changes(),
        });
        self.open = false;
        Ok(())
    }

    /// Applies `operations` as the rest of the last edit applied, undoing and redoing with it:
    /// for operations whose values hold entities that edit made, which have handles once it's
    /// applied. Refused, they change nothing and that edit is rolled back exactly and dropped,
    /// so the two are all or nothing. If the world has moved on since and refuses that roll
    /// back, the edit stays applied and logged. With nothing applied and not undone, they apply
    /// as an edit of their own.
    ///
    /// Fails like [`apply_all`](Self::apply_all).
    pub(crate) fn append(&mut self, world: &mut World, operations: Vec<Operation>) -> Result<()> {
        let Some(mut last) = self.done.pop() else {
            return self.apply_all(world, operations);
        };
        let mut rest = Log::default();
        match rest.apply_all(world, operations) {
            Ok(()) => {
                let entry = rest.done.pop();
                let entry = entry.unwrap_or(Entry {
                    operations: Vec::new(),
                    inverses: Vec::new(),
                    of: Vec::new(),
                    scenes: last.scenes,
                });
                let before = last.operations.len();
                last.operations.extend(entry.operations);
                last.inverses.extend(entry.inverses);
                last.of.extend(entry.of.into_iter().map(|i| before + i));
                self.done.push(last);
                self.open = false;
                Ok(())
            }
            Err(error) => {
                let order: Vec<usize> = (0..last.inverses.len()).rev().collect();
                match turn(world, &mut last.inverses, &order, World::roll_back) {
                    Ok(()) => release(world, last),
                    Err((i, refused)) => {
                        let operation = &last.operations[last.of[i]];
                        warn!(
                            code = "ops.kept",
                            refused = refused.code(),
                            entity = operation.entity().map(display),
                            operation = %operation,
                            "can't roll back {operation} as an append to its edit is refused: \
                             {refused}; the edit stays applied and logged"
                        );
                        self.done.push(last);
                    }
                }
                Err(error)
            }
        }
    }

    /// Like [`apply`](Self::apply), but during a gesture a set of the field the last entry set
    /// joins that entry: it takes the new value and keeps the entry's inverse plus the exclusive
    /// fields each set made false, so one undo undoes the gesture. A set starts a gesture;
    /// [`seal`](Self::seal) ends it. A [`PrefabRoot`](crate::PrefabRoot) set joins none: each
    /// relinks its instance, and the first set's inverse would take out only its own spawns.
    ///
    /// It is [`amend_all`](Self::amend_all) of one operation.
    pub fn amend(&mut self, world: &mut World, operation: Operation) -> Result<()> {
        self.amend_all(world, vec![operation])
    }

    /// [`amend`](Self::amend) for each of `operations`, all or none. A gesture's entry is sets
    /// of one field of one component, on one entity or several (a drag of several handles). If
    /// `operations` all set that field, they join it: a set of an entity the entry already sets
    /// takes the new value and keeps the entry's inverse; a set of another entity is added with
    /// its own inverse. One undo undoes every entity's sets. A refusal rolls back this call's
    /// sets and leaves the entry as it was.
    ///
    /// Otherwise they apply like [`apply_all`](Self::apply_all), as their own entry, and a
    /// refusal changes nothing. That entry starts a gesture if they're sets of one field, no two
    /// of one entity; [`seal`](Self::seal) ends it. Empty `operations` do nothing.
    pub fn amend_all(&mut self, world: &mut World, mut operations: Vec<Operation>) -> Result<()> {
        let field = set_field(&operations);
        let link = world.component::<crate::PrefabRoot>();
        let relinks = field.is_some_and(|(component, _)| {
            link.is_some() && world.component_named(component) == link
        });
        let scenes = world.scene_changes();
        let joins = self.open
            && field.is_some()
            && !relinks
            && self.done.last().is_some_and(|entry| {
                entry.scenes == scenes
                    && set_field(&entry.operations) == field
                    && entry.inverses.len() == entry.operations.len()
            });
        if !joins {
            // A gesture goes on from an entry of sets of one field, of an entity each.
            let mut entities: Vec<_> = operations.iter().filter_map(Operation::entity).collect();
            entities.sort_unstable();
            let distinct = entities.windows(2).all(|pair| pair[0] != pair[1]);
            let gesture = field.is_some() && distinct;
            self.apply_all(world, operations)?;
            self.open = gesture;
            return Ok(());
        }
        let mut laters = Vec::with_capacity(operations.len());
        for i in 0..operations.len() {
            match world.apply(&mut operations[i]) {
                Ok(later) => laters.push(later),
                Err(error) => {
                    roll_back(world, laters);
                    return Err(refused(error, operations.len(), i, &operations[i], false));
                }
            }
        }
        // What undoes an entity's first set in the gesture undoes its later ones too, except the
        // exclusive fields these made false, which the entry takes on.
        let entry = self.done.last_mut().expect("an entry joined");
        for (operation, later) in operations.into_iter().zip(laters) {
            let at = entry
                .operations
                .iter()
                .position(|set| set.entity() == operation.entity());
            match at {
                Some(at) => {
                    world.join(&mut entry.inverses[at], later);
                    entry.operations[at] = operation;
                }
                None => {
                    entry.of.push(entry.operations.len());
                    entry.operations.push(operation);
                    entry.inverses.push(later);
                }
            }
        }
        Ok(())
    }

    /// Ends the gesture going on: the next set is an entry of its own.
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Undoes the last edit applied and not undone; returns whether there was one. Each set
    /// sets its field back, checked like any set, so later changes to the component's other
    /// fields stay.
    ///
    /// Refused when an entity it changed is gone, a created entity has children the create
    /// didn't make, or a set field holds what a set refuses (code may have stored it). Refused,
    /// it changes nothing and its entry is dropped with what it kept: the world has moved on and
    /// it would block earlier entries. The error says why and is logged as `ops.dropped`. An
    /// entry from before a scene change is refused too but stays (`ops.scene_changed`, see
    /// [`Log`]).
    pub fn undo(&mut self, world: &mut World) -> Result<bool> {
        self.step(world, Step::Undo)
    }

    /// Redoes the last edit undone; returns whether there was one. Refused like
    /// [`undo`](Self::undo).
    pub fn redo(&mut self, world: &mut World) -> Result<bool> {
        self.step(world, Step::Redo)
    }

    /// The operations of the edit [`undo`](Self::undo) undoes next.
    pub fn undoable(&self) -> Option<&[Operation]> {
        self.done.last().map(|entry| entry.operations.as_slice())
    }

    /// The operations of the edit [`redo`](Self::redo) redoes next.
    pub fn redoable(&self) -> Option<&[Operation]> {
        self.undone.last().map(|entry| entry.operations.as_slice())
    }

    /// The operations of each edit applied and not undone, the first applied first.
    pub fn edits(&self) -> impl ExactSizeIterator<Item = &[Operation]> {
        self.done.iter().map(|entry| entry.operations.as_slice())
    }

    /// The operations of each edit undone, the last undone last (redone first).
    pub fn undone(&self) -> impl ExactSizeIterator<Item = &[Operation]> {
        self.undone.iter().map(|entry| entry.operations.as_slice())
    }

    /// A copy for a copy of the world, as a SyncTest reruns a tick. Its entries keep what the
    /// log's keep and still belong to the world, so keep it aside as the log was when the tick
    /// started, and [rebind](Self::rebind) it to the world copy taken then. Never use it in the
    /// world itself: it and the log would each give back what they keep.
    #[cfg(any(feature = "dev", test))]
    pub(crate) fn copied(&self) -> Log {
        let copied = |entries: &[Entry]| -> Vec<Entry> {
            let entry = |entry: &Entry| Entry {
                operations: entry.operations.clone(),
                inverses: entry.inverses.iter().map(Inverse::copied).collect(),
                of: entry.of.clone(),
                scenes: entry.scenes,
            };
            entries.iter().map(entry).collect()
        };
        Log {
            done: copied(&self.done),
            undone: copied(&self.undone),
            open: self.open,
        }
    }

    /// Rebinds entries of the world with serial `from` to `to`, its
    /// [copy](Self::copied). Another world's stay its own, refused in both.
    #[cfg(any(feature = "dev", test))]
    pub(crate) fn rebind(&mut self, from: u64, to: u64) {
        for entry in self.done.iter_mut().chain(&mut self.undone) {
            for inverse in &mut entry.inverses {
                inverse.rebind(from, to);
            }
        }
    }

    /// Drops every entry, and what they kept, from `world`, which they were applied to.
    pub fn clear(&mut self, world: &mut World) {
        for dropped in self.done.drain(..).chain(self.undone.drain(..)) {
            release(world, dropped);
        }
        self.open = false;
    }

    /// Undoes or redoes the last entry, its inverses last first. Each turns into its own
    /// reverse, so a [`turn`] refused partway rolls back those done.
    fn step(&mut self, world: &mut World, step: Step) -> Result<bool> {
        self.open = false;
        let from = match step {
            Step::Undo => &mut self.done,
            Step::Redo => &mut self.undone,
        };
        let (doing, code, done) = match step {
            Step::Undo => ("undo", "ops.undone", "Undid an operation"),
            Step::Redo => ("redo", "ops.redone", "Redid an operation"),
        };
        let scenes = world.scene_changes();
        if let Some(entry) = from.last()
            && entry.scenes != scenes
        {
            let operation = &entry.operations[0];
            let message = format!(
                "can't {doing} {operation}, applied before the world's scene changed, which may \
                 have put another entity under its ID: the edit stays in the log, refused"
            );
            return Err(Error::new("ops.scene_changed", message));
        }
        let Some(mut entry) = from.pop() else {
            return Ok(false);
        };
        // Undone last first, redone first first, so each finds the world as the one after it (or
        // before it) left it.
        let order: Vec<usize> = match step {
            Step::Undo => (0..entry.inverses.len()).rev().collect(),
            Step::Redo => (0..entry.inverses.len()).collect(),
        };
        if let Err((i, error)) = turn(world, &mut entry.inverses, &order, World::undo) {
            let operation = &entry.operations[entry.of[i]];
            let message = format!("can't {doing} {operation}: {error}");
            warn!(
                code = "ops.dropped",
                refused = error.code(),
                entity = operation.entity().map(display),
                operation = %operation,
                "{message}; dropped its edit from the log"
            );
            release(world, entry);
            return Err(error.with_message(message));
        }
        for operation in &entry.operations {
            let entity = operation.entity().map(display);
            debug!(code, entity, operation = %operation, "{done}");
        }
        match step {
            // Nothing rolls back an entry undone: a redo applies it afresh.
            Step::Undo => {
                entry.inverses.iter_mut().for_each(Inverse::settle);
                self.undone.push(entry);
            }
            Step::Redo => self.push_done(entry),
        }
        Ok(true)
    }

    /// Pushes `entry` as the last done, which keeps its roll-back data; the one before lets its
    /// own go, as no append rolls it back.
    fn push_done(&mut self, entry: Entry) {
        if let Some(before) = self.done.last_mut() {
            before.inverses.iter_mut().for_each(Inverse::settle);
        }
        self.done.push(entry);
    }
}

/// Applies `inverses` in `order` through `apply` ([`World::undo`] or [`World::roll_back`]),
/// which leaves each one's reverse in its place. Refused partway, it rolls back those applied,
/// last first, leaving the world exactly as it was, and fails with the refused index and why.
fn turn(
    world: &mut World,
    inverses: &mut [Inverse],
    order: &[usize],
    apply: fn(&mut World, &mut Inverse) -> Result<()>,
) -> std::result::Result<(), (usize, Error)> {
    for (n, &i) in order.iter().enumerate() {
        if let Err(error) = apply(world, &mut inverses[i]) {
            for &back in order[..n].iter().rev() {
                world
                    .roll_back(&mut inverses[back])
                    .expect("an inverse just applied rolls back");
            }
            return Err((i, error));
        }
    }
    Ok(())
}

/// The component and field that `operations` all set, if there are any and all set one field.
fn set_field(operations: &[Operation]) -> Option<(&str, &str)> {
    let mut field = None;
    for operation in operations {
        let Operation::Set {
            component, path, ..
        } = operation
        else {
            return None;
        };
        let this = (&component[..], &path[..]);
        if *field.get_or_insert(this) != this {
            return None;
        }
    }
    field
}

/// Rolls back the `inverses` of an edit refused partway, in reverse; each restores the world as
/// the one before it found it.
fn roll_back(world: &mut World, inverses: Vec<Inverse>) {
    for mut inverse in inverses.into_iter().rev() {
        world
            .roll_back(&mut inverse)
            .expect("an operation just applied rolls back");
        world.release(inverse);
    }
}

/// `error` as the refusal of `operation` (the `i`th of `count`, or a clearing set if `clearing`),
/// naming it if there are others.
fn refused(error: Error, count: usize, i: usize, operation: &Operation, clearing: bool) -> Error {
    // The set's own refusal says which set: `can't set ... of ...: ...`.
    let error = if clearing {
        let message = format!("can't {operation}: {error}");
        error.with_message(message)
    } else {
        error
    };
    if count == 1 {
        return error;
    }
    let message = format!("operation {}, {operation}: {error}", i + 1);
    error.with_message(message)
}

/// Releases what `entry`'s inverses keep from `world`.
fn release(world: &mut World, entry: Entry) {
    for inverse in entry.inverses {
        world.release(inverse);
    }
}

#[derive(Clone, Copy)]
enum Step {
    Undo,
    Redo,
}

#[cfg(test)]
impl Log {
    /// How many slots its entries keep.
    pub(super) fn kept(&self) -> usize {
        let entries = self.done.iter().chain(&self.undone);
        let inverses = entries.flat_map(|entry| &entry.inverses);
        inverses.map(Inverse::kept).sum()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use gg_core::math::{DVec3, Quat, Vec3, Vec4};

    use super::*;
    use crate::ecs::hierarchy::tests::check_links;
    use crate::ecs::id::tests::{check_ids, vacated};
    use crate::ecs::id::{IdStream, splitmix64};
    use crate::ecs::ops::tests::{Sealed, fixture, lamps, lit, sorted};
    use crate::ecs::tests::Rng;
    use crate::reflect::tests::{Everything, Handle, everything, same_snapshots};
    use crate::reflect::{Ident, Value};
    use crate::{Entity, EntityId, Parent, Reflect, Sprite, Transform};

    /// A set of `entity`'s `Everything` field at `path`.
    fn set(entity: EntityId, path: &str, value: Value) -> Operation {
        Operation::Set {
            entity,
            component: "Everything".into(),
            path: path.to_owned(),
            value,
        }
    }

    fn count(entity: EntityId, count: u64) -> Operation {
        set(entity, "count", Value::UInt(count))
    }

    /// A create of an entity without components.
    const EMPTY: Operation = Operation::Create {
        entity: None,
        parent: None,
        components: Vec::new(),
    };

    #[test]
    fn undo_and_redo_step_through_the_log_in_order() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        let operations = [
            count(f, 1),
            // -0.0 over 0.0: `==` calls them equal, the scene doesn't; logged all the same.
            Operation::Set {
                entity: x.id(x.r),
                component: "Transform".into(),
                path: "translation.x".to_owned(),
                value: Value::F64(-0.0),
            },
            Operation::Delete { entity: x.id(x.a) },
            Operation::Create {
                entity: None,
                parent: Some(x.id(x.s)),
                components: vec![("Transform".into(), Transform::IDENTITY.to_value())],
            },
            Operation::Reparent {
                entity: x.id(x.g),
                parent: Some(x.id(x.r)),
            },
        ];
        let mut states = vec![sorted(&x.world)];
        for operation in operations {
            let applied = log.apply(&mut x.world, operation);
            applied.unwrap_or_else(|error| panic!("{error}"));
            states.push(sorted(&x.world));
        }
        // The create is logged with the ID it drew, which its entity has.
        let created = log.edits().nth(3);
        let Some(
            [
                Operation::Create {
                    entity: Some(id), ..
                },
            ],
        ) = created
        else {
            panic!("{created:?}");
        };
        assert!(x.world.entity(*id).is_some(), "{id}");
        assert_eq!(log.edits().len(), 5);
        for state in states.iter().rev().skip(1) {
            assert!(log.undo(&mut x.world).expect("undone"));
            assert!(same_snapshots(&sorted(&x.world), state));
        }
        assert!(!log.undo(&mut x.world).expect("nothing to undo"));
        assert_eq!(log.undoable(), None);
        for state in &states[1..] {
            assert!(log.redo(&mut x.world).expect("redone"));
            assert!(same_snapshots(&sorted(&x.world), state));
        }
        assert!(!log.redo(&mut x.world).expect("nothing to redo"));
        check_ids(&x.world);
        check_links(&x.world);
    }

    #[test]
    fn an_operation_after_an_undo_drops_what_was_undone() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        log.apply(&mut x.world, EMPTY).expect("created");
        let Some(
            &[
                Operation::Create {
                    entity: Some(id), ..
                },
            ],
        ) = log.undoable()
        else {
            panic!("{:?}", log.undoable());
        };
        let created = x.world.entity(id).expect("alive");
        // Undone, a create is a delete, which keeps the entity's slot, so a redo gives it back
        // under its handle.
        log.undo(&mut x.world).expect("undone");
        assert_eq!((log.kept(), vacated(&x.world)), (1, 1));
        assert!(log.redo(&mut x.world).expect("redone"));
        assert_eq!(x.world.entity(id), Some(created));
        log.undo(&mut x.world).expect("undone");
        // Refused, an operation changes nothing, so what was undone stays to redo.
        let refused = log.apply(&mut x.world, count(f, 200));
        refused.expect_err("above its max");
        assert!(log.redoable().is_some());
        assert_eq!((log.kept(), vacated(&x.world)), (1, 1));
        log.apply(&mut x.world, count(f, 2)).expect("set");
        assert_eq!(log.redoable(), None);
        assert_eq!((log.kept(), vacated(&x.world)), (0, 0));
        let next = x.world.spawn(());
        assert_eq!(next.index, created.index, "its slot, freed");
        assert!(!x.world.contains(created));
    }

    /// Undoing a create takes out only its entity: refused, entry dropped, if it has children
    /// the log didn't give it. Redoing a delete takes its subtree as it stands.
    #[test]
    fn undoing_a_create_takes_only_what_it_made() {
        let mut x = fixture();
        let mut log = Log::default();
        let children = |world: &World, entity| -> Vec<Entity> {
            world.children(entity).expect("alive").collect()
        };
        let create = |parent| Operation::Create {
            entity: None,
            parent: Some(parent),
            components: vec![("Transform".into(), Transform::IDENTITY.to_value())],
        };
        // Undone and redone, so the redo's undo is checked as the first was.
        let under_b = create(x.id(x.b));
        log.apply(&mut x.world, under_b).expect("created");
        let Some(
            &[
                Operation::Create {
                    entity: Some(made), ..
                },
            ],
        ) = log.undoable()
        else {
            panic!("{:?}", log.undoable());
        };
        assert!(log.undo(&mut x.world).expect("undone"));
        assert!(log.redo(&mut x.world).expect("redone"));
        let handle = x.world.entity(made).expect("alive");
        // A child the log gives it is undone before it; one the game gives it stays.
        log.apply(&mut x.world, create(made))
            .expect("created a child");
        x.world.set_parent(x.g, Some(handle)).expect("no cycle");
        assert!(log.undo(&mut x.world).expect("the child's create undone"));
        let before = sorted(&x.world);
        let error = log.undo(&mut x.world).expect_err("g is its child");
        assert_eq!(error.code(), "ecs.has_children", "{error}");
        let says = format!(
            "can't undo create {made}: entity {} is its child",
            x.id(x.g)
        );
        assert!(error.to_string().starts_with(&says), "{error}");
        assert!(same_snapshots(&sorted(&x.world), &before));
        assert_eq!(children(&x.world, handle), [x.g]);
        assert_eq!(log.undoable(), None, "dropped");
        assert!(log.redo(&mut x.world).expect("the child redone"));
        assert_eq!(children(&x.world, handle).len(), 2);
        // Redoing a delete takes its subtree as it stands.
        let delete = Operation::Delete { entity: x.id(x.a) };
        log.apply(&mut x.world, delete).expect("deleted");
        assert!(log.undo(&mut x.world).expect("undone"));
        x.world.set_parent(x.s, Some(x.a)).expect("no cycle");
        assert!(log.redo(&mut x.world).expect("redone"));
        assert!(!x.world.contains(x.s), "taken with a");
        assert!(log.undo(&mut x.world).expect("undone"));
        assert_eq!(children(&x.world, x.a), [x.b, x.c, x.d, x.s]);
        assert_eq!(x.world.get::<Parent>(x.g), Some(&Parent(handle)));
        check_ids(&x.world);
        check_links(&x.world);
    }

    /// A draw never gives an entity an ID a delete keeps (as with a living entity's): the stream
    /// reseeds, identically every run. The ID stays kept while any vacated slot holds it, even
    /// if a create took it meanwhile.
    #[test]
    fn a_draw_never_gives_an_id_a_delete_keeps() {
        // A scene saved by a world seeded alike, whose entity has the second draw's ID.
        let mut first = World::seeded(21);
        first.spawn(());
        let saved = first.spawn(Transform::IDENTITY);
        let text = first.save_scene(&[saved]).expect("saved");
        let id = first.id(saved).expect("alive");
        let mut world = World::seeded(21);
        let other = world.spawn(Transform::IDENTITY);
        let loaded = world.load_scene(&text).expect("loaded")[0];
        let mut log = Log::default();
        let delete = Operation::Delete { entity: id };
        log.apply(&mut world, delete).expect("deleted");
        // Given to another entity by a create that an undo takes out again: two slots keep it,
        // and an operation after the undo drops the create, freeing one.
        let create = Operation::Create {
            entity: Some(id),
            parent: None,
            components: Vec::new(),
        };
        log.apply(&mut world, create).expect("created");
        assert!(log.undo(&mut world).expect("undone"));
        assert_eq!((log.kept(), vacated(&world)), (2, 2));
        let set = Operation::Set {
            entity: world.id(other).expect("alive"),
            component: "Transform".into(),
            path: "translation.x".to_owned(),
            value: Value::F64(1.0),
        };
        log.apply(&mut world, set).expect("set");
        assert_eq!((log.kept(), vacated(&world)), (1, 1));
        // The stream's next draw is the ID kept: the draw reseeds instead.
        let spawned = world.spawn(());
        let mut stream = IdStream::new(21);
        stream.draw();
        assert_eq!(stream.draw(), id);
        stream.reseed();
        assert_eq!(world.id(spawned), Some(stream.draw()));
        // So the undo the log offers isn't refused.
        assert!(log.undo(&mut world).expect("the set undone"));
        assert!(log.undo(&mut world).expect("the delete undone"));
        assert_eq!(world.entity(id), Some(loaded));
        assert_eq!((log.kept(), vacated(&world)), (0, 0));
        check_ids(&world);
        check_links(&world);
    }

    #[test]
    fn a_gestures_sets_of_one_field_are_one_entry() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        let before = sorted(&x.world);
        for n in [1, 2, 3] {
            log.amend(&mut x.world, count(f, n)).expect("set");
        }
        assert_eq!(log.edits().len(), 1, "one field, one gesture");
        assert_eq!(log.undoable(), Some(&[count(f, 3)][..]));
        log.amend(&mut x.world, set(f, "byte", Value::UInt(9)))
            .expect("set");
        assert_eq!(log.edits().len(), 2, "another field");
        log.seal();
        log.amend(&mut x.world, set(f, "byte", Value::UInt(10)))
            .expect("set");
        assert_eq!(log.edits().len(), 3, "another gesture");
        let scale = |entity| Operation::Set {
            entity,
            component: "Transform".into(),
            path: "scale.x".to_owned(),
            value: Value::F32(2.0),
        };
        let [r, s] = [x.r, x.s].map(|entity| x.id(entity));
        log.amend(&mut x.world, scale(r)).expect("set");
        log.amend(&mut x.world, scale(s)).expect("set");
        log.amend(&mut x.world, count(f, 4)).expect("set");
        log.apply(&mut x.world, count(f, 5)).expect("set");
        log.amend(&mut x.world, count(f, 6)).expect("set");
        assert_eq!(
            log.edits().len(),
            7,
            "another entity's sets of a field join, another field's, and one applied, don't"
        );
        while log.undo(&mut x.world).expect("undone") {}
        assert!(same_snapshots(&sorted(&x.world), &before));
        // Redone, the gesture sets its last value.
        assert!(log.redo(&mut x.world).expect("redone"));
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.count, 3);
    }

    /// A set of `entity`'s `scale.x`.
    fn scale_x(entity: EntityId, x: f32) -> Operation {
        Operation::Set {
            entity,
            component: "Transform".into(),
            path: "scale.x".to_owned(),
            value: Value::F32(x),
        }
    }

    #[test]
    fn a_gestures_sets_of_one_field_of_several_entities_are_one_entry() {
        let mut x = fixture();
        let mut log = Log::default();
        let [r, s] = [x.r, x.s].map(|entity| x.id(entity));
        let before = sorted(&x.world);
        // Interleaved, as a drag of two handles makes them.
        for n in [2.0, 3.0] {
            log.amend(&mut x.world, scale_x(r, n)).expect("set");
            log.amend(&mut x.world, scale_x(s, n + 10.0)).expect("set");
        }
        assert_eq!(log.edits().len(), 1, "one field of two entities");
        let last = [scale_x(r, 3.0), scale_x(s, 13.0)];
        assert_eq!(log.undoable(), Some(&last[..]));
        // Another field of one of them is another edit.
        let y = Operation::Set {
            entity: r,
            component: "Transform".into(),
            path: "scale.y".to_owned(),
            value: Value::F32(5.0),
        };
        log.amend(&mut x.world, y).expect("set");
        assert_eq!(log.edits().len(), 2);
        assert!(log.undo(&mut x.world).expect("undone"));
        assert!(log.undo(&mut x.world).expect("undone"));
        assert!(same_snapshots(&sorted(&x.world), &before), "both back");
        assert!(log.redo(&mut x.world).expect("redone"));
        let scale = |entity| {
            x.world
                .get::<Transform>(entity)
                .expect("a transform")
                .scale
                .x
        };
        assert_eq!([scale(x.r), scale(x.s)], [3.0, 13.0]);
    }

    #[test]
    fn amend_all_with_one_set_refused_changes_nothing() {
        let mut x = fixture();
        let mut log = Log::default();
        let [r, s] = [x.r, x.s].map(|entity| x.id(entity));
        let gone: EntityId = "0000000000000001".parse().expect("an ID");
        // A new entry: nothing applied, nothing logged, no gesture open.
        let before = sorted(&x.world);
        let refused = log.amend_all(&mut x.world, vec![scale_x(r, 2.0), scale_x(gone, 2.0)]);
        refused.expect_err("an entity that's gone");
        assert!(same_snapshots(&sorted(&x.world), &before));
        assert_eq!(log.edits().len(), 0);
        // Joining: the entry stays as it was; sets before the refused one are put back.
        log.amend_all(&mut x.world, vec![scale_x(r, 2.0), scale_x(s, 2.0)])
            .expect("set");
        let joined = sorted(&x.world);
        let refused = log.amend_all(&mut x.world, vec![scale_x(s, 3.0), scale_x(gone, 3.0)]);
        refused.expect_err("an entity that's gone");
        assert!(same_snapshots(&sorted(&x.world), &joined));
        assert_eq!(log.edits().len(), 1);
        assert_eq!(
            log.undoable(),
            Some(&[scale_x(r, 2.0), scale_x(s, 2.0)][..])
        );
        // It still joins, and undoes whole.
        log.amend_all(&mut x.world, vec![scale_x(s, 4.0), scale_x(r, 4.0)])
            .expect("set");
        assert_eq!(log.edits().len(), 1);
        assert!(log.undo(&mut x.world).expect("undone"));
        assert!(same_snapshots(&sorted(&x.world), &before));
    }

    /// Undoing a gesture lights again every lamp its sets put out, though its first put none out.
    #[test]
    fn undoing_a_gesture_sets_back_the_exclusive_fields_its_later_sets_made_false() {
        let (mut world, lamps, _) = lamps();
        let c = world.id(lamps[2]).expect("alive");
        let light = |lit| Operation::Set {
            entity: c,
            component: "Lamp".into(),
            path: "lit".to_owned(),
            value: Value::Bool(lit),
        };
        let mut log = Log::default();
        for on in [false, true, false, true] {
            log.amend(&mut world, light(on)).expect("set");
        }
        assert_eq!(log.edits().len(), 1, "one gesture");
        assert_eq!(lit(&world, lamps), [false, false, true]);
        assert!(log.undo(&mut world).expect("undone"));
        assert_eq!(lit(&world, lamps), [true, true, false]);
        assert!(log.redo(&mut world).expect("redone"));
        assert_eq!(lit(&world, lamps), [false, false, true]);
    }

    #[test]
    fn an_undo_refused_drops_its_entry() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        log.apply(&mut x.world, count(f, 1)).expect("set");
        let scale = Operation::Set {
            entity: x.id(x.s),
            component: "Transform".into(),
            path: "scale.x".to_owned(),
            value: Value::F32(2.0),
        };
        log.apply(&mut x.world, scale).expect("set");
        x.world.despawn(x.s).expect("alive");
        let error = log.undo(&mut x.world).expect_err("its entity is gone");
        assert_eq!(error.code(), "ecs.no_entity", "{error}");
        let says = format!("can't undo set `Transform.scale.x` of {}: ", x.id(x.s));
        assert!(error.to_string().starts_with(&says), "{error}");
        assert_eq!(log.undoable(), Some(&[count(f, 1)][..]));
        assert_eq!(log.redoable(), None);
        assert!(log.undo(&mut x.world).expect("undone"));
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.count, 42);
        // A delete's entry dropped so frees the slots it kept.
        let delete = Operation::Delete { entity: x.id(x.c) };
        log.apply(&mut x.world, delete).expect("deleted");
        x.world.despawn(x.a).expect("alive");
        let undone = log.undo(&mut x.world).map_err(|error| error.code());
        assert_eq!(undone, Err("ecs.no_entity"));
        assert_eq!((log.kept(), vacated(&x.world)), (0, 0));
        check_ids(&x.world);
    }

    /// A multi-operation edit applies all or none, and undoes and redoes whole; an undo refused
    /// partway turns back what it undid.
    #[test]
    fn an_edits_operations_go_together() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        let scale = Operation::Set {
            entity: x.id(x.s),
            component: "Transform".into(),
            path: "scale.x".to_owned(),
            value: Value::F32(2.0),
        };
        let before = sorted(&x.world);
        let refused = log.apply_all(
            &mut x.world,
            vec![count(f, 5), scale.clone(), count(f, 200)],
        );
        let error = refused.expect_err("above its max");
        assert!(
            error.to_string().starts_with("operation 3, set "),
            "{error}"
        );
        assert!(same_snapshots(&sorted(&x.world), &before), "none applied");
        assert_eq!(log.edits().len(), 0);
        let delete = Operation::Delete { entity: x.id(x.c) };
        let edit = vec![count(f, 5), scale, delete];
        log.apply_all(&mut x.world, edit.clone()).expect("applied");
        assert_eq!(log.undoable(), Some(edit.as_slice()));
        let after = sorted(&x.world);
        assert!(log.undo(&mut x.world).expect("undone"));
        assert!(same_snapshots(&sorted(&x.world), &before));
        assert!(log.redo(&mut x.world).expect("redone"));
        assert!(same_snapshots(&sorted(&x.world), &after));
        // The scale's entity gone: its undo is refused after the delete's, which comes back out.
        x.world.despawn(x.s).expect("alive");
        let error = log.undo(&mut x.world).expect_err("s is gone");
        assert_eq!(error.code(), "ecs.no_entity", "{error}");
        assert!(!x.world.contains(x.c), "deleted again");
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.count, 5);
        assert_eq!(log.undoable(), None, "dropped");
        assert_eq!((log.kept(), vacated(&x.world)), (0, 0));
        check_ids(&x.world);
        check_links(&x.world);
    }

    /// A rotation that isn't a unit, which a set refuses but code may store (2D physics leaves a
    /// body's so when long spun).
    const DRIFTED: Quat = Quat::from_xyzw(0.0, 0.0, 0.6, 0.9);

    /// A set of `entity`'s rotation.
    fn rotate(entity: EntityId, rotation: Quat) -> Operation {
        Operation::Set {
            entity,
            component: "Transform".into(),
            path: "rotation".to_owned(),
            value: Value::Quat(rotation),
        }
    }

    /// Gives `entity` the rotation [`DRIFTED`], as code may.
    fn drift(world: &mut World, entity: Entity) {
        let transform = world.get_mut::<Transform>(entity).expect("a transform");
        transform.rotation = DRIFTED;
    }

    /// Whether `entity`'s rotation is `rotation`, bit for bit.
    fn rotated(world: &World, entity: Entity, rotation: Quat) -> bool {
        let transform = world.get::<Transform>(entity).expect("a transform");
        let bits = |rotation: Quat| rotation.to_array().map(f32::to_bits);
        bits(transform.rotation) == bits(rotation)
    }

    /// An edit refused partway rolls back earlier operations exactly, even where code stored what
    /// a set refuses (a non-unit rotation, a count above its max): bit for bit, so the hash is
    /// unchanged.
    #[test]
    fn an_edit_refused_partway_rolls_back_what_code_stored_that_a_set_refuses() {
        let mut x = fixture();
        let mut log = Log::default();
        let [r, f] = [x.r, x.f].map(|entity| x.id(entity));
        drift(&mut x.world, x.r);
        let all = x.world.get_mut::<Everything>(x.f).expect("everything");
        all.count = 200;
        let before = x.world.hash();
        let edit = vec![rotate(r, Quat::IDENTITY), count(f, 5), count(f, 101)];
        let refused = log.apply_all(&mut x.world, edit);
        let error = refused.expect_err("above its max");
        assert_eq!(error.code(), "reflect.out_of_range", "{error}");
        assert!(error.to_string().starts_with("operation 3, "), "{error}");
        assert_eq!(x.world.hash(), before);
        assert!(rotated(&x.world, x.r, DRIFTED));
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.count, 200);
        assert_eq!(log.edits().len(), 0);
    }

    /// A refused append rolls back the edit before it exactly, even where code stored what a set
    /// refuses. If the world has moved on and refuses that roll back, the edit stays applied and
    /// logged.
    #[test]
    fn an_append_refused_rolls_back_the_edit_before_it_exactly() {
        let mut x = fixture();
        let mut log = Log::default();
        let [r, c, f] = [x.r, x.c, x.f].map(|entity| x.id(entity));
        drift(&mut x.world, x.r);
        let before = x.world.hash();
        let edit = vec![rotate(r, Quat::IDENTITY)];
        log.apply_all(&mut x.world, edit).expect("applied");
        let refused = log.append(&mut x.world, vec![count(f, 101)]);
        let error = refused.expect_err("above its max");
        assert_eq!(error.code(), "reflect.out_of_range", "{error}");
        assert_eq!(x.world.hash(), before);
        assert!(rotated(&x.world, x.r, DRIFTED));
        assert_eq!(log.edits().len(), 0, "dropped");
        // The edit's delete can't be rolled back once its taken entity's parent is gone: the set
        // rolled back before it is turned back.
        let edit = vec![Operation::Delete { entity: c }, rotate(r, Quat::IDENTITY)];
        log.apply_all(&mut x.world, edit.clone()).expect("applied");
        x.world.despawn(x.a).expect("alive");
        let before = x.world.hash();
        let refused = log.append(&mut x.world, vec![count(f, 101)]);
        let error = refused.expect_err("above its max");
        assert_eq!(error.code(), "reflect.out_of_range", "{error}");
        assert_eq!(x.world.hash(), before);
        assert!(rotated(&x.world, x.r, Quat::IDENTITY));
        assert_eq!(log.undoable(), Some(edit.as_slice()), "kept");
        assert_eq!((log.kept(), vacated(&x.world)), (1, 1));
        check_ids(&x.world);
        check_links(&x.world);
    }

    /// An undo refused partway turns back what it undid exactly, even if code has since stored
    /// what a set refuses: the rotation returns bit for bit.
    #[test]
    fn an_undo_refused_partway_turns_back_what_code_stored_exactly() {
        let mut x = fixture();
        let mut log = Log::default();
        let [r, c] = [x.r, x.c].map(|entity| x.id(entity));
        let edit = vec![
            Operation::Delete { entity: c },
            rotate(r, Quat::from_rotation_z(0.5)),
        ];
        log.apply_all(&mut x.world, edit).expect("applied");
        drift(&mut x.world, x.r);
        // The delete's undo is refused, after the set's.
        x.world.despawn(x.a).expect("alive");
        let before = x.world.hash();
        let error = log.undo(&mut x.world).expect_err("c's parent is gone");
        assert_eq!(error.code(), "ecs.no_entity", "{error}");
        assert_eq!(x.world.hash(), before);
        assert!(rotated(&x.world, x.r, DRIFTED));
        assert_eq!(log.undoable(), None, "dropped");
        assert_eq!((log.kept(), vacated(&x.world)), (0, 0));
        check_ids(&x.world);
    }

    #[test]
    fn clearing_the_log_frees_what_it_kept() {
        let mut x = fixture();
        let mut log = Log::default();
        let delete = Operation::Delete { entity: x.id(x.a) };
        log.apply(&mut x.world, delete).expect("deleted");
        log.apply(&mut x.world, EMPTY).expect("created");
        log.undo(&mut x.world).expect("undone");
        assert_eq!((log.kept(), vacated(&x.world)), (6, 6));
        log.clear(&mut x.world);
        assert_eq!((log.kept(), vacated(&x.world)), (0, 0));
        assert_eq!((log.undoable(), log.redoable()), (None, None));
    }

    /// The scene of every entity of `world`; `None` if one refers to an entity that isn't alive
    /// (as one whose target a delete took, until the delete is undone).
    fn saved(world: &World) -> Option<String> {
        let roots = world.entities(None).expect("every entity");
        let roots = roots.filter(|&entity| world.get::<Parent>(entity).is_none());
        match world.save_scene(&roots.collect::<Vec<Entity>>()) {
            Ok(text) => Some(text),
            Err(error) => {
                assert_eq!(error.code(), "ecs.no_entity", "{error}");
                let mut targets = world.query::<&Everything>().filter_map(|all| all.target);
                assert!(targets.any(|target| !world.contains(target)), "{error}");
                None
            }
        }
    }

    /// What a random operation may be refused with.
    const REFUSED: [&str; 8] = [
        "ecs.id_taken",
        "ecs.parent_cycle",
        "reflect.duplicate_component",
        "reflect.has_component",
        "reflect.missing_component",
        "reflect.opaque",
        "reflect.out_of_range",
        "reflect.read_only",
    ];

    /// The component types a value gives an entity, and those it never does (refused).
    const MADE: [&str; 3] = ["Transform", "Sprite", "Everything"];
    const UNMADE: [&str; 2] = ["Sealed", "Parent"];

    /// A component of type `name` and its value, refused now and then: `Everything`'s above its
    /// count's max, and those of `UNMADE`.
    fn component(
        name: &'static str,
        rng: &mut Rng,
        living: &[Entity],
        step: usize,
    ) -> (Ident, Value) {
        let value = match name {
            "Transform" => Transform {
                translation: DVec3::new(f64::from(step as f32), -0.0, 1.0),
                ..Transform::IDENTITY
            }
            .to_value(),
            "Sprite" => Sprite {
                color: Vec4::new(0.5, 0.25, 1.0, 1.0),
                ..Sprite::default()
            }
            .to_value(),
            "Everything" => {
                let target = rng.below(2) == 0 && !living.is_empty();
                let target = target.then(|| living[rng.below(living.len())]);
                let all = Everything {
                    target,
                    count: rng.below(120) as u8,
                    ..everything()
                };
                all.to_value()
            }
            "Sealed" => Value::Opaque,
            _ => Value::Struct(Vec::new()),
        };
        (name.into(), value)
    }

    /// One of `likely`, mostly, if there are any; otherwise any type, as likely to be refused.
    fn name(rng: &mut Rng, likely: &[&'static str]) -> &'static str {
        if !likely.is_empty() && rng.below(5) != 0 {
            return likely[rng.below(likely.len())];
        }
        let any = [MADE.as_slice(), &UNMADE].concat();
        any[rng.below(any.len())]
    }

    /// The first entity whose `Everything` targets one `gone` says is, and that one.
    fn referring(world: &World, gone: impl Fn(Entity) -> bool) -> Option<Entity> {
        let mut all = world.query::<(Entity, &Everything)>();
        let found = all.find(|(_, all)| all.target.is_some_and(&gone));
        found.map(|(entity, _)| entity)
    }

    /// A random operation on `world`'s living entities, mostly one that applies. A reference a
    /// delete leaves dangling keeps the world from saving: mostly avoided, and set to `None` soon
    /// after, so the scene shows most steps.
    pub(in crate::ecs) fn random(world: &World, rng: &mut Rng, step: usize) -> Operation {
        let living: Vec<Entity> = world.entities(None).expect("every entity").collect();
        let id = |entity| world.id(entity).expect("alive");
        let pick = |rng: &mut Rng| living[rng.below(living.len())];
        let unset = |referrer| Operation::Set {
            entity: id(referrer),
            component: "Everything".into(),
            path: "target".to_owned(),
            value: Value::Option(None),
        };
        if let Some(referrer) = referring(world, |target| !world.contains(target))
            && rng.below(2) == 0
        {
            return unset(referrer);
        }
        if living.is_empty() || rng.below(10) < 3 {
            let entity = match rng.below(4) {
                0 => {
                    let fresh = format!("{:016x}", splitmix64(&mut rng.0) | 1);
                    Some(fresh.parse().expect("an ID"))
                }
                // Taken.
                1 if !living.is_empty() && rng.below(3) == 0 => Some(id(pick(rng))),
                _ => None,
            };
            let parent = rng.below(2) == 0 && !living.is_empty();
            let parent = parent.then(|| id(pick(rng)));
            let mut names: Vec<&'static str> =
                MADE.into_iter().filter(|_| rng.below(2) == 0).collect();
            if rng.below(8) == 0 {
                // Refused: twice, or one no value makes.
                names.push(name(rng, &[]));
            }
            let components = names
                .into_iter()
                .map(|name| component(name, rng, &living, step));
            return Operation::Create {
                entity,
                parent,
                components: components.collect(),
            };
        }
        let subject = pick(rng);
        let has: Vec<&str> = world.components_of(subject).expect("alive").collect();
        let entity = id(subject);
        match rng.below(7) {
            0 => {
                let taken = world.subtree(subject);
                let into = |target: Entity| taken.iter().any(|&(entity, _)| entity == target);
                let referrer = referring(world, into).filter(|outside| !into(*outside));
                match referrer {
                    Some(referrer) if rng.below(5) != 0 => unset(referrer),
                    _ => Operation::Delete { entity },
                }
            }
            1 => {
                let lacks: Vec<&'static str> = MADE
                    .into_iter()
                    .filter(|name| !has.contains(name))
                    .collect();
                let (component, value) = component(name(rng, &lacks), rng, &living, step);
                Operation::Add {
                    entity,
                    component,
                    value,
                }
            }
            2 => {
                let own: Vec<&'static str> =
                    MADE.into_iter().filter(|name| has.contains(name)).collect();
                let component = name(rng, &own).into();
                Operation::Remove { entity, component }
            }
            3..=5 => {
                let float = match rng.below(3) {
                    0 => -0.0,
                    1 => f32::NAN,
                    _ => step as f32 * 0.5,
                };
                let target = rng.below(3) != 0;
                let target = target.then(|| Box::new(Value::Entity(pick(rng))));
                let sets = [
                    ("Transform", "translation.x", Value::F64(f64::from(float))),
                    ("Transform", "scale", Value::Vec3(Vec3::splat(float))),
                    ("Everything", "target", Value::Option(target)),
                    // Above its max now and then.
                    ("Everything", "count", Value::UInt(rng.below(120) as u64)),
                    ("Sprite", "color", Value::Vec4(Vec4::splat(0.25))),
                    // Whole, opaque parts given as `Opaque`.
                    (
                        "Transform",
                        "",
                        component("Transform", rng, &living, step).1,
                    ),
                    ("Sprite", "", component("Sprite", rng, &living, step).1),
                    (
                        "Everything",
                        "",
                        component("Everything", rng, &living, step).1,
                    ),
                ];
                let mut sets = Vec::from(sets);
                // Mostly of a component it has.
                if rng.below(5) != 0 && sets.iter().any(|set| has.contains(&set.0)) {
                    sets.retain(|set| has.contains(&set.0));
                }
                let (component, path, value) = sets.swap_remove(rng.below(sets.len()));
                Operation::Set {
                    entity,
                    component: component.into(),
                    path: path.to_owned(),
                    value,
                }
            }
            _ => {
                let parent = rng.below(4) != 0;
                let parent = parent.then(|| id(pick(rng)));
                Operation::Reparent { entity, parent }
            }
        }
    }

    /// Random operations through the log, with some undos and redos, then all undone. Going
    /// back, the scene (byte for byte) and content hash match their state before each operation,
    /// and at the end the start, every entity (opaque parts included) by its handle.
    #[test]
    fn undoing_a_log_of_random_operations_gives_back_the_scene_and_its_content_hash() {
        let mut x = fixture();
        let start = saved(&x.world).expect("the start saves");
        let start_sorted = sorted(&x.world);
        let start_content = x.world.hash().content;
        let world = &mut x.world;
        let mut log = Log::default();
        let mut rng = Rng(11);
        // The scene, where it saved, and the content hash, before each entry done.
        let mut before: Vec<(Option<String>, u64)> = Vec::new();
        let mut applied = 0;
        for step in 0..1000 {
            if rng.below(8) == 0 && !before.is_empty() {
                let now = saved(world);
                let mut undone = Vec::new();
                for _ in 0..1 + rng.below(before.len().min(6)) {
                    let stepped = log.undo(world);
                    assert!(stepped.unwrap_or_else(|error| panic!("step {step}: {error}")));
                    let (text, content) = before.pop().expect("an entry");
                    assert_eq!(saved(world), text, "step {step}");
                    assert_eq!(world.hash().content, content, "step {step}: the content");
                    undone.push((text, content));
                }
                while let Some(entry) = undone.pop() {
                    let stepped = log.redo(world);
                    assert!(stepped.unwrap_or_else(|error| panic!("step {step}: {error}")));
                    before.push(entry);
                }
                assert_eq!(saved(world), now, "step {step}");
            } else {
                let operation = random(world, &mut rng, step);
                let said = operation.to_string();
                let (text, version) = (saved(world), world.version());
                let content = world.hash().content;
                match log.apply(world, operation) {
                    Ok(()) => {
                        before.push((text, content));
                        applied += 1;
                    }
                    Err(error) => {
                        let code = error.code();
                        assert!(REFUSED.contains(&code), "step {step}, {said}: {error}");
                        assert_eq!(saved(world), text, "step {step}, {said}");
                        assert_eq!(world.version(), version, "step {step}, {said}");
                    }
                }
            }
            check_ids(world);
            check_links(world);
            assert_eq!(vacated(world), log.kept(), "step {step}");
        }
        // Most steps change the world rather than being refused.
        assert!(applied > 500, "{applied} applied");
        let last = saved(world);
        while let Some((text, content)) = before.pop() {
            assert!(log.undo(world).expect("undone"));
            assert_eq!(saved(world), text);
            assert_eq!(world.hash().content, content);
        }
        assert!(!log.undo(world).expect("nothing to undo"));
        assert_eq!(saved(world).as_deref(), Some(start.as_str()));
        assert_eq!(world.hash().content, start_content);
        assert!(same_snapshots(&sorted(world), &start_sorted));
        let sprite = world.get::<Sprite>(x.a).expect("a sprite");
        assert_eq!(sprite.texture, Some(x.texture));
        let all = world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.handle, Handle(99));
        assert_eq!(world.get::<Sealed>(x.g), Some(&Sealed(5)));
        while log.redo(world).expect("redone") {}
        assert_eq!(saved(world), last);
        log.clear(world);
        assert_eq!(vacated(world), 0);
        check_ids(world);
        check_links(world);
    }

    /// A log copy (of random operations, some undone) rebound to a world copy taken at the same
    /// state steps there as the log does in the world. After each step the worlds hash alike and
    /// each keeps its log's slots, which clearing frees. An unbound copy is refused there as
    /// another world's and changes nothing.
    #[test]
    fn a_log_copied_for_a_copy_of_its_world_steps_there_as_the_log_does_in_the_world() {
        let mut x = fixture();
        let world = &mut x.world;
        let mut log = Log::default();
        let mut rng = Rng(13);
        for step in 0..400 {
            let operation = random(world, &mut rng, step);
            let _ = log.apply(world, operation);
            if rng.below(4) == 0 {
                let _ = log.undo(world);
            }
        }
        let mut copy = world.clone();
        let mut unbound = log.copied();
        let mut copied = log.copied();
        copied.rebind(world.serial(), copy.serial());
        let mut stepped = 0;
        for step in 0..300 {
            let (ours, theirs) = match rng.below(3) {
                0 => (log.undo(world), copied.undo(&mut copy)),
                1 => (log.redo(world), copied.redo(&mut copy)),
                _ => {
                    let operation = random(world, &mut rng, 1000 + step);
                    let theirs = copied.apply(&mut copy, operation.clone());
                    (
                        log.apply(world, operation).map(|()| true),
                        theirs.map(|()| true),
                    )
                }
            };
            let code = |done: Result<bool>| done.map_err(|error| error.code().to_owned());
            let (ours, theirs) = (code(ours), code(theirs));
            assert_eq!(ours, theirs, "step {step}");
            stepped += usize::from(ours == Ok(true));
            assert_eq!(world.hash(), copy.hash(), "step {step}");
            assert_eq!(vacated(world), log.kept(), "step {step}");
            assert_eq!(vacated(&copy), copied.kept(), "step {step}");
        }
        assert!(stepped > 100, "{stepped} steps taken");
        assert!(log.kept() > 0, "slots kept, which the copy keeps too");
        let hash = copy.hash();
        let refused = unbound.undo(&mut copy).map_err(|error| error.code());
        assert_eq!(refused, Err("ops.other_world"));
        assert_eq!(copy.hash(), hash);
        log.clear(world);
        copied.clear(&mut copy);
        assert_eq!((vacated(world), vacated(&copy)), (0, 0));
        check_ids(&copy);
        check_links(&copy);
    }
}
