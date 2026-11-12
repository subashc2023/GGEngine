//! The hierarchy: entities with parents, which carry their children along. A child has its parent
//! as its [`Parent`] component, and the world keeps each entity's links to its children, so that
//! despawning one takes its descendants with it; both change only through [`World::set_parent`]
//! and [`World::despawn`]; as a scene's entities are spawned with theirs, by
//! [`World::load_scene`] and [`World::place_prefab`]; and through operations, as a create spawns
//! an entity with its parent, which its undo takes out again, and a delete takes a subtree out,
//! which its undo puts back. Where its ancestors carry an entity is its pose.

use std::iter;

use gg_core::math::{Mat4, Quat};
use gg_core::{Error, Result};

use super::{Component, Entity, EntityId, Operation, World};
use crate::Transform;
use crate::reflect::Reflect;

/// The entity this one is a child of, which carries it along: its [`Transform`] is relative to its
/// parent's. The world keeps it, so that the hierarchy stays whole: a scene's entities are spawned
/// with theirs, [`World::set_parent`] gives it, changes it and takes it away, and code reads it but
/// can't change it, nor can tools by name.
///
/// ```
/// use gg_engine::{Parent, Transform, World};
///
/// let mut world = World::default();
/// let parent = world.spawn(Transform::IDENTITY);
/// let child = world.spawn(Transform::IDENTITY);
/// world.set_parent(child, Some(parent))?;
/// assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)));
/// # Ok::<(), gg_engine::Error>(())
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

/// Where an entity is in the world, as [`World::pose`] gives it, as the latest tick left it: where
/// frames draw it once they've drawn up to the tick, rather than where they draw it on the way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// The matrix from its own space to the world's: its root's [`Transform`]'s times each of its
    /// descendants' in turn down to its own, as drawing places it, bit for bit. An ancestor without
    /// a transform carries its children as though its transform were the identity, at its parent's
    /// origin, unturned and unscaled, and an entity without one is placed so too. Its last column
    /// is where its origin is.
    pub matrix: Mat4,
    /// Which way it faces: its own rotation turned by each of its ancestors' in turn, their scales
    /// aside, as the world's camera faces from it, so that a mirrored ancestor mirrors where it is
    /// but not which way it faces; multiplied from its root down, as the matrix is. A root's is
    /// its own; a child's is normalized, since each product of unit quaternions rounds a little
    /// off a unit.
    pub rotation: Quat,
}

impl Pose {
    /// The world's origin, unturned, at its own size: where a root's parent would be.
    pub const IDENTITY: Self = Self {
        matrix: Mat4::IDENTITY,
        rotation: Quat::IDENTITY,
    };
}

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
    /// keeps its [`Transform`], which is relative to its parent's, so that it moves in the world as
    /// its new parent is placed. Making it a child of the parent it has changes nothing, not even
    /// its place among its siblings. Spawning a child from code is spawning it, then setting its
    /// parent, which moves it to another table.
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

    /// Replaces the contents of `ids` with the IDs of the entity whose ID is `root` and of every
    /// entity under it, children, theirs and so on, sorted, which draws' IDs are searched by: none
    /// if no entity has that ID. `pending` is scratch space for the walk, which empties it. Neither
    /// allocates once it has held as many entities.
    pub(crate) fn fill_subtree_ids(
        &self,
        root: EntityId,
        pending: &mut Vec<Entity>,
        ids: &mut Vec<u64>,
    ) {
        ids.clear();
        pending.clear();
        pending.extend(self.entity(root));
        while let Some(entity) = pending.pop() {
            ids.push(self.id(entity).expect("alive").get());
            pending.extend(self.children(entity).expect("alive"));
        }
        ids.sort_unstable();
    }

    /// `entity`'s parent, as its [`Parent`] has it; `None` if it's a root.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn parent(&self, entity: Entity) -> Result<Option<Entity>> {
        self.locate(entity)?;
        Ok(self.parent_of(entity.index))
    }

    /// Where `entity` is in the world, as the latest tick left it, as [`Pose`] says: such as for a
    /// tool's handles, which show where it is and which way it faces.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    pub fn pose(&self, entity: Entity) -> Result<Pose> {
        self.locate(entity)?;
        let own = self.get::<Transform>(entity).copied();
        let own = own.unwrap_or(Transform::IDENTITY);
        let own = (own.matrix(), own.rotation);
        let Some(parent) = self.parent_of(entity.index) else {
            return Ok(Pose {
                matrix: own.0,
                rotation: own.1,
            });
        };
        let (matrix, rotation) = match self.placed(parent) {
            Some((matrix, rotation)) => (matrix * own.0, rotation * own.1),
            None => own,
        };
        Ok(Pose {
            matrix,
            rotation: rotation.normalize(),
        })
    }

    /// The operations that make the entity of ID `entity` the last of `parent`'s children, or a
    /// root for `None`, keeping where it is in the world, as an editor's reparent does: the
    /// reparent, which keeps its own [`Transform`], so that it moves as its new parent is placed,
    /// and a set of that transform to where it is in its new parent's space, from its matrix in the
    /// world and the new parent's, decomposed, which loses any shear the two give it between them
    /// and rounds a little. Applied as one edit, they undo as one. The reparent alone where there's
    /// nothing to keep, or nothing can be: for an entity without a transform, which goes to its
    /// new parent's origin as one does; for one whose parent stays; under a parent flattened to no
    /// size; and for an entity or a parent that isn't alive, or a parent that's the entity or one
    /// of its descendants, which the reparent's apply refuses.
    pub fn reparent_in_place(&self, entity: EntityId, parent: Option<EntityId>) -> Vec<Operation> {
        let reparent = Operation::Reparent { entity, parent };
        let Some(child) = self.entity(entity) else {
            return vec![reparent];
        };
        let to = match parent.map(|parent| self.entity(parent)) {
            Some(None) => return vec![reparent],
            Some(Some(to)) => Some(to),
            None => None,
        };
        let stays = self.parent(child).ok() == Some(to);
        let (Some(_), false, Ok(placed)) = (self.get::<Transform>(child), stays, self.pose(child))
        else {
            return vec![reparent];
        };
        let space = match to.map(|to| self.pose(to)) {
            Some(Ok(pose)) => pose.matrix,
            Some(Err(_)) => return vec![reparent],
            None => Mat4::IDENTITY,
        };
        let (scale, rotation, translation) =
            (space.inverse() * placed.matrix).to_scale_rotation_translation();
        let finite = scale.is_finite() && rotation.is_finite() && translation.is_finite();
        if space.determinant() == 0.0 || !finite {
            return vec![reparent];
        }
        let local = Transform {
            translation,
            rotation: rotation.normalize(),
            scale,
        };
        let set = Operation::Set {
            entity,
            component: "Transform".into(),
            path: String::new(),
            value: local.to_value(),
        };
        vec![reparent, set]
    }

    /// Where `entity`, which is alive, is as the latest tick left it, as drawing places it: its
    /// parent's matrix and rotation times its transform's, from its root down; `None` at the
    /// origin, unturned, at its own size, as a root without a transform is, and what it carries
    /// without one. Its lineage is walked, however deep it goes, rather than recursed, which a
    /// chain some thousands deep would overflow the stack with: kept on the stack to the depths
    /// games nest to, so that it allocates nothing, as a sound that follows an entity reads it each
    /// frame, and in a list past them.
    fn placed(&self, entity: Entity) -> Option<(Mat4, Quat)> {
        const NEAR: usize = 32;
        let (mut near, mut far) = ([NONE; NEAR], Vec::new());
        let lineage = iter::successors(Some(entity.index), |&at| {
            some(self.meta[at as usize].links.parent)
        });
        let mut depth = 0;
        for slot in lineage {
            match near.get_mut(depth) {
                Some(kept) => *kept = slot,
                None => far.push(slot),
            }
            depth += 1;
        }
        let down = far.iter().rev().chain(near[..depth.min(NEAR)].iter().rev());
        down.fold(None, |parent, &slot| {
            let own = self.get::<Transform>(self.entity_at(slot));
            let own = own.map(|own| (own.matrix(), own.rotation));
            match (parent, own) {
                (Some(parent), Some(own)) => Some((parent.0 * own.0, parent.1 * own.1)),
                (parent, own) => own.or(parent),
            }
        })
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

    /// `root`, which is alive, and its descendants, breadth first: each parent before its
    /// children, each parent's children in order, each beside its parent's index here. It's the
    /// order a [`Duplicate`](super::Operation::Duplicate)'s copies take their IDs in, which logs
    /// keep.
    pub(super) fn subtree(&self, root: Entity) -> Vec<(Entity, Option<usize>)> {
        let mut subtree = vec![(root, None)];
        let mut at = 0;
        while let Some(&(parent, _)) = subtree.get(at) {
            let children = self.children(parent).expect("alive");
            subtree.extend(children.map(|child| (child, Some(at))));
            at += 1;
        }
        subtree
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
    use crate::placed::Placed;
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

    /// An entity's pose is where drawing draws it, bit for bit, while it doesn't move: a child under
    /// ancestors that turn and scale unevenly, one mirrored and one without a transform; its origin
    /// and the way it faces are where the world's camera would see from. A root's is its
    /// transform's, and an entity without one is at its parent's origin.
    #[test]
    fn a_pose_is_where_drawing_draws_an_entity() {
        use gg_core::math::Vec3;
        use gg_renderer::{Draws, Shaders, Sprite, Textures};

        use crate::draw::{self, Poses, Scratch};
        use crate::font::Fonts;
        use crate::interpolation::Past;

        let placed = |x: f32, turn: f32, scale: [f32; 3]| Transform {
            translation: Vec3::new(x, 0.25 * x, -0.5 * x),
            rotation: Quat::from_rotation_z(turn) * Quat::from_rotation_x(0.5 * turn),
            scale: Vec3::from(scale),
        };
        let mut world = World::default();
        let root_at = placed(1.5, 0.7, [2.0, 0.5, 1.5]);
        let root = world.spawn(root_at);
        let bare = world.spawn(Mark(0));
        let middle_at = placed(-0.75, -1.1, [0.5, 1.5, -1.0]);
        let middle = world.spawn(middle_at);
        let child_at = placed(0.3, 0.4, [1.0, 3.0, 1.0]);
        let child = world.spawn((child_at, Sprite::default()));
        for (child, parent) in [(bare, root), (middle, bare), (child, middle)] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let pose = world.pose(child).expect("alive");
        let mut draws = Draws::default();
        let past = Past::default();
        let shaders = &mut Shaders::default();
        draw::draw_world(
            &world,
            &past,
            &Placed::of(&world, &past, 1.0),
            1.0,
            0,
            &mut draws,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        // Its one sprite's transform, which `Debug` shows to the bit, a zero's sign too.
        let drawn = format!("{draws:?}");
        let matrix = format!("transforms: [{:?}]", pose.matrix);
        assert!(drawn.contains(&matrix), "{matrix} in {drawn}");
        let at = world.location(child).expect("alive");
        let placed = Placed::of(&world, &past, 1.0);
        let poses = Poses::new(&world, &past, &placed, 1.0).expect("transforms");
        let (position, rotation) = poses.pose(at.table as usize, at.row as usize);
        let bits = |floats: &[f32]| floats.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        let origin = pose.matrix.w_axis.truncate();
        assert_eq!(bits(&origin.to_array()), bits(&position.to_array()));
        assert_eq!(bits(&pose.rotation.to_array()), bits(&rotation.to_array()));
        let root_pose = Pose {
            matrix: root_at.matrix(),
            rotation: root_at.rotation,
        };
        assert_eq!(world.pose(root).expect("alive"), root_pose);
        let bare_pose = world.pose(bare).expect("alive");
        assert_eq!(bare_pose.matrix, root_at.matrix());
        assert!(bare_pose.rotation.abs_diff_eq(root_at.rotation, 1e-6));
        world.despawn(root).expect("alive");
        let error = world.pose(child).expect_err("despawned");
        assert_eq!(error.code(), "ecs.no_entity");
    }

    /// An entity's pose is found however deep it is, as code or a scene may nest one, and so is its
    /// subtree despawned: by walks along its lineage, never a call for each level, which a chain
    /// some thousands deep would overflow the stack with.
    #[test]
    fn a_pose_is_found_however_deep_its_entity_is() {
        use gg_core::math::Vec3;

        let mut world = World::default();
        let step = Transform {
            translation: Vec3::X,
            ..Transform::IDENTITY
        };
        // Each made the parent of the root before, so that no reparent walks the chain.
        let deepest = world.spawn(step);
        let mut root = deepest;
        for _ in 1..100_000 {
            let parent = world.spawn(step);
            world.set_parent(root, Some(parent)).expect("no cycle");
            root = parent;
        }
        let pose = world.pose(deepest).expect("alive");
        assert_eq!(
            pose.matrix.w_axis.truncate(),
            Vec3::new(100_000.0, 0.0, 0.0)
        );
        world.despawn(root).expect("alive");
        assert!(!world.contains(deepest));
    }

    /// A reparent in place keeps where its entity is in the world, as its pose has it, under
    /// another parent, moved, turned and scaled, and at the roots, a set of its transform joining
    /// the reparent in one edit, which an undo takes back exactly; and is the reparent alone where
    /// there's nothing to keep: for an entity without a transform, one whose parent stays, and
    /// under a parent flattened to no size.
    #[test]
    fn a_reparent_in_place_keeps_where_its_entity_is_in_the_world() {
        use gg_core::math::Vec3;

        let mut world = World::default();
        let placed = |translation, turn, scale| Transform {
            translation,
            rotation: Quat::from_rotation_y(turn),
            scale,
        };
        let from = world.spawn(placed(Vec3::new(1.0, 2.0, 3.0), 0.7, Vec3::splat(2.0)));
        let to = world.spawn(placed(Vec3::new(-4.0, 0.5, 2.0), -1.2, Vec3::splat(0.5)));
        let flat = world.spawn(placed(Vec3::ZERO, 0.0, Vec3::new(1.0, 0.0, 1.0)));
        let child = placed(Vec3::new(0.5, -1.0, 0.25), 0.3, Vec3::new(1.0, 2.0, 1.0));
        let child = world.spawn(child);
        world.set_parent(child, Some(from)).expect("no cycle");
        let bare = world.spawn(Mark(0));
        let id = |world: &World, entity| world.id(entity).expect("alive");
        let before = state(&world);
        let seen = world.pose(child).expect("alive").matrix;
        let mut log = crate::Log::default();
        for parent in [Some(to), None] {
            let operations =
                world.reparent_in_place(id(&world, child), parent.map(|p| id(&world, p)));
            assert_eq!(operations.len(), 2, "{operations:?}");
            log.apply_all(&mut world, operations).expect("applied");
            assert_eq!(world.parent(child).expect("alive"), parent);
            let now = world.pose(child).expect("alive").matrix;
            assert!(now.abs_diff_eq(seen, 1e-4), "{now} from {seen}");
        }
        for _ in 0..2 {
            assert!(log.undo(&mut world).expect("undone"));
        }
        assert!(same_snapshots(&state(&world), &before));
        for (entity, parent) in [(bare, from), (child, from), (child, flat)] {
            let (entity, parent) = (id(&world, entity), Some(id(&world, parent)));
            let alone = vec![Operation::Reparent { entity, parent }];
            assert_eq!(world.reparent_in_place(entity, parent), alone);
        }
    }
}
