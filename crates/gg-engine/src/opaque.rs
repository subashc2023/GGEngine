//! The world's opaque meshes, as the renderer's scene holds them: each entity with a transform and a
//! [`MeshDraw`] whose shader writes depth, in the scene's slot of the entity's own, set as its draw,
//! its place or its table changes, and taken away as it goes; so that drawing them costs what
//! changed of them, rather than all of them every frame.

use gg_core::math::DVec3;
use gg_renderer::{MeshDraw, Scene};

use crate::placed::Placed;
use crate::skins::Skins;
use crate::transform::from_origin;
use crate::{Entity, Transform, World};

/// What the renderer's scene holds of the world's opaque meshes, as the module says, by the
/// entities' slots: kept from frame to frame, so that it allocates nothing once it's grown.
#[derive(Default)]
pub(crate) struct OpaqueMeshes {
    /// The world held, by its serial, and its version and change tick as it was, the changes
    /// after which it holds again; `None` before it holds one.
    seen: Option<(u64, u64, u64)>,
    /// The frame's origin the scene's matrices are from.
    origin: DVec3,
    /// By slot: the entity whose mesh the scene holds there, [`Entity::DEAD`] for none.
    held: Vec<Entity>,
    /// The entities drawn on their way between ticks last frame, set again where the tick left
    /// them once they aren't on their way.
    moved: Vec<Entity>,
    /// By slot: the last pass that set it, so that a pass sets each once.
    set_in: Vec<u32>,
    pass: u32,
}

impl OpaqueMeshes {
    /// Has `scene` hold `world`'s opaque meshes as `placed` has them draw this frame, a skinned
    /// entity's of the mesh of its own that `skins` gives it, setting and
    /// taking away what changed since it last did: everything, in another world; else each whose
    /// mesh changed, or that joined a table, as one spawned or given a mesh or a transform does;
    /// each that `placed` placed again, or draws on its way between ticks, or drew so last frame
    /// and doesn't now; each that `skins` gave a mesh of its own this frame; and, as entities' rows
    /// changed, each held that went, or lost its transform
    /// or its mesh, or whose mesh no longer writes depth, which it looks over all held for; and
    /// every one held, from `origin`, the frame's, once it's another than the last frame's. Called
    /// once a frame, after `placed` has drawn it and `skins` found it.
    pub(crate) fn hold(
        &mut self,
        world: &World,
        placed: &Placed,
        skins: &Skins,
        scene: &mut Scene,
        origin: DVec3,
    ) {
        let _zone = gg_core::zone!();
        let (serial, version) = world.version();
        let since = match self.seen {
            Some((held, was, tick)) if held == serial => Some((was, tick)),
            _ => None,
        };
        self.seen = Some((serial, version, world.change_tick()));
        let pass = self.next_pass(world);
        let moved_origin = self.origin != origin;
        self.origin = origin;
        if since.is_none() {
            scene.clear();
            self.held.fill(Entity::DEAD);
            self.moved.clear();
        }
        let (Some(mesh), Some(transform)) = (
            world.component::<MeshDraw>(),
            world.component::<Transform>(),
        ) else {
            return;
        };
        // Those that went, or no longer draw opaque.
        if since.is_some_and(|(was, _)| was != version) {
            for (slot, held) in (0..).zip(&mut self.held) {
                if *held != Entity::DEAD && opaque(world, *held).is_none() {
                    scene.remove(slot);
                    *held = Entity::DEAD;
                }
            }
        }
        // Every one held, from the frame's origin, once it moved.
        if moved_origin && since.is_some() {
            for slot in 0..self.held.len() {
                let entity = self.held[slot];
                if entity != Entity::DEAD
                    && let Some(draw) = opaque(world, entity)
                {
                    self.set(world, placed, skins, scene, entity, draw, pass);
                }
            }
        }
        // Those whose meshes changed, or that joined their tables.
        for table in world.tables() {
            let Some(meshes) = table.column::<MeshDraw>(mesh) else {
                continue;
            };
            if !table.has(transform) {
                continue;
            }
            let entities = table.entities();
            let marks = [table.changed(mesh).expect("it has meshes"), table.joined()];
            let since = since.map(|(_, tick)| tick);
            if since.is_some_and(|tick| marks.iter().all(|marks| !marks.since(tick))) {
                continue;
            }
            for (row, &entity) in entities.iter().enumerate() {
                let changed = |tick| marks.iter().any(|marks| marks.row(row) > tick);
                if !since.is_none_or(changed) {
                    continue;
                }
                let slot = entity.slot();
                if meshes[row].batch().is_some() {
                    self.set(world, placed, skins, scene, entity, &meshes[row], pass);
                } else if self.held[slot] == entity {
                    scene.remove(slot as u32);
                    self.held[slot] = Entity::DEAD;
                }
            }
        }
        // Those placed again, or on their way between ticks, this frame or the last, and those
        // given a mesh of their own.
        let mut moved = std::mem::take(&mut self.moved);
        let placed_again = placed.changed().iter().chain(placed.moving()).chain(&moved);
        for &entity in placed_again.chain(skins.fresh()) {
            if self.held[entity.slot()] == entity
                && let Some(draw) = opaque(world, entity)
            {
                self.set(world, placed, skins, scene, entity, draw, pass);
            }
        }
        moved.clear();
        let held = |entity: &Entity| self.held[entity.slot()] == *entity;
        moved.extend(placed.moving().iter().copied().filter(held));
        self.moved = moved;
    }

    /// Sets `entity`'s mesh, `draw`, or the mesh of its own `skins` gives it, in its slot of
    /// `scene`, where `placed` has it draw, unless pass `pass` has.
    #[expect(
        clippy::too_many_arguments,
        reason = "what setting a slot reads, each apart"
    )]
    fn set(
        &mut self,
        world: &World,
        placed: &Placed,
        skins: &Skins,
        scene: &mut Scene,
        entity: Entity,
        draw: &MeshDraw,
        pass: u32,
    ) {
        let slot = entity.slot();
        if self.set_in[slot] == pass {
            return;
        }
        self.set_in[slot] = pass;
        self.held[slot] = entity;
        let place = placed.drawn(entity).expect("a drawn entity is placed");
        let id = world.id(entity).expect("a row's entity is alive").get();
        let matrix = from_origin(place.matrix, self.origin);
        scene.set(slot as u32, matrix, &skins.draw_of(entity, draw), id);
    }

    /// Starts a pass, with room for each of `world`'s slots, and returns its number.
    fn next_pass(&mut self, world: &World) -> u32 {
        let slots = world.slots();
        if self.held.len() < slots {
            self.held.resize(slots, Entity::DEAD);
            self.set_in.resize(slots, 0);
        }
        // Numbered from 1, so that 0 is none's; after 2^32 passes, from 1 again, none counting.
        self.pass = self.pass.checked_add(1).unwrap_or_else(|| {
            self.set_in.fill(0);
            1
        });
        self.pass
    }
}

/// `entity`'s mesh, if it's alive, with a transform, and its mesh's shader writes depth.
fn opaque(world: &World, entity: Entity) -> Option<&MeshDraw> {
    world.get::<Transform>(entity)?;
    world
        .get::<MeshDraw>(entity)
        .filter(|draw| draw.batch().is_some())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gg_core::math::{DAffine3, Mat4};
    use gg_renderer::Uploads;

    use super::*;
    use crate::interpolation::{self, Past};
    use crate::transform::from_origin;

    fn at(x: f32) -> Transform {
        Transform {
            translation: DVec3::new(f64::from(x), 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    /// A draw whose shader writes depth, if `opaque`, or blends.
    fn draw(opaque: bool) -> MeshDraw {
        MeshDraw::from_parts(0, 0, opaque, false, &[1, 2, 3, 4]).expect("four bytes")
    }

    /// What a scene holds: each slot's transform, ID and draw.
    type Held = BTreeMap<u32, (Mat4, u64, MeshDraw)>;

    /// What drawing the world has a renderer's scene hold, as each frame does, at `alpha` of the
    /// way on from `past`, the latest tick having started at change tick `started`, taking what
    /// it changed into `held`; and the slots it changed, as the frame's drawing moves the world's
    /// change tick on.
    fn frame(
        world: &mut World,
        (placed, opaque, scene): &mut (Placed, OpaqueMeshes, Scene),
        (past, alpha, started): (&Past, f32, u64),
        held: &mut Held,
    ) -> Vec<u32> {
        placed.place(world);
        placed.draw(world, past, alpha, started, world.component::<MeshDraw>());
        opaque.hold(world, placed, &Skins::default(), scene, DVec3::ZERO);
        world.advance();
        let mut changed = Vec::new();
        for change in scene.changes() {
            match change {
                Some((slot, set)) => {
                    changed.push(slot);
                    match set {
                        Some(set) => held.insert(slot, set),
                        None => held.remove(&slot),
                    };
                }
                None => held.clear(),
            }
        }
        Uploads::default().take_scene(scene);
        changed.sort_unstable();
        changed.dedup();
        changed
    }

    /// The scene holds each opaque mesh with a transform in its entity's slot, where it's placed,
    /// with its ID, and changes only what changed: a mesh placed again, as its parent moves; one
    /// that came, went, or blends now; everything, in a restored world; and one on its way between
    /// ticks, each frame, and once more as it stops.
    #[test]
    fn the_scene_holds_the_opaque_meshes_as_they_change() {
        let mut world = World::default();
        let a = world.spawn((at(1.0), draw(true)));
        let parent = world.spawn(at(2.0));
        let b = world.spawn((at(3.0), draw(true)));
        world.set_parent(b, Some(parent)).expect("no cycle");
        world.spawn((at(4.0), draw(false)));
        world.spawn(draw(true));
        let mut state = (Placed::default(), OpaqueMeshes::default(), Scene::default());
        let mut held = Held::new();
        let past = Past::default();
        let still = (&past, 1.0, 0);
        let slot = |entity: Entity| entity.slot() as u32;
        let id = |world: &World, entity| world.id(entity).expect("alive").get();

        assert_eq!(
            frame(&mut world, &mut state, still, &mut held),
            [slot(a), slot(b)]
        );
        let carried = at(2.0).matrix() * at(3.0).matrix();
        // As the scene holds them, from the frame's origin.
        let want = |world: &World, a_at: DAffine3, b_at: DAffine3| {
            let [a_at, b_at] = [a_at, b_at].map(|at| from_origin(at, DVec3::ZERO));
            Held::from([
                (slot(a), (a_at, id(world, a), draw(true))),
                (slot(b), (b_at, id(world, b), draw(true))),
            ])
        };
        assert_eq!(held, want(&world, at(1.0).matrix(), carried));
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [0; 0]);

        world
            .get_mut::<Transform>(parent)
            .expect("it has one")
            .translation
            .y = 1.0;
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [slot(b)]);
        let carried = world.pose(b).expect("alive").matrix;
        assert_eq!(held, want(&world, at(1.0).matrix(), carried));

        world.insert(a, draw(false)).expect("alive");
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [slot(a)]);
        world.despawn(b).expect("alive");
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [slot(b)]);
        assert_eq!(held, Held::new());
        let e = world.spawn((at(5.0), draw(true)));
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [slot(e)]);
        let copy = world.clone();
        world.clone_from(&copy);
        assert_eq!(frame(&mut world, &mut state, still, &mut held), [slot(e)]);
        let only_e =
            |world: &World, matrix| Held::from([(slot(e), (matrix, id(world, e), draw(true)))]);
        assert_eq!(held, only_e(&world, at(5.0).matrix_from(DVec3::ZERO)));

        // A tick that moves it, drawn on its way over two frames; then a tick that doesn't.
        let mut past = Past::default();
        past.capture(&mut world);
        let started = world.change_tick();
        world
            .get_mut::<Transform>(e)
            .expect("it has one")
            .translation
            .x = 6.0;
        past.resolve(&world);
        for alpha in [0.5, 0.75] {
            let changed = frame(&mut world, &mut state, (&past, alpha, started), &mut held);
            assert_eq!(changed, [slot(e)], "{alpha}");
            let way = interpolation::between(&at(5.0), &at(6.0), alpha).matrix();
            assert_eq!(held, only_e(&world, from_origin(way, DVec3::ZERO)));
        }
        past.capture(&mut world);
        let started = world.change_tick();
        past.resolve(&world);
        let settled = frame(&mut world, &mut state, (&past, 0.25, started), &mut held);
        assert_eq!(settled, [slot(e)]);
        assert_eq!(held, only_e(&world, at(6.0).matrix_from(DVec3::ZERO)));
        let after = frame(&mut world, &mut state, (&past, 0.5, started), &mut held);
        assert_eq!(after, [0; 0]);
    }

    /// As the frame's origin moves, the scene holds every opaque mesh again from the new origin,
    /// though none moved, a child where its parent carries it; and a frame from that origin after
    /// changes nothing.
    #[test]
    fn the_scene_holds_every_mesh_again_from_a_moved_origin() {
        let mut world = World::default();
        let far = Transform {
            translation: DVec3::new(1.0e6 + 0.25, 0.0, -1.0e6),
            ..Transform::IDENTITY
        };
        let a = world.spawn((far, draw(true)));
        let b = world.spawn((at(3.0), draw(true)));
        world.set_parent(b, Some(a)).expect("no cycle");
        let (mut placed, mut opaque) = (Placed::default(), OpaqueMeshes::default());
        let mut scene = Scene::default();
        let past = Past::default();
        // The slots the frame from `origin` sets, with their transforms.
        let mut hold = |world: &mut World, origin: DVec3| {
            placed.place(world);
            placed.draw(world, &past, 1.0, 0, world.component::<MeshDraw>());
            opaque.hold(world, &placed, &Skins::default(), &mut scene, origin);
            world.advance();
            let changes = scene.changes();
            let mut set: Vec<(u32, Mat4)> = changes
                .filter_map(|change| Some((change?.0, change?.1?.0)))
                .collect();
            Uploads::default().take_scene(&mut scene);
            set.sort_by_key(|&(slot, _)| slot);
            set
        };
        let first = DVec3::new(1_000_448.0, 0.0, -1_000_448.0);
        hold(&mut world, first);
        assert_eq!(hold(&mut world, first), []);
        let moved = first + DVec3::new(1024.0, 0.0, 0.0);
        let place = |entity: Entity| {
            let pose = world.pose(entity).expect("alive");
            (entity.slot() as u32, from_origin(pose.matrix, moved))
        };
        let mut want = vec![place(a), place(b)];
        want.sort_by_key(|&(slot, _)| slot);
        assert_eq!(hold(&mut world, moved), want);
        assert_eq!(hold(&mut world, moved), []);
    }
}
