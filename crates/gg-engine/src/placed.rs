//! Where the world's entities are: each one's matrix to world space (its transform's, then its
//! ancestors' in turn) and its rotation. They're kept from frame to frame as the latest tick left
//! them, and placed again only where a transform or parent changed, with what it carries. Also
//! where entities on their way between ticks draw this frame, which follows only what the latest
//! tick moved.

use std::mem;

use gg_core::math::{DAffine3, DVec3, Quat};

use crate::ecs::ComponentId;
use crate::interpolation::{self, Past};
use crate::{Entity, Parent, Transform, World};

/// Where the world's entities are, by slot. Kept from frame to frame: it allocates nothing once
/// grown.
#[derive(Default)]
pub(crate) struct Placed {
    /// The world placed: its serial and the change tick it had then, after which it places again.
    /// `None` before the first placing.
    seen: Option<(u64, u64)>,
    /// By slot: the entity placed there, [`Entity::DEAD`] for none.
    entities: Vec<Entity>,
    /// By slot: where its entity is as the latest tick left it. `None` is the origin, unturned, at
    /// its own size: a root without a transform, and what it carries without one.
    kept: Vec<Option<Place>>,
    /// By slot: the last pass that found its entity changed, and the last that placed it or drew
    /// it on its way.
    found: Vec<u32>,
    placed: Vec<u32>,
    /// The pass under way: each placing and each drawing is one.
    pass: u32,
    list: Vec<Entity>,
    changed: Vec<Entity>,
    /// By slot: where its entity draws on its way between ticks, in the pass `drawing` numbers, if
    /// one has drawn since the latest placing; 0 if none has.
    drawn: Vec<Option<Place>>,
    drawing: u32,
    moving: Vec<Entity>,
    stack: Vec<(Entity, Option<Place>)>,
}

/// Where an entity is: its matrix to the world's space, in `f64` (its transform's, then each
/// ancestor's in turn; an ancestor without a transform carries its children as if at the origin,
/// unturned, at its own size), and its rotation, its own turned by each ancestor's, scales aside.
/// So a mirrored ancestor mirrors where it is but not which way it faces, as a viewpoint and a
/// light face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub(crate) matrix: DAffine3,
    pub(crate) rotation: Quat,
}

impl Place {
    const ORIGIN: Self = Self {
        matrix: DAffine3::IDENTITY,
        rotation: Quat::IDENTITY,
    };

    /// Where an entity with transform `local`, if any, is under a parent at `parent`: parent's
    /// matrix times its own, likewise rotation, from the root down, so a parent's place is found
    /// once for all its children. Nothing is multiplied for what's at the origin, which keeps
    /// each value as the transform has it.
    fn under(parent: Option<Place>, local: Option<&Transform>) -> Option<Place> {
        let own = local.map(|local| Place {
            matrix: local.matrix(),
            rotation: local.rotation,
        });
        match (parent, own) {
            (Some(parent), Some(own)) => Some(Place {
                matrix: parent.matrix * own.matrix,
                rotation: parent.rotation * own.rotation,
            }),
            (parent, own) => own.or(parent),
        }
    }

    /// Its position, where a sprite of its matrix draws, and its rotation, normalized: each product
    /// of unit quaternions rounds a little off a unit.
    pub(crate) fn pose(&self) -> (DVec3, Quat) {
        (self.matrix.translation, self.rotation.normalize())
    }
}

impl Placed {
    /// Places `world`'s entities that changed since last placed, or all in another world: each
    /// whose transform or parent changed or that joined a table (as spawning or giving a
    /// transform or parent does), and what each carries, from the topmost changed. Cost follows
    /// what changed and what it carries, apart from a look over each table's marks.
    pub(crate) fn place(&mut self, world: &World) {
        let _zone = gg_core::zone!();
        let since = match self.seen {
            Some((serial, tick)) if serial == world.serial() => Some(tick),
            _ => None,
        };
        self.seen = Some((world.serial(), world.change_tick()));
        self.next_pass(world);
        self.drawing = 0;
        self.changed.clear();
        let mut list = mem::take(&mut self.list);
        list.clear();
        let (transform, parent) = (world.component::<Transform>(), world.component::<Parent>());
        for table in world.tables() {
            let entities = table.entities();
            let Some(since) = since else {
                list.extend_from_slice(entities);
                continue;
            };
            let marks = [
                Some(table.joined()),
                transform.and_then(|id| table.changed(id)),
                parent.and_then(|id| table.changed(id)),
            ];
            for marks in marks.into_iter().flatten() {
                if marks.since(since) {
                    let rows = (0..entities.len()).filter(|&row| marks.row(row) > since);
                    list.extend(rows.map(|row| entities[row]));
                }
            }
        }
        self.walk_from_tops(world, &list, None);
        self.list = list;
    }

    /// Finds where entities on their way between ticks draw this frame, with what they carry:
    /// each whose transform changed since change tick `started` and that `past` has draw `alpha`
    /// of the way on from somewhere, or draw as it says instead. Roots carrying nothing, which
    /// drawing blends itself, are left out, except in tables with component `held`, whose places
    /// are held elsewhere, as the renderer's scene holds a mesh's. At `alpha` 1, all but what
    /// `past` has draw instead is left out. Called once a frame, after the placing that follows
    /// its ticks.
    pub(crate) fn draw(
        &mut self,
        world: &World,
        past: &Past,
        alpha: f32,
        started: u64,
        held: Option<ComponentId>,
    ) {
        self.moving.clear();
        let pass = self.next_pass(world);
        self.drawing = pass;
        let mut list = mem::take(&mut self.list);
        list.clear();
        let parent = world.component::<Parent>();
        if let Some(transform) = world.component::<Transform>() {
            for (index, table) in world.tables().iter().enumerate() {
                let Some(marks) = table.changed(transform) else {
                    continue;
                };
                let entities = table.entities();
                let child = parent.is_some_and(|id| table.has(id));
                let all = child || held.is_some_and(|id| table.has(id));
                // What a root carries draws as the root does; one carrying nothing blends alone.
                let counts =
                    |entity: Entity| all || world.children(entity).expect("alive").next().is_some();
                let instead = past.instead_of(index).iter();
                let instead = instead.map(|&(_, row, _)| entities[row as usize]);
                list.extend(instead.filter(|&entity| counts(entity)));
                let from = (alpha < 1.0).then(|| past.from(world, index)).flatten();
                if from.is_some() && marks.since(started.saturating_sub(1)) {
                    let rows = (0..entities.len()).filter(|&row| marks.row(row) >= started);
                    list.extend(
                        rows.map(|row| entities[row])
                            .filter(|&entity| counts(entity)),
                    );
                }
            }
        }
        self.walk_from_tops(world, &list, Some((past, alpha, started)));
        self.list = list;
    }

    /// Places each of `list` and what it carries, from the topmost in its lineage down, so each is
    /// placed once, under its parent's kept place; or draws them so, with `drawn`, as
    /// [`draw`](Self::draw) says.
    fn walk_from_tops(&mut self, world: &World, list: &[Entity], drawn: Option<(&Past, f32, u64)>) {
        let pass = self.pass;
        for &entity in list {
            self.found[entity.slot()] = pass;
        }
        for &entity in list {
            if self.placed[entity.slot()] == pass {
                continue;
            }
            let mut top = entity;
            let mut up = world.parent(top).expect("a row's entity is alive");
            while let Some(parent) = up.filter(|parent| self.found[parent.slot()] == pass) {
                top = parent;
                up = world.parent(top).expect("a parent is alive");
            }
            let under = up.and_then(|parent| {
                let slot = parent.slot();
                debug_assert_eq!(self.entities[slot], parent, "a parent is placed before");
                self.kept[slot]
            });
            self.walk(world, top, under, drawn);
        }
    }

    /// Places `top`, under its parent's place `under`, and what it carries, as the latest tick
    /// left them; or draws them, with `drawn`, as [`draw`](Self::draw) says.
    fn walk(
        &mut self,
        world: &World,
        top: Entity,
        under: Option<Place>,
        drawn: Option<(&Past, f32, u64)>,
    ) {
        let transform = world.component::<Transform>();
        let pass = self.pass;
        self.stack.push((top, under));
        while let Some((entity, under)) = self.stack.pop() {
            let (index, row) = world.table_row(entity).expect("a placed entity is alive");
            let (index, row) = (index as usize, row as usize);
            let table = &world.tables()[index];
            let local = transform.and_then(|id| table.column::<Transform>(id));
            let local = local.map(|transforms| match drawn {
                Some((past, alpha, started)) => {
                    drawn_as(world, past, alpha, started, (index, row), transforms[row])
                }
                None => transforms[row],
            });
            let place = Place::under(under, local.as_ref());
            let slot = entity.slot();
            self.placed[slot] = pass;
            if drawn.is_some() {
                self.drawn[slot] = place;
                self.moving.push(entity);
            } else {
                self.entities[slot] = entity;
                self.kept[slot] = place;
                self.changed.push(entity);
            }
            let children = world.children(entity).expect("a placed entity is alive");
            self.stack.extend(children.map(|child| (child, place)));
        }
    }

    pub(crate) fn kept(&self, entity: Entity) -> Option<Place> {
        let slot = entity.slot();
        (self.entities.get(slot) == Some(&entity)).then(|| self.kept[slot].unwrap_or(Place::ORIGIN))
    }

    /// Where `entity` draws this frame, if placed: on its way between ticks, else as the latest
    /// tick left it.
    pub(crate) fn drawn(&self, entity: Entity) -> Option<Place> {
        let slot = entity.slot();
        if self.drawing != 0 && self.placed.get(slot) == Some(&self.drawing) {
            return Some(self.drawn[slot].unwrap_or(Place::ORIGIN));
        }
        self.kept(entity)
    }

    /// The entities the latest placing placed again.
    pub(crate) fn changed(&self) -> &[Entity] {
        &self.changed
    }

    pub(crate) fn moving(&self) -> &[Entity] {
        &self.moving
    }

    /// Starts a pass, with room for every slot, and returns its number.
    fn next_pass(&mut self, world: &World) -> u32 {
        let slots = world.slots();
        if self.entities.len() < slots {
            self.entities.resize(slots, Entity::DEAD);
            self.kept.resize(slots, None);
            self.drawn.resize(slots, None);
            self.found.resize(slots, 0);
            self.placed.resize(slots, 0);
        }
        // Numbered from 1: 0 is none. After 2^32 passes, from 1 again, none counting.
        self.pass = self.pass.checked_add(1).unwrap_or_else(|| {
            self.found.fill(0);
            self.placed.fill(0);
            self.drawing = 0;
            1
        });
        self.pass
    }
}

/// The transform the row (table index, row) with transform `local` draws as: what `past` has it
/// draw instead; else, if it changed since change tick `started`, `alpha` of the way on from what
/// `past` has it draw from; else `local`.
fn drawn_as(
    world: &World,
    past: &Past,
    alpha: f32,
    started: u64,
    (index, row): (usize, usize),
    local: Transform,
) -> Transform {
    let instead = past.instead_of(index);
    if let Ok(at) = instead.binary_search_by_key(&(row as u32), |&(_, of, _)| of) {
        return instead[at].2;
    }
    let table = &world.tables()[index];
    let moved = (world.component::<Transform>())
        .and_then(|id| table.changed(id))
        .is_some_and(|marks| marks.row(row) >= started);
    match (alpha < 1.0 && moved)
        .then(|| past.from(world, index))
        .flatten()
    {
        Some(from) => interpolation::between(&from[row], &local, alpha),
        None => local,
    }
}

#[cfg(test)]
impl Placed {
    /// `world` placed and drawn `alpha` of the way on from `past`, every transform taken as
    /// changed since the latest tick started: for tests, which draw a world once.
    pub(crate) fn of(world: &World, past: &Past, alpha: f32) -> Self {
        let mut placed = Self::default();
        placed.place(world);
        placed.draw(world, past, alpha, 0, None);
        placed
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec3;

    use super::*;

    fn at(x: f32, y: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, y, 0.0).as_dvec3(),
            rotation: Quat::from_rotation_z(x + y),
            scale: Vec3::new(1.0, 2.0, 1.0),
        }
    }

    fn bits(place: Option<Place>) -> String {
        format!("{place:?}")
    }

    /// Places `world` with `kept`, as each frame does, checks each live one of `entities` against
    /// a fresh placing, bit for bit, advances the world's change tick as a frame's drawing does,
    /// and returns the entities placed again, sorted.
    fn place(world: &mut World, kept: &mut Placed, entities: &[Entity]) -> Vec<Entity> {
        kept.place(world);
        let fresh = Placed::of(world, &Past::default(), 1.0);
        for &entity in entities.iter().filter(|&&entity| world.contains(entity)) {
            assert_eq!(
                bits(kept.kept(entity)),
                bits(fresh.kept(entity)),
                "{entity}"
            );
            let pose = world.pose(entity).expect("alive");
            let place = kept.kept(entity).expect("placed");
            assert_eq!(format!("{:?}", pose.matrix), format!("{:?}", place.matrix));
        }
        world.advance();
        let mut changed = kept.changed().to_vec();
        changed.sort();
        changed
    }

    fn sorted<const N: usize>(mut entities: [Entity; N]) -> Vec<Entity> {
        entities.sort();
        entities.to_vec()
    }

    /// A placing kept between frames places each entity as a fresh one and `World::pose` do, bit
    /// for bit, as transforms and parents change and entities come, go, gain and lose transforms.
    /// It places again only what changed and what it carries, or all of another world.
    #[test]
    fn a_kept_placing_places_as_a_fresh_one_does() {
        let mut world = World::default();
        let root = world.spawn(at(1.0, 0.0));
        let [a, b] = [at(0.5, 1.0), at(-2.0, 0.25)].map(|to| world.spawn(to));
        let bare = world.spawn(());
        let [c, d] = [at(3.0, -1.0), at(0.0, 0.5)].map(|to| world.spawn(to));
        for (child, parent) in [(a, root), (b, a), (c, bare), (d, c)] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let all = [root, a, b, bare, c, d];
        let mut kept = Placed::default();
        assert_eq!(place(&mut world, &mut kept, &all), sorted(all));
        assert_eq!(place(&mut world, &mut kept, &all), []);

        world
            .get_mut::<Transform>(root)
            .expect("it has one")
            .translation
            .x = 4.0;
        assert_eq!(place(&mut world, &mut kept, &all), sorted([root, a, b]));
        world.set_parent(c, Some(a)).expect("no cycle");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([c, d]));
        world.remove::<Transform>(a).expect("alive");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([a, b, c, d]));
        world.insert(bare, at(0.0, -3.0)).expect("alive");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([bare]));
        // A root without a parent leaves no mark but joining another table.
        world.remove::<Transform>(root).expect("alive");
        assert_eq!(
            place(&mut world, &mut kept, &all),
            sorted([root, a, b, c, d])
        );
        let e = world.spawn(at(1.0, 1.0));
        world.set_parent(e, Some(b)).expect("no cycle");
        let all = [root, a, b, bare, c, d, e];
        assert_eq!(place(&mut world, &mut kept, &all), sorted([e]));
        world.despawn(a).expect("alive");
        assert_eq!(place(&mut world, &mut kept, &all), []);
        let f = world.spawn(at(2.0, 2.0));
        assert!(kept.kept(f).is_none(), "its slot was another's");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([f]));

        let copy = world.clone();
        world.clone_from(&copy);
        let all = [root, bare, f];
        assert_eq!(place(&mut world, &mut kept, &all), sorted(all));
    }

    /// Between ticks, what the latest tick moved draws on its way, with what it carries (each
    /// child its parent's drawn place times its own); the rest, and everything at alpha 1, where
    /// the tick left it.
    #[test]
    fn what_the_tick_moved_draws_on_its_way_and_carries_what_it_carries() {
        let mut world = World::default();
        let [mover, kid, still, calm] =
            [at(0.0, 0.0), at(0.0, 1.0), at(5.0, 0.0), at(0.0, 2.0)].map(|to| world.spawn(to));
        world.set_parent(kid, Some(mover)).expect("no cycle");
        world.set_parent(calm, Some(still)).expect("no cycle");
        let mut placed = Placed::default();
        placed.place(&world);
        world.advance();
        let mut past = Past::default();
        past.capture(&mut world);
        let started = world.change_tick();
        let from = *world.get::<Transform>(mover).expect("it has one");
        world
            .get_mut::<Transform>(mover)
            .expect("it has one")
            .translation
            .x = 2.0;
        let to = *world.get::<Transform>(mover).expect("it has one");
        past.resolve(&world);
        placed.place(&world);
        placed.draw(&world, &past, 0.5, started, None);

        let mut moving = placed.moving().to_vec();
        moving.sort();
        assert_eq!(moving, sorted([mover, kid]));
        let local = |entity| world.get::<Transform>(entity).expect("it has one").matrix();
        let way = interpolation::between(&from, &to, 0.5).matrix();
        let drawn = |entity| placed.drawn(entity).expect("placed").matrix;
        assert_eq!(
            format!("{:?}", drawn(kid)),
            format!("{:?}", way * local(kid))
        );
        assert_eq!(format!("{:?}", drawn(mover)), format!("{way:?}"));
        let kept = |entity| placed.kept(entity).expect("placed").matrix;
        assert_eq!(kept(kid), to.matrix() * local(kid));
        assert_eq!(drawn(calm), kept(calm));

        placed.draw(&world, &past, 1.0, started, None);
        assert_eq!(placed.moving(), []);
        assert_eq!(placed.drawn(kid), placed.kept(kid));
    }

    /// 1,000 km from the world's origin, a parent moving 1 mm a tick draws on its way, and its
    /// child 1 mm from it draws 1 mm from it, as drawing draws them: in `f32` from a nearby
    /// origin. Placed in `f32` from the world's origin they'd be 6 cm steps.
    #[test]
    fn far_from_the_origin_what_moves_draws_on_its_way_to_the_millimeter() {
        let mut world = World::default();
        let far = DVec3::new(1.0e6 + 0.3, 0.0, -1.0e6);
        let parent = world.spawn(Transform {
            translation: far,
            ..Transform::IDENTITY
        });
        let child = world.spawn(Transform {
            translation: DVec3::new(0.001, 0.0, 0.0),
            ..Transform::IDENTITY
        });
        world.set_parent(child, Some(parent)).expect("no cycle");
        let mut placed = Placed::default();
        placed.place(&world);
        world.advance();
        let mut past = Past::default();
        past.capture(&mut world);
        let started = world.change_tick();
        let moved = world.get_mut::<Transform>(parent).expect("it has one");
        moved.translation.x += 0.001;
        past.resolve(&world);
        placed.place(&world);
        placed.draw(&world, &past, 0.5, started, None);
        let origin = DVec3::new(1.0e6, 0.0, -1.0e6);
        let drawn = |entity| {
            let place = placed.drawn(entity).expect("placed");
            crate::transform::from_origin(place.matrix, origin)
                .w_axis
                .truncate()
        };
        let halfway = Vec3::new(0.3005, 0.0, 0.0);
        assert!(
            drawn(parent).abs_diff_eq(halfway, 1e-6),
            "{}",
            drawn(parent)
        );
        let apart = drawn(child) - drawn(parent);
        assert!(apart.abs_diff_eq(Vec3::X * 0.001, 1e-6), "{apart}");
    }
}
