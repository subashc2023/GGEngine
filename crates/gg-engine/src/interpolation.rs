//! Drawing between ticks. A copy holds where entities were as the latest tick started; each frame
//! draws them part of the way from there to where the tick left them. What each table row draws
//! from is found from the copy once per frame that ticked. A table's copy takes again only rows
//! whose transforms or parents changed, while no row joined or left it. So an entity despawned in
//! a tick vanishes up to a tick before it reaches where it stopped, and an Inspector set glides
//! there over a tick, as a drag does.

use std::iter;

use gg_core::math::{DVec3, Vec3};

use crate::{Entity, EntityId, Parent, Transform, World};

/// For a slot no row copied.
const NONE: (u32, u32) = (u32::MAX, u32::MAX);

/// What a table's rows draw from.
#[derive(Clone, Copy)]
enum Rows {
    /// Nothing: no transforms.
    None,
    /// The table's copy: its rows are the entities copied, with the parents copied, and each
    /// reset row's copy is its transform.
    Copied,
    /// What was found for each row, in `from` starting here.
    Found(u32),
}

/// A table's rows as the latest tick started: entities, transforms, and parents ([`Entity::DEAD`]
/// for a root's); and whether it had transforms then.
#[derive(Default)]
struct Copy {
    transformed: bool,
    entities: Vec<Entity>,
    transforms: Vec<Transform>,
    parents: Vec<Entity>,
}

impl Copy {
    fn clear(&mut self) {
        self.transformed = false;
        self.entities.clear();
        self.transforms.clear();
        self.parents.clear();
    }
}

/// Where the world's entities were as its latest tick started, and what each row of its tables
/// draws from. Kept from tick to tick: it allocates nothing once grown, while the world's tables
/// keep their sizes.
#[derive(Default)]
pub(crate) struct Past {
    /// The world copied: its serial, the change tick it had as copied (later changes are copied
    /// again), and how many interpolation resets it had made. `None` before the first copy.
    copied: Option<(u64, u64, u64)>,
    /// Each table's rows copied, by table index.
    tables: Vec<Copy>,
    /// Each slot's table and row in the copy, or [`NONE`]. Made only for a tick that left some
    /// table's rows other than its copy's (despawns and table changes move rows, spawns append),
    /// so a moved row still draws on from where it was.
    slots: Vec<(u32, u32)>,
    /// Whether what rows draw from has been found since the copy was taken.
    resolved: bool,
    starts: Vec<Rows>,
    /// What each row of the tables not drawing from the copy draws from, table by table.
    from: Vec<Transform>,
    /// The world serial and version `from` was found for; `None` while there's nothing to draw on
    /// from: before the first tick, and after a world restored or replaced in a tick.
    found: Option<(u64, u64)>,
    /// Poses rows draw as instead, by table and row, sorted: see [`instead`](Self::instead).
    instead: Vec<(u32, u32, Transform)>,
}

impl Past {
    /// Copies where `world`'s entities are, as a tick starts: every table with transforms, drawn
    /// or not, since a drawn child's ancestors can be in any. A table whose rows are those it
    /// copied before (none joined or left) copies only rows whose transforms or parents changed;
    /// another copies whole columns. Then moves the world's change tick on, so what the tick
    /// changes is marked as changed since the copy.
    pub(crate) fn capture(&mut self, world: &mut World) {
        let _zone = gg_core::zone!();
        let since = match self.copied {
            Some((serial, tick, _)) if serial == world.serial() => Some(tick),
            _ => None,
        };
        self.copied = Some((world.serial(), world.change_tick(), world.resets()));
        self.resolved = false;
        let (transform, parent) = (world.component::<Transform>(), world.component::<Parent>());
        self.tables.resize_with(world.tables().len(), Copy::default);
        for (table, copy) in world.tables().iter().zip(&mut self.tables) {
            let Some(transform) = transform.filter(|&id| table.has(id)) else {
                copy.clear();
                continue;
            };
            let transforms = table
                .column::<Transform>(transform)
                .expect("it has transforms");
            let parents = parent.and_then(|id| table.column::<Parent>(id));
            let entities = table.entities();
            let same = since.filter(|&tick| {
                copy.transformed
                    && copy.entities.len() == entities.len()
                    && !table.joined().since(tick)
            });
            if let Some(tick) = same {
                let marks = [
                    table.changed(transform),
                    parent.and_then(|id| table.changed(id)),
                ];
                for marks in marks
                    .into_iter()
                    .flatten()
                    .filter(|marks| marks.since(tick))
                {
                    for row in (0..entities.len()).filter(|&row| marks.row(row) > tick) {
                        copy.transforms[row] = transforms[row];
                        copy.parents[row] = parents.map_or(Entity::DEAD, |parents| parents[row].0);
                    }
                }
                continue;
            }
            copy.clear();
            copy.transformed = true;
            copy.entities.extend_from_slice(entities);
            copy.transforms.extend_from_slice(transforms);
            match parents {
                Some(parents) => copy.parents.extend(parents.iter().map(|parent| parent.0)),
                None => (copy.parents).extend(iter::repeat_n(Entity::DEAD, entities.len())),
            }
        }
        world.advance();
    }

    /// Finds what each row of `world`'s tables draws from, once per tick copied: where its entity
    /// was as the tick started, unless it jumped. A jumped entity's own transform draws from where
    /// it is, though its ancestors still carry it on their way. It jumped if:
    /// - it wasn't copied: spawned in the tick, given its transform there, or put back by an
    ///   undone delete;
    /// - its slot holds another entity than was copied;
    /// - its parent changed;
    /// - it was reset.
    ///
    /// A world restored or replaced in the tick draws all where it is. A table with no rows joined
    /// or left and no parent changes since its copy (most, at most ticks) draws from its copy,
    /// which takes the transform of each changed row that was reset. Other tables' rows are found
    /// one by one.
    pub(crate) fn resolve(&mut self, world: &World) {
        if self.resolved {
            return;
        }
        let _zone = gg_core::zone!();
        let Self {
            copied,
            tables,
            slots,
            resolved,
            starts,
            from,
            found,
            instead: _,
        } = self;
        *resolved = true;
        *found = None;
        starts.clear();
        from.clear();
        let Some((serial, tick, resets)) = *copied else {
            return;
        };
        let Some(transform) = world.component::<Transform>() else {
            return;
        };
        if serial != world.serial() {
            return;
        }
        let parent = world.component::<Parent>();
        let reset = world.resets() != resets;
        let mut mapped = false;
        for (index, table) in world.tables().iter().enumerate() {
            let Some(transforms) = table.column::<Transform>(transform) else {
                starts.push(Rows::None);
                continue;
            };
            let parents = parent.and_then(|id| table.column::<Parent>(id));
            let parented = parent.and_then(|id| table.changed(id));
            let entities = table.entities();
            let copy = tables.get_mut(index).filter(|copy| copy.transformed);
            if let Some(copy) = copy
                && copy.entities.len() == entities.len()
                && !table.joined().since(tick)
                && parented.is_none_or(|marks| !marks.since(tick))
            {
                let marks = table.changed(transform).expect("it has transforms");
                if reset && marks.since(tick) {
                    for row in (0..entities.len()).filter(|&row| marks.row(row) > tick) {
                        if world.reset_since(entities[row], resets) {
                            copy.transforms[row] = transforms[row];
                        }
                    }
                }
                starts.push(Rows::Copied);
                continue;
            }
            starts.push(Rows::Found(
                u32::try_from(from.len()).expect("fewer than 2^32 rows"),
            ));
            for (row, (&entity, now)) in entities.iter().zip(transforms).enumerate() {
                // The same row of its table's copy, else the row its slot was copied in, if that
                // was its entity's.
                let same = (tables.get(index))
                    .filter(|copy| copy.entities.get(row) == Some(&entity))
                    .map(|_| (index, row));
                let at = same.or_else(|| {
                    if !mapped {
                        map(tables, slots);
                        mapped = true;
                    }
                    let &(table, row) = slots.get(entity.slot())?;
                    let (table, row) = (table as usize, row as usize);
                    let copied = tables.get(table)?.entities.get(row)?;
                    (*copied == entity).then_some((table, row))
                });
                let parent = parents.map_or(Entity::DEAD, |parents| parents[row].0);
                let was = at.filter(|&(table, row)| {
                    tables[table].parents[row] == parent && !world.reset_since(entity, resets)
                });
                from.push(was.map_or(*now, |(table, row)| tables[table].transforms[row]));
            }
        }
        *found = Some(world.version());
    }

    /// Has frames of `world` as it is draw each entity of `poses` (by ID) as its pose says until
    /// given others: how a client draws other players' entities where the server's states had them.
    pub(crate) fn instead(&mut self, world: &World, poses: &[(EntityId, Transform)]) {
        self.instead.clear();
        for &(id, pose) in poses {
            if let Some((table, row)) = world.entity(id).and_then(|entity| world.table_row(entity))
            {
                self.instead.push((table, row, pose));
            }
        }
        self.instead
            .sort_unstable_by_key(|&(table, row, _)| (table, row));
    }

    /// The poses table `table`'s rows draw as instead, with table and row, by row.
    pub(crate) fn instead_of(&self, table: usize) -> &[(u32, u32, Transform)] {
        let table = u32::try_from(table).expect("fewer than 2^32 tables");
        let start = self.instead.partition_point(|&(of, ..)| of < table);
        let end = self.instead.partition_point(|&(of, ..)| of <= table);
        &self.instead[start..end]
    }

    /// What table `table`'s rows draw from, from its first row, if they draw between ticks and
    /// it's been found for the world as it is.
    pub(crate) fn from(&self, world: &World, table: usize) -> Option<&[Transform]> {
        if self.found != Some(world.version()) {
            return None;
        }
        match *self.starts.get(table)? {
            Rows::None => None,
            Rows::Copied => Some(&self.tables[table].transforms),
            Rows::Found(start) => Some(&self.from[start as usize..]),
        }
    }
}

/// Fills `slots` with the table and row of `tables`' copy that each slot's entity was copied in.
fn map(tables: &[Copy], slots: &mut Vec<(u32, u32)>) {
    slots.clear();
    for (index, copy) in (0..).zip(tables) {
        for (row, entity) in (0..).zip(&copy.entities) {
            let slot = entity.slot();
            if slots.len() <= slot {
                slots.resize(slot + 1, NONE);
            }
            slots[slot] = (index, row);
        }
    }
}

/// Whether an entity whose transform was `was` as a tick started, and is `now`, can draw between
/// them: `was` is finite and no scale changed sign.
fn continuous(was: &Transform, now: &Transform) -> bool {
    let finite = was.translation.is_finite() && was.rotation.is_finite() && was.scale.is_finite();
    finite && was.scale.is_negative_bitmask() == now.scale.is_negative_bitmask()
}

/// The transform `alpha` (0 to 1) of the way from `from` to `to`; or `to` if `from` isn't finite
/// or a scale changed sign (a mirrored sprite would shrink to nothing on its way). What's equal
/// in both comes out as `to` has it, bit for bit, so what didn't move draws as without
/// interpolating. Rotation takes the shorter way round, normalized: near enough to a slerp for a
/// tick's angle. Translation goes in `f64`, so an entity far from the origin moves as smoothly as
/// one at it.
pub(crate) fn between(from: &Transform, to: &Transform, alpha: f32) -> Transform {
    if !continuous(from, to) {
        return *to;
    }
    let blend = |from: Vec3, to: Vec3| Vec3::select(from.cmpeq(to), to, from.lerp(to, alpha));
    let (from_at, to_at) = (from.translation, to.translation);
    let moved = from_at.lerp(to_at, f64::from(alpha));
    let translation = DVec3::select(from_at.cmpeq(to_at), to_at, moved);
    let rotation = if from.rotation == to.rotation {
        to.rotation
    } else {
        from.rotation.lerp(to.rotation, alpha)
    };
    Transform {
        translation,
        rotation,
        scale: blend(from.scale, to.scale),
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec4;
    use gg_renderer::{Draws, Shaders, Sprite, Textures};

    use super::*;
    use crate::draw::{Scratch, draw_world};
    use crate::font::Fonts;
    use crate::placed::Placed;
    use crate::transform::from_origin;
    use crate::{Log, Operation};

    fn at(x: f32, z: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, z).as_dvec3(),
            ..Transform::IDENTITY
        }
    }

    fn sprite(n: f32) -> Sprite {
        Sprite {
            color: Vec4::splat(n),
            ..Sprite::default()
        }
    }

    fn moved(world: &mut World, entity: Entity, to: Transform) {
        *world.get_mut::<Transform>(entity).expect("alive") = to;
    }

    fn halfway(world: &World, past: &mut Past) -> Draws {
        past.resolve(world);
        let mut draws = Draws::default();
        let shaders = &mut Shaders::default();
        draw_world(
            world,
            past,
            &Placed::of(world, past, 0.5),
            (0.5, DVec3::ZERO),
            0,
            &mut draws,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        draws
    }

    /// Each way a tick breaks an entity's way on from where it was draws it where the tick left
    /// it, beside entities halfway on (one in a row a despawn moved it to): spawned; given a
    /// transform; in a despawned entity's slot; put back by an undone delete; moved between
    /// parents, or from none to one; reset; from a non-finite transform; mirrored.
    #[test]
    fn what_a_tick_breaks_draws_where_the_tick_left_it() {
        let mut world = World::default();
        let pivot = world.spawn(Transform::IDENTITY);
        let other = world.spawn(at(20.0, 0.0));
        let mover = world.spawn((at(0.0, 0.0), sprite(0.0)));
        let freed = world.spawn((at(5.0, 0.3), sprite(9.0)));
        let deleted = world.spawn((at(7.0, 0.4), sprite(4.0)));
        let rooted = world.spawn((at(9.0, 0.6), sprite(6.0)));
        let reset = world.spawn((at(11.0, 0.7), sprite(7.0)));
        let unfinite = world.spawn((at(f32::NAN, 0.8), sprite(8.0)));
        let mirrored = world.spawn((at(15.0, 0.85), sprite(10.0)));
        let given = world.spawn(sprite(2.0));
        let child = world.spawn((at(1.0, 0.5), sprite(5.0)));
        world.set_parent(child, Some(pivot)).expect("alive");
        let mut log = Log::default();
        let id = world.id(deleted).expect("alive");
        let delete = Operation::Delete { entity: id };
        log.apply(&mut world, delete).expect("deleted");
        let swapped = world.spawn((at(30.0, 0.9), sprite(11.0)));
        let mut past = Past::default();
        past.capture(&mut world);

        // The tick.
        moved(&mut world, mover, at(2.0, 0.0));
        moved(&mut world, swapped, at(32.0, 0.9));
        // The table's last row, `swapped`, takes its row; the next spawn takes its slot.
        world.despawn(freed).expect("alive");
        let reused = world.spawn((at(6.0, 0.3), sprite(3.0)));
        assert_eq!(reused.slot(), freed.slot(), "the slot freed");
        let spawned = world.spawn((at(3.0, 0.1), sprite(1.0)));
        assert!(log.undo(&mut world).expect("undone"));
        moved(&mut world, deleted, at(8.0, 0.4));
        world.insert(given, at(4.0, 0.2)).expect("alive");
        world.set_parent(child, Some(other)).expect("alive");
        moved(&mut world, child, at(2.0, 0.5));
        world.set_parent(rooted, Some(pivot)).expect("alive");
        moved(&mut world, rooted, at(10.0, 0.6));
        moved(&mut world, reset, at(13.0, 0.7));
        world.reset_interpolation(reset).expect("alive");
        moved(&mut world, unfinite, at(14.0, 0.8));
        let flipped = Transform {
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..at(15.0, 0.85)
        };
        moved(&mut world, mirrored, flipped);

        // Back to front by z, with entity IDs; the deleted one is put back with its own.
        let deleted = world.entity(id).expect("put back");
        let mut want = Draws::default();
        want.draw_scene();
        for (matrix, n, entity) in [
            (at(1.0, 0.0).matrix_from(DVec3::ZERO), 0.0, mover),
            (at(3.0, 0.1).matrix_from(DVec3::ZERO), 1.0, spawned),
            (at(4.0, 0.2).matrix_from(DVec3::ZERO), 2.0, given),
            (at(6.0, 0.3).matrix_from(DVec3::ZERO), 3.0, reused),
            (at(8.0, 0.4).matrix_from(DVec3::ZERO), 4.0, deleted),
            (
                from_origin(at(20.0, 0.0).matrix() * at(2.0, 0.5).matrix(), DVec3::ZERO),
                5.0,
                child,
            ),
            (
                from_origin(
                    Transform::IDENTITY.matrix() * at(10.0, 0.6).matrix(),
                    DVec3::ZERO,
                ),
                6.0,
                rooted,
            ),
            (at(13.0, 0.7).matrix_from(DVec3::ZERO), 7.0, reset),
            (at(14.0, 0.8).matrix_from(DVec3::ZERO), 8.0, unfinite),
            (flipped.matrix_from(DVec3::ZERO), 10.0, mirrored),
            (at(31.0, 0.9).matrix_from(DVec3::ZERO), 11.0, swapped),
        ] {
            let id = world.id(entity).expect("alive").get();
            want.extend_sprites(iter::once((matrix, sprite(n), id)));
        }
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// An entity given a pose to draw as instead (as a client draws another player's) draws as
    /// the pose says, not between ticks, carrying its child, beside one drawn halfway.
    #[test]
    fn an_entity_given_a_pose_draws_as_it_says_carrying_its_children() {
        let mut world = World::default();
        let theirs = world.spawn((at(0.0, 0.1), sprite(1.0)));
        let child = world.spawn((at(1.0, 0.05), sprite(2.0)));
        world.set_parent(child, Some(theirs)).expect("alive");
        let ours = world.spawn((at(0.0, 0.3), sprite(3.0)));
        let mut past = Past::default();
        past.capture(&mut world);
        moved(&mut world, theirs, at(2.0, 0.1));
        moved(&mut world, ours, at(2.0, 0.3));
        let id = |entity: Entity| world.id(entity).expect("alive");
        let pose = at(10.0, 0.1);
        past.instead(&world, &[(id(theirs), pose)]);
        let mut want = Draws::default();
        want.draw_scene();
        for (matrix, n, entity) in [
            (pose.matrix_from(DVec3::ZERO), 1.0, theirs),
            (
                from_origin(pose.matrix() * at(1.0, 0.05).matrix(), DVec3::ZERO),
                2.0,
                child,
            ),
            (at(1.0, 0.3).matrix_from(DVec3::ZERO), 3.0, ours),
        ] {
            want.extend_sprites(iter::once((matrix, sprite(n), id(entity).get())));
        }
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// A child moved to another parent, which moves no row, draws where the tick left it, beside a
    /// sibling that still draws on its way.
    #[test]
    fn a_child_given_another_parent_draws_where_the_tick_left_it() {
        let mut world = World::default();
        let first = world.spawn(Transform::IDENTITY);
        let second = world.spawn(at(20.0, 0.0));
        let [moved_away, sibling] = [(1.0, 0.5, 5.0), (0.0, 0.6, 6.0)].map(|(x, z, n)| {
            let child = world.spawn((at(x, z), sprite(n)));
            world.set_parent(child, Some(first)).expect("alive");
            child
        });
        let mut past = Past::default();
        past.capture(&mut world);
        world.set_parent(moved_away, Some(second)).expect("alive");
        moved(&mut world, moved_away, at(2.0, 0.5));
        moved(&mut world, sibling, at(2.0, 0.6));
        let mut want = Draws::default();
        want.draw_scene();
        let id = |entity: Entity| world.id(entity).expect("alive").get();
        let carried = at(20.0, 0.0).matrix() * at(2.0, 0.5).matrix();
        let halfway_there = Transform::IDENTITY.matrix() * at(1.0, 0.6).matrix();
        let drawn = [
            (
                from_origin(carried, DVec3::ZERO),
                sprite(5.0),
                id(moved_away),
            ),
            (
                from_origin(halfway_there, DVec3::ZERO),
                sprite(6.0),
                id(sibling),
            ),
        ];
        want.extend_sprites(drawn.into_iter());
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// A world restored from a snapshot in a tick, or replaced, draws all of it where the tick
    /// left it, though its handles and rows are as before.
    #[test]
    fn a_world_restored_or_replaced_draws_where_the_tick_left_it() {
        let mut world = World::seeded(1);
        let mover = world.spawn((at(0.0, 0.0), sprite(0.0)));
        let snapshot = world.clone();
        let mut past = Past::default();
        past.capture(&mut world);
        world.clone_from(&snapshot);
        moved(&mut world, mover, at(2.0, 0.0));
        let id = world.id(mover).expect("alive").get();
        let mut want = Draws::default();
        want.draw_scene();
        want.extend_sprites(iter::once((
            at(2.0, 0.0).matrix_from(DVec3::ZERO),
            sprite(0.0),
            id,
        )));
        assert_eq!(halfway(&world, &mut past), want, "restored");

        past.capture(&mut world);
        let mut other = World::seeded(1);
        let same = other.spawn((at(4.0, 0.0), sprite(0.0)));
        assert_eq!(same, mover, "the same handle");
        world = other;
        let id = world.id(same).expect("alive").get();
        let mut want = Draws::default();
        want.draw_scene();
        want.extend_sprites(iter::once((
            at(4.0, 0.0).matrix_from(DVec3::ZERO),
            sprite(0.0),
            id,
        )));
        assert_eq!(halfway(&world, &mut past), want, "replaced");
    }

    fn held(past: &Past) -> Vec<(usize, usize)> {
        let Past {
            copied: _,
            tables,
            slots,
            resolved: _,
            starts,
            from,
            found: _,
            instead: _,
        } = past;
        let mut held = vec![
            (tables.as_ptr().addr(), tables.capacity()),
            (slots.as_ptr().addr(), slots.capacity()),
            (starts.as_ptr().addr(), starts.capacity()),
            (from.as_ptr().addr(), from.capacity()),
        ];
        for copy in tables {
            held.push((copy.entities.as_ptr().addr(), copy.entities.capacity()));
            held.push((copy.transforms.as_ptr().addr(), copy.transforms.capacity()));
            held.push((copy.parents.as_ptr().addr(), copy.parents.capacity()));
        }
        held
    }

    /// Copying and finding, through ticks that move every entity, despawn one (moving another's
    /// row) and spawn into its slot, take no more memory once grown.
    #[test]
    fn capturing_and_resolving_again_take_no_more_memory() {
        let mut world = World::default();
        let pivot = world.spawn(Transform::IDENTITY);
        let mut entities: Vec<Entity> = (0..100)
            .map(|i| world.spawn((at(i as f32, 0.0), sprite(0.0))))
            .collect();
        let child = world.spawn((Transform::IDENTITY, sprite(1.0)));
        world.set_parent(child, Some(pivot)).expect("alive");
        let mut past = Past::default();
        let mut grown = None;
        for round in 0..4 {
            past.capture(&mut world);
            for transform in world.query_mut::<&mut Transform>() {
                transform.translation.x += 1.0;
            }
            let gone = entities.swap_remove(round * 7);
            world.despawn(gone).expect("alive");
            entities.push(world.spawn((at(0.0, 0.0), sprite(0.0))));
            // Emptied, keeping its memory, so only this round's map fills it.
            past.slots.clear();
            halfway(&world, &mut past);
            assert!(!past.slots.is_empty(), "round {round} moved a row");
            let held = held(&past);
            assert_eq!(
                *grown.get_or_insert_with(|| held.clone()),
                held,
                "round {round}"
            );
        }
    }

    /// A kept past, copying only changes, draws each row as a past copied afresh each tick does,
    /// bit for bit, through ticks that move entities, change a parent, reset, change table,
    /// spawn, despawn, and change nothing.
    #[test]
    fn a_kept_past_draws_from_what_a_fresh_one_does() {
        let mut world = World::default();
        let entities: Vec<Entity> = (0..8)
            .map(|i| world.spawn((at(i as f32, 0.0), sprite(0.0))))
            .collect();
        let pivot = world.spawn(at(-1.0, 0.0));
        world
            .set_parent(entities[0], Some(pivot))
            .expect("no cycle");
        let mut kept = Past::default();
        type Tick = fn(&mut World, &[Entity], Entity);
        let ticks: [(&str, Tick); 7] = [
            ("some moved", |world, entities, _| {
                moved(world, entities[2], at(9.0, 0.0))
            }),
            ("nothing", |_, _, _| {}),
            ("a parent changed", |world, entities, pivot| {
                world
                    .set_parent(entities[3], Some(pivot))
                    .expect("no cycle");
            }),
            ("one reset", |world, entities, _| {
                moved(world, entities[4], at(20.0, 0.0));
                world.reset_interpolation(entities[4]).expect("alive");
            }),
            ("one to another table", |world, entities, _| {
                world
                    .insert(entities[5], gg_renderer::Circle::default())
                    .expect("alive");
                moved(world, entities[5], at(30.0, 0.0));
            }),
            ("a despawn", |world, entities, _| {
                world.despawn(entities[6]).expect("alive")
            }),
            ("a spawn", |world, _, _| {
                world.spawn((at(40.0, 0.0), sprite(0.0)));
            }),
        ];
        for (what, tick) in ticks {
            // As a tick starts: a copy afresh, and the kept one's of changes since it last copied.
            let mut fresh = Past::default();
            fresh.capture(&mut world);
            kept.capture(&mut world);
            tick(&mut world, &entities, pivot);
            fresh.resolve(&world);
            kept.resolve(&world);
            for index in 0..world.tables().len() {
                let bits = |past: &Past| format!("{:?}", past.from(&world, index));
                assert_eq!(bits(&kept), bits(&fresh), "after {what}, table {index}");
            }
        }
    }
}
