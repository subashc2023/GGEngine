//! Drawing between ticks: a copy of where the world's entities were as its latest tick started,
//! which each frame draws them on from, part of the way to where the tick left them, and what each
//! row of the world's tables draws from, found from the copy once a frame that ticked.

use std::iter;

use gg_core::math::Vec3;

use crate::{Entity, Parent, Transform, World};

/// For a slot that no row copied.
const NONE: u32 = u32::MAX;

/// What a table's rows draw from.
#[derive(Clone, Copy)]
enum Rows {
    /// Nothing: the table has no transforms.
    None,
    /// The copy of its rows, from where they start: they're the entities copied, with the parents
    /// copied, and the copy of each reset is its transform.
    Copied(u32),
    /// What was found for each of its rows, in `from`, from where they start.
    Found(u32),
}

/// Where the world's entities were as its latest tick started, and what each row of its tables
/// draws from: kept from tick to tick, so that it allocates nothing once it's grown, while the
/// world's tables keep their sizes.
#[derive(Default)]
pub(crate) struct Past {
    /// The world copied, by its serial, and how many resets of interpolation it had made then;
    /// `None` before the first copy.
    copied: Option<(u64, u64)>,
    /// Each table's rows in the copy, by the table's index, `start..end`; `None` for a table
    /// without transforms.
    tables: Vec<Option<(u32, u32)>>,
    /// Each row copied: its entity, its transform, and its parent, [`Entity::DEAD`] for a root.
    entities: Vec<Entity>,
    transforms: Vec<Transform>,
    parents: Vec<Entity>,
    /// Each slot's row in the copy, or [`NONE`]: made only for a tick that left some row where its
    /// copy isn't, as a despawn or a change of table moves rows, and a spawn appends one, so that a
    /// row moved still draws on from where it was.
    slots: Vec<u32>,
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
}

impl Past {
    /// Copies where `world`'s entities are, as a tick starts: every table with transforms, drawn
    /// or not, since a drawn child's ancestors can be in any. It copies a table at a time, rather
    /// than by entity, so that it's a few copies of whole columns.
    pub(crate) fn capture(&mut self, world: &World) {
        let _zone = gg_core::zone!();
        self.copied = Some((world.version().0, world.resets()));
        self.resolved = false;
        self.tables.clear();
        self.entities.clear();
        self.transforms.clear();
        self.parents.clear();
        let Some(transform) = world.component::<Transform>() else {
            return;
        };
        let parent = world.component::<Parent>();
        for table in world.tables() {
            let Some(transforms) = table.column::<Transform>(transform) else {
                self.tables.push(None);
                continue;
            };
            let start = self.entities.len();
            self.entities.extend_from_slice(table.entities());
            self.transforms.extend_from_slice(transforms);
            match parent.and_then(|id| table.column::<Parent>(id)) {
                Some(parents) => self.parents.extend(parents.iter().map(|parent| parent.0)),
                None => self
                    .parents
                    .extend(iter::repeat_n(Entity::DEAD, transforms.len())),
            }
            let rows = [start, self.entities.len()];
            let [start, end] = rows.map(|at| u32::try_from(at).expect("fewer than 2^32 rows"));
            self.tables.push(Some((start, end)));
        }
    }

    /// Finds what each row of `world`'s tables draws from, once a tick has been copied since it
    /// last did: where its entity was as the tick started, unless it jumped. It jumped, and its own
    /// transform draws from where it is, though its ancestors still carry it on their way, if it
    /// wasn't copied, as an entity spawned in the tick, given its transform there, or put back by
    /// an undone delete wasn't; if its slot holds another entity than was copied; if its parent
    /// changed; or if it was reset. A world restored or replaced in the tick draws where it is, all
    /// of it. A table whose rows are the entities copied, with the parents copied, as most are at
    /// most ticks, draws from its copy, which takes each reset row's transform; the others' rows
    /// are found one by one.
    pub(crate) fn resolve(&mut self, world: &World) {
        if self.resolved {
            return;
        }
        let _zone = gg_core::zone!();
        let Self {
            copied,
            tables,
            entities,
            transforms: copies,
            parents: copied_parents,
            slots,
            resolved,
            starts,
            from,
            found,
        } = self;
        *resolved = true;
        *found = None;
        starts.clear();
        from.clear();
        let Some((serial, resets)) = *copied else {
            return;
        };
        let Some(transform) = world.component::<Transform>() else {
            return;
        };
        if serial != world.version().0 {
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
            let rows = tables.get(index).copied().flatten();
            if let Some((start, end)) = rows {
                let copy = start as usize..end as usize;
                // A table's parents are copied with its index while the world is the same, which
                // only adds tables.
                let same = entities[copy.clone()] == *table.entities()
                    && parents.is_none_or(|parents| {
                        let now = parents.iter().map(|parent| parent.0);
                        now.eq(copied_parents[copy.clone()].iter().copied())
                    });
                if same {
                    let rows = table.entities().iter().zip(transforms);
                    for (copied, (&entity, now)) in copies[copy].iter_mut().zip(rows) {
                        if reset && world.reset_since(entity, resets) {
                            *copied = *now;
                        }
                    }
                    starts.push(Rows::Copied(start));
                    continue;
                }
            }
            starts.push(Rows::Found(
                u32::try_from(from.len()).expect("fewer than 2^32 rows"),
            ));
            for (row, (&entity, now)) in table.entities().iter().zip(transforms).enumerate() {
                // The same row, or else the row its slot was copied in, if it was its entity's.
                let same = rows.and_then(|(start, end)| {
                    let at = start as usize + row;
                    (at < end as usize && entities[at] == entity).then_some(at)
                });
                let at = same.or_else(|| {
                    if !mapped {
                        map(entities, slots);
                        mapped = true;
                    }
                    let at = slots.get(entity.slot()).copied().filter(|&at| at != NONE);
                    at.map(|at| at as usize)
                        .filter(|&at| entities[at] == entity)
                });
                let parent = parents.map_or(Entity::DEAD, |parents| parents[row].0);
                let was = at.filter(|&at| {
                    copied_parents[at] == parent && !world.reset_since(entity, resets)
                });
                from.push(was.map_or(*now, |at| copies[at]));
            }
        }
        *found = Some(world.version());
    }

    /// What the rows of `world`'s table `table` draw from, from its first row on, if they draw
    /// between ticks: once what they draw from has been found for the world as it is.
    pub(crate) fn from(&self, world: &World, table: usize) -> Option<&[Transform]> {
        if self.found != Some(world.version()) {
            return None;
        }
        match *self.starts.get(table)? {
            Rows::None => None,
            Rows::Copied(start) => Some(&self.transforms[start as usize..]),
            Rows::Found(start) => Some(&self.from[start as usize..]),
        }
    }
}

/// Fills `slots` with the row of the copy each slot's entity was copied in, by `entities`, the
/// copy's.
fn map(entities: &[Entity], slots: &mut Vec<u32>) {
    slots.clear();
    for (at, entity) in entities.iter().enumerate() {
        let slot = entity.slot();
        if slots.len() <= slot {
            slots.resize(slot + 1, NONE);
        }
        slots[slot] = at as u32;
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
        past.capture(&world);

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
        past.capture(&world);
        world.set_parent(moved_away, Some(second)).expect("alive");
        moved(&mut world, moved_away, at(2.0, 0.5));
        moved(&mut world, sibling, at(2.0, 0.6));
        let mut want = Draws::default();
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
        past.capture(&world);
        world.clone_from(&snapshot);
        moved(&mut world, mover, at(2.0, 0.0));
        let id = world.id(mover).expect("alive").get();
        let mut want = Draws::default();
        want.extend_sprites(iter::once((at(2.0, 0.0).matrix(), sprite(0.0), id)));
        assert_eq!(halfway(&world, &mut past), want, "restored");

        past.capture(&world);
        let mut other = World::seeded(1);
        let same = other.spawn((at(4.0, 0.0), sprite(0.0)));
        assert_eq!(same, mover, "the same handle");
        world = other;
        let id = world.id(same).expect("alive").get();
        let mut want = Draws::default();
        want.extend_sprites(iter::once((at(4.0, 0.0).matrix(), sprite(0.0), id)));
        assert_eq!(halfway(&world, &mut past), want, "replaced");
    }

    /// Where a past's buffers are, and how big, which copying and finding again without taking
    /// more memory leaves as they were.
    fn held(past: &Past) -> [(usize, usize); 7] {
        let Past {
            copied: _,
            tables,
            entities,
            transforms,
            parents,
            slots,
            resolved: _,
            starts,
            from,
            found: _,
        } = past;
        [
            (tables.as_ptr().addr(), tables.capacity()),
            (entities.as_ptr().addr(), entities.capacity()),
            (transforms.as_ptr().addr(), transforms.capacity()),
            (parents.as_ptr().addr(), parents.capacity()),
            (slots.as_ptr().addr(), slots.capacity()),
            (starts.as_ptr().addr(), starts.capacity()),
            (from.as_ptr().addr(), from.capacity()),
        ]
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
            past.capture(&world);
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
            assert_eq!(*grown.get_or_insert(held), held, "round {round}");
        }
    }
}
