//! The grid a view shows ([`Context::draw_grid`](crate::Context::draw_grid)).
//!
//! Lines a power of ten apart on a world axis plane, with the plane's axes colored, in either
//! projection. The renderer's grid shader draws it on a unit square that `quad` lays in the plane
//! each frame.

use gg_core::math::{DVec2, DVec3, Mat4, Vec3, libm};
use gg_renderer::{Grid, GridLines, Projection, Viewpoint, Viewport};

/// A world axis plane a grid lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// 2D games' plane; Front and Back face it.
    XY,
    /// The ground; Top and Bottom face it.
    XZ,
    /// Right and Left face it.
    YZ,
}

impl Plane {
    /// The axis square to it: Z, Y or X.
    pub fn normal(self) -> Vec3 {
        match self {
            Self::XY => Vec3::Z,
            Self::XZ => Vec3::Y,
            Self::YZ => Vec3::X,
        }
    }

    /// Its axes in name order, with their colors.
    fn axes(self) -> [(Vec3, Vec3); 2] {
        match self {
            Self::XY => [(Vec3::X, RED), (Vec3::Y, GREEN)],
            Self::XZ => [(Vec3::X, RED), (Vec3::Z, BLUE)],
            Self::YZ => [(Vec3::Y, GREEN), (Vec3::Z, BLUE)],
        }
    }
}

/// The world axes' colors, linear: X red, Y green, Z blue. Also the tools' handles and gizmo's.
pub const RED: Vec3 = Vec3::new(0.8, 0.1, 0.1);
pub const GREEN: Vec3 = Vec3::new(0.1, 0.7, 0.1);
pub const BLUE: Vec3 = Vec3::new(0.1, 0.3, 0.9);

/// How near camera forward F must be to a plane's normal n, as 1 − |n·F|, to face it along an
/// axis: within about a twelfth of a degree.
const ALONG_AXIS: f64 = 1e-6;

/// How squarely the camera must see the plane, as |n·v| for a direction v, for any of it to show:
/// where the shader's fade toward grazing angles reaches 0.
const GRAZING: f64 = 0.002;

/// The perspective quad's span in camera heights above the plane. Its nearest edges are 500
/// heights from the camera's foot, where |n·v| is just under `GRAZING`, so lines have faded
/// out before them.
const ACROSS: f64 = 1000.0;

/// The orthographic quad's span in last-frame view widths, so it covers the view in a frame that
/// widens it (each frame of a splitter's drag may): the projection keeps its height whatever the
/// view's size, and widens with it.
const SLACK: f64 = 2.0;

/// The least spacing, in pixels, of the finest lines that show, as in the grid shader.
const FINEST: f64 = 8.0;

/// The plane a grid shows for `pick` as `camera` sees it: the plane an orthographic view along an
/// axis faces (XY for Front and Back, XZ for Top and Bottom, YZ for Right and Left), else `pick`.
pub fn shown(camera: &Viewpoint, pick: Plane) -> Plane {
    match camera.projection {
        Projection::Orthographic { .. } => facing(camera).unwrap_or(pick),
        Projection::Perspective { .. } => pick,
    }
}

/// Whether `camera`'s basis lines up with `view`'s pixels, as an axis view's does.
///
/// It faces along an axis and its right lies within a quarter of a pixel, across the view, of one
/// of the facing plane's axes. A roll, or a yaw at the poles, fails it. A square orthographic camera draws
/// crisp grid lines, a pixel each.
pub fn square(camera: &Viewpoint, view: Viewport) -> bool {
    let Some(plane) = facing(camera) else {
        return false;
    };
    let right = camera.rotation.as_dquat() * DVec3::X;
    let [first, second] = plane
        .axes()
        .map(|(axis, _)| right.dot(axis.as_dvec3()).abs());
    // The longer side, along which a line turned with the image strays furthest.
    let across = f64::from(view.size[0].max(view.size[1]));
    first.min(second) * across < 0.25
}

/// The plane `camera` faces along an axis, if any.
fn facing(camera: &Viewpoint) -> Option<Plane> {
    let forward = camera.rotation.as_dquat() * DVec3::NEG_Z;
    let along = |plane: &Plane| plane.normal().as_dvec3().dot(forward).abs() > 1.0 - ALONG_AXIS;
    [Plane::XY, Plane::XZ, Plane::YZ].into_iter().find(along)
}

/// The grid's quad for `camera` seeing `plane` in `view`: the transform from the unit square to
/// the world from `origin` (the frame's), and the shader's data.
///
/// - Perspective: a square around the camera's foot, `ACROSS` times its height above the plane
///   across.
/// - Orthographic: the parallelogram where rays through the view's corners meet the plane,
///   stretched across to `SLACK` view widths.
///
/// `None` while the view has no area or the plane is so near edge-on that the fade toward
/// grazing angles hides all of it.
pub(crate) fn quad(
    camera: &Viewpoint,
    view: Viewport,
    plane: Plane,
    origin: DVec3,
) -> Option<(Mat4, Grid)> {
    if view.size.contains(&0) {
        return None;
    }
    let [width, height] = view.size.map(f64::from);
    let aspect = f64::from(view.aspect());
    let rotation = camera.rotation.as_dquat();
    let forward = rotation * DVec3::NEG_Z;
    let eye = camera.position;
    let normal = plane.normal().as_dvec3();
    let [(first, first_color), (second, second_color)] = plane.axes();
    let [first, second] = [first.as_dvec3(), second.as_dvec3()];
    // Each plane passes through the origin, so a point's height above it is n·point.
    let (center, edges, coarsest, lines) = match camera.projection {
        Projection::Perspective { fov, near } => {
            let above = normal.dot(eye);
            // Beyond the near plane, so seen at |n·v| of at most height over `near`.
            if above.abs() < GRAZING * f64::from(near) {
                return None;
            }
            let side = ACROSS * above.abs();
            // A plane coordinate changes per pixel by at least the least pixel angle times the
            // camera's height. That angle is at the view's corners: tan(fov / 2) over half the
            // height on the axis, times the squared cosine of the angle off it.
            let tan = libm::tan(f64::from(fov) / 2.0);
            let least = 2.0 * tan / height / (1.0 + tan * tan * (1.0 + aspect * aspect));
            // Four powers of ten coarser than the finest lines near the foot, where coordinates
            // within half of it keep lines within a fortieth of a pixel. Lines that far apart
            // crowd closer than the finest that show some hundred heights out and fade there,
            // before the quad's edge.
            let coarsest = level(above.abs() * least) + 4;
            let edges = [first * side, second * side];
            (
                eye - normal * above,
                edges,
                coarsest,
                GridLines::Perspective,
            )
        }
        Projection::Orthographic { height: tall, .. } => {
            // How squarely the camera looks at the plane, n·F.
            let aim = normal.dot(forward);
            if aim.abs() < GRAZING {
                return None;
            }
            // Along the view's axis onto the plane.
            let onto = |v: DVec3| v - forward * (normal.dot(v) / aim);
            let half = f64::from(tall) / 2.0;
            let across = onto(rotation * DVec3::X * (half * aspect));
            let up = onto(rotation * DVec3::Y * half);
            let center = onto(eye);
            // How far each plane coordinate moves from pixel to pixel, the same all over the view.
            let footprint = |axis: DVec3| {
                let pixel = DVec2::new(axis.dot(across) / width, axis.dot(up) / height);
                pixel.length() * 2.0
            };
            // A power of ten past the coarsest lines that show, so the shader's footprints,
            // which round differently, can't pass it where they pick the next power up.
            let coarsest = level(footprint(first).max(footprint(second))) + 3;
            // The GPU's `f32` matrices round by a few times the float precision at the camera's
            // and quad's distances from the frame's origin: a larger margin keeps the view
            // covered far from it.
            let from = (eye - origin).length() + (center - origin).length();
            let rounding = 16.0 * f64::from(f32::EPSILON) * from;
            let edges = [
                across * (2.0 * SLACK + 2.0 * rounding / (half * aspect)),
                up * (2.0 + 2.0 * rounding / half),
            ];
            let lines = if square(camera, view) {
                GridLines::Crisp
            } else {
                GridLines::Orthographic
            };
            (center, edges, coarsest, lines)
        }
    };
    // Where the GPU has it, from the frame's origin.
    let center = (center - origin).as_vec3();
    let exact = origin + center.as_dvec3();
    let modulus = libm::pow(10.0, f64::from(coarsest));
    // Within half the modulus of 0, which every spacing drawn divides, so lines stay where they'd
    // be from the plane's origin.
    let reduce = |along: f64| (along - modulus * (along / modulus).round()) as f32;
    let grid = Grid {
        first: first_color,
        first_center: reduce(exact.dot(first)),
        second: second_color,
        second_center: reduce(exact.dot(second)),
        eye: match lines {
            GridLines::Perspective => (eye - exact).as_vec3(),
            GridLines::Orthographic | GridLines::Crisp => forward.as_vec3(),
        },
        coarsest: coarsest as f32,
        normal: plane.normal(),
        lines,
    };
    let [a, b] = edges.map(|edge| edge.as_vec3().extend(0.0));
    let transform = Mat4::from_cols(a, b, plane.normal().extend(0.0), center.extend(1.0));
    Some((transform, grid))
}

/// The power of ten of the finest lines that show where a coordinate changes by `footprint` per
/// pixel, as the shader picks it: the least with lines `FINEST` or more pixels apart.
fn level(footprint: f64) -> i32 {
    libm::log10(footprint * FINEST).ceil() as i32
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_3, PI};

    use gg_core::math::{EulerRot, Quat, Vec2, Vec4};

    use super::*;

    const VIEW: Viewport = Viewport {
        origin: [0, 0],
        size: [1600, 827],
    };

    const CORNERS: [Vec2; 4] = [
        Vec2::new(-0.5, -0.5),
        Vec2::new(0.5, -0.5),
        Vec2::new(0.5, 0.5),
        Vec2::new(-0.5, 0.5),
    ];

    fn radians(degrees: f32) -> f32 {
        degrees * PI / 180.0
    }

    /// An editor orbit camera: turned by `yaw`, `pitch`, `roll` (degrees), `distance` back from
    /// `pivot`, 60° bottom to top in perspective, or as tall at the pivot orthographically.
    fn camera(
        pivot: Vec3,
        [yaw, pitch, roll]: [f32; 3],
        distance: f32,
        perspective: bool,
    ) -> Viewpoint {
        let [yaw, pitch, roll] = [yaw, pitch, roll].map(radians);
        let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
        let projection = if perspective {
            Projection::Perspective {
                fov: FRAC_PI_3,
                near: distance * 1e-4,
            }
        } else {
            let height = 2.0 * distance * libm::tanf(FRAC_PI_3 / 2.0);
            Projection::Orthographic {
                height,
                near: -1000.0 * height,
                far: 1000.0 * height,
            }
        };
        Viewpoint {
            position: (pivot - rotation * Vec3::NEG_Z * distance).as_dvec3(),
            rotation,
            projection,
        }
    }

    /// The editor's start view: front, over the origin, 10 units tall.
    fn start() -> Viewpoint {
        camera(
            Vec3::ZERO,
            [0.0; 3],
            5.0 / libm::tanf(FRAC_PI_3 / 2.0),
            false,
        )
    }

    /// The quad's corners in Vulkan's normalized device coordinates in [`VIEW`] made `wider`
    /// times as wide, placed as the shader does: center and corner offsets go through the
    /// camera's matrix apart.
    fn corners(camera: &Viewpoint, transform: Mat4, wider: f32) -> [Vec2; 4] {
        let clip = camera.view_projection(1600.0 * wider / 827.0);
        let center = clip * transform.w_axis;
        CORNERS.map(|corner| {
            let at = center + clip * (transform * corner.extend(0.0).extend(0.0));
            Vec2::new(at.x, at.y) / at.w
        })
    }

    #[test]
    fn an_orthographic_quad_covers_the_view_and_one_twice_as_wide() {
        let far = Vec3::new(12345.6, -9876.5, 0.0);
        for (camera, plane) in [
            (start(), Plane::XY),
            // Rolled, from above at an angle, nearly edge-on, far from the origin up close.
            (camera(Vec3::ZERO, [0.0, 0.0, 20.0], 8.0, false), Plane::XY),
            (
                camera(Vec3::ONE, [45.0, -35.0, 10.0], 30.0, false),
                Plane::XZ,
            ),
            (camera(Vec3::ZERO, [30.0, -0.2, 0.0], 8.0, false), Plane::XZ),
            (camera(far, [0.0; 3], 0.1, false), Plane::XY),
            (camera(far, [20.0, 30.0, 0.0], 1.0, false), Plane::XY),
        ] {
            let (transform, _) = quad(&camera, VIEW, plane, DVec3::ZERO).expect("the plane shows");
            // In the view, and in one twice as wide, as a frame may have widened it since.
            for wider in [1.0, 2.0] {
                let corners = corners(&camera, transform, wider);
                for (corner, at) in CORNERS.iter().zip(corners) {
                    // On the view's edge or past it, or a tenth of a pixel short at most, for
                    // rounding; Vulkan's Y points down.
                    let past = at * Vec2::new(corner.x.signum(), -corner.y.signum());
                    let short = Vec2::new(0.2 / (1600.0 * wider), 0.2 / 827.0);
                    assert!(
                        past.cmpge(Vec2::ONE - short).all(),
                        "{camera:?}, {wider} times as wide: {corners:?}"
                    );
                }
            }
            // Over the origin: exactly the view's height, twice its width.
            if camera == start() {
                let corners = corners(&camera, transform, 1.0);
                let reach = corners
                    .map(Vec2::abs)
                    .into_iter()
                    .fold(Vec2::ZERO, Vec2::max);
                assert!(reach.abs_diff_eq(Vec2::new(2.0, 1.0), 1e-5), "{corners:?}");
            }
        }
    }

    #[test]
    fn a_perspective_quad_lies_around_the_cameras_foot() {
        // From above at 30° and 60°, from below, and along the ground just over it.
        for (turn, distance) in [
            ([20.0, -30.0, 0.0], 10.0),
            ([20.0, -60.0, 0.0], 0.01),
            ([-70.0, 30.0, 0.0], 1e5),
            ([0.0, 0.0, 0.0], 3.0),
        ] {
            let mut camera = camera(Vec3::new(1.0, 0.0, -2.0), turn, distance, true);
            camera.position.y += 1e-3;
            let height = camera.position.y as f32;
            let (transform, grid) =
                quad(&camera, VIEW, Plane::XZ, DVec3::ZERO).expect("the plane shows");
            let foot = Vec3::new(camera.position.x as f32, 0.0, camera.position.z as f32);
            let side = 1000.0 * height.abs();
            let expected = Mat4::from_cols(
                Vec4::new(side, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, side, 0.0),
                Vec4::Y,
                foot.extend(1.0),
            );
            let close = transform.abs_diff_eq(expected, side * 1e-6 + foot.length() * 1e-6);
            assert!(close, "{turn:?}: {transform}, not {expected}");
            let eye = Vec3::new(0.0, height, 0.0);
            assert!(grid.eye.abs_diff_eq(eye, height.abs() * 1e-3), "{grid:?}");
            assert_eq!(grid.lines, GridLines::Perspective);
        }
    }

    #[test]
    fn nothing_draws_while_the_plane_is_edge_on() {
        // Orthographically, by facing: just inside the fade's zero, and just out.
        let seen_at = |facing: f32| {
            let pitch = -libm::asinf(facing) * 180.0 / PI;
            quad(
                &camera(Vec3::ZERO, [10.0, pitch, 0.0], 8.0, false),
                VIEW,
                Plane::XZ,
                DVec3::ZERO,
            )
        };
        assert!(seen_at(0.0019).is_none());
        assert!(seen_at(0.0021).is_some());
        // In perspective, by height: in the plane nothing shows; just over it the plane shows
        // toward the view's bottom though the camera looks along it.
        let mut level = camera(Vec3::ZERO, [10.0, 0.0, 0.0], 8.0, true);
        assert!(quad(&level, VIEW, Plane::XZ, DVec3::ZERO).is_none());
        // Twelve times the least height that shows past its near plane, 0.0008 away.
        level.position.y = 2e-5;
        let (transform, grid) =
            quad(&level, VIEW, Plane::XZ, DVec3::ZERO).expect("the plane shows");
        let finite = transform.is_finite() && grid.eye.is_finite();
        let centers = [grid.first_center, grid.second_center, grid.coarsest];
        assert!(
            finite && centers.iter().all(|at| at.is_finite()),
            "{grid:?}"
        );
    }

    #[test]
    fn lines_stay_in_place_far_from_the_origin() {
        let far = Vec3::new(12345.6, -9876.5, 0.0);
        for camera in [
            camera(far, [0.0; 3], 0.1, false),
            camera(far, [30.0, -20.0, 0.0], 0.1, true),
        ] {
            let (transform, grid) =
                quad(&camera, VIEW, Plane::XY, DVec3::ZERO).expect("the plane shows");
            let center = Vec2::new(grid.first_center, grid.second_center);
            for corner in [
                [0.0, 0.0],
                [1e-4, -3e-4],
                [0.003, 0.002],
                [-0.1, 0.4],
                [0.5, -0.5],
            ] {
                // The plane's coordinates as the shader finds them, against the point in f64.
                let offset = transform * Vec4::new(corner[0], corner[1], 0.0, 0.0);
                let shader = center + Vec2::new(offset.x, offset.y);
                let world = transform.w_axis.truncate().as_dvec3() + offset.truncate().as_dvec3();
                // A pixel's least footprint there: orthographically, the view's height over its
                // pixels; in perspective, the distance from the camera times the least pixel
                // angle, about 0.00054 at the corners.
                let pixel = match camera.projection {
                    Projection::Orthographic { height, .. } => f64::from(height) / 827.0,
                    Projection::Perspective { .. } => {
                        let above = f64::from(grid.eye.z.abs());
                        5e-4 * offset.truncate().as_dvec3().length().max(above)
                    }
                };
                for exponent in grid.coarsest as i32 - 4..=grid.coarsest as i32 {
                    let spacing = libm::pow(10.0, f64::from(exponent));
                    for (shader, world) in [(shader.x, world.x), (shader.y, world.y)] {
                        let gap = (f64::from(shader) - world).rem_euclid(spacing);
                        let gap = gap.min(spacing - gap);
                        assert!(
                            gap < pixel / 20.0,
                            "{corner:?}: {shader} is {gap} from {world} modulo {spacing}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lines_go_crisp_only_orthographically_with_a_square_basis() {
        let lines = |camera: &Viewpoint| {
            let quad = quad(camera, VIEW, shown(camera, Plane::XY), DVec3::ZERO);
            quad.map(|(_, grid)| grid.lines)
        };
        let ortho = |turn| camera(Vec3::ZERO, turn, 8.0, false);
        assert_eq!(lines(&start()), Some(GridLines::Crisp));
        assert_eq!(lines(&ortho([0.0, -90.0, 0.0])), Some(GridLines::Crisp));
        // Rolled 20°, and Top yawed 30° (it turns the image): anti-aliased.
        assert_eq!(
            lines(&ortho([0.0, 0.0, 20.0])),
            Some(GridLines::Orthographic)
        );
        let yawed = ortho([30.0, -90.0, 0.0]);
        assert_eq!(shown(&yawed, Plane::XY), Plane::XZ);
        assert!(!square(&yawed, VIEW));
        assert_eq!(lines(&yawed), Some(GridLines::Orthographic));
        // Square in perspective, but never crisp.
        let front = camera(Vec3::ZERO, [0.0; 3], 8.0, true);
        assert!(square(&front, VIEW));
        assert_eq!(lines(&front), Some(GridLines::Perspective));
        // A roll under a quarter pixel across the width is square; a little more isn't, nor in
        // a view as tall as that's wide.
        let quarter = 0.25 / 1600.0 * 180.0 / PI;
        assert!(square(&ortho([0.0, 0.0, quarter * 0.9]), VIEW));
        assert!(!square(&ortho([0.0, 0.0, quarter * 1.1]), VIEW));
        let tall = Viewport {
            origin: [0, 0],
            size: [827, 1600],
        };
        assert!(!square(&ortho([0.0, 0.0, quarter * 1.1]), tall));
    }

    #[test]
    fn an_orthographic_axis_view_shows_the_plane_facing_it_and_any_other_the_pick() {
        for (turn, facing) in [
            ([0.0, 0.0], Plane::XY),
            ([180.0, 0.0], Plane::XY),
            ([0.0, -90.0], Plane::XZ),
            ([0.0, 90.0], Plane::XZ),
            ([90.0, 0.0], Plane::YZ),
            ([-90.0, 0.0], Plane::YZ),
        ] {
            let [yaw, pitch] = turn;
            for pick in [Plane::XY, Plane::XZ, Plane::YZ] {
                let ortho = camera(Vec3::ONE, [yaw, pitch, 0.0], 8.0, false);
                assert_eq!(shown(&ortho, pick), facing, "{turn:?}");
                let perspective = camera(Vec3::ONE, [yaw, pitch, 0.0], 8.0, true);
                assert_eq!(shown(&perspective, pick), pick, "{turn:?}");
                // Five degrees off the axis (a yaw at the poles only turns the image).
                let off = [yaw + 5.0, pitch.clamp(-85.0, 85.0), 0.0];
                assert_eq!(
                    shown(&camera(Vec3::ONE, off, 8.0, false), pick),
                    pick,
                    "{turn:?}"
                );
            }
        }
    }
}
