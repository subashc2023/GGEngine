//! Snapshots: the world copied whole. The tool API's `play` runs a copy and `stop` returns to
//! the original; rollback ([`rewind`](World::rewind)) and SyncTest restore one. A snapshot taken
//! into a kept copy reuses that copy's memory, allocating as [`World`] says.

use super::{World, next_serial};

impl Clone for World {
    /// A snapshot of all of the world ([`World`]): another world, with its own serial.
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
            change_tick,
            tables,
            by_signature,
            components,
            scratch: _,
            lists: _,
            resources,
            assets,
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
            change_tick: *change_tick,
            tables: tables.clone(),
            by_signature: by_signature.clone(),
            components: components.clone(),
            scratch: Vec::new(),
            lists: Vec::new(),
            resources: resources.clone(),
            assets: assets.clone(),
        }
    }

    /// Makes the world a copy of `source` in its own memory: a restore, or a snapshot into a kept
    /// one. It's another world after.
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
            change_tick,
            tables,
            by_signature,
            components,
            scratch: _,
            lists: _,
            resources,
            assets,
        } = source;
        if !self.components.same(components) {
            // Its tables' ids would name other types than `source`'s.
            self.components.clone_from(components);
            self.tables.clear();
            self.by_signature.clear();
        }
        self.components.mods_loaded = components.mods_loaded;
        // The map of tables by types is `source`'s already if the tables' types match in order;
        // copying it would allocate each key.
        let ours = self.tables.iter().map(|table| table.components());
        let shaped = ours.eq(tables.iter().map(|table| table.components()));
        // Cuts away tables made since, and copies each table into its place's memory.
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
        // Only grows, so a tick drawing saw is never one the world marks again.
        self.change_tick = self.change_tick.max(*change_tick);
        self.resources.clone_from(resources);
        self.assets.clone_from(assets);
        self.serial = next_serial();
    }
}

impl World {
    /// Takes the world back to `snapshot`, taken since a log last changed it (rollback's rerun
    /// point). Like [`clone_from`](Clone::clone_from), but:
    /// - the serial stays, so its logs still undo through it;
    /// - the version moves past both worlds', so what's kept by version (drawing's order) is
    ///   found again;
    /// - every row of every column is marked changed at a tick past both worlds', so what's kept
    ///   by ticks (drawing's copies) refreshes.
    pub(crate) fn rewind(&mut self, snapshot: &World) {
        let (serial, version) = (self.serial, self.version.max(snapshot.version));
        self.clone_from(snapshot);
        self.serial = serial;
        self.version = version + 1;
        self.change_tick += 1;
        for table in &mut self.tables {
            table.mark_all(self.change_tick);
        }
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

    /// A type no entity has until a snapshot is taken, so a table of it is made since.
    #[derive(Clone, Copy, crate::Component)]
    struct Marked;

    /// A type registered only after a snapshot is taken.
    #[derive(Clone, Copy, crate::Component)]
    struct Late;

    /// Panics unless `world` equals `twin` except serial and scratch: slots (free ones in order),
    /// tables and rows, IDs, values, opaque parts, hierarchy, interpolation resets, IDs kept for
    /// vacant slots, the ID stream, and component types.
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
                table.components(),
                other.components(),
                "{case}: table {index}'s types"
            );
            assert_eq!(
                table.entities(),
                other.entities(),
                "{case}: table {index}'s rows"
            );
            for world in [world, twin] {
                let found = world.by_signature.get(table.components());
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

    /// A random step in `world`: mostly an operation through `log`; sometimes an undo, redo,
    /// spawn, despawn (frees slots to the free list's top), or a root saved and reloaded or
    /// placed as a prefab. Equal worlds and `rng`s step and are refused alike.
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

    /// A world and its log, and a twin and its log, taking the same steps.
    struct Twins {
        x: World,
        x_log: Log,
        twin: World,
        twin_log: Log,
    }

    impl Twins {
        /// Takes the same random step in both worlds.
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

    /// Two worlds take the same random steps. Each round, one snapshots and then goes on alone
    /// (edits, loads, prefabs, a log cleared, a table made, and every other round a type
    /// registered) before restoring. It then equals the other field by field, refuses its log's
    /// entries, and steps like the other after.
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
        // Interpolation resets, so the snapshots' copies of them show.
        worlds.both(|world| {
            let some: Vec<Entity> = world.query::<Entity>().step_by(3).collect();
            for entity in some {
                world.reset_interpolation(entity).expect("alive");
            }
        });
        assert_same(&worlds.x, &worlds.twin, "in step");
        let mut snap = worlds.x.clone();
        // Slots the snapshots kept and log entries the restored world refused: the rounds must
        // have some.
        let (mut kept, mut refused) = (0, 0);
        for round in 0..4 {
            let step = 1000 * (round + 1);
            match round {
                // Taken into a kept snapshot, over one that held a smaller world, and a larger.
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
            // The restored world's log is another world's: refused whole.
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
            // The twin's log is dropped uncleared, so the slots it keeps stay vacant, as the
            // restored world's do for good.
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

    /// A world rewound to its snapshot after steps outside any log (as ticks do) holds the
    /// snapshot's content, keeps its serial, and has a version past both's. Its log undoes and
    /// redoes through it as the twin's, which never went on, does through the twin.
    #[test]
    fn a_world_rewound_is_the_same_world_which_its_log_undoes_as_before() {
        let mut worlds = Twins {
            x: fixture().world,
            x_log: Log::default(),
            twin: fixture().world,
            twin_log: Log::default(),
        };
        let mut rng = Rng(9);
        for step in 0..300 {
            worlds.step(&mut rng, step);
        }
        let (serial, snap) = (worlds.x.serial, worlds.x.clone());
        let x = &mut worlds.x;
        for _ in 0..20 {
            x.spawn((Transform::IDENTITY, Sprite::default()));
        }
        let living: Vec<Entity> = x.entities(None).expect("every entity").collect();
        for entity in living.into_iter().step_by(3) {
            if x.contains(entity) {
                x.despawn(entity).expect("alive");
            }
        }
        let version = x.version;
        x.rewind(&snap);
        assert_eq!(x.serial, serial);
        assert!(x.version > version.max(snap.version));
        let same = |x: &World, twin: &World, case: &str| {
            assert_eq!(x.hash(), twin.hash(), "{case}");
            assert!(same_snapshots(&snapshot(x), &snapshot(twin)), "{case}");
            assert_eq!(vacated(x), vacated(twin), "{case}");
        };
        same(&worlds.x, &worlds.twin, "rewound");
        let mut undone = 0;
        for step in 0..300 {
            let undo = step % 3 != 2;
            let go = |world: &mut World, log: &mut Log| {
                refused(if undo {
                    log.undo(world)
                } else {
                    log.redo(world)
                })
            };
            let ours = go(&mut worlds.x, &mut worlds.x_log);
            let theirs = go(&mut worlds.twin, &mut worlds.twin_log);
            assert_eq!(ours, theirs, "step {step}");
            undone += usize::from(undo && ours.is_none());
            same(&worlds.x, &worlds.twin, &format!("step {step}"));
        }
        assert!(undone > 0, "the log held edits to undo");
    }

    /// Same types registered in another order, so its ids name others; its own tables and slots.
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
        // Its first table has the ids of the fixture's first: other types.
        for count in 0..20 {
            world.spawn(Sealed(count));
        }
        world.spawn((Transform::IDENTITY, Sprite::default()));
        world
    }

    /// The fixture's types in its order, so ids match, but tables made in another order: some
    /// tables' types differ from the fixture's in their places.
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

    /// Restoring a snapshot over a world of other types, or snapshotting such a world into a kept
    /// one, yields the source's world, which then steps like it. So does a world of the same
    /// types with tables made in another order (another branch from the snapshot).
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
        // Snapshots on two branches from one world, each making other tables in one place.
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
        // One without tables over one with them: none found.
        let mut empty = World::seeded(1);
        let mut over = reversed();
        over.clone_from(&empty);
        assert_same(&over, &empty, "restored from a world without tables");
        for world in [&mut over, &mut empty] {
            world.spawn(Transform::IDENTITY);
        }
        assert_same(&over, &empty, "a step after the restore without tables");
    }

    /// `world`'s buffers' addresses and sizes: unchanged by a same-shape snapshot or restore.
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
            held.push((table.entities().as_ptr().addr(), table.capacity()));
            if let Some(column) = table.column::<Transform>(transform) {
                held.push((column.as_ptr().addr(), column.len()));
            }
        }
        // In the map's own order, which a copy keeping the map keeps.
        let keys = world.by_signature.keys();
        held.extend(keys.map(|key| (key.as_ptr().addr(), key.len())));
        held
    }

    /// While only values change, snapshotting again and restoring reuse each world's memory.
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
