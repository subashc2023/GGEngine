//! The log of the operations applied to a world, in order, each with its inverse: what undo and
//! redo step through, and what a review reads and a replay applies.

use gg_core::Result;
use tracing::field::display;
use tracing::{debug, warn};

use super::World;
use super::ops::{Inverse, Operation};

/// The operations applied to a world, in order, each with what undoes it: undo and redo, and a
/// gesture's sets of one field, as a drag makes, merged into one edit.
///
/// An entry whose undo or redo puts entities back, a delete done or a create undone, keeps them,
/// their components typed and their slots, so that it gives each back under its handle, as it was,
/// until the entry is dropped: by an operation applied after an undo, which drops what was undone;
/// by an undo or a redo the world refuses; or by [`clear`](Self::clear). A log dropped without
/// `clear` leaves those slots unused for good, so whoever keeps one clears it.
#[derive(Default)]
pub struct Log {
    /// Applied and not undone, the first applied first.
    done: Vec<Entry>,
    /// Undone, the last undone last.
    undone: Vec<Entry>,
    /// Whether the last entry done is a gesture's that goes on, which
    /// [`amend`](Self::amend) merges a set of its field into.
    open: bool,
}

struct Entry {
    operation: Operation,
    inverse: Inverse,
}

impl Log {
    /// Applies `operation` to `world` and logs it, dropping what was undone; a create given no ID
    /// is logged with the one it drew. Refused, it changes nothing, and logs nothing, as
    /// [`Operation`] says.
    pub fn apply(&mut self, world: &mut World, mut operation: Operation) -> Result<()> {
        let inverse = world.apply(&mut operation)?;
        debug!(
            code = "ops.applied",
            entity = operation.entity().map(display),
            operation = %operation,
            "Applied an operation"
        );
        for dropped in self.undone.drain(..) {
            world.release(dropped.inverse);
        }
        self.done.push(Entry { operation, inverse });
        self.open = false;
        Ok(())
    }

    /// As [`apply`](Self::apply), but a set of the field the last entry set, while its gesture
    /// goes on, joins that entry: it takes the new value and keeps its inverse, so that undoing it
    /// undoes the gesture. A set starts a gesture, which [`seal`](Self::seal) ends.
    pub fn amend(&mut self, world: &mut World, mut operation: Operation) -> Result<()> {
        let joins = |entry: &Entry| match (&entry.operation, &operation) {
            (
                Operation::Set {
                    entity,
                    component,
                    path,
                    ..
                },
                Operation::Set {
                    entity: next,
                    component: of,
                    path: at,
                    ..
                },
            ) => (entity, component, path) == (next, of, at),
            _ => false,
        };
        if !(self.open && self.done.last().is_some_and(joins)) {
            let set = matches!(operation, Operation::Set { .. });
            self.apply(world, operation)?;
            self.open = set;
            return Ok(());
        }
        let inverse = world.apply(&mut operation)?;
        // What undoes this set undoes only the set before it, in the gesture.
        world.release(inverse);
        self.done.last_mut().expect("an entry joined").operation = operation;
        Ok(())
    }

    /// Ends the gesture going on, so that the next set is an entry of its own.
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Undoes the last operation applied and not undone, and returns whether there was one.
    ///
    /// Refused, as when the entity it changed is gone since, it changes nothing, and its entry is
    /// dropped, with what it kept, since the world has moved on from it and it would block the
    /// entries before it: the error says why, and is logged as `ops.dropped`.
    pub fn undo(&mut self, world: &mut World) -> Result<bool> {
        self.step(world, Step::Undo)
    }

    /// Redoes the last operation undone, and returns whether there was one; refused, as
    /// [`undo`](Self::undo) is.
    pub fn redo(&mut self, world: &mut World) -> Result<bool> {
        self.step(world, Step::Redo)
    }

    /// The operation [`undo`](Self::undo) undoes next.
    pub fn undoable(&self) -> Option<&Operation> {
        self.done.last().map(|entry| &entry.operation)
    }

    /// The operation [`redo`](Self::redo) redoes next.
    pub fn redoable(&self) -> Option<&Operation> {
        self.undone.last().map(|entry| &entry.operation)
    }

    /// The operations applied and not undone, the first applied first: what a review reads, and
    /// a replay applies.
    pub fn operations(&self) -> impl ExactSizeIterator<Item = &Operation> {
        self.done.iter().map(|entry| &entry.operation)
    }

    /// Drops every entry, and what they kept, from `world`, which they were applied to.
    pub fn clear(&mut self, world: &mut World) {
        for dropped in self.done.drain(..).chain(self.undone.drain(..)) {
            world.release(dropped.inverse);
        }
        self.open = false;
    }

    fn step(&mut self, world: &mut World, step: Step) -> Result<bool> {
        self.open = false;
        let (from, to) = match step {
            Step::Undo => (&mut self.done, &mut self.undone),
            Step::Redo => (&mut self.undone, &mut self.done),
        };
        let Some(mut entry) = from.pop() else {
            return Ok(false);
        };
        let (doing, code, done) = match step {
            Step::Undo => ("undo", "ops.undone", "Undid an operation"),
            Step::Redo => ("redo", "ops.redone", "Redid an operation"),
        };
        match world.undo(&mut entry.inverse) {
            Ok(()) => {
                let entity = entry.operation.entity().map(display);
                debug!(code, entity, operation = %entry.operation, "{done}");
                to.push(entry);
                Ok(true)
            }
            Err(error) => {
                let message = format!("can't {doing} {}: {error}", entry.operation);
                warn!(
                    code = "ops.dropped",
                    refused = error.code(),
                    entity = entry.operation.entity().map(display),
                    operation = %entry.operation,
                    "{message}; dropped it from the log"
                );
                world.release(entry.inverse);
                Err(error.with_message(message))
            }
        }
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
        entries.map(|entry| entry.inverse.kept()).sum()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use gg_core::math::{Vec3, Vec4};

    use super::*;
    use crate::ecs::hierarchy::tests::check_links;
    use crate::ecs::id::splitmix64;
    use crate::ecs::id::tests::check_ids;
    use crate::ecs::ops::tests::{Sealed, fixture, sorted};
    use crate::ecs::tests::Rng;
    use crate::reflect::tests::{Everything, Handle, everything, same_snapshots};
    use crate::reflect::{Name, Value};
    use crate::{Entity, EntityId, Parent, Reflect, Sprite, Transform};

    /// How many of `world`'s slots are vacant, kept for entities a delete took.
    fn vacated(world: &World) -> usize {
        let mut vacant = 0;
        for (index, meta) in world.meta.iter().enumerate() {
            let free = world.free.contains(&(index as u32));
            let retired = meta.generation == NonZeroU32::MAX;
            if meta.location.is_none() && !free && !retired {
                vacant += 1;
            }
        }
        vacant
    }

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
            // -0.0 over 0.0, which `==` calls equal and the scene doesn't: logged all the same.
            Operation::Set {
                entity: x.id(x.r),
                component: "Transform".into(),
                path: "translation.x".to_owned(),
                value: Value::F32(-0.0),
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
        let created = log.operations().nth(3);
        let Some(Operation::Create {
            entity: Some(id), ..
        }) = created
        else {
            panic!("{created:?}");
        };
        assert!(x.world.entity(*id).is_some(), "{id}");
        assert_eq!(log.operations().len(), 5);
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
        let Some(&Operation::Create {
            entity: Some(id), ..
        }) = log.undoable()
        else {
            panic!("{:?}", log.undoable());
        };
        let created = x.world.entity(id).expect("alive");
        // Undone, a create is a delete, which keeps the entity's slot, so that a redo gives it
        // back under its handle.
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

    #[test]
    fn a_gestures_sets_of_one_field_are_one_entry() {
        let mut x = fixture();
        let mut log = Log::default();
        let f = x.id(x.f);
        let before = sorted(&x.world);
        for n in [1, 2, 3] {
            log.amend(&mut x.world, count(f, n)).expect("set");
        }
        assert_eq!(log.operations().len(), 1, "one field, one gesture");
        assert_eq!(log.undoable(), Some(&count(f, 3)));
        log.amend(&mut x.world, set(f, "byte", Value::UInt(9)))
            .expect("set");
        assert_eq!(log.operations().len(), 2, "another field");
        log.seal();
        log.amend(&mut x.world, set(f, "byte", Value::UInt(10)))
            .expect("set");
        assert_eq!(log.operations().len(), 3, "another gesture");
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
            log.operations().len(),
            8,
            "another component's, another entity's, and one applied"
        );
        while log.undo(&mut x.world).expect("undone") {}
        assert!(same_snapshots(&sorted(&x.world), &before));
        // Redone, the gesture sets its last value.
        assert!(log.redo(&mut x.world).expect("redone"));
        let all = x.world.get::<Everything>(x.f).expect("everything");
        assert_eq!(all.count, 3);
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
        assert_eq!(log.undoable(), Some(&count(f, 1)));
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

    /// The scene of every entity of `world`; `None` if one refers to an entity that isn't alive,
    /// as one whose target a delete took does until the delete is undone.
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
    const REFUSED: [&str; 7] = [
        "ecs.id_taken",
        "ecs.parent_cycle",
        "reflect.has_component",
        "reflect.missing_component",
        "reflect.opaque",
        "reflect.out_of_range",
        "reflect.read_only",
    ];

    /// The component types a value gives an entity; and those it never does, which are refused.
    const MADE: [&str; 3] = ["Transform", "Sprite", "Everything"];
    const UNMADE: [&str; 2] = ["Sealed", "Parent"];

    /// A component of type `name` and its value, which is refused now and then: `Everything`'s
    /// above its count's max, and those of `UNMADE`.
    fn component(
        name: &'static str,
        rng: &mut Rng,
        living: &[Entity],
        step: usize,
    ) -> (Name, Value) {
        let value = match name {
            "Transform" => Transform {
                translation: Vec3::new(step as f32, -0.0, 1.0),
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

    /// `entity` and its descendants.
    fn subtree(world: &World, entity: Entity) -> Vec<Entity> {
        let mut all = vec![entity];
        let mut next = 0;
        while let Some(&at) = all.get(next) {
            all.extend(world.children(at).expect("alive"));
            next += 1;
        }
        all
    }

    /// A random operation on `world`'s living entities, mostly one that applies. A reference left
    /// dangling by a delete, which keeps the world from saving, is mostly avoided, and set to
    /// `None` soon after, so that the scene shows most steps.
    fn random(world: &World, rng: &mut Rng, step: usize) -> Operation {
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
                let taken = subtree(world, subject);
                let into = |target| taken.contains(&target);
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
                    ("Transform", "translation.x", Value::F32(float)),
                    ("Transform", "scale", Value::Vec3(Vec3::splat(float))),
                    ("Everything", "target", Value::Option(target)),
                    // Above its max now and then.
                    ("Everything", "count", Value::UInt(rng.below(120) as u64)),
                    ("Sprite", "color", Value::Vec4(Vec4::splat(0.25))),
                    // Whole, what's opaque in them given as `Opaque`.
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

    /// Applies random operations through the log, now and then undoing and redoing some of them,
    /// and then undoes them all: as it goes back, the world's scene is as it was before each, byte
    /// for byte, and in the end as it was at the start, and so is every entity, by its handle, with
    /// what's opaque.
    #[test]
    fn undoing_a_log_of_random_operations_gives_back_the_scene() {
        let mut x = fixture();
        let start = saved(&x.world).expect("the start saves");
        let start_sorted = sorted(&x.world);
        let world = &mut x.world;
        let mut log = Log::default();
        let mut rng = Rng(11);
        // The scene before each entry done, where it saved.
        let mut before: Vec<Option<String>> = Vec::new();
        let mut applied = 0;
        for step in 0..1000 {
            if rng.below(8) == 0 && !before.is_empty() {
                let now = saved(world);
                let mut undone = Vec::new();
                for _ in 0..1 + rng.below(before.len().min(6)) {
                    let stepped = log.undo(world);
                    assert!(stepped.unwrap_or_else(|error| panic!("step {step}: {error}")));
                    let text = before.pop().expect("an entry");
                    assert_eq!(saved(world), text, "step {step}");
                    undone.push(text);
                }
                while let Some(text) = undone.pop() {
                    let stepped = log.redo(world);
                    assert!(stepped.unwrap_or_else(|error| panic!("step {step}: {error}")));
                    before.push(text);
                }
                assert_eq!(saved(world), now, "step {step}");
            } else {
                let operation = random(world, &mut rng, step);
                let said = operation.to_string();
                let (text, version) = (saved(world), world.version());
                match log.apply(world, operation) {
                    Ok(()) => {
                        before.push(text);
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
        // So that most steps change the world, rather than being refused.
        assert!(applied > 500, "{applied} applied");
        let last = saved(world);
        while let Some(text) = before.pop() {
            assert!(log.undo(world).expect("undone"));
            assert_eq!(saved(world), text);
        }
        assert!(!log.undo(world).expect("nothing to undo"));
        assert_eq!(saved(world).as_deref(), Some(start.as_str()));
        assert!(same_snapshots(&sorted(world), &start_sorted));
        let sprite = world.get::<Sprite>(x.a).expect("a sprite");
        assert_eq!(sprite.texture, x.texture);
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
}
