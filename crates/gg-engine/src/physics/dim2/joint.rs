//! Joints: components that hold two bodies together in the XY plane, as rapier's impulse joints do,
//! each a [`Joint2D`] on an entity of its own or on either body's, naming both.

use gg_core::math::{Vec2, Vec3};
use rapier2d::prelude::{
    ColliderSet, GenericJoint, GenericJointBuilder, ImpulseJointHandle, JointAxesMask, JointAxis,
    MotorModel, RigidBodyHandle, RigidBodySet,
};
use serde::{Deserialize, Serialize};

use super::{Physics2D, Rapier, vector};
use crate::draw::Poses;
use crate::physics::{Link, joined, lines_between, warn_unjoined};
use crate::{Component, Entity, Reflect, World, Writable};

/// What a [`Joint2D`] lets the bodies it joins do, from where it holds them, its anchors in each
/// body's space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum JointKind2D {
    /// Nothing: they move as one, turned alike.
    #[default]
    Fixed,
    /// Turn about its anchors alone, held together there, as a pin: between its limits while it's
    /// limited, and driven by its motor.
    Revolute,
    /// Slide along its axis alone, turned alike, as a slider: between its limits while it's
    /// limited, and driven by its motor.
    Prismatic,
    /// Anything but go further apart, at their anchors, than its length, as a rope lets them.
    Rope,
    /// Anything, their anchors pulled toward its length apart, as a spring pulls them, by its
    /// stiffness and damping.
    Spring,
}

/// A joint that holds two 2D bodies together, at each tick a layer runs
/// [`step_physics`](crate::step_physics) in, as rapier's impulse joints do: `body_a` and `body_b`,
/// each an entity with a [`RigidBody2D`](super::RigidBody2D) that physics moves, held at its
/// anchor, in its space, scaled with it as its colliders are, and let move from there as its kind
/// says. A joint is on an entity of its own or on either body's, and an entity has one, so that a
/// body with several, as a chain's link has two, has them on entities of their own. Its frames are
/// its anchors and its axis in each body's space, the axis turned with the body but not scaled, so
/// that a fixed or a prismatic joint holds its bodies turned alike, whatever their scales, a
/// mirrored one's too, and a revolute's angle is 0 where they're turned alike. A prismatic joint's
/// motor pushes its second body at its anchor, so that it drives one anchored at its middle
/// hardest, as rapier's solver gives way to the turn a push from elsewhere makes. The bodies it
/// joins don't collide with each other unless `bodies_collide` says so.
///
/// One naming an entity that isn't a body physics moves, as one gone, one without a
/// `RigidBody2D` or one under a parent, joins nothing, warned of once for each reason
/// (`physics.joint_no_body`), as does one naming one body twice (`physics.joint_one_body`). A change
/// to it is taken at the next step, as is a body made again; and its joint goes as it does, or its
/// entity, or either body. An edit's delete of either body clears the field naming it, as
/// [`Component::cleared`] says, so that it names nothing gone, which a scene wouldn't save.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Joint2D {
    /// The first body it joins.
    pub body_a: Option<Entity>,
    /// The second body it joins.
    pub body_b: Option<Entity>,
    /// What it lets its bodies do.
    pub kind: JointKind2D,
    /// Where it holds the first body, in its space.
    pub anchor_a: Vec2,
    /// Where it holds the second body, in its space.
    pub anchor_b: Vec2,
    /// The axis a prismatic joint slides along, in each body's space, turned with it but not
    /// scaled, made a unit: X for one of no length.
    #[reflect(default = Vec2::X)]
    pub axis: Vec2,
    /// Whether a revolute or a prismatic joint keeps from `lower` to `upper`.
    pub limited: bool,
    /// The least a limited revolute joint turns, in radians, counterclockwise, or a prismatic one
    /// slides along its axis, in units.
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
impl Component for Joint2D {
    fn cleared() -> &'static [&'static str] {
        &["body_a", "body_b"]
    }
}

impl Writable for Joint2D {}

impl Default for Joint2D {
    fn default() -> Self {
        Self {
            body_a: None,
            body_b: None,
            kind: JointKind2D::Fixed,
            anchor_a: Vec2::ZERO,
            anchor_b: Vec2::ZERO,
            axis: Vec2::X,
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

impl Joint2D {
    /// rapier's joint, as this one says, between bodies whose entities are of `scales`, across and
    /// up, which scale its anchors.
    fn generic(&self, scales: [Vec2; 2]) -> GenericJoint {
        let locked = match self.kind {
            JointKind2D::Fixed => JointAxesMask::LOCKED_FIXED_AXES,
            JointKind2D::Revolute => JointAxesMask::LOCKED_REVOLUTE_AXES,
            JointKind2D::Prismatic => JointAxesMask::LOCKED_PRISMATIC_AXES,
            JointKind2D::Rope | JointKind2D::Spring => JointAxesMask::empty(),
        };
        let mut built = GenericJointBuilder::new(locked)
            .local_anchor1(vector(self.anchor_a * scales[0]))
            .local_anchor2(vector(self.anchor_b * scales[1]))
            .contacts_enabled(self.bodies_collide);
        // Compared, so that one that isn't a number drives with none either.
        let max_force = if self.max_force > 0.0 {
            self.max_force
        } else {
            0.0
        };
        let free = match self.kind {
            JointKind2D::Revolute => Some(JointAxis::AngX),
            JointKind2D::Prismatic => {
                // Unscaled, as each frame's turn: scaled, a mirrored body's or one stretched across
                // an axis off its own would frame it turned otherwise than its fellow, which the
                // joint would turn it to.
                let axis = vector(self.axis.try_normalize().unwrap_or(Vec2::X));
                built = built.local_axis1(axis).local_axis2(axis);
                Some(JointAxis::LinX)
            }
            _ => None,
        };
        if let Some(free) = free {
            if self.limited {
                built = built.limits(free, [self.lower, self.upper]);
            }
            // rapier's motor with neither drives as hard as it can toward nothing, which none means.
            if self.stiffness > 0.0 || self.damping > 0.0 {
                built = built
                    .set_motor(
                        free,
                        self.target_position,
                        self.target_velocity,
                        self.stiffness,
                        self.damping,
                    )
                    .motor_max_force(free, max_force);
            }
        }
        // A rope and a spring are rapier's: the distance between their anchors, limited to the
        // length, or driven toward it by a motor of forces, which, of neither stiffness nor damping,
        // would hold them rigidly apart.
        match self.kind {
            JointKind2D::Rope => {
                built = built
                    .coupled_axes(JointAxesMask::LIN_AXES)
                    .limits(JointAxis::LinX, [0.0, self.length]);
            }
            JointKind2D::Spring => {
                built = built.coupled_axes(JointAxesMask::LIN_AXES);
                if self.stiffness > 0.0 || self.damping > 0.0 {
                    built = built
                        .motor_position(JointAxis::LinX, self.length, self.stiffness, self.damping)
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
    /// The bodies rapier's joint joins, and their entities' scales across and up, which its
    /// anchors are scaled by.
    pub(super) bodies: [RigidBodyHandle; 2],
    pub(super) scales: [Vec2; 2],
    pub(super) joint: Joint2D,
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
    pub(super) scales: [[f32; 2]; 2],
    pub(super) joint: String,
    pub(super) warned: Option<String>,
}

impl Physics2D {
    /// Takes out each joint whose entity has gone or lost its [`Joint2D`]; then, for each entity
    /// with one, in the order queries find them, makes its joint, takes in what changed of it or of
    /// its bodies, or takes it out where it joins nothing, warning of that once for each reason.
    /// After the bodies are found, so that it joins those made this step.
    pub(super) fn join(&mut self, world: &World) {
        let joints = &mut self.rapier.impulse_joints;
        self.joints.retain(|jointed| {
            let kept = world.get::<Joint2D>(jointed.entity).is_some();
            if let (false, Some(handle)) = (kept, jointed.handle) {
                joints.remove(handle, true);
            }
            kept
        });
        for (entity, &joint) in world.query::<(Entity, &Joint2D)>() {
            let bodies = [joint.body_a, joint.body_b];
            let joined = joined(world, bodies, "RigidBody2D", |body| {
                let linked = self.links.get(body);
                linked.map(|linked| (linked.body, linked.transform.scale.truncate()))
            });
            if self.joints.get(entity).is_none() {
                self.joints.push(Jointed {
                    entity,
                    handle: None,
                    bodies: [RigidBodyHandle::invalid(); 2],
                    scales: [Vec2::ONE; 2],
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
                    warn_unjoined(world, entity, "Joint2D", unjoined, &mut jointed.warned);
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

/// Gives `each` the lines of each [`Joint2D`] in `world`, with its entity's ID, in the order queries
/// find them, as [`lines_between`] has them: from its first body to its anchors and on to its
/// second, where they draw this frame as `poses` has them.
pub(in crate::physics) fn joint_lines(
    world: &World,
    poses: &Poses<'_>,
    each: &mut impl FnMut(Vec3, Vec3, u64),
) {
    for (entity, joint) in world.query::<(Entity, &Joint2D)>() {
        let id = world.id(entity).expect("found alive").get();
        let bodies = [joint.body_a, joint.body_b];
        let anchors = [joint.anchor_a.extend(0.0), joint.anchor_b.extend(0.0)];
        lines_between(world, poses, bodies, anchors, id, each);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{crate_at, step, transform};
    use super::*;
    use crate::{BodyType, BoxCollider2D, RigidBody2D, Transform};

    /// A static unit box at `x`, `y`.
    fn post(world: &mut World, x: f32, y: f32) -> Entity {
        let at = Transform {
            translation: Vec3::new(x, y, 0.0),
            ..Transform::IDENTITY
        };
        let body = RigidBody2D {
            body_type: BodyType::Static,
            ..RigidBody2D::default()
        };
        world.spawn((at, body, BoxCollider2D::default()))
    }

    /// `joint` between `a` and `b`, on an entity of its own.
    fn join(world: &mut World, a: Entity, b: Entity, joint: Joint2D) -> Entity {
        world.spawn(Joint2D {
            body_a: Some(a),
            body_b: Some(b),
            ..joint
        })
    }

    /// Where `anchor`, in `entity`'s space, is in the world's, across and up.
    fn anchored(world: &World, entity: Entity, anchor: Vec2) -> Vec2 {
        let matrix = transform(world, entity).matrix();
        matrix.transform_point3(anchor.extend(0.0)).truncate()
    }

    /// A rig of each kind of 2D joint, from a post: a chain of three boxes on revolute joints,
    /// level as they start, a box that a motor slides between limits, a box on a rope, one on a
    /// spring and one welded. Gives the chain's links, and the slider and its joint.
    fn rig(world: &mut World) -> ([Entity; 3], (Entity, Entity)) {
        let held = post(world, 0.0, 6.0);
        let mut last = held;
        let chain = [0.0f32, 1.0, 2.0].map(|i| {
            let link = crate_at(world, 1.0 + i, 6.0, 0.0);
            let pin = Joint2D {
                kind: JointKind2D::Revolute,
                anchor_a: Vec2::new(0.5, 0.0),
                anchor_b: Vec2::new(-0.5, 0.0),
                ..Joint2D::default()
            };
            join(world, last, link, pin);
            last = link;
            link
        });
        // Anchored at its middle, where its motor pushes it without turning it.
        let slider = crate_at(world, -2.0, 6.0, 0.0);
        let slide = Joint2D {
            kind: JointKind2D::Prismatic,
            anchor_a: Vec2::new(-2.0, 0.0),
            axis: Vec2::Y,
            limited: true,
            lower: -1.0,
            upper: 1.5,
            target_velocity: 2.0,
            damping: 30.0,
            ..Joint2D::default()
        };
        let slid = join(world, held, slider, slide);
        for (kind, x) in [(JointKind2D::Rope, -5.0), (JointKind2D::Spring, -7.0)] {
            let hung = crate_at(world, x, 5.0, 0.3);
            let line = Joint2D {
                kind,
                anchor_a: Vec2::new(x, 0.0),
                length: 2.0,
                stiffness: 60.0,
                damping: 2.0,
                ..Joint2D::default()
            };
            join(world, held, hung, line);
        }
        let welded = crate_at(world, 0.0, 7.0, 0.0);
        let weld = Joint2D {
            anchor_a: Vec2::new(0.0, 0.5),
            anchor_b: Vec2::new(0.0, -0.5),
            ..Joint2D::default()
        };
        join(world, held, welded, weld);
        (chain, (slider, slid))
    }

    /// A chain hung level from a post swings down, each of its pins' anchors held together, its
    /// links' z as it was; and a box that a motor slides up stops at its upper limit.
    #[test]
    fn joints_hold_their_bodies_and_motors_drive_them() {
        let mut world = World::default();
        let (chain, (slider, _)) = rig(&mut world);
        for _ in 0..128 {
            step(&mut world, 1);
            let mut last = Vec2::new(0.5, 6.0);
            for link in chain {
                let off = anchored(&world, link, Vec2::new(-0.5, 0.0)).distance(last);
                assert!(off < 0.02, "a pin's anchors together: {off}");
                last = anchored(&world, link, Vec2::new(0.5, 0.0));
            }
        }
        let hanging = transform(&world, chain[2]).translation;
        assert!(hanging.y < 4.0, "swung down: {hanging}");
        let slid = transform(&world, slider).translation;
        assert!(
            (slid.y - 7.5).abs() < 0.02 && (slid.x + 2.0).abs() < 0.01,
            "{slid}"
        );
        assert_eq!(hanging.z, 2.0, "its z as it was");
    }

    /// A world of each kind of 2D joint, moving, copied with nothing changed since its last step,
    /// and with a joint changed, and saved whole to text as a snapshot file holds it and loaded into
    /// a world of another process's making, steps as the world it came from did, bit for bit.
    #[test]
    fn a_world_with_joints_restored_steps_as_it_did() {
        let mut world = World::seeded(7);
        let (_, (_, slid)) = rig(&mut world);
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
            .get_mut::<Joint2D>(slid)
            .expect("alive")
            .target_velocity = -2.0;
        let mut copy = world.clone();
        step(&mut copy, 60);
        #[cfg(feature = "dev")]
        let mut loaded = {
            world.register_saved::<Physics2D>().expect("saved");
            let mut text = String::new();
            world.save_whole(&mut text).expect("saved");
            assert!(text.contains("Prismatic"), "{text}");
            let mut loaded = World::seeded(1);
            for registered in [
                loaded.register::<Transform>(),
                loaded.register::<RigidBody2D>(),
                loaded.register::<BoxCollider2D>(),
                loaded.register::<Joint2D>(),
                loaded.register_saved::<Physics2D>(),
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

    /// A world rolled back with `clone_from` to a snapshot from before a joint was made, or taken
    /// out, steps as a copy of the snapshot does, bit for bit: rapier's workspace, which keeps the
    /// joints it last solved, goes with the world it stepped.
    #[test]
    fn a_world_rolled_back_past_a_joints_making_or_going_steps_as_a_copy() {
        let pin = Joint2D {
            kind: JointKind2D::Revolute,
            anchor_a: Vec2::new(0.5, 0.0),
            anchor_b: Vec2::new(-0.5, 0.0),
            ..Joint2D::default()
        };
        for made in [true, false] {
            // A chain of four boxes hung level from a post, which swings down.
            let mut world = World::seeded(7);
            let held = post(&mut world, 0.0, 6.0);
            let mut last = held;
            let pins = [1.0f32, 2.0, 3.0, 4.0].map(|x| {
                let link = crate_at(&mut world, x, 6.0, 0.0);
                let joint = join(&mut world, last, link, pin);
                last = link;
                joint
            });
            step(&mut world, 40);
            let snapshot = world.clone();
            step(&mut world, 5);
            if made {
                let hung = crate_at(&mut world, -2.0, 6.0, 0.0);
                let other = Joint2D {
                    anchor_a: Vec2::new(-0.5, 0.0),
                    anchor_b: Vec2::new(1.5, 0.0),
                    ..pin
                };
                join(&mut world, held, hung, other);
            } else {
                world.remove::<Joint2D>(pins[1]).expect("alive");
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

    /// A slider between a box and one mirrored across it, by a scale of -1, holds them turned
    /// alike, its axis framing each unscaled: the mirrored box, started a little turned, turns back
    /// to its fellow's turn, rather than on round to the other side.
    #[test]
    fn a_slider_holds_a_mirrored_body_turned_as_the_other() {
        let mut world = World::default();
        let held = post(&mut world, 0.0, 4.0);
        let mirrored = crate_at(&mut world, 2.0, 4.0, 0.2);
        world.get_mut::<Transform>(mirrored).expect("alive").scale.x = -1.0;
        let slide = Joint2D {
            kind: JointKind2D::Prismatic,
            ..Joint2D::default()
        };
        join(&mut world, held, mirrored, slide);
        step(&mut world, 30);
        let at = transform(&world, mirrored);
        let across = at.rotation * Vec3::X;
        assert!(across.abs_diff_eq(Vec3::X, 1e-3), "turned: {across}");
        assert!((at.translation.y - 4.0).abs() < 0.01, "{at:?}");
    }

    /// The world's hash after physics has stepped the rig of each kind of 2D joint, its motor
    /// driving, over a floor, and a small fast ball with continuous collision fired at a thin wall,
    /// for four seconds, which CI checks on every platform: a difference on some platforms only is
    /// a bug in determinism, not a value to change here.
    #[test]
    fn joints_hash_as_their_reference_says() {
        let mut world = World::seeded(7);
        let fixed = RigidBody2D {
            body_type: BodyType::Static,
            ..RigidBody2D::default()
        };
        let placed = |x: f32, y: f32, scale: Vec2| Transform {
            translation: Vec3::new(x, y, 0.0),
            scale: scale.extend(1.0),
            ..Transform::IDENTITY
        };
        let floor = placed(0.0, 0.0, Vec2::new(24.0, 1.0));
        world.spawn((floor, fixed, BoxCollider2D::default()));
        rig(&mut world);
        let wall = placed(9.0, 1.5, Vec2::new(0.05, 3.0));
        world.spawn((wall, fixed, BoxCollider2D::default()));
        let fired = RigidBody2D {
            continuous_collision: true,
            velocity: Vec2::new(50.0, 0.0),
            ..RigidBody2D::default()
        };
        let ball = crate::CircleCollider2D {
            radius: 0.1,
            ..crate::CircleCollider2D::default()
        };
        world.spawn((placed(-9.0, 1.5, Vec2::ONE), fired, ball));
        step(&mut world, 256);
        let content = world.hash().content;
        assert_eq!(content, 0xbc40_66fa_d42e_202f, "{content:#018x}");
    }
}
