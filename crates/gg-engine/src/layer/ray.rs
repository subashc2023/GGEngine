//! A camera's ray through a window pixel.

use gg_core::math::{DVec3, Quat, Vec3, libm};
use gg_renderer::{Projection, Viewpoint, Viewport};

use super::Context;
use crate::Cast;

impl Context {
    /// The ray from the view's camera through `pixel`, as a [`Cast`] for
    /// [`World::cast_ray`](crate::World::cast_ray) or [`Cast::meets_plane`]: the cursor as a point
    /// in the world. Unlimited, through every collider.
    ///
    /// - `pixel` is in the main window's physical pixels from its top left, as
    ///   [`Input::cursor`](crate::Input::cursor) and
    ///   [`Event::CursorMoved`](crate::Event::CursorMoved) give.
    /// - It reads the viewpoint and [`view`](Self::view) the last frame handed off drew with, so a
    ///   viewpoint set since, or a camera moved by a tick since, shows from the next frame. In a
    ///   development build the view may be a panel of the editor's window, or seen from the look's
    ///   viewpoint, as drawn.
    /// - A perspective ray starts at the camera and goes through the pixel. An orthographic one
    ///   starts at the pixel's point on the near plane, going the way the camera faces.
    ///
    /// `None` outside the view (`pixel` not a number among them), before a frame has drawn, and
    /// in replays.
    pub fn ray(&self, pixel: [f64; 2]) -> Option<Cast> {
        through(&self.drawn?, self.view?, pixel)
    }
}

/// The ray from `viewpoint` through `pixel` of a view at `view` in the window.
fn through(viewpoint: &Viewpoint, view: Viewport, pixel: [f64; 2]) -> Option<Cast> {
    let [left, top] = view.origin.map(f64::from);
    let [width, height] = view.size.map(f64::from);
    let [x, y] = [pixel[0] - left, pixel[1] - top];
    // Not a NaN, which no range contains.
    let inside = (0.0..width).contains(&x) && (0.0..height).contains(&y);
    if !inside || !viewpoint.projection.projects() {
        return None;
    }
    // From -1 at the view's left and bottom to 1 at its right and top.
    let across = (x / width * 2.0 - 1.0) as f32;
    let up = (1.0 - y / height * 2.0) as f32;
    let aspect = view.aspect();
    let (from, toward) = match viewpoint.projection {
        Projection::Perspective { fov, .. } => {
            let half = libm::tanf(fov * 0.5);
            let toward = Vec3::new(across * half * aspect, up * half, -1.0);
            (viewpoint.position, toward)
        }
        Projection::Orthographic { height, near, far } => {
            let half = height * 0.5;
            let on_plane = Vec3::new(across * half * aspect, up * half, -near.min(far));
            let from = viewpoint.position + rotated(viewpoint.rotation, on_plane);
            (from, Vec3::NEG_Z)
        }
    };
    Some(Cast::new(from, viewpoint.rotation * toward))
}

/// `vector` turned by `rotation`, in `f64`.
fn rotated(rotation: Quat, vector: Vec3) -> DVec3 {
    (rotation * vector).as_dvec3()
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec3;

    use super::*;

    fn view(origin: [u32; 2], size: [u32; 2]) -> Viewport {
        Viewport { origin, size }
    }

    fn perspective(position: DVec3, rotation: Quat) -> Viewpoint {
        Viewpoint {
            position,
            rotation,
            projection: Projection::Perspective {
                fov: std::f32::consts::FRAC_PI_2,
                near: 0.1,
            },
        }
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-5)
    }

    /// The middle of a perspective view goes straight ahead from the camera; its edges go as far
    /// aside as the field of view says, wider across by the aspect ratio.
    #[test]
    fn perspective_rays_start_at_the_camera_through_the_pixel() {
        let eye = DVec3::new(1.0, 2.0, 3.0);
        let seen = perspective(eye, Quat::IDENTITY);
        let window = view([0, 0], [200, 100]);
        let middle = through(&seen, window, [100.0, 50.0]).expect("the middle");
        assert_eq!(middle.from, eye);
        assert!(close(middle.toward.normalize(), Vec3::NEG_Z), "{middle:?}");
        assert_eq!(middle.max_distance, f32::INFINITY);
        // A right-angle field of view: tan 45 degrees is 1 at the top, 2 at the right.
        let corner = through(&seen, window, [0.0, 0.0]).expect("top left");
        let want = Vec3::new(-2.0, 1.0, -1.0).normalize();
        assert!(close(corner.toward.normalize(), want), "{corner:?}");
        // Turned a quarter left about Y, it looks down -X.
        let left = perspective(eye, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let ahead = through(&left, window, [100.0, 50.0]).expect("the middle");
        assert!(close(ahead.toward.normalize(), Vec3::NEG_X), "{ahead:?}");
    }

    /// An orthographic view's rays are parallel, from the pixel's point on the near plane.
    #[test]
    fn orthographic_rays_start_at_the_pixel_on_the_near_plane() {
        let seen = Viewpoint {
            position: DVec3::new(10.0, 20.0, 30.0),
            rotation: Quat::IDENTITY,
            projection: Projection::Orthographic {
                height: 4.0,
                near: -1.0,
                far: 1.0,
            },
        };
        let window = view([0, 0], [200, 100]);
        let middle = through(&seen, window, [100.0, 50.0]).expect("the middle");
        // The near plane of -1 is a unit behind the camera.
        assert_eq!(middle.from, DVec3::new(10.0, 20.0, 31.0));
        assert_eq!(middle.toward, Vec3::NEG_Z);
        // Four units from bottom to top, eight across.
        let corner = through(&seen, window, [200.0 - 1e-9, 0.0]).expect("top right");
        assert!(
            corner.from.abs_diff_eq(DVec3::new(14.0, 22.0, 31.0), 1e-6),
            "{corner:?}"
        );
        assert_eq!(corner.toward, Vec3::NEG_Z);
        let ground = middle.meets_plane(DVec3::new(0.0, 0.0, 5.0), Vec3::Z);
        assert_eq!(ground, Some(DVec3::new(10.0, 20.0, 5.0)));
        // Turned a quarter about X to look down, +Y up the screen is -Z in the world.
        let down = Viewpoint {
            rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            ..seen
        };
        let top = through(&down, window, [100.0, 0.0]).expect("the top");
        assert!(close(top.toward, Vec3::NEG_Y), "{top:?}");
        let hit = top.meets_plane(DVec3::ZERO, Vec3::Y).expect("the ground");
        assert!(hit.abs_diff_eq(DVec3::new(10.0, 0.0, 28.0), 1e-4), "{hit}");
    }

    /// A view offset from the window's corner, as a dock panel is, reads pixels from its own
    /// corner; those outside it, or not numbers, have no ray.
    #[test]
    fn a_view_offset_in_the_window_reads_its_own_pixels() {
        let seen = perspective(DVec3::ZERO, Quat::IDENTITY);
        let panel = view([300, 40], [200, 100]);
        let middle = through(&seen, panel, [400.0, 90.0]).expect("the middle");
        assert!(close(middle.toward.normalize(), Vec3::NEG_Z), "{middle:?}");
        let corner = through(&seen, panel, [300.0, 40.0]).expect("its top left");
        assert!(corner.toward.x < 0.0 && corner.toward.y > 0.0, "{corner:?}");
        for outside in [
            [299.9, 90.0],
            [500.0, 90.0],
            [400.0, 39.9],
            [400.0, 140.0],
            [5.0, 5.0],
            [f64::NAN, 90.0],
        ] {
            assert_eq!(through(&seen, panel, outside), None, "{outside:?}");
        }
    }

    /// A projection that can't project has no ray, rather than a bad one.
    #[test]
    fn a_projection_that_cannot_project_has_no_ray() {
        let seen = Viewpoint {
            projection: Projection::Perspective {
                fov: 0.0,
                near: 0.1,
            },
            ..Viewpoint::default()
        };
        assert_eq!(through(&seen, view([0, 0], [10, 10]), [5.0, 5.0]), None);
    }

    /// A context's ray reads the view and viewpoint the last frame handed off, and has none before.
    #[test]
    fn a_context_rays_through_the_last_frame_drawn() {
        let mut cx = Context::seeded(1);
        assert_eq!(cx.ray([5.0, 5.0]), None);
        cx.view = Some(view([0, 0], [10, 10]));
        assert_eq!(cx.ray([5.0, 5.0]), None);
        cx.drawn = Some(perspective(DVec3::new(0.0, 5.0, 0.0), Quat::IDENTITY));
        let ray = cx.ray([5.0, 5.0]).expect("the middle");
        assert_eq!(ray.from, DVec3::new(0.0, 5.0, 0.0));
        cx.view = None;
        assert_eq!(cx.ray([5.0, 5.0]), None);
    }
}
