//! Resources: what a world holds beside its entities, one of each type, such as 2D physics' bodies
//! and contacts. Snapshots copy them and restores bring them back. Snapshot files hold those that
//! save themselves. No scene, hash, recording or dump holds any.

use std::any::{Any, TypeId};

use gg_core::{Error, Result};

use super::{Entity, World};
use crate::cells::{CellState, Seen};

/// What a world holds beside its entities, one of each type: the rest of the simulation, such as
/// [`Physics2D`](crate::Physics2D)'s bodies and contacts. Any `Clone + Send + Sync` type is one.
///
/// No scene, hash, recording or dump holds it: a world loaded from a scene starts without it, and
/// the hash, which reads components, ignores it. So it's what can be remade from the entities, or
/// what the simulation carries on from.
pub trait Resource: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Resource for T {}

/// A resource that snapshot files hold, so a run restored in another process gets it back as it
/// was, like [`Physics2D`](crate::Physics2D). It has a name and is one line of text, written by
/// [`save`](Self::save) and read by [`load`](Self::load).
///
/// A file holds the types registered with [`World::register_saved`]. Others are left out and
/// remade from the entities, as after a scene loads.
pub trait Saved: Resource {
    /// Its name in snapshot files, unique among saved resource types.
    const NAME: &'static str;

    /// Writes it as one line of text, naming entities by their [`World::id`]s.
    ///
    /// Fails as the type says, for what it can't write.
    fn save(&self, world: &World) -> Result<String>;

    /// Like [`save`](Self::save), as a server's client sees `world`: without the entities `hides`
    /// says, which the client's world lacks. Defaults to `save`; physics leaves out hidden
    /// entities' bodies.
    ///
    /// Fails like `save`.
    fn save_seen(&self, world: &World, hides: &dyn Fn(Entity) -> bool) -> Result<String> {
        let _ = hides;
        self.save(world)
    }

    /// Makes it again from `text`, which [`save`](Self::save) wrote, in `world`, whose entities
    /// are back under their IDs.
    ///
    /// Fails as the type says, for text it doesn't take.
    fn load(text: &str, world: &World) -> Result<Self>;
}

/// A world's resources in the order their types first came, which copies keep so a repeated copy
/// goes resource into resource; plus the types that save themselves.
#[derive(Default)]
pub(super) struct Resources {
    slots: Vec<Slot>,
    saved: Vec<SavedType>,
}

/// A [`Saved`] resource's type: its name, and what writes and reads it without the type.
#[derive(Clone, Copy)]
pub(super) struct SavedType {
    pub(super) type_id: TypeId,
    pub(super) name: &'static str,
    save: SaveSeen,
    pub(super) load: fn(&str, &World) -> Result<Box<dyn AnyResource>>,
}

/// [`Saved::save_seen`], without the type.
type SaveSeen = fn(&dyn AnyResource, &World, &dyn Fn(Entity) -> bool) -> Result<String>;

fn save_as<T: Saved>(
    resource: &dyn AnyResource,
    world: &World,
    hides: &dyn Fn(Entity) -> bool,
) -> Result<String> {
    let resource: &dyn Any = resource;
    let resource = resource.downcast_ref::<T>();
    resource.expect("a type's resource").save_seen(world, hides)
}

fn load_as<T: Saved>(text: &str, world: &World) -> Result<Box<dyn AnyResource>> {
    Ok(Box::new(T::load(text, world)?))
}

/// `None` while [`World::resource_scope`] has the resource out, or after it was removed.
struct Slot {
    type_id: TypeId,
    resource: Option<Box<dyn AnyResource>>,
}

/// A resource without its type: what copies it.
pub(super) trait AnyResource: Any + Send + Sync {
    fn cloned(&self) -> Box<dyn AnyResource>;

    /// Makes `to`, of the same type, a copy of this one, in the memory it holds.
    fn copy_to(&self, to: &mut dyn AnyResource);
}

impl<T: Resource> AnyResource for T {
    fn cloned(&self) -> Box<dyn AnyResource> {
        Box::new(self.clone())
    }

    fn copy_to(&self, to: &mut dyn AnyResource) {
        let to: &mut dyn Any = to;
        let to = to.downcast_mut::<T>();
        to.expect("a slot holds its type's resource")
            .clone_from(self);
    }
}

impl Clone for Resources {
    fn clone(&self) -> Self {
        let slots = self.slots.iter().map(|slot| Slot {
            type_id: slot.type_id,
            resource: slot.resource.as_ref().map(|resource| (**resource).cloned()),
        });
        Self {
            slots: slots.collect(),
            saved: self.saved.clone(),
        }
    }

    /// Copies each resource into the one in its place, in the memory it holds, while the slots
    /// are of the same types in the same order; copies them whole otherwise.
    fn clone_from(&mut self, source: &Self) {
        self.saved.clone_from(&source.saved);
        let ours = self.slots.iter().map(|slot| slot.type_id);
        if !ours.eq(source.slots.iter().map(|slot| slot.type_id)) {
            *self = source.clone();
            return;
        }
        for (slot, from) in self.slots.iter_mut().zip(&source.slots) {
            match (&mut slot.resource, &from.resource) {
                (Some(to), Some(from)) => (**from).copy_to(&mut **to),
                (to, from) => *to = from.as_ref().map(|from| (**from).cloned()),
            }
        }
    }
}

impl Resources {
    fn slot<T: Resource>(&self) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|slot| slot.type_id == TypeId::of::<T>())
    }

    fn slot_mut<T: Resource>(&mut self) -> Option<&mut Slot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.type_id == TypeId::of::<T>())
    }

    /// No resources, but the same saved types.
    pub(super) fn emptied(&self) -> Self {
        Self {
            slots: Vec::new(),
            saved: self.saved.clone(),
        }
    }

    pub(super) fn saved(&self, name: &str) -> Option<SavedType> {
        self.saved.iter().find(|saved| saved.name == name).copied()
    }

    /// Each saved resource's text, by name, in type order. With `seen`, as that client sees it:
    /// [`Saved::save_seen`], and its cells' state in place of the world's.
    ///
    /// Fails like [`Saved::save_seen`], with `snapshot.bad_name` for a name that isn't a word and
    /// `snapshot.line_break` for text with a line break.
    pub(super) fn save(
        &self,
        world: &World,
        seen: Option<&Seen>,
    ) -> Result<Vec<(&'static str, String)>> {
        use super::file::{one_line, one_word};

        let hidden = seen.map_or(&[][..], |seen| &seen.hidden[..]);
        let hides = |entity: Entity| super::hash::hidden(hidden, entity);
        let mut texts = Vec::new();
        for slot in &self.slots {
            let saved = self
                .saved
                .iter()
                .find(|saved| saved.type_id == slot.type_id);
            let (Some(saved), Some(resource)) = (saved, &slot.resource) else {
                continue;
            };
            one_word(saved.name)?;
            let cells = seen.map(|seen| &seen.cells);
            let text = match cells.filter(|_| slot.type_id == TypeId::of::<CellState>()) {
                Some(cells) => cells.save(world)?,
                None => (saved.save)(&**resource, world, &hides)?,
            };
            one_line(&text, &format!("resource `{}`", saved.name))?;
            texts.push((saved.name, text));
        }
        Ok(texts)
    }

    /// Puts `resource`, of type `type_id`, in its slot.
    pub(super) fn restore(&mut self, type_id: TypeId, resource: Box<dyn AnyResource>) {
        match self.slots.iter_mut().find(|slot| slot.type_id == type_id) {
            Some(slot) => slot.resource = Some(resource),
            None => self.slots.push(Slot {
                type_id,
                resource: Some(resource),
            }),
        }
    }

    /// Takes, after its own, each resource `from` holds of a type it holds none of. A world loaded
    /// from a file keeps the resources its layers made as they loaded.
    pub(super) fn keep(&mut self, from: Self) {
        for slot in from.slots {
            let ours = self.slots.iter().any(|ours| ours.type_id == slot.type_id);
            if slot.resource.is_some() && !ours {
                self.slots.push(slot);
            }
        }
    }
}

impl World {
    /// Has snapshot files hold the world's `T`, as [`Saved`] says, and its copies' and loaded
    /// worlds'. Registering it again changes nothing.
    ///
    /// Fails with `ecs.name_taken` if another type saves under its name.
    pub fn register_saved<T: Saved>(&mut self) -> Result<()> {
        let saved = &mut self.resources.saved;
        if let Some(other) = saved.iter().find(|saved| saved.name == T::NAME) {
            if other.type_id == TypeId::of::<T>() {
                return Ok(());
            }
            return Err(Error::new(
                "ecs.name_taken",
                format!(
                    "`{}` saves under the name `{}`, which another resource's type has",
                    std::any::type_name::<T>(),
                    T::NAME
                ),
            ));
        }
        saved.push(SavedType {
            type_id: TypeId::of::<T>(),
            name: T::NAME,
            save: save_as::<T>,
            load: load_as::<T>,
        });
        Ok(())
    }

    /// The world's `T`, if it has one.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        let resource: &dyn Any = &**self.resources.slot::<T>()?.resource.as_ref()?;
        resource.downcast_ref()
    }

    /// The world's `T`, to change, if it has one.
    pub fn resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        let slot = self.resources.slot_mut::<T>()?;
        let resource: &mut dyn Any = &mut **slot.resource.as_mut()?;
        resource.downcast_mut()
    }

    /// Gives the world `resource`, and returns the `T` it replaces, if any.
    pub fn insert_resource<T: Resource>(&mut self, resource: T) -> Option<T> {
        let old = self.remove_resource::<T>();
        let boxed: Box<dyn AnyResource> = Box::new(resource);
        match self.resources.slot_mut::<T>() {
            Some(slot) => slot.resource = Some(boxed),
            None => self.resources.slots.push(Slot {
                type_id: TypeId::of::<T>(),
                resource: Some(boxed),
            }),
        }
        old
    }

    /// Takes the world's `T` away, and returns it, if it had one.
    pub fn remove_resource<T: Resource>(&mut self) -> Option<T> {
        let resource: Box<dyn Any> = self.resources.slot_mut::<T>()?.resource.take()?;
        let resource = resource.downcast::<T>();
        Some(*resource.expect("a slot holds its type's resource"))
    }

    /// Runs `f` on the world and its `T` apart, so it can change both, and returns `f`'s result;
    /// `None`, without running `f`, if there's no `T`.
    ///
    /// Meanwhile the world has no `T`: a copy leaves it out, and one `f` inserts is replaced when
    /// the `T` goes back. It allocates nothing.
    pub fn resource_scope<T: Resource, R>(
        &mut self,
        f: impl FnOnce(&mut World, &mut T) -> R,
    ) -> Option<R> {
        let mut resource = self.resources.slot_mut::<T>()?.resource.take()?;
        let any: &mut dyn Any = &mut *resource;
        let done = f(
            self,
            any.downcast_mut()
                .expect("a slot holds its type's resource"),
        );
        match self.resources.slot_mut::<T>() {
            Some(slot) => slot.resource = Some(resource),
            None => self.resources.slots.push(Slot {
                type_id: TypeId::of::<T>(),
                resource: Some(resource),
            }),
        }
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Transform, World};

    /// Keeps its own memory across a copy taken into it.
    #[derive(Debug, PartialEq)]
    struct Counted(Vec<u32>);

    impl Clone for Counted {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }

        fn clone_from(&mut self, source: &Self) {
            self.0.clone_from(&source.0);
        }
    }

    /// Another, so that a world holds two.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Gravity(f32);

    /// A world holds one resource of each type, which an insert replaces and a removal takes away.
    #[test]
    fn a_world_holds_one_resource_of_each_type() {
        let mut world = World::default();
        assert_eq!(world.resource::<Gravity>(), None);
        assert_eq!(world.insert_resource(Gravity(-9.81)), None);
        assert_eq!(world.insert_resource(Counted(vec![1])), None);
        assert_eq!(world.insert_resource(Gravity(-1.0)), Some(Gravity(-9.81)));
        world.resource_mut::<Counted>().expect("inserted").0.push(2);
        assert_eq!(world.resource::<Counted>(), Some(&Counted(vec![1, 2])));
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(-1.0)));
        assert_eq!(world.remove_resource::<Gravity>(), Some(Gravity(-1.0)));
        assert_eq!(world.resource::<Gravity>(), None);
        assert_eq!(world.remove_resource::<Gravity>(), None);
        assert_eq!(world.insert_resource(Gravity(2.0)), None);
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(2.0)));
    }

    /// Restoring a snapshot brings the resources back as they were, into the memory the same type
    /// holds; one the snapshot lacked goes, and one the world lacked comes back.
    #[test]
    fn snapshots_copy_the_resources_and_restores_bring_them_back() {
        let mut world = World::default();
        world.insert_resource(Counted(Vec::with_capacity(64)));
        world.insert_resource(Gravity(-9.81));
        let snapshot = world.clone();
        let counted = world.resource_mut::<Counted>().expect("inserted");
        counted.0.push(7);
        let held = counted.0.as_ptr();
        world.insert_resource(Gravity(0.0));
        world.clone_from(&snapshot);
        let counted = world.resource::<Counted>().expect("restored");
        assert_eq!(counted, &Counted(Vec::new()));
        assert_eq!(counted.0.as_ptr(), held, "copied into the memory it held");
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(-9.81)));
        world.remove_resource::<Gravity>();
        let mut other = World::default();
        other.insert_resource(Gravity(3.0));
        other.insert_resource(Counted(vec![5]));
        other.clone_from(&world);
        assert_eq!(other.resource::<Counted>(), Some(&Counted(Vec::new())));
        assert_eq!(other.resource::<Gravity>(), None, "the copy had none");
        other.clone_from(&snapshot);
        assert_eq!(other.resource::<Gravity>(), Some(&Gravity(-9.81)));
    }

    /// Clearing a world, or replacing its entities with a scene's, keeps its resources, as it keeps
    /// its component types and stream of IDs: they belong to the world, not its entities.
    #[test]
    fn clearing_or_replacing_the_entities_keeps_the_resources() {
        let mut world = World::default();
        world.spawn(Transform::IDENTITY);
        world.insert_resource(Gravity(-9.81));
        world.clear();
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(-9.81)));
        let scene = world.save_scene(&[]).expect("an empty scene saves");
        world.replace_scene(&scene).expect("it loads");
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(-9.81)));
        let refused = world.replace_scene("(entities: {");
        assert!(refused.is_err());
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(-9.81)));
    }

    /// A scope has the world without the resource meanwhile, so a copy leaves it out; it goes back
    /// at the end, in place of one inserted meanwhile. A world without it runs nothing.
    #[test]
    fn a_scope_has_the_world_and_its_resource_apart() {
        let mut world = World::default();
        assert_eq!(world.resource_scope(|_, _: &mut Gravity| ()), None);
        world.insert_resource(Gravity(-9.81));
        let entity = world.spawn(Transform::IDENTITY);
        let copied = world.resource_scope(|world, gravity: &mut Gravity| {
            let at = world.get_mut::<Transform>(entity).expect("alive");
            at.translation.y = f64::from(gravity.0);
            gravity.0 = 1.0;
            let copy = world.clone();
            world.insert_resource(Gravity(5.0));
            copy.resource::<Gravity>().copied()
        });
        assert_eq!(copied, Some(None), "a copy leaves it out");
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(1.0)));
        let at = world.get::<Transform>(entity).expect("alive");
        assert_eq!(at.translation.y, f64::from(-9.81f32));
    }
}
