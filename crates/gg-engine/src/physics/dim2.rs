//! 2D physics: bodies rapier moves at the world's ticks in the XY plane, as their entities'
//! components describe them, and the resource holding rapier's bodies and contacts, which
//! snapshots copy and snapshot files hold.

use std::collections::HashSet;

use gg_core::math::{DVec2, Mat4, Quat, Vec2, Vec3, libm};
use rapier2d::prelude::{
    ActiveCollisionTypes, BroadPhaseBvh, CCDSolver, Collider, ColliderBuilder, ColliderHandle,
    ColliderSet, Group, ImpulseJointSet, IntegrationParameters, InteractionGroups,
    InteractionTestMode, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    RigidBodyBuilder, RigidBodyHandle, RigidBodySet, RigidBodyType, Rotation, SoftBodySet, Vector,
};
use serde::{Deserialize, Serialize};

use super::contacts::{Contacts, SavedContacts};
use super::{
    BodyType, Contact, ContactEvent, Finite, Link, Links, Outline, kept, kept_collider, unfinite,
};
use crate::draw::Poses;
use crate::recording::text;
use crate::{Component, Entity, EntityId, Error, Reflect, Result, Saved, Tick, Transform, World};

mod joint;

pub(super) use joint::joint_lines;
pub use joint::{Joint2D, JointKind2D};
use joint::{Jointed, SavedJoint};

/// A body 2D physics moves at each [`step_physics`](crate::step_physics), in the XY plane.
///
/// Physics moves its entity's [`Transform`] (position, turn about Z) and the velocities here, which
/// each step reads and writes back. It collides as its entity's [`BoxCollider2D`] and
/// [`CircleCollider2D`], either or both, which also give a dynamic body its mass; [`Joint2D`]s hold
/// it to other bodies. A body under a parent isn't moved: its transform is in its parent's space
/// (`physics.child_body`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct RigidBody2D {
    /// How it moves. Dynamic: by its velocity, which gravity and collisions change. Kinematic: by
    /// its velocity alone, pushing dynamic bodies aside, like a moving platform. Static: not at
    /// all, like a floor.
    pub body_type: BodyType,
    /// Whether it never turns, like an upright character.
    pub fixed_rotation: bool,
    /// Whether a fast step sweeps it along its path against moving bodies too (rapier's continuous
    /// collision), so a small fast ball stops at a thin kinematic or dynamic wall instead of
    /// passing through between steps. Fast dynamic bodies are swept against static ones anyway;
    /// this adds all bodies but others with it, like rapier's bullets, at some cost.
    pub continuous_collision: bool,
    /// Speed across and up, in units per second.
    pub velocity: Vec2,
    /// Turn rate, counterclockwise, in radians per second.
    #[reflect(degrees)]
    pub angular_velocity: f32,
}

/// The box a [`RigidBody2D`] on the same entity collides as, in the entity's space, so it turns
/// and scales with it. Default: a unit square, a sprite's.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct BoxCollider2D {
    /// Its middle, from the entity's origin.
    pub offset: Vec2,
    /// Width and height.
    #[reflect(min = 0.0, default = Vec2::ONE)]
    pub size: Vec2,
    /// Mass per square unit; a dynamic body's mass comes from it.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps ([`World::contacts`]) but
    /// pushes nothing and nothing pushes it. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for BoxCollider2D {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            size: Vec2::ONE,
            density: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sensor: false,
            membership: u32::MAX,
            filter: u32::MAX,
        }
    }
}

impl BoxCollider2D {
    /// Size and offset scaled across and up, mirrored entities alike.
    fn collider(&self, scale: Vec3) -> Collider {
        let scale = scale.truncate();
        let half = (self.size * scale).abs() * 0.5;
        let shape = ColliderBuilder::cuboid(half.x.into(), half.y.into())
            .translation(vector(self.offset * scale))
            .density(self.density.into())
            .friction(self.friction.into())
            .restitution(self.restitution.into());
        met(shape, self.sensor, self.membership, self.filter).build()
    }
}

/// The circle a [`RigidBody2D`] on the same entity collides as, in the entity's space.
///
/// It's around its offset, which turns and scales with the entity. The radius scales by the larger
/// of the entity's scales across and up, since a circle stays one. Default: half a unit, a unit
/// across like a circle drawn on a sprite's square.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct CircleCollider2D {
    /// Its middle, from the entity's origin.
    pub offset: Vec2,
    /// Distance from its middle to its edge.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// Mass per square unit; a dynamic body's mass comes from it.
    #[reflect(min = 0.0, default = 1.0)]
    pub density: f32,
    /// How much it resists sliding: 0 none, like ice.
    #[reflect(min = 0.0, default = 0.5)]
    pub friction: f32,
    /// Fraction of impact speed it bounces back with: 0 none, 1 all.
    #[reflect(min = 0.0, max = 1.0)]
    pub restitution: f32,
    /// Whether it's a sensor, like a trigger zone: it finds overlaps ([`World::contacts`]) but
    /// pushes nothing and nothing pushes it. Its density still weighs its body.
    #[reflect(default = false, hashed_when_set)]
    pub sensor: bool,
    /// The groups it's in, a bit for each of 32: all by default.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub membership: u32,
    /// The groups whose colliders it meets, a bit for each: all by default. Two colliders touch or
    /// overlap only where each is in a group the other's filter has.
    #[reflect(default = u32::MAX, hashed_when_set)]
    pub filter: u32,
}

impl Default for CircleCollider2D {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
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

impl CircleCollider2D {
    fn collider(&self, scale: Vec3) -> Collider {
        let scale = scale.truncate();
        let shape = ColliderBuilder::ball((self.radius * scale.abs().max_element()).into())
            .translation(vector(self.offset * scale))
            .density(self.density.into())
            .friction(self.friction.into())
            .restitution(self.restitution.into());
        met(shape, self.sensor, self.membership, self.filter).build()
    }
}

/// `shape` with a collider's sensor flag and groups. A sensor finds kinematic and static bodies
/// too (rapier's default: dynamic only), except that a static sensor doesn't find static bodies.
fn met(shape: ColliderBuilder, sensor: bool, membership: u32, filter: u32) -> ColliderBuilder {
    let groups = InteractionGroups::new(
        Group::from_bits_retain(membership),
        Group::from_bits_retain(filter),
        InteractionTestMode::And,
    );
    let shape = shape.sensor(sensor).collision_groups(groups);
    if sensor {
        let types = ActiveCollisionTypes::all().difference(ActiveCollisionTypes::FIXED_FIXED);
        shape.active_collision_types(types)
    } else {
        shape
    }
}

/// The world's 2D physics, a [`Resource`](crate::Resource) holding rapier's state.
///
/// It has rapier's bodies, colliders, joints and contacts, which each step continues from, and each
/// body's and joint's entity, so a restored snapshot steps as its world did.
/// [`step_physics`](crate::step_physics) adds it at the first step of a world with a
/// [`RigidBody2D`] and none; a game may insert one first, with its gravity. A world cleared, or
/// whose entities a scene replaced, keeps it; the next step follows the entities there.
pub struct Physics2D {
    /// Which way dynamic bodies fall, and how fast they speed up, in units per second squared:
    /// down at 9.81 by default, Earth's if a unit is a meter.
    pub gravity: Vec2,
    rapier: Rapier,
    /// rapier's workspace. A copy makes it anew: no step carries anything in it over.
    pipeline: PhysicsPipeline,
    links: Links<Linked>,
    joints: Links<Jointed>,
    pub(super) contacts: Contacts,
}

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
    /// Colliders of its `BoxCollider2D` and `CircleCollider2D`, in that order, for those it has.
    colliders: [Option<ColliderHandle>; 2],
    rigid_body: RigidBody2D,
    transform: Transform,
    box_collider: Option<BoxCollider2D>,
    circle_collider: Option<CircleCollider2D>,
    /// The body's turn as rapier gave it; the next step writes the change into the transform.
    angle: f64,
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
                    dt: Tick::DT.as_secs_f64(),
                    ..IntegrationParameters::default()
                },
            },
            pipeline: PhysicsPipeline::new(),
            links: Links::default(),
            joints: Links::default(),
            contacts: Contacts::default(),
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
            contacts: self.contacts.clone(),
        }
    }

    /// Copies `source` into this one's memory where rapier's types allow, except the workspace:
    /// its pipeline keeps the joints it last solved and the solver's layout for them, by counts
    /// the copied joint set restores, so kept it would solve another world's or gone joints. A new
    /// one reallocates in the next step.
    fn clone_from(&mut self, source: &Self) {
        self.gravity = source.gravity;
        self.rapier.clone_from(&source.rapier);
        self.pipeline = PhysicsPipeline::new();
        self.links.clone_from(&source.links);
        self.joints.clone_from(&source.joints);
        self.contacts.clone_from(&source.contacts);
    }
}

/// What a snapshot file holds of [`Physics2D`] (`R`: rapier's state, owned or borrowed): gravity;
/// rapier's state through its own serialization, which continues exactly; each body and joint
/// with its entity's ID; IDs of entities warned of as children; contacts and events. Missing
/// joints or contacts read as none, so a world saved without them loads.
#[derive(Serialize, Deserialize)]
struct SavedPhysics<R> {
    gravity: [f32; 2],
    rapier: R,
    linked: Vec<SavedLinked>,
    children: Vec<Option<u64>>,
    #[serde(default)]
    joints: Vec<SavedJoint>,
    #[serde(default)]
    contacts: SavedContacts,
}

/// A body in a snapshot file: its entity's ID (none if gone since the last step, which the next
/// takes the body out for); handles; the components as a recording's text writes values, so a
/// changed component type is matched as the component is; its turn.
#[derive(Serialize, Deserialize)]
struct SavedLinked {
    entity: Option<u64>,
    body: RigidBodyHandle,
    colliders: [Option<ColliderHandle>; 2],
    rigid_body: String,
    transform: String,
    box_collider: Option<String>,
    circle_collider: Option<String>,
    angle: f64,
}

/// Saved through rapier's own serialization as RON, so a world restored in another process steps
/// on as the saved one would: contacts' warm starts, islands' sleep and handles stay, which bodies
/// remade from components would lose. Text a different rapier version can't read, or whose bodies
/// or joints name a rigid body, collider or joint its rapier state doesn't hold as theirs, leaves
/// physics out, to be remade from the entities (`physics.load`).
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
            contacts: self.contacts.save(),
        };
        ron::to_string(&saved).map_err(|error| Error::new("physics.save", error.to_string()))
    }

    /// Like [`save`](Saved::save), without the bodies (and their joints) of entities `hides`
    /// says, taken out of a copy as a world's next step without them would. A client's world,
    /// which lacks them, reads only the bodies it sees, its rapier as its own step leaves it.
    fn save_seen(&self, world: &World, hides: &dyn Fn(Entity) -> bool) -> Result<String> {
        if !self.links.linked.iter().any(|linked| hides(linked.entity)) {
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
        seen.save(world)
    }

    fn load(text: &str, world: &World) -> Result<Self> {
        let mut saved: SavedPhysics<Rapier> =
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
        // A step reaches rapier by the links' handles, which text from elsewhere (a host or
        // server) may name wrongly: each body must be there and linked once, its colliders on it,
        // each entity linked once.
        let refused = |entity: Entity, what: &str| {
            let who = world.id(entity);
            let who = who.map_or_else(|| "an entity gone".to_owned(), |id| format!("entity {id}"));
            Error::new("physics.load", format!("{who}'s body {what}"))
        };
        let rapier = &mut saved.rapier;
        let mut bodies = HashSet::with_capacity(linked.len());
        for linked in &linked {
            let body = rapier.bodies.get(linked.body);
            let Some(body) = body.filter(|_| bodies.insert(linked.body)) else {
                let what = "names no rigid body of rapier's, or another body's";
                return Err(refused(linked.entity, what));
            };
            let alive = linked.entity != Entity::DEAD;
            // Text from a build whose bodies held no entity has user data 0, which no entity's
            // bits are: set it, as contacts find the entity by it.
            if alive && body.user_data == 0 {
                let body = rapier.bodies.get_mut(linked.body).expect("there");
                body.user_data = user_data(linked.entity);
            } else if alive && body.user_data != user_data(linked.entity) {
                return Err(refused(
                    linked.entity,
                    "names another entity in rapier's state",
                ));
            }
            let on = |collider: &ColliderHandle| {
                rapier.colliders.get(*collider).and_then(Collider::parent) == Some(linked.body)
            };
            if !linked.colliders.iter().flatten().all(on) {
                let what = "names a collider that isn't on its rigid body";
                return Err(refused(linked.entity, what));
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
            contacts: Contacts::load(saved.contacts, world),
        })
    }
}

/// Steps `world`'s 2D physics a tick ([`step_physics`](crate::step_physics)), adding
/// [`Physics2D`] first if there's a [`RigidBody2D`] and none.
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
    /// Takes in changes (joints after their bodies), steps rapier, writes back.
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
        self.contacts.take(world, touching(&self.rapier));
        self.write_back(world);
    }

    /// 2D's alone of [`World::contacts`].
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.contacts.touching()
    }

    /// 2D's alone of [`World::contact_events`].
    pub fn contact_events(&self) -> impl Iterator<Item = ContactEvent> + '_ {
        self.contacts.events()
    }

    /// Takes out each body whose entity is gone, lost its `RigidBody2D` or `Transform`, or came
    /// under a parent; the rest keep their order.
    fn unlink_gone(&mut self, world: &World) {
        let rapier = &mut self.rapier;
        self.links.retain(|linked| {
            let entity = linked.entity;
            let has = world.get::<RigidBody2D>(entity).is_some()
                && world.get::<Transform>(entity).is_some();
            if has && matches!(world.parent(entity), Ok(None)) {
                return true;
            }
            remove_body(rapier, linked.body);
            false
        });
    }

    /// Writes each moving body's position, turn and velocities into its entity's components,
    /// each only if changed, to the bit: a sleeping body leaves its rows unchanged, which the
    /// hash's incremental parts and the open scene's check read. It writes from what the step
    /// took in, so a refused transform's NaNs aren't carried over.
    fn write_back(&mut self, world: &mut World) {
        for linked in &mut self.links.linked {
            let body = &self.rapier.bodies[linked.body];
            if body.body_type() == RigidBodyType::Fixed {
                continue;
            }
            let at = body.translation();
            let turned = body.rotation().angle();
            // Rapier's velocities are `f64`, the components' `f32`: a step takes them in only
            // once they change from what was written.
            let linvel = body.linvel();
            let velocity = Vec2::new(linvel.x as f32, linvel.y as f32);
            let angular_velocity = body.angvel() as f32;
            let was = &mut linked.transform;
            let moved = at.x.to_bits() != was.translation.x.to_bits()
                || at.y.to_bits() != was.translation.y.to_bits();
            let bits = |a: f32, b: f32| a.to_bits() != b.to_bits();
            if moved || turned != linked.angle {
                if turned != linked.angle {
                    let by = Quat::from_rotation_z((turned - linked.angle) as f32);
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

/// Body user data: how contacts find the entity.
fn user_data(entity: Entity) -> u128 {
    u128::from(entity.to_bits())
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

fn make_body(rapier: &mut Rapier, world: &World, entity: Entity) -> Linked {
    let rigid_body = *world.get::<RigidBody2D>(entity).expect("found with one");
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let box_collider = world.get::<BoxCollider2D>(entity).copied();
    let circle_collider = world.get::<CircleCollider2D>(entity).copied();
    let mut built = RigidBodyBuilder::new(body_type(rigid_body.body_type))
        .translation(at(transform.translation.truncate()))
        .rotation(angle(transform.rotation))
        .linvel(vector(rigid_body.velocity))
        .angvel(rigid_body.angular_velocity.into())
        .ccd_enabled(rigid_body.continuous_collision)
        .user_data(user_data(entity));
    if rigid_body.fixed_rotation {
        built = built.lock_rotations();
    }
    let body = rapier.bodies.insert(built);
    // As rapier returns it: possibly another float than given.
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

/// Takes into `linked`'s body what changed in its components since the last step left them,
/// waking it. A component that isn't finite is taken as it was ([`Links`] refuses it).
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
        body.set_angvel(rigid_body.angular_velocity.into(), true);
    }
    let moved = transform.translation.truncate();
    if moved != linked.transform.translation.truncate() {
        body.set_translation(at(moved), true);
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

/// rapier's kind of body: kinematic by velocity, as Box2D's.
fn body_type(body_type: BodyType) -> RigidBodyType {
    match body_type {
        BodyType::Dynamic => RigidBodyType::Dynamic,
        BodyType::Kinematic => RigidBodyType::KinematicVelocityBased,
        BodyType::Static => RigidBodyType::Fixed,
    }
}

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

/// Gives `each` the outline and entity ID of each [`BoxCollider2D`] and [`CircleCollider2D`] on an
/// entity with a [`Transform`], by table, then row. It sits where the entity draws this frame per
/// `poses` (between ticks, carried by ancestors), shaped as physics shapes it. Outlined whether
/// physics moves the entity or not (no [`RigidBody2D`], or under a parent), so what's being made
/// shows.
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

/// How far `rotation` turns +X about Z, counterclockwise from +Z: an entity's turn in the XY
/// plane, as 2D physics has it, in `f64` like rapier's.
fn angle(rotation: Quat) -> f64 {
    let x = rotation * Vec3::X;
    libm::atan2f(x.y, x.x).into()
}

/// `v` as rapier's vector, whose glam is another, of `f64`s.
fn vector(v: Vec2) -> Vector {
    Vector::new(v.x.into(), v.y.into())
}

fn at(v: DVec2) -> Vector {
    Vector::new(v.x, v.y)
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Vec4};

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
            translation: Vec3::new(x, y, 2.0).as_dvec3(),
            rotation: Quat::from_rotation_z(angle),
            ..Transform::IDENTITY
        };
        world.spawn((at, RigidBody2D::default(), BoxCollider2D::default()))
    }

    pub(super) fn step(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            step_physics(world);
        }
    }

    pub(super) fn transform(world: &World, entity: Entity) -> Transform {
        *world.get::<Transform>(entity).expect("alive, with one")
    }

    /// Physics saved for a client's view, which hides some entities, lacks their bodies; stepped
    /// on in a world without those entities it matches physics saved whole, bit for bit, and the
    /// box the client sees falls as the server's does.
    #[test]
    fn physics_seen_steps_on_as_physics_whole_does() {
        let mut world = World::seeded(3);
        floor(&mut world);
        let near = crate_at(&mut world, 0.0, 2.0, 0.0);
        let hidden = crate_at(&mut world, 100.0, 2.0, 0.0);
        step(&mut world, 20);
        let hides = |entity: Entity| entity == hidden;
        let physics = world.resource::<Physics2D>().expect("stepped");
        let whole = physics.save(&world).expect("saved");
        let seen = physics.save_seen(&world, &hides).expect("saved");
        assert_eq!(physics.save_seen(&world, &|_| false).expect("saved"), whole);
        let mut client = world.clone();
        client.despawn(hidden).expect("alive");
        let [mut from_whole, mut from_seen] = [&whole, &seen].map(|text| {
            let mut copy = client.clone();
            let loaded = Physics2D::load(text, &copy).expect("loaded");
            copy.insert_resource(loaded);
            copy
        });
        let physics = from_seen.resource::<Physics2D>().expect("loaded");
        assert_eq!(
            physics.rapier.bodies.len(),
            2,
            "the floor's and the near box's"
        );
        step(&mut from_whole, 60);
        step(&mut from_seen, 60);
        let text = |world: &World| {
            let physics = world.resource::<Physics2D>().expect("stepped");
            physics.save(world).expect("saved")
        };
        assert_eq!(text(&from_seen), text(&from_whole));
        assert_eq!(from_seen.hash(), from_whole.hash());
        step(&mut world, 60);
        assert_eq!(transform(&from_seen, near), transform(&world, near));
    }

    /// A dropped box falls, lands and rests on a floor, its z and scale unchanged; the static
    /// floor stays as it was.
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

    /// 1,000 km from the origin, a turned box falls, tips and rests as one at the origin does,
    /// every step within 0.1 mm and its quaternion within 1e-4; `f32` would place it to 6 cm.
    #[test]
    fn far_from_the_origin_a_box_falls_and_rests_as_at_it() {
        let way = |offset: DVec3| {
            let mut world = World::default();
            let floor = floor(&mut world);
            let dropped = crate_at(&mut world, 0.5, 3.0, 0.3);
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
        let far = DVec3::new(1.0e6, -1.0e6, 0.0);
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

    /// A sleeping body leaves its entity's components unchanged, so change followers (the hash's
    /// incremental parts, the open scene's check) have nothing to do for it; a moving one changes
    /// them.
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

    /// A NaN or infinity in a component is refused, warned of once while it stays
    /// (`physics.not_finite`): a resting body with such a transform stays put through 30 steps
    /// and the box touching it stays finite; set finite, it's taken. An angular velocity keeps
    /// its last finite value; a circle with such a radius is made once finite; a body with such a
    /// transform from the start isn't made until finite.
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
            .x = f64::from(f32::NAN);
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
        // A velocity and a circle's radius keep their last values.
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
            .translation = DVec3::new(-2.0, 1.0, 2.0);
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

    /// Boxes and balls (every third a ball), turned and overlapping, over a floor.
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

    /// A snapshot of moving, touching bodies, restored or copied, steps as its source did, bit for
    /// bit, workspace aside: the world holds all physics carries between steps.
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

    /// A world of moving, touching bodies, saved whole to text and loaded into another process's
    /// world, steps as the saved one did, bit for bit; a component change since the last step is
    /// taken by the next.
    #[test]
    #[cfg(feature = "dev")]
    fn a_world_saved_whole_steps_as_it_did() {
        let mut world = World::seeded(7);
        world.register_saved::<Physics2D>().expect("saved");
        let bodies = pile(&mut world);
        // Until a step whose contacts started or stopped, which the loaded world must have too.
        step(&mut world, 40);
        for _ in 0..40 {
            if world.contact_events().next().is_some() {
                break;
            }
            step(&mut world, 1);
        }
        // An edit since the last step.
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
        // Contacts and events too, by entity ID (the loaded world's own).
        let ids = |world: &World| {
            let id = |entity| world.id(entity).map(EntityId::get);
            let events = world.contact_events().map(|event| {
                let contact = event.contact;
                (contact.entities.map(id), contact.sensor, event.started)
            });
            events.collect::<Vec<_>>()
        };
        assert!(!ids(&world).is_empty());
        assert_eq!(ids(&loaded), ids(&world), "loaded");
        for _ in 0..60 {
            step(&mut world, 1);
            step(&mut loaded, 1);
            assert_eq!(ids(&loaded), ids(&world), "stepped on");
        }
        assert_eq!(loaded.hash(), world.hash(), "stepped on");
    }

    /// Text from before bodies held their entities (user data 0) loads, each given its link's
    /// entity, and steps like its source; a body naming another entity is refused.
    #[test]
    fn saved_bodies_without_their_entities_are_given_them() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 40);
        let physics = world.resource::<Physics2D>().expect("stepped");
        let text = physics.save(&world).expect("saved");
        let mut saved: SavedPhysics<Rapier> = ron::from_str(&text).expect("read");
        for (_, body) in saved.rapier.bodies.iter_mut() {
            body.user_data = 0;
        }
        let old = ron::to_string(&saved).expect("written");
        let loaded = Physics2D::load(&old, &world).expect("loaded");
        let mut copy = world.clone();
        copy.insert_resource(loaded);
        for _ in 0..60 {
            step(&mut world, 1);
            step(&mut copy, 1);
            assert!(copy.contact_events().eq(world.contact_events()));
        }
        assert!(copy.contacts().next().is_some());
        assert!(copy.contacts().eq(world.contacts()));
        assert_eq!(copy.hash(), world.hash());
        let (_, body) = saved.rapier.bodies.iter_mut().next().expect("a body");
        body.user_data = 1;
        let other = ron::to_string(&saved).expect("written");
        let refused = Physics2D::load(&other, &world)
            .err()
            .map(|error| error.code());
        assert_eq!(refused, Some("physics.load"));
    }

    /// Text naming what its rapier state doesn't hold as the body's is refused (`physics.load`),
    /// not stepped into a panic: a missing rigid body, another body's, another body's collider,
    /// an entity's second.
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

    /// The world's hash after four seconds of a pile, checked by CI on every platform: a difference
    /// on some platforms only is a determinism bug, not a value to change here.
    #[test]
    fn a_pile_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        pile(&mut world);
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0xee9d_ca22_6f92_2904, "{content:#018x}");
    }

    /// Changes between steps to a body's position, turn, velocity, type and collider reach it.
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

    /// A small ball fast enough to cross a thin kinematic wall in one step passes through it; one
    /// with continuous collision stops at it.
    #[test]
    fn continuous_collision_stops_what_a_step_would_carry_through_a_thin_wall() {
        let mut world = World::default();
        let wall = Transform {
            translation: DVec3::new(5.0, 0.0, 0.0),
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
                translation: DVec3::new(0.0, f64::from(y), 0.0),
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

    /// A ball's size is its radius times the larger scale: half a unit, scaled 2 across and 1 up,
    /// rests a unit over the floor's top.
    #[test]
    fn a_ball_is_as_large_as_its_larger_scale() {
        let mut world = World::default();
        floor(&mut world);
        let at = Transform {
            translation: DVec3::new(0.0, 3.0, 0.0),
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

    /// A body collides as its box and circle both, and as each changes: an upright box with a
    /// circle under it rests on the circle, then on its box once the circle's removed.
    #[test]
    fn a_body_collides_as_its_box_and_its_circle() {
        let mut world = World::default();
        floor(&mut world);
        let at = Transform {
            translation: DVec3::new(0.0, 4.0, 0.0),
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
        // Floor top, circle radius, offset.
        let on_foot = transform(&world, body).translation.y;
        assert!((on_foot - 2.0).abs() < 0.02, "on its foot: {on_foot}");
        world.remove::<CircleCollider2D>(body).expect("alive");
        step(&mut world, 240);
        let on_box = transform(&world, body).translation.y;
        assert!((on_box - 1.0).abs() < 0.02, "on its box: {on_box}");
    }

    /// Outlines go where their entity draws, with its ID: a box sized and offset by the scale; a
    /// circle around its scaled offset, radius times the larger scale; both for an entity with
    /// both, box first; one under a parent, carried by it; none without a transform.
    #[test]
    fn colliders_outline_where_their_entities_draw() {
        let mut world = World::default();
        let stretched = Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
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
            translation: DVec3::new(-3.0, 0.0, 0.0),
            ..Transform::IDENTITY
        });
        let child = world.spawn((Transform::IDENTITY, round));
        world.set_parent(child, Some(parent)).expect("no cycle");
        world.spawn(boxed);
        let past = Past::default();
        let placed = Placed::of(&world, &past, 1.0);
        let poses = Poses::new(&world, &past, &placed, 1.0, DVec3::ZERO).expect("transforms");
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
        // The box's top-right corner: offset plus half its size, in the entity's space.
        let corner = boxy.transform_point3(Vec3::new(0.5, 0.5, 0.0));
        let entitys = stretched.matrix();
        assert!(near(
            corner,
            (entitys.transform_point3(DVec3::new(1.0, 0.5, 0.0))).as_vec3()
        ));
        // The circle's middle, and its edge 0.75 times 2 from it.
        let middle = ring.transform_point3(Vec3::ZERO);
        assert!(near(
            middle,
            (entitys.transform_point3(DVec3::new(0.5, 1.0, 0.0))).as_vec3()
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

    /// A body goes with its entity, or when it loses its `RigidBody2D`: what rested on it falls,
    /// and its entity stays as it was. A cleared world keeps its physics, without bodies.
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

    /// A kinematic body moves by its velocity alone and pushes a dynamic box in its way; a dynamic
    /// box with fixed rotation doesn't tip off the floor's edge, one without does.
    #[test]
    fn kinematic_bodies_push_and_fixed_rotations_stay() {
        let mut world = World::default();
        floor(&mut world);
        let pusher = world.spawn((
            Transform {
                translation: DVec3::new(-3.0, 1.0, 0.0),
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
        // A unit ahead of the pusher: half each, touching.
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

    /// A static box 4 units wide around (0, 4), a sensor.
    fn zone(world: &mut World) -> Entity {
        let at = Transform {
            translation: DVec3::new(0.0, 4.0, 0.0),
            scale: Vec3::new(4.0, 1.0, 1.0),
            ..Transform::IDENTITY
        };
        let body = RigidBody2D {
            body_type: BodyType::Static,
            ..RigidBody2D::default()
        };
        let sensor = BoxCollider2D {
            sensor: true,
            ..BoxCollider2D::default()
        };
        world.spawn((at, body, sensor))
    }

    fn events(world: &mut World, ticks: u32) -> Vec<Vec<ContactEvent>> {
        (0..ticks)
            .map(|_| {
                step(world, 1);
                world.contact_events().collect()
            })
            .collect()
    }

    /// A box falls through a sensor, unpushed, and lands on the floor: the overlap starts and
    /// stops, the floor contact starts and stays, each once; at last only the floor is in contact.
    #[test]
    fn a_sensor_finds_what_overlaps_it_and_pushes_nothing() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let zone = zone(&mut world);
        let dropped = crate_at(&mut world, 0.0, 7.0, 0.0);
        let mut free = World::default();
        let falling = crate_at(&mut free, 0.0, 7.0, 0.0);
        let mut found = Vec::new();
        for tick in 0..240 {
            step(&mut world, 1);
            step(&mut free, 1);
            found.extend(world.contact_events());
            // Falls as without the sensor, to just above the floor.
            let y = transform(&world, dropped).translation.y;
            if y > 1.6 {
                assert_eq!(y, transform(&free, falling).translation.y, "tick {tick}");
            }
        }
        let event = |other, sensor, started| {
            let entities = if world.id(dropped) < world.id(other) {
                [dropped, other]
            } else {
                [other, dropped]
            };
            ContactEvent {
                contact: Contact { entities, sensor },
                started,
            }
        };
        let want = [
            event(zone, true, true),
            event(zone, true, false),
            event(floor, false, true),
        ];
        assert_eq!(found, want);
        let rested = transform(&world, dropped).translation.y;
        assert!((rested - 1.0).abs() < 0.02, "on the floor: {rested}");
        let contacts: Vec<_> = world.contacts().collect();
        assert_eq!(contacts, [want[2].contact]);
        assert_eq!(contacts[0].other(dropped), Some(floor));
    }

    /// Colliders meet only where each is in a group the other's filter has: a box in group 2 falls
    /// through a floor whose filter omits group 2, and through one outside the box's filter, while
    /// a box in every group lands on both; no contact with what it passed. A sensor whose filter
    /// omits a box's group doesn't find it.
    #[test]
    fn groups_choose_which_colliders_meet() {
        let mut world = World::default();
        let floor = floor(&mut world);
        let zone = zone(&mut world);
        world.get_mut::<BoxCollider2D>(zone).expect("alive").filter = !2;
        world.get_mut::<BoxCollider2D>(floor).expect("alive").filter = !2;
        let through = crate_at(&mut world, -3.0, 7.0, 0.0);
        world
            .get_mut::<BoxCollider2D>(through)
            .expect("alive")
            .membership = 2;
        let landing = crate_at(&mut world, 3.0, 2.0, 0.0);
        step(&mut world, 120);
        assert!(transform(&world, through).translation.y < -2.0);
        assert!((transform(&world, landing).translation.y - 1.0).abs() < 0.02);
        let touched: Vec<_> = world.contacts().filter_map(|it| it.other(floor)).collect();
        assert_eq!(touched, [landing]);
        // A floor in group 4 alone, outside the box's filter.
        let floor = world.get_mut::<BoxCollider2D>(floor).expect("alive");
        floor.filter = u32::MAX;
        floor.membership = 4;
        world
            .get_mut::<BoxCollider2D>(landing)
            .expect("alive")
            .filter = !4;
        step(&mut world, 60);
        assert!(transform(&world, landing).translation.y < -2.0);
        assert_eq!(world.contacts().count(), 0);
    }

    /// After a restore (as a rollback does) or copy, each step's contact events match the
    /// source's, in order; a pile with a sensor has both kinds.
    #[test]
    fn contact_events_are_alike_after_a_restore() {
        let mut world = World::seeded(7);
        pile(&mut world);
        zone(&mut world);
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
    }
}
