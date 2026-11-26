//! The sphere and boxes round what an entity and its descendants draw: the tool API's `entity`
//! answers them as `bounds` and `box`, the editor frames the sphere, and the Bounds overlay draws a
//! box.

use gg_core::math::{DAffine3, DVec3, Vec3};
use gg_renderer::{Circle, MeshDraw, Meshes, Sprite};

use crate::{Entity, Transform, World};

/// A sphere: center (`f64`, like positions) and radius.
pub(crate) type Sphere = (DVec3, f64);

/// The radius of the sphere round a unit square centered on its origin: its half diagonal.
const SQUARE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The smallest sphere round what `root` and its descendants draw, as the latest tick left them:
/// each draw's sphere through its entity's pose, merged.
///
/// - Mesh: the sphere `meshes` keeps.
/// - Sprite or circle: round its unit square, scaled by the pose's largest axis.
/// - Left out: text (its extent needs layout), particles, lines, meshes without a finite sphere.
///
/// `None` if nothing draws or `root` isn't alive.
pub(crate) fn of(world: &World, meshes: &Meshes, root: Entity) -> Option<Sphere> {
    let mut bounds: Option<Sphere> = None;
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        stack.extend(world.children(entity).ok()?);
        if world.get::<Transform>(entity).is_none() {
            continue;
        }
        let mut own = [None; 2];
        own[0] = world
            .get::<MeshDraw>(entity)
            .and_then(|draw| meshes.sphere_of(draw));
        if world.get::<Sprite>(entity).is_some() || world.get::<Circle>(entity).is_some() {
            own[1] = Some((Vec3::ZERO, SQUARE));
        }
        if own.iter().all(Option::is_none) {
            continue;
        }
        let pose = world.pose(entity).ok()?.matrix;
        let axes = pose.matrix3;
        let scale = [axes.x_axis, axes.y_axis, axes.z_axis]
            .map(DVec3::length)
            .into_iter()
            .fold(0.0, f64::max);
        for (center, radius) in own.into_iter().flatten() {
            let center = pose.transform_point3(center.as_dvec3());
            let placed = (center, f64::from(radius) * scale);
            if placed.0.is_finite() && placed.1.is_finite() {
                bounds = Some(bounds.map_or(placed, |bounds| union(bounds, placed)));
            }
        }
    }
    bounds
}

/// The box round what `root` and its descendants draw, least and most corners in `root`'s space,
/// as the latest tick left them, with `root`'s pose, which places it: each draw's box through its
/// pose, merged. The Bounds overlay draws it, a box hugging a long wall or a scaled sprite where a
/// sphere would reach far past it.
///
/// - Mesh: the box `meshes` keeps, else the cube round its sphere.
/// - Sprite or circle: its unit square, flat.
/// - Left out as [`of`] leaves them out.
///
/// `None` if nothing draws, `root` isn't alive or its pose has no inverse.
pub(crate) fn boxed(
    world: &World,
    meshes: &Meshes,
    root: Entity,
) -> Option<(DAffine3, [DVec3; 2])> {
    let placed = world.pose(root).ok()?.matrix;
    let into_root = placed.inverse();
    if !into_root.is_finite() {
        return None;
    }
    Some((placed, box_in(world, meshes, root, into_root)?))
}

/// The box along the world's axes round what `root` and its descendants draw, as [`boxed`] reads
/// them: the tool API's `entity` answers it as `box`.
pub(crate) fn world_box(world: &World, meshes: &Meshes, root: Entity) -> Option<[DVec3; 2]> {
    box_in(world, meshes, root, DAffine3::IDENTITY)
}

/// The box round what `root` and its descendants draw in the space `space` takes the world into,
/// as [`boxed`] says.
fn box_in(world: &World, meshes: &Meshes, root: Entity, space: DAffine3) -> Option<[DVec3; 2]> {
    let mut bounds: Option<[DVec3; 2]> = None;
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        stack.extend(world.children(entity).ok()?);
        if world.get::<Transform>(entity).is_none() {
            continue;
        }
        let mut own = [None; 2];
        own[0] = world.get::<MeshDraw>(entity).and_then(|draw| {
            let round = |(center, radius): (Vec3, f32)| [center - radius, center + radius];
            meshes
                .box_of(draw)
                .or_else(|| meshes.sphere_of(draw).map(round))
        });
        if world.get::<Sprite>(entity).is_some() || world.get::<Circle>(entity).is_some() {
            own[1] = Some([Vec3::new(-0.5, -0.5, 0.0), Vec3::new(0.5, 0.5, 0.0)]);
        }
        if own.iter().all(Option::is_none) {
            continue;
        }
        let pose = space * world.pose(entity).ok()?.matrix;
        for [low, high] in own.into_iter().flatten() {
            let [low, high] = [low, high].map(Vec3::as_dvec3);
            for corner in 0..8 {
                let pick = |bit: usize, axis: usize| match corner >> bit & 1 {
                    0 => low[axis],
                    _ => high[axis],
                };
                let at = pose.transform_point3(DVec3::new(pick(0, 0), pick(1, 1), pick(2, 2)));
                if at.is_finite() {
                    bounds = Some(bounds.map_or([at, at], |[l, h]| [l.min(at), h.max(at)]));
                }
            }
        }
    }
    bounds
}

/// The smallest sphere that holds both `a` and `b`.
fn union((a, a_radius): Sphere, (b, b_radius): Sphere) -> Sphere {
    let apart = a.distance(b);
    if apart + b_radius <= a_radius {
        return (a, a_radius);
    }
    if apart + a_radius <= b_radius {
        return (b, b_radius);
    }
    // Neither holds the other, so the centers differ.
    let radius = (apart + a_radius + b_radius) / 2.0;
    (a + (b - a) * ((radius - a_radius) / apart), radius)
}

#[cfg(test)]
mod tests {
    use gg_core::math::Quat;
    use gg_renderer::{Lod, Lods};

    use super::*;

    /// A mesh in `meshes` with the sphere `radius` round `center`, as a draw component.
    fn mesh(meshes: &mut Meshes, center: Vec3, radius: f32) -> MeshDraw {
        let lods = Lods {
            levels: vec![Lod {
                start: 0,
                count: 3,
                error: 0.0,
            }],
            center,
            radius,
        };
        let corners = [[0.0_f32; 3]; 3];
        let mesh = (meshes.add_lods("m", &corners, &[0, 1, 2], lods)).expect("a triangle");
        MeshDraw::from_parts(mesh.index(), 0, false, false, &[]).expect("no parameters")
    }

    fn at(x: f32) -> Transform {
        Transform {
            translation: DVec3::new(f64::from(x), 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    fn close(sphere: Option<Sphere>, center: DVec3, radius: f64) {
        let (found, found_radius) = sphere.expect("a sphere");
        assert!(
            found.distance(center) < 1e-5 && (found_radius - radius).abs() < 1e-5,
            "{found:?} {found_radius}, not {center:?} {radius}"
        );
    }

    #[test]
    fn a_mesh_s_sphere_goes_through_its_pose() {
        let (mut world, mut meshes) = (World::default(), Meshes::default());
        let draw = mesh(&mut meshes, Vec3::new(1.0, 0.0, 0.0), 2.0);
        let turned = Transform {
            translation: DVec3::new(0.0, 10.0, 0.0),
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::new(2.0, 3.0, 1.0),
        };
        let entity = world.spawn((turned, draw));
        // (1, 0, 0) scales to (2, 0, 0), turns to (0, 2, 0), moves up 10; the radius takes the
        // largest scale.
        close(of(&world, &meshes, entity), DVec3::new(0.0, 12.0, 0.0), 6.0);
    }

    #[test]
    fn a_parent_holds_what_its_children_draw_and_a_child_only_its_own() {
        let (mut world, mut meshes) = (World::default(), Meshes::default());
        let crate_ = world.spawn(at(0.0));
        let node = world.spawn((at(0.0), mesh(&mut meshes, Vec3::ZERO, 1.0)));
        let far = world.spawn((at(10.0), mesh(&mut meshes, Vec3::ZERO, 1.0)));
        for child in [node, far] {
            world.set_parent(child, Some(crate_)).expect("alive");
        }
        let root = world.spawn(at(100.0));
        world.set_parent(crate_, Some(root)).expect("alive");
        // Two unit spheres 10 apart, under parents at 100: radius 6 between them.
        close(of(&world, &meshes, root), DVec3::new(105.0, 0.0, 0.0), 6.0);
        close(
            of(&world, &meshes, crate_),
            DVec3::new(105.0, 0.0, 0.0),
            6.0,
        );
        close(of(&world, &meshes, node), DVec3::new(100.0, 0.0, 0.0), 1.0);
    }

    #[test]
    fn sprites_and_circles_are_round_their_unit_squares() {
        let (mut world, meshes) = (World::default(), Meshes::default());
        let scaled = Transform {
            scale: Vec3::new(4.0, 2.0, 1.0),
            ..at(1.0)
        };
        let sprite = world.spawn((scaled, Sprite::default()));
        let circle = world.spawn((at(0.0), Circle::default()));
        let half_diagonal = SQUARE * 4.0;
        close(
            of(&world, &meshes, sprite),
            DVec3::X,
            f64::from(half_diagonal),
        );
        close(of(&world, &meshes, circle), DVec3::ZERO, f64::from(SQUARE));
    }

    #[test]
    fn a_box_hugs_a_scaled_sprite_and_holds_children_in_the_root_s_space() {
        let (mut world, mut meshes) = (World::default(), Meshes::default());
        let wall = Transform {
            scale: Vec3::new(20.0, 2.0, 1.0),
            ..at(1.0)
        };
        let wall = world.spawn((wall, Sprite::default()));
        let (placed, [low, high]) = boxed(&world, &meshes, wall).expect("a box");
        assert_eq!(
            [low, high],
            [DVec3::new(-0.5, -0.5, 0.0), DVec3::new(0.5, 0.5, 0.0)]
        );
        let corner = placed.transform_point3(high);
        assert!(
            corner.distance(DVec3::new(11.0, 1.0, 0.0)) < 1e-9,
            "{corner}"
        );
        // Along the world's axes, turned a right angle about Z: 2 across and 20 up.
        let world_corners = [DVec3::new(-9.0, -1.0, 0.0), DVec3::new(11.0, 1.0, 0.0)];
        assert_eq!(world_box(&world, &meshes, wall), Some(world_corners));
        let turned = Transform {
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::new(20.0, 2.0, 1.0),
            ..at(0.0)
        };
        let turned = world.spawn((turned, Sprite::default()));
        let [low, high] = world_box(&world, &meshes, turned).expect("a box");
        assert!(low.distance(DVec3::new(-1.0, -10.0, 0.0)) < 1e-5, "{low}");
        assert!(high.distance(DVec3::new(1.0, 10.0, 0.0)) < 1e-5, "{high}");
        // A child's mesh, its cube round its sphere, two units along the root's X.
        let child = world.spawn((at(2.0), mesh(&mut meshes, Vec3::ZERO, 1.0)));
        let root = world.spawn(at(100.0));
        world.set_parent(child, Some(root)).expect("alive");
        let (placed, [low, high]) = boxed(&world, &meshes, root).expect("a box");
        assert!(low.distance(DVec3::new(1.0, -1.0, -1.0)) < 1e-9, "{low}");
        assert!(high.distance(DVec3::new(3.0, 1.0, 1.0)) < 1e-9, "{high}");
        assert!(
            placed
                .transform_point3(low)
                .distance(DVec3::new(101.0, -1.0, -1.0))
                < 1e-9
        );
        let empty = world.spawn(at(0.0));
        assert_eq!(boxed(&world, &meshes, empty), None);
    }

    #[test]
    fn what_draws_nothing_known_has_no_sphere() {
        let (mut world, mut meshes) = (World::default(), Meshes::default());
        let empty = world.spawn(at(0.0));
        let child = world.spawn(at(1.0));
        world.set_parent(child, Some(empty)).expect("alive");
        assert_eq!(of(&world, &meshes, empty), None);
        // A mesh without a known extent is left out.
        let whole = (meshes.add("whole", &[[0.0_f32; 3]; 3], &[0, 1, 2])).expect("a triangle");
        let draw = MeshDraw::from_parts(whole.index(), 0, false, false, &[]).expect("none");
        world.insert(child, draw).expect("alive");
        assert_eq!(of(&world, &meshes, empty), None);
        // A despawned entity has none.
        world.despawn(empty).expect("alive");
        assert_eq!(of(&world, &meshes, empty), None);
    }

    #[test]
    fn two_spheres_merge_into_the_smallest_that_holds_both() {
        let a = (DVec3::ZERO, 1.0);
        let b = (DVec3::new(4.0, 0.0, 0.0), 1.0);
        close(Some(union(a, b)), DVec3::new(2.0, 0.0, 0.0), 3.0);
        // One inside the other gives the larger, in either order.
        let big = (DVec3::new(1.0, 0.0, 0.0), 5.0);
        for (x, y) in [(a, big), (big, a)] {
            close(Some(union(x, y)), big.0, big.1);
        }
        close(Some(union(a, a)), a.0, a.1);
    }
}
