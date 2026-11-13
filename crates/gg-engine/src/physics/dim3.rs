//! 3D physics: bodies that rapier moves at the world's ticks, as their entities' components
//! describe them, characters that its character controller moves ([`character`]), casts through
//! their colliders ([`query`]), and the world's resource that holds rapier's bodies and contacts,
//! which snapshots copy and snapshot files hold.

use std::collections::HashSet;
use std::sync::Arc;

use gg_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
use rapier3d::control::CharacterCollision;
use rapier3d::prelude::{
    ActiveCollisionTypes, BroadPhaseBvh, CCDSolver, Collider, ColliderBuilder, ColliderHandle,
    ColliderSet, Group, ImpulseJointSet, IntegrationParameters, InteractionGroups,
    InteractionTestMode, IslandManager, MassProperties, MultibodyJointSet, NarrowPhase,
    PhysicsPipeline, Pose, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, RigidBodyType,
    Rotation, SoftBodySet, TriMeshFlags, Vector,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::contacts::{Contacts, SavedContacts};
use super::{
    BodyType, Contact, ContactEvent, Finite, Link, Links, Outline, kept, kept_collider, unfinite,
};
use crate::draw::Poses;
use crate::model::{ModelRoot, Surface};
use crate::recording::text;
use crate::{Component, Entity, EntityId, Error, Reflect, Result, Saved, Tick, Transform, World};

mod character;
mod joint;
mod query;

use character::Character;
pub use character::CharacterController3D;
pub(super) use joint::joint_lines;
pub use joint::{Joint3D, JointKind3D};
use joint::{Jointed, SavedJoint};
pub use query::{Cast, Hit3D, Shape3D};

/// A body that 3D physics moves, at each tick a layer runs [`step_physics`](crate::step_physics)
/// in: its entity's [`Transform`]'s translation and rotation, and the velocities here, which each
/// step reads and writes back. It collides as its entity's [`BoxCollider3D`], [`SphereCollider3D`]
/// and [`CapsuleCollider3D`], whichever it has, which give a dynamic body its mass too, together
/// where it has more than one; and [`Joint3D`]s hold it to other bodies. A body under a parent
/// isn't moved, since its transform is in its parent's space (`physics.child_body`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct RigidBody3D {
    /// How it moves: dynamic, by its velocity, which gravity and what it hits change; kinematic, by
    /// its velocity alone, pushing what's dynamic aside, as a moving platform does; or static, not
    /// at all, as a floor or a wall.
    pub body_type: BodyType,
    /// Whether it never turns, as a character that stays upright: what sets its rotation still
    /// turns it.
    pub fixed_rotation: bool,
    /// Whether a step that it goes fast in sweeps it along its way against what moves too, rapier's
    /// continuous collision, so that it stops at a kinematic or dynamic body that it would
    /// otherwise pass through between one step and the next, as a small fast ball passes through a
    /// thin moving wall. Each dynamic body that goes fast is swept against what's static anyway,
    /// and one with this against all but others with it, as rapier sweeps bullets, at some cost.
    pub continuous_collision: bool,
    /// How fast it goes, in units a second.
    pub velocity: Vec3,
    /// How fast it turns, about the axis it points along, in radians a second: counterclockwise as
    /// seen from that axis's tip.
    pub angular_velocity: Vec3,
}

/// The box a [`RigidBody3D`] on the same entity collides as, in the entity's space, so that it
/// turns and scales with it: by default a unit cube.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct BoxCollider3D {
    /// Where its middle is, from its entity's origin.
    pub offset: Vec3,
    /// How wide, tall and deep it is.
    #[reflect(min = 0.0, default = Vec3::ONE)]
    pub size: Vec3,
    /// Its mass a cubic unit, which a dynamic body's mass comes from.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it holds back what slides along it: 0 not at all, as ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// How much of the speed it hits something with it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, as a trigger zone is: it finds what overlaps it, as
    /// [`World::contacts`](crate::World::contacts) has it, but pushes nothing and nothing pushes
    /// it, characters moving through it as casts pass through it. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch,
    /// or overlap, only where each is in a group the other's filter has; a character moves
    /// through what its capsule doesn't meet.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for BoxCollider3D {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            size: Vec3::ONE,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            membership: u32::MAX,
            filter: u32::MAX,
        }
    }
}

impl BoxCollider3D {
    /// The collider it makes for an entity of `scale`: its size and its offset scaled, a mirrored
    /// entity's box the same.
    fn collider(&self, scale: Vec3) -> Collider {
        let half = (self.size * scale).abs() * 0.5;
        let shape = ColliderBuilder::cuboid(half.x.into(), half.y.into(), half.z.into());
        let Self {
            offset,
            density,
            friction,
            restitution,
            sensor,
            membership,
            filter,
            ..
        } = *self;
        let shape = built(shape, offset * scale, density, friction, restitution);
        met(shape, sensor, membership, filter).build()
    }
}

/// The sphere a [`RigidBody3D`] on the same entity collides as, in the entity's space: around its
/// offset, which turns and scales with the entity, its radius scaled by the largest of the entity's
/// scales, since a sphere stays one however its entity stretches. By default half a unit, so that
/// it's a unit across.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct SphereCollider3D {
    /// Where its middle is, from its entity's origin.
    pub offset: Vec3,
    /// How far its surface is from its middle.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// Its mass a cubic unit, which a dynamic body's mass comes from.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it holds back what slides along it: 0 not at all, as ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// How much of the speed it hits something with it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, as a trigger zone is: it finds what overlaps it, as
    /// [`World::contacts`](crate::World::contacts) has it, but pushes nothing and nothing pushes
    /// it, characters moving through it as casts pass through it. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch,
    /// or overlap, only where each is in a group the other's filter has; a character moves
    /// through what its capsule doesn't meet.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for SphereCollider3D {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            radius: 0.5,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            membership: u32::MAX,
            filter: u32::MAX,
        }
    }
}

impl SphereCollider3D {
    /// The collider it makes for an entity of `scale`: its offset scaled, and its radius by the
    /// largest of the scales.
    fn collider(&self, scale: Vec3) -> Collider {
        let shape = ColliderBuilder::ball((self.radius * scale.abs().max_element()).into());
        let Self {
            offset,
            density,
            friction,
            restitution,
            sensor,
            membership,
            filter,
            ..
        } = *self;
        let shape = built(shape, offset * scale, density, friction, restitution);
        met(shape, sensor, membership, filter).build()
    }
}

/// The capsule a [`RigidBody3D`] on the same entity collides as, in the entity's space: what's
/// within its radius of a segment along the entity's Y, a cylinder capped by half spheres, upright
/// as a character stands; around its offset, which turns and scales with the entity, its height
/// scaled by the entity's scale along Y, and its radius by the larger of its scales along X and Z,
/// since it stays round however its entity stretches. One no taller than twice its radius is a
/// sphere of that radius. By default 2 units tall and half a unit in radius, a person's size where
/// a unit is a meter.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct CapsuleCollider3D {
    /// Where its middle is, from its entity's origin.
    pub offset: Vec3,
    /// How tall it is, from the bottom of its lower cap to the top of its upper.
    #[reflect(min = 0.0, default = 2.0)]
    pub height: f32,
    /// How far its side is from the segment through its middle.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// Its mass a cubic unit, which a dynamic body's mass comes from.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it holds back what slides along it: 0 not at all, as ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// How much of the speed it hits something with it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, as a trigger zone is: it finds what overlaps it, as
    /// [`World::contacts`](crate::World::contacts) has it, but pushes nothing and nothing pushes
    /// it, characters moving through it as casts pass through it. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch,
    /// or overlap, only where each is in a group the other's filter has; a character moves
    /// through what its capsule doesn't meet.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for CapsuleCollider3D {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            height: 2.0,
            radius: 0.5,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            membership: u32::MAX,
            filter: u32::MAX,
        }
    }
}

impl CapsuleCollider3D {
    /// Its radius for an entity of `scale`, and how far its segment reaches each way of its middle.
    pub(crate) fn shape(&self, scale: Vec3) -> (f32, f32) {
        let scale = scale.abs();
        let radius = self.radius * scale.x.max(scale.z);
        // Compared rather than `max`ed, whose zero's sign std leaves to the platform.
        let half = self.height * scale.y * 0.5 - radius;
        (radius, if half > 0.0 { half } else { 0.0 })
    }

    /// The collider it makes for an entity of `scale`: its offset scaled, and its shape as
    /// [`shape`](Self::shape) has it.
    fn collider(&self, scale: Vec3) -> Collider {
        let (radius, half) = self.shape(scale);
        let shape = ColliderBuilder::capsule_y(half.into(), radius.into());
        let Self {
            offset,
            density,
            friction,
            restitution,
            sensor,
            membership,
            filter,
            ..
        } = *self;
        let shape = built(shape, offset * scale, density, friction, restitution);
        met(shape, sensor, membership, filter).build()
    }
}

/// The triangles a [`RigidBody3D`] on the same entity collides as: those of the model its entity's
/// [`ModelRoot`] names, each of the model's nodes' where the node is in the model, in the entity's
/// space, so that they turn with it and scale along each axis as it does, as a level's floors and
/// walls do. Triangles hold no volume, so it gives a body no mass, and suits static and kinematic
/// bodies; a dynamic one takes its mass from its other colliders. One whose entity has no
/// `ModelRoot`, or whose model has no triangles, as a model that didn't load, collides as nothing,
/// warned of as it's made (`physics.no_surface`). Its outline isn't drawn, as its model shows where
/// it is.
///
/// A world saved whole holds its triangles whole, as rapier serializes them, at a cost DEFECTS.md
/// weighs.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct MeshCollider3D {
    /// How much it holds back what slides along it: 0 not at all, as ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// How much of the speed it hits something with it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, as a trigger zone is: it finds what overlaps it, as
    /// [`World::contacts`](crate::World::contacts) has it, but pushes nothing and nothing pushes
    /// it, characters moving through it as casts pass through it.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch,
    /// or overlap, only where each is in a group the other's filter has; a character moves
    /// through what its capsule doesn't meet.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for MeshCollider3D {
    fn default() -> Self {
        Self {
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            membership: u32::MAX,
            filter: u32::MAX,
        }
    }
}

impl MeshCollider3D {
    /// The collider it makes for `entity`, of `scale`, in `world`, with the surfaces of the model it
    /// made it of, which a model read again replaces: none where the entity has no model, or its
    /// model no triangles, which it warns of.
    fn collider(
        &self,
        world: &World,
        entity: Entity,
        scale: Vec3,
    ) -> (Option<Collider>, Option<Arc<Vec<Surface>>>) {
        let nodes = model_of(world, entity).and_then(|model| world.assets().nodes(model));
        let made = nodes.and_then(|nodes| {
            let (vertices, indices) = nodes.triangles();
            let vertices = vertices.into_iter().map(|at| vector(at * scale)).collect();
            // Without topology or pseudo-normals, which would double what it holds and what a world
            // saved whole writes of it.
            let shape =
                ColliderBuilder::trimesh_with_flags(vertices, indices, TriMeshFlags::empty());
            let shape = shape.ok()?.mass_properties(MassProperties::default());
            let shape = (shape.friction(self.friction.into())).restitution(self.restitution.into());
            Some(met(shape, self.sensor, self.membership, self.filter).build())
        });
        if made.is_none() {
            let id = world.id(entity).expect("found alive");
            let why = match model_of(world, entity) {
                Some(_) => "its model has no triangles",
                None => "it has no ModelRoot",
            };
            warn!(
                code = "physics.no_surface",
                entity = %id,
                "Entity {id}'s MeshCollider3D collides as nothing: {why}"
            );
        }
        (made, nodes.map(|nodes| Arc::clone(&nodes.surfaces)))
    }
}

/// The model `entity`'s [`ModelRoot`] names, if it has one.
fn model_of(world: &World, entity: Entity) -> Option<crate::Model> {
    world.get::<ModelRoot>(entity).map(|root| root.0)
}

/// Whether `collider` holds the triangles of the model `entity`'s [`ModelRoot`] names, for an
/// entity of `scale`, as [`MeshCollider3D`] makes them of it now, to the bit.
fn made_of(collider: &Collider, world: &World, entity: Entity, scale: Vec3) -> bool {
    let Some(mesh) = collider.shape().as_trimesh() else {
        return false;
    };
    let nodes = model_of(world, entity).and_then(|model| world.assets().nodes(model));
    let Some(nodes) = nodes else {
        return false;
    };
    let (vertices, indices) = nodes.triangles();
    let bits = |at: Vector| at.to_array().map(f64::to_bits);
    mesh.indices() == indices.as_slice()
        && mesh.vertices().len() == vertices.len()
        && (vertices.iter().zip(mesh.vertices()))
            .all(|(&at, &held)| bits(vector(at * scale)) == bits(held))
}

/// The surfaces of the model `entity`'s [`ModelRoot`] names, as the world's assets hold them now.
fn surfaces(world: &World, entity: Entity) -> Option<Arc<Vec<Surface>>> {
    let nodes = world.assets().nodes(model_of(world, entity)?)?;
    Some(Arc::clone(&nodes.surfaces))
}

/// A collider of `shape` around `offset`, of what a collider's component gives it.
fn built(
    shape: ColliderBuilder,
    offset: Vec3,
    density: f32,
    friction: f32,
    restitution: f32,
) -> ColliderBuilder {
    shape
        .translation(vector(offset))
        .density(density.into())
        .friction(friction.into())
        .restitution(restitution.into())
}

/// `shape` with what a collider's component says of whether it's a sensor and which groups it
/// meets: a sensor finds kinematic and static bodies too, characters among them, which rapier's
/// default leaves to dynamic ones, but for a static body's sensor finding another static body.
fn met(shape: ColliderBuilder, sensor: bool, membership: u32, filter: u32) -> ColliderBuilder {
    let shape = shape
        .sensor(sensor)
        .collision_groups(groups(membership, filter));
    if sensor {
        let types = ActiveCollisionTypes::all().difference(ActiveCollisionTypes::FIXED_FIXED);
        shape.active_collision_types(types)
    } else {
        shape
    }
}

/// rapier's groups of a collider in the groups `membership` has, meeting those `filter` has.
fn groups(membership: u32, filter: u32) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(membership),
        Group::from_bits_retain(filter),
        InteractionTestMode::And,
    )
}

/// The world's 3D physics, a [`Resource`](crate::Resource) that
/// [`step_physics`](crate::step_physics) gives a world the first time it steps one with a
/// [`RigidBody3D`] and without it, or that a game inserts first, with its gravity: rapier's bodies,
/// colliders, joints and contacts, which each step carries on from, and each body's and joint's
/// entity, so that a snapshot restored steps as its world did. A world cleared, or whose entities a
/// scene replaced, keeps it, its bodies and joints going at the next step, as their entities have.
pub struct Physics3D {
    /// Which way everything dynamic falls, and how much faster it goes each second, in units a
    /// second: down along -Y at 9.81 by default, Earth's, as though a unit were a meter; and the
    /// characters, as much as each's [`CharacterController3D::gravity`] says.
    pub gravity: Vec3,
    rapier: Rapier,
    /// rapier's workspace, which a copy makes anew, since no step carries anything in it over.
    pipeline: PhysicsPipeline,
    links: Links<Linked>,
    characters: Links<Character>,
    joints: Links<Jointed>,
    /// The entities with a `CharacterController3D` that aren't characters, as they lack a
    /// `CapsuleCollider3D` or have a `RigidBody3D`, warned of once each.
    unmoved: Vec<Entity>,
    /// What a character hit as it moved, a workspace as `pipeline` is.
    hits: Vec<CharacterCollision>,
    /// The kinematic bodies that go by their velocities, with those velocities, which the
    /// characters' moves hold them from meanwhile: a workspace too.
    moving: Vec<character::Moving>,
    pub(super) contacts: Contacts,
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
    /// The colliders of its entity's `BoxCollider3D`, `SphereCollider3D`, `CapsuleCollider3D` and
    /// `MeshCollider3D`, in that order, of those it has.
    colliders: [Option<ColliderHandle>; 4],
    rigid_body: RigidBody3D,
    transform: Transform,
    box_collider: Option<BoxCollider3D>,
    sphere_collider: Option<SphereCollider3D>,
    capsule_collider: Option<CapsuleCollider3D>,
    mesh_collider: Option<MeshCollider3D>,
    /// The surfaces of the model its mesh collider was made of, which a model read again
    /// replaces, its collider made again then.
    surfaces: Option<Arc<Vec<Surface>>>,
}

impl Link for Linked {
    fn entity(&self) -> Entity {
        self.entity
    }
}

impl Default for Physics3D {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
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
                    dt: Tick::DT.as_secs_f64(),
                    ..IntegrationParameters::default()
                },
            },
            pipeline: PhysicsPipeline::new(),
            links: Links::default(),
            characters: Links::default(),
            joints: Links::default(),
            unmoved: Vec::new(),
            hits: Vec::new(),
            moving: Vec::new(),
            contacts: Contacts::default(),
        }
    }
}

impl Clone for Physics3D {
    fn clone(&self) -> Self {
        Self {
            gravity: self.gravity,
            rapier: self.rapier.clone(),
            pipeline: PhysicsPipeline::new(),
            links: self.links.clone(),
            characters: self.characters.clone(),
            joints: self.joints.clone(),
            unmoved: self.unmoved.clone(),
            hits: Vec::new(),
            moving: Vec::new(),
            contacts: self.contacts.clone(),
        }
    }

    /// Copies `source` into the memory this one holds, as far as rapier's types copy so, keeping
    /// its own workspace for the characters, but not rapier's: its pipeline keeps the joints it last
    /// solved and its solver's layout of them, by counts that the joint set copied brings back, so
    /// that, kept, it would solve another world's joints, or ones gone, at the next step. A new one,
    /// as a copy has, costs its workspace's memory made again as that step goes.
    fn clone_from(&mut self, source: &Self) {
        self.gravity = source.gravity;
        self.rapier.clone_from(&source.rapier);
        self.pipeline = PhysicsPipeline::new();
        self.links.clone_from(&source.links);
        self.characters.clone_from(&source.characters);
        self.joints.clone_from(&source.joints);
        self.unmoved.clone_from(&source.unmoved);
        self.contacts.clone_from(&source.contacts);
    }
}

/// What a snapshot file holds of [`Physics3D`], `R` its rapier's state, as its own or borrowed: its
/// gravity; rapier's state, through rapier's own serialization, which carries on exactly; each body,
/// each character and each joint, with its entity by its ID; and the bodies under a parent and the
/// entities with a character's controller that aren't characters warned of, by their entities'
/// IDs; and the contacts and their events. Text that lists no joints or contacts reads as having
/// none, so that a world saved without them loads.
#[derive(Serialize, Deserialize)]
struct SavedPhysics<R> {
    gravity: [f32; 3],
    rapier: R,
    linked: Vec<SavedLinked>,
    children: Vec<Option<u64>>,
    characters: Vec<SavedCharacter>,
    character_children: Vec<Option<u64>>,
    unmoved: Vec<Option<u64>>,
    #[serde(default)]
    joints: Vec<SavedJoint>,
    #[serde(default)]
    contacts: SavedContacts,
}

/// A body as a snapshot file holds it: its entity's ID, none for an entity gone since the last step,
/// which the next takes the body out for; its handles; and what the last step left in its entity's
/// components, each as a recording's text writes values, so that a component whose type changed
/// is matched to it as the component is.
#[derive(Serialize, Deserialize)]
struct SavedLinked {
    entity: Option<u64>,
    body: RigidBodyHandle,
    colliders: [Option<ColliderHandle>; 4],
    rigid_body: String,
    transform: String,
    box_collider: Option<String>,
    sphere_collider: Option<String>,
    capsule_collider: Option<String>,
    mesh_collider: Option<String>,
}

/// A character as a snapshot file holds it, as [`SavedLinked`] holds a body.
#[derive(Serialize, Deserialize)]
struct SavedCharacter {
    entity: Option<u64>,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    controller: String,
    transform: String,
    capsule: String,
}

/// Saved through rapier's own serialization, written as RON, so that a world restored in another
/// process steps on as the one saved would have: its contacts' warm starts, its islands' sleep and
/// its handles as they were, which bodies made again from their components would lose. A rapier of
/// another version may not read it, which leaves physics out, to be made again from the entities,
/// as does text whose bodies name a rigid body or a collider, or whose joints a joint, that its
/// rapier's state doesn't hold as theirs (`physics.load`).
impl Saved for Physics3D {
    const NAME: &'static str = "Physics3D";

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
            sphere_collider: linked.sphere_collider.as_ref().map(|it| value(it)),
            capsule_collider: linked.capsule_collider.as_ref().map(|it| value(it)),
            mesh_collider: linked.mesh_collider.as_ref().map(|it| value(it)),
        });
        let characters = self
            .characters
            .linked
            .iter()
            .map(|character| SavedCharacter {
                entity: id(character.entity),
                body: character.body,
                collider: character.collider,
                controller: value(&character.controller),
                transform: value(&character.transform),
                capsule: value(&character.capsule),
            });
        let joints = self.joints.linked.iter().map(|jointed| SavedJoint {
            entity: id(jointed.entity),
            handle: jointed.handle,
            bodies: jointed.bodies,
            scales: jointed.scales.map(|scale| scale.to_array()),
            joint: value(&jointed.joint),
            warned: jointed.warned.clone(),
        });
        let ids = |entities: &[Entity]| entities.iter().map(|&entity| id(entity)).collect();
        let saved = SavedPhysics {
            gravity: self.gravity.to_array(),
            rapier: &self.rapier,
            linked: linked.collect(),
            children: ids(&self.links.children),
            characters: characters.collect(),
            character_children: ids(&self.characters.children),
            unmoved: ids(&self.unmoved),
            joints: joints.collect(),
            contacts: self.contacts.save(),
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
        /// The collider `text` writes, if it's given.
        fn collider<C: Component>(world: &World, text: Option<String>) -> Result<Option<C>> {
            let read = text.map(|text| world.component_from_text(&text));
            read.transpose()
        }
        let mut linked = Vec::with_capacity(saved.linked.len());
        for body in saved.linked {
            let entity = entity(body.entity);
            let mesh_collider = collider(world, body.mesh_collider)?;
            let alive = entity != Entity::DEAD;
            let made_of = mesh_collider
                .filter(|_| alive)
                .and_then(|_| surfaces(world, entity));
            linked.push(Linked {
                entity,
                body: body.body,
                colliders: body.colliders,
                rigid_body: world.component_from_text(&body.rigid_body)?,
                transform: world.component_from_text(&body.transform)?,
                box_collider: collider(world, body.box_collider)?,
                sphere_collider: collider(world, body.sphere_collider)?,
                capsule_collider: collider(world, body.capsule_collider)?,
                mesh_collider,
                surfaces: made_of,
            });
        }
        let mut characters = Vec::with_capacity(saved.characters.len());
        for character in saved.characters {
            characters.push(Character {
                entity: entity(character.entity),
                body: character.body,
                collider: character.collider,
                controller: world.component_from_text(&character.controller)?,
                transform: world.component_from_text(&character.transform)?,
                capsule: world.component_from_text(&character.capsule)?,
            });
        }
        // What a step reaches by the links' handles, which text from elsewhere, a host's or a
        // server's, may name otherwise than rapier's sets hold: each body there and linked once,
        // naming its entity as its link does, which casts find it by, its colliders on it, and
        // each entity linked once.
        let refused = |entity: Entity, what: &str| {
            let who = world.id(entity);
            let who = who.map_or_else(|| "an entity gone".to_owned(), |id| format!("entity {id}"));
            Error::new("physics.load", format!("{who}'s body {what}"))
        };
        let rapier = &saved.rapier;
        let bodies_of = (linked.iter().map(|it| (it.entity, it.body, it.colliders))).chain(
            characters.iter().map(|it| {
                let colliders = [Some(it.collider), None, None, None];
                (it.entity, it.body, colliders)
            }),
        );
        let mut bodies = HashSet::with_capacity(linked.len() + characters.len());
        let mut entities = HashSet::with_capacity(linked.len() + characters.len());
        for (entity, body, colliders) in bodies_of {
            let Some(rigid_body) = rapier.bodies.get(body).filter(|_| bodies.insert(body)) else {
                let what = "names no rigid body of rapier's, or another body's";
                return Err(refused(entity, what));
            };
            let alive = entity != Entity::DEAD;
            if alive && rigid_body.user_data != u128::from(entity.to_bits()) {
                return Err(refused(entity, "names another entity in rapier's state"));
            }
            if alive && !entities.insert(entity) {
                return Err(refused(entity, "is its second"));
            }
            let on = |collider: &ColliderHandle| {
                rapier.colliders.get(*collider).and_then(Collider::parent) == Some(body)
            };
            if !colliders.iter().flatten().all(on) {
                let what = "names a collider that isn't on its rigid body";
                return Err(refused(entity, what));
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
                scales: joint.scales.map(Vec3::from_array),
                joint: world.component_from_text(&joint.joint)?,
                warned: joint.warned,
            });
        }
        let entities = |ids: Vec<Option<u64>>| ids.into_iter().map(entity).collect();
        let links = Links::new(linked, entities(saved.children));
        let links = links.map_err(|twice| refused(twice, "is its second"))?;
        let characters = Links::new(characters, entities(saved.character_children));
        let characters = characters.map_err(|twice| refused(twice, "is its second"))?;
        let joints = Links::new(joints, Vec::new());
        let joints = joints.map_err(|twice| refused_joint(twice, "is its second"))?;
        Ok(Self {
            gravity: Vec3::from_array(saved.gravity),
            rapier: saved.rapier,
            pipeline: PhysicsPipeline::new(),
            links,
            characters,
            joints,
            unmoved: entities(saved.unmoved),
            hits: Vec::new(),
            moving: Vec::new(),
            contacts: Contacts::load(saved.contacts, world),
        })
    }
}

/// Steps `world`'s 3D physics a tick, as [`step_physics`](crate::step_physics) describes, giving it
/// a [`Physics3D`] first if it has a [`RigidBody3D`] or a [`CharacterController3D`] and none.
pub(super) fn step(world: &mut World) {
    if world.resource::<Physics3D>().is_none() {
        let bodies = world.query::<&RigidBody3D>().next().is_some();
        if !bodies && world.query::<&CharacterController3D>().next().is_none() {
            return;
        }
        world.insert_resource(Physics3D::default());
    }
    world.resource_scope(|world, physics: &mut Physics3D| physics.step(world));
}

impl Physics3D {
    /// Takes in what changed, the joints after the bodies and the characters they join, moves the
    /// characters as far as what they move into lets them, then steps rapier, which carries the
    /// characters there, and writes back where everything went.
    fn step(&mut self, world: &mut World) {
        self.unlink_gone(world);
        self.unlink_gone_characters(world);
        let rapier = &mut self.rapier;
        let refused = |entity| {
            (unfinite::<Transform>(world, entity, "Transform"))
                .or_else(|| unfinite::<RigidBody3D>(world, entity, "RigidBody3D"))
                .or_else(|| unfinite::<BoxCollider3D>(world, entity, "BoxCollider3D"))
                .or_else(|| unfinite::<SphereCollider3D>(world, entity, "SphereCollider3D"))
                .or_else(|| unfinite::<CapsuleCollider3D>(world, entity, "CapsuleCollider3D"))
                .or_else(|| unfinite::<MeshCollider3D>(world, entity, "MeshCollider3D"))
        };
        self.links
            .find::<RigidBody3D>(world, "RigidBody3D", refused, |links, entity| {
                match links.get_mut(entity) {
                    Some(linked) => take_changes(rapier, linked, world),
                    None => links.push(make_body(rapier, world, entity)),
                }
            });
        self.find_characters(world);
        self.join(world);
        self.move_characters(world);
        self.step_rapier();
        self.contacts.take(world, touching(&self.rapier));
        self.write_back(world);
        self.write_back_characters(world);
    }

    /// Steps rapier a tick, its bodies, characters and joints as they've been taken in.
    fn step_rapier(&mut self) {
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
    }

    /// Takes out each body whose entity has gone, lost its `RigidBody3D` or its `Transform`, or
    /// come under a parent; the rest keep their order.
    fn unlink_gone(&mut self, world: &World) {
        let rapier = &mut self.rapier;
        self.links.retain(|linked| {
            let entity = linked.entity;
            let has = world.get::<RigidBody3D>(entity).is_some()
                && world.get::<Transform>(entity).is_some();
            if has && matches!(world.parent(entity), Ok(None)) {
                return true;
            }
            remove_body(rapier, linked.body);
            false
        });
    }

    /// Writes where each body that moves went, how it's turned, and how fast it goes, into its
    /// entity's components: each only as it changed, so that a body at rest, as one asleep, leaves
    /// its entity's rows unchanged, which the renderer's scene would write to the GPU again.
    fn write_back(&mut self, world: &mut World) {
        for linked in &mut self.links.linked {
            let body = &self.rapier.bodies[linked.body];
            if body.body_type() == RigidBodyType::Fixed {
                continue;
            }
            // Its rotation and velocities are rapier's `f64`s, the components' `f32`s, which a step
            // takes in only as they change from what was written.
            let at = DVec3::from_array(body.translation().to_array());
            let turned = Quat::from_array(body.rotation().to_array().map(|x| x as f32));
            let velocity = Vec3::from_array(body.linvel().to_array().map(|x| x as f32));
            let angular_velocity = Vec3::from_array(body.angvel().to_array().map(|x| x as f32));
            // The entity's components as this step took them in: as they are, but a transform
            // refused, which stays as it was here, so that its NaNs aren't kept as taken.
            let (was, went) = (&linked.transform, &linked.rigid_body);
            if (at, turned) != (was.translation, was.rotation) {
                let transform = world.get_mut::<Transform>(linked.entity);
                let transform = transform.expect("linked this step: alive, with one");
                transform.translation = at;
                transform.rotation = turned;
                linked.transform.translation = at;
                linked.transform.rotation = turned;
            }
            if (velocity, angular_velocity) != (went.velocity, went.angular_velocity) {
                let rigid_body = world.get_mut::<RigidBody3D>(linked.entity);
                let rigid_body = rigid_body.expect("linked this step: alive, with one");
                rigid_body.velocity = velocity;
                rigid_body.angular_velocity = angular_velocity;
                linked.rigid_body = *rigid_body;
            }
        }
    }

    /// The contacts the last step left, as [`World::contacts`] says, 3D's alone.
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.contacts.touching()
    }

    /// The contacts that started and stopped at the last step, as [`World::contact_events`] says,
    /// 3D's alone.
    pub fn contact_events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        self.contacts.events()
    }

    /// The body of `entity`, a body's or a character's, if it has one.
    fn body_of(&self, entity: Entity) -> Option<RigidBodyHandle> {
        let body = self.links.get(entity).map(|linked| linked.body);
        body.or_else(|| self.characters.get(entity).map(|character| character.body))
    }
}

/// Takes `body` out of `rapier`, with its colliders.
fn remove_body(rapier: &mut Rapier, body: RigidBodyHandle) {
    rapier.bodies.remove(
        body,
        &mut rapier.islands,
        &mut rapier.colliders,
        &mut rapier.impulse_joints,
        &mut rapier.multibody_joints,
        &mut rapier.soft_bodies,
        true,
    );
}

/// Each pair of colliders in `rapier` that touch, or overlap where either is a sensor, by their
/// bodies' entities, and whether it's a sensor's, in rapier's order.
fn touching(rapier: &Rapier) -> impl Iterator<Item = (Entity, Entity, bool)> + '_ {
    let entity = |collider: ColliderHandle| {
        let body = rapier
            .bodies
            .get(rapier.colliders.get(collider)?.parent()?)?;
        Entity::from_bits(u64::try_from(body.user_data).ok()?)
    };
    let narrow_phase = &rapier.narrow_phase;
    let contacts = (narrow_phase.contact_pairs())
        .filter(|pair| pair.has_any_active_contact())
        .map(|pair| (pair.collider1, pair.collider2, false));
    let overlaps = (narrow_phase.intersection_pairs())
        .filter(|&(.., intersecting)| intersecting)
        .map(|(a, b, _)| (a, b, true));
    let pairs = contacts.chain(overlaps);
    pairs.filter_map(move |(a, b, sensor)| Some((entity(a)?, entity(b)?, sensor)))
}

/// What the body of `entity` holds as its user data, which casts and contacts find the entity by.
fn user_data(entity: Entity) -> u128 {
    u128::from(entity.to_bits())
}

/// Makes `entity`'s body in `rapier`, as its components say, and gives back its link.
fn make_body(rapier: &mut Rapier, world: &World, entity: Entity) -> Linked {
    let rigid_body = *world.get::<RigidBody3D>(entity).expect("found with one");
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let at = Pose::from_parts(at(transform.translation), rotation(transform.rotation));
    let mut built = RigidBodyBuilder::new(body_type(rigid_body.body_type))
        .pose(at)
        .linvel(vector(rigid_body.velocity))
        .angvel(vector(rigid_body.angular_velocity))
        .ccd_enabled(rigid_body.continuous_collision)
        .user_data(user_data(entity));
    if rigid_body.fixed_rotation {
        built = built.lock_rotations();
    }
    let body = rapier.bodies.insert(built);
    let box_collider = world.get::<BoxCollider3D>(entity).copied();
    let sphere_collider = world.get::<SphereCollider3D>(entity).copied();
    let capsule_collider = world.get::<CapsuleCollider3D>(entity).copied();
    let mesh_collider = world.get::<MeshCollider3D>(entity).copied();
    let (meshed, surfaces) = mesh_collider.map_or((None, None), |mesh| {
        mesh.collider(world, entity, transform.scale)
    });
    let mut colliders = [None; 4];
    let made = [
        box_collider.map(|made| made.collider(transform.scale)),
        sphere_collider.map(|made| made.collider(transform.scale)),
        capsule_collider.map(|made| made.collider(transform.scale)),
        meshed,
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
        sphere_collider,
        capsule_collider,
        mesh_collider,
        surfaces,
    }
}

/// Takes in what changed of `linked`'s components since the last step left them, which the last
/// step didn't make, into its body: waking it, so that it moves from there. A component that isn't
/// finite is taken as it was, as [`Links`] refuses it. Its mesh collider is made again as its model
/// is read again too, but for one a snapshot restored before its model's surfaces were there, which
/// keeps the shape the snapshot had, and its contacts, as its model's come, if they make it.
fn take_changes(rapier: &mut Rapier, linked: &mut Linked, world: &World) {
    let entity = linked.entity;
    let rigid_body = *world.get::<RigidBody3D>(entity).expect("found with one");
    let rigid_body = kept(rigid_body, linked.rigid_body);
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let transform = kept(transform, linked.transform);
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
        body.set_angvel(vector(rigid_body.angular_velocity), true);
    }
    if transform.translation != linked.transform.translation {
        body.set_translation(at(transform.translation), true);
    }
    if transform.rotation != linked.transform.rotation {
        body.set_rotation(rotation(transform.rotation), true);
    }
    let box_collider = world.get::<BoxCollider3D>(entity).copied();
    let box_collider = kept_collider(box_collider, linked.box_collider);
    let sphere_collider = world.get::<SphereCollider3D>(entity).copied();
    let sphere_collider = kept_collider(sphere_collider, linked.sphere_collider);
    let capsule_collider = world.get::<CapsuleCollider3D>(entity).copied();
    let capsule_collider = kept_collider(capsule_collider, linked.capsule_collider);
    let mesh_collider = world.get::<MeshCollider3D>(entity).copied();
    let mesh_collider = kept_collider(mesh_collider, linked.mesh_collider);
    let rescaled = transform.scale != linked.transform.scale;
    let [boxed, round, capsuled, meshed] = &mut linked.colliders;
    if box_collider != linked.box_collider || rescaled {
        let made = box_collider.map(|made| made.collider(transform.scale));
        replace(rapier, boxed, made, linked.body);
    }
    if sphere_collider != linked.sphere_collider || rescaled {
        let made = sphere_collider.map(|made| made.collider(transform.scale));
        replace(rapier, round, made, linked.body);
    }
    if capsule_collider != linked.capsule_collider || rescaled {
        let made = capsule_collider.map(|made| made.collider(transform.scale));
        replace(rapier, capsuled, made, linked.body);
    }
    let now = mesh_collider.and_then(|_| surfaces(world, entity));
    let remodeled = match (&now, &linked.surfaces) {
        (Some(now), Some(was)) => !Arc::ptr_eq(now, was),
        // Restored with its shape, before its model's surfaces were there to make it of: kept,
        // with its contacts, if it's the one they make, as the snapshot's run made it of them;
        // made again of them if the model has changed since, as its file may have.
        (Some(now), None) if meshed.is_some() => {
            let restored = meshed.and_then(|meshed| rapier.colliders.get(meshed));
            let same = restored.is_some_and(|it| made_of(it, world, entity, transform.scale));
            if same {
                linked.surfaces = Some(Arc::clone(now));
            }
            !same
        }
        (now, was) => now.is_some() != was.is_some(),
    };
    if mesh_collider != linked.mesh_collider || rescaled || remodeled {
        let (made, surfaces) = mesh_collider.map_or((None, None), |mesh| {
            mesh.collider(world, entity, transform.scale)
        });
        replace(rapier, meshed, made, linked.body);
        linked.surfaces = surfaces;
    }
    linked.rigid_body = rigid_body;
    linked.transform = transform;
    linked.box_collider = box_collider;
    linked.sphere_collider = sphere_collider;
    linked.capsule_collider = capsule_collider;
    linked.mesh_collider = mesh_collider;
}

impl Finite for RigidBody3D {
    fn finite(&self) -> bool {
        self.velocity.is_finite() && self.angular_velocity.is_finite()
    }
}

impl Finite for BoxCollider3D {
    fn finite(&self) -> bool {
        let numbers = [self.density, self.friction, self.restitution];
        self.offset.is_finite() && self.size.is_finite() && numbers.iter().all(|it| it.is_finite())
    }
}

impl Finite for SphereCollider3D {
    fn finite(&self) -> bool {
        let numbers = [self.radius, self.density, self.friction, self.restitution];
        self.offset.is_finite() && numbers.iter().all(|it| it.is_finite())
    }
}

impl Finite for CapsuleCollider3D {
    fn finite(&self) -> bool {
        let numbers = [
            self.height,
            self.radius,
            self.density,
            self.friction,
            self.restitution,
        ];
        self.offset.is_finite() && numbers.iter().all(|it| it.is_finite())
    }
}

impl Finite for MeshCollider3D {
    fn finite(&self) -> bool {
        self.friction.is_finite() && self.restitution.is_finite()
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

/// Gives `each` the outline of each [`BoxCollider3D`], [`SphereCollider3D`] and
/// [`CapsuleCollider3D`] in `world` whose entity has a [`Transform`], with the entity's ID, by
/// table, then row: where its entity draws this frame as `poses` has it, between ticks and carried
/// by its ancestors, as physics shapes it, a box sized and offset as its entity scales, a sphere
/// around its offset so scaled, as large as its radius scaled by the largest of those scales, and a
/// capsule around its offset so scaled, as tall and as wide as its entity's scales make it. It
/// outlines a collider whether physics moves its entity or not, as it doesn't without a
/// [`RigidBody3D`] or under a parent, so that what's being made shows as it's made.
pub(super) fn outlines(world: &World, poses: &Poses<'_>, each: &mut impl FnMut(Outline, u64)) {
    let (Some(transform), boxed, round, capsuled) = (
        world.component::<Transform>(),
        world.component::<BoxCollider3D>(),
        world.component::<SphereCollider3D>(),
        world.component::<CapsuleCollider3D>(),
    ) else {
        return;
    };
    for (index, table) in world.tables().iter().enumerate() {
        let boxes = boxed.and_then(|id| table.column::<BoxCollider3D>(id));
        let spheres = round.and_then(|id| table.column::<SphereCollider3D>(id));
        let capsules = capsuled.and_then(|id| table.column::<CapsuleCollider3D>(id));
        if boxes.is_none() && spheres.is_none() && capsules.is_none() || !table.has(transform) {
            continue;
        }
        for (row, &entity) in table.entities().iter().enumerate() {
            let matrix = poses.matrix(index, row);
            let id = world.id(entity).expect("a table's rows are alive").get();
            if let Some(boxes) = boxes {
                let BoxCollider3D { offset, size, .. } = boxes[row];
                let placed = matrix * Mat4::from_translation(offset) * Mat4::from_scale(size);
                each(Outline::Cuboid(placed), id);
            }
            let (scales, rotation, _) = matrix.to_scale_rotation_translation();
            if let Some(spheres) = spheres {
                let SphereCollider3D { offset, radius, .. } = spheres[row];
                let middle = matrix.transform_point3(offset);
                let radius = radius * scales.abs().max_element();
                let placed =
                    Mat4::from_scale_rotation_translation(Vec3::splat(radius), rotation, middle);
                each(Outline::Sphere(placed), id);
            }
            if let Some(capsules) = capsules {
                let capsule = capsules[row];
                let middle = matrix.transform_point3(capsule.offset);
                let (radius, half) = capsule.shape(scales);
                let placed = Mat4::from_rotation_translation(rotation, middle);
                each(
                    Outline::Capsule {
                        placed,
                        radius,
                        half,
                    },
                    id,
                );
            }
        }
    }
}

/// `rotation` as rapier's, made a unit, as rapier's must be for its shapes to keep their sizes: no
/// turn, where it has no length to make one of.
fn rotation(rotation: Quat) -> Rotation {
    let unit = Vec4::from(rotation).try_normalize();
    unit.map_or(Rotation::IDENTITY, |unit| {
        Rotation::from_array(unit.as_dvec4().to_array())
    })
}

/// `v`, a component's, as rapier's, whose glam is another, of `f64`s.
fn vector(v: Vec3) -> Vector {
    Vector::from_array(v.as_dvec3().to_array())
}

/// `v`, a position, as rapier's.
pub(super) fn at(v: DVec3) -> Vector {
    Vector::from_array(v.to_array())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpolation::Past;
    use crate::placed::Placed;
    use crate::step_physics;

    /// A floor 10 units across and deep and a unit tall, its top at y 0.5, static.
    pub(super) fn floor(world: &mut World) -> Entity {
        let at = Transform {
            scale: Vec3::new(10.0, 1.0, 10.0),
            ..Transform::IDENTITY
        };
        let body = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        world.spawn((at, body, BoxCollider3D::default()))
    }

    /// A dynamic unit box at `at`, turned by `rotation`.
    pub(super) fn crate_at(world: &mut World, at: Vec3, rotation: Quat) -> Entity {
        let at = Transform {
            translation: at.as_dvec3(),
            rotation,
            ..Transform::IDENTITY
        };
        world.spawn((at, RigidBody3D::default(), BoxCollider3D::default()))
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

    /// Whether `a` turns as `b` does, within about a tenth of a degree.
    fn turned_as(a: Quat, b: Quat) -> bool {
        a.angle_between(b) < 2e-3
    }

    /// A box dropped onto a floor falls, lands and comes to rest on it, where it fell, unturned,
    /// its scale as it was; the floor, static, stays where it is, its components as they were.
    #[test]
    fn a_box_falls_onto_a_floor_and_rests_on_it() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let dropped = crate_at(&mut world, Vec3::new(0.5, 3.0, -0.25), Quat::IDENTITY);
        step(&mut world, 16);
        let falling = transform(&world, dropped);
        assert!(falling.translation.y < 3.0, "{falling:?}");
        let body = *world.get::<RigidBody3D>(dropped).expect("alive");
        assert!(body.velocity.y < -1.0, "{body:?}");
        step(&mut world, 240);
        let rested = transform(&world, dropped);
        let on_top = Vec3::new(0.5, 1.0, -0.25);
        assert!(
            rested.translation.abs_diff_eq(on_top.as_dvec3(), 0.02),
            "on the floor's top: {rested:?}"
        );
        assert!(turned_as(rested.rotation, Quat::IDENTITY), "{rested:?}");
        assert_eq!(rested.scale, Vec3::ONE);
        let body = *world.get::<RigidBody3D>(dropped).expect("alive");
        assert!(body.velocity.length() < 0.01, "at rest: {body:?}");
        assert_eq!(
            transform(&world, floor),
            Transform {
                scale: Vec3::new(10.0, 1.0, 10.0),
                ..Transform::IDENTITY
            }
        );
    }

    /// 1,000 km from the origin, a turned box falls onto a floor, tips and rests on it as one at
    /// the origin does, every step of its way within a tenth of a millimeter, its quaternion's
    /// parts within 1e-4, where `f32` would place it to the nearest 6 cm.
    #[test]
    fn far_from_the_origin_a_box_falls_and_rests_as_at_it() {
        let way = |offset: DVec3| {
            let mut world = World::default();
            let floor = floor(&mut world);
            let turned = Quat::from_rotation_x(0.3) * Quat::from_rotation_z(0.2);
            let dropped = crate_at(&mut world, Vec3::new(0.5, 3.0, -0.25), turned);
            for entity in [floor, dropped] {
                let at = world.get_mut::<Transform>(entity).expect("one");
                at.translation += offset;
            }
            let mut way = Vec::new();
            for _ in 0..30 {
                step(&mut world, 8);
                way.push(transform(&world, dropped));
            }
            way
        };
        let far = DVec3::new(1.0e6, -1.0e6, 1.0e6);
        let (near, far_way) = (way(DVec3::ZERO), way(far));
        let (mut most_apart, mut most_turned) = (0.0f64, 0.0f32);
        for (near, far_at) in near.iter().zip(&far_way) {
            let apart = far_at.translation - far - near.translation;
            most_apart = most_apart.max(apart.abs().max_element());
            let turned = Vec4::from(near.rotation) - Vec4::from(far_at.rotation);
            most_turned = most_turned.max(turned.abs().max_element());
        }
        // Its fall within a few nanometers; as it settles, rapier's contacts, which let it into the
        // floor by up to 5 mm, rest it within a few hundredths of a millimeter of the first.
        assert!(most_apart < 1e-4, "{most_apart}");
        assert!(most_turned < 1e-4, "{most_turned}");
        let rested = near.last().expect("stepped").translation;
        assert!(
            (rested.y - 1.0).abs() < 0.02,
            "on the floor's top: {rested}"
        );
    }

    /// A body at rest, asleep, leaves its entity's components unchanged as the steps go on, so that
    /// what follows their changes, as the renderer's scene does, has nothing to do for it.
    #[test]
    fn bodies_at_rest_change_nothing() {
        let mut world = World::default();
        floor(&mut world);
        let dropped = crate_at(&mut world, Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        step(&mut world, 480);
        let physics = world.resource::<Physics3D>().expect("stepped");
        let linked = physics.links.linked.iter().find(|it| it.entity == dropped);
        let body = linked.expect("linked").body;
        assert!(physics.rapier.bodies[body].is_sleeping());
        world.advance();
        let tick = world.change_tick();
        step(&mut world, 8);
        let mut tables = world.tables().iter();
        let table = tables.find(|table| table.entities().contains(&dropped));
        let table = table.expect("in a table");
        let row = table
            .entities()
            .iter()
            .position(|&entity| entity == dropped);
        let row = row.expect("a row");
        for id in [
            world.component::<Transform>(),
            world.component::<RigidBody3D>(),
        ] {
            let changed = table.changed(id.expect("registered")).expect("a column");
            assert!(changed.row(row) < tick, "{} {tick}", changed.row(row));
        }
    }

    /// A component set to a NaN or an infinity is refused, warned of once while it stays so
    /// (`physics.not_finite`), and doesn't reach rapier, where it would spread through contacts:
    /// a body at rest whose transform isn't finite stays where it was, at rest, its neighbor
    /// finite, through 30 steps that warn once, as does a character's; a falling body's goes on
    /// falling from its last, writing where it goes over the refused value at the next step; a
    /// velocity keeps the last that was, and a box's size the box it had. A body whose transform
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
        let resting = crate_at(&mut world, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        let touching = crate_at(&mut world, Vec3::new(1.0, 1.0, 0.0), Quat::IDENTITY);
        let walker = super::character::tests::character(&mut world, Vec3::new(-3.0, 1.52, 0.0));
        step(&mut world, 240);
        let body_of = |world: &World, entity| {
            let physics = world.resource::<Physics3D>().expect("stepped");
            physics.links.get(entity).expect("linked").body
        };
        let held = transform(&world, resting);
        let stood = transform(&world, walker);
        let nan = Quat::from_xyzw(0.0, f32::NAN, 0.0, 1.0);
        world.get_mut::<Transform>(resting).expect("alive").rotation = nan;
        world
            .get_mut::<Transform>(walker)
            .expect("alive")
            .translation
            .y = f64::from(f32::INFINITY);
        step(&mut world, 30);
        assert_eq!(warned(), 2, "once each, though they stepped 30 times");
        assert!(
            transform(&world, resting).rotation.is_nan(),
            "left to the game"
        );
        let physics = world.resource::<Physics3D>().expect("stepped");
        let body = &physics.rapier.bodies[body_of(&world, resting)];
        let at = DVec3::from_array(body.translation().to_array());
        assert!(
            body.is_sleeping() && body.rotation().is_finite(),
            "where it was"
        );
        assert!(at.abs_diff_eq(held.translation, 1e-4), "{at}");
        let character = physics.characters.get(walker).expect("linked");
        let stands = physics.rapier.bodies[character.body].translation();
        assert!(DVec3::from_array(stands.to_array()).abs_diff_eq(stood.translation, 1e-3));
        assert!(transform(&world, touching).translation.is_finite());
        world.get_mut::<Transform>(resting).expect("alive").rotation = Quat::IDENTITY;
        world
            .get_mut::<Transform>(resting)
            .expect("alive")
            .translation = DVec3::new(-1.0, 1.0, 2.0);
        step(&mut world, 1);
        let taken = transform(&world, resting).translation;
        assert!(
            taken.abs_diff_eq(DVec3::new(-1.0, 1.0, 2.0), 0.05),
            "{taken}"
        );
        // Falling, its body's pose is written over the value refused.
        let falling = crate_at(&mut world, Vec3::new(3.0, 6.0, 0.0), Quat::IDENTITY);
        step(&mut world, 8);
        world
            .get_mut::<Transform>(falling)
            .expect("alive")
            .translation
            .y = f64::from(f32::NAN);
        step(&mut world, 1);
        assert_eq!(warned(), 1);
        let fell = transform(&world, falling).translation;
        assert!(fell.is_finite() && fell.y < 6.0 && fell.y > 4.0, "{fell}");
        // A velocity and a box's size, each kept as it was.
        let rigid_body = world.get_mut::<RigidBody3D>(touching).expect("alive");
        rigid_body.velocity = Vec3::new(f32::NAN, 0.0, 0.0);
        world
            .get_mut::<BoxCollider3D>(touching)
            .expect("alive")
            .size
            .x = f32::INFINITY;
        step(&mut world, 30);
        assert_eq!(warned(), 1, "once for both");
        let physics = world.resource::<Physics3D>().expect("stepped");
        let body = &physics.rapier.bodies[body_of(&world, touching)];
        assert!(body.linvel().is_finite() && body.translation().is_finite());
        let collider = body.colliders()[0];
        let aabb = physics.rapier.colliders[collider].compute_aabb();
        assert!(aabb.maxs.is_finite() && aabb.mins.is_finite(), "{aabb:?}");
        let spawned = Transform {
            translation: DVec3::new(f64::from(f32::NAN), 3.0, 0.0),
            ..Transform::IDENTITY
        };
        let unmade = world.spawn((spawned, RigidBody3D::default(), BoxCollider3D::default()));
        step(&mut world, 4);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert!(physics.links.get(unmade).is_none(), "not made");
        assert_eq!(warned(), 1);
        world
            .get_mut::<Transform>(unmade)
            .expect("alive")
            .translation
            .x = 3.0;
        step(&mut world, 1);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert!(physics.links.get(unmade).is_some(), "made once it's finite");
    }

    /// A pile of boxes, spheres and capsules in turn, turned and overlapping as they start, over a
    /// floor.
    fn pile(world: &mut World) -> Vec<Entity> {
        floor(world);
        let mut bodies = Vec::new();
        for i in 0..18u8 {
            let i = f32::from(i);
            let at = Vec3::new((i % 3.0) * 0.8 - 0.8, 1.5 + i * 0.7, (i % 2.0) * 0.8 - 0.4);
            let turned = Quat::from_rotation_x(i * 0.3) * Quat::from_rotation_z(i * 0.2);
            let body = crate_at(world, at, turned);
            if i % 3.0 != 0.0 {
                world.remove::<BoxCollider3D>(body).expect("alive");
                let added = if i % 3.0 == 1.0 {
                    world.insert(body, SphereCollider3D::default())
                } else {
                    let capsule = CapsuleCollider3D {
                        height: 1.2,
                        radius: 0.3,
                        ..CapsuleCollider3D::default()
                    };
                    world.insert(body, capsule)
                };
                added.expect("alive");
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
        remade.remove_resource::<Physics3D>();
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
        world.register_saved::<Physics3D>().expect("saved");
        let bodies = pile(&mut world);
        step(&mut world, 40);
        // An edit since the last step, which the next takes.
        let pushed = world.get_mut::<RigidBody3D>(bodies[5]).expect("a body");
        pushed.velocity = Vec3::new(3.0, 1.0, -2.0);
        let mut text = String::new();
        world.save_whole(&mut text).expect("saved");
        assert!(text.contains("\nresource Physics3D "), "{text}");
        let mut loaded = World::seeded(1);
        for registered in [
            loaded.register::<Transform>(),
            loaded.register::<RigidBody3D>(),
            loaded.register::<BoxCollider3D>(),
            loaded.register::<SphereCollider3D>(),
            loaded.register::<CapsuleCollider3D>(),
            loaded.register_saved::<Physics3D>(),
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
        let physics = world.resource::<Physics3D>().expect("stepped");
        let text = physics.save(&world).expect("saved");
        Physics3D::load(&text, &world).expect("its own text loads");
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
            let refused = Physics3D::load(&changed, &world)
                .err()
                .map(|error| error.code());
            assert_eq!(refused, Some("physics.load"), "change {i}");
        }
    }

    /// The world's hash after physics has stepped a pile of boxes, spheres and capsules for four
    /// seconds, which CI checks on every platform: a difference on some platforms only is a bug in
    /// determinism, not a value to change here.
    #[test]
    fn a_pile_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0xc7bb_d25c_50b2_cc7d, "{content:#018x}");
    }

    /// What a game or an edit changes of a body's components between steps, the next step takes:
    /// where it is and how it's turned, how fast it goes, how it moves, and its collider.
    #[test]
    fn changes_between_steps_reach_the_bodies() {
        let mut world = World::default();
        floor(&mut world);
        let moved = crate_at(&mut world, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        step(&mut world, 4);
        let placed = Vec3::new(3.0, 1.0, -2.0);
        world
            .get_mut::<Transform>(moved)
            .expect("alive")
            .translation = placed.as_dvec3();
        step(&mut world, 1);
        let at = transform(&world, moved).translation;
        assert!(at.abs_diff_eq(placed.as_dvec3(), 1e-3), "{at}");
        world.get_mut::<RigidBody3D>(moved).expect("alive").velocity = Vec3::new(0.0, 8.0, 0.0);
        step(&mut world, 1);
        assert!(transform(&world, moved).translation.y > 1.1);
        let rigid_body = world.get_mut::<RigidBody3D>(moved).expect("alive");
        rigid_body.body_type = BodyType::Static;
        let held = transform(&world, moved);
        step(&mut world, 30);
        assert_eq!(transform(&world, moved), held, "static: where it was put");
        let rigid_body = world.get_mut::<RigidBody3D>(moved).expect("alive");
        rigid_body.body_type = BodyType::Dynamic;
        rigid_body.velocity = Vec3::ZERO;
        let collider = world.get_mut::<BoxCollider3D>(moved).expect("alive");
        collider.size = Vec3::new(1.0, 3.0, 1.0);
        step(&mut world, 240);
        let rested = transform(&world, moved);
        assert!(
            (rested.translation.y - 2.0).abs() < 0.02,
            "on the floor, 3 tall: {rested:?}"
        );
        let turned = Quat::from_rotation_y(0.5);
        world.get_mut::<Transform>(moved).expect("alive").rotation = turned;
        step(&mut world, 1);
        let rotation = transform(&world, moved).rotation;
        assert!(turned_as(rotation, turned), "{rotation:?}");
    }

    /// A small ball going fast enough that a step carries it from one side of a thin kinematic wall
    /// to the other passes through it, but one with continuous collision stops at it, and stays
    /// this side, as does one given continuous collision between steps, from the next; and one
    /// without stops at a thin static wall, as every fast body is swept against what's static.
    #[test]
    fn continuous_collision_stops_what_a_step_would_carry_through_a_thin_wall() {
        let mut world = World::default();
        let wall = |world: &mut World, z: f32, body_type| {
            let wall = Transform {
                translation: DVec3::new(5.0, 0.0, f64::from(z)),
                scale: Vec3::new(0.05, 4.0, 3.0),
                ..Transform::IDENTITY
            };
            let body = RigidBody3D {
                body_type,
                ..RigidBody3D::default()
            };
            world.spawn((wall, body, BoxCollider3D::default()));
        };
        wall(&mut world, -2.0, BodyType::Kinematic);
        wall(&mut world, 2.0, BodyType::Static);
        // 0.9375 units a step: from x 0, at 4.6875, short of the walls, then at 5.625, past them.
        let fired = |world: &mut World, z: f32, continuous_collision: bool| {
            let at = Transform {
                translation: DVec3::new(0.0, 0.0, f64::from(z)),
                ..Transform::IDENTITY
            };
            let body = RigidBody3D {
                continuous_collision,
                velocity: Vec3::new(60.0, 0.0, 0.0),
                ..RigidBody3D::default()
            };
            let ball = SphereCollider3D {
                radius: 0.1,
                ..SphereCollider3D::default()
            };
            world.spawn((at, body, ball))
        };
        let through = fired(&mut world, -3.0, false);
        let stopped = fired(&mut world, -2.0, true);
        let late = fired(&mut world, -1.0, false);
        let swept = fired(&mut world, 2.0, false);
        step(&mut world, 1);
        world
            .get_mut::<RigidBody3D>(late)
            .expect("alive")
            .continuous_collision = true;
        step(&mut world, 15);
        let x = |entity| transform(&world, entity).translation.x;
        assert!(x(through) > 5.5, "through the wall: {}", x(through));
        for ball in [stopped, late, swept] {
            assert!(x(ball) < 4.975, "stopped this side: {}", x(ball));
        }
    }

    /// A sphere is as large as its radius scaled by the largest of its entity's scales, and a
    /// capsule as tall as its height scaled along Y and as wide as its radius scaled by the larger
    /// of its scales along X and Z; and one no taller than twice its radius is a sphere of it.
    #[test]
    fn shapes_are_as_large_as_their_scales() {
        let mut world = World::default();
        floor(&mut world);
        let dropped = |world: &mut World, x: f32, scale: Vec3| {
            let at = Transform {
                translation: DVec3::new(f64::from(x), 4.0, 0.0),
                scale,
                ..Transform::IDENTITY
            };
            let upright = RigidBody3D {
                fixed_rotation: true,
                ..RigidBody3D::default()
            };
            world.spawn((at, upright))
        };
        let sphere = dropped(&mut world, -3.0, Vec3::new(1.0, 2.0, 3.0));
        world
            .insert(sphere, SphereCollider3D::default())
            .expect("alive");
        let tall = dropped(&mut world, 0.0, Vec3::new(2.0, 1.5, 1.0));
        world
            .insert(tall, CapsuleCollider3D::default())
            .expect("alive");
        let short = dropped(&mut world, 3.0, Vec3::ONE);
        let short_capsule = CapsuleCollider3D {
            height: 0.5,
            ..CapsuleCollider3D::default()
        };
        world.insert(short, short_capsule).expect("alive");
        step(&mut world, 240);
        // The floor's top and each one's half height.
        for (entity, middle) in [(sphere, 2.0), (tall, 2.0), (short, 1.0)] {
            let y = transform(&world, entity).translation.y;
            assert!((y - middle).abs() < 0.02, "{y}, not {middle}");
        }
    }

    /// A body collides as each of its colliders, and as each changes between steps: a box kept
    /// upright on a sphere under it, as a foot, rests on the sphere, then on its box once the
    /// sphere's taken away.
    #[test]
    fn a_body_collides_as_each_of_its_colliders() {
        let mut world = World::default();
        floor(&mut world);
        let at = Transform {
            translation: DVec3::new(0.0, 4.0, 0.0),
            ..Transform::IDENTITY
        };
        let upright = RigidBody3D {
            fixed_rotation: true,
            ..RigidBody3D::default()
        };
        let foot = SphereCollider3D {
            offset: Vec3::new(0.0, -1.0, 0.0),
            ..SphereCollider3D::default()
        };
        let body = world.spawn((at, upright, BoxCollider3D::default(), foot));
        step(&mut world, 240);
        // The floor's top, the sphere's radius, then its offset.
        let on_foot = transform(&world, body).translation.y;
        assert!((on_foot - 2.0).abs() < 0.02, "on its foot: {on_foot}");
        world.remove::<SphereCollider3D>(body).expect("alive");
        step(&mut world, 240);
        let on_box = transform(&world, body).translation.y;
        assert!((on_box - 1.0).abs() < 0.02, "on its box: {on_box}");
    }

    /// A kinematic body goes by its velocity alone, gravity aside, pushing a dynamic box in its
    /// way, and turns by its angular velocity; a dynamic box whose rotation is fixed doesn't tip
    /// off the floor's edge, where one whose rotation isn't does.
    #[test]
    fn kinematic_bodies_push_and_turn_and_fixed_rotations_stay() {
        let mut world = World::default();
        floor(&mut world);
        let kinematic = |velocity, angular_velocity| RigidBody3D {
            body_type: BodyType::Kinematic,
            velocity,
            angular_velocity,
            ..RigidBody3D::default()
        };
        let at = |translation| Transform {
            translation,
            ..Transform::IDENTITY
        };
        let pusher = world.spawn((
            at(DVec3::new(-3.0, 1.0, 0.0)),
            kinematic(Vec3::new(2.0, 0.0, 0.0), Vec3::ZERO),
            BoxCollider3D::default(),
        ));
        let spinner = world.spawn((
            at(DVec3::new(0.0, 1.0, 3.0)),
            kinematic(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)),
            BoxCollider3D::default(),
        ));
        let pushed = crate_at(&mut world, Vec3::new(-1.0, 1.0, 0.0), Quat::IDENTITY);
        step(&mut world, 64);
        let pushing = transform(&world, pusher).translation;
        assert!(
            pushing.abs_diff_eq(DVec3::new(-1.0, 1.0, 0.0), 1e-4),
            "{pushing}"
        );
        // Pushed half a unit, a unit in front of the pusher, touching it.
        let ahead = transform(&world, pushed).translation.x;
        assert!((ahead - pushing.x - 1.0).abs() < 0.02, "{ahead}");
        let spun = transform(&world, spinner);
        assert!(
            turned_as(spun.rotation, Quat::from_rotation_y(1.0)),
            "{spun:?}"
        );
        assert_eq!(spun.translation, DVec3::new(0.0, 1.0, 3.0));
        let mut world = World::default();
        floor(&mut world);
        let tipping = crate_at(&mut world, Vec3::new(5.2, 1.0, 0.0), Quat::IDENTITY);
        let upright = crate_at(&mut world, Vec3::new(-5.2, 1.0, 0.0), Quat::IDENTITY);
        world
            .get_mut::<RigidBody3D>(upright)
            .expect("alive")
            .fixed_rotation = true;
        step(&mut world, 120);
        let tipped = transform(&world, tipping).rotation;
        assert!(tipped.angle_between(Quat::IDENTITY) > 0.3, "{tipped:?}");
        assert_eq!(transform(&world, upright).rotation, Quat::IDENTITY);
    }

    /// A body goes as its entity does, or as it loses its `RigidBody3D`, its entity staying as it
    /// was; one under a parent isn't moved, and is warned of once.
    #[test]
    fn bodies_go_with_their_entities_and_stay_under_parents() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let dropped = crate_at(&mut world, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        let stripped = crate_at(&mut world, Vec3::new(3.0, 1.0, 0.0), Quat::IDENTITY);
        let parent = world.spawn(Transform::IDENTITY);
        let child = crate_at(&mut world, Vec3::new(-3.0, 3.0, 0.0), Quat::IDENTITY);
        world.set_parent(child, Some(parent)).expect("no cycle");
        step(&mut world, 30);
        let resting = transform(&world, dropped).translation.y;
        world.despawn(floor).expect("alive");
        world.remove::<RigidBody3D>(stripped).expect("alive");
        let held = transform(&world, stripped);
        step(&mut world, 30);
        assert!(transform(&world, dropped).translation.y < resting - 0.5);
        assert_eq!(transform(&world, stripped), held);
        assert_eq!(transform(&world, child).translation.y, 3.0);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert_eq!(physics.links.linked.len(), 1);
        assert_eq!(physics.rapier.bodies.len(), 1);
        assert_eq!(physics.links.children, [child], "warned once");
    }

    /// Each collider's outline goes where its entity draws, with its entity's ID: a box sized and
    /// offset as its entity scales; a sphere around its offset so scaled, as large as its radius
    /// scaled by the largest of the scales; and a capsule around its offset so scaled, its height
    /// scaled along Y and its radius by the larger of the scales along X and Z, turned as its
    /// entity is; each of an entity with all three, in that order.
    #[test]
    fn colliders_outline_where_their_entities_draw() {
        let mut world = World::default();
        let turned = Quat::from_rotation_x(0.4) * Quat::from_rotation_y(0.7);
        let stretched = Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: turned,
            scale: Vec3::new(2.0, 0.5, 1.5),
        };
        let boxed = BoxCollider3D {
            offset: Vec3::new(0.25, -0.5, 0.5),
            size: Vec3::new(1.5, 2.0, 0.5),
            ..BoxCollider3D::default()
        };
        let round = SphereCollider3D {
            offset: Vec3::new(0.5, 1.0, -1.0),
            radius: 0.75,
            ..SphereCollider3D::default()
        };
        let capsuled = CapsuleCollider3D {
            offset: Vec3::new(0.0, 1.0, 0.0),
            height: 6.0,
            radius: 0.5,
            ..CapsuleCollider3D::default()
        };
        let all = world.spawn((stretched, boxed, round, capsuled));
        world.spawn(boxed);
        let past = Past::default();
        let placed = Placed::of(&world, &past, 1.0);
        let poses = Poses::new(&world, &past, &placed, 1.0, DVec3::ZERO).expect("transforms");
        let mut found = Vec::new();
        outlines(&world, &poses, &mut |outline, id| found.push((outline, id)));
        let id = world.id(all).expect("alive").get();
        let [
            (Outline::Cuboid(cube), first),
            (Outline::Sphere(sphere), second),
            (
                Outline::Capsule {
                    placed: capsule,
                    radius,
                    half,
                },
                third,
            ),
        ] = found[..]
        else {
            panic!("{found:?}");
        };
        assert_eq!([first, second, third], [id; 3]);
        let near = |a: Vec3, b: Vec3| a.abs_diff_eq(b, 1e-5);
        let entitys = stretched.matrix();
        // The box's corner: its offset and half its size, in its entity's space.
        let corner = cube.transform_point3(Vec3::splat(0.5));
        let want = entitys.transform_point3(DVec3::new(1.0, 0.5, 0.75));
        assert!(near(corner, want.as_vec3()), "{corner} {want}");
        // The sphere's middle, and its surface 0.75 times 2 from it, every way.
        let middle = sphere.transform_point3(Vec3::ZERO);
        assert!(near(
            middle,
            entitys.transform_point3(round.offset.as_dvec3()).as_vec3()
        ));
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let reach = sphere.transform_point3(axis).distance(middle);
            assert!((reach - 1.5).abs() < 1e-5, "{reach}");
        }
        // The capsule's middle, its radius 0.5 times 2, and its segment along its entity's Y, as
        // far each way as half of 6 times 0.5, less its radius.
        let middle = capsule.transform_point3(Vec3::ZERO);
        assert!(near(
            middle,
            entitys
                .transform_point3(capsuled.offset.as_dvec3())
                .as_vec3()
        ));
        assert!((radius - 1.0).abs() < 1e-6 && (half - 0.5).abs() < 1e-6);
        let along = capsule.transform_vector3(Vec3::Y);
        assert!(near(along, turned * Vec3::Y), "{along}");
    }

    /// A context whose world has the level's model, a floor at y 0, and a static root of it at
    /// `at` with a `MeshCollider3D`.
    fn level_at(at: Transform) -> (crate::Context, Entity) {
        let mut cx = crate::Context::seeded(7);
        let (model, _) = crate::model::tests::added(&mut cx, "level", crate::model::tests::level());
        let world = cx.world_mut();
        let root = model.spawn(world, at);
        let body = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        world.insert(root, body).expect("alive");
        world
            .insert(root, MeshCollider3D::default())
            .expect("alive");
        (cx, root)
    }

    /// A box dropped onto a model's triangles rests on them, a character walks on them, and a ray
    /// down hits them, giving the model's root; scaled, they're where its scale puts them.
    #[test]
    fn bodies_and_characters_stand_on_a_models_triangles() {
        let (mut cx, root) = level_at(Transform::IDENTITY);
        let world = cx.world_mut();
        let dropped = crate_at(world, Vec3::new(1.0, 3.0, -2.0), Quat::IDENTITY);
        let walker = super::character::tests::character(world, Vec3::new(-3.0, 3.0, 0.0));
        step(world, 120);
        let rested = transform(world, dropped).translation;
        assert!((rested.y - 0.5).abs() < 0.02, "{rested}");
        let grounded = world
            .get::<CharacterController3D>(walker)
            .expect("alive")
            .grounded;
        assert!(grounded, "standing on the triangles");
        let down = Cast::new(DVec3::new(4.0, 2.0, 4.0), Vec3::NEG_Y);
        let hit = world.cast_ray(down).expect("the floor");
        assert_eq!(hit.entity, root);
        assert!(
            hit.normal.abs_diff_eq(Vec3::Y, 1e-5) && hit.point.y.abs() < 1e-5,
            "{hit:?}"
        );
        let (mut cx, _) = level_at(Transform {
            translation: DVec3::new(0.0, 1.0, 0.0),
            scale: Vec3::new(0.25, 2.0, 0.25),
            ..Transform::IDENTITY
        });
        let world = cx.world_mut();
        step(world, 1);
        let narrow = world.cast_ray(Cast::new(DVec3::new(3.0, 5.0, 0.0), Vec3::NEG_Y));
        assert_eq!(narrow, None, "past its scaled edge, 2.5 from its middle");
        let hit = world.cast_ray(Cast::new(DVec3::new(2.0, 5.0, 0.0), Vec3::NEG_Y));
        assert!((hit.expect("within it").point.y - 1.0).abs() < 1e-5);
    }

    /// A mesh collider whose entity has no model collides as nothing, which it warns of, and what
    /// falls onto it falls through; one whose model is read again takes its new triangles.
    #[test]
    fn mesh_colliders_follow_their_models() {
        let (mut cx, root) = level_at(Transform::IDENTITY);
        let world = cx.world_mut();
        let body = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        let modelless = world.spawn((Transform::IDENTITY, body, MeshCollider3D::default()));
        step(world, 1);
        let physics = world.resource::<Physics3D>().expect("stepped");
        let collider = |entity| {
            let linked = physics
                .links
                .linked
                .iter()
                .find(|linked| linked.entity == entity);
            linked.expect("linked").colliders[3]
        };
        assert!(collider(root).is_some() && collider(modelless).is_none());
        let dropped = crate_at(world, Vec3::new(30.0, 1.0, 0.0), Quat::IDENTITY);
        world
            .get_mut::<Transform>(modelless)
            .expect("alive")
            .translation
            .x = 30.0;
        step(world, 32);
        assert!(transform(world, dropped).translation.y < 0.0, "through it");
        // The model read again, as a development run reads it as its file changes, its floor a
        // unit higher.
        #[cfg(feature = "dev")]
        {
            let ModelRoot(model) = *world.get::<ModelRoot>(root).expect("a root");
            let raised = crate::model::tests::surface(
                vec![[-10.0, 1.0, -10.0], [-10.0, 1.0, 10.0], [10.0, 1.0, 10.0]],
                vec![0, 1, 2],
                Transform::IDENTITY,
            );
            let mut other = crate::Context::seeded(1);
            let (read, _) = crate::model::tests::added(&mut other, "raised", raised);
            let nodes = other.world().assets().nodes(read).expect("added").clone();
            world.assets_mut().set_nodes(model, nodes);
            step(world, 1);
            let hit = world.cast_ray(Cast::new(DVec3::new(-5.0, 5.0, 5.0), Vec3::NEG_Y));
            assert!(
                (hit.expect("the floor").point.y - 1.0).abs() < 1e-5,
                "{hit:?}"
            );
        }
    }

    /// A world standing on a model's triangles, its physics saved to text and loaded before the
    /// model's surfaces are there, as a snapshot restored while its model loads is, keeps the
    /// triangles the text holds, and its contacts with them, as the surfaces come: it steps as the
    /// world it was saved from does, bit for bit, where a collider made again of them would drop
    /// its contacts' warm starts.
    #[test]
    fn a_mesh_collider_restored_before_its_model_keeps_its_shape() {
        let (mut cx, root) = level_at(Transform::IDENTITY);
        let world = cx.world_mut();
        for i in 0..4u8 {
            let i = f32::from(i);
            crate_at(
                world,
                Vec3::new(i - 1.5, 1.0 + i, 0.5 * i),
                Quat::from_rotation_x(i),
            );
        }
        step(world, 40);
        let physics = world.resource::<Physics3D>().expect("stepped");
        let text = physics.save(world).expect("saved");
        let mut loaded = Physics3D::load(&text, world).expect("loaded");
        // As a load finds it while its model is still loading, without its surfaces.
        let restored = loaded.links.get_mut(root).expect("linked");
        assert!(restored.surfaces.is_some() && restored.colliders[3].is_some());
        restored.surfaces = None;
        let mut copy = world.clone();
        copy.insert_resource(loaded);
        step(world, 60);
        step(&mut copy, 60);
        assert_eq!(copy.hash(), world.hash());
        let physics = copy.resource::<Physics3D>().expect("stepped");
        assert!(physics.links.get(root).expect("linked").surfaces.is_some());
        // One whose model changed while it was saved, as its file may have, is made again of the
        // model's triangles as they are now: its floor a unit higher.
        #[cfg(feature = "dev")]
        {
            let mut loaded = Physics3D::load(&text, world).expect("loaded");
            loaded.links.get_mut(root).expect("linked").surfaces = None;
            let mut copy = world.clone();
            copy.insert_resource(loaded);
            let ModelRoot(model) = *copy.get::<ModelRoot>(root).expect("a root");
            let raised = crate::model::tests::surface(
                vec![[-10.0, 1.0, -10.0], [-10.0, 1.0, 10.0], [10.0, 1.0, 10.0]],
                vec![0, 1, 2],
                Transform::IDENTITY,
            );
            let mut other = crate::Context::seeded(1);
            let (read, _) = crate::model::tests::added(&mut other, "raised", raised);
            let nodes = other.world().assets().nodes(read).expect("added").clone();
            copy.assets_mut().set_nodes(model, nodes);
            step(&mut copy, 1);
            let hit = copy.cast_ray(Cast::new(DVec3::new(-5.0, 5.0, 5.0), Vec3::NEG_Y));
            let hit = hit.expect("the raised floor");
            assert!((hit.point.y - 1.0).abs() < 1e-5, "{hit:?}");
        }
    }

    /// A world standing on a model's triangles, its physics saved to text, which holds them whole,
    /// and loaded, steps as it did, bit for bit.
    #[test]
    fn a_mesh_collider_saved_steps_as_it_did() {
        let (mut cx, _) = level_at(Transform::IDENTITY);
        let world = cx.world_mut();
        for i in 0..4u8 {
            let i = f32::from(i);
            crate_at(
                world,
                Vec3::new(i - 1.5, 1.0 + i, 0.5 * i),
                Quat::from_rotation_x(i),
            );
        }
        super::character::tests::character(world, Vec3::new(-3.0, 1.6, 2.0));
        step(world, 40);
        let physics = world.resource::<Physics3D>().expect("stepped");
        let text = physics.save(world).expect("saved");
        assert!(text.contains("TriMesh"), "the triangles whole");
        let loaded = Physics3D::load(&text, world).expect("loaded");
        let mut copy = world.clone();
        copy.insert_resource(loaded);
        step(world, 60);
        step(&mut copy, 60);
        assert_eq!(copy.hash(), world.hash());
    }

    /// The contact events of each of `ticks` steps.
    fn events(world: &mut World, ticks: u32) -> Vec<Vec<ContactEvent>> {
        (0..ticks)
            .map(|_| {
                step(world, 1);
                world.contact_events().collect()
            })
            .collect()
    }

    /// The contact events of the steps after a snapshot, of a pile falling through a sensor and a
    /// character walking into it, restored, as a rollback restores it, copied, or saved to text
    /// and loaded, are those the world it was taken from had at each, in the same order; and a
    /// dynamic body falls through the sensor as it would without it.
    #[test]
    fn contact_events_are_alike_after_a_restore() {
        let mut world = World::seeded(7);
        pile(&mut world);
        let at = Transform {
            translation: DVec3::new(0.0, 4.0, 0.0),
            scale: Vec3::new(4.0, 1.0, 4.0),
            ..Transform::IDENTITY
        };
        let fixed = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        let sensor = BoxCollider3D {
            sensor: true,
            ..BoxCollider3D::default()
        };
        world.spawn((at, fixed, sensor));
        let walker = super::character::tests::character(&mut world, Vec3::new(-4.0, 1.6, 0.0));
        world
            .get_mut::<CharacterController3D>(walker)
            .expect("alive")
            .velocity = Vec3::X;
        step(&mut world, 30);
        let snapshot = world.clone();
        let went = events(&mut world, 90);
        for kind in [false, true] {
            let any = went
                .iter()
                .flatten()
                .any(|event| event.contact.sensor == kind);
            assert!(any, "a sensor's: {kind}");
        }
        let mut copy = snapshot.clone();
        assert_eq!(events(&mut copy, 90), went, "a copy");
        world.clone_from(&snapshot);
        assert!(world.contact_events().eq(snapshot.contact_events()));
        assert_eq!(events(&mut world, 90), went, "restored");
        let physics = snapshot.resource::<Physics3D>().expect("stepped");
        let text = physics.save(&snapshot).expect("saved");
        let mut loaded = snapshot.clone();
        loaded.insert_resource(Physics3D::load(&text, &snapshot).expect("loaded"));
        assert!(loaded.contact_events().eq(snapshot.contact_events()));
        assert_eq!(events(&mut loaded, 90), went, "loaded");
    }
}
