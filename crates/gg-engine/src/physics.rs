//! 2D physics: bodies that rapier moves at the world's ticks, in the XY plane, as their entities'
//! components describe them, and the world's resource that holds rapier's bodies and contacts,
//! which snapshots copy.

use gg_core::math::{Quat, Vec2, Vec3, libm};
use rapier2d::prelude::{
    BroadPhaseBvh, CCDSolver, Collider, ColliderBuilder, ColliderHandle, ColliderSet,
    ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase,
    PhysicsPipeline, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, RigidBodyType, Rotation,
    SoftBodySet, Vector,
};
use tracing::warn;

use crate::{Component, Entity, Reflect, Tick, Transform, World};

/// How a [`RigidBody2D`] moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum BodyType {
    /// By its velocity, which gravity and what it hits change.
    #[default]
    Dynamic,
    /// By its velocity alone, which nothing it hits changes, pushing what's dynamic aside, as a
    /// moving platform does.
    Kinematic,
    /// Not at all: it stays where its transform puts it, as a floor or a wall does.
    Static,
}

/// A body that 2D physics moves, at each tick a layer runs [`step_physics`] in: in the XY plane,
/// its entity's [`Transform`] across and up and turned about Z, and the velocities here, which
/// each step reads and writes back. It collides as its entity's [`BoxCollider2D`], if it has one,
/// which gives a dynamic body its mass too. A body under a parent isn't moved, since its transform
/// is in its parent's space (`physics.child_body`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct RigidBody2D {
    pub body_type: BodyType,
    /// Whether it never turns, as a character that stays upright.
    pub fixed_rotation: bool,
    /// How fast it goes, across and up, in units a second.
    pub velocity: Vec2,
    /// How fast it turns, counterclockwise, in radians a second.
    #[reflect(degrees)]
    pub angular_velocity: f32,
}

/// The box a [`RigidBody2D`] on the same entity collides as, in the entity's space, so that it
/// turns and scales with it: by default a unit square, a sprite's.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct BoxCollider2D {
    /// Where its middle is, from its entity's origin.
    pub offset: Vec2,
    /// How wide and tall it is.
    #[reflect(min = 0.0, default = Vec2::ONE)]
    pub size: Vec2,
    /// Its mass a square unit, which a dynamic body's mass comes from.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it holds back what slides along it: 0 not at all, as ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// How much of the speed it hits something with it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
}

impl Default for BoxCollider2D {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            size: Vec2::ONE,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
        }
    }
}

/// The world's 2D physics, a [`Resource`](crate::Resource) that [`step_physics`] gives a world the
/// first time it steps one without it, or that a game inserts first, with its gravity: rapier's
/// bodies, colliders and contacts, which each step carries on from, and each body's entity, so that
/// a snapshot restored steps as its world did. A world cleared, or whose entities a scene replaced,
/// keeps it, its bodies going at the next step, as their entities have.
pub struct Physics2D {
    /// Which way everything dynamic falls, and how much faster it goes each second, in units a
    /// second: down at 9.81 by default, Earth's, as though a unit were a meter.
    pub gravity: Vec2,
    rapier: Rapier,
    /// rapier's workspace, which a copy makes anew, since no step carries anything in it over.
    pipeline: PhysicsPipeline,
    /// A body for each entity with a `RigidBody2D` and a `Transform`, but those under a parent, in
    /// the order they came.
    linked: Vec<Linked>,
    /// Each body's index in `linked`, by its entity's slot; `NONE` for none.
    by_slot: Vec<u32>,
    /// The entities with bodies, as a step finds them, in memory kept from one step to the next.
    found: Vec<Entity>,
    /// The bodies under a parent, which aren't moved, warned of once each as they're found there.
    children: Vec<Entity>,
}

/// No body, in [`Physics2D::by_slot`].
const NONE: u32 = u32::MAX;

/// What rapier keeps from one step to the next.
#[derive(Clone)]
struct Rapier {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd: CCDSolver,
    parameters: IntegrationParameters,
}

/// A body, its entity's, and what the last step left in the entity's components, or what they
/// were as it took them in: a change to them since is a game's or an edit's, which the next step
/// takes.
#[derive(Clone)]
struct Linked {
    entity: Entity,
    body: RigidBodyHandle,
    collider: Option<ColliderHandle>,
    rigid_body: RigidBody2D,
    transform: Transform,
    box_collider: Option<BoxCollider2D>,
    /// How far the body was turned, as rapier gives it, from which the next step's turn is
    /// written into the transform.
    angle: f32,
}

impl Default for Physics2D {
    fn default() -> Self {
        Self {
            gravity: Vec2::new(0.0, -9.81),
            rapier: Rapier {
                bodies: RigidBodySet::new(),
                colliders: ColliderSet::new(),
                islands: IslandManager::new(),
                broad_phase: BroadPhaseBvh::new(),
                narrow_phase: NarrowPhase::new(),
                impulse_joints: ImpulseJointSet::new(),
                multibody_joints: MultibodyJointSet::new(),
                soft_bodies: SoftBodySet::new(),
                ccd: CCDSolver::new(),
                parameters: IntegrationParameters {
                    dt: Tick::DT.as_secs_f32(),
                    ..IntegrationParameters::default()
                },
            },
            pipeline: PhysicsPipeline::new(),
            linked: Vec::new(),
            by_slot: Vec::new(),
            found: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl Clone for Physics2D {
    fn clone(&self) -> Self {
        Self {
            gravity: self.gravity,
            rapier: self.rapier.clone(),
            pipeline: PhysicsPipeline::new(),
            linked: self.linked.clone(),
            by_slot: self.by_slot.clone(),
            found: Vec::new(),
            children: self.children.clone(),
        }
    }

    /// Copies `source` into the memory this one holds, as far as rapier's types copy so, keeping
    /// its own workspace.
    fn clone_from(&mut self, source: &Self) {
        self.gravity = source.gravity;
        self.rapier.clone_from(&source.rapier);
        self.linked.clone_from(&source.linked);
        self.by_slot.clone_from(&source.by_slot);
        self.children.clone_from(&source.children);
    }
}

/// Steps `world`'s 2D physics a tick, [`Tick::DT`], as a system a layer's tick runs, after what
/// sets its bodies' velocities, such as scripts: it takes in what changed since its last step,
/// making a body for each entity that has come to have a [`RigidBody2D`] and a [`Transform`], in
/// the order queries find them, and taking out each whose entity has gone or lost either; and takes
/// a change to a body's components that the last step didn't make, a game's or an edit's, as where
/// the body is, how fast it goes, how it moves or its collider. Then it moves the bodies, and writes
/// where each went and how fast into its components: its translation across and up, its z as it
/// was; its rotation turned about Z as far as the body turned, so that what turns it otherwise
/// stays; and its velocities. A world without a [`Physics2D`] is given one first.
pub fn step_physics(world: &mut World) {
    let _zone = gg_core::zone!();
    if world.resource::<Physics2D>().is_none() {
        world.insert_resource(Physics2D::default());
    }
    world.resource_scope(|world, physics: &mut Physics2D| physics.step(world));
}

impl Physics2D {
    fn step(&mut self, world: &mut World) {
        self.unlink_gone(world);
        self.link(world);
        let Rapier {
            bodies,
            colliders,
            islands,
            broad_phase,
            narrow_phase,
            impulse_joints,
            multibody_joints,
            soft_bodies,
            ccd,
            parameters,
        } = &mut self.rapier;
        self.pipeline.step(
            vector(self.gravity),
            parameters,
            islands,
            broad_phase,
            narrow_phase,
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            soft_bodies,
            ccd,
            &(),
            &(),
        );
        self.write_back(world);
    }

    /// Takes out each body whose entity has gone, lost its `RigidBody2D` or its `Transform`, or
    /// come under a parent; the rest keep their order.
    fn unlink_gone(&mut self, world: &World) {
        let before = self.linked.len();
        let rapier = &mut self.rapier;
        self.linked.retain(|linked| {
            let entity = linked.entity;
            let has = world.get::<RigidBody2D>(entity).is_some()
                && world.get::<Transform>(entity).is_some();
            if has && matches!(world.parent(entity), Ok(None)) {
                return true;
            }
            rapier.bodies.remove(
                linked.body,
                &mut rapier.islands,
                &mut rapier.colliders,
                &mut rapier.impulse_joints,
                &mut rapier.multibody_joints,
                &mut rapier.soft_bodies,
                true,
            );
            false
        });
        if self.linked.len() != before {
            self.by_slot.fill(NONE);
            for (index, linked) in (0..).zip(&self.linked) {
                self.by_slot[linked.entity.slot()] = index;
            }
        }
    }

    /// Makes a body for each entity with a `RigidBody2D` and a `Transform` that has none, but those
    /// under a parent, which it warns of once each; and takes in what changed of each other's.
    fn link(&mut self, world: &World) {
        let mut found = std::mem::take(&mut self.found);
        found.clear();
        let bodies = world.query::<(Entity, &RigidBody2D, &Transform)>();
        found.extend(bodies.map(|(entity, ..)| entity));
        self.children.retain(|&child| {
            let body = world.get::<RigidBody2D>(child).is_some();
            body && matches!(world.parent(child), Ok(Some(_)))
        });
        for &entity in &found {
            if matches!(world.parent(entity), Ok(Some(_))) {
                if !self.children.contains(&entity) {
                    let id = world.id(entity).expect("found alive");
                    warn!(
                        code = "physics.child_body",
                        entity = %id,
                        "Entity {id}'s RigidBody2D isn't moved: it's under a parent, whose space \
                         its transform is in"
                    );
                    self.children.push(entity);
                }
                continue;
            }
            let index = self.by_slot.get(entity.slot()).copied().unwrap_or(NONE);
            match self.linked.get_mut(index as usize) {
                Some(linked) if linked.entity == entity => {
                    Self::take_changes(&mut self.rapier, linked, world);
                }
                _ => self.make_body(world, entity),
            }
        }
        self.found = found;
    }

    /// Makes `entity`'s body, as its components say.
    fn make_body(&mut self, world: &World, entity: Entity) {
        let rigid_body = *world.get::<RigidBody2D>(entity).expect("found with one");
        let transform = *world.get::<Transform>(entity).expect("found with one");
        let box_collider = world.get::<BoxCollider2D>(entity).copied();
        let mut built = RigidBodyBuilder::new(body_type(rigid_body.body_type))
            .translation(vector(transform.translation.truncate()))
            .rotation(angle(transform.rotation))
            .linvel(vector(rigid_body.velocity))
            .angvel(rigid_body.angular_velocity);
        if rigid_body.fixed_rotation {
            built = built.lock_rotations();
        }
        let rapier = &mut self.rapier;
        let body = rapier.bodies.insert(built);
        // As rapier gives it back, which may be another float than the one it was given.
        let angle = rapier.bodies[body].rotation().angle();
        let collider = box_collider.map(|box_collider| {
            let made = collider(&box_collider, transform.scale);
            rapier
                .colliders
                .insert_with_parent(made, body, &mut rapier.bodies)
        });
        let index = u32::try_from(self.linked.len()).expect("fewer than 2^32 bodies");
        if self.by_slot.len() <= entity.slot() {
            self.by_slot.resize(entity.slot() + 1, NONE);
        }
        self.by_slot[entity.slot()] = index;
        self.linked.push(Linked {
            entity,
            body,
            collider,
            rigid_body,
            transform,
            box_collider,
            angle,
        });
    }

    /// Takes in what changed of `linked`'s components since the last step left them, which the
    /// last step didn't make, into its body: waking it, so that it moves from there.
    fn take_changes(rapier: &mut Rapier, linked: &mut Linked, world: &World) {
        let entity = linked.entity;
        let rigid_body = *world.get::<RigidBody2D>(entity).expect("found with one");
        let transform = *world.get::<Transform>(entity).expect("found with one");
        let box_collider = world.get::<BoxCollider2D>(entity).copied();
        let body = &mut rapier.bodies[linked.body];
        let was = linked.rigid_body;
        if rigid_body.body_type != was.body_type {
            body.set_body_type(body_type(rigid_body.body_type), true);
        }
        if rigid_body.fixed_rotation != was.fixed_rotation {
            body.lock_rotations(rigid_body.fixed_rotation, true);
        }
        if rigid_body.velocity != was.velocity {
            body.set_linvel(vector(rigid_body.velocity), true);
        }
        if rigid_body.angular_velocity != was.angular_velocity {
            body.set_angvel(rigid_body.angular_velocity, true);
        }
        let at = transform.translation.truncate();
        if at != linked.transform.translation.truncate() {
            body.set_translation(vector(at), true);
        }
        if transform.rotation != linked.transform.rotation {
            body.set_rotation(Rotation::new(angle(transform.rotation)), true);
            linked.angle = body.rotation().angle();
        }
        let rescaled = transform.scale.truncate() != linked.transform.scale.truncate();
        if box_collider != linked.box_collider || rescaled {
            if let Some(collider) = linked.collider.take() {
                rapier.colliders.remove(
                    collider,
                    &mut rapier.islands,
                    &mut rapier.bodies,
                    &mut rapier.soft_bodies,
                    true,
                );
            }
            linked.collider = box_collider.map(|box_collider| {
                let made = collider(&box_collider, transform.scale);
                rapier
                    .colliders
                    .insert_with_parent(made, linked.body, &mut rapier.bodies)
            });
        }
        linked.rigid_body = rigid_body;
        linked.transform = transform;
        linked.box_collider = box_collider;
    }

    /// Writes where each body that moves went, and how fast it goes, into its entity's components.
    fn write_back(&mut self, world: &mut World) {
        for linked in &mut self.linked {
            let body = &self.rapier.bodies[linked.body];
            if body.body_type() == RigidBodyType::Fixed {
                continue;
            }
            let at = body.translation();
            let turned = body.rotation().angle();
            let (velocity, angular_velocity) = (body.linvel(), body.angvel());
            let transform = world.get_mut::<Transform>(linked.entity);
            let transform = transform.expect("linked this step: alive, with one");
            transform.translation.x = at.x;
            transform.translation.y = at.y;
            if turned != linked.angle {
                let by = Quat::from_rotation_z(turned - linked.angle);
                transform.rotation = by * transform.rotation;
                linked.angle = turned;
            }
            linked.transform = *transform;
            let rigid_body = world.get_mut::<RigidBody2D>(linked.entity);
            let rigid_body = rigid_body.expect("linked this step: alive, with one");
            rigid_body.velocity = Vec2::new(velocity.x, velocity.y);
            rigid_body.angular_velocity = angular_velocity;
            linked.rigid_body = *rigid_body;
        }
    }
}

/// rapier's kind of body for `body_type`: kinematic by its velocity, as Box2D's kinematic bodies go.
fn body_type(body_type: BodyType) -> RigidBodyType {
    match body_type {
        BodyType::Dynamic => RigidBodyType::Dynamic,
        BodyType::Kinematic => RigidBodyType::KinematicVelocityBased,
        BodyType::Static => RigidBodyType::Fixed,
    }
}

/// The collider `box_collider` makes, for an entity of `scale`: its size and its offset scaled
/// across and up, a mirrored entity's box the same.
fn collider(box_collider: &BoxCollider2D, scale: Vec3) -> Collider {
    let scale = scale.truncate();
    let half = (box_collider.size * scale).abs() * 0.5;
    ColliderBuilder::cuboid(half.x, half.y)
        .translation(vector(box_collider.offset * scale))
        .density(box_collider.density)
        .friction(box_collider.friction)
        .restitution(box_collider.restitution)
        .build()
}

/// How far `rotation` turns +X about Z, counterclockwise as seen from +Z: how an entity is turned
/// in the XY plane, the turn 2D physics has.
fn angle(rotation: Quat) -> f32 {
    let x = rotation * Vec3::X;
    libm::atan2f(x.y, x.x)
}

/// `v` as rapier's, whose glam is another.
fn vector(v: Vec2) -> Vector {
    Vector::new(v.x, v.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor 10 units wide and a unit tall, its top at y 0.5, static.
    fn floor(world: &mut World) -> Entity {
        let at = Transform {
            scale: Vec3::new(10.0, 1.0, 1.0),
            ..Transform::IDENTITY
        };
        let body = RigidBody2D {
            body_type: BodyType::Static,
            ..RigidBody2D::default()
        };
        world.spawn((at, body, BoxCollider2D::default()))
    }

    /// A dynamic unit box at `x`, `y`, z 2, turned `angle` about Z.
    fn crate_at(world: &mut World, x: f32, y: f32, angle: f32) -> Entity {
        let at = Transform {
            translation: Vec3::new(x, y, 2.0),
            rotation: Quat::from_rotation_z(angle),
            ..Transform::IDENTITY
        };
        world.spawn((at, RigidBody2D::default(), BoxCollider2D::default()))
    }

    /// Steps `world`'s physics `ticks` times.
    fn step(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            step_physics(world);
        }
    }

    fn transform(world: &World, entity: Entity) -> Transform {
        *world.get::<Transform>(entity).expect("alive, with one")
    }

    /// A box dropped onto a floor falls, lands and comes to rest on it, its z and its scale as
    /// they were; the floor, static, stays where it is, its components as they were.
    #[test]
    fn a_box_falls_onto_a_floor_and_rests_on_it() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let dropped = crate_at(&mut world, 0.0, 3.0, 0.0);
        step(&mut world, 16);
        let falling = transform(&world, dropped);
        assert!(falling.translation.y < 3.0, "{falling:?}");
        let body = *world.get::<RigidBody2D>(dropped).expect("alive");
        assert!(body.velocity.y < -1.0, "{body:?}");
        step(&mut world, 240);
        let rested = transform(&world, dropped);
        assert!(
            (rested.translation.y - 1.0).abs() < 0.02,
            "on the floor's top: {rested:?}"
        );
        assert_eq!(rested.translation.z, 2.0);
        assert_eq!(rested.scale, Vec3::ONE);
        let body = *world.get::<RigidBody2D>(dropped).expect("alive");
        assert!(body.velocity.length() < 0.01, "at rest: {body:?}");
        assert_eq!(
            transform(&world, floor),
            Transform {
                scale: Vec3::new(10.0, 1.0, 1.0),
                ..Transform::IDENTITY
            }
        );
    }

    /// A pile of boxes, turned and overlapping as they start, over a floor.
    fn pile(world: &mut World) -> Vec<Entity> {
        floor(world);
        let mut boxes = Vec::new();
        for i in 0..12u8 {
            let i = f32::from(i);
            let x = (i % 4.0) * 0.8 - 1.2;
            let y = 1.5 + i * 0.7;
            boxes.push(crate_at(world, x, y, i * 0.3));
        }
        boxes
    }

    /// A snapshot of a world whose bodies are moving and touching, restored, steps as the world it
    /// was taken from did, bit for bit, and so does a copy, though neither has rapier's workspace:
    /// the world holds all that physics carries from one step to the next, which bodies made again
    /// from their components alone lack.
    #[test]
    fn a_snapshot_steps_as_its_world_did() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 40);
        let snapshot = world.clone();
        step(&mut world, 60);
        let went = world.hash();
        let mut copy = snapshot.clone();
        step(&mut copy, 60);
        assert_eq!(copy.hash(), went, "a copy");
        world.clone_from(&snapshot);
        step(&mut world, 60);
        assert_eq!(world.hash(), went, "restored");
        let mut remade = snapshot.clone();
        remade.remove_resource::<Physics2D>();
        step(&mut remade, 60);
        assert_ne!(remade.hash(), went, "its bodies made again");
    }

    /// The world's hash after physics has stepped a pile of boxes for four seconds, which CI checks
    /// on every platform: a difference on some platforms only is a bug in determinism, not a value
    /// to change here.
    #[test]
    fn a_pile_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0x7adc_67d3_cc48_b6b7, "{content:#018x}");
    }

    /// What a game or an edit changes of a body's components between steps, the next step takes:
    /// where it is and how it's turned, how fast it goes, how it moves, and its collider.
    #[test]
    fn changes_between_steps_reach_the_bodies() {
        let mut world = World::default();
        floor(&mut world);
        let moved = crate_at(&mut world, 0.0, 1.0, 0.0);
        step(&mut world, 4);
        world
            .get_mut::<Transform>(moved)
            .expect("alive")
            .translation
            .x = 3.0;
        step(&mut world, 1);
        assert_eq!(transform(&world, moved).translation.x, 3.0);
        world.get_mut::<RigidBody2D>(moved).expect("alive").velocity = Vec2::new(0.0, 8.0);
        step(&mut world, 1);
        assert!(transform(&world, moved).translation.y > 1.1);
        let rigid_body = world.get_mut::<RigidBody2D>(moved).expect("alive");
        rigid_body.body_type = BodyType::Static;
        let held = transform(&world, moved);
        step(&mut world, 30);
        assert_eq!(transform(&world, moved), held, "static: where it was put");
        let rigid_body = world.get_mut::<RigidBody2D>(moved).expect("alive");
        rigid_body.body_type = BodyType::Dynamic;
        rigid_body.velocity = Vec2::ZERO;
        let collider = world.get_mut::<BoxCollider2D>(moved).expect("alive");
        collider.size = Vec2::new(1.0, 3.0);
        step(&mut world, 240);
        let rested = transform(&world, moved);
        assert!(
            (rested.translation.y - 2.0).abs() < 0.02,
            "on the floor, 3 tall: {rested:?}"
        );
        let turned = Quat::from_rotation_z(0.5);
        world.get_mut::<Transform>(moved).expect("alive").rotation = turned;
        step(&mut world, 1);
        let rotation = transform(&world, moved).rotation;
        assert!(rotation.angle_between(turned) < 0.05, "{rotation:?}");
    }

    /// A body goes as its entity does, or as it loses its `RigidBody2D`: what rested on it falls,
    /// and its entity stays as it was. A world cleared keeps its physics, without bodies.
    #[test]
    fn bodies_go_with_their_entities() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let dropped = crate_at(&mut world, 0.0, 1.0, 0.0);
        let stripped = crate_at(&mut world, 3.0, 1.0, 0.0);
        step(&mut world, 30);
        let resting = transform(&world, dropped).translation.y;
        world.despawn(floor).expect("alive");
        world.remove::<RigidBody2D>(stripped).expect("alive");
        let held = transform(&world, stripped);
        step(&mut world, 30);
        assert!(transform(&world, dropped).translation.y < resting - 0.5);
        assert_eq!(transform(&world, stripped), held);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert_eq!(physics.linked.len(), 1);
        assert_eq!(physics.rapier.bodies.len(), 1);
        world.clear();
        step(&mut world, 1);
        let physics = world.resource::<Physics2D>().expect("kept");
        assert_eq!(physics.rapier.bodies.len(), 0);
        assert_eq!(physics.rapier.colliders.len(), 0);
    }

    /// A body under a parent isn't moved, and is warned of once; a root again, it falls.
    #[test]
    fn a_body_under_a_parent_isnt_moved() {
        let mut world = World::default();
        let parent = world.spawn(Transform::IDENTITY);
        let child = crate_at(&mut world, 0.0, 1.0, 0.0);
        world.set_parent(child, Some(parent)).expect("no cycle");
        let held = transform(&world, child);
        step(&mut world, 10);
        assert_eq!(transform(&world, child), held);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert_eq!(physics.children, [child], "warned once");
        assert!(physics.linked.is_empty());
        world.set_parent(child, None).expect("a root");
        step(&mut world, 10);
        assert!(transform(&world, child).translation.y < held.translation.y);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert!(physics.children.is_empty());
    }

    /// A kinematic body goes by its velocity alone, gravity aside, and pushes a dynamic box in its
    /// way; a dynamic box whose rotation is fixed doesn't tip off the floor's edge, where one
    /// whose rotation isn't does.
    #[test]
    fn kinematic_bodies_push_and_fixed_rotations_stay() {
        let mut world = World::default();
        floor(&mut world);
        let pusher = world.spawn((
            Transform {
                translation: Vec3::new(-3.0, 1.0, 0.0),
                ..Transform::IDENTITY
            },
            RigidBody2D {
                body_type: BodyType::Kinematic,
                velocity: Vec2::new(2.0, 0.0),
                ..RigidBody2D::default()
            },
            BoxCollider2D::default(),
        ));
        let pushed = crate_at(&mut world, -1.0, 1.0, 0.0);
        step(&mut world, 64);
        let at = transform(&world, pusher).translation;
        assert!((at.x - -1.0).abs() < 1e-4 && at.y == 1.0, "{at}");
        // Pushed half a unit, a unit in front of the pusher, touching it.
        let ahead = transform(&world, pushed).translation.x;
        assert!((ahead - at.x - 1.0).abs() < 0.02, "{ahead}");
        let mut world = World::default();
        floor(&mut world);
        let tipping = crate_at(&mut world, 5.2, 1.0, 0.0);
        let upright = crate_at(&mut world, -5.2, 1.0, 0.0);
        world
            .get_mut::<RigidBody2D>(upright)
            .expect("alive")
            .fixed_rotation = true;
        step(&mut world, 120);
        assert!(angle(transform(&world, tipping).rotation).abs() > 0.3);
        assert_eq!(transform(&world, upright).rotation, Quat::IDENTITY);
    }
}
