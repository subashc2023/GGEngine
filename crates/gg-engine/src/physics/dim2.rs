//! 2D physics: bodies that rapier moves at the world's ticks, in the XY plane, as their entities'
//! components describe them, and the world's resource that holds rapier's bodies and contacts,
//! which snapshots copy and snapshot files hold.

use std::collections::HashSet;

use gg_core::math::{Mat4, Quat, Vec2, Vec3, libm};
use rapier2d::prelude::{
    BroadPhaseBvh, CCDSolver, Collider, ColliderBuilder, ColliderHandle, ColliderSet,
    ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase,
    PhysicsPipeline, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, RigidBodyType, Rotation,
    SoftBodySet, Vector,
};
use serde::{Deserialize, Serialize};

use super::{BodyType, Finite, Link, Links, Outline, kept, kept_collider, unfinite};
use crate::draw::Poses;
use crate::recording::text;
use crate::{Component, Entity, EntityId, Error, Reflect, Result, Saved, Tick, Transform, World};

mod joint;

pub(super) use joint::joint_lines;
pub use joint::{Joint2D, JointKind2D};
use joint::{Jointed, SavedJoint};

/// A body that 2D physics moves, at each tick a layer runs [`step_physics`](crate::step_physics)
/// in: in the XY plane, its entity's [`Transform`] across and up and turned about Z, and the
/// velocities here, which each step reads and writes back. It collides as its entity's
/// [`BoxCollider2D`] and [`CircleCollider2D`], whichever it has, which give a dynamic body its mass
/// too, both together where it has both; and [`Joint2D`]s hold it to other bodies. A body under a
/// parent isn't moved, since its transform is in its parent's space (`physics.child_body`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct RigidBody2D {
    /// How it moves: dynamic, by its velocity, which gravity and what it hits change; kinematic, by
    /// its velocity alone, pushing what's dynamic aside, as a moving platform does; or static, not
    /// at all, as a floor or a wall.
    pub body_type: BodyType,
    /// Whether it never turns, as a character that stays upright.
    pub fixed_rotation: bool,
    /// Whether a step that it goes fast in sweeps it along its way against what moves too, rapier's
    /// continuous collision, so that it stops at a kinematic or dynamic body that it would
    /// otherwise pass through between one step and the next, as a small fast ball passes through a
    /// thin moving wall. Each dynamic body that goes fast is swept against what's static anyway,
    /// and one with this against all but others with it, as rapier sweeps bullets, at some cost.
    pub continuous_collision: bool,
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

impl BoxCollider2D {
    /// The collider it makes for an entity of `scale`: its size and its offset scaled across and
    /// up, a mirrored entity's box the same.
    fn collider(&self, scale: Vec3) -> Collider {
        let scale = scale.truncate();
        let half = (self.size * scale).abs() * 0.5;
        ColliderBuilder::cuboid(half.x, half.y)
            .translation(vector(self.offset * scale))
            .density(self.density)
            .friction(self.friction)
            .restitution(self.restitution)
            .build()
    }
}

/// The circle a [`RigidBody2D`] on the same entity collides as, in the entity's space: around its
/// offset, which turns and scales with the entity, its radius scaled by the larger of the entity's
/// scales across and up, since a circle stays one however its entity stretches. By default half a
/// unit, so that it's a unit across, as a circle drawn on a sprite's square is.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct CircleCollider2D {
    /// Where its middle is, from its entity's origin.
    pub offset: Vec2,
    /// How far its edge is from its middle.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
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

impl Default for CircleCollider2D {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            radius: 0.5,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
        }
    }
}

impl CircleCollider2D {
    /// The collider it makes for an entity of `scale`: its offset scaled across and up, and its
    /// radius by the larger of the two scales.
    fn collider(&self, scale: Vec3) -> Collider {
        let scale = scale.truncate();
        ColliderBuilder::ball(self.radius * scale.abs().max_element())
            .translation(vector(self.offset * scale))
            .density(self.density)
            .friction(self.friction)
            .restitution(self.restitution)
            .build()
    }
}

/// The world's 2D physics, a [`Resource`](crate::Resource) that
/// [`step_physics`](crate::step_physics) gives a world the first time it steps one with a
/// [`RigidBody2D`] and without it, or that a game inserts first, with its gravity: rapier's bodies,
/// colliders, joints and contacts, which each step carries on from, and each body's and joint's
/// entity, so that a snapshot restored steps as its world did. A world cleared, or whose entities a
/// scene replaced, keeps it, its bodies and joints going at the next step, as their entities have.
pub struct Physics2D {
    /// Which way everything dynamic falls, and how much faster it goes each second, in units a
    /// second: down at 9.81 by default, Earth's, as though a unit were a meter.
    pub gravity: Vec2,
    rapier: Rapier,
    /// rapier's workspace, which a copy makes anew, since no step carries anything in it over.
    pipeline: PhysicsPipeline,
    links: Links<Linked>,
    joints: Links<Jointed>,
}

/// What rapier keeps from one step to the next.
#[derive(Clone, Serialize, Deserialize)]
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
    /// The colliders of its entity's `BoxCollider2D` and `CircleCollider2D`, in that order, of
    /// those it has.
    colliders: [Option<ColliderHandle>; 2],
    rigid_body: RigidBody2D,
    transform: Transform,
    box_collider: Option<BoxCollider2D>,
    circle_collider: Option<CircleCollider2D>,
    /// How far the body was turned, as rapier gives it, from which the next step's turn is
    /// written into the transform.
    angle: f32,
}

impl Link for Linked {
    fn entity(&self) -> Entity {
        self.entity
    }
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
            links: Links::default(),
            joints: Links::default(),
        }
    }
}

impl Clone for Physics2D {
    fn clone(&self) -> Self {
        Self {
            gravity: self.gravity,
            rapier: self.rapier.clone(),
            pipeline: PhysicsPipeline::new(),
            links: self.links.clone(),
            joints: self.joints.clone(),
        }
    }

    /// Copies `source` into the memory this one holds, as far as rapier's types copy so, but for
    /// rapier's workspace: its pipeline keeps the joints it last solved and its solver's layout of
    /// them, by counts that the joint set copied brings back, so that, kept, it would solve another
    /// world's joints, or ones gone, at the next step. A new one, as a copy has, costs its
    /// workspace's memory made again as that step goes.
    fn clone_from(&mut self, source: &Self) {
        self.gravity = source.gravity;
        self.rapier.clone_from(&source.rapier);
        self.pipeline = PhysicsPipeline::new();
        self.links.clone_from(&source.links);
        self.joints.clone_from(&source.joints);
    }
}

/// What a snapshot file holds of [`Physics2D`], `R` its rapier's state, as its own or borrowed: its
/// gravity; rapier's state, through rapier's own serialization, which carries on exactly; each body
/// and each joint, with its entity by its ID; and the bodies under a parent warned of, by their
/// entities' IDs. Text that lists no joints reads as having none, so that a world saved without
/// them loads.
#[derive(Serialize, Deserialize)]
struct SavedPhysics<R> {
    gravity: [f32; 2],
    rapier: R,
    linked: Vec<SavedLinked>,
    children: Vec<Option<u64>>,
    #[serde(default)]
    joints: Vec<SavedJoint>,
}

/// A body as a snapshot file holds it: its entity's ID, none for an entity gone since the last step,
/// which the next takes the body out for; its handles; what the last step left in its entity's
/// components, each as a recording's text writes values, so that a component whose type changed
/// is matched to it as the component is; and how far it was turned.
#[derive(Serialize, Deserialize)]
struct SavedLinked {
    entity: Option<u64>,
    body: RigidBodyHandle,
    colliders: [Option<ColliderHandle>; 2],
    rigid_body: String,
    transform: String,
    box_collider: Option<String>,
    circle_collider: Option<String>,
    angle: f32,
}

/// Saved through rapier's own serialization, written as RON, so that a world restored in another
/// process steps on as the one saved would have: its contacts' warm starts, its islands' sleep and
/// its handles as they were, which bodies made again from their components would lose. A rapier of
/// another version may not read it, which leaves physics out, to be made again from the entities,
/// as does text whose bodies name a rigid body or a collider, or whose joints a joint, that its
/// rapier's state doesn't hold as theirs (`physics.load`).
impl Saved for Physics2D {
    const NAME: &'static str = "Physics2D";

    fn save(&self, world: &World) -> Result<String> {
        let id = |entity: Entity| world.id(entity).map(EntityId::get);
        let value = |component: &dyn Reflect| {
            let mut out = String::new();
            text::write(&mut out, &component.to_value(), world);
            out
        };
        let linked = self.links.linked.iter().map(|linked| SavedLinked {
            entity: id(linked.entity),
            body: linked.body,
            colliders: linked.colliders,
            rigid_body: value(&linked.rigid_body),
            transform: value(&linked.transform),
            box_collider: linked.box_collider.as_ref().map(|it| value(it)),
            circle_collider: linked.circle_collider.as_ref().map(|it| value(it)),
            angle: linked.angle,
        });
        let joints = self.joints.linked.iter().map(|jointed| SavedJoint {
            entity: id(jointed.entity),
            handle: jointed.handle,
            bodies: jointed.bodies,
            scales: jointed.scales.map(|scale| scale.to_array()),
            joint: value(&jointed.joint),
            warned: jointed.warned.clone(),
        });
        let saved = SavedPhysics {
            gravity: self.gravity.to_array(),
            rapier: &self.rapier,
            linked: linked.collect(),
            children: self.links.children.iter().map(|&child| id(child)).collect(),
            joints: joints.collect(),
        };
        ron::to_string(&saved).map_err(|error| Error::new("physics.save", error.to_string()))
    }

    fn load(text: &str, world: &World) -> Result<Self> {
        let saved: SavedPhysics<Rapier> =
            ron::from_str(text).map_err(|error| Error::new("physics.load", error.to_string()))?;
        let entity = |id: Option<u64>| {
            let id = id.and_then(EntityId::new);
            id.and_then(|id| world.entity(id)).unwrap_or(Entity::DEAD)
        };
        let mut linked = Vec::with_capacity(saved.linked.len());
        for body in saved.linked {
            let boxed = body.box_collider.as_ref();
            let circled = body.circle_collider.as_ref();
            linked.push(Linked {
                entity: entity(body.entity),
                body: body.body,
                colliders: body.colliders,
                rigid_body: world.component_from_text(&body.rigid_body)?,
                transform: world.component_from_text(&body.transform)?,
                box_collider: boxed
                    .map(|text| world.component_from_text(text))
                    .transpose()?,
                circle_collider: circled
                    .map(|text| world.component_from_text(text))
                    .transpose()?,
                angle: body.angle,
            });
        }
        // What a step reaches by the links' handles, which text from elsewhere, a host's or a
        // server's, may name otherwise than rapier's sets hold: each body there and linked once,
        // its colliders on it, and each entity linked once.
        let refused = |entity: Entity, what: &str| {
            let who = world.id(entity);
            let who = who.map_or_else(|| "an entity gone".to_owned(), |id| format!("entity {id}"));
            Error::new("physics.load", format!("{who}'s body {what}"))
        };
        let rapier = &saved.rapier;
        let mut bodies = HashSet::with_capacity(linked.len());
        for linked in &linked {
            if rapier.bodies.get(linked.body).is_none() || !bodies.insert(linked.body) {
                let what = "names no rigid body of rapier's, or another body's";
                return Err(refused(linked.entity, what));
            }
            let on = |collider: &ColliderHandle| {
                rapier.colliders.get(*collider).and_then(Collider::parent) == Some(linked.body)
            };
            if !linked.colliders.iter().flatten().all(on) {
                let what = "names a collider that isn't on its rigid body";
                return Err(refused(linked.entity, what));
            }
        }
        // A joint's handle, which a step reaches rapier's joint by: there, and linked once.
        let refused_joint = |entity: Entity, what: &str| {
            let who = world.id(entity);
            let who = who.map_or_else(|| "an entity gone".to_owned(), |id| format!("entity {id}"));
            Error::new("physics.load", format!("{who}'s joint {what}"))
        };
        let mut joints = Vec::with_capacity(saved.joints.len());
        let mut handles = HashSet::with_capacity(saved.joints.len());
        for joint in saved.joints {
            let entity = entity(joint.entity);
            if let Some(handle) = joint.handle
                && (!rapier.impulse_joints.contains(handle) || !handles.insert(handle))
            {
                let what = "names no joint of rapier's, or another joint's";
                return Err(refused_joint(entity, what));
            }
            joints.push(Jointed {
                entity,
                handle: joint.handle,
                bodies: joint.bodies,
                scales: joint.scales.map(Vec2::from_array),
                joint: world.component_from_text(&joint.joint)?,
                warned: joint.warned,
            });
        }
        let children = saved.children.into_iter().map(entity).collect();
        let links = Links::new(linked, children);
        let links = links.map_err(|twice| refused(twice, "is its second"))?;
        let joints = Links::new(joints, Vec::new());
        let joints = joints.map_err(|twice| refused_joint(twice, "is its second"))?;
        Ok(Self {
            gravity: Vec2::from_array(saved.gravity),
            rapier: saved.rapier,
            pipeline: PhysicsPipeline::new(),
            links,
            joints,
        })
    }
}

/// Steps `world`'s 2D physics a tick, as [`step_physics`](crate::step_physics) describes, giving it
/// a [`Physics2D`] first if it has a [`RigidBody2D`] and none.
pub(super) fn step(world: &mut World) {
    if world.resource::<Physics2D>().is_none() {
        if world.query::<&RigidBody2D>().next().is_none() {
            return;
        }
        world.insert_resource(Physics2D::default());
    }
    world.resource_scope(|world, physics: &mut Physics2D| physics.step(world));
}

impl Physics2D {
    /// Takes in what changed, the joints after the bodies they join, then steps rapier, and writes
    /// back where everything went.
    fn step(&mut self, world: &mut World) {
        self.unlink_gone(world);
        let rapier = &mut self.rapier;
        let refused = |entity| {
            (unfinite::<Transform>(world, entity, "Transform"))
                .or_else(|| unfinite::<RigidBody2D>(world, entity, "RigidBody2D"))
                .or_else(|| unfinite::<BoxCollider2D>(world, entity, "BoxCollider2D"))
                .or_else(|| unfinite::<CircleCollider2D>(world, entity, "CircleCollider2D"))
        };
        self.links
            .find::<RigidBody2D>(world, "RigidBody2D", refused, |links, entity| {
                match links.get_mut(entity) {
                    Some(linked) => take_changes(rapier, linked, world),
                    None => links.push(make_body(rapier, world, entity)),
                }
            });
        self.join(world);
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
        let rapier = &mut self.rapier;
        self.links.retain(|linked| {
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
    }

    /// Writes where each body that moves went, how it's turned, and how fast it goes, into its
    /// entity's components: each only as it changed, to the bit, so that a body at rest, as one
    /// asleep, leaves its entity's rows unchanged, as the hash's incremental parts and the open
    /// scene's check read them. It writes from what the step took in, which a transform refused
    /// leaves as it was, so that the refused one's NaNs aren't carried into what it writes.
    fn write_back(&mut self, world: &mut World) {
        for linked in &mut self.links.linked {
            let body = &self.rapier.bodies[linked.body];
            if body.body_type() == RigidBodyType::Fixed {
                continue;
            }
            let at = body.translation();
            let turned = body.rotation().angle();
            let (velocity, angular_velocity) = (body.linvel(), body.angvel());
            let was = &mut linked.transform;
            let bits = |a: f32, b: f32| a.to_bits() != b.to_bits();
            let moved = bits(at.x, was.translation.x) || bits(at.y, was.translation.y);
            if moved || turned != linked.angle {
                if turned != linked.angle {
                    let by = Quat::from_rotation_z(turned - linked.angle);
                    was.rotation = by * was.rotation;
                    linked.angle = turned;
                }
                was.translation.x = at.x;
                was.translation.y = at.y;
                let transform = world.get_mut::<Transform>(linked.entity);
                let transform = transform.expect("linked this step: alive, with one");
                transform.translation.x = at.x;
                transform.translation.y = at.y;
                transform.rotation = was.rotation;
            }
            let went = &mut linked.rigid_body;
            let sped = bits(velocity.x, went.velocity.x) || bits(velocity.y, went.velocity.y);
            if sped || bits(angular_velocity, went.angular_velocity) {
                went.velocity = Vec2::new(velocity.x, velocity.y);
                went.angular_velocity = angular_velocity;
                let rigid_body = world.get_mut::<RigidBody2D>(linked.entity);
                let rigid_body = rigid_body.expect("linked this step: alive, with one");
                rigid_body.velocity = went.velocity;
                rigid_body.angular_velocity = angular_velocity;
            }
        }
    }
}

/// Makes `entity`'s body in `rapier`, as its components say, and gives back its link.
fn make_body(rapier: &mut Rapier, world: &World, entity: Entity) -> Linked {
    let rigid_body = *world.get::<RigidBody2D>(entity).expect("found with one");
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let box_collider = world.get::<BoxCollider2D>(entity).copied();
    let circle_collider = world.get::<CircleCollider2D>(entity).copied();
    let mut built = RigidBodyBuilder::new(body_type(rigid_body.body_type))
        .translation(vector(transform.translation.truncate()))
        .rotation(angle(transform.rotation))
        .linvel(vector(rigid_body.velocity))
        .angvel(rigid_body.angular_velocity)
        .ccd_enabled(rigid_body.continuous_collision);
    if rigid_body.fixed_rotation {
        built = built.lock_rotations();
    }
    let body = rapier.bodies.insert(built);
    // As rapier gives it back, which may be another float than the one it was given.
    let angle = rapier.bodies[body].rotation().angle();
    let mut colliders = [None; 2];
    let made = [
        box_collider.map(|made| made.collider(transform.scale)),
        circle_collider.map(|made| made.collider(transform.scale)),
    ];
    for (handle, made) in colliders.iter_mut().zip(made) {
        replace(rapier, handle, made, body);
    }
    Linked {
        entity,
        body,
        colliders,
        rigid_body,
        transform,
        box_collider,
        circle_collider,
        angle,
    }
}

/// Takes in what changed of `linked`'s components since the last step left them, which the last
/// step didn't make, into its body: waking it, so that it moves from there. A component that isn't
/// finite is taken as it was, as [`Links`] refuses it.
fn take_changes(rapier: &mut Rapier, linked: &mut Linked, world: &World) {
    let entity = linked.entity;
    let rigid_body = *world.get::<RigidBody2D>(entity).expect("found with one");
    let rigid_body = kept(rigid_body, linked.rigid_body);
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let transform = kept(transform, linked.transform);
    let box_collider = world.get::<BoxCollider2D>(entity).copied();
    let box_collider = kept_collider(box_collider, linked.box_collider);
    let circle_collider = world.get::<CircleCollider2D>(entity).copied();
    let circle_collider = kept_collider(circle_collider, linked.circle_collider);
    let body = &mut rapier.bodies[linked.body];
    let was = linked.rigid_body;
    if rigid_body.body_type != was.body_type {
        body.set_body_type(body_type(rigid_body.body_type), true);
    }
    if rigid_body.fixed_rotation != was.fixed_rotation {
        body.lock_rotations(rigid_body.fixed_rotation, true);
    }
    if rigid_body.continuous_collision != was.continuous_collision {
        body.enable_ccd(rigid_body.continuous_collision);
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
    let [boxed, round] = &mut linked.colliders;
    if box_collider != linked.box_collider || rescaled {
        let made = box_collider.map(|made| made.collider(transform.scale));
        replace(rapier, boxed, made, linked.body);
    }
    if circle_collider != linked.circle_collider || rescaled {
        let made = circle_collider.map(|made| made.collider(transform.scale));
        replace(rapier, round, made, linked.body);
    }
    linked.rigid_body = rigid_body;
    linked.transform = transform;
    linked.box_collider = box_collider;
    linked.circle_collider = circle_collider;
}

impl Finite for RigidBody2D {
    fn finite(&self) -> bool {
        self.velocity.is_finite() && self.angular_velocity.is_finite()
    }
}

impl Finite for BoxCollider2D {
    fn finite(&self) -> bool {
        let numbers = [self.density, self.friction, self.restitution];
        self.offset.is_finite() && self.size.is_finite() && numbers.iter().all(|it| it.is_finite())
    }
}

impl Finite for CircleCollider2D {
    fn finite(&self) -> bool {
        let numbers = [self.radius, self.density, self.friction, self.restitution];
        self.offset.is_finite() && numbers.iter().all(|it| it.is_finite())
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

/// Takes the collider `handle` holds out of `rapier`, if it holds one, and puts `made` on `body` in
/// its place, if it's given, as `handle`'s.
fn replace(
    rapier: &mut Rapier,
    handle: &mut Option<ColliderHandle>,
    made: Option<Collider>,
    body: RigidBodyHandle,
) {
    if let Some(collider) = handle.take() {
        rapier.colliders.remove(
            collider,
            &mut rapier.islands,
            &mut rapier.bodies,
            &mut rapier.soft_bodies,
            true,
        );
    }
    *handle = made.map(|made| {
        rapier
            .colliders
            .insert_with_parent(made, body, &mut rapier.bodies)
    });
}

/// Gives `each` the outline of each [`BoxCollider2D`] and [`CircleCollider2D`] in `world` whose
/// entity has a [`Transform`], with the entity's ID, by table, then row: where its entity draws
/// this frame as `poses` has it, between ticks and carried by its ancestors, as physics shapes it,
/// a box sized and offset across and up as its entity scales, and a circle around its offset so
/// scaled, as large as its radius scaled by the larger of those scales. It outlines a collider
/// whether physics moves its entity or not, as it doesn't without a [`RigidBody2D`] or under a
/// parent, so that what's being made shows as it's made.
pub(super) fn outlines(world: &World, poses: &Poses<'_>, each: &mut impl FnMut(Outline, u64)) {
    let (Some(transform), boxed, round) = (
        world.component::<Transform>(),
        world.component::<BoxCollider2D>(),
        world.component::<CircleCollider2D>(),
    ) else {
        return;
    };
    for (index, table) in world.tables().iter().enumerate() {
        let boxes = boxed.and_then(|id| table.column::<BoxCollider2D>(id));
        let circles = round.and_then(|id| table.column::<CircleCollider2D>(id));
        if boxes.is_none() && circles.is_none() || !table.has(transform) {
            continue;
        }
        for (row, &entity) in table.entities().iter().enumerate() {
            let matrix = poses.matrix(index, row);
            let id = world.id(entity).expect("a table's rows are alive").get();
            if let Some(boxes) = boxes {
                let BoxCollider2D { offset, size, .. } = boxes[row];
                let placed = matrix
                    * Mat4::from_translation(offset.extend(0.0))
                    * Mat4::from_scale(size.extend(1.0));
                each(Outline::Rect(placed), id);
            }
            if let Some(circles) = circles {
                let CircleCollider2D { offset, radius, .. } = circles[row];
                let (scales, rotation, _) = matrix.to_scale_rotation_translation();
                let middle = matrix.transform_point3(offset.extend(0.0));
                let radius = radius * scales.truncate().abs().max_element();
                let placed = Mat4::from_scale_rotation_translation(
                    Vec3::new(radius, radius, 1.0),
                    rotation,
                    middle,
                );
                each(Outline::Circle(placed), id);
            }
        }
    }
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
    use crate::interpolation::Past;
    use crate::placed::Placed;
    use crate::step_physics;

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
    pub(super) fn crate_at(world: &mut World, x: f32, y: f32, angle: f32) -> Entity {
        let at = Transform {
            translation: Vec3::new(x, y, 2.0),
            rotation: Quat::from_rotation_z(angle),
            ..Transform::IDENTITY
        };
        world.spawn((at, RigidBody2D::default(), BoxCollider2D::default()))
    }

    /// Steps `world`'s physics `ticks` times.
    pub(super) fn step(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            step_physics(world);
        }
    }

    pub(super) fn transform(world: &World, entity: Entity) -> Transform {
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

    /// A body at rest, asleep, leaves its entity's components unchanged as the steps go on, so that
    /// what follows their changes, as the hash's incremental parts and the open scene's check do,
    /// has nothing to do for it; one moving changes them.
    #[test]
    fn bodies_at_rest_change_nothing() {
        let mut world = World::default();
        floor(&mut world);
        let dropped = crate_at(&mut world, 0.0, 1.2, 0.0);
        let pushed = crate_at(&mut world, 3.0, 1.0, 0.0);
        step(&mut world, 480);
        let physics = world.resource::<Physics2D>().expect("stepped");
        let linked = physics.links.get(dropped).expect("linked");
        assert!(physics.rapier.bodies[linked.body].is_sleeping());
        world
            .get_mut::<RigidBody2D>(pushed)
            .expect("alive")
            .velocity = Vec2::new(2.0, 0.0);
        world.advance();
        let tick = world.change_tick();
        step(&mut world, 8);
        let changed = |entity: Entity| {
            let mut tables = world.tables().iter();
            let table = tables.find(|table| table.entities().contains(&entity));
            let table = table.expect("in a table");
            let row = table.entities().iter().position(|&it| it == entity);
            let row = row.expect("a row");
            [
                world.component::<Transform>(),
                world.component::<RigidBody2D>(),
            ]
            .map(|id| {
                let changed = table.changed(id.expect("registered")).expect("a column");
                changed.row(row) >= tick
            })
        };
        assert_eq!(changed(dropped), [false, false], "at rest");
        assert_eq!(changed(pushed), [true, true], "moving");
    }

    /// A component set to a NaN or an infinity is refused, warned of once while it stays so
    /// (`physics.not_finite`): a body at rest whose transform isn't stays where it was, through 30
    /// steps that warn once, and the box touching it stays finite, as it would not were the NaN
    /// taken into rapier; set finite, it's taken. An angular velocity keeps the last that was, and
    /// a circle given with a radius that isn't is made only once it is. A body whose transform
    /// isn't finite as it comes isn't made until it is.
    #[test]
    fn what_isnt_finite_is_refused() {
        let logged = crate::testing::logged();
        let warned = || {
            let codes = logged.codes.try_iter();
            codes.filter(|code| code == "physics.not_finite").count()
        };
        let mut world = World::default();
        floor(&mut world);
        let blown = crate_at(&mut world, 0.0, 1.0, 0.0);
        let touching = crate_at(&mut world, 1.0, 1.0, 0.0);
        step(&mut world, 240);
        let held = transform(&world, blown);
        world
            .get_mut::<Transform>(blown)
            .expect("alive")
            .translation
            .x = f32::NAN;
        step(&mut world, 30);
        assert_eq!(warned(), 1, "once, though it stepped 30 times");
        assert!(
            transform(&world, blown).translation.x.is_nan(),
            "left to the game"
        );
        let physics = world.resource::<Physics2D>().expect("stepped");
        let body = &physics.rapier.bodies[physics.links.get(blown).expect("linked").body];
        assert!(
            body.is_sleeping() && body.translation().is_finite(),
            "where it was"
        );
        assert!((body.translation().x - held.translation.x).abs() < 1e-4);
        assert!(transform(&world, touching).translation.is_finite());
        // A velocity, and a circle's radius, each kept as it was.
        let rigid_body = world.get_mut::<RigidBody2D>(touching).expect("alive");
        rigid_body.angular_velocity = f32::INFINITY;
        let round = CircleCollider2D {
            radius: f32::NAN,
            ..CircleCollider2D::default()
        };
        world.insert(touching, round).expect("alive");
        step(&mut world, 30);
        assert_eq!(warned(), 1, "once for both");
        let physics = world.resource::<Physics2D>().expect("stepped");
        let linked = physics.links.get(touching).expect("linked");
        let body = &physics.rapier.bodies[linked.body];
        assert!(body.angvel().is_finite() && body.translation().is_finite());
        assert!(linked.colliders[1].is_none(), "no circle made");
        world
            .get_mut::<Transform>(blown)
            .expect("alive")
            .translation = Vec3::new(-2.0, 1.0, 2.0);
        step(&mut world, 1);
        let taken = transform(&world, blown).translation;
        assert!((taken.x - -2.0).abs() < 1e-3, "{taken}");
        let spawned = Transform {
            scale: Vec3::new(1.0, f32::INFINITY, 1.0),
            ..Transform::IDENTITY
        };
        let unmade = world.spawn((spawned, RigidBody2D::default(), BoxCollider2D::default()));
        step(&mut world, 4);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert!(physics.links.get(unmade).is_none(), "not made");
        assert_eq!(warned(), 1);
        world.get_mut::<Transform>(unmade).expect("alive").scale = Vec3::ONE;
        step(&mut world, 1);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert!(physics.links.get(unmade).is_some(), "made once it's finite");
    }

    /// A pile of boxes and balls, every third a ball, turned and overlapping as they start, over a
    /// floor.
    fn pile(world: &mut World) -> Vec<Entity> {
        floor(world);
        let mut bodies = Vec::new();
        for i in 0..12u8 {
            let i = f32::from(i);
            let x = (i % 4.0) * 0.8 - 1.2;
            let y = 1.5 + i * 0.7;
            let body = crate_at(world, x, y, i * 0.3);
            if i % 3.0 == 2.0 {
                world.remove::<BoxCollider2D>(body).expect("alive");
                (world.insert(body, CircleCollider2D::default())).expect("alive");
            }
            bodies.push(body);
        }
        bodies
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

    /// A world whose bodies are moving and touching, saved whole to text as a snapshot file holds
    /// it and loaded into a world of another process's making, steps as the world it was saved
    /// from did, bit for bit: physics' bodies and contacts come back through its saved state, and a
    /// change to a body's component since its last step is taken by the next, as it would have
    /// been.
    #[test]
    #[cfg(feature = "dev")]
    fn a_world_saved_whole_steps_as_it_did() {
        let mut world = World::seeded(7);
        world.register_saved::<Physics2D>().expect("saved");
        let bodies = pile(&mut world);
        step(&mut world, 40);
        // An edit since the last step, which the next takes.
        let pushed = world.get_mut::<RigidBody2D>(bodies[5]).expect("a body");
        pushed.velocity = Vec2::new(3.0, 1.0);
        let mut text = String::new();
        world.save_whole(&mut text).expect("saved");
        assert!(text.contains("\nresource Physics2D "), "{text}");
        let mut loaded = World::seeded(1);
        for registered in [
            loaded.register::<Transform>(),
            loaded.register::<RigidBody2D>(),
            loaded.register::<BoxCollider2D>(),
            loaded.register::<CircleCollider2D>(),
            loaded.register_saved::<Physics2D>(),
        ] {
            registered.expect("registered");
        }
        let lines = (1..).zip(text.lines());
        let changes = loaded.load_whole(lines, true).expect("loaded");
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(loaded.hash(), world.hash(), "loaded");
        step(&mut world, 60);
        step(&mut loaded, 60);
        assert_eq!(loaded.hash(), world.hash(), "stepped on");
    }

    /// Saved text whose bodies name what its rapier's state doesn't hold as theirs, as text from a
    /// host or a server may, is refused (`physics.load`), rather than stepped into a panic: a rigid
    /// body there's none of, another body's, a collider on another body, and an entity's second.
    #[test]
    fn saved_bodies_naming_what_rapier_holds_not_are_refused() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 4);
        let physics = world.resource::<Physics2D>().expect("stepped");
        let text = physics.save(&world).expect("saved");
        Physics2D::load(&text, &world).expect("its own text loads");
        let changes: [fn(&mut SavedPhysics<Rapier>); 4] = [
            |saved| saved.linked[0].body = RigidBodyHandle::invalid(),
            |saved| saved.linked[1].body = saved.linked[0].body,
            |saved| saved.linked[0].colliders = saved.linked[1].colliders,
            |saved| saved.linked[1].entity = saved.linked[0].entity,
        ];
        for (i, change) in changes.into_iter().enumerate() {
            let mut saved: SavedPhysics<Rapier> = ron::from_str(&text).expect("read");
            change(&mut saved);
            let changed = ron::to_string(&saved).expect("written");
            let refused = Physics2D::load(&changed, &world)
                .err()
                .map(|error| error.code());
            assert_eq!(refused, Some("physics.load"), "change {i}");
        }
    }

    /// The world's hash after physics has stepped a pile of boxes and balls for four seconds, which
    /// CI checks on every platform: a difference on some platforms only is a bug in determinism,
    /// not a value to change here.
    #[test]
    fn a_pile_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0x58bd_495d_6fa4_58c8, "{content:#018x}");
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

    /// A small ball going fast enough that a step carries it from one side of a thin kinematic wall
    /// to the other passes through it, but one with continuous collision stops at it, and stays
    /// this side.
    #[test]
    fn continuous_collision_stops_what_a_step_would_carry_through_a_thin_wall() {
        let mut world = World::default();
        let wall = Transform {
            translation: Vec3::new(5.0, 0.0, 0.0),
            scale: Vec3::new(0.05, 8.0, 1.0),
            ..Transform::IDENTITY
        };
        let kinematic = RigidBody2D {
            body_type: BodyType::Kinematic,
            ..RigidBody2D::default()
        };
        world.spawn((wall, kinematic, BoxCollider2D::default()));
        // 0.9375 units a step: from x 0, at 4.6875, short of the wall, then at 5.625, past it.
        let fired = |world: &mut World, y: f32, continuous_collision: bool| {
            let at = Transform {
                translation: Vec3::new(0.0, y, 0.0),
                ..Transform::IDENTITY
            };
            let body = RigidBody2D {
                continuous_collision,
                velocity: Vec2::new(60.0, 0.0),
                ..RigidBody2D::default()
            };
            let ball = CircleCollider2D {
                radius: 0.1,
                ..CircleCollider2D::default()
            };
            world.spawn((at, body, ball))
        };
        let through = fired(&mut world, 2.0, false);
        let stopped = fired(&mut world, -2.0, true);
        step(&mut world, 16);
        let x = |entity| transform(&world, entity).translation.x;
        assert!(x(through) > 5.5, "through the wall: {}", x(through));
        assert!(x(stopped) < 4.975, "stopped this side: {}", x(stopped));
    }

    /// A ball is as large as its radius scaled by the larger of its entity's scales across and up:
    /// half a unit, scaled 2 across and 1 up, rests with its middle a unit over the floor's top.
    #[test]
    fn a_ball_is_as_large_as_its_larger_scale() {
        let mut world = World::default();
        floor(&mut world);
        let at = Transform {
            translation: Vec3::new(0.0, 3.0, 0.0),
            scale: Vec3::new(2.0, 1.0, 1.0),
            ..Transform::IDENTITY
        };
        let ball = (at, RigidBody2D::default(), CircleCollider2D::default());
        let ball = world.spawn(ball);
        step(&mut world, 240);
        let rested = transform(&world, ball);
        assert!(
            (rested.translation.y - 1.5).abs() < 0.02,
            "a unit over the floor's top: {rested:?}"
        );
    }

    /// A body collides as its box and its circle both, and as each of them changes between steps:
    /// a box kept upright on a circle under it, as a foot, rests on the circle, then on its box once
    /// the circle's taken away.
    #[test]
    fn a_body_collides_as_its_box_and_its_circle() {
        let mut world = World::default();
        floor(&mut world);
        let at = Transform {
            translation: Vec3::new(0.0, 4.0, 0.0),
            ..Transform::IDENTITY
        };
        let upright = RigidBody2D {
            fixed_rotation: true,
            ..RigidBody2D::default()
        };
        let foot = CircleCollider2D {
            offset: Vec2::new(0.0, -1.0),
            ..CircleCollider2D::default()
        };
        let body = world.spawn((at, upright, BoxCollider2D::default(), foot));
        step(&mut world, 240);
        // The floor's top, the circle's radius, then its offset.
        let on_foot = transform(&world, body).translation.y;
        assert!((on_foot - 2.0).abs() < 0.02, "on its foot: {on_foot}");
        world.remove::<CircleCollider2D>(body).expect("alive");
        step(&mut world, 240);
        let on_box = transform(&world, body).translation.y;
        assert!((on_box - 1.0).abs() < 0.02, "on its box: {on_box}");
    }

    /// Each collider's outline goes where its entity draws, with its entity's ID: a box sized and
    /// offset across and up as its entity scales; a circle around its offset so scaled, as large as
    /// its radius scaled by the larger of the scales, and round; both of an entity with both, the
    /// box's first; one under a parent, carried by it, though physics doesn't move it; and none of
    /// an entity without a transform.
    #[test]
    fn colliders_outline_where_their_entities_draw() {
        let mut world = World::default();
        let stretched = Transform {
            translation: Vec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_z(0.4),
            scale: Vec3::new(2.0, 0.5, 1.0),
        };
        let boxed = BoxCollider2D {
            offset: Vec2::new(0.25, -0.5),
            size: Vec2::new(1.5, 2.0),
            ..BoxCollider2D::default()
        };
        let round = CircleCollider2D {
            offset: Vec2::new(0.5, 1.0),
            radius: 0.75,
            ..CircleCollider2D::default()
        };
        let both = world.spawn((stretched, boxed, round));
        let parent = world.spawn(Transform {
            translation: Vec3::new(-3.0, 0.0, 0.0),
            ..Transform::IDENTITY
        });
        let child = world.spawn((Transform::IDENTITY, round));
        world.set_parent(child, Some(parent)).expect("no cycle");
        world.spawn(boxed);
        let past = Past::default();
        let placed = Placed::of(&world, &past, 1.0);
        let poses = Poses::new(&world, &past, &placed, 1.0).expect("transforms");
        let mut found = Vec::new();
        outlines(&world, &poses, &mut |outline, id| found.push((outline, id)));
        let id = |entity| world.id(entity).expect("alive").get();
        let [
            (Outline::Rect(boxy), first),
            (Outline::Circle(ring), second),
            (Outline::Circle(carried), third),
        ] = found[..]
        else {
            panic!("{found:?}");
        };
        assert_eq!([first, second, third], [id(both), id(both), id(child)]);
        let near = |a: Vec3, b: Vec3| a.abs_diff_eq(b, 1e-5);
        // The box's corner on its top right: its offset and half its size, in its entity's space.
        let corner = boxy.transform_point3(Vec3::new(0.5, 0.5, 0.0));
        let entitys = stretched.matrix();
        assert!(near(
            corner,
            entitys.transform_point3(Vec3::new(1.0, 0.5, 0.0))
        ));
        // The circle's middle, and its edge 0.75 times 2 from it, across and up.
        let middle = ring.transform_point3(Vec3::ZERO);
        assert!(near(
            middle,
            entitys.transform_point3(Vec3::new(0.5, 1.0, 0.0))
        ));
        for axis in [Vec3::X, Vec3::Y] {
            let reach = ring.transform_point3(axis).distance(middle);
            assert!((reach - 1.5).abs() < 1e-5, "{reach}");
        }
        let middle = carried.transform_point3(Vec3::ZERO);
        assert!(near(middle, Vec3::new(-2.5, 1.0, 0.0)), "{middle}");
        let reach = carried.transform_point3(Vec3::X).distance(middle);
        assert!((reach - 0.75).abs() < 1e-5, "{reach}");
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
        assert_eq!(physics.links.linked.len(), 1);
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
        assert_eq!(physics.links.children, [child], "warned once");
        assert!(physics.links.linked.is_empty());
        world.set_parent(child, None).expect("a root");
        step(&mut world, 10);
        assert!(transform(&world, child).translation.y < held.translation.y);
        let physics = world.resource::<Physics2D>().expect("stepped");
        assert!(physics.links.children.is_empty());
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
