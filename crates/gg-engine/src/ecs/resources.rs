//! Resources: what a world holds beside its entities, one of each type, such as 2D physics' bodies
//! and contacts, which snapshots copy and restores bring back with the entities, but which no
//! scene, hash, recording or dump holds.

use std::any::{Any, TypeId};

use super::World;

/// What a world holds beside its entities, one of each type: the rest of the simulation, such as
/// [`Physics2D`](crate::Physics2D)'s bodies and contacts, so that a snapshot restored goes on as
/// the world it was taken from did. Clone, so that snapshots copy it, and `Send` and `Sync`, as
/// components are; any such type is one. No scene, hash, recording or dump holds it, so it's what
/// can be made again from the entities, or that the simulation carries on from: a world loaded
/// from a scene starts without it, and its hash, which reads the entities' components, says
/// nothing of it.
pub trait Resource: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Resource for T {}

/// A world's resources, in the order their types first came, which a copy keeps, so that one taken
/// again into a copy kept from before copies each into the one in its place.
#[derive(Default)]
pub(super) struct Resources(Vec<Slot>);

/// A type's resource, or `None` while [`World::resource_scope`] has it out of the world, or since
/// it was removed.
struct Slot {
    type_id: TypeId,
    resource: Option<Box<dyn AnyResource>>,
}

/// A resource, of a type the world doesn't know: what copies it.
trait AnyResource: Any + Send + Sync {
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
        let slots = self.0.iter().map(|slot| Slot {
            type_id: slot.type_id,
            resource: slot.resource.as_ref().map(|resource| (**resource).cloned()),
        });
        Self(slots.collect())
    }

    /// Copies each resource into the one of its type in its place, in the memory it holds, while
    /// the slots are of the same types in the same order; and copies them whole otherwise.
    fn clone_from(&mut self, source: &Self) {
        let ours = self.0.iter().map(|slot| slot.type_id);
        if !ours.eq(source.0.iter().map(|slot| slot.type_id)) {
            *self = source.clone();
            return;
        }
        for (slot, from) in self.0.iter_mut().zip(&source.0) {
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
        self.0.iter().find(|slot| slot.type_id == TypeId::of::<T>())
    }

    fn slot_mut<T: Resource>(&mut self) -> Option<&mut Slot> {
        self.0
            .iter_mut()
            .find(|slot| slot.type_id == TypeId::of::<T>())
    }
}

impl World {
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
            None => self.resources.0.push(Slot {
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
            None => self.resources.0.push(Slot {
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
            at.translation.y = gravity.0;
            gravity.0 = 1.0;
            let copy = world.clone();
            world.insert_resource(Gravity(5.0));
            copy.resource::<Gravity>().copied()
        });
        assert_eq!(copied, Some(None), "a copy leaves it out");
        assert_eq!(world.resource::<Gravity>(), Some(&Gravity(1.0)));
        let at = world.get::<Transform>(entity).expect("alive");
        assert_eq!(at.translation.y, -9.81);
    }
}
