//! The sphere round what an entity and every entity under it draw, which the tool API's `entity`
//! answers as `bounds` and the editor frames.

use gg_core::math::Vec3;
use gg_renderer::{Circle, MeshDraw, Meshes, Sprite};

use crate::{Entity, Transform, World};

/// A sphere: its center and radius.
pub(crate) type Sphere = (Vec3, f32);

/// The radius of the sphere round a sprite's or a circle's unit square, centered on its origin: the
/// square's half diagonal.
const SQUARE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The smallest sphere round what `root` and everything under it draws, in the world, as the latest
/// tick left them: each draw's own sphere through its entity's pose, merged. A mesh's is the one
/// `meshes` keeps; a sprite's and a circle's is round its unit square, so that an uneven scale is
/// covered by its largest axis's. Text, whose extent only laying it out gives, particles and lines
/// are left out, and so is a mesh without a finite sphere. `None` if nothing there draws, or
/// `root` isn't alive.
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
        let scale = [pose.x_axis, pose.y_axis, pose.z_axis]
            .map(|axis| axis.truncate().length())
            .into_iter()
            .fold(0.0, f32::max);
        for (center, radius) in own.into_iter().flatten() {
            let placed = (pose.transform_point3(center), radius * scale);
            if placed.0.is_finite() && placed.1.is_finite() {
                bounds = Some(bounds.map_or(placed, |bounds| union(bounds, placed)));
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

    /// A mesh added to `meshes` whose sphere is `radius` round `center`, drawn by a component.
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
            translation: Vec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    fn close(sphere: Option<Sphere>, center: Vec3, radius: f32) {
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
            translation: Vec3::new(0.0, 10.0, 0.0),
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::new(2.0, 3.0, 1.0),
        };
        let entity = world.spawn((turned, draw));
        // (1, 0, 0) scaled to (2, 0, 0), turned a quarter about Z to (0, 2, 0), then moved up 10;
        // the radius is scaled by the largest scale.
        close(of(&world, &meshes, entity), Vec3::new(0.0, 12.0, 0.0), 6.0);
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
        // Two unit spheres 10 apart, under parents at 100: one of radius 6 between them.
        close(of(&world, &meshes, root), Vec3::new(105.0, 0.0, 0.0), 6.0);
        close(of(&world, &meshes, crate_), Vec3::new(105.0, 0.0, 0.0), 6.0);
        close(of(&world, &meshes, node), Vec3::new(100.0, 0.0, 0.0), 1.0);
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
        close(of(&world, &meshes, sprite), Vec3::X, half_diagonal);
        close(of(&world, &meshes, circle), Vec3::ZERO, SQUARE);
    }

    #[test]
    fn what_draws_nothing_known_has_no_sphere() {
        let (mut world, mut meshes) = (World::default(), Meshes::default());
        let empty = world.spawn(at(0.0));
        let child = world.spawn(at(1.0));
        world.set_parent(child, Some(empty)).expect("alive");
        assert_eq!(of(&world, &meshes, empty), None);
        // A mesh whose extent isn't known is left out.
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
        let a = (Vec3::ZERO, 1.0);
        let b = (Vec3::new(4.0, 0.0, 0.0), 1.0);
        close(Some(union(a, b)), Vec3::new(2.0, 0.0, 0.0), 3.0);
        // One inside the other gives the larger, whichever way round.
        let big = (Vec3::new(1.0, 0.0, 0.0), 5.0);
        for (x, y) in [(a, big), (big, a)] {
            close(Some(union(x, y)), big.0, big.1);
        }
        close(Some(union(a, a)), a.0, a.1);
    }
}
