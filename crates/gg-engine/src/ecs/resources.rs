//! Resources: what a world holds beside its entities, one of each type, such as 2D physics' bodies
//! and contacts, which snapshots copy and restores bring back with the entities, and snapshot files
//! hold those that save themselves, but which no scene, hash, recording or dump holds.

use std::any::{Any, TypeId};

use gg_core::{Error, Result};

use super::World;
use crate::cells::CellState;

/// What a world holds beside its entities, one of each type: the rest of the simulation, such as
/// [`Physics2D`](crate::Physics2D)'s bodies and contacts, so that a snapshot restored goes on as
/// the world it was taken from did. Clone, so that snapshots copy it, and `Send` and `Sync`, as
/// components are; any such type is one. No scene, hash, recording or dump holds it, so it's what
/// can be made again from the entities, or that the simulation carries on from: a world loaded
/// from a scene starts without it, and its hash, which reads the entities' components, says
/// nothing of it.
pub trait Resource: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Resource for T {}

/// A resource that snapshot files hold, so that a run restored from one in another process takes it
/// back as it was, as [`Physics2D`](crate::Physics2D) does: under a name of its own, as a line of
/// text that [`save`](Self::save) writes and [`load`](Self::load) reads. A world's file holds those
/// whose types it registered with [`World::register_saved`]; others are left out, made again from
/// the entities, as after a scene loads.
pub trait Saved: Resource {
    /// Its name in snapshot files, which no other saved resource's type has.
    const NAME: &'static str;

    /// Writes it as a line of text, without line breaks, in `world`, whose entities it names by
    /// their IDs, as [`World::id`] gives them.
    ///
    /// Fails as the type says, for what it can't write.
    fn save(&self, world: &World) -> Result<String>;

    /// Makes it again from `text`, which [`save`](Self::save) wrote, in `world`, whose entities
    /// are back, each under its ID.
    ///
    /// Fails as the type says, for text it doesn't take.
    fn load(text: &str, world: &World) -> Result<Self>;
}

/// A world's resources, in the order their types first came, which a copy keeps, so that one taken
/// again into a copy kept from before copies each into the one in its place; and the types that
/// save themselves.
#[derive(Default)]
pub(super) struct Resources {
    slots: Vec<Slot>,
    saved: Vec<SavedType>,
}

/// A [`Saved`] resource's type: its name, and what writes and reads a resource of it without its
/// type.
#[derive(Clone, Copy)]
pub(super) struct SavedType {
    pub(super) type_id: TypeId,
    pub(super) name: &'static str,
    save: fn(&dyn AnyResource, &World) -> Result<String>,
    pub(super) load: fn(&str, &World) -> Result<Box<dyn AnyResource>>,
}

fn save_as<T: Saved>(resource: &dyn AnyResource, world: &World) -> Result<String> {
    let resource: &dyn Any = resource;
    let resource = resource.downcast_ref::<T>();
    resource.expect("a type's resource").save(world)
}

fn load_as<T: Saved>(text: &str, world: &World) -> Result<Box<dyn AnyResource>> {
    Ok(Box::new(T::load(text, world)?))
}

/// A type's resource, or `None` while [`World::resource_scope`] has it out of the world, or since
/// it was removed.
struct Slot {
    type_id: TypeId,
    resource: Option<Box<dyn AnyResource>>,
}

/// A resource, of a type the world doesn't know: what copies it.
pub(super) trait AnyResource: Any + Send + Sync {
    fn cloned(&self) -> Box<dyn AnyResource>;

    /// Makes `to`, a resource of the same type, a copy of this one, in the memory it holds.
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

    /// Copies each resource into the one of its type in its place, in the memory it holds, while
    /// the slots are of the same types in the same order; and copies them whole otherwise.
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
    /// The slot of `T`'s resource, if `T` has one.
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

    /// No resources, but the types that save themselves these have.
    pub(super) fn emptied(&self) -> Self {
        Self {
            slots: Vec::new(),
            saved: self.saved.clone(),
        }
    }

    /// The type that saves its resources under `name`, if one does.
    pub(super) fn saved(&self, name: &str) -> Option<SavedType> {
        self.saved.iter().find(|saved| saved.name == name).copied()
    }

    /// Writes each resource that saves itself after `out`, a line each, `resource`, its name and
    /// its text, in `world`, in the order their types first came: `cells` in place of the world's
    /// cells' state, if it's given, as a client's view has its own.
    ///
    /// Fails as a resource's [`Saved::save`] does, with `snapshot.bad_name` for a name that isn't a
    /// word, and with `snapshot.line_break` for text with a line break.
    pub(super) fn save(
        &self,
        world: &World,
        out: &mut String,
        cells: Option<&CellState>,
    ) -> Result<()> {
        use std::fmt::Write as _;

        use super::file::{one_line, one_word};

        for slot in &self.slots {
            let saved = self
                .saved
                .iter()
                .find(|saved| saved.type_id == slot.type_id);
            let (Some(saved), Some(resource)) = (saved, &slot.resource) else {
                continue;
            };
            one_word(saved.name)?;
            let text = match cells.filter(|_| slot.type_id == TypeId::of::<CellState>()) {
                Some(cells) => cells.save(world)?,
                None => (saved.save)(&**resource, world)?,
            };
            one_line(&text, &format!("resource `{}`", saved.name))?;
            writeln!(out, "resource {} {text}", saved.name).expect("a string takes any text");
        }
        Ok(())
    }

    /// Puts `resource`, of the type `type_id` names, in its type's slot.
    pub(super) fn restore(&mut self, type_id: TypeId, resource: Box<dyn AnyResource>) {
        match self.slots.iter_mut().find(|slot| slot.type_id == type_id) {
            Some(slot) => slot.resource = Some(resource),
            None => self.slots.push(Slot {
                type_id,
                resource: Some(resource),
            }),
        }
    }

    /// Takes each resource `from` holds of a type it holds none of, after its own, as a world
    /// loaded from a file keeps those its layers made as they loaded, which the file doesn't hold.
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
    /// Has snapshot files hold the world's `T`, as [`Saved`] says, and its copies', and the
    /// worlds a file loads into it. Registering it again changes nothing.
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

    /// Gives the world `resource`, and returns the `T` it had, if any, which `resource` replaces.
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

    /// Runs `f` with the world and its `T` apart, so that it changes both, as a system that steps
    /// its resource over the entities' components does, and returns what `f` does; or, if the
    /// world has no `T`, returns `None` without running it. Meanwhile the world has no `T`: a copy
    /// taken leaves it out, and one that `f` inserts is replaced as the `T` goes back, into the
    /// world as `f` leaves it. It allocates nothing.
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

    /// A resource that counts, in memory of its own, which a copy taken into it keeps.
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

    /// A snapshot copies the resources, and restoring it brings them back as they were, into the
    /// memory a resource of the same type holds; one the world had that the snapshot hadn't goes,
    /// and one the snapshot had that the world hadn't comes back.
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
    /// its component types and its stream of IDs: they go with the world, not with its entities.
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

    /// A scope has the world and its resource apart, the world without it meanwhile, so that a
    /// copy leaves it out; it goes back as the scope ends, in place of one inserted meanwhile. A
    /// world without the resource runs nothing.
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
