//! The grid the editor's view shows: lines a power of ten apart on one of the world's axis planes,
//! and the plane's two axes in their colors, in either projection. `shaders/grid.slang` draws it on
//! a unit square that [`quad`] lays in the plane each frame.

use std::mem::offset_of;
use std::path::Path;

use bytemuck::NoUninit;
use gg_engine::math::{DVec2, DVec3, Mat4, Vec2, Vec3, libm};
use gg_engine::{Blend, Camera, Context, Mesh, Projection, Shader, Viewport};

/// One of the world's axis planes, which the grid lies in: what the toolbar picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// The plane 2D games are in, which the Front and Back views face.
    XY,
    /// The ground, which the Top and Bottom views face.
    XZ,
    /// The plane the Right and Left views face.
    YZ,
}

impl Plane {
    /// The axis it's square to: Z, Y or X.
    pub fn normal(self) -> Vec3 {
        match self {
            Self::XY => Vec3::Z,
            Self::XZ => Vec3::Y,
            Self::YZ => Vec3::X,
        }
    }

    /// Its two axes, in the order its name gives them, each with its color.
    fn axes(self) -> [(Vec3, Vec3); 2] {
        match self {
            Self::XY => [(Vec3::X, RED), (Vec3::Y, GREEN)],
            Self::XZ => [(Vec3::X, RED), (Vec3::Z, BLUE)],
            Self::YZ => [(Vec3::Y, GREEN), (Vec3::Z, BLUE)],
        }
    }
}

/// The colors of the world's axes, linear: X red, Y green and Z blue, as the gizmo shows them too.
pub const RED: Vec3 = Vec3::new(0.8, 0.1, 0.1);
pub const GREEN: Vec3 = Vec3::new(0.1, 0.7, 0.1);
pub const BLUE: Vec3 = Vec3::new(0.1, 0.3, 0.9);

/// How near a camera's forward F must be to a plane's normal n, as 1 − |n·F|, for it to face the
/// plane along an axis: within about a twelfth of a degree.
const ALONG_AXIS: f64 = 1e-6;

/// How squarely the camera must see some of the plane, as |n·v| for a direction v it's seen in,
/// for any of it to show: where `shaders/grid.slang`'s fade toward grazing angles reaches 0.
const GRAZING: f64 = 0.002;

/// How many times the camera's height above the plane the quad spans in perspective. Its nearest
/// edges are then 500 heights from the camera's foot, where |n·v| is just under `GRAZING`, so the
/// lines have faded out before them.
const ACROSS: f64 = 1000.0;

/// How many times the width of the view the last frame showed the orthographic quad spans, so that
/// it still covers the view in a frame that widens it, as each frame of a splitter's drag across it
/// may: the projection keeps its height whatever the view's size, and widens with the view.
const SLACK: f64 = 2.0;

/// How far apart, in pixels, the finest lines that show are at least, as `shaders/grid.slang` has
/// it.
const FINEST: f64 = 8.0;

/// The unit square, about its center: its center, then its corners counterclockwise.
const SQUARE: [Vec2; 5] = [
    Vec2::ZERO,
    Vec2::new(-0.5, -0.5),
    Vec2::new(0.5, -0.5),
    Vec2::new(0.5, 0.5),
    Vec2::new(-0.5, 0.5),
];

/// The square as four triangles from its center, where perspective puts the camera's foot: the
/// lines are finest there, and the plane's coordinates are interpolated most precisely near a
/// vertex.
const FAN: [u32; 12] = [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1];

/// The grid's mesh and shader, which draw it in a plane each frame.
pub struct Grid {
    mesh: Mesh<Vec2>,
    shader: Shader<Vec2, Params>,
}

impl Grid {
    /// Adds the grid's mesh and shader.
    ///
    /// # Panics
    ///
    /// If the shader doesn't compile, or breaks the shader contract, with what went wrong: without
    /// it the editor's view has no grid.
    pub fn new(cx: &mut Context) -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders/grid.slang");
        let shader = cx.add_shader("grid", path, Blend::Alpha);
        let mesh = cx.add_mesh("grid", &SQUARE, &FAN);
        Self {
            mesh: mesh.expect("the square is four whole triangles"),
            shader: shader.expect("the grid shader compiles"),
        }
    }

    /// Draws the grid this frame as `camera` sees it in `view`, in the plane `pick` unless an
    /// orthographic view along an axis faces another, as [`shown`] says: `camera` is the one the
    /// frame renders with, and `view` where [`Context::view`] says the view is.
    pub fn draw(&self, cx: &mut Context, camera: &Camera, view: Viewport, pick: Plane) {
        if let Some((transform, params)) = quad(camera, view, shown(camera, pick)) {
            cx.draw(self.mesh, self.shader, transform, params);
        }
    }
}

/// The plane the grid shows while the toolbar picks `pick`: the one an orthographic view along an
/// axis faces, XY for Front and Back, XZ for Top and Bottom and YZ for Right and Left; otherwise the
/// pick.
pub fn shown(camera: &Camera, pick: Plane) -> Plane {
    match camera.projection {
        Projection::Orthographic { .. } => facing(camera).unwrap_or(pick),
        Projection::Perspective { .. } => pick,
    }
}

/// Whether `camera`'s basis lines up with the pixels of `view`, as an axis view's does: it faces
/// along an axis, and its right lies within a quarter of a pixel, across the view, of one of the
/// facing plane's two axes. Rolling, or a yaw at the poles, turns the image and fails it. An
/// orthographic camera that's square draws the grid's lines crisp, a pixel each.
pub fn square(camera: &Camera, view: Viewport) -> bool {
    let Some(plane) = facing(camera) else {
        return false;
    };
    let right = camera.rotation.as_dquat() * DVec3::X;
    let [first, second] = plane
        .axes()
        .map(|(axis, _)| right.dot(axis.as_dvec3()).abs());
    // Across the view's longer side, along which a line turned with the image strays furthest.
    let across = f64::from(view.size[0].max(view.size[1]));
    first.min(second) * across < 0.25
}

/// The plane whose normal `camera` looks along, if it looks along an axis.
fn facing(camera: &Camera) -> Option<Plane> {
    let forward = camera.rotation.as_dquat() * DVec3::NEG_Z;
    let along = |plane: &Plane| plane.normal().as_dvec3().dot(forward).abs() > 1.0 - ALONG_AXIS;
    [Plane::XY, Plane::XZ, Plane::YZ].into_iter().find(along)
}

/// What the grid's shader gets with its quad, which `Grid` in `shaders/grid.slang` matches byte for
/// byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
struct Params {
    /// The color of the plane's first axis: X in XY and XZ, Y in YZ.
    first: Vec3,
    /// Where the quad's center is along the first axis, modulo the coarsest spacing drawn, and
    /// within half of it of 0.
    first_center: f32,
    /// The color of its second axis: Y in XY, Z in XZ and YZ.
    second: Vec3,
    /// Where the quad's center is along the second axis, as `first_center` is along the first.
    second_center: f32,
    /// In perspective, where the camera is from the quad's center; orthographically, the way it
    /// faces.
    eye: Vec3,
    /// The power of ten that the coarsest spacing drawn is, which the center is taken modulo.
    coarsest: f32,
    /// The plane's normal.
    normal: Vec3,
    /// How the shader draws the lines.
    lines: Lines,
}

// Each `Vec3` begins a row of 16 bytes, which a 4-byte field ends, so that `Params` is laid out
// alike in C, as Slang lays out what an address points at, and in std430, which aligns a float3 to
// 16 bytes.
const _: () = {
    assert!(offset_of!(Params, second) == 16);
    assert!(offset_of!(Params, eye) == 32);
    assert!(offset_of!(Params, normal) == 48);
    assert!(size_of::<Params>() == 64);
};

/// How the grid's shader draws the lines, as the camera sees the plane.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, NoUninit)]
enum Lines {
    /// Anti-aliased, through perspective from a point.
    Perspective,
    /// Anti-aliased, orthographically along a direction.
    Orthographic,
    /// Crisp, a pixel each, orthographically, for a camera that's [`square`].
    Crisp,
}

/// Where the grid's quad goes, as the transform from the unit square to the world, and what its
/// shader gets, for `camera` seeing `plane` in `view`: in perspective, a square around the camera's
/// foot on the plane, `ACROSS` times its height above the plane across; orthographically, the
/// parallelogram where the rays through the view's corners meet the plane, stretched across to
/// `SLACK` times the view's width. `None` while the plane is too near edge-on for any of it to show
/// through the fade toward grazing angles.
fn quad(camera: &Camera, view: Viewport, plane: Plane) -> Option<(Mat4, Params)> {
    if view.size.contains(&0) {
        return None;
    }
    let [width, height] = view.size.map(f64::from);
    // As the renderer works it out.
    let aspect = f64::from(view.size[0] as f32 / view.size[1] as f32);
    let rotation = camera.rotation.as_dquat();
    let forward = rotation * DVec3::NEG_Z;
    let eye = camera.position.as_dvec3();
    let normal = plane.normal().as_dvec3();
    let [(first, first_color), (second, second_color)] = plane.axes();
    let [first, second] = [first.as_dvec3(), second.as_dvec3()];
    // Each plane passes through the origin, so a point's height above it is its normal's dot
    // product with the point.
    let (center, edges, coarsest, lines) = match camera.projection {
        Projection::Perspective { fov, near } => {
            let above = normal.dot(eye);
            // What's beyond the near plane is at least `near` from the camera, and so seen at an
            // |n·v| of at most its height over `near`.
            if above.abs() < GRAZING * f64::from(near) {
                return None;
            }
            let side = ACROSS * above.abs();
            // Each of the plane's coordinates changes from one pixel to the next by at least the
            // least angle a pixel spans times its distance, and so times the camera's height. The
            // least angle is at the view's corners: the view's axis, where it's tan(fov / 2) over
            // half the view's height, times the square of the cosine of the angle off it.
            let tan = libm::tan(f64::from(fov) / 2.0);
            let least = 2.0 * tan / height / (1.0 + tan * tan * (1.0 + aspect * aspect));
            // Four powers of ten coarser than the finest lines can be, near the camera's foot,
            // where coordinates within half of it keep the lines within a fortieth of a pixel.
            // Lines that far apart crowd closer than the finest that show some hundred heights
            // from the foot, and fade out there, before the quad's edge.
            let coarsest = level(above.abs() * least) + 4;
            let edges = [first * side, second * side];
            (eye - normal * above, edges, coarsest, Lines::Perspective)
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
            // How far each of the plane's coordinates goes from one pixel to the next, the same
            // all over the view.
            let footprint = |axis: DVec3| {
                let pixel = DVec2::new(axis.dot(across) / width, axis.dot(up) / height);
                pixel.length() * 2.0
            };
            // A power of ten past the coarsest lines that show, so that the shader's footprints,
            // which round apart from these, can't pass it where they pick the next power up.
            let coarsest = level(footprint(first).max(footprint(second))) + 3;
            // The GPU places the quad through `f32` matrices, which round by a few times a
            // float's precision at the camera's and the quad's distances from the origin: a margin
            // of more than that keeps the view covered far from the origin.
            let rounding = 16.0 * f64::from(f32::EPSILON) * (eye.length() + center.length());
            let edges = [
                across * (2.0 * SLACK + 2.0 * rounding / (half * aspect)),
                up * (2.0 + 2.0 * rounding / half),
            ];
            let lines = if square(camera, view) {
                Lines::Crisp
            } else {
                Lines::Orthographic
            };
            (center, edges, coarsest, lines)
        }
    };
    // Where the GPU has it.
    let center = center.as_vec3();
    let exact = center.as_dvec3();
    let modulus = libm::pow(10.0, f64::from(coarsest));
    // Within half the modulus of 0, which every spacing drawn divides, so that the lines are where
    // they'd be from the plane's origin.
    let reduce = |along: f64| (along - modulus * (along / modulus).round()) as f32;
    let params = Params {
        first: first_color,
        first_center: reduce(exact.dot(first)),
        second: second_color,
        second_center: reduce(exact.dot(second)),
        eye: match lines {
            Lines::Perspective => (eye - exact).as_vec3(),
            Lines::Orthographic | Lines::Crisp => forward.as_vec3(),
        },
        coarsest: coarsest as f32,
        normal: plane.normal(),
        lines,
    };
    let [a, b] = edges.map(|edge| edge.as_vec3().extend(0.0));
    let transform = Mat4::from_cols(a, b, plane.normal().extend(0.0), center.extend(1.0));
    Some((transform, params))
}

/// The power of ten whose lines are the finest that show where a coordinate changes by `footprint`
/// from one pixel to the next, as the shader picks it: the least at least `FINEST` pixels apart.
fn level(footprint: f64) -> i32 {
    libm::log10(footprint * FINEST).ceil() as i32
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_3, PI};

    use gg_engine::math::{EulerRot, Quat, Vec4};

    use super::*;

    /// The view of the editor's first run on this machine, in pixels.
    const VIEW: Viewport = Viewport {
        origin: [0, 0],
        size: [1600, 827],
    };

    fn radians(degrees: f32) -> f32 {
        degrees * PI / 180.0
    }

    /// A camera as the editor's orbit camera places it: turned by `yaw`, `pitch` and `roll`, in
    /// degrees, `distance` back from `pivot`, seeing 60° from the bottom of the view to its top in
    /// perspective, or orthographically as much as that shows at the pivot.
    fn camera(
        pivot: Vec3,
        [yaw, pitch, roll]: [f32; 3],
        distance: f32,
        perspective: bool,
    ) -> Camera {
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
        Camera {
            position: pivot - rotation * Vec3::NEG_Z * distance,
            rotation,
            projection,
        }
    }

    /// The editor's start view: from the front, over the origin, 10 units tall.
    fn start() -> Camera {
        camera(
            Vec3::ZERO,
            [0.0; 3],
            5.0 / libm::tanf(FRAC_PI_3 / 2.0),
            false,
        )
    }

    /// The quad's corners in Vulkan's normalized device coordinates, as the shader places them in
    /// [`VIEW`] made `wider` times as wide: the quad's center and each corner's offset from it
    /// through the camera's matrix apart.
    fn corners(camera: &Camera, transform: Mat4, wider: f32) -> [Vec2; 4] {
        let clip = camera.view_projection(1600.0 * wider / 827.0);
        let center = clip * transform.w_axis;
        [1, 2, 3, 4].map(|corner| {
            let at = center + clip * (transform * SQUARE[corner].extend(0.0).extend(0.0));
            Vec2::new(at.x, at.y) / at.w
        })
    }

    #[test]
    fn the_grid_shader_compiles_and_keeps_to_the_contract() {
        Grid::new(&mut Context::seeded(0));
    }

    #[test]
    fn an_orthographic_quad_covers_the_view_and_one_twice_as_wide() {
        let far = Vec3::new(12345.6, -9876.5, 0.0);
        for (camera, plane) in [
            (start(), Plane::XY),
            // Rolled, from above at an angle, nearly edge-on, and far from the origin up close.
            (camera(Vec3::ZERO, [0.0, 0.0, 20.0], 8.0, false), Plane::XY),
            (
                camera(Vec3::ONE, [45.0, -35.0, 10.0], 30.0, false),
                Plane::XZ,
            ),
            (camera(Vec3::ZERO, [30.0, -0.2, 0.0], 8.0, false), Plane::XZ),
            (camera(far, [0.0; 3], 0.1, false), Plane::XY),
            (camera(far, [20.0, 30.0, 0.0], 1.0, false), Plane::XY),
        ] {
            let (transform, _) = quad(&camera, VIEW, plane).expect("the plane shows");
            // In the view, and in one that's widened to twice its width since, as the view may
            // have in the frame the quad is drawn in.
            for wider in [1.0, 2.0] {
                let corners = corners(&camera, transform, wider);
                for (corner, at) in SQUARE[1..].iter().zip(corners) {
                    // Each corner of the quad on the view's or past it, by less than a tenth of a
                    // pixel short of it at most, as rounding has it; Vulkan's Y points down.
                    let past = at * Vec2::new(corner.x.signum(), -corner.y.signum());
                    let short = Vec2::new(0.2 / (1600.0 * wider), 0.2 / 827.0);
                    assert!(
                        past.cmpge(Vec2::ONE - short).all(),
                        "{camera:?}, {wider} times as wide: {corners:?}"
                    );
                }
            }
            // Over the origin, exactly the view's height, and twice its width.
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
        // From above at 30° and 60°, from below, and looking along the ground just over it, which
        // it sees toward the bottom of the view.
        for (turn, distance) in [
            ([20.0, -30.0, 0.0], 10.0),
            ([20.0, -60.0, 0.0], 0.01),
            ([-70.0, 30.0, 0.0], 1e5),
            ([0.0, 0.0, 0.0], 3.0),
        ] {
            let mut camera = camera(Vec3::new(1.0, 0.0, -2.0), turn, distance, true);
            camera.position.y += 1e-3;
            let height = camera.position.y;
            let (transform, params) = quad(&camera, VIEW, Plane::XZ).expect("the plane shows");
            let foot = Vec3::new(camera.position.x, 0.0, camera.position.z);
            let side = 1000.0 * height.abs();
            let expected = Mat4::from_cols(
                Vec4::new(side, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, side, 0.0),
                Vec4::Y,
                foot.extend(1.0),
            );
            let close = transform.abs_diff_eq(expected, side * 1e-6 + foot.length() * 1e-6);
            assert!(close, "{turn:?}: {transform}, not {expected}");
            // The camera straight up from the quad's center.
            let eye = Vec3::new(0.0, height, 0.0);
            assert!(
                params.eye.abs_diff_eq(eye, height.abs() * 1e-3),
                "{params:?}"
            );
            assert_eq!(params.lines, Lines::Perspective);
        }
    }

    #[test]
    fn nothing_draws_while_the_plane_is_edge_on() {
        // Orthographically by the way the camera faces: just inside the fade's zero, and just out.
        let seen_at = |facing: f32| {
            let pitch = -libm::asinf(facing) * 180.0 / PI;
            quad(
                &camera(Vec3::ZERO, [10.0, pitch, 0.0], 8.0, false),
                VIEW,
                Plane::XZ,
            )
        };
        assert!(seen_at(0.0019).is_none());
        assert!(seen_at(0.0021).is_some());
        // In perspective by the camera's height above it: in the plane nothing shows, but just over
        // it the plane shows toward the bottom of the view, though the camera looks along it.
        let mut level = camera(Vec3::ZERO, [10.0, 0.0, 0.0], 8.0, true);
        assert!(quad(&level, VIEW, Plane::XZ).is_none());
        // Twelve times the least height that shows past its near plane, 0.0008 away.
        level.position.y = 2e-5;
        let (transform, params) = quad(&level, VIEW, Plane::XZ).expect("the plane shows");
        let finite = transform.is_finite() && params.eye.is_finite();
        let centers = [params.first_center, params.second_center, params.coarsest];
        assert!(
            finite && centers.iter().all(|at| at.is_finite()),
            "{params:?}"
        );
    }

    #[test]
    fn lines_stay_in_place_far_from_the_origin() {
        let far = Vec3::new(12345.6, -9876.5, 0.0);
        // Facing the plane orthographically, up close; and in perspective at an angle.
        for camera in [
            camera(far, [0.0; 3], 0.1, false),
            camera(far, [30.0, -20.0, 0.0], 0.1, true),
        ] {
            let (transform, params) = quad(&camera, VIEW, Plane::XY).expect("the plane shows");
            let center = Vec2::new(params.first_center, params.second_center);
            for corner in [
                [0.0, 0.0],
                [1e-4, -3e-4],
                [0.003, 0.002],
                [-0.1, 0.4],
                [0.5, -0.5],
            ] {
                // As the shader finds the plane's coordinates, against where the point is, in f64.
                let offset = transform * Vec4::new(corner[0], corner[1], 0.0, 0.0);
                let shader = center + Vec2::new(offset.x, offset.y);
                let world = transform.w_axis.truncate().as_dvec3() + offset.truncate().as_dvec3();
                // A pixel's least footprint there: the view's height over its pixels
                // orthographically; in perspective, at least the distance from the camera times
                // the least angle a pixel of the view spans, about 0.00054 at its corners.
                let pixel = match camera.projection {
                    Projection::Orthographic { height, .. } => f64::from(height) / 827.0,
                    Projection::Perspective { .. } => {
                        let above = f64::from(params.eye.z.abs());
                        5e-4 * offset.truncate().as_dvec3().length().max(above)
                    }
                };
                for exponent in params.coarsest as i32 - 4..=params.coarsest as i32 {
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
        let lines = |camera: &Camera| {
            let quad = quad(camera, VIEW, shown(camera, Plane::XY));
            quad.map(|(_, params)| params.lines)
        };
        let ortho = |turn| camera(Vec3::ZERO, turn, 8.0, false);
        // The start view, and Top, which faces XZ.
        assert_eq!(lines(&start()), Some(Lines::Crisp));
        assert_eq!(lines(&ortho([0.0, -90.0, 0.0])), Some(Lines::Crisp));
        // Rolled 20°, and Top yawed 30°, which turns the image about the pole: anti-aliased.
        assert_eq!(lines(&ortho([0.0, 0.0, 20.0])), Some(Lines::Orthographic));
        let yawed = ortho([30.0, -90.0, 0.0]);
        assert_eq!(shown(&yawed, Plane::XY), Plane::XZ);
        assert!(!square(&yawed, VIEW));
        assert_eq!(lines(&yawed), Some(Lines::Orthographic));
        // Square in perspective too, but never crisp.
        let front = camera(Vec3::ZERO, [0.0; 3], 8.0, true);
        assert!(square(&front, VIEW));
        assert_eq!(lines(&front), Some(Lines::Perspective));
        // A roll that strays less than a quarter of a pixel across the view's width is square; a
        // little more isn't, nor in a view as tall as that's wide, whose lines stray furthest
        // down it.
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
        // Front, Back, Top, Bottom, Right and Left.
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
                // Five degrees off the axis: a yaw at the poles only turns the image.
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
