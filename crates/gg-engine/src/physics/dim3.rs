//! 3D physics: bodies rapier moves at the world's ticks as their entities' components describe
//! them, characters its character controller moves ([`character`]), casts through their
//! colliders ([`query`]), and the resource holding rapier's bodies and contacts, which snapshots
//! copy and snapshot files hold.

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

/// A body 3D physics moves at each tick a layer runs [`step_physics`](crate::step_physics): its
/// entity's [`Transform`] (translation, rotation) and the velocities here, which each step reads
/// and writes back. It collides as its entity's [`BoxCollider3D`], [`SphereCollider3D`] and
/// [`CapsuleCollider3D`], any or all, which also give a dynamic body its mass; [`Joint3D`]s hold
/// it to other bodies. A body under a parent isn't moved: its transform is in its parent's space
/// (`physics.child_body`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct RigidBody3D {
    /// How it moves. Dynamic: by its velocity, which gravity and collisions change. Kinematic: by
    /// its velocity alone, pushing dynamic bodies aside, like a moving platform. Static: not at
    /// all, like a floor.
    pub body_type: BodyType,
    /// Whether it never turns, like an upright character; what sets its rotation still turns it.
    pub fixed_rotation: bool,
    /// Whether a fast step sweeps it along its path against moving bodies too (rapier's continuous
    /// collision), so a small fast ball stops at a thin kinematic or dynamic wall instead of
    /// passing through between steps. Fast dynamic bodies are swept against static ones anyway;
    /// this adds all bodies but others with it, like rapier's bullets, at some cost.
    pub continuous_collision: bool,
    /// Speed, in units per second.
    pub velocity: Vec3,
    /// Turn rate about the axis it points along, in radians per second, counterclockwise seen
    /// from the axis's tip.
    pub angular_velocity: Vec3,
}

/// The box a [`RigidBody3D`] on the same entity collides as, in the entity's space, so it turns
/// and scales with it. Default: a unit cube.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct BoxCollider3D {
    /// Its middle, from the entity's origin.
    pub offset: Vec3,
    /// Width, height and depth.
    #[reflect(min = 0.0, default = Vec3::ONE)]
    pub size: Vec3,
    /// Mass per cubic unit; a dynamic body's mass comes from it.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps
    /// ([`World::contacts`](crate::World::contacts)) but pushes nothing and nothing pushes it;
    /// characters move through it as casts pass through. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has; a character moves through
    /// what its capsule doesn't meet.
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
    /// Size and offset scaled, mirrored entities alike.
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
/// offset, which turns and scales with the entity. The radius scales by the largest of the
/// entity's scales, since a sphere stays one. Default: half a unit, a unit across.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct SphereCollider3D {
    /// Its middle, from the entity's origin.
    pub offset: Vec3,
    /// Distance from its middle to its surface.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// Mass per cubic unit; a dynamic body's mass comes from it.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps
    /// ([`World::contacts`](crate::World::contacts)) but pushes nothing and nothing pushes it;
    /// characters move through it as casts pass through. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has; a character moves through
    /// what its capsule doesn't meet.
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
    /// Offset scaled, radius by the largest scale.
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
/// like a standing character. It's around its offset, which turns and scales with the entity. Its
/// height scales by the entity's scale along Y, its radius by the larger of its scales along X and
/// Z, so it stays round. One no taller than twice its radius is a sphere of that radius. Default:
/// 2 units tall and half a unit in radius, a person's size where a unit is a meter.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct CapsuleCollider3D {
    /// Its middle, from the entity's origin.
    pub offset: Vec3,
    /// Height from the bottom of its lower cap to the top of its upper.
    #[reflect(min = 0.0, default = 2.0)]
    pub height: f32,
    /// Distance from the segment through its middle to its side.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// Mass per cubic unit; a dynamic body's mass comes from it.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps
    /// ([`World::contacts`](crate::World::contacts)) but pushes nothing and nothing pushes it;
    /// characters move through it as casts pass through. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has; a character moves through
    /// what its capsule doesn't meet.
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
    /// Its radius for `scale`, and how far its segment reaches each way of its middle.
    pub(crate) fn shape(&self, scale: Vec3) -> (f32, f32) {
        let scale = scale.abs();
        let radius = self.radius * scale.x.max(scale.z);
        // Compared rather than `max`ed, whose zero's sign std leaves to the platform.
        let half = self.height * scale.y * 0.5 - radius;
        (radius, if half > 0.0 { half } else { 0.0 })
    }

    /// Offset scaled, shape per [`shape`](Self::shape).
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
/// [`ModelRoot`] names, each node's where it is in the model, in the entity's space, so they turn
/// with it and scale along each axis as it does, like a level's floors and walls. Triangles hold
/// no volume, so it gives a body no mass and suits static and kinematic bodies; a dynamic one
/// takes its mass from its other colliders. An entity with no `ModelRoot`, or a model without
/// triangles (as one that didn't load), collides as nothing, warned of when made
/// (`physics.no_surface`). Its outline isn't drawn: its model shows where it is.
///
/// A world saved whole holds its triangles whole, as rapier serializes them, at a cost DEFECTS.md
/// weighs.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct MeshCollider3D {
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps
    /// ([`World::contacts`](crate::World::contacts)) but pushes nothing and nothing pushes it;
    /// characters move through it as casts pass through.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has; a character moves through
    /// what its capsule doesn't meet.
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
    /// The collider for `entity` with the surfaces of the model it's made of (a model read again
    /// replaces them): none, with a warning, if the entity has no model or it has no triangles.
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
            // No topology or pseudo-normals: they'd double what it holds and a whole save writes.
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

fn model_of(world: &World, entity: Entity) -> Option<crate::Model> {
    world.get::<ModelRoot>(entity).map(|root| root.0)
}

/// Whether `collider` holds, to the bit, the triangles [`MeshCollider3D`] would now make.
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

fn surfaces(world: &World, entity: Entity) -> Option<Arc<Vec<Surface>>> {
    let nodes = world.assets().nodes(model_of(world, entity)?)?;
    Some(Arc::clone(&nodes.surfaces))
}

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

/// `shape` with a collider's sensor flag and groups. A sensor finds kinematic and static bodies
/// too, characters among them (rapier's default: dynamic only), except that a static sensor
/// doesn't find static bodies.
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

fn groups(membership: u32, filter: u32) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(membership),
        Group::from_bits_retain(filter),
        InteractionTestMode::And,
    )
}

/// The world's 3D physics, a [`Resource`](crate::Resource): rapier's bodies, colliders, joints and
/// contacts, which each step continues from, and each body's and joint's entity, so a restored
/// snapshot steps as its world did. [`step_physics`](crate::step_physics) adds it at the first step
/// of a world with a [`RigidBody3D`] and none; a game may insert one first, with its gravity. A
/// world cleared, or whose entities a scene replaced, keeps it; the next step follows the entities
/// there.
pub struct Physics3D {
    /// Which way dynamic bodies fall, and how fast they speed up, in units per second squared:
    /// down along -Y at 9.81 by default, Earth's if a unit is a meter. Characters fall as much as
    /// each's [`CharacterController3D::gravity`] says.
    pub gravity: Vec3,
    rapier: Rapier,
    /// rapier's workspace. A copy makes it anew: no step carries anything in it over.
    pipeline: PhysicsPipeline,
    links: Links<Linked>,
    characters: Links<Character>,
    joints: Links<Jointed>,
    /// Controllers whose entities aren't characters (no `CapsuleCollider3D`, or a `RigidBody3D`),
    /// warned of once each.
    unmoved: Vec<Entity>,
    /// What a character hit as it moved; a workspace like `pipeline`.
    hits: Vec<CharacterCollision>,
    /// The kinematic bodies that go by their velocities, with those velocities, which the
    /// characters' moves hold them from meanwhile; a workspace too.
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

/// An entity's body, with its components as the last step left them, or as it took them in. A
/// change since is a game's or an edit's; the next step takes it.
#[derive(Clone)]
struct Linked {
    entity: Entity,
    body: RigidBodyHandle,
    /// Colliders of its `BoxCollider3D`, `SphereCollider3D`, `CapsuleCollider3D` and
    /// `MeshCollider3D`, in that order, for those it has.
    colliders: [Option<ColliderHandle>; 4],
    rigid_body: RigidBody3D,
    transform: Transform,
    box_collider: Option<BoxCollider3D>,
    sphere_collider: Option<SphereCollider3D>,
    capsule_collider: Option<CapsuleCollider3D>,
    mesh_collider: Option<MeshCollider3D>,
    /// Surfaces of the model its mesh collider was made of; a model read again replaces them, and
    /// the collider is remade.
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

    /// Copies `source` into this one's memory where rapier's types allow, keeping its own
    /// character workspace but not rapier's: its pipeline keeps the joints it last solved and the
    /// solver's layout for them, by counts the copied joint set restores, so kept it would solve
    /// another world's or gone joints. A new one reallocates in the next step.
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

/// What a snapshot file holds of [`Physics3D`] (`R`: rapier's state, owned or borrowed): gravity;
/// rapier's state through its own serialization, which continues exactly; each body, character
/// and joint with its entity's ID; IDs of entities warned of as children, or as controllers that
/// aren't characters; contacts and events. Missing joints or contacts read as none, so a world
/// saved without them loads.
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

/// A body in a snapshot file: its entity's ID (none if gone since the last step, which the next
/// takes the body out for); handles; the components as a recording's text writes values, so a
/// changed component type is matched as the component is.
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

#[derive(Serialize, Deserialize)]
struct SavedCharacter {
    entity: Option<u64>,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    controller: String,
    transform: String,
    capsule: String,
}

/// Saved through rapier's own serialization as RON, so a world restored in another process steps
/// on as the saved one would: contacts' warm starts, islands' sleep and handles stay, which bodies
/// remade from components would lose. Text a different rapier version can't read, or whose bodies
/// or joints name a rigid body, collider or joint its rapier state doesn't hold as theirs, leaves
/// physics out, to be remade from the entities (`physics.load`).
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

    /// Like [`save`](Saved::save), without the bodies and characters (and their joints) of
    /// entities `hides` says, taken out of a copy as a world's next step without them would. A
    /// client's world, which lacks them, reads only the bodies it sees, its rapier as its own step
    /// leaves it.
    fn save_seen(&self, world: &World, hides: &dyn Fn(Entity) -> bool) -> Result<String> {
        let bodies = self.links.linked.iter().map(|linked| linked.entity);
        let characters = self
            .characters
            .linked
            .iter()
            .map(|character| character.entity);
        if !bodies.chain(characters).any(hides) {
            return self.save(world);
        }
        let mut seen = self.clone();
        let rapier = &mut seen.rapier;
        seen.links.retain(|linked| {
            if !hides(linked.entity) {
                return true;
            }
            remove_body(rapier, linked.body);
            false
        });
        seen.characters.retain(|character| {
            if !hides(character.entity) {
                return true;
            }
            remove_body(rapier, character.body);
            false
        });
        seen.save(world)
    }

    fn load(text: &str, world: &World) -> Result<Self> {
        let saved: SavedPhysics<Rapier> =
            ron::from_str(text).map_err(|error| Error::new("physics.load", error.to_string()))?;
        let entity = |id: Option<u64>| {
            let id = id.and_then(EntityId::new);
            id.and_then(|id| world.entity(id)).unwrap_or(Entity::DEAD)
        };
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
        // A step reaches rapier by the links' handles, which text from elsewhere (a host or
        // server) may name wrongly: each body must be there and linked once, name its entity as
        // its link does (casts find it by that), have its colliders on it, each entity linked once.
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
        // A joint's handle must be in rapier's set, and linked once.
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

/// Steps `world`'s 3D physics a tick ([`step_physics`](crate::step_physics)), adding
/// [`Physics3D`] first if there's a [`RigidBody3D`] or [`CharacterController3D`] and none.
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
    /// Takes in changes (joints after the bodies and characters they join), moves characters as
    /// far as what they move into lets them, steps rapier, which carries them there, writes back.
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

    /// Takes out each body whose entity is gone, lost its `RigidBody3D` or `Transform`, or came
    /// under a parent; the rest keep their order.
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

    /// Writes each moving body's position, turn and velocities into its entity's components,
    /// each only if changed: a sleeping body leaves its rows unchanged, which the renderer's scene
    /// would write to the GPU again.
    fn write_back(&mut self, world: &mut World) {
        for linked in &mut self.links.linked {
            let body = &self.rapier.bodies[linked.body];
            if body.body_type() == RigidBodyType::Fixed {
                continue;
            }
            // Rapier's rotation and velocities are `f64`, the components' `f32`: a step takes
            // them in only once they change from what was written.
            let at = DVec3::from_array(body.translation().to_array());
            let turned = Quat::from_array(body.rotation().to_array().map(|x| x as f32));
            let velocity = Vec3::from_array(body.linvel().to_array().map(|x| x as f32));
            let angular_velocity = Vec3::from_array(body.angvel().to_array().map(|x| x as f32));
            // The components as this step took them in; a refused transform stays as it was, so
            // its NaNs aren't kept.
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

    /// 3D's alone of [`World::contacts`].
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.contacts.touching()
    }

    /// 3D's alone of [`World::contact_events`].
    pub fn contact_events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        self.contacts.events()
    }

    fn body_of(&self, entity: Entity) -> Option<RigidBodyHandle> {
        let body = self.links.get(entity).map(|linked| linked.body);
        body.or_else(|| self.characters.get(entity).map(|character| character.body))
    }
}

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

/// Each touching (or, with a sensor, overlapping) pair of colliders, as their bodies' entities
/// and whether a sensor's, in rapier's order.
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

/// Body user data: how casts and contacts find the entity.
fn user_data(entity: Entity) -> u128 {
    u128::from(entity.to_bits())
}

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

/// Takes into `linked`'s body what changed in its components since the last step left them,
/// waking it. A component that isn't finite is taken as it was ([`Links`] refuses it). Its mesh
/// collider is remade when its model is read again, except one restored from a snapshot before
/// its model's surfaces were there: that keeps the snapshot's shape and contacts if the surfaces,
/// once they come, make it.
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
        // Restored with its shape before its model's surfaces were there: kept, with its
        // contacts, if they'd make it; remade if the model has changed since, as its file may.
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

/// rapier's kind of body: kinematic by velocity, as Box2D's.
fn body_type(body_type: BodyType) -> RigidBodyType {
    match body_type {
        BodyType::Dynamic => RigidBodyType::Dynamic,
        BodyType::Kinematic => RigidBodyType::KinematicVelocityBased,
        BodyType::Static => RigidBodyType::Fixed,
    }
}

/// Swaps the collider `handle` holds for `made`, on `body`.
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

/// Gives `each` the outline and entity ID of each [`BoxCollider3D`], [`SphereCollider3D`] and
/// [`CapsuleCollider3D`] on an entity with a [`Transform`], by table, then row. It sits where the
/// entity draws this frame per `poses` (between ticks, carried by ancestors), shaped as physics
/// shapes it. Outlined whether physics moves the entity or not (no [`RigidBody3D`], or under a
/// parent), so what's being made shows.
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

/// `rotation` as rapier's, normalized so its shapes keep their sizes; no turn if it has no length.
fn rotation(rotation: Quat) -> Rotation {
    let unit = Vec4::from(rotation).try_normalize();
    unit.map_or(Rotation::IDENTITY, |unit| {
        Rotation::from_array(unit.as_dvec4().to_array())
    })
}

/// `v` as rapier's vector, whose glam is another, of `f64`s.
fn vector(v: Vec3) -> Vector {
    Vector::from_array(v.as_dvec3().to_array())
}

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

    /// A dropped box falls, lands and rests on a floor where it fell, unturned, its scale
    /// unchanged; the static floor stays as it was.
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

    /// 1,000 km from the origin, a turned box falls, tips and rests as one at the origin does,
    /// every step within 0.1 mm and its quaternion within 1e-4; `f32` would place it to 6 cm.
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
        // The fall agrees within nanometers; settling, rapier's contacts (up to 5 mm into the
        // floor) agree within hundredths of a millimeter.
        assert!(most_apart < 1e-4, "{most_apart}");
        assert!(most_turned < 1e-4, "{most_turned}");
        let rested = near.last().expect("stepped").translation;
        assert!(
            (rested.y - 1.0).abs() < 0.02,
            "on the floor's top: {rested}"
        );
    }

    /// A sleeping body leaves its entity's components unchanged, so change followers (the
    /// renderer's scene) have nothing to do for it.
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

    /// A NaN or infinity in a component is refused, warned of once while it stays
    /// (`physics.not_finite`), and never reaches rapier, where it would spread through contacts:
    /// a resting body with such a transform stays put, its neighbor finite, through 30 steps (a
    /// character likewise); a falling body keeps falling from its last finite transform,
    /// written over the refused value at the next step; a velocity or box size keeps its last
    /// finite value. A body with such a transform from the start isn't made until finite.
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
        // Falling, the body's pose is written over the refused value.
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
        // A velocity and a box's size keep their last values.
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

    /// Boxes, spheres and capsules in turn, turned and overlapping, over a floor.
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

    /// A snapshot of moving, touching bodies, restored or copied, steps as its source did, bit for
    /// bit: the world holds all physics carries between steps.
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

    /// A world of moving, touching bodies, saved whole to text and loaded into another world,
    /// steps as the saved one did, bit for bit; an edit since the last step is taken by the next.
    #[test]
    #[cfg(feature = "dev")]
    fn a_world_saved_whole_steps_as_it_did() {
        let mut world = World::seeded(7);
        world.register_saved::<Physics3D>().expect("saved");
        let bodies = pile(&mut world);
        step(&mut world, 40);
        // An edit since the last step.
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

    /// Physics saved for a client's view, which hides some entities, lacks their bodies and
    /// characters; stepped on in a world without those entities it matches physics saved whole,
    /// bit for bit, and the crate the client sees falls as the server's does.
    #[test]
    fn physics_seen_steps_on_as_physics_whole_does() {
        let mut world = World::seeded(3);
        floor(&mut world);
        let far = Transform {
            translation: DVec3::new(100.0, 0.0, 0.0),
            scale: Vec3::new(10.0, 1.0, 10.0),
            ..Transform::IDENTITY
        };
        let fixed = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        let far_floor = world.spawn((far, fixed, BoxCollider3D::default()));
        let near = crate_at(&mut world, Vec3::new(0.0, 2.0, 0.0), Quat::IDENTITY);
        let hidden = crate_at(&mut world, Vec3::new(100.0, 2.0, 0.0), Quat::IDENTITY);
        let walker = Transform {
            translation: DVec3::new(103.0, 2.0, 0.0),
            ..Transform::IDENTITY
        };
        let controller = CharacterController3D::default();
        let walker = world.spawn((walker, controller, CapsuleCollider3D::default()));
        step(&mut world, 20);
        let gone = [far_floor, hidden, walker];
        let hides = |entity: Entity| gone.contains(&entity);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert_eq!(physics.characters.linked.len(), 1);
        let whole = physics.save(&world).expect("saved");
        let seen = physics.save_seen(&world, &hides).expect("saved");
        assert_eq!(physics.save_seen(&world, &|_| false).expect("saved"), whole);
        let mut client = world.clone();
        for entity in gone {
            client.despawn(entity).expect("alive");
        }
        let [mut from_whole, mut from_seen] = [&whole, &seen].map(|text| {
            let mut copy = client.clone();
            let loaded = Physics3D::load(text, &copy).expect("loaded");
            copy.insert_resource(loaded);
            copy
        });
        let physics = from_seen.resource::<Physics3D>().expect("loaded");
        let held = (physics.rapier.bodies.len(), physics.characters.linked.len());
        assert_eq!(
            held,
            (2, 0),
            "the near floor's and crate's bodies, and no character"
        );
        step(&mut from_whole, 60);
        step(&mut from_seen, 60);
        let text = |world: &World| {
            let physics = world.resource::<Physics3D>().expect("stepped");
            physics.save(world).expect("saved")
        };
        assert_eq!(text(&from_seen), text(&from_whole));
        assert_eq!(from_seen.hash(), from_whole.hash());
        step(&mut world, 60);
        assert_eq!(transform(&from_seen, near), transform(&world, near));
    }

    /// Text naming what its rapier state doesn't hold as the body's is refused (`physics.load`),
    /// not stepped into a panic: a missing rigid body, another body's, another body's collider,
    /// an entity's second.
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

    /// The world's hash after four seconds of a pile, checked by CI on every platform: a difference
    /// on some platforms only is a determinism bug, not a value to change here.
    #[test]
    fn a_pile_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0xc7bb_d25c_50b2_cc7d, "{content:#018x}");
    }

    /// Changes between steps to a body's position, turn, velocity, type and collider reach it.
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

    /// A small ball fast enough to cross a thin kinematic wall in one step passes through it; one
    /// with continuous collision stops at it, as does one given it between steps, from the next;
    /// one without stops at a thin static wall, as every fast body is swept against static ones.
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

    /// A sphere's size is its radius times the largest scale; a capsule's height scales along Y
    /// and its radius by the larger of X and Z; one no taller than twice its radius is a sphere.
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
        // Floor top plus each one's half height.
        for (entity, middle) in [(sphere, 2.0), (tall, 2.0), (short, 1.0)] {
            let y = transform(&world, entity).translation.y;
            assert!((y - middle).abs() < 0.02, "{y}, not {middle}");
        }
    }

    /// A body collides as each of its colliders, and as each changes: an upright box with a
    /// sphere under it rests on the sphere, then on its box once the sphere's removed.
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
        // Floor top, sphere radius, offset.
        let on_foot = transform(&world, body).translation.y;
        assert!((on_foot - 2.0).abs() < 0.02, "on its foot: {on_foot}");
        world.remove::<SphereCollider3D>(body).expect("alive");
        step(&mut world, 240);
        let on_box = transform(&world, body).translation.y;
        assert!((on_box - 1.0).abs() < 0.02, "on its box: {on_box}");
    }

    /// A kinematic body moves by its velocity alone, pushing a dynamic box in its way, and turns
    /// by its angular velocity; a dynamic box with fixed rotation doesn't tip off the floor's
    /// edge, one without does.
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
        // A unit ahead of the pusher: half each, touching.
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

    /// A body goes with its entity, or when it loses its `RigidBody3D`, which stays as it was; one
    /// under a parent isn't moved, and is warned of once.
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

    /// Outlines go where their entity draws, with its ID: a box sized and offset by the scale; a
    /// sphere around its scaled offset, radius times the largest scale; a capsule around its
    /// scaled offset, height scaled along Y, radius by the larger of X and Z, turned with the
    /// entity; all three for an entity with all, in that order.
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
        // The box's corner: offset plus half its size, in the entity's space.
        let corner = cube.transform_point3(Vec3::splat(0.5));
        let want = entitys.transform_point3(DVec3::new(1.0, 0.5, 0.75));
        assert!(near(corner, want.as_vec3()), "{corner} {want}");
        // The sphere's middle, and its surface 0.75 times 2 from it.
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

    /// A context with the level's model, a floor at y 0, and a static root of it at `at` with a
    /// `MeshCollider3D`.
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

    /// A box dropped onto a model's triangles rests on them, a character walks on them, a ray down
    /// hits them (giving the model's root); scaled, they follow the scale.
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

    /// A mesh collider without a model collides as nothing, with a warning; one whose model is
    /// read again takes its new triangles.
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
        // The model read again (its floor a unit higher), as when its file changes.
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

    /// A world standing on a model's triangles, its physics saved and loaded before the model's
    /// surfaces are there (a snapshot restored while its model loads), keeps the text's triangles
    /// and contacts as the surfaces come: it steps like its source, bit for bit, where a collider
    /// remade from them would drop its contacts' warm starts.
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
        // As a load finds it while its model loads, without surfaces.
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
        // A model changed since the save, as its file may, is remade from its triangles as they
        // are now: its floor a unit higher.
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

    /// A world on a model's triangles, saved to text (which holds them whole) and loaded, steps
    /// as before, bit for bit.
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

    fn events(world: &mut World, ticks: u32) -> Vec<Vec<ContactEvent>> {
        (0..ticks)
            .map(|_| {
                step(world, 1);
                world.contact_events().collect()
            })
            .collect()
    }

    /// After a snapshot of a pile falling through a sensor, and a character walking into it, is
    /// restored (as a rollback does), copied, or saved and loaded, each step's contact events
    /// match the source's, in order; a dynamic body falls through the sensor as without it.
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
