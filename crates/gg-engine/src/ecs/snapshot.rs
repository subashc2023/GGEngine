//! Snapshots: the world copied whole, which restores it, for rollback, client-side prediction and
//! hot reload; taken again into a copy kept from before, in its memory, which allocates as little
//! as [`World`] says.

use super::{World, next_serial};

impl Clone for World {
    /// A snapshot of the world: all of it, as [`World`] says, but another world, with a serial of
    /// its own.
    fn clone(&self) -> Self {
        let _zone = gg_core::zone!();
        let World {
            meta,
            free,
            entity_ids,
            resets,
            reset_count,
            by_id,
            id_room,
            kept,
            stream,
            serial: _,
            version,
            tables,
            by_signature,
            components,
            scratch: _,
            spawned: _,
        } = self;
        Self {
            meta: meta.clone(),
            free: free.clone(),
            entity_ids: entity_ids.clone(),
            resets: resets.clone(),
            reset_count: *reset_count,
            // With its buckets, and so its room.
            by_id: by_id.clone(),
            id_room: *id_room,
            kept: kept.clone(),
            stream: *stream,
            serial: next_serial(),
            version: *version,
            tables: tables.clone(),
            by_signature: by_signature.clone(),
            components: components.clone(),
            scratch: Vec::new(),
            spawned: Vec::new(),
        }
    }

    /// Makes the world a copy of `source`, in the memory it holds: restores it from a snapshot,
    /// or takes a snapshot of `source` into one kept from before. It's another world after, as a
    /// copy is.
    fn clone_from(&mut self, source: &Self) {
        let _zone = gg_core::zone!();
        let World {
            meta,
            free,
            entity_ids,
            resets,
            reset_count,
            by_id,
            id_room,
            kept,
            stream,
            serial: _,
            version,
            tables,
            by_signature,
            components,
            scratch: _,
            spawned: _,
        } = source;
        if !self.components.same(components) {
            // Its tables' ids would name other types than `source`'s same ids.
            self.components.clone_from(components);
            self.tables.clear();
            self.by_signature.clear();
        }
        // The map of tables by their types follows from the tables, so it's `source`'s already if
        // they have the same types in the same order; copying it would allocate each key.
        let ours = self.tables.iter().map(|table| &table.components);
        let shaped = ours.eq(tables.iter().map(|table| &table.components));
        // Cuts away the tables made since, and copies each table into the one in its place, in
        // its memory.
        self.tables.clone_from(tables);
        if !shaped {
            self.by_signature.clone_from(by_signature);
        }
        self.meta.clone_from(meta);
        self.free.clone_from(free);
        self.entity_ids.clone_from(entity_ids);
        self.resets.clone_from(resets);
        self.reset_count = *reset_count;
        // With `source`'s buckets, and so its room.
        self.by_id.clone_from(by_id);
        self.id_room = *id_room;
        self.kept.clone_from(kept);
        self.stream = *stream;
        self.version = *version;
        self.serial = next_serial();
    }
}

#[cfg(test)]
mod tests {
    use gg_core::Result;

    use super::*;
    use crate::ecs::hierarchy::tests::check_links;
    use crate::ecs::id::tests::{check_ids, vacated};
    use crate::ecs::log::tests::random;
    use crate::ecs::ops::tests::{Sealed, fixture};
    use crate::ecs::tests::Rng;
    use crate::reflect::tests::{Everything, everything, same_snapshots, snapshot};
    use crate::{Entity, Log, Parent, Sprite, Transform};

    /// A type no entity has until a world's snapshot has been taken, so that a table of it is one
    /// made since.
    #[derive(Clone, Copy, crate::Component)]
    struct Marked;

    /// A type that a world registers only once a snapshot has been taken of it.
    #[derive(Clone, Copy, crate::Component)]
    struct Late;

    /// Panics unless `world` is `twin`, but for its serial and its scratch: the same slots, the
    /// free ones in the same order, the same entities in the same tables and rows, with the same
    /// IDs, values, what's opaque in them, parents and children, resets of interpolation, the same
    /// IDs kept for vacant slots, the same stream of IDs to come, and the same component types.
    fn assert_same(world: &World, twin: &World, case: &str) {
        for world in [world, twin] {
            check_ids(world);
            check_links(world);
        }
        assert_eq!(world.meta, twin.meta, "{case}: the slots");
        assert_eq!(world.free, twin.free, "{case}: the free slots");
        assert_eq!(world.entity_ids, twin.entity_ids, "{case}: the IDs");
        assert_eq!(world.resets, twin.resets, "{case}: the resets");
        assert_eq!(
            world.reset_count, twin.reset_count,
            "{case}: the resets' count"
        );
        for &id in world.entity_ids.iter().flatten() {
            assert_eq!(world.entity(id), twin.entity(id), "{case}: {id}");
        }
        assert_eq!(world.by_id.len(), twin.by_id.len(), "{case}: the IDs found");
        assert_eq!(world.kept, twin.kept, "{case}: the IDs kept");
        assert_eq!(world.stream, twin.stream, "{case}: the stream");
        assert_eq!(world.version, twin.version, "{case}: the version");
        assert_eq!(world.tables.len(), twin.tables.len(), "{case}: the tables");
        for (index, (table, other)) in world.tables.iter().zip(&twin.tables).enumerate() {
            assert_eq!(
                table.components, other.components,
                "{case}: table {index}'s types"
            );
            assert_eq!(
                table.entities, other.entities,
                "{case}: table {index}'s rows"
            );
            for world in [world, twin] {
                let found = world.by_signature.get(&table.components);
                assert_eq!(
                    found,
                    Some(&(index as u32)),
                    "{case}: table {index} by its types"
                );
            }
        }
        let found = (world.by_signature.len(), twin.by_signature.len());
        assert_eq!(
            found,
            (world.tables.len(), twin.tables.len()),
            "{case}: tables by types"
        );
        assert!(
            same_snapshots(&snapshot(world), &snapshot(twin)),
            "{case}: the values"
        );
        assert_eq!(world.hash(), twin.hash(), "{case}: the hash");
        // What reflection doesn't read.
        for entity in world.entities(None).expect("every entity") {
            let texture = |world: &World| world.get::<Sprite>(entity).map(|sprite| sprite.texture);
            assert_eq!(texture(world), texture(twin), "{case}: {entity}'s texture");
            let handle = |world: &World| {
                world
                    .get::<Everything>(entity)
                    .map(|all| all.handle.clone())
            };
            assert_eq!(handle(world), handle(twin), "{case}: {entity}'s handle");
            let sealed = |world: &World| world.get::<Sealed>(entity).cloned();
            assert_eq!(sealed(world), sealed(twin), "{case}: {entity} sealed");
        }
        let types = world.component_types();
        assert!(
            types.eq(twin.component_types()),
            "{case}: the component types"
        );
        let described = world.types().iter().map(|(_, info)| &info.name);
        let theirs = twin.types().iter().map(|(_, info)| &info.name);
        assert!(described.eq(theirs), "{case}: the types described");
    }

    /// What refused a step, if anything did.
    fn refused<T>(result: Result<T>) -> Option<String> {
        result.err().map(|error| error.code().to_owned())
    }

    /// Takes a random step in `world`: an operation through `log`, mostly; now and then an undo or
    /// a redo; a spawn, or a despawn, which frees slots to the top of the free list; or a root
    /// saved and then loaded again in its place, or placed as a prefab. Two worlds that are the
    /// same take the same step from the same `rng`, and are refused alike.
    fn wander(world: &mut World, log: &mut Log, rng: &mut Rng, step: usize) -> Option<String> {
        let living: Vec<Entity> = world.entities(None).expect("every entity").collect();
        match rng.below(16) {
            0 => refused(log.undo(world)),
            1 => refused(log.redo(world)),
            2 => {
                world.spawn((Transform::IDENTITY, Sprite::default()));
                None
            }
            3 if !living.is_empty() => refused(world.despawn(living[rng.below(living.len())])),
            4 => {
                let roots: Vec<Entity> = living
                    .into_iter()
                    .filter(|&entity| world.get::<Parent>(entity).is_none())
                    .collect();
                if roots.is_empty() {
                    return None;
                }
                let root = roots[rng.below(roots.len())];
                let text = match world.save_scene(&[root]) {
                    Ok(text) => text,
                    Err(error) => return Some(error.code().to_owned()),
                };
                if rng.below(2) == 0 {
                    world.despawn(root).expect("alive");
                    refused(world.load_scene(&text))
                } else {
                    refused(world.place_prefab(&text, None))
                }
            }
            _ => {
                let operation = random(world, rng, step);
                refused(log.apply(world, operation))
            }
        }
    }

    /// A world and its log, and its twin and the twin's, which take the same steps.
    struct Twins {
        x: World,
        x_log: Log,
        twin: World,
        twin_log: Log,
    }

    impl Twins {
        /// Takes the same random step in both worlds, which are the same.
        fn step(&mut self, rng: &mut Rng, step: usize) {
            let mut same = Rng(rng.0);
            let ours = wander(&mut self.x, &mut self.x_log, rng, step);
            let theirs = wander(&mut self.twin, &mut self.twin_log, &mut same, step);
            assert_eq!(ours, theirs, "step {step}");
        }

        /// Does `change` to both worlds.
        fn both(&mut self, change: impl Fn(&mut World)) {
            change(&mut self.x);
            change(&mut self.twin);
        }
    }

    /// Two worlds take the same random steps, and then, round after round, one of them a snapshot,
    /// and the other stops there while the first goes on alone, through operations, undos, spawns
    /// and despawns, loads, prefabs, a log cleared, a table made and, every other round, a type
    /// registered, and is restored: it's then the other, field by field, refuses what its log held,
    /// and takes the same steps as the other after.
    #[test]
    fn a_restored_snapshot_matches_the_world_it_was_taken_from() {
        let mut worlds = Twins {
            x: fixture().world,
            x_log: Log::default(),
            twin: fixture().world,
            twin_log: Log::default(),
        };
        let mut rng = Rng(5);
        for step in 0..300 {
            worlds.step(&mut rng, step);
        }
        worlds.both(|world| world.register::<Marked>().expect("registered"));
        // Resets of interpolation, so that the snapshots' copies of them show.
        worlds.both(|world| {
            let some: Vec<Entity> = world.query::<Entity>().step_by(3).collect();
            for entity in some {
                world.reset_interpolation(entity).expect("alive");
            }
        });
        assert_same(&worlds.x, &worlds.twin, "in step");
        let mut snap = worlds.x.clone();
        // The slots the snapshots kept, and the entries of the restored world's logs it refused,
        // which the rounds are to have.
        let (mut kept, mut refused) = (0, 0);
        for round in 0..4 {
            let step = 1000 * (round + 1);
            match round {
                // Taken into a snapshot kept from before, over one that held a smaller world, and
                // a larger one.
                2 => {
                    worlds.both(|world| {
                        for _ in 0..40 {
                            world.spawn((Transform::IDENTITY, Sprite::default()));
                        }
                    });
                    snap.clone_from(&worlds.x);
                }
                3 => {
                    worlds.both(|world| {
                        let living: Vec<Entity> = world.entities(None).expect("all").collect();
                        for entity in living.into_iter().step_by(2) {
                            if world.contains(entity) {
                                world.despawn(entity).expect("alive");
                            }
                        }
                    });
                    snap.clone_from(&worlds.x);
                }
                0 => {}
                _ => snap.clone_from(&worlds.x),
            }
            assert_same(&snap, &worlds.twin, &format!("round {round}'s snapshot"));
            assert_ne!(snap.serial, worlds.x.serial, "round {round}");
            kept += vacated(&snap);
            let x = &mut worlds.x;
            for at in 0..50 {
                wander(x, &mut worlds.x_log, &mut rng, step + at);
                if at == 25 {
                    worlds.x_log.clear(x);
                    x.spawn((Transform::IDENTITY, Marked));
                    if round % 2 == 1 {
                        x.spawn(Late);
                    }
                }
            }
            assert!(
                x.tables.len() > snap.tables.len(),
                "round {round}: a table made since"
            );
            match round {
                1 => worlds.x = snap.clone(),
                2 => {
                    // Restored twice from the one snapshot.
                    x.clone_from(&snap);
                    for at in 50..60 {
                        wander(x, &mut worlds.x_log, &mut rng, step + at);
                    }
                    x.clone_from(&snap);
                }
                _ => x.clone_from(&snap),
            }
            let case = format!("round {round}, restored");
            assert_same(&worlds.x, &worlds.twin, &case);
            let serials = [worlds.twin.serial, snap.serial];
            assert!(!serials.contains(&worlds.x.serial), "{case}");
            assert_eq!(vacated(&worlds.x), vacated(&worlds.twin), "{case}");
            // The log of the world restored is another world's, which it refuses whole.
            let x = &mut worlds.x;
            while worlds.x_log.undoable().is_some() {
                let error = worlds.x_log.undo(x).expect_err("another world's");
                assert_eq!(error.code(), "ops.other_world", "{case}");
                refused += 1;
            }
            while worlds.x_log.redoable().is_some() {
                let error = worlds.x_log.redo(x).expect_err("another world's");
                assert_eq!(error.code(), "ops.other_world", "{case}");
                refused += 1;
            }
            worlds.x_log.clear(x);
            assert_same(&worlds.x, &worlds.twin, &format!("{case}, its log refused"));
            // The twin's log is dropped without being cleared, so that the slots it keeps stay
            // vacant, as the restored world's do for good.
            worlds.x_log = Log::default();
            worlds.twin_log = Log::default();
            for at in 100..200 {
                worlds.step(&mut rng, step + at);
                assert_same(
                    &worlds.x,
                    &worlds.twin,
                    &format!("{case}, step {}", step + at),
                );
            }
        }
        assert!(
            kept > 0 && refused > 0,
            "{kept} slots kept, {refused} entries refused"
        );
    }

    /// A world that registered the same types in another order, so that its ids name others, with
    /// tables and slots of its own.
    fn reversed() -> World {
        let mut world = World::seeded(7);
        let registered = [
            world.register::<Sealed>(),
            world.register::<Everything>(),
            world.register::<Sprite>(),
            world.register::<Transform>(),
        ];
        registered
            .into_iter()
            .for_each(|done| done.expect("registered"));
        // Its first table has the ids of the fixture's first, which are other types.
        for count in 0..20 {
            world.spawn(Sealed(count));
        }
        world.spawn((Transform::IDENTITY, Sprite::default()));
        world
    }

    /// A world that registered the fixture's types in the fixture's order, so that its ids name the
    /// same types, but made its tables in another order, so that some of its tables' types aren't
    /// those of the fixture's tables in their places.
    fn reordered() -> World {
        let mut world = World::seeded(9);
        let registered = [
            world.register::<Transform>(),
            world.register::<Sprite>(),
            world.register::<Everything>(),
            world.register::<Sealed>(),
        ];
        registered
            .into_iter()
            .for_each(|done| done.expect("registered"));
        world.spawn(Sealed(1));
        world.spawn((Transform::IDENTITY, Sprite::default()));
        world.spawn(everything());
        world
    }

    /// A world of other types restored from a snapshot is the snapshot's world, as a snapshot of a
    /// world of other types taken into one kept from before is that world, and either goes on as
    /// its source does; and so is a world of the same types whose tables were made in another
    /// order, as by another branch from the snapshot.
    #[test]
    fn a_snapshot_restores_into_a_world_of_other_types() {
        let x = fixture().world;
        let mut over = reordered();
        assert!(over.components.same(&x.components), "the fixture's types");
        over.clone_from(&x);
        assert_same(&over, &x, "restored over other tables");
        let mut kept = x.clone();
        let from = reordered();
        kept.clone_from(&from);
        assert_same(&kept, &from, "taken over other tables");
        // Snapshots taken on two branches from one world, which made other tables in one place.
        let mut x = fixture().world;
        let before = x.clone();
        x.spawn((Sprite::default(), Sealed(1)));
        let mut kept = x.clone();
        x.clone_from(&before);
        x.spawn((Transform::IDENTITY, Sealed(2)));
        kept.clone_from(&x);
        assert_same(&kept, &x, "taken over another branch");
        for world in [&mut kept, &mut x] {
            world.spawn((Sprite::default(), Sealed(3)));
        }
        assert_same(&kept, &x, "a step after the snapshot over another branch");
        let mut x = fixture().world;
        let mut over = reversed();
        over.clone_from(&x);
        assert_same(&over, &x, "restored over other types");
        let mut kept = x.clone();
        let mut from = reversed();
        kept.clone_from(&from);
        assert_same(&kept, &from, "taken over other types");
        for world in [&mut over, &mut x, &mut kept, &mut from] {
            world.spawn((Transform::IDENTITY, Sprite::default()));
            world.spawn(Marked);
        }
        assert_same(&over, &x, "a step after the restore");
        assert_same(&kept, &from, "a step after the snapshot");
        // One without tables over one with them, which leaves none of them found.
        let mut empty = World::seeded(1);
        let mut over = reversed();
        over.clone_from(&empty);
        assert_same(&over, &empty, "restored from a world without tables");
        for world in [&mut over, &mut empty] {
            world.spawn(Transform::IDENTITY);
        }
        assert_same(&over, &empty, "a step after the restore without tables");
    }

    /// Where `world`'s buffers are, and how big, which taking a snapshot into it again, or
    /// restoring it from one, of the same shape, leaves as they were.
    fn held(world: &World) -> Vec<(usize, usize)> {
        let mut held = vec![
            (world.meta.as_ptr().addr(), world.meta.capacity()),
            (world.free.as_ptr().addr(), world.free.capacity()),
            (
                world.entity_ids.as_ptr().addr(),
                world.entity_ids.capacity(),
            ),
            (world.resets.as_ptr().addr(), world.resets.capacity()),
            (0, world.by_id.capacity()),
            (0, world.kept.capacity()),
        ];
        let transform = world.component::<Transform>().expect("registered");
        for table in &world.tables {
            held.push((table.entities.as_ptr().addr(), table.entities.capacity()));
            if let Some(column) = table.column::<Transform>(transform) {
                held.push((column.as_ptr().addr(), column.len()));
            }
        }
        // In the map's own order, which a copy that keeps the map keeps.
        let keys = world.by_signature.keys();
        held.extend(keys.map(|key| (key.as_ptr().addr(), key.len())));
        held
    }

    /// A snapshot taken again into the one kept, and the world restored from it, while only
    /// values change, are copied into the memory each holds.
    #[test]
    fn taking_and_restoring_again_keep_their_memory() {
        let mut x = fixture();
        x.world.despawn(x.g).expect("alive");
        let mut snap = x.world.clone();
        let moved = |world: &mut World| {
            let all = world.query_mut::<&mut Transform>();
            all.for_each(|transform| transform.translation.x += 1.0);
        };
        moved(&mut x.world);
        let before = held(&snap);
        snap.clone_from(&x.world);
        assert_eq!(held(&snap), before, "taken again");
        let before = held(&x.world);
        moved(&mut x.world);
        x.world.clone_from(&snap);
        assert_eq!(held(&x.world), before, "restored");
        let transform = x.world.get::<Transform>(x.r).expect("a transform");
        assert_eq!(transform.translation.x, 1.0);
    }
}
