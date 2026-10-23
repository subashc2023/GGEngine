//! The hierarchy: entities with parents, which carry their children along. A child has its parent
//! as its [`Parent`] component, and the world keeps each entity's links to its children, so that
//! despawning one takes its descendants with it; both change only through [`World::set_parent`]
//! and [`World::despawn`]; as a scene's entities are spawned with theirs, by
//! [`World::load_scene`] and [`World::place_prefab`]; and through operations, as a create spawns
//! an entity with its parent, and a delete takes a subtree out, which its undo puts back.

use std::iter;

use gg_core::{Error, Result};

use super::table::Table;
use super::{Component, Entity, World};
use crate::reflect::Reflect;

/// The entity this one is a child of, which carries it along: its
/// [`Transform`](crate::Transform) is relative to its parent's. The world keeps it, so that the
/// hierarchy stays whole: a scene's entities are spawned with theirs, [`World::set_parent`] gives
/// it, changes it and takes it away, and code reads it but can't change it, nor can tools by name.
///
/// ```
/// use gg_engine::{Parent, Transform, World};
///
/// let mut world = World::default();
/// let parent = world.spawn(Transform::IDENTITY);
/// let child = world.spawn(Transform::IDENTITY);
/// world.set_parent(child, Some(parent))?;
/// assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)));
/// # Ok::<(), gg_engine::reflect::Error>(())
/// ```
///
/// It isn't [`Writable`](crate::Writable), so code that would change it doesn't compile:
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// # let entity = world.spawn(());
/// world.get_mut::<gg_engine::Parent>(entity);
/// ```
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// # let entity = world.spawn(());
/// world.insert(entity, gg_engine::Parent(entity));
/// ```
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// # let entity = world.spawn(());
/// world.remove::<gg_engine::Parent>(entity);
/// ```
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// world.query_mut::<&mut gg_engine::Parent>();
/// ```
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// world.query_mut::<(gg_engine::Entity, &mut gg_engine::Parent)>();
/// ```
///
/// ```compile_fail,E0277
/// # use gg_engine::{Parent, Transform, World};
/// # let mut world = World::default();
/// # let entity = world.spawn(());
/// world.spawn((Transform::IDENTITY, Parent(entity)));
/// ```
///
/// ```compile_fail,E0277
/// # let mut world = gg_engine::World::default();
/// # let entity = world.spawn(());
/// world.spawn(gg_engine::Parent(entity));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct Parent(pub Entity);

// And not `Writable`, so that only the world changes it.
impl Component for Parent {}

/// No slot.
const NONE: u32 = u32::MAX;

/// Where a slot's entity is in the hierarchy, by slots rather than handles, since a linked slot's
/// entity is alive: its parent, as its [`Parent`] has it, so that walks up the hierarchy read only
/// the world's slots; its first child; and the children before and after it among its parent's,
/// who ring round, the first's `prev` being the last. [`NONE`] where there's none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Links {
    parent: u32,
    first: u32,
    prev: u32,
    next: u32,
}

impl Links {
    /// A root's, without children.
    pub(super) const NONE: Self = Self {
        parent: NONE,
        first: NONE,
        prev: NONE,
        next: NONE,
    };
}

/// `slot`, unless it's [`NONE`].
fn some(slot: u32) -> Option<u32> {
    (slot != NONE).then_some(slot)
}

impl World {
    /// Makes `child` a child of `parent`, the last of its children, or a root if `parent` is
    /// `None`, and returns its old parent, which sets it back, as that parent's last child. It
    /// keeps its [`Transform`](crate::Transform), which is relative to its parent's, so that it
    /// moves in the world as its new parent is placed. Making it a child of the parent it has
    /// changes nothing, not even its place among its siblings. Spawning a child from code is
    /// spawning it, then setting its parent, which moves it to another table.
    ///
    /// Fails with `ecs.no_entity` if `child` or `parent` isn't alive, and with `ecs.parent_cycle`
    /// if `parent` is `child` or one of its descendants. A refusal changes nothing.
    pub fn set_parent(&mut self, child: Entity, parent: Option<Entity>) -> Result<Option<Entity>> {
        let name = self.name_of(child);
        let refused = |error: Error| {
            let message = format!("can't set entity {name}'s parent: {error}");
            error.with_message(message)
        };
        self.locate(child).map_err(refused)?;
        if let Some(parent) = parent {
            self.locate(parent).map_err(refused)?;
        }
        let old = self.parent_of(child.index);
        if old == parent {
            return Ok(old);
        }
        if let Some(parent) = parent {
            let mut at = parent.index;
            while let Some(up) = some(at) {
                if up == child.index {
                    let what = if parent == child {
                        "itself"
                    } else {
                        "one of its descendants"
                    };
                    let parent = self.name_of(parent);
                    return Err(Error::new(
                        "ecs.parent_cycle",
                        format!(
                            "can't make entity {name} a child of {parent}, which is {what}: \
                             an entity is never its own ancestor"
                        ),
                    ));
                }
                at = self.meta[up as usize].links.parent;
            }
        }
        self.unlink(child.index);
        match parent {
            Some(parent) => {
                // In place if it had a parent, or onto another table if it was a root.
                self.put(child, Parent(parent)).expect("the child is alive");
                self.append(parent.index, child.index);
            }
            None => {
                self.take::<Parent>(child).expect("the child is alive");
            }
        }
        Ok(old)
    }

    /// `entity`'s children, in the order they became its children: an order that follows from the
    /// changes made, as queries' does, and means nothing a game should depend on.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn children(&self, entity: Entity) -> Result<impl Iterator<Item = Entity> + '_> {
        self.locate(entity)?;
        let first = some(self.meta[entity.index as usize].links.first);
        let next = move |&at: &u32| {
            some(self.meta[at as usize].links.next).filter(|&next| Some(next) != first)
        };
        Ok(iter::successors(first, next).map(|slot| self.entity_at(slot)))
    }

    /// Removes `entity`, its components and its descendants, children before their parents.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive, removing nothing.
    pub fn despawn(&mut self, entity: Entity) -> Result<()> {
        self.locate(entity)?;
        self.take_apart(entity, |world, taken, _| world.remove_slot(taken.index));
        Ok(())
    }

    /// Takes `entity`, which is alive, and its descendants out of the hierarchy, children before
    /// their parents, each parent's first to last: each is unlinked, a root without children, and
    /// given to `out` beside its parent, if it had one, to take it out of the world.
    pub(super) fn take_apart(
        &mut self,
        entity: Entity,
        mut out: impl FnMut(&mut Self, Entity, Option<Entity>),
    ) {
        // Leaves first: down through first children to one without any, which is removed, and
        // then from its parent again, which needs no stack. Each removal moves a table's last row,
        // which may be one of the subtree's, so each is found where it is as it's removed.
        let mut at = entity.index;
        loop {
            while let Some(first) = some(self.meta[at as usize].links.first) {
                at = first;
            }
            let up = self.meta[at as usize].links.parent;
            let (taken, parent) = (self.entity_at(at), self.parent_of(at));
            self.unlink(at);
            out(self, taken, parent);
            if at == entity.index {
                return;
            }
            at = up;
        }
    }

    /// The table and row of `parent`, and of each of its ancestors in turn up to its root, for the
    /// engine's drawing, which carries each child along with them.
    pub(crate) fn lineage(&self, parent: Parent) -> impl Iterator<Item = (&Table, usize)> + '_ {
        let up = |&at: &u32| some(self.meta[at as usize].links.parent);
        iter::successors(Some(parent.0.index), up).map(|at| {
            let at = self.meta[at as usize]
                .location
                .expect("a linked slot's entity is alive");
            (&self.tables[at.table as usize], at.row as usize)
        })
    }

    /// The entity in slot `index`, which is alive.
    fn entity_at(&self, index: u32) -> Entity {
        Entity {
            index,
            generation: self.meta[index as usize].generation,
        }
    }

    /// The parent of the entity in slot `index`, if it has one.
    fn parent_of(&self, index: u32) -> Option<Entity> {
        some(self.meta[index as usize].links.parent).map(|parent| self.entity_at(parent))
    }

    /// Takes slot `index` out of its parent's children, if it has a parent, and leaves it a root
    /// with no siblings; its own children stay its.
    fn unlink(&mut self, index: u32) {
        let Links {
            parent, prev, next, ..
        } = self.meta[index as usize].links;
        let Some(parent) = some(parent) else {
            return;
        };
        if next == index {
            self.meta[parent as usize].links.first = NONE;
        } else {
            self.meta[prev as usize].links.next = next;
            self.meta[next as usize].links.prev = prev;
            let up = &mut self.meta[parent as usize].links;
            if up.first == index {
                up.first = next;
            }
        }
        let links = &mut self.meta[index as usize].links;
        links.parent = NONE;
        links.prev = NONE;
        links.next = NONE;
    }

    /// Makes slot `child`, a root, the last of slot `parent`'s children.
    pub(super) fn append(&mut self, parent: u32, child: u32) {
        let first = self.meta[parent as usize].links.first;
        let (prev, next) = match some(first) {
            None => {
                self.meta[parent as usize].links.first = child;
                (child, child)
            }
            Some(first) => {
                let last = self.meta[first as usize].links.prev;
                self.meta[last as usize].links.next = child;
                self.meta[first as usize].links.prev = child;
                (last, first)
            }
        };
        let links = &mut self.meta[child as usize].links;
        links.parent = parent;
        links.prev = prev;
        links.next = next;
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::reflect::Value;
    use crate::reflect::tests::{same_snapshots, snapshot};
    use crate::{EntityId, Transform};

    #[derive(Clone, Copy, Debug, PartialEq, crate::Component)]
    struct Mark(u32);

    /// Panics unless the world's links agree with its `Parent`s: each living entity's parent, as
    /// its links have it, is the one its `Parent` names, which is alive and lists it among its
    /// children once, between siblings who list it as theirs; a root has no siblings; no entity is
    /// its own ancestor; and a free, vacant or retired slot has no links.
    pub(in crate::ecs) fn check_links(world: &World) {
        let slots = world.meta.len();
        for (index, meta) in world.meta.iter().enumerate() {
            let index = index as u32;
            let links = meta.links;
            if meta.location.is_none() {
                assert_eq!(links, Links::NONE, "free slot {index}");
                continue;
            }
            let entity = world.entity_at(index);
            let parent = world.get::<Parent>(entity).map(|parent| parent.0);
            assert_eq!(world.parent_of(index), parent, "{entity}'s parent");
            match parent {
                Some(parent) => {
                    assert!(world.contains(parent), "{entity}'s parent {parent}");
                    let siblings = world.children(parent).expect("alive").take(slots + 1);
                    let listed = siblings.filter(|&sibling| sibling == entity).count();
                    assert_eq!(listed, 1, "{entity} among {parent}'s children");
                    let prev = world.meta[links.prev as usize].links;
                    let next = world.meta[links.next as usize].links;
                    assert_eq!(
                        (prev.next, next.prev),
                        (index, index),
                        "{entity}'s siblings"
                    );
                    let parents = (prev.parent, next.parent);
                    assert_eq!(parents, (links.parent, links.parent), "{entity}'s siblings");
                }
                None => assert_eq!((links.prev, links.next), (NONE, NONE), "root {entity}"),
            }
            for child in world.children(entity).expect("alive").take(slots + 1) {
                let parent = world.get::<Parent>(child);
                assert_eq!(parent, Some(&Parent(entity)), "{entity}'s child {child}");
            }
            let up = iter::successors(Some(index), |&at| {
                some(world.meta[at as usize].links.parent)
            });
            assert!(
                up.take(slots + 1).count() <= slots,
                "{entity} is its own ancestor"
            );
        }
    }

    fn kids(world: &World, entity: Entity) -> Vec<Entity> {
        world.children(entity).expect("alive").collect()
    }

    /// Every component of every entity, by entity, whichever tables they're in.
    fn state(world: &World) -> Vec<(Entity, EntityId, String, Value)> {
        let mut state = snapshot(world);
        state.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
        state
    }

    #[test]
    fn a_parent_is_given_changed_taken_away_and_set_back() {
        let mut world = World::default();
        let [a, b, child] = [0, 1, 2].map(|i| world.spawn((Transform::IDENTITY, Mark(i))));
        let before = state(&world);
        assert_eq!(world.set_parent(child, Some(a)).expect("no cycle"), None);
        assert_eq!(world.get::<Parent>(child), Some(&Parent(a)));
        // From one parent to another in place, its row where it was.
        let order: Vec<Entity> = world.query::<Entity>().collect();
        assert_eq!(world.set_parent(child, Some(b)).expect("no cycle"), Some(a));
        assert_eq!(world.query::<Entity>().collect::<Vec<_>>(), order);
        assert_eq!(world.get::<Parent>(child), Some(&Parent(b)));
        assert_eq!((kids(&world, a), kids(&world, b)), (vec![], vec![child]));
        check_links(&world);
        // Each old parent sets it back.
        assert_eq!(world.set_parent(child, Some(a)).expect("no cycle"), Some(b));
        assert_eq!(world.set_parent(child, None).expect("a root"), Some(a));
        assert_eq!(world.get::<Parent>(child), None);
        let names: Vec<&str> = world.components_of(child).expect("alive").collect();
        assert_eq!(names, ["Transform", "Mark"]);
        assert!(kids(&world, a).is_empty());
        assert!(same_snapshots(&state(&world), &before));
        check_links(&world);
    }

    #[test]
    fn the_parent_an_entity_has_changes_nothing() {
        let mut world = World::default();
        let parent = world.spawn(Mark(0));
        let children: Vec<Entity> = (1..4).map(|i| world.spawn(Mark(i))).collect();
        for &child in &children {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let before = state(&world);
        let first = children[0];
        assert_eq!(
            world.set_parent(first, Some(parent)).expect("no cycle"),
            Some(parent)
        );
        assert_eq!(kids(&world, parent), children, "the first stays first");
        assert_eq!(world.set_parent(parent, None).expect("a root"), None);
        assert!(same_snapshots(&state(&world), &before));
        check_links(&world);
    }

    #[test]
    fn cycles_and_dead_entities_are_refused_changing_nothing() {
        let mut world = World::default();
        let chain: Vec<Entity> = (0..4).map(|i| world.spawn(Mark(i))).collect();
        for pair in chain.windows(2) {
            world.set_parent(pair[1], Some(pair[0])).expect("no cycle");
        }
        let dead = world.spawn(Mark(9));
        world.despawn(dead).expect("alive");
        let seen = |world: &World| {
            let children: Vec<Vec<Entity>> = chain.iter().map(|&e| kids(world, e)).collect();
            (state(world), children)
        };
        let before = seen(&world);
        let (root, last) = (chain[0], chain[3]);
        // Each refusal, and the entities its message names: the living by their IDs alone, and the
        // dead by their handles.
        let refusals = [
            (
                root,
                Some(last),
                "ecs.parent_cycle",
                "one of its descendants",
                vec![root, last],
            ),
            (
                chain[1],
                Some(chain[2]),
                "ecs.parent_cycle",
                "descendants",
                vec![chain[1], chain[2]],
            ),
            (
                chain[1],
                Some(chain[1]),
                "ecs.parent_cycle",
                "itself",
                vec![chain[1]],
            ),
            (dead, Some(root), "ecs.no_entity", "", vec![dead]),
            (root, Some(dead), "ecs.no_entity", "", vec![root, dead]),
            (dead, None, "ecs.no_entity", "", vec![dead]),
        ];
        for (child, parent, code, says, named) in refusals {
            let error = world.set_parent(child, parent).expect_err("refused");
            assert_eq!(error.code(), code, "{error}");
            let message = error.to_string();
            assert!(message.contains(says), "{message}");
            for entity in named {
                match world.id(entity) {
                    Some(id) => {
                        assert!(message.contains(&id.to_string()), "{message}");
                        assert!(!message.contains(&entity.to_string()), "{message}");
                    }
                    None => assert!(message.contains(&entity.to_string()), "{message}"),
                }
            }
            let after = seen(&world);
            assert!(same_snapshots(&after.0, &before.0), "{error}");
            assert_eq!(after.1, before.1, "{error}");
            check_links(&world);
        }
    }

    #[test]
    fn children_keep_the_order_they_became_children_in() {
        let mut world = World::default();
        let parent = world.spawn(Mark(0));
        let [a, b, c, d] = [1, 2, 3, 4].map(|i| world.spawn(Mark(i)));
        for child in [a, b, c, d] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        assert_eq!(kids(&world, parent), [a, b, c, d]);
        // The first, one in the middle, the last and the only one leave in turn.
        for (leaving, left) in [
            (a, vec![b, c, d]),
            (c, vec![b, d]),
            (d, vec![b]),
            (b, vec![]),
        ] {
            world.set_parent(leaving, None).expect("a root");
            assert_eq!(kids(&world, parent), left, "after {leaving} left");
            check_links(&world);
        }
        for child in [d, a] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        assert_eq!(kids(&world, parent), [d, a]);
        check_links(&world);
    }

    /// The subtree goes with its root, leaves first. It shares a table with the root, which is
    /// a child itself, after them, so that each removal moves another of them to another row: a
    /// location kept from before would remove the wrong entity.
    #[test]
    fn despawning_takes_the_subtree_with_it() {
        let mut world = World::default();
        let [top, other] = [0, 1].map(|i| world.spawn(Mark(i)));
        // First into the table of children, and stays.
        let bystander = world.spawn(Mark(2));
        world.set_parent(bystander, Some(other)).expect("no cycle");
        let [root, middle, first, second] = [3, 4, 5, 6].map(|i| world.spawn(Mark(i)));
        for (child, parent) in [
            (first, middle),
            (second, middle),
            (middle, root),
            (root, top),
        ] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let after = world.spawn(Mark(7));
        let id = |entity: Entity| world.id(entity).expect("alive");
        let subtree = [root, middle, first, second].map(|gone| (gone, id(gone)));
        let kept = [(top, 0), (other, 1), (bystander, 2), (after, 7)];
        let kept = kept.map(|(kept, mark)| (kept, id(kept), mark));
        world.despawn(root).expect("alive");
        for (gone, id) in subtree {
            assert!(!world.contains(gone), "{gone}");
            assert_eq!(world.entity(id), None, "{gone}'s ID");
        }
        for (kept, id, mark) in kept {
            assert_eq!(world.get::<Mark>(kept), Some(&Mark(mark)), "{kept}");
            assert_eq!(world.id(kept), Some(id), "{kept}");
            assert_eq!(world.entity(id), Some(kept), "{kept}");
        }
        assert!(kids(&world, top).is_empty());
        assert_eq!(kids(&world, other), [bystander]);
        check_links(&world);
        // A lone child leaves its parent without children.
        world.despawn(bystander).expect("alive");
        assert!(kids(&world, other).is_empty());
        check_links(&world);
        let error = world.despawn(root).expect_err("despawned");
        assert_eq!(error.code(), "ecs.no_entity");
    }

    #[test]
    fn a_slot_taken_again_or_retired_has_no_links() {
        let mut world = World::default();
        let parent = world.spawn(Mark(0));
        let child = world.spawn(Mark(1));
        world.set_parent(child, Some(parent)).expect("no cycle");
        world.despawn(parent).expect("alive");
        // The child's slot was freed first, so the parent's is taken first.
        let [first, second] = [2, 3].map(|i| world.spawn(Mark(i)));
        assert_eq!([first.index, second.index], [parent.index, child.index]);
        for entity in [first, second] {
            assert!(kids(&world, entity).is_empty(), "{entity}");
            assert_eq!(world.get::<Parent>(entity), None, "{entity}");
        }
        check_links(&world);
        // As though its slot had held 4 billion entities.
        let retiring = world.spawn(Mark(4));
        world.set_parent(retiring, Some(first)).expect("no cycle");
        world.meta[retiring.index as usize].generation = NonZeroU32::MAX;
        world.despawn(first).expect("alive");
        // Two, since its parent's slot, freed last, is taken first.
        let taken = [5, 6].map(|i| world.spawn(Mark(i)));
        assert!(
            taken.iter().all(|entity| entity.index != retiring.index),
            "a retired slot takes no entity"
        );
        check_links(&world);
    }

    #[test]
    fn entities_keep_their_parents_and_children_as_they_change_tables() {
        let mut world = World::default();
        let parent = world.spawn(Mark(0));
        let [a, b] = [1, 2].map(|i| world.spawn(Mark(i)));
        for child in [a, b] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        world.insert(parent, Transform::IDENTITY).expect("alive");
        world.insert(a, Transform::IDENTITY).expect("alive");
        assert_eq!(kids(&world, parent), [a, b]);
        world.remove::<Mark>(parent).expect("alive");
        world.remove::<Mark>(b).expect("alive");
        assert_eq!(kids(&world, parent), [a, b]);
        for child in [a, b] {
            assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)), "{child}");
        }
        check_links(&world);
    }
}
