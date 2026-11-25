//! Where the world's entities are: each one's matrix to world space (its transform's, then its
//! ancestors' in turn), its rotation, and its tint (its own [`Tint`] times its ancestors'). They're
//! kept from frame to frame as the latest tick left them, and placed again only where a transform,
//! tint or parent changed, with what it carries. A root carrying nothing, outside tables whose
//! places are held elsewhere, isn't kept: drawing reads its transform and tint, and
//! [`Placed::kept`] finds its place from it. Also where entities on their way between ticks draw
//! this frame, which follows only what the latest tick moved.

use std::mem;

use gg_core::math::{DAffine3, DVec3, Quat, Vec4};

use crate::ecs::ComponentId;
use crate::interpolation::{self, Past};
use crate::{Entity, Parent, Tint, Transform, World};

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
    /// By slot: its entity's tint as the latest tick left it, for one placed in `kept`.
    tints: Vec<Vec4>,
    /// By slot: whether its entity was last placed as a root carrying nothing, whose place isn't
    /// in `kept` but its transform's.
    own: Vec<bool>,
    /// By slot: the last pass that found its entity changed, and the last that placed it or drew
    /// it on its way.
    found: Vec<u32>,
    placed: Vec<u32>,
    /// By slot: the last placing that placed its entity again; `placing` is the latest's pass.
    again: Vec<u32>,
    placing: u32,
    /// The pass under way: each placing and each drawing is one.
    pass: u32,
    list: Vec<Entity>,
    changed: Vec<Entity>,
    /// By slot: where its entity draws on its way between ticks, in the pass `drawing` numbers, if
    /// one has drawn since the latest placing; 0 if none has.
    drawn: Vec<Option<Place>>,
    drawing: u32,
    moving: Vec<Entity>,
    stack: Vec<(Entity, Option<Place>, Vec4)>,
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
    /// whose transform, tint or parent changed or that joined a table (as spawning, or giving or
    /// taking a transform, tint or parent, does), and what each carries, from the topmost changed.
    /// A root carrying nothing, in a table without component `held`, is left to its transform: not
    /// placed, nor among [`changed`](Self::changed) (see [`kept`](Self::kept)). Cost follows what
    /// changed and what it carries, apart from a look over each table's marks.
    pub(crate) fn place(&mut self, world: &World, held: Option<ComponentId>) {
        let _zone = gg_core::zone!();
        let since = match self.seen {
            Some((serial, tick)) if serial == world.serial() => Some(tick),
            _ => None,
        };
        self.seen = Some((world.serial(), world.change_tick()));
        self.placing = self.next_pass(world);
        self.drawing = 0;
        self.changed.clear();
        let mut list = mem::take(&mut self.list);
        list.clear();
        let (transform, parent) = (world.component::<Transform>(), world.component::<Parent>());
        let tint = world.component::<Tint>();
        for table in world.tables() {
            let entities = table.entities();
            let roots = !parent.is_some_and(|id| table.has(id));
            let own = roots && !held.is_some_and(|id| table.has(id));
            let Some(since) = since else {
                for &entity in entities {
                    self.add(world, own, entity, &mut list);
                }
                continue;
            };
            let marks = [
                Some(table.joined()),
                transform.and_then(|id| table.changed(id)),
                parent.and_then(|id| table.changed(id)),
                tint.and_then(|id| table.changed(id)),
            ];
            for marks in marks.into_iter().flatten() {
                if marks.since(since) {
                    for row in (0..entities.len()).filter(|&row| marks.row(row) > since) {
                        self.add(world, own, entities[row], &mut list);
                    }
                }
            }
        }
        self.walk_from_tops(world, &list, None);
        self.list = list;
    }

    /// Adds `entity`, which changed, to `list` to place; or, if `own` (its table's roots draw by
    /// their transforms) and it carries nothing, only notes it in its slot: its place is its
    /// transform's.
    fn add(&mut self, world: &World, own: bool, entity: Entity, list: &mut Vec<Entity>) {
        let carries = || {
            let mut children = world.children(entity).expect("a row's entity is alive");
            children.next().is_some()
        };
        if !own || carries() {
            list.push(entity);
            return;
        }
        let slot = entity.slot();
        self.entities[slot] = entity;
        self.own[slot] = true;
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
        let _zone = gg_core::zone!();
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
            let (under, tint) = up.map_or((None, Vec4::ONE), |parent| {
                let slot = parent.slot();
                debug_assert_eq!(self.entities[slot], parent, "a parent is placed before");
                (self.place_of(world, parent), self.tint_of(world, parent))
            });
            self.walk(world, top, (under, tint), drawn);
        }
    }

    /// Places `top`, under its parent's place and tint `under`, and what it carries, as the latest
    /// tick left them; or draws them, with `drawn`, as [`draw`](Self::draw) says.
    fn walk(
        &mut self,
        world: &World,
        top: Entity,
        (under, tinted): (Option<Place>, Vec4),
        drawn: Option<(&Past, f32, u64)>,
    ) {
        let transform = world.component::<Transform>();
        let tint = world.component::<Tint>();
        let pass = self.pass;
        self.stack.push((top, under, tinted));
        while let Some((entity, under, tinted)) = self.stack.pop() {
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
            let own = tint.and_then(|id| table.column::<Tint>(id));
            let tinted = own.map_or(tinted, |tints| tinted * tints[row].0);
            let slot = entity.slot();
            self.placed[slot] = pass;
            if drawn.is_some() {
                self.drawn[slot] = place;
                self.moving.push(entity);
            } else {
                self.entities[slot] = entity;
                self.kept[slot] = place;
                self.tints[slot] = tinted;
                self.own[slot] = false;
                self.again[slot] = pass;
                self.changed.push(entity);
            }
            let children = world.children(entity).expect("a placed entity is alive");
            self.stack
                .extend(children.map(|child| (child, place, tinted)));
        }
    }

    /// Where `entity` of `world` is as the latest tick left it, if placed. A root the placing
    /// left unplaced is where its transform puts it in `world`, which hasn't changed since.
    pub(crate) fn kept(&self, world: &World, entity: Entity) -> Option<Place> {
        let slot = entity.slot();
        (self.entities.get(slot) == Some(&entity))
            .then(|| self.place_of(world, entity).unwrap_or(Place::ORIGIN))
    }

    /// Where `entity` of `world` draws this frame, if placed: on its way between ticks, else as
    /// the latest tick left it.
    pub(crate) fn drawn(&self, world: &World, entity: Entity) -> Option<Place> {
        let slot = entity.slot();
        if self.drawing != 0 && self.placed.get(slot) == Some(&self.drawing) {
            return Some(self.drawn[slot].unwrap_or(Place::ORIGIN));
        }
        self.kept(world, entity)
    }

    /// The place of `entity`, placed in its slot, as `kept` holds them: `None` is the origin.
    fn place_of(&self, world: &World, entity: Entity) -> Option<Place> {
        let slot = entity.slot();
        match self.own[slot] {
            true => Place::under(None, world.get::<Transform>(entity)),
            false => self.kept[slot],
        }
    }

    /// `entity`'s tint, if placed: its own [`Tint`] times its ancestors', as the latest tick left
    /// them; white for none.
    pub(crate) fn tint(&self, world: &World, entity: Entity) -> Vec4 {
        let slot = entity.slot();
        match self.entities.get(slot) == Some(&entity) {
            true => self.tint_of(world, entity),
            false => Vec4::ONE,
        }
    }

    /// The tint of `entity`, placed in its slot: a root left to its transform has its own.
    fn tint_of(&self, world: &World, entity: Entity) -> Vec4 {
        let slot = entity.slot();
        match self.own[slot] {
            true => world.get::<Tint>(entity).map_or(Vec4::ONE, |tint| tint.0),
            false => self.tints[slot],
        }
    }

    /// The entities the latest placing placed again. Roots it left to their transforms aren't
    /// among them: their tables' marks say they changed.
    pub(crate) fn changed(&self) -> &[Entity] {
        &self.changed
    }

    pub(crate) fn moving(&self) -> &[Entity] {
        &self.moving
    }

    /// Whether `entity` is among [`changed`](Self::changed), found by its slot.
    pub(crate) fn placed_again(&self, entity: Entity) -> bool {
        let slot = entity.slot();
        self.placing != 0
            && self.again.get(slot) == Some(&self.placing)
            && self.entities[slot] == entity
    }

    /// Whether `entity` is among [`moving`](Self::moving), found by its slot.
    pub(crate) fn is_moving(&self, entity: Entity) -> bool {
        self.drawing != 0 && self.placed.get(entity.slot()) == Some(&self.drawing)
    }

    /// Starts a pass, with room for every slot, and returns its number.
    fn next_pass(&mut self, world: &World) -> u32 {
        let slots = world.slots();
        if self.entities.len() < slots {
            self.entities.resize(slots, Entity::DEAD);
            self.kept.resize(slots, None);
            self.tints.resize(slots, Vec4::ONE);
            self.own.resize(slots, false);
            self.drawn.resize(slots, None);
            self.found.resize(slots, 0);
            self.placed.resize(slots, 0);
            self.again.resize(slots, 0);
        }
        // Numbered from 1: 0 is none. After 2^32 passes, from 1 again, none counting.
        self.pass = self.pass.checked_add(1).unwrap_or_else(|| {
            self.found.fill(0);
            self.placed.fill(0);
            self.again.fill(0);
            (self.drawing, self.placing) = (0, 0);
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
        placed.place(world, None);
        placed.draw(world, past, alpha, 0, None);
        placed
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Vec3, Vec4};

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
    /// a fresh placing, place and tint bit for bit, advances the world's change tick as a frame's
    /// drawing does, and returns the entities placed again, sorted.
    fn place(world: &mut World, kept: &mut Placed, entities: &[Entity]) -> Vec<Entity> {
        kept.place(world, None);
        let fresh = Placed::of(world, &Past::default(), 1.0);
        for &entity in entities.iter().filter(|&&entity| world.contains(entity)) {
            assert_eq!(
                bits(kept.kept(world, entity)),
                bits(fresh.kept(world, entity)),
                "{entity}"
            );
            let tints = [kept.tint(world, entity), fresh.tint(world, entity)];
            assert_eq!(tints[0], tints[1], "{entity}'s tint");
            let pose = world.pose(entity).expect("alive");
            let place = kept.kept(world, entity).expect("placed");
            assert_eq!(format!("{:?}", pose.matrix), format!("{:?}", place.matrix));
            let listed = kept.changed().contains(&entity);
            assert_eq!(kept.placed_again(entity), listed, "{entity}");
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
    /// It places again, and lists, only what changed and what it carries, or all of another world;
    /// roots carrying nothing it leaves to their transforms, unlisted.
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
        assert_eq!(place(&mut world, &mut kept, &all), []);
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
        assert!(kept.kept(&world, f).is_none(), "its slot was another's");
        let all = [root, bare, f];
        assert_eq!(place(&mut world, &mut kept, &all), []);

        // `f`, a root carrying nothing in `a`'s old slot, is left unplaced: where its transform
        // now puts it, as a skin asks of a joint, and under where it now is for a child it gains.
        let moved = |world: &mut World, entity, x| {
            (world.get_mut::<Transform>(entity))
                .expect("it has one")
                .translation
                .x = x;
        };
        moved(&mut world, f, 5.0);
        assert_eq!(place(&mut world, &mut kept, &all), []);
        let g = world.spawn(at(0.5, -0.5));
        world.set_parent(g, Some(f)).expect("no cycle");
        let all = [root, bare, f, g];
        assert_eq!(place(&mut world, &mut kept, &all), sorted([g]));
        moved(&mut world, f, -1.0);
        assert_eq!(place(&mut world, &mut kept, &all), sorted([f, g]));

        let copy = world.clone();
        world.clone_from(&copy);
        assert_eq!(place(&mut world, &mut kept, &all), sorted([f, g]));
    }

    /// A kept placing tints as a fresh one does, each entity its own tint times its ancestors', as
    /// tints come, change and go. It places again, and lists, what a changed tint tints.
    #[test]
    fn a_kept_placing_tints_as_a_fresh_one_does() {
        let mut world = World::default();
        let half = Tint(Vec4::splat(0.5));
        let red = Tint(Vec4::new(1.0, 0.0, 0.0, 1.0));
        let root = world.spawn((at(1.0, 0.0), red));
        let [a, b] = [at(0.5, 1.0), at(-2.0, 0.25)].map(|to| world.spawn((to, half)));
        let bare = world.spawn(half);
        let c = world.spawn(at(3.0, -1.0));
        let lone = world.spawn((at(0.0, 2.0), red));
        for (child, parent) in [(a, root), (b, a), (c, bare)] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }
        let all = [root, a, b, bare, c, lone];
        let mut kept = Placed::default();
        place(&mut world, &mut kept, &all);
        let tint = |kept: &Placed, world: &World, entity| kept.tint(world, entity);
        assert_eq!(tint(&kept, &world, b), Vec4::new(0.25, 0.0, 0.0, 0.25));
        assert_eq!(tint(&kept, &world, c), half.0);
        assert_eq!(
            tint(&kept, &world, lone),
            red.0,
            "a root left to its transform"
        );

        world.get_mut::<Tint>(a).expect("tinted").0 = Vec4::ONE;
        assert_eq!(place(&mut world, &mut kept, &all), sorted([a, b]));
        assert_eq!(tint(&kept, &world, b), Vec4::new(0.5, 0.0, 0.0, 0.5));
        world.remove::<Tint>(root).expect("alive");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([root, a, b]));
        assert_eq!(tint(&kept, &world, b), half.0);
        world.insert(c, red).expect("alive");
        assert_eq!(place(&mut world, &mut kept, &all), sorted([c]));
        assert_eq!(tint(&kept, &world, c), red.0 * half.0);
        world.get_mut::<Tint>(lone).expect("tinted").0 = Vec4::ONE;
        assert_eq!(place(&mut world, &mut kept, &all), []);
        assert_eq!(tint(&kept, &world, lone), Vec4::ONE);
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
        placed.place(&world, None);
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
        placed.place(&world, None);
        placed.draw(&world, &past, 0.5, started, None);

        let mut moving = placed.moving().to_vec();
        moving.sort();
        assert_eq!(moving, sorted([mover, kid]));
        for entity in [mover, kid, still, calm] {
            assert_eq!(
                placed.is_moving(entity),
                moving.contains(&entity),
                "{entity}"
            );
        }
        let local = |entity| world.get::<Transform>(entity).expect("it has one").matrix();
        let way = interpolation::between(&from, &to, 0.5).matrix();
        let drawn = |entity| placed.drawn(&world, entity).expect("placed").matrix;
        assert_eq!(
            format!("{:?}", drawn(kid)),
            format!("{:?}", way * local(kid))
        );
        assert_eq!(format!("{:?}", drawn(mover)), format!("{way:?}"));
        let kept = |entity| placed.kept(&world, entity).expect("placed").matrix;
        assert_eq!(kept(kid), to.matrix() * local(kid));
        assert_eq!(drawn(calm), kept(calm));

        placed.draw(&world, &past, 1.0, started, None);
        assert_eq!(placed.moving(), []);
        assert!(!placed.is_moving(mover));
        assert_eq!(placed.drawn(&world, kid), placed.kept(&world, kid));
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
        placed.place(&world, None);
        world.advance();
        let mut past = Past::default();
        past.capture(&mut world);
        let started = world.change_tick();
        let moved = world.get_mut::<Transform>(parent).expect("it has one");
        moved.translation.x += 0.001;
        past.resolve(&world);
        placed.place(&world, None);
        placed.draw(&world, &past, 0.5, started, None);
        let origin = DVec3::new(1.0e6, 0.0, -1.0e6);
        let drawn = |entity| {
            let place = placed.drawn(&world, entity).expect("placed");
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
