//! Casts through the world's 3D colliders: a ray, or a shape swept along a line, giving what it
//! hits first.

use gg_core::math::{Quat, Vec3};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::pipeline::QueryPipeline;
use rapier3d::prelude::{
    Ball, Capsule, Collider, ColliderHandle, Cuboid, Pose, QueryFilter, Ray, Shape,
};

use super::{Physics3D, vector};
use crate::{Entity, World};

/// A cast through the world's 3D colliders: where it starts, which way it goes, how far, and an
/// entity whose colliders it passes through, as a character's own as it looks about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cast {
    /// Where it starts, in the world's space.
    pub from: Vec3,
    /// Which way it goes, of any length but none.
    pub toward: Vec3,
    /// How far it goes, in units: without end at `f32::INFINITY`.
    pub max_distance: f32,
    /// An entity whose colliders it passes through, if any.
    pub exclude: Option<Entity>,
}

impl Cast {
    /// A cast from `from` toward `toward`, without end, through every collider.
    pub fn new(from: Vec3, toward: Vec3) -> Self {
        Self {
            from,
            toward,
            max_distance: f32::INFINITY,
            exclude: None,
        }
    }

    /// This cast, going no farther than `max_distance`.
    pub fn within(self, max_distance: f32) -> Self {
        Self {
            max_distance,
            ..self
        }
    }

    /// The unit vector of the way it goes, if it goes anywhere: none for a way of no length, a
    /// start that isn't a point, or a distance that isn't 0 or more.
    fn way(&self) -> Option<Vec3> {
        let goes = self.max_distance >= 0.0 && self.from.is_finite();
        self.toward.try_normalize().filter(|_| goes)
    }

    /// This cast, passing through `entity`'s colliders.
    pub fn excluding(self, entity: Entity) -> Self {
        Self {
            exclude: Some(entity),
            ..self
        }
    }
}

/// A shape that [`World::cast_shape`] sweeps through the world, in the space of where it's cast
/// from, as the colliders of the same names are, unscaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape3D {
    /// A box around its middle, as wide, tall and deep as `size`.
    Box { size: Vec3 },
    /// A sphere around its middle.
    Sphere { radius: f32 },
    /// What's within `radius` of a segment along its Y through its middle, as tall as `height`
    /// from the bottom of its lower cap to the top of its upper, a sphere where that's no more than
    /// twice its radius.
    Capsule { height: f32, radius: f32 },
}

impl Shape3D {
    /// What `f` gives of the shape as rapier's, of sizes of no less than 0, made where it's
    /// given rather than allocated, as a script's cast allocates nothing.
    fn with<R>(self, f: impl FnOnce(&dyn Shape) -> R) -> R {
        let at_least_0 = |x: f32| if x > 0.0 { x } else { 0.0 };
        match self {
            Self::Box { size } => f(&Cuboid::new(vector(size.abs() * 0.5))),
            Self::Sphere { radius } => f(&Ball::new(at_least_0(radius))),
            Self::Capsule { height, radius } => {
                let radius = at_least_0(radius);
                f(&Capsule::new_y(at_least_0(height * 0.5 - radius), radius))
            }
        }
    }
}

/// What a cast hit first: the entity whose collider it was, where it hit, the way the collider's
/// surface faces there, and how far along the cast that was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit3D {
    pub entity: Entity,
    /// Where it hit the collider, in the world's space.
    pub point: Vec3,
    /// The unit vector out of the collider's surface where it hit: none where the cast started
    /// inside it.
    pub normal: Vec3,
    /// How far along the cast it hit, in units: 0 where it started inside the collider.
    pub distance: f32,
}

impl World {
    /// What a ray from `cast.from` toward `cast.toward` hits first of the world's 3D colliders,
    /// within `cast.max_distance`, but sensors, those of `cast.exclude` and those of entities gone:
    /// nothing
    /// for a world that has never stepped 3D physics, for a way of no length, or for a distance
    /// that isn't 0 or more. It sees the colliders where the last step of physics left them, and
    /// none made since; a ray that starts inside one hits it there.
    pub fn cast_ray(&self, cast: Cast) -> Option<Hit3D> {
        let physics = self.resource::<Physics3D>()?;
        let toward = cast.way()?;
        let ray = Ray::new(vector(cast.from), vector(toward));
        physics.cast(self, cast.exclude, |queries| {
            let (collider, hit) = queries.cast_ray_and_get_normal(&ray, cast.max_distance, true)?;
            let point = cast.from + toward * hit.time_of_impact;
            let normal = Vec3::from_array(hit.normal.to_array());
            Some((collider, point, normal, hit.time_of_impact))
        })
    }

    /// What `shape`, turned by `rotation` and swept from `cast.from` toward `cast.toward`, hits
    /// first of the world's 3D colliders, as [`cast_ray`](Self::cast_ray) says: where its surface
    /// first touches theirs, on the collider hit, as the shape is at `distance` along the way. A
    /// shape that starts inside one hits it there.
    pub fn cast_shape(&self, shape: Shape3D, rotation: Quat, cast: Cast) -> Option<Hit3D> {
        let physics = self.resource::<Physics3D>()?;
        let toward = cast.way()?;
        let at = Pose::from_parts(vector(cast.from), super::rotation(rotation));
        let options = ShapeCastOptions {
            max_time_of_impact: cast.max_distance,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        physics.cast(self, cast.exclude, |queries| {
            let cast = |shape: &dyn Shape| queries.cast_shape(&at, vector(toward), shape, options);
            let (collider, hit) = shape.with(cast)?;
            let point = Vec3::from_array(hit.witness1.to_array());
            let normal = Vec3::from_array(hit.normal1.to_array());
            Some((collider, point, normal, hit.time_of_impact))
        })
    }
}

impl Physics3D {
    /// What `cast` gives, run against the colliders, but sensors and those of `exclude` and of
    /// entities gone from `world`, as its hit, with the entity whose collider it hit.
    fn cast(
        &self,
        world: &World,
        exclude: Option<Entity>,
        cast: impl FnOnce(&QueryPipeline<'_>) -> Option<Cast3D>,
    ) -> Option<Hit3D> {
        let rapier = &self.rapier;
        let entity_of = |collider: &Collider| {
            let body = rapier.bodies.get(collider.parent()?)?;
            let bits = u64::try_from(body.user_data).ok()?;
            Entity::from_bits(bits).filter(|&entity| world.contains(entity))
        };
        let alive = |_: ColliderHandle, collider: &Collider| entity_of(collider).is_some();
        let mut filter = QueryFilter::new().exclude_sensors().predicate(&alive);
        if let Some(body) = exclude.and_then(|entity| self.body_of(entity)) {
            filter = filter.exclude_rigid_body(body);
        }
        let dispatcher = rapier.narrow_phase.query_dispatcher();
        let queries = (rapier.broad_phase).as_query_pipeline(
            dispatcher,
            &rapier.bodies,
            &rapier.colliders,
            filter,
        );
        let (collider, point, normal, distance) = cast(&queries)?;
        Some(Hit3D {
            entity: entity_of(&rapier.colliders[collider])?,
            point,
            normal,
            distance,
        })
    }
}

/// What a cast gives as it hits: the collider, where it hit, its surface's way there, and how far.
type Cast3D = (ColliderHandle, Vec3, Vec3, f32);

#[cfg(test)]
mod tests {
    use super::super::character::tests::{character, wall};
    use super::super::tests::{floor, step};
    use super::*;

    /// A floor, its top at y 0.5, and a unit box on it around (0, 1, 0), stepped once.
    fn stepped() -> (World, Entity, Entity) {
        let mut world = World::default();
        let floor = floor(&mut world);
        let boxed = wall(
            &mut world,
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::ONE,
            Quat::IDENTITY,
        );
        step(&mut world, 1);
        (world, floor, boxed)
    }

    /// A ray hits the nearest collider in its way, giving its entity, where, the surface's normal
    /// there and how far; and passes through those of the entity it excludes, and stops short as
    /// far as it goes. One that starts inside a collider hits it there.
    #[test]
    fn rays_hit_the_nearest_collider() {
        let (world, floor, boxed) = stepped();
        let down = Cast::new(Vec3::new(0.25, 5.0, -0.25), Vec3::new(0.0, -2.0, 0.0));
        let hit = world.cast_ray(down).expect("the box");
        assert_eq!(hit.entity, boxed);
        assert!(
            hit.point.abs_diff_eq(Vec3::new(0.25, 1.5, -0.25), 1e-5),
            "{hit:?}"
        );
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5), "{hit:?}");
        assert!((hit.distance - 3.5).abs() < 1e-5, "{hit:?}");
        let through = world.cast_ray(down.excluding(boxed)).expect("the floor");
        assert_eq!(through.entity, floor);
        assert!((through.distance - 4.5).abs() < 1e-5, "{through:?}");
        assert_eq!(world.cast_ray(down.within(3.0)), None);
        let across = Cast::new(Vec3::new(-4.0, 1.0, 0.0), Vec3::X);
        let side = world.cast_ray(across).expect("the box's side");
        assert!(side.normal.abs_diff_eq(Vec3::NEG_X, 1e-5) && (side.distance - 3.5).abs() < 1e-5);
        let inside = world.cast_ray(Cast::new(Vec3::new(0.0, 1.0, 0.0), Vec3::X));
        let inside = inside.expect("the box, from inside it");
        assert_eq!((inside.entity, inside.distance), (boxed, 0.0));
    }

    /// A sphere, a capsule and a box swept down hit the box's top as far down as their lowest
    /// points reach it, where their surfaces touch; a turned box reaches as its corner does.
    #[test]
    fn shapes_hit_where_their_surfaces_touch() {
        let (world, _, boxed) = stepped();
        let down = Cast::new(Vec3::new(0.1, 5.0, 0.0), Vec3::NEG_Y);
        for (shape, reach) in [
            (Shape3D::Sphere { radius: 0.5 }, 0.5),
            (
                Shape3D::Capsule {
                    height: 2.0,
                    radius: 0.25,
                },
                1.0,
            ),
            (
                Shape3D::Box {
                    size: Vec3::splat(0.5),
                },
                0.25,
            ),
        ] {
            let hit = world
                .cast_shape(shape, Quat::IDENTITY, down)
                .expect("the box");
            assert_eq!(hit.entity, boxed, "{shape:?}");
            assert!(
                (hit.distance - (3.5 - reach)).abs() < 1e-3,
                "{shape:?}: {hit:?}"
            );
            assert!((hit.point.y - 1.5).abs() < 1e-3, "{shape:?}: {hit:?}");
            assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-3), "{shape:?}: {hit:?}");
        }
        let turned = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let corner = world.cast_shape(Shape3D::Box { size: Vec3::ONE }, turned, down);
        let corner = corner.expect("the box");
        let half_diagonal = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (corner.distance - (3.5 - half_diagonal)).abs() < 1e-3,
            "{corner:?}"
        );
    }

    /// A cast finds nothing in a world that has never stepped 3D physics, for a way of no length
    /// or a distance that isn't a number; nor the colliders of an entity gone since the last step,
    /// nor those made since, as a character's.
    #[test]
    fn casts_find_only_what_the_last_step_left() {
        let empty = World::default();
        let down = Cast::new(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        assert_eq!(empty.cast_ray(down), None);
        let (mut world, floor, boxed) = stepped();
        assert_eq!(world.cast_ray(Cast::new(Vec3::ZERO, Vec3::ZERO)), None);
        assert_eq!(world.cast_ray(down.within(f32::NAN)), None);
        world.despawn(boxed).expect("alive");
        let made = character(&mut world, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(world.cast_ray(down).map(|hit| hit.entity), Some(floor));
        step(&mut world, 1);
        assert_eq!(world.cast_ray(down).map(|hit| hit.entity), Some(made));
        let past = world.cast_ray(down.excluding(made));
        assert_eq!(past.map(|hit| hit.entity), Some(floor));
    }
}
