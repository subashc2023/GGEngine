//! Drawing between ticks: a copy of where the world's entities were as its latest tick started,
//! which each frame draws them on from, part of the way to where the tick left them, and what each
//! row of the world's tables draws from, found from the copy once a frame that ticked. Both follow
//! what changed: a table's copy takes again only the rows whose transforms or parents changed
//! since, while no row joined or left it, and its rows draw from it as they are while none did.

use std::iter;

use gg_core::math::Vec3;

use crate::{Entity, EntityId, Parent, Transform, World};

/// For a slot that no row copied.
const NONE: (u32, u32) = (u32::MAX, u32::MAX);

/// What a table's rows draw from.
#[derive(Clone, Copy)]
enum Rows {
    /// Nothing: the table has no transforms.
    None,
    /// The table's copy: its rows are the entities copied, with the parents copied, and the copy
    /// of each reset is its transform.
    Copied,
    /// What was found for each of its rows, in `from`, from where they start.
    Found(u32),
}

/// A table's rows as the latest tick started: their entities, their transforms, and their
/// parents, [`Entity::DEAD`] for a root's; and whether it had transforms then.
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
/// draws from: kept from tick to tick, so that it allocates nothing once it's grown, while the
/// world's tables keep their sizes.
#[derive(Default)]
pub(crate) struct Past {
    /// The world copied, by its serial, the change tick it had as it was copied, after which its
    /// changes are copied again, and how many resets of interpolation it had made then; `None`
    /// before the first copy.
    copied: Option<(u64, u64, u64)>,
    /// Each table's rows copied, by the table's index.
    tables: Vec<Copy>,
    /// Each slot's table and row in the copy, or [`NONE`]: made only for a tick that left some
    /// table's rows other than its copy's, as a despawn or a change of table moves rows, and a
    /// spawn appends one, so that a row moved still draws on from where it was.
    slots: Vec<(u32, u32)>,
    /// Whether what rows draw from has been found from the copy since it was taken.
    resolved: bool,
    /// What each table's rows draw from, by the table's index.
    starts: Vec<Rows>,
    /// What each row of the tables whose rows aren't the ones copied draws from, table by table.
    from: Vec<Transform>,
    /// The world, by its serial, and its version, that `from` was found for; `None` while there's
    /// nothing to draw on from, as before the first tick and after a world restored or replaced in
    /// a tick.
    found: Option<(u64, u64)>,
    /// The poses rows draw as in place of where they'd draw, by their tables and rows, sorted, as
    /// [`instead`](Self::instead) has them.
    instead: Vec<(u32, u32, Transform)>,
}

impl Past {
    /// Copies where `world`'s entities are, as a tick starts: every table with transforms, drawn
    /// or not, since a drawn child's ancestors can be in any; of a table whose rows are those it
    /// copied before, as no row joined or left it since, the rows whose transforms or parents
    /// changed alone, and of another, all of it, a few copies of whole columns. Then moves the
    /// world's change tick on, so that what the tick changes is marked as changed since the copy.
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

    /// Finds what each row of `world`'s tables draws from, once a tick has been copied since it
    /// last did: where its entity was as the tick started, unless it jumped. It jumped, and its own
    /// transform draws from where it is, though its ancestors still carry it on their way, if it
    /// wasn't copied, as an entity spawned in the tick, given its transform there, or put back by
    /// an undone delete wasn't; if its slot holds another entity than was copied; if its parent
    /// changed; or if it was reset. A world restored or replaced in the tick draws where it is, all
    /// of it. A table that no row joined or left since its copy, and whose parents didn't change,
    /// as most are at most ticks, draws from its copy, which takes the transform of each of its
    /// rows that changed and was reset; the others' rows are found one by one.
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
                // The same row of its table's copy, or else the row its slot was copied in, if it
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

    /// Has the frames draw each entity of `poses`, by its ID, of `world` as it is, as its pose
    /// says, in place of where it would draw, as a client draws the other players' entities where
    /// the server's states had them: until they're given others, which a frame of `world` as it
    /// is takes.
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

    /// The poses table `table`'s rows draw as in place of where they'd draw, each with its table
    /// and row, by row.
    pub(crate) fn instead_of(&self, table: usize) -> &[(u32, u32, Transform)] {
        let table = u32::try_from(table).expect("fewer than 2^32 tables");
        let start = self.instead.partition_point(|&(of, ..)| of < table);
        let end = self.instead.partition_point(|&(of, ..)| of <= table);
        &self.instead[start..end]
    }

    /// What the rows of `world`'s table `table` draw from, from its first row on, if they draw
    /// between ticks: once what they draw from has been found for the world as it is.
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

/// Fills `slots` with the table and row of the copy each slot's entity was copied in, by
/// `tables`, the copy's.
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

/// Whether an entity whose transform was `was` as a tick started, and is `now`, can draw on its way
/// between them: `was` is finite, and no scale has changed its sign.
fn continuous(was: &Transform, now: &Transform) -> bool {
    let finite = was.translation.is_finite() && was.rotation.is_finite() && was.scale.is_finite();
    finite && was.scale.is_negative_bitmask() == now.scale.is_negative_bitmask()
}

/// The transform `alpha`, from 0 to 1, of the way from `from` to `to`; or `to`, where `from` isn't
/// finite, or a scale changed its sign, as a sprite mirrored does, which would otherwise shrink to
/// nothing on its way. What's equal in both comes out as `to` has it, bit for bit, so that what
/// didn't move draws as it would without interpolating. A rotation takes the shorter way round,
/// normalized, which is near enough to a slerp for the angle a tick turns.
pub(crate) fn between(from: &Transform, to: &Transform, alpha: f32) -> Transform {
    if !continuous(from, to) {
        return *to;
    }
    let blend = |from: Vec3, to: Vec3| Vec3::select(from.cmpeq(to), to, from.lerp(to, alpha));
    let rotation = if from.rotation == to.rotation {
        to.rotation
    } else {
        from.rotation.lerp(to.rotation, alpha)
    };
    Transform {
        translation: blend(from.translation, to.translation),
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
    use crate::{Log, Operation};

    /// A transform at `x` and `z`, which tell the cases apart.
    fn at(x: f32, z: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, z),
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

    /// What `world` draws halfway on from `past`, found for it.
    fn halfway(world: &World, past: &mut Past) -> Draws {
        past.resolve(world);
        let mut draws = Draws::default();
        let shaders = &mut Shaders::default();
        draw_world(
            world,
            past,
            &Placed::of(world, past, 0.5),
            0.5,
            &mut draws,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        draws
    }

    /// Each way a tick breaks an entity's way on from where it was draws it where the tick left
    /// it, beside entities halfway on their way, one of them in a row a despawn moved it to:
    /// spawned; given a transform; in the slot of an entity despawned; put back by an undone
    /// delete; moved from one parent to another, and from none to one; reset; from a transform
    /// that isn't finite; and mirrored.
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
        // The table's last row, `swapped`, takes its row, and the next entity spawned its slot.
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

        // Back to front by z, each with its entity's ID, the deleted one's put back with its own.
        let deleted = world.entity(id).expect("put back");
        let mut want = Draws::default();
        want.draw_scene();
        for (matrix, n, entity) in [
            (at(1.0, 0.0).matrix(), 0.0, mover),
            (at(3.0, 0.1).matrix(), 1.0, spawned),
            (at(4.0, 0.2).matrix(), 2.0, given),
            (at(6.0, 0.3).matrix(), 3.0, reused),
            (at(8.0, 0.4).matrix(), 4.0, deleted),
            (at(20.0, 0.0).matrix() * at(2.0, 0.5).matrix(), 5.0, child),
            (
                Transform::IDENTITY.matrix() * at(10.0, 0.6).matrix(),
                6.0,
                rooted,
            ),
            (at(13.0, 0.7).matrix(), 7.0, reset),
            (at(14.0, 0.8).matrix(), 8.0, unfinite),
            (flipped.matrix(), 10.0, mirrored),
            (at(31.0, 0.9).matrix(), 11.0, swapped),
        ] {
            let id = world.id(entity).expect("alive").get();
            want.extend_sprites(iter::once((matrix, sprite(n), id)));
        }
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// An entity given a pose to draw as instead, as a client draws another player's, draws as the
    /// pose says, rather than on its way between ticks, carrying its child, beside an entity that
    /// draws halfway on its way.
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
            (pose.matrix(), 1.0, theirs),
            (pose.matrix() * at(1.0, 0.05).matrix(), 2.0, child),
            (at(1.0, 0.3).matrix(), 3.0, ours),
        ] {
            want.extend_sprites(iter::once((matrix, sprite(n), id(entity).get())));
        }
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// A child moved from one parent to another, which moves no row, draws its own transform where
    /// the tick left it, beside a sibling whose rows stay its own, which still draws on its way.
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
            (carried, sprite(5.0), id(moved_away)),
            (halfway_there, sprite(6.0), id(sibling)),
        ];
        want.extend_sprites(drawn.into_iter());
        assert_eq!(halfway(&world, &mut past), want);
    }

    /// A world restored from a snapshot in a tick, or replaced by another, draws where the tick
    /// left it, all of it, though its handles and rows are the same as before.
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
        want.extend_sprites(iter::once((at(2.0, 0.0).matrix(), sprite(0.0), id)));
        assert_eq!(halfway(&world, &mut past), want, "restored");

        past.capture(&mut world);
        let mut other = World::seeded(1);
        let same = other.spawn((at(4.0, 0.0), sprite(0.0)));
        assert_eq!(same, mover, "the same handle");
        world = other;
        let id = world.id(same).expect("alive").get();
        let mut want = Draws::default();
        want.draw_scene();
        want.extend_sprites(iter::once((at(4.0, 0.0).matrix(), sprite(0.0), id)));
        assert_eq!(halfway(&world, &mut past), want, "replaced");
    }

    /// Where a past's buffers are, and how big, which copying and finding again without taking
    /// more memory leaves as they were.
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

    /// Copying a world, and finding what it draws from, again, through ticks that move every
    /// entity, despawn one, which moves another's row, and spawn one into its slot, take no more
    /// memory once they've grown.
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
            // Emptied, keeping its memory, so that only this round's map fills it.
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

    /// A past kept from tick to tick, which copies again only what changed, draws each row from
    /// what a past copying the world afresh as each tick starts does, bit for bit, through ticks
    /// that move some entities, change a parent, reset one, move one to another table, spawn and
    /// despawn, and change nothing.
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
            // As a tick starts, a copy afresh, and the kept one's, of the changes since it last
            // copied.
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
