//! Characters: entities that rapier's kinematic character controller moves at each step, as fast as
//! their games ask, sliding along what they move into, climbing slopes and steps, and kept on the
//! ground going down.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use gg_core::math::Vec3;
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude::{
    Collider, ColliderHandle, Pose, QueryFilter, RigidBodyBuilder, RigidBodyHandle, Vector,
};
use tracing::warn;

use super::{
    CapsuleCollider3D, Physics3D, Rapier, RigidBody3D, remove_body, replace, rotation, user_data,
    vector,
};
use crate::physics::Link;
use crate::{Component, Entity, Tick, Transform, World};

/// A character that 3D physics moves, at each tick a layer runs
/// [`step_physics`](crate::step_physics) in, as its game asks, through rapier's kinematic character
/// controller: as fast as `velocity` says, sliding along what it moves into, climbing slopes up to
/// `max_slope` and steps up to `step_height`, and kept on the ground going down as far as `snap`.
/// It moves as its entity's [`CapsuleCollider3D`], its body kinematic, which pushes what's dynamic
/// in its way as it goes and stops for the rest, characters among them, but passes through what a
/// [`Joint3D`](super::Joint3D) joins it to without colliding. Its entity needs no
/// [`RigidBody3D`]: one with one is moved as a body, and its controller does nothing, warned of
/// once (`physics.not_character`), as is one without a `CapsuleCollider3D`; one under a parent
/// isn't moved (`physics.child_body`), as a body under one isn't.
///
/// Its state is the world's, as the rest of physics' is, so that replays and peers move it alike:
/// `velocity`, which gravity pulls down while it's off the ground, and `grounded`, whether it
/// stood on something after it last moved. Its entity's rotation is the game's to set, as which
/// way it faces; physics moves its translation alone.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct CharacterController3D {
    /// How fast it means to go, in units a second, which its game sets, as from its player's
    /// input: across, as it walks, and up, as it jumps. Each step pulls it down by the world's
    /// gravity, as much as `gravity` says, and stops its falling as it lands, and its rising as
    /// something above it stops it; what's across stays as the game set it. Walking, it goes a
    /// hundredth or so slower than this, as rapier's controller slides it along the ground.
    pub velocity: Vec3,
    /// How much of the world's gravity, [`Physics3D::gravity`], pulls it down: 1 all of it, 0 none,
    /// as for something that flies.
    #[reflect(default = 1.0)]
    pub gravity: f32,
    /// How steep a slope it climbs, from level, up to a quarter turn: steeper ones are walls, which
    /// it slides along, and down if it stands on one. One past a quarter turn is taken as one.
    #[reflect(degrees, min = 0.0, default = FRAC_PI_4)]
    pub max_slope: f32,
    /// How high a step it climbs onto as it walks into one, in units: none at 0.
    #[reflect(min = 0.0, default = 0.3)]
    pub step_height: f32,
    /// How far below it the ground may fall away as it walks, in units, as down a slope or a step,
    /// and it still keep to it rather than leave it: none at 0.
    #[reflect(min = 0.0, default = 0.2)]
    pub snap: f32,
    /// Whether it stood on something after it last moved, which a game reads, as to let it jump.
    pub grounded: bool,
}

impl Default for CharacterController3D {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            gravity: 1.0,
            max_slope: FRAC_PI_4,
            step_height: 0.3,
            snap: 0.2,
            grounded: false,
        }
    }
}

impl CharacterController3D {
    /// rapier's controller, as this one says.
    fn controller(&self) -> KinematicCharacterController {
        // Compared rather than `min`ed, so that a slope that isn't a number climbs nothing.
        let slope = if self.max_slope < FRAC_PI_2 {
            self.max_slope
        } else {
            FRAC_PI_2
        };
        let slope = if slope > 0.0 { slope } else { 0.0 };
        let autostep = (self.step_height > 0.0).then_some(CharacterAutostep {
            max_height: CharacterLength::Absolute(self.step_height),
            // The room it needs past a step's edge to climb onto it: rapier's own, half its width,
            // leaves no room for stairs narrower than that, as a person's are.
            min_width: CharacterLength::Absolute(0.1),
            // It pushes what's dynamic rather than climbing onto it.
            include_dynamic_bodies: false,
        });
        KinematicCharacterController {
            up: Vector::Y,
            autostep,
            max_slope_climb_angle: slope,
            min_slope_slide_angle: slope,
            snap_to_ground: (self.snap > 0.0).then_some(CharacterLength::Absolute(self.snap)),
            ..KinematicCharacterController::default()
        }
    }
}

/// A character, its entity's, and what the last step left in the entity's components, or what they
/// were as it took them in, as a body's link keeps them.
#[derive(Clone)]
pub(super) struct Character {
    pub(super) entity: Entity,
    pub(super) body: RigidBodyHandle,
    /// The collider of its entity's `CapsuleCollider3D`.
    pub(super) collider: ColliderHandle,
    pub(super) controller: CharacterController3D,
    pub(super) transform: Transform,
    pub(super) capsule: CapsuleCollider3D,
}

impl Link for Character {
    fn entity(&self) -> Entity {
        self.entity
    }
}

/// Why `entity`, which has a [`CharacterController3D`], isn't a character, if it isn't.
fn unmoved(world: &World, entity: Entity) -> Option<&'static str> {
    if world.get::<RigidBody3D>(entity).is_some() {
        Some("it has a RigidBody3D, which moves it as a body")
    } else if world.get::<CapsuleCollider3D>(entity).is_none() {
        Some("it has no CapsuleCollider3D, which a character moves as")
    } else {
        None
    }
}

impl Physics3D {
    /// Takes out each character whose entity has gone, lost its controller, its capsule or its
    /// `Transform`, been given a `RigidBody3D`, or come under a parent; the rest keep their order.
    pub(super) fn unlink_gone_characters(&mut self, world: &World) {
        let rapier = &mut self.rapier;
        self.characters.retain(|character| {
            let entity = character.entity;
            let has = world.get::<CharacterController3D>(entity).is_some()
                && world.get::<Transform>(entity).is_some()
                && unmoved(world, entity).is_none();
            if has && matches!(world.parent(entity), Ok(None)) {
                return true;
            }
            remove_body(rapier, character.body);
            false
        });
    }

    /// Makes a character for each entity that has come to be one, in the order queries find them,
    /// and takes in what changed of each that was, as bodies are found; warning once of each
    /// entity with a controller that isn't one, as [`CharacterController3D`] says.
    pub(super) fn find_characters(&mut self, world: &World) {
        let rapier = &mut self.rapier;
        let warned = &mut self.unmoved;
        warned.retain(|&entity| {
            let controlled = world.get::<CharacterController3D>(entity).is_some();
            controlled && unmoved(world, entity).is_some()
        });
        let name = "CharacterController3D";
        self.characters
            .find::<CharacterController3D>(world, name, |characters, entity| {
                if let Some(why) = unmoved(world, entity) {
                    if !warned.contains(&entity) {
                        let id = world.id(entity).expect("found alive");
                        warn!(
                            code = "physics.not_character",
                            entity = %id,
                            "Entity {id}'s CharacterController3D doesn't move it: {why}"
                        );
                        warned.push(entity);
                    }
                    return;
                }
                match characters.get_mut(entity) {
                    Some(character) => character.take_changes(rapier, world),
                    None => characters.push(Character::make(rapier, world, entity)),
                }
            });
    }

    /// Moves each character, in their order, as far as its controller lets it go toward where its
    /// velocity takes it this step, against the colliders as the last step left them but those of
    /// the bodies joined to it without colliding, as a [`Joint3D`](super::Joint3D)'s
    /// `bodies_collide` says: its body goes there as rapier steps, pushing what's dynamic that it
    /// hit on the way. Writes what that left of its controller into its entity's, as it changed:
    /// its velocity, pulled down by gravity and stopped by the ground and by what's above it, and
    /// whether it's grounded.
    pub(super) fn move_characters(&mut self, world: &mut World) {
        let dt = Tick::DT.as_secs_f32();
        let Rapier {
            bodies,
            colliders,
            broad_phase,
            narrow_phase,
            impulse_joints,
            ..
        } = &mut self.rapier;
        for character in &mut self.characters.linked {
            let mut controller = character.controller;
            // Gravity pulls it as it walks too, as rapier's controller keeps its gap from the
            // ground so, though it slides it along a hundredth or so slower than it goes; without,
            // a character on a large box sinks into it by some centimeters as the snaps miss it.
            // Standing still, it moves by nothing, which rapier's controller snaps to the ground,
            // and which leaves it unchanged, where gravity's push would leave it a little higher
            // or lower each step.
            let still = controller.velocity == Vec3::ZERO;
            let mut velocity = controller.velocity;
            if !(still && controller.grounded) {
                velocity += self.gravity * controller.gravity * dt;
            }
            let mut way = velocity * dt;
            // A way that isn't finite, as from a velocity a game left so, goes nowhere.
            if !way.is_finite() {
                way = Vec3::ZERO;
            }
            let body = &bodies[character.body];
            let collider = &colliders[character.collider];
            let shape = collider.shared_shape().clone();
            let on_body = collider.position_wrt_parent().copied();
            // Where the capsule is now: its body's pose, which a move this tick changed, and its
            // place on it, rather than where the last step left the collider.
            let at = *body.position() * on_body.unwrap_or(Pose::IDENTITY);
            let mass = collider.mass();
            let moving = controller.controller();
            let own = character.body;
            // What a joint keeps it from colliding with, as rapier's contacts leave out but its
            // queries don't: asked only of a character joined to something, so that the rest sweep
            // as they did.
            let apart = |_: ColliderHandle, other: &Collider| {
                let joined = other
                    .parent()
                    .map(|other| impulse_joints.joints_between(own, other));
                !joined
                    .is_some_and(|mut joints| joints.any(|(_, joint)| !joint.data.contacts_enabled))
            };
            let mut alone = QueryFilter::new().exclude_rigid_body(own);
            if impulse_joints.attached_joints(own).next().is_some() {
                alone = alone.predicate(&apart);
            }
            let dispatcher = narrow_phase.query_dispatcher();
            let queries = broad_phase.as_query_pipeline(dispatcher, bodies, colliders, alone);
            self.hits.clear();
            let hits = &mut self.hits;
            let moved = moving.move_shape(dt, &queries, &*shape, &at, vector(way), |hit| {
                hits.push(hit);
            });
            let mut pushing =
                broad_phase.as_query_pipeline_mut(dispatcher, bodies, colliders, alone);
            moving.solve_character_collision_impulses(dt, &mut pushing, &*shape, mass, &self.hits);
            let body = &mut bodies[character.body];
            body.set_next_kinematic_translation(body.translation() + moved.translation);
            let went = Vec3::from_array(moved.translation.to_array());
            if moved.grounded && velocity.y < 0.0 {
                velocity.y = 0.0;
            } else if way.y > 0.0 && went.y < way.y {
                // Something above it took some of the way up, if not all.
                velocity.y = went.y / dt;
            }
            controller.velocity = velocity;
            controller.grounded = moved.grounded;
            if controller != character.controller {
                let kept = world.get_mut::<CharacterController3D>(character.entity);
                let kept = kept.expect("linked this step: alive, with one");
                kept.velocity = controller.velocity;
                kept.grounded = controller.grounded;
                character.controller = *kept;
            }
        }
    }

    /// Writes where each character went into its entity's translation, as it changed.
    pub(super) fn write_back_characters(&mut self, world: &mut World) {
        for character in &mut self.characters.linked {
            let body = &self.rapier.bodies[character.body];
            let at = Vec3::from_array(body.translation().to_array());
            if at != character.transform.translation {
                let transform = world.get_mut::<Transform>(character.entity);
                let transform = transform.expect("linked this step: alive, with one");
                transform.translation = at;
                character.transform = *transform;
            }
        }
    }
}

impl Character {
    /// Makes `entity`'s character in `rapier`: a kinematic body where its transform is, moved by
    /// where it's next put, and its capsule on it.
    fn make(rapier: &mut Rapier, world: &World, entity: Entity) -> Self {
        let controller = *world
            .get::<CharacterController3D>(entity)
            .expect("found with one");
        let transform = *world.get::<Transform>(entity).expect("found with one");
        let capsule = *world.get::<CapsuleCollider3D>(entity).expect("not unmoved");
        let at = Pose::from_parts(vector(transform.translation), rotation(transform.rotation));
        let built = RigidBodyBuilder::kinematic_position_based()
            .pose(at)
            .user_data(user_data(entity));
        let body = rapier.bodies.insert(built);
        let mut collider = None;
        replace(
            rapier,
            &mut collider,
            Some(capsule.collider(transform.scale)),
            body,
        );
        Self {
            entity,
            body,
            collider: collider.expect("put on the body"),
            controller,
            transform,
            capsule,
        }
    }

    /// Takes in what changed of its components since the last step left them, which the last
    /// step didn't make: where it's put and how it's turned, which it moves from; its capsule; and
    /// its controller, which the next move goes by.
    fn take_changes(&mut self, rapier: &mut Rapier, world: &World) {
        let entity = self.entity;
        let controller = *world
            .get::<CharacterController3D>(entity)
            .expect("found with one");
        let transform = *world.get::<Transform>(entity).expect("found with one");
        let capsule = *world.get::<CapsuleCollider3D>(entity).expect("not unmoved");
        let body = &mut rapier.bodies[self.body];
        if transform.translation != self.transform.translation {
            body.set_translation(vector(transform.translation), true);
        }
        if transform.rotation != self.transform.rotation {
            body.set_rotation(rotation(transform.rotation), true);
        }
        if capsule != self.capsule || transform.scale != self.transform.scale {
            let mut collider = Some(self.collider);
            replace(
                rapier,
                &mut collider,
                Some(capsule.collider(transform.scale)),
                self.body,
            );
            self.collider = collider.expect("put on the body");
        }
        self.controller = controller;
        self.transform = transform;
        self.capsule = capsule;
    }
}

#[cfg(test)]
pub(super) mod tests {
    use gg_core::math::{Quat, libm, to_radians};

    use super::super::BoxCollider3D;
    use super::super::tests::{crate_at, floor, step, transform};
    use super::*;
    use crate::BodyType;

    /// A character, its capsule the default's, 2 units tall and half a unit in radius, its middle
    /// at `at`.
    pub(in super::super) fn character(world: &mut World, at: Vec3) -> Entity {
        let at = Transform {
            translation: at,
            ..Transform::IDENTITY
        };
        let moved = (at, CharacterController3D::default());
        world.spawn((moved.0, moved.1, CapsuleCollider3D::default()))
    }

    /// A static box of `size` around `at`, turned by `rotation`.
    pub(in super::super) fn wall(
        world: &mut World,
        at: Vec3,
        size: Vec3,
        rotation: Quat,
    ) -> Entity {
        let at = Transform {
            translation: at,
            rotation,
            ..Transform::IDENTITY
        };
        let body = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        let boxed = BoxCollider3D {
            size,
            ..BoxCollider3D::default()
        };
        world.spawn((at, body, boxed))
    }

    fn controller(world: &World, entity: Entity) -> CharacterController3D {
        *world
            .get::<CharacterController3D>(entity)
            .expect("alive, with one")
    }

    fn set_velocity(world: &mut World, entity: Entity, velocity: Vec3) {
        let controller = world
            .get_mut::<CharacterController3D>(entity)
            .expect("alive");
        controller.velocity = velocity;
    }

    /// Where a character's middle rests on the floor's top, at y 0.5: its half height above it, and
    /// the gap rapier's controller keeps, a hundredth of its height.
    const ON_FLOOR: f32 = 0.5 + 1.0 + 0.02;

    /// A character dropped onto a floor falls, pulled by gravity, lands on it, grounded, its
    /// falling stopped; walks across it as fast as its velocity says, within the few hundredths
    /// rapier's controller slides it along the ground slower, keeping to it; and, stopped, stays
    /// where it stands, its components unchanged from step to step.
    #[test]
    fn a_character_falls_lands_walks_and_stands() {
        let mut world = World::default();
        floor(&mut world);
        let walker = character(&mut world, Vec3::new(0.0, 4.0, 0.0));
        step(&mut world, 16);
        let falling = controller(&world, walker);
        assert!(
            !falling.grounded && falling.velocity.y < -2.0,
            "{falling:?}"
        );
        assert!(transform(&world, walker).translation.y < 4.0);
        step(&mut world, 64);
        let landed = controller(&world, walker);
        assert!(landed.grounded, "{landed:?}");
        assert_eq!(landed.velocity, Vec3::ZERO);
        let at = transform(&world, walker).translation;
        assert!((at.y - ON_FLOOR).abs() < 0.02, "{at}");
        set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, -1.0));
        step(&mut world, 64);
        let walked = transform(&world, walker).translation;
        let want = Vec3::new(at.x + 2.0, at.y, at.z - 1.0);
        assert!(walked.distance(want) < 0.08, "{walked}, not {want}");
        assert!((walked.y - at.y).abs() < 0.005, "{walked}");
        assert!(controller(&world, walker).grounded);
        assert_eq!(
            controller(&world, walker).velocity,
            Vec3::new(2.0, 0.0, -1.0)
        );
        set_velocity(&mut world, walker, Vec3::ZERO);
        step(&mut world, 8);
        world.advance();
        let tick = world.change_tick();
        let stood = transform(&world, walker);
        step(&mut world, 8);
        assert_eq!(transform(&world, walker), stood);
        let mut tables = world.tables().iter();
        let table = tables.find(|table| table.entities().contains(&walker));
        let table = table.expect("in a table");
        let row = table.entities().iter().position(|&it| it == walker);
        let row = row.expect("a row");
        for id in [
            world.component::<Transform>(),
            world.component::<CharacterController3D>(),
        ] {
            let changed = table.changed(id.expect("registered")).expect("a column");
            assert!(changed.row(row) < tick, "{} {tick}", changed.row(row));
        }
    }

    /// A character walking into a wall at a slant slides along it, as far as the part of its
    /// velocity along the wall takes it, and no farther into it than its capsule lets it.
    #[test]
    fn a_character_slides_along_walls() {
        let mut world = World::default();
        floor(&mut world);
        // Its face at x 1.5.
        wall(
            &mut world,
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(1.0, 3.0, 10.0),
            Quat::IDENTITY,
        );
        let walker = character(&mut world, Vec3::new(0.0, ON_FLOOR, -1.5));
        step(&mut world, 4);
        set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, 2.0));
        step(&mut world, 64);
        let at = transform(&world, walker).translation;
        // Against the wall, its radius and the gap from it.
        assert!((at.x - 0.98).abs() < 0.03, "{at}");
        assert!(at.z > 0.0, "slid along: {at}");
    }

    /// A character climbs a slope gentler than its `max_slope`, and not one steeper, which it
    /// slides along at its foot.
    #[test]
    fn a_character_climbs_slopes_up_to_its_limit() {
        let mut world = World::default();
        floor(&mut world);
        // Ramps rising along +X from the floor, at 20 and 60 degrees, in lanes along Z.
        for (z, degrees) in [(-2.5_f32, 20.0_f32), (2.5, 60.0)] {
            let angle = to_radians(degrees);
            let (sin, cos) = libm::sincosf(angle);
            let length = 6.0;
            // Its top face's low edge at x 0 on the floor's top.
            let middle = Vec3::new(
                length * 0.5 * cos + 0.1 * sin,
                0.5 + length * 0.5 * sin - 0.1 * cos,
                z,
            );
            let size = Vec3::new(length, 0.2, 3.0);
            wall(&mut world, middle, size, Quat::from_rotation_z(angle));
        }
        let gentle = character(&mut world, Vec3::new(-1.5, ON_FLOOR, -2.5));
        let steep = character(&mut world, Vec3::new(-1.5, ON_FLOOR, 2.5));
        step(&mut world, 4);
        for walker in [gentle, steep] {
            set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, 0.0));
        }
        step(&mut world, 96);
        let climbed = transform(&world, gentle).translation;
        assert!(climbed.y > ON_FLOOR + 0.4, "up the gentle slope: {climbed}");
        let stopped = transform(&world, steep).translation;
        assert!(
            stopped.y < ON_FLOOR + 0.3,
            "at the steep slope's foot: {stopped}"
        );
        assert!(stopped.x < 0.5, "{stopped}");
    }

    /// A character climbs onto a step no higher than its `step_height` as it walks into it, and not
    /// onto a higher one, and walks off its edge onto the ground below, grounded at every step;
    /// and walking down a slope, keeps to it within its `snap`, where one that doesn't snap
    /// leaves it.
    #[test]
    fn a_character_climbs_steps_and_keeps_to_the_ground() {
        let mut world = World::default();
        floor(&mut world);
        // Steps 0.2 and 0.5 high, from x 1 to 2.5, in lanes along Z.
        wall(
            &mut world,
            Vec3::new(1.75, 0.6, -2.5),
            Vec3::new(1.5, 0.2, 3.0),
            Quat::IDENTITY,
        );
        wall(
            &mut world,
            Vec3::new(1.75, 0.75, 2.5),
            Vec3::new(1.5, 0.5, 3.0),
            Quat::IDENTITY,
        );
        let low = character(&mut world, Vec3::new(-1.0, ON_FLOOR, -2.5));
        let high = character(&mut world, Vec3::new(-1.0, ON_FLOOR, 2.5));
        step(&mut world, 4);
        for walker in [low, high] {
            set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, 0.0));
        }
        step(&mut world, 96);
        let up = transform(&world, low).translation;
        assert!(
            (up.y - (ON_FLOOR + 0.2)).abs() < 0.03 && up.x > 1.2,
            "on the step: {up}"
        );
        let held = transform(&world, high).translation;
        assert!(
            (held.y - ON_FLOOR).abs() < 0.03 && held.x < 0.6,
            "at its foot: {held}"
        );
        // Off the low step's far edge, at x 2.5, down to the floor: grounded throughout.
        let mut left = 0;
        for _ in 0..64 {
            step(&mut world, 1);
            left += u32::from(!controller(&world, low).grounded);
        }
        let down = transform(&world, low).translation;
        assert!(
            down.x > 3.0 && (down.y - ON_FLOOR).abs() < 0.03,
            "off the step: {down}"
        );
        assert_eq!(left, 0, "kept to the ground");
        // Down a slope of 30 degrees, falling away faster than gravity takes a character down at
        // first: one that snaps keeps to it, one that doesn't leaves it.
        let mut world = World::default();
        floor(&mut world);
        let angle = to_radians(30.0);
        let (sin, cos) = libm::sincosf(angle);
        let length = 6.0;
        let middle = Vec3::new(
            -length * 0.5 * cos - 0.1 * sin,
            0.5 + length * 0.5 * sin - 0.1 * cos,
            0.0,
        );
        let size = Vec3::new(length, 0.2, 4.0);
        wall(&mut world, middle, size, Quat::from_rotation_z(-angle));
        let snapping = character(&mut world, Vec3::new(-4.0, 5.0, -1.0));
        let unsnapped = character(&mut world, Vec3::new(-4.0, 5.0, 1.0));
        let held = world
            .get_mut::<CharacterController3D>(unsnapped)
            .expect("alive");
        held.snap = 0.0;
        step(&mut world, 64);
        let mut left = [0, 0];
        for walker in [snapping, unsnapped] {
            assert!(controller(&world, walker).grounded, "on the slope");
            set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, 0.0));
        }
        for _ in 0..32 {
            step(&mut world, 1);
            for (left, walker) in left.iter_mut().zip([snapping, unsnapped]) {
                *left += u32::from(!controller(&world, walker).grounded);
            }
        }
        assert!(left[0] == 0 && left[1] > 0, "ticks off the slope: {left:?}");
    }

    /// A character given a velocity up leaves the ground, slows as gravity pulls it and comes back
    /// down, landing where it left; and one rising into something above it stops rising there.
    #[test]
    fn a_character_jumps_and_lands_and_stops_under_ceilings() {
        let mut world = World::default();
        floor(&mut world);
        let jumper = character(&mut world, Vec3::new(-2.0, ON_FLOOR, 0.0));
        // Its bottom face at y 3.5.
        wall(
            &mut world,
            Vec3::new(2.0, 4.0, 0.0),
            Vec3::new(2.0, 1.0, 2.0),
            Quat::IDENTITY,
        );
        let under = character(&mut world, Vec3::new(2.0, ON_FLOOR, 0.0));
        step(&mut world, 4);
        for walker in [jumper, under] {
            set_velocity(&mut world, walker, Vec3::new(0.0, 6.0, 0.0));
        }
        step(&mut world, 8);
        let rising = controller(&world, jumper);
        assert!(!rising.grounded && rising.velocity.y > 4.0 && rising.velocity.y < 6.0);
        assert!(transform(&world, jumper).translation.y > ON_FLOOR + 0.4);
        // About 1.2 s up and down at 6 units a second.
        step(&mut world, 96);
        let landed = controller(&world, jumper);
        assert!(
            landed.grounded && landed.velocity == Vec3::ZERO,
            "{landed:?}"
        );
        let at = transform(&world, jumper).translation;
        assert!((at.y - ON_FLOOR).abs() < 0.03 && at.x == -2.0, "{at}");
        let stopped = transform(&world, under).translation;
        // Its top, a unit above its middle, under the ceiling, the gap from it.
        assert!(stopped.y < 2.5, "{stopped}");
    }

    /// A character walking into a dynamic box pushes it along.
    #[test]
    fn a_character_pushes_dynamic_bodies() {
        let mut world = World::default();
        floor(&mut world);
        let pushed = crate_at(&mut world, Vec3::new(1.5, 1.0, 0.0), Quat::IDENTITY);
        let walker = character(&mut world, Vec3::new(-1.0, ON_FLOOR, 0.0));
        step(&mut world, 16);
        set_velocity(&mut world, walker, Vec3::new(2.0, 0.0, 0.0));
        step(&mut world, 96);
        let moved = transform(&world, pushed).translation;
        assert!(moved.x > 2.0, "pushed: {moved}");
        assert!(transform(&world, walker).translation.x > 1.0);
    }

    /// An entity with a controller and a `RigidBody3D`, or without a `CapsuleCollider3D`, isn't a
    /// character, warned of once: the first is moved as a body, the second not at all; and one
    /// given a capsule is a character from the next step on.
    #[test]
    fn controllers_move_only_characters() {
        let mut world = World::default();
        floor(&mut world);
        let bodied = character(&mut world, Vec3::new(-2.0, 3.0, 0.0));
        world.insert(bodied, RigidBody3D::default()).expect("alive");
        let shapeless = character(&mut world, Vec3::new(2.0, 3.0, 0.0));
        world.remove::<CapsuleCollider3D>(shapeless).expect("alive");
        step(&mut world, 64);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert_eq!(physics.unmoved, [bodied, shapeless]);
        assert_eq!(physics.characters.linked.len(), 0);
        assert!(
            transform(&world, bodied).translation.y < 2.0,
            "fell as a body"
        );
        assert_eq!(transform(&world, shapeless).translation.y, 3.0);
        assert!(!controller(&world, shapeless).grounded);
        world
            .insert(shapeless, CapsuleCollider3D::default())
            .expect("alive");
        step(&mut world, 64);
        let physics = world.resource::<Physics3D>().expect("stepped");
        assert_eq!(physics.unmoved, [bodied]);
        assert_eq!(physics.characters.linked.len(), 1);
        assert!(controller(&world, shapeless).grounded);
    }

    /// Characters walking about among crates, over a floor with a ramp and a step, turning as they
    /// go: what the tests of exactness and the reference hash step.
    pub(in super::super) fn course(world: &mut World) -> Vec<Entity> {
        floor(world);
        let angle = to_radians(20.0);
        wall(
            world,
            Vec3::new(2.0, 0.7, 2.0),
            Vec3::new(4.0, 0.2, 2.0),
            Quat::from_rotation_z(angle),
        );
        wall(
            world,
            Vec3::new(-2.0, 0.6, -2.0),
            Vec3::new(2.0, 0.2, 2.0),
            Quat::IDENTITY,
        );
        for i in 0..6u8 {
            let i = f32::from(i);
            crate_at(
                world,
                Vec3::new(i - 2.5, 1.0 + i * 0.4, (i % 2.0) * 1.5 - 0.75),
                Quat::IDENTITY,
            );
        }
        let mut walkers = Vec::new();
        for i in 0..4u8 {
            let i = f32::from(i);
            let walker = character(world, Vec3::new(i * 1.5 - 2.25, 3.0, 3.5 - i));
            let velocity = Vec3::new(1.5 - i, 0.0, i - 1.5) * 1.5;
            let controller = world
                .get_mut::<CharacterController3D>(walker)
                .expect("alive");
            controller.velocity = velocity;
            walkers.push(walker);
        }
        walkers
    }

    /// Turns each walker's velocity a little about Y, and has the first jump whenever it stands, as
    /// a game's input would, at each step of `ticks`.
    pub(in super::super) fn walk(world: &mut World, walkers: &[Entity], ticks: u32) {
        let turn = Quat::from_rotation_y(0.05);
        for _ in 0..ticks {
            for (i, &walker) in walkers.iter().enumerate() {
                let controller = world
                    .get_mut::<CharacterController3D>(walker)
                    .expect("alive");
                let across = turn * Vec3::new(controller.velocity.x, 0.0, controller.velocity.z);
                controller.velocity = Vec3::new(across.x, controller.velocity.y, across.z);
                if i == 0 && controller.grounded {
                    controller.velocity.y = 4.0;
                }
            }
            step(world, 1);
        }
    }

    /// A snapshot of a world whose characters are walking about and pushing crates, restored,
    /// steps as the world it was taken from did, bit for bit, as does a copy.
    #[test]
    fn a_snapshot_with_characters_steps_as_its_world_did() {
        let mut world = World::seeded(7);
        let walkers = course(&mut world);
        walk(&mut world, &walkers, 40);
        let snapshot = world.clone();
        walk(&mut world, &walkers, 60);
        let went = world.hash();
        let mut copy = snapshot.clone();
        walk(&mut copy, &walkers, 60);
        assert_eq!(copy.hash(), went, "a copy");
        world.clone_from(&snapshot);
        walk(&mut world, &walkers, 60);
        assert_eq!(world.hash(), went, "restored");
    }

    /// A world whose characters are walking about and pushing crates, saved whole to text as a
    /// snapshot file holds it and loaded into a world of another process's making, steps as the
    /// world it was saved from did, bit for bit.
    #[test]
    #[cfg(feature = "dev")]
    fn a_world_saved_whole_with_characters_steps_as_it_did() {
        use crate::Saved as _;

        let mut world = World::seeded(7);
        world.register_saved::<Physics3D>().expect("saved");
        let walkers = course(&mut world);
        walk(&mut world, &walkers, 40);
        let mut text = String::new();
        world.save_whole(&mut text).expect("saved");
        let mut loaded = World::seeded(1);
        for registered in [
            loaded.register::<Transform>(),
            loaded.register::<RigidBody3D>(),
            loaded.register::<BoxCollider3D>(),
            loaded.register::<CapsuleCollider3D>(),
            loaded.register::<CharacterController3D>(),
            loaded.register_saved::<Physics3D>(),
        ] {
            registered.expect("registered");
        }
        let lines = (1..).zip(text.lines());
        let changes = loaded.load_whole(lines, true).expect("loaded");
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(loaded.hash(), world.hash(), "loaded");
        let physics = loaded.resource::<Physics3D>().expect("loaded");
        assert_eq!(physics.characters.linked.len(), walkers.len());
        let saved = physics.save(&loaded).expect("saved again");
        assert!(text.contains(&saved), "it saves as it loaded");
        walk(&mut world, &walkers, 60);
        walk(&mut loaded, &walkers, 60);
        assert_eq!(loaded.hash(), world.hash(), "stepped on");
    }

    /// The world's hash after characters have walked the course for four seconds, which CI checks
    /// on every platform: a difference on some platforms only is a bug in determinism, not a value
    /// to change here.
    #[test]
    fn a_course_hashes_as_its_reference_says() {
        let mut world = World::seeded(7);
        let walkers = course(&mut world);
        walk(&mut world, &walkers, 256);
        let content = world.hash().content;
        assert_eq!(content, 0x869f_f714_d0cc_6e4d, "{content:#018x}");
    }
}
