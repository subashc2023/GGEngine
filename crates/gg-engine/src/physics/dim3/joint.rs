//! Joints: components that hold two bodies together, as rapier's impulse joints do, each a
//! [`Joint3D`] on an entity of its own or on either body's, naming both.

use gg_core::math::Vec3;
use rapier3d::prelude::{
    ColliderSet, GenericJoint, GenericJointBuilder, ImpulseJointHandle, JointAxesMask, JointAxis,
    MotorModel, RigidBodyHandle, RigidBodySet,
};
use serde::{Deserialize, Serialize};

use super::{Physics3D, Rapier, vector};
use crate::draw::Poses;
use crate::physics::{Link, joined, lines_between, warn_unjoined};
use crate::{Component, Entity, Reflect, World, Writable};

/// What a [`Joint3D`] lets the bodies it joins do, from where it holds them, its anchors and axis
/// in each body's space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum JointKind3D {
    /// Nothing: they move as one, turned alike.
    #[default]
    Fixed,
    /// Turn about its axis alone, as a hinge: between its limits while it's limited, and driven by
    /// its motor.
    Revolute,
    /// Slide along its axis alone, turned alike, as a slider: between its limits while it's
    /// limited, and driven by its motor.
    Prismatic,
    /// Turn every way about its anchors, held together there, as a ball joint.
    Spherical,
    /// Anything but go further apart, at their anchors, than its length, as a rope lets them.
    Rope,
    /// Anything, their anchors pulled toward its length apart, as a spring pulls them, by its
    /// stiffness and damping.
    Spring,
}

/// A joint that holds two 3D bodies together, at each tick a layer runs
/// [`step_physics`](crate::step_physics) in, as rapier's impulse joints do: `body_a` and `body_b`,
/// each an entity that physics moves, a [`RigidBody3D`](super::RigidBody3D)'s or a character's,
/// held at its anchor, in its space, scaled with it as its colliders are, and let move from there as
/// its kind says. A joint is on an entity of its own or on either body's, and an entity has one, so
/// that a body with several, as a chain's link has two, has them on entities of their own. Its
/// frames are its anchors and its axis in each body's space, the axis turned with the body but not
/// scaled, so that a fixed or a prismatic joint holds its bodies turned alike, whatever their
/// scales, and a revolute's angle is 0 where they're turned alike. A prismatic joint's motor pushes
/// its second body at its anchor, so that it drives one anchored at its middle hardest, as rapier's
/// solver gives way to the turn a push from elsewhere makes. The bodies it joins don't collide with
/// each other unless `bodies_collide` says so, a character's sweep passing through what it's joined
/// to as their contacts do.
///
/// One naming an entity that isn't a body physics moves, as one gone, one without a
/// `RigidBody3D` or one under a parent, joins nothing, warned of once for each reason
/// (`physics.joint_no_body`), as does one naming one body twice (`physics.joint_one_body`). A change
/// to it is taken at the next step, as is a body made again; and its joint goes as it does, or its
/// entity, or either body. An edit's delete of either body clears the field naming it, as
/// [`Component::cleared`] says, so that it names nothing gone, which a scene wouldn't save.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Joint3D {
    /// The first body it joins.
    pub body_a: Option<Entity>,
    /// The second body it joins.
    pub body_b: Option<Entity>,
    /// What it lets its bodies do.
    pub kind: JointKind3D,
    /// Where it holds the first body, in its space.
    pub anchor_a: Vec3,
    /// Where it holds the second body, in its space.
    pub anchor_b: Vec3,
    /// The axis a revolute joint turns about and a prismatic one slides along, in each body's
    /// space, turned with it but not scaled, made a unit: X for one of no length.
    #[reflect(default = Vec3::X)]
    pub axis: Vec3,
    /// Whether a revolute or a prismatic joint keeps from `lower` to `upper`.
    pub limited: bool,
    /// The least a limited revolute joint turns, in radians, counterclockwise as its axis points
    /// at the viewer, or a prismatic one slides along its axis, in units.
    pub lower: f32,
    /// The most a limited revolute joint turns, or a prismatic one slides.
    pub upper: f32,
    /// How fast a revolute's or a prismatic's motor drives it, in radians or units a second, as
    /// hard as `damping` says.
    pub target_velocity: f32,
    /// Where a revolute's or a prismatic's motor drives it to, in radians or units, as hard as
    /// `stiffness` says.
    pub target_position: f32,
    /// How hard a motor pulls toward its target position, or a spring toward its length, for each
    /// radian or unit it's away: a motor or a spring without stiffness or damping, as by default,
    /// drives or pulls nothing. A motor's are accelerations, so that it drives a heavy body as it
    /// does a light one; a spring's are forces, so that a heavy body stretches it further.
    #[reflect(min = 0.0)]
    pub stiffness: f32,
    /// How hard a motor holds to its target velocity, or a spring against its anchors' speed apart,
    /// for each radian or unit a second it's off.
    #[reflect(min = 0.0)]
    pub damping: f32,
    /// The most force, or torque, a motor or a spring drives it with: without limit at infinity, as
    /// by default, and none at 0 or below.
    #[reflect(default = f32::INFINITY)]
    pub max_force: f32,
    /// How far apart a rope lets its anchors go, or a spring rests, in units.
    #[reflect(min = 0.0, default = 1.0)]
    pub length: f32,
    /// Whether the bodies it joins collide with each other.
    pub bodies_collide: bool,
}

/// `body_a` and `body_b` are cleared by a delete of what they name.
impl Component for Joint3D {
    fn cleared() -> &'static [&'static str] {
        &["body_a", "body_b"]
    }
}

impl Writable for Joint3D {}

impl Default for Joint3D {
    fn default() -> Self {
        Self {
            body_a: None,
            body_b: None,
            kind: JointKind3D::Fixed,
            anchor_a: Vec3::ZERO,
            anchor_b: Vec3::ZERO,
            axis: Vec3::X,
            limited: false,
            lower: 0.0,
            upper: 0.0,
            target_velocity: 0.0,
            target_position: 0.0,
            stiffness: 0.0,
            damping: 0.0,
            max_force: f32::INFINITY,
            length: 1.0,
            bodies_collide: false,
        }
    }
}

impl Joint3D {
    /// rapier's joint, as this one says, between bodies whose entities are of `scales`, which scale
    /// its anchors.
    fn generic(&self, scales: [Vec3; 2]) -> GenericJoint {
        let locked = match self.kind {
            JointKind3D::Fixed => JointAxesMask::LOCKED_FIXED_AXES,
            JointKind3D::Revolute => JointAxesMask::LOCKED_REVOLUTE_AXES,
            JointKind3D::Prismatic => JointAxesMask::LOCKED_PRISMATIC_AXES,
            JointKind3D::Spherical => JointAxesMask::LOCKED_SPHERICAL_AXES,
            JointKind3D::Rope | JointKind3D::Spring => JointAxesMask::empty(),
        };
        let mut built = GenericJointBuilder::new(locked)
            .local_anchor1(vector(self.anchor_a * scales[0]))
            .local_anchor2(vector(self.anchor_b * scales[1]))
            .contacts_enabled(self.bodies_collide);
        // Compared, so that one that isn't a number drives with none either.
        let max_force = if self.max_force > 0.0 {
            f64::from(self.max_force)
        } else {
            0.0
        };
        let free = match self.kind {
            JointKind3D::Revolute => Some(JointAxis::AngX),
            JointKind3D::Prismatic => Some(JointAxis::LinX),
            _ => None,
        };
        if let Some(free) = free {
            // Unscaled, as each frame's turn: scaled, a mirrored body's or one stretched across an
            // axis off its own would frame it turned otherwise than its fellow, which the joint
            // would turn it to.
            let axis = vector(self.axis.try_normalize().unwrap_or(Vec3::X));
            built = built.local_axis1(axis).local_axis2(axis);
            if self.limited {
                built = built.limits(free, [self.lower.into(), self.upper.into()]);
            }
            // rapier's motor with neither drives as hard as it can toward nothing, which none means.
            if self.stiffness > 0.0 || self.damping > 0.0 {
                built = built
                    .set_motor(
                        free,
                        self.target_position.into(),
                        self.target_velocity.into(),
                        self.stiffness.into(),
                        self.damping.into(),
                    )
                    .motor_max_force(free, max_force);
            }
        }
        // A rope and a spring are rapier's: the distance between their anchors, limited to the
        // length, or driven toward it by a motor of forces, which, of neither stiffness nor damping,
        // would hold them rigidly apart.
        match self.kind {
            JointKind3D::Rope => {
                built = built
                    .coupled_axes(JointAxesMask::LIN_AXES)
                    .limits(JointAxis::LinX, [0.0, self.length.into()]);
            }
            JointKind3D::Spring => {
                built = built.coupled_axes(JointAxesMask::LIN_AXES);
                if self.stiffness > 0.0 || self.damping > 0.0 {
                    let (length, stiffness) = (self.length.into(), self.stiffness.into());
                    built = built
                        .motor_position(JointAxis::LinX, length, stiffness, self.damping.into())
                        .motor_model(JointAxis::LinX, MotorModel::ForceBased)
                        .motor_max_force(JointAxis::LinX, max_force);
                }
            }
            _ => {}
        }
        built.build()
    }
}

/// A joint, its entity's, and what it was made of as the last step left it, as a body's link keeps
/// what it was made of: a change since is a game's or an edit's, which the next step takes.
#[derive(Clone)]
pub(super) struct Jointed {
    pub(super) entity: Entity,
    /// rapier's joint, while it joins two bodies.
    pub(super) handle: Option<ImpulseJointHandle>,
    /// The bodies rapier's joint joins, and their entities' scales, which its anchors are scaled
    /// by.
    pub(super) bodies: [RigidBodyHandle; 2],
    pub(super) scales: [Vec3; 2],
    pub(super) joint: Joint3D,
    /// Why it joins nothing, as it was last warned of, while it joins nothing.
    pub(super) warned: Option<String>,
}

impl Link for Jointed {
    fn entity(&self) -> Entity {
        self.entity
    }
}

/// A joint as a snapshot file holds it, as [`SavedLinked`](super::SavedLinked) holds a body.
#[derive(Serialize, Deserialize)]
pub(super) struct SavedJoint {
    pub(super) entity: Option<u64>,
    pub(super) handle: Option<ImpulseJointHandle>,
    pub(super) bodies: [RigidBodyHandle; 2],
    pub(super) scales: [[f32; 3]; 2],
    pub(super) joint: String,
    pub(super) warned: Option<String>,
}

impl Physics3D {
    /// Takes out each joint whose entity has gone or lost its [`Joint3D`]; then, for each entity
    /// with one, in the order queries find them, makes its joint, takes in what changed of it or of
    /// its bodies, or takes it out where it joins nothing, warning of that once for each reason.
    /// After the bodies and the characters are found, so that it joins those made this step.
    pub(super) fn join(&mut self, world: &World) {
        let joints = &mut self.rapier.impulse_joints;
        self.joints.retain(|jointed| {
            let kept = world.get::<Joint3D>(jointed.entity).is_some();
            if let (false, Some(handle)) = (kept, jointed.handle) {
                joints.remove(handle, true);
            }
            kept
        });
        for (entity, &joint) in world.query::<(Entity, &Joint3D)>() {
            let bodies = [joint.body_a, joint.body_b];
            let joined = joined(world, bodies, "RigidBody3D", |body| {
                self.body_and_scale(body)
            });
            if self.joints.get(entity).is_none() {
                self.joints.push(Jointed {
                    entity,
                    handle: None,
                    bodies: [RigidBodyHandle::invalid(); 2],
                    scales: [Vec3::ONE; 2],
                    joint,
                    warned: None,
                });
            }
            let jointed = self.joints.get_mut(entity).expect("linked");
            let Rapier {
                bodies: rigid,
                colliders,
                impulse_joints: joints,
                ..
            } = &mut self.rapier;
            let [(a, scale_a), (b, scale_b)] = match joined {
                Ok(joined) => joined,
                Err(unjoined) => {
                    if let Some(handle) = jointed.handle.take() {
                        joints.remove(handle, true);
                    }
                    warn_unjoined(world, entity, "Joint3D", unjoined, &mut jointed.warned);
                    jointed.joint = joint;
                    continue;
                }
            };
            let (bodies, scales) = ([a, b], [scale_a, scale_b]);
            // rapier's joint, if it still joins these bodies: a body taken out takes its joints
            // with it, and one made again is another.
            let held = jointed.handle.filter(|&handle| {
                let held = joints.get(handle);
                held.is_some_and(|held| [held.body1(), held.body2()] == bodies)
            });
            let collide_anew = match held {
                Some(_) if (jointed.joint, jointed.scales) == (joint, scales) => false,
                Some(handle) => {
                    let held = joints.get_mut(handle, true).expect("held");
                    held.data = joint.generic(scales);
                    joint.bodies_collide != jointed.joint.bodies_collide
                }
                None => {
                    if let Some(handle) = jointed.handle {
                        joints.remove(handle, true);
                    }
                    let made = joints.insert(a, b, joint.generic(scales), true);
                    jointed.handle = Some(made);
                    true
                }
            };
            if collide_anew {
                recollide(rigid, colliders, bodies);
            }
            jointed.bodies = bodies;
            jointed.scales = scales;
            jointed.joint = joint;
            jointed.warned = None;
        }
    }

    /// The body of `entity`, a body's or a character's, if it has one, and its entity's scale, as
    /// this step took it in.
    fn body_and_scale(&self, entity: Entity) -> Option<(RigidBodyHandle, Vec3)> {
        let body = (self.links.get(entity)).map(|linked| (linked.body, linked.transform.scale));
        body.or_else(|| {
            let character = self.characters.get(entity);
            character.map(|character| (character.body, character.transform.scale))
        })
    }
}

/// Has rapier find the contacts between the colliders of `bodies`, in `rigid`, and the rest afresh
/// at the next step, as a joint between them is made or lets them collide anew: it recycles a
/// touching pair's contacts while the pair barely moves, past its check of whether a joint lets the
/// pair's bodies collide, unless a collider changed otherwise than by moving, as one put back where
/// it is on its body has.
fn recollide(rigid: &RigidBodySet, colliders: &mut ColliderSet, bodies: [RigidBodyHandle; 2]) {
    for body in bodies {
        for &collider in rigid[body].colliders() {
            if let Some(collider) = colliders.get_mut(collider)
                && let Some(&at) = collider.position_wrt_parent()
            {
                collider.set_position_wrt_parent(at);
            }
        }
    }
}

/// Gives `each` the lines of each [`Joint3D`] in `world`, with its entity's ID, in the order queries
/// find them, as [`lines_between`] has them: from its first body to its anchors and on to its
/// second, where they draw this frame as `poses` has them.
pub(in crate::physics) fn joint_lines(
    world: &World,
    poses: &Poses<'_>,
    each: &mut impl FnMut(Vec3, Vec3, u64),
) {
    for (entity, joint) in world.query::<(Entity, &Joint3D)>() {
        let id = world.id(entity).expect("found alive").get();
        let bodies = [joint.body_a, joint.body_b];
        let anchors = [joint.anchor_a, joint.anchor_b];
        lines_between(world, poses, bodies, anchors, id, each);
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{DVec3, Quat, libm};

    use super::super::tests::{crate_at, step, transform};
    use super::*;
    use crate::interpolation::Past;
    use crate::placed::Placed;
    use crate::{BodyType, BoxCollider3D, RigidBody3D, Transform};

    /// A static unit box at `at`.
    fn post(world: &mut World, at: Vec3) -> Entity {
        let at = Transform {
            translation: at.as_dvec3(),
            ..Transform::IDENTITY
        };
        let body = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        world.spawn((at, body, BoxCollider3D::default()))
    }

    /// `joint` between `a` and `b`, on an entity of its own.
    fn join(world: &mut World, a: Entity, b: Entity, joint: Joint3D) -> Entity {
        world.spawn(Joint3D {
            body_a: Some(a),
            body_b: Some(b),
            ..joint
        })
    }

    /// Where `anchor`, in `entity`'s space, is in the world's.
    fn anchored(world: &World, entity: Entity, anchor: Vec3) -> Vec3 {
        transform(world, entity)
            .matrix()
            .transform_point3(anchor.as_dvec3())
            .as_vec3()
    }

    /// How far `entity` is turned about Y, counterclockwise as seen from above.
    fn turned_about_y(world: &World, entity: Entity) -> f32 {
        let x = transform(world, entity).rotation * Vec3::X;
        libm::atan2f(-x.z, x.x)
    }

    /// A box welded beside a post stays there, turned as the post is; one hung from it by a ball
    /// joint and pushed aside swings about the post's anchor, which its own stays at; and one on a
    /// rope, dropped from nearer, falls until the rope is taut, no further than its length.
    #[test]
    fn joints_hold_their_bodies_together() {
        let mut world = World::default();
        let held = post(&mut world, Vec3::new(0.0, 4.0, 0.0));
        let welded = crate_at(&mut world, Vec3::new(1.5, 4.0, 0.0), Quat::IDENTITY);
        let weld = Joint3D {
            anchor_a: Vec3::new(1.5, 0.0, 0.0),
            ..Joint3D::default()
        };
        join(&mut world, held, welded, weld);
        let swung = crate_at(&mut world, Vec3::new(0.0, 2.5, 3.0), Quat::IDENTITY);
        world.get_mut::<RigidBody3D>(swung).expect("alive").velocity = Vec3::new(3.0, 0.0, 0.0);
        let ball = Joint3D {
            kind: JointKind3D::Spherical,
            anchor_a: Vec3::new(0.0, 0.0, 3.0),
            anchor_b: Vec3::new(0.0, 1.5, 0.0),
            ..Joint3D::default()
        };
        join(&mut world, held, swung, ball);
        let roped = crate_at(&mut world, Vec3::new(0.0, 3.5, -3.0), Quat::IDENTITY);
        let rope = Joint3D {
            kind: JointKind3D::Rope,
            anchor_a: Vec3::new(0.0, 0.0, -3.0),
            length: 2.0,
            ..Joint3D::default()
        };
        join(&mut world, held, roped, rope);
        let mut furthest = 0.0f32;
        for _ in 0..128 {
            step(&mut world, 1);
            let at = anchored(&world, swung, ball.anchor_b);
            let off = at.distance(Vec3::new(0.0, 4.0, 3.0));
            assert!(off < 0.02, "the ball joint's anchors together: {off}");
            let apart = transform(&world, roped).translation;
            furthest = furthest.max(apart.distance(DVec3::new(0.0, 4.0, -3.0)) as f32);
        }
        let weld_at = transform(&world, welded);
        assert!(
            weld_at
                .translation
                .abs_diff_eq(DVec3::new(1.5, 4.0, 0.0), 0.02),
            "{weld_at:?}"
        );
        assert!(weld_at.rotation.angle_between(Quat::IDENTITY) < 0.01);
        let swinging = transform(&world, swung).translation;
        assert!(swinging.x.abs() > 0.1, "swung aside: {swinging}");
        assert!(
            (furthest - 2.0).abs() < 0.03,
            "taut at its length: {furthest}"
        );
        assert_eq!(
            world
                .resource::<Physics3D>()
                .expect("stepped")
                .rapier
                .impulse_joints
                .len(),
            3
        );
    }

    /// A door hinged to a post turns no further than its limit as its motor drives it at a speed,
    /// then, as its motor is given a target to turn to instead, turns there and holds: the step
    /// after a change to its joint takes it.
    #[test]
    fn a_revolute_joint_keeps_to_its_limits_and_its_motor_drives_it() {
        let mut world = World::default();
        let hinged = post(&mut world, Vec3::new(0.0, 1.0, 0.0));
        let door = crate_at(&mut world, Vec3::new(1.0, 1.0, 0.0), Quat::IDENTITY);
        let hinge = Joint3D {
            kind: JointKind3D::Revolute,
            anchor_a: Vec3::new(0.5, 0.0, 0.0),
            anchor_b: Vec3::new(-0.5, 0.0, 0.0),
            axis: Vec3::Y,
            limited: true,
            lower: -0.5,
            upper: 1.0,
            target_velocity: 2.0,
            damping: 50.0,
            ..Joint3D::default()
        };
        let joint = join(&mut world, hinged, door, hinge);
        step(&mut world, 128);
        let angle = turned_about_y(&world, door);
        assert!((angle - 1.0).abs() < 0.02, "at its upper limit: {angle}");
        let motor = world.get_mut::<Joint3D>(joint).expect("alive");
        motor.target_velocity = 0.0;
        motor.target_position = 0.3;
        motor.stiffness = 40.0;
        motor.damping = 10.0;
        step(&mut world, 192);
        let angle = turned_about_y(&world, door);
        assert!((angle - 0.3).abs() < 0.01, "at its target: {angle}");
        let hinge_at = anchored(&world, door, hinge.anchor_b);
        assert!(
            hinge_at.abs_diff_eq(Vec3::new(0.5, 1.0, 0.0), 0.01),
            "{hinge_at}"
        );
    }

    /// A joint goes as its component does, or as either body's entity goes, its body falling free
    /// then; and one naming an entity gone or that isn't a body, or one body twice, joins nothing,
    /// warned of once for each reason as it goes from one to the next, until it joins two bodies
    /// again.
    #[test]
    fn a_joint_goes_with_its_component_and_its_bodies() {
        let logged = crate::testing::logged();
        let warned = || -> Vec<String> {
            let codes = logged.codes.try_iter();
            codes.filter(|code| code.starts_with("physics.")).collect()
        };
        let mut world = World::default();
        let held = post(&mut world, Vec3::new(0.0, 4.0, 0.0));
        let hung = crate_at(&mut world, Vec3::new(0.0, 2.5, 0.0), Quat::IDENTITY);
        let ball = Joint3D {
            kind: JointKind3D::Spherical,
            anchor_b: Vec3::new(0.0, 1.5, 0.0),
            ..Joint3D::default()
        };
        let joint = join(&mut world, held, hung, ball);
        step(&mut world, 16);
        let joints = |world: &World| {
            let physics = world.resource::<Physics3D>().expect("stepped");
            let jointed = physics
                .joints
                .get(joint)
                .map(|it| (it.handle.is_some(), it.warned.is_some()));
            (physics.rapier.impulse_joints.len(), jointed)
        };
        assert_eq!(joints(&world), (1, Some((true, false))));
        world.remove::<Joint3D>(joint).expect("alive");
        step(&mut world, 16);
        assert_eq!(joints(&world), (0, None), "its component taken away");
        assert!(transform(&world, hung).translation.y < 2.4, "falling free");
        assert!(warned().is_empty());
        let given = Joint3D {
            body_a: Some(held),
            body_b: Some(hung),
            ..ball
        };
        world.insert(joint, given).expect("alive");
        let placed = Transform {
            translation: DVec3::new(0.0, 2.5, 0.0),
            ..Transform::IDENTITY
        };
        world.insert(hung, placed).expect("alive");
        step(&mut world, 16);
        assert_eq!(joints(&world), (1, Some((true, false))), "given back");
        world.despawn(held).expect("alive");
        step(&mut world, 16);
        assert_eq!(joints(&world), (0, Some((false, true))), "its body gone");
        assert_eq!(
            warned(),
            ["physics.joint_no_body"],
            "once, though it stepped 16 times"
        );
        let plain = world.spawn(Transform::IDENTITY);
        world.get_mut::<Joint3D>(joint).expect("alive").body_a = Some(plain);
        step(&mut world, 16);
        assert_eq!(joints(&world), (0, Some((false, true))), "not a body");
        assert_eq!(warned(), ["physics.joint_no_body"], "for another reason");
        world.get_mut::<Joint3D>(joint).expect("alive").body_a = Some(hung);
        step(&mut world, 16);
        assert_eq!(joints(&world), (0, Some((false, true))), "one body twice");
        assert_eq!(warned(), ["physics.joint_one_body"]);
        let again = post(&mut world, Vec3::new(5.0, 4.0, 0.0));
        world.get_mut::<Joint3D>(joint).expect("alive").body_a = Some(again);
        step(&mut world, 1);
        assert_eq!(joints(&world), (1, Some((true, false))), "two bodies again");
        world.get_mut::<Joint3D>(joint).expect("alive").body_a = Some(hung);
        step(&mut world, 1);
        assert_eq!(
            warned(),
            ["physics.joint_one_body"],
            "again, having joined two meanwhile"
        );
    }

    /// An edit's delete of a jointed body clears the field of each joint naming it, as sets of the
    /// same edit, so that the scene saves, where a field naming an entity gone would be refused
    /// (`ecs.no_entity`): a joint on an entity of its own and one on the other body's, but not one
    /// the delete takes with it. Undone, they name it again and join it as before; redone, they're
    /// cleared again.
    #[test]
    fn a_delete_of_a_body_clears_the_joints_naming_it() {
        let mut world = World::default();
        let held = post(&mut world, Vec3::new(0.0, 4.0, 0.0));
        let hung = crate_at(&mut world, Vec3::new(0.0, 2.5, 0.0), Quat::IDENTITY);
        let ball = Joint3D {
            kind: JointKind3D::Spherical,
            anchor_b: Vec3::new(0.0, 1.5, 0.0),
            ..Joint3D::default()
        };
        let apart = join(&mut world, held, hung, ball);
        let other = post(&mut world, Vec3::new(3.0, 4.0, 0.0));
        let on_other = Joint3D {
            body_a: Some(hung),
            body_b: Some(other),
            ..ball
        };
        world.insert(other, on_other).expect("alive");
        // Under the body deleted, which goes with it.
        let under = join(&mut world, held, hung, ball);
        world.set_parent(under, Some(hung)).expect("no cycle");
        step(&mut world, 4);
        let id = world.id(hung).expect("alive");
        let mut log = crate::Log::default();
        let delete = crate::Operation::Delete { entity: id };
        log.apply(&mut world, delete.clone()).expect("deleted");
        // Shown as its author gave it, as the Inspector's undo names it.
        assert_eq!(log.undoable(), Some(&[delete][..]));
        let named = |world: &World, joint: Entity| {
            let joint = world.get::<Joint3D>(joint).expect("kept");
            [joint.body_a, joint.body_b]
        };
        assert_eq!(named(&world, apart), [Some(held), None]);
        assert_eq!(named(&world, other), [None, Some(other)]);
        assert!(!world.contains(under), "taken with it");
        let roots: Vec<Entity> = world.roots().collect();
        world
            .save_scene(&roots)
            .expect("nothing names an entity gone");
        step(&mut world, 4);
        let joints = |world: &World| {
            let physics = world.resource::<Physics3D>().expect("stepped");
            physics.rapier.impulse_joints.len()
        };
        assert_eq!(joints(&world), 0);
        log.undo(&mut world).expect("undone");
        assert_eq!(named(&world, apart), [Some(held), Some(hung)]);
        assert_eq!(named(&world, other), [Some(hung), Some(other)]);
        assert!(world.contains(under));
        step(&mut world, 4);
        assert_eq!(joints(&world), 3, "joined again");
        log.redo(&mut world).expect("redone");
        assert_eq!(named(&world, apart), [Some(held), None]);
        assert_eq!(named(&world, other), [None, Some(other)]);
    }

    /// A spring without stiffness or damping, as by default, pulls nothing: a box hung 3 below a
    /// post on one of length 1 falls as one beside it on no joint does.
    #[test]
    fn a_spring_without_stiffness_or_damping_pulls_nothing() {
        let mut world = World::default();
        let held = post(&mut world, Vec3::new(0.0, 6.0, 0.0));
        let hung = crate_at(&mut world, Vec3::new(0.0, 3.0, 0.0), Quat::IDENTITY);
        let free = crate_at(&mut world, Vec3::new(10.0, 3.0, 0.0), Quat::IDENTITY);
        let spring = Joint3D {
            kind: JointKind3D::Spring,
            ..Joint3D::default()
        };
        join(&mut world, held, hung, spring);
        step(&mut world, 64);
        let [fell, fell_free] = [hung, free].map(|body| transform(&world, body).translation.y);
        assert!(fell < -1.0, "fell: {fell}");
        assert!((fell - fell_free).abs() < 1e-4, "{fell} {fell_free}");
    }

    /// Two boxes resting on each other, welded at runtime by a joint whose bodies don't collide,
    /// lose their contact at once, though rapier recycles a resting pair's contacts, and keep it
    /// lost; and find it again as their joint lets them collide.
    #[test]
    fn a_joint_made_between_touching_bodies_parts_their_contacts() {
        let mut world = World::default();
        super::super::tests::floor(&mut world);
        let lower = crate_at(&mut world, Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        let upper = crate_at(&mut world, Vec3::new(0.0, 2.0, 0.0), Quat::IDENTITY);
        step(&mut world, 120);
        let touching = |world: &World| {
            let physics = world.resource::<Physics3D>().expect("stepped");
            let collider = |body| physics.links.get(body).expect("linked").colliders[0];
            let (lower, upper) = (collider(lower), collider(upper));
            let pair = physics
                .rapier
                .narrow_phase
                .contact_pair(lower.expect("a box"), upper.expect("a box"));
            pair.is_some_and(|pair| pair.has_any_active_contact())
        };
        assert!(touching(&world), "resting on each other");
        let weld = Joint3D {
            anchor_a: Vec3::new(0.0, 0.5, 0.0),
            anchor_b: Vec3::new(0.0, -0.5, 0.0),
            ..Joint3D::default()
        };
        let joint = join(&mut world, lower, upper, weld);
        for tick in 0..60 {
            step(&mut world, 1);
            assert!(!touching(&world), "touching {tick} steps after the weld");
        }
        world
            .get_mut::<Joint3D>(joint)
            .expect("alive")
            .bodies_collide = true;
        step(&mut world, 2);
        assert!(touching(&world), "colliding again");
    }

    /// A character walks through a wall it's joined to by a joint whose bodies don't collide, as
    /// its sweep leaves the wall out, where one not joined to its wall stops at it.
    #[test]
    fn a_character_passes_through_what_its_joined_to_without_colliding() {
        use super::super::CharacterController3D;
        use super::super::character::tests::{character, wall};

        let mut world = World::default();
        super::super::tests::floor(&mut world);
        let walkers = [-2.0, 2.0].map(|z| {
            let walker = character(&mut world, Vec3::new(-3.0, 1.52, z));
            let walled = wall(
                &mut world,
                Vec3::new(0.0, 1.5, z),
                Vec3::new(0.2, 3.0, 2.0),
                Quat::IDENTITY,
            );
            let controller = world.get_mut::<CharacterController3D>(walker);
            controller.expect("a character").velocity = Vec3::new(2.0, 0.0, 0.0);
            (walker, walled)
        });
        let [(through, its_wall), (stopped, _)] = walkers;
        let rope = Joint3D {
            kind: JointKind3D::Rope,
            length: 20.0,
            ..Joint3D::default()
        };
        join(&mut world, through, its_wall, rope);
        step(&mut world, 192);
        let x = |entity| transform(&world, entity).translation.x;
        assert!(x(through) > 1.5, "through its wall: {}", x(through));
        assert!(x(stopped) < -0.5, "stopped at its wall: {}", x(stopped));
    }

    /// A rig of each kind of joint, from a post: a chain of four boxes on revolute joints, a door
    /// that a motor turns, a box that a motor slides between limits, a pendulum on a ball joint, a
    /// box on a rope, one on a spring and one welded. Gives the motorized joints' entities.
    fn rig(world: &mut World) -> [Entity; 2] {
        let held = post(world, Vec3::new(0.0, 6.0, 0.0));
        let mut last = held;
        for i in 0..4u8 {
            let link = crate_at(
                world,
                Vec3::new(1.0 + f32::from(i), 6.0, 0.0),
                Quat::IDENTITY,
            );
            let hinge = Joint3D {
                kind: JointKind3D::Revolute,
                anchor_a: Vec3::new(0.5, 0.0, 0.0),
                anchor_b: Vec3::new(-0.5, 0.0, 0.0),
                axis: Vec3::Z,
                ..Joint3D::default()
            };
            join(world, last, link, hinge);
            last = link;
        }
        let door = crate_at(world, Vec3::new(-1.0, 6.0, 0.0), Quat::IDENTITY);
        let hinge = Joint3D {
            kind: JointKind3D::Revolute,
            anchor_a: Vec3::new(-0.5, 0.0, 0.0),
            anchor_b: Vec3::new(0.5, 0.0, 0.0),
            axis: Vec3::Y,
            target_velocity: 1.5,
            damping: 20.0,
            ..Joint3D::default()
        };
        let turned = join(world, held, door, hinge);
        // Anchored at its middle, where its motor pushes it without turning it.
        let slider = crate_at(world, Vec3::new(0.0, 6.0, 2.0), Quat::IDENTITY);
        let slide = Joint3D {
            kind: JointKind3D::Prismatic,
            anchor_a: Vec3::new(0.0, 0.0, 2.0),
            axis: Vec3::Y,
            limited: true,
            lower: -1.0,
            upper: 1.0,
            target_velocity: -2.0,
            damping: 30.0,
            max_force: 400.0,
            ..Joint3D::default()
        };
        let slid = join(world, held, slider, slide);
        let pendulum = crate_at(world, Vec3::new(0.0, 4.5, -2.0), Quat::IDENTITY);
        world
            .get_mut::<RigidBody3D>(pendulum)
            .expect("alive")
            .velocity = Vec3::new(2.0, 0.0, 1.0);
        let ball = Joint3D {
            kind: JointKind3D::Spherical,
            anchor_a: Vec3::new(0.0, 0.0, -2.0),
            anchor_b: Vec3::new(0.0, 1.5, 0.0),
            ..Joint3D::default()
        };
        join(world, held, pendulum, ball);
        for (kind, x) in [(JointKind3D::Rope, -3.0), (JointKind3D::Spring, -5.0)] {
            let hung = crate_at(world, Vec3::new(x, 5.0, 0.0), Quat::from_rotation_x(0.3));
            let line = Joint3D {
                kind,
                anchor_a: Vec3::new(x, 0.0, 0.0),
                length: 2.0,
                stiffness: 60.0,
                damping: 2.0,
                ..Joint3D::default()
            };
            join(world, held, hung, line);
        }
        let welded = crate_at(world, Vec3::new(0.0, 7.0, 0.0), Quat::IDENTITY);
        let weld = Joint3D {
            anchor_a: Vec3::new(0.0, 0.5, 0.0),
            anchor_b: Vec3::new(0.0, -0.5, 0.0),
            ..Joint3D::default()
        };
        join(world, held, welded, weld);
        [turned, slid]
    }

    /// A world of each kind of joint, moving, copied with nothing changed since its last step, and
    /// with a joint changed, and saved whole to text as a snapshot file holds it and loaded into a
    /// world of another process's making, steps as the world it came from did, bit for bit:
    /// rapier's joints come back through its saved state, and a change to a joint since its last
    /// step is taken by the next, as it would have been.
    #[test]
    fn a_world_with_joints_restored_steps_as_it_did() {
        let mut world = World::seeded(7);
        let [turned, _] = rig(&mut world);
        step(&mut world, 40);
        // A copy taken between steps, with nothing changed since the last.
        let mut plain = world.clone();
        step(&mut plain, 20);
        step(&mut world, 20);
        assert_eq!(
            plain.hash(),
            world.hash(),
            "a copy taken with nothing changed"
        );
        world
            .get_mut::<Joint3D>(turned)
            .expect("alive")
            .target_velocity = -1.0;
        let mut copy = world.clone();
        step(&mut copy, 60);
        #[cfg(feature = "dev")]
        let mut loaded = {
            world.register_saved::<Physics3D>().expect("saved");
            let mut text = String::new();
            world.save_whole(&mut text).expect("saved");
            assert!(text.contains("Revolute"), "{text}");
            let mut loaded = World::seeded(1);
            for registered in [
                loaded.register::<Transform>(),
                loaded.register::<RigidBody3D>(),
                loaded.register::<BoxCollider3D>(),
                loaded.register::<Joint3D>(),
                loaded.register_saved::<Physics3D>(),
            ] {
                registered.expect("registered");
            }
            let lines = (1..).zip(text.lines());
            let changes = loaded.load_whole(lines, true).expect("loaded");
            assert!(changes.is_empty(), "{changes:?}");
            assert_eq!(loaded.hash(), world.hash(), "loaded");
            loaded
        };
        step(&mut world, 60);
        assert_eq!(copy.hash(), world.hash(), "a copy");
        #[cfg(feature = "dev")]
        {
            step(&mut loaded, 60);
            assert_eq!(loaded.hash(), world.hash(), "loaded and stepped on");
        }
    }

    /// A revolute joint about Z from the middle of one unit box's right side to the next's left.
    fn hinge() -> Joint3D {
        Joint3D {
            kind: JointKind3D::Revolute,
            anchor_a: Vec3::new(0.5, 0.0, 0.0),
            anchor_b: Vec3::new(-0.5, 0.0, 0.0),
            axis: Vec3::Z,
            ..Joint3D::default()
        }
    }

    /// A chain of four boxes hung level from a post, which swings down. Gives the post and the
    /// hinges.
    fn chain(world: &mut World) -> (Entity, [Entity; 4]) {
        let held = post(world, Vec3::new(0.0, 6.0, 0.0));
        let mut last = held;
        let hinges = [1.0f32, 2.0, 3.0, 4.0].map(|x| {
            let link = crate_at(world, Vec3::new(x, 6.0, 0.0), Quat::IDENTITY);
            let joint = join(world, last, link, hinge());
            last = link;
            joint
        });
        (held, hinges)
    }

    /// Hangs a box from the left of `held`, a chain's post, by a hinge.
    fn hang_left(world: &mut World, held: Entity) {
        let hung = crate_at(world, Vec3::new(-2.0, 6.0, 0.0), Quat::IDENTITY);
        let other = Joint3D {
            anchor_a: Vec3::new(-0.5, 0.0, 0.0),
            anchor_b: Vec3::new(1.5, 0.0, 0.0),
            ..hinge()
        };
        join(world, held, hung, other);
    }

    /// A world rolled back with `clone_from` to a snapshot from before a joint was made, or taken
    /// out, steps as a copy of the snapshot does, bit for bit: rapier's workspace, which keeps the
    /// joints it last solved, goes with the world it stepped.
    #[test]
    fn a_world_rolled_back_past_a_joints_making_or_going_steps_as_a_copy() {
        for made in [true, false] {
            let mut world = World::seeded(7);
            let (held, hinges) = chain(&mut world);
            step(&mut world, 40);
            let snapshot = world.clone();
            step(&mut world, 5);
            if made {
                hang_left(&mut world, held);
            } else {
                world.remove::<Joint3D>(hinges[1]).expect("alive");
            }
            step(&mut world, 25);
            world.clone_from(&snapshot);
            let mut copy = snapshot.clone();
            for tick in 0..20 {
                step(&mut world, 1);
                step(&mut copy, 1);
                assert_eq!(
                    world.hash(),
                    copy.hash(),
                    "made: {made}, {tick} steps after"
                );
            }
        }
    }

    /// Physics copied into memory kept from before a joint was made in it steps rapier as a copy of
    /// it does, bit for bit, at rapier's own steps, without what a step takes in of the world first:
    /// rapier keeps the joints it last solved in its workspace, which goes with the physics it
    /// stepped, where a world's step would mask it, marking every body changed as it takes them in.
    #[test]
    fn physics_copied_back_past_a_joints_making_steps_rapier_as_a_copy() {
        let mut world = World::seeded(7);
        let (held, _) = chain(&mut world);
        step(&mut world, 40);
        let snapshot = world.resource::<Physics3D>().expect("stepped").clone();
        let mut physics = world.remove_resource::<Physics3D>().expect("stepped");
        for _ in 0..5 {
            physics.step_rapier();
        }
        world.insert_resource(physics);
        hang_left(&mut world, held);
        step(&mut world, 1);
        let mut physics = world.remove_resource::<Physics3D>().expect("stepped");
        for _ in 0..25 {
            physics.step_rapier();
        }
        physics.clone_from(&snapshot);
        let mut copy = snapshot.clone();
        let bits = |physics: &Physics3D| -> Vec<u64> {
            let bodies = physics.rapier.bodies.iter();
            let poses = bodies.flat_map(|(_, body)| {
                let (at, turned) = (body.translation(), body.rotation());
                [at.x, at.y, at.z, turned.x, turned.y, turned.z, turned.w]
            });
            poses.map(f64::to_bits).collect()
        };
        for tick in 0..20 {
            physics.step_rapier();
            copy.step_rapier();
            assert_eq!(bits(&physics), bits(&copy), "{tick} steps after");
        }
    }

    /// A joint's lines go from where its first body draws to its anchor there, on to its second's,
    /// and to its second body, with the joint's entity's ID; and none of a joint that names a body
    /// gone.
    #[test]
    fn a_joints_lines_go_from_its_bodies_to_its_anchors() {
        let mut world = World::default();
        let a = post(&mut world, Vec3::new(0.0, 4.0, 0.0));
        let b = crate_at(
            &mut world,
            Vec3::new(2.0, 1.0, 0.0),
            Quat::from_rotation_z(0.5),
        );
        let joint = Joint3D {
            anchor_a: Vec3::new(0.5, 0.0, 0.0),
            anchor_b: Vec3::new(0.0, 0.5, 0.0),
            ..Joint3D::default()
        };
        let jointed = join(&mut world, a, b, joint);
        let gone = crate_at(&mut world, Vec3::ZERO, Quat::IDENTITY);
        join(&mut world, a, gone, joint);
        world.despawn(gone).expect("alive");
        let past = Past::default();
        let placed = Placed::of(&world, &past, 1.0);
        let poses = Poses::new(&world, &past, &placed, 1.0, DVec3::ZERO).expect("transforms");
        let mut found = Vec::new();
        joint_lines(&world, &poses, &mut |from, to, id| {
            found.push((from, to, id))
        });
        let id = world.id(jointed).expect("alive").get();
        let points = [
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(0.5, 4.0, 0.0),
            anchored(&world, b, joint.anchor_b),
            Vec3::new(2.0, 1.0, 0.0),
        ];
        let want: Vec<_> = points
            .windows(2)
            .map(|pair| (pair[0], pair[1], id))
            .collect();
        assert_eq!(found, want);
    }

    /// The world's hash after physics has stepped the rig of each kind of joint, its motors driving,
    /// over a floor, and a small fast ball with continuous collision fired at a thin wall, for four
    /// seconds, which CI checks on every platform: a difference on some platforms only is a bug in
    /// determinism, not a value to change here.
    #[test]
    fn joints_hash_as_their_reference_says() {
        let mut world = World::seeded(7);
        super::super::tests::floor(&mut world);
        rig(&mut world);
        let wall = Transform {
            translation: DVec3::new(0.0, 1.5, 4.0),
            scale: Vec3::new(4.0, 2.0, 0.05),
            ..Transform::IDENTITY
        };
        let fixed = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        world.spawn((wall, fixed, BoxCollider3D::default()));
        let at = Transform {
            translation: DVec3::new(0.0, 1.5, -4.0),
            ..Transform::IDENTITY
        };
        let fired = RigidBody3D {
            continuous_collision: true,
            velocity: Vec3::new(0.0, 0.0, 50.0),
            ..RigidBody3D::default()
        };
        let ball = crate::SphereCollider3D {
            radius: 0.1,
            ..crate::SphereCollider3D::default()
        };
        world.spawn((at, fired, ball));
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0xa721_426b_9615_ba06, "{content:#018x}");
    }
}
