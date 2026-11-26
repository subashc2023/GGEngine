//! Casts through the world's 3D colliders: a ray, or a shape swept along a line.

use gg_core::math::{DVec3, Quat, Vec3};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::pipeline::QueryPipeline;
use rapier3d::prelude::{
    Ball, Capsule, Collider, ColliderHandle, Cuboid, Pose, QueryFilter, Ray, Shape,
};

use super::{Physics3D, at, vector};
use crate::{Entity, World};

/// A cast through the world's 3D colliders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cast {
    /// Where it starts, in the world's space, in `f64` as positions are.
    pub from: DVec3,
    /// Which way it goes: any length but 0.
    pub toward: Vec3,
    /// How far it goes, in units: unlimited at `f32::INFINITY`.
    pub max_distance: f32,
    /// An entity whose colliders it passes through, as a character's own, if any.
    pub exclude: Option<Entity>,
}

impl Cast {
    /// A cast from `from` toward `toward`, unlimited, through every collider.
    pub fn new(from: DVec3, toward: Vec3) -> Self {
        Self {
            from,
            toward,
            max_distance: f32::INFINITY,
            exclude: None,
        }
    }

    /// This cast, no farther than `max_distance`.
    pub fn within(self, max_distance: f32) -> Self {
        Self {
            max_distance,
            ..self
        }
    }

    /// The unit direction, if it goes anywhere: none for a zero direction, a non-finite start, or
    /// a distance that isn't 0 or more.
    fn way(&self) -> Option<Vec3> {
        let goes = self.max_distance >= 0.0 && self.from.is_finite();
        self.toward.try_normalize().filter(|_| goes)
    }

    /// Where this cast meets the plane through `point` whose normal is `normal` (any length but 0),
    /// ahead of [`from`](Self::from) and within [`max_distance`](Self::max_distance): where a
    /// cursor's [ray](crate::Context::ray) meets the ground (`DVec3::ZERO`, `Vec3::Y`) or a 2D
    /// game's z = 0 plane (`DVec3::ZERO`, `Vec3::Z`).
    ///
    /// `None` for a cast that goes nowhere (as [`World::cast_ray`] reads one), one parallel to the
    /// plane, a plane behind it or beyond its distance, or a `point` or `normal` that isn't finite
    /// or is 0.
    pub fn meets_plane(&self, point: DVec3, normal: Vec3) -> Option<DVec3> {
        let way = self.way()?;
        let normal = normal.as_dvec3();
        let (way, length) = (way.as_dvec3(), normal.length());
        let closing = way.dot(normal);
        // `way` is an `f32`'s unit: a rounded direction along the plane still counts as parallel.
        let across = closing.abs() > length * 1e-6;
        let distance = (point - self.from).dot(normal) / closing;
        let reached = distance >= 0.0 && distance <= f64::from(self.max_distance);
        (across && reached && length.is_finite() && point.is_finite())
            .then(|| self.from + way * distance)
    }

    /// This cast, passing through `entity`'s colliders.
    pub fn excluding(self, entity: Entity) -> Self {
        Self {
            exclude: Some(entity),
            ..self
        }
    }
}

/// A shape that [`World::cast_shape`] sweeps through the world, unscaled, in the space it's cast
/// from, like the colliders of the same names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape3D {
    /// A box around its middle, `size` wide, tall and deep.
    Box { size: Vec3 },
    /// A sphere around its middle.
    Sphere { radius: f32 },
    /// What's within `radius` of a segment along Y through its middle, `height` tall from the
    /// bottom of its lower cap to the top of its upper. A sphere where `height` is at most twice
    /// `radius`.
    Capsule { height: f32, radius: f32 },
}

impl Shape3D {
    /// What `f` gives of the shape as rapier's, with sizes of at least 0, made in place so a
    /// script's cast allocates nothing.
    fn with<R>(self, f: impl FnOnce(&dyn Shape) -> R) -> R {
        let at_least_0 = |x: f32| f64::from(if x > 0.0 { x } else { 0.0 });
        match self {
            Self::Box { size } => f(&Cuboid::new(vector(size.abs() * 0.5))),
            Self::Sphere { radius } => f(&Ball::new(at_least_0(radius))),
            Self::Capsule { height, radius } => {
                let radius = if radius > 0.0 { radius } else { 0.0 };
                f(&Capsule::new_y(
                    at_least_0(height * 0.5 - radius),
                    radius.into(),
                ))
            }
        }
    }
}

/// What a cast hit first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit3D {
    /// The entity whose collider it was.
    pub entity: Entity,
    /// Where it hit, in the world's space, in `f64` as positions are.
    pub point: DVec3,
    /// The unit vector out of the collider's surface there: none where the cast started inside.
    pub normal: Vec3,
    /// How far along the cast it hit, in units: 0 where it started inside the collider.
    pub distance: f32,
}

impl World {
    /// What a ray from `cast.from` toward `cast.toward` hits first of the world's 3D colliders,
    /// within `cast.max_distance`, skipping sensors, those of `cast.exclude` and those of entities
    /// gone. Nothing for a world that has never stepped 3D physics, a zero direction, or a
    /// distance that isn't 0 or more. It sees the colliders as the last step left them, none made
    /// since; a ray that starts inside one hits it there.
    pub fn cast_ray(&self, cast: Cast) -> Option<Hit3D> {
        self.ray(cast, false)
    }

    /// As [`cast_ray`](Self::cast_ray), passing through the colliders `cast.from` is inside, as a
    /// first-person camera is inside its player's.
    #[cfg(feature = "dev")]
    pub(crate) fn cast_ray_from_outside(&self, cast: Cast) -> Option<Hit3D> {
        self.ray(cast, true)
    }

    fn ray(&self, cast: Cast, from_outside: bool) -> Option<Hit3D> {
        let physics = self.resource::<Physics3D>()?;
        let toward = cast.way()?;
        let ray = Ray::new(at(cast.from), vector(toward));
        let max_distance = f64::from(cast.max_distance);
        let inside = from_outside.then_some(cast.from);
        physics.cast(self, cast.exclude, inside, |queries| {
            let (collider, hit) = queries.cast_ray_and_get_normal(&ray, max_distance, true)?;
            let point = cast.from + toward.as_dvec3() * hit.time_of_impact;
            let normal = Vec3::from_array(hit.normal.to_array().map(|x| x as f32));
            Some((collider, point, normal, hit.time_of_impact as f32))
        })
    }

    /// What `shape`, turned by `rotation` and swept from `cast.from` toward `cast.toward`, hits
    /// first, as for [`cast_ray`](Self::cast_ray): `point` is where its surface first touches the
    /// collider's, with the shape at `distance` along the way. A shape that starts inside one
    /// hits it there.
    pub fn cast_shape(&self, shape: Shape3D, rotation: Quat, cast: Cast) -> Option<Hit3D> {
        let physics = self.resource::<Physics3D>()?;
        let toward = cast.way()?;
        let at = Pose::from_parts(at(cast.from), super::rotation(rotation));
        let options = ShapeCastOptions {
            max_time_of_impact: cast.max_distance.into(),
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        physics.cast(self, cast.exclude, None, |queries| {
            let cast = |shape: &dyn Shape| queries.cast_shape(&at, vector(toward), shape, options);
            let (collider, hit) = shape.with(cast)?;
            let point = DVec3::from_array(hit.witness1.to_array());
            let normal = Vec3::from_array(hit.normal1.to_array().map(|x| x as f32));
            Some((collider, point, normal, hit.time_of_impact as f32))
        })
    }
}

impl Physics3D {
    /// Runs `cast` against the colliders, skipping sensors, `exclude`'s, those of entities gone
    /// from `world` and those holding the point `inside`, and adds the entity hit.
    fn cast(
        &self,
        world: &World,
        exclude: Option<Entity>,
        inside: Option<DVec3>,
        cast: impl FnOnce(&QueryPipeline<'_>) -> Option<Cast3D>,
    ) -> Option<Hit3D> {
        let rapier = &self.rapier;
        let entity_of = |collider: &Collider| {
            let body = rapier.bodies.get(collider.parent()?)?;
            let bits = u64::try_from(body.user_data).ok()?;
            Entity::from_bits(bits).filter(|&entity| world.contains(entity))
        };
        let outside = |collider: &Collider| {
            inside.is_none_or(|point| {
                !collider
                    .shape()
                    .contains_point(collider.position(), at(point))
            })
        };
        let alive = |_: ColliderHandle, collider: &Collider| {
            entity_of(collider).is_some() && outside(collider)
        };
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

/// What a cast gives on a hit: collider, point, normal, distance.
type Cast3D = (ColliderHandle, DVec3, Vec3, f32);

#[cfg(test)]
mod tests {
    use super::super::character::tests::{character, wall};
    use super::super::tests::{floor, step};
    use super::*;

    /// A floor with its top at y 0.5 and a unit box on it around (0, 1, 0), stepped once.
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

    /// A cast meets a plane ahead of it and within its distance, wherever the plane's normal faces,
    /// and never one parallel to it or behind.
    #[test]
    fn casts_meet_planes_ahead_of_them() {
        let down = Cast::new(DVec3::new(1.0, 4.0, -2.0), Vec3::new(0.0, -3.0, 0.0));
        let ground = down.meets_plane(DVec3::ZERO, Vec3::Y).expect("the ground");
        assert!(
            ground.abs_diff_eq(DVec3::new(1.0, 0.0, -2.0), 1e-9),
            "{ground}"
        );
        let shifted = down.meets_plane(DVec3::new(9.0, 1.0, 9.0), Vec3::new(0.0, -2.0, 0.0));
        assert_eq!(shifted, Some(DVec3::new(1.0, 1.0, -2.0)));
        let slanted = Cast::new(DVec3::new(0.0, 4.0, 0.0), Vec3::new(0.0, -1.0, 1.0));
        let slanted = slanted.meets_plane(DVec3::ZERO, Vec3::Y).expect("slanted");
        assert!(
            slanted.abs_diff_eq(DVec3::new(0.0, 0.0, 4.0), 1e-5),
            "{slanted}"
        );
        assert_eq!(down.within(3.9).meets_plane(DVec3::ZERO, Vec3::Y), None);
        assert!(down.within(4.0).meets_plane(DVec3::ZERO, Vec3::Y).is_some());
        let up = Cast::new(DVec3::new(0.0, 4.0, 0.0), Vec3::Y);
        assert_eq!(up.meets_plane(DVec3::ZERO, Vec3::Y), None);
        let along = Cast::new(DVec3::new(0.0, 4.0, 0.0), Vec3::X);
        assert_eq!(along.meets_plane(DVec3::ZERO, Vec3::Y), None);
        assert_eq!(down.meets_plane(DVec3::ZERO, Vec3::ZERO), None);
        let nowhere = Cast::new(DVec3::new(0.0, 4.0, 0.0), Vec3::ZERO);
        assert_eq!(nowhere.meets_plane(DVec3::ZERO, Vec3::Y), None);
        // A 2D game's plane, from in front.
        let front = Cast::new(DVec3::new(2.0, 3.0, 5.0), Vec3::NEG_Z);
        let front = front.meets_plane(DVec3::ZERO, Vec3::Z);
        assert_eq!(front, Some(DVec3::new(2.0, 3.0, 0.0)));
    }

    /// A ray hits the nearest collider in its way, passes through its excluded entity's, and stops
    /// short at its distance. One that starts inside a collider hits it there.
    #[test]
    fn rays_hit_the_nearest_collider() {
        let (world, floor, boxed) = stepped();
        let down = Cast::new(DVec3::new(0.25, 5.0, -0.25), Vec3::new(0.0, -2.0, 0.0));
        let hit = world.cast_ray(down).expect("the box");
        assert_eq!(hit.entity, boxed);
        assert!(
            hit.point.abs_diff_eq(DVec3::new(0.25, 1.5, -0.25), 1e-5),
            "{hit:?}"
        );
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5), "{hit:?}");
        assert!((hit.distance - 3.5).abs() < 1e-5, "{hit:?}");
        let through = world.cast_ray(down.excluding(boxed)).expect("the floor");
        assert_eq!(through.entity, floor);
        assert!((through.distance - 4.5).abs() < 1e-5, "{through:?}");
        assert_eq!(world.cast_ray(down.within(3.0)), None);
        let across = Cast::new(DVec3::new(-4.0, 1.0, 0.0), Vec3::X);
        let side = world.cast_ray(across).expect("the box's side");
        assert!(side.normal.abs_diff_eq(Vec3::NEG_X, 1e-5) && (side.distance - 3.5).abs() < 1e-5);
        let inside = world.cast_ray(Cast::new(DVec3::new(0.0, 1.0, 0.0), Vec3::X));
        let inside = inside.expect("the box, from inside it");
        assert_eq!((inside.entity, inside.distance), (boxed, 0.0));
    }

    /// A sphere, capsule and box swept down hit the box's top where their lowest points touch it;
    /// a turned box reaches as its corner does.
    #[test]
    fn shapes_hit_where_their_surfaces_touch() {
        let (world, _, boxed) = stepped();
        let down = Cast::new(DVec3::new(0.1, 5.0, 0.0), Vec3::NEG_Y);
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

    /// A cast finds nothing in a world that never stepped 3D physics, for a zero direction or a NaN
    /// distance; nor the colliders of an entity gone since the last step, nor those made since,
    /// as a character's.
    #[test]
    fn casts_find_only_what_the_last_step_left() {
        let empty = World::default();
        let down = Cast::new(DVec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        assert_eq!(empty.cast_ray(down), None);
        let (mut world, floor, boxed) = stepped();
        assert_eq!(world.cast_ray(Cast::new(DVec3::ZERO, Vec3::ZERO)), None);
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
