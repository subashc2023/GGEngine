//! Viewpoints: where the game's view sees the world from, and how what's in front of it is
//! projected onto the view.

use std::f32::consts::PI;

use gg_core::math::camera::rh::proj::vulkan;
use gg_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};

/// Where the game's view sees the world from, and how. World space is right-handed with +Y up, and
/// a viewpoint without rotation looks down -Z, with +X to its right and +Y up, as glTF's cameras
/// do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewpoint {
    /// Where it is, in `f64`, as the engine's translations are, so that it sees as precisely 1,000
    /// km from the origin as at it: a frame draws relative to its
    /// [`origin`](crate::RenderData::origin), near the viewpoint, in `f32`.
    pub position: DVec3,
    /// Which way it faces, as a unit quaternion. A 2D game's camera turns about Z, with
    /// [`Quat::from_rotation_z`]: turning it counterclockwise turns the scene clockwise on screen.
    pub rotation: Quat,
    pub projection: Projection,
}

/// How a camera projects what's in front of it onto the view, and its depth: both projections run
/// it reversed, from 1 at the near plane to 0 at the far one, or far away, which keeps the
/// precision of the view's depth, floating-point, where it's needed, and lets perspective see
/// infinitely far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Without perspective, as 2D games draw: the view spans `height` world units from bottom to
    /// top, and as many across as its aspect ratio makes, so nothing stretches as it's resized. It
    /// shows what's from `near` to `far` in front of the camera; either may be negative, to show
    /// what's behind it too, as 2D games' cameras do, and either the nearer.
    ///
    /// Rendering panics unless `height` is a positive normal number, and `near` and `far` are
    /// finite and different: otherwise the view would be blank or upside down.
    Orthographic { height: f32, near: f32, far: f32 },
    /// With perspective, as 3D games draw: the view spans `fov` radians from bottom to top, as
    /// glTF's `yfov` does, and as much across as its aspect ratio makes. It shows what's at least
    /// `near` in front of the camera, however far.
    ///
    /// Rendering panics unless `fov` is more than 0 and less than π, and `near` is a positive
    /// normal number: otherwise the view would be blank or upside down.
    Perspective { fov: f32, near: f32 },
}

impl Projection {
    /// Whether it can project, as its variant says: rendering panics on one that can't.
    pub fn projects(&self) -> bool {
        match *self {
            Self::Orthographic { height, near, far } => {
                height.is_normal()
                    && height > 0.0
                    && near.is_finite()
                    && far.is_finite()
                    && near != far
            }
            Self::Perspective { fov, near } => {
                fov > 0.0 && fov < PI && near.is_normal() && near > 0.0
            }
        }
    }
}

impl Default for Viewpoint {
    /// At the origin, without rotation, seeing the 2 units around it from bottom to top, and from
    /// 1 behind it to 1 in front.
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Orthographic {
                height: 2.0,
                near: -1.0,
                far: 1.0,
            },
        }
    }
}

impl Viewpoint {
    /// The viewpoint where it is in a world whose origin is moved to `origin`, as a frame draws
    /// relative to its own: the same but for its position, `origin` taken from it, in `f64`.
    pub fn relative_to(self, origin: DVec3) -> Self {
        Self {
            position: self.position - origin,
            ..self
        }
    }

    /// The matrix from world space to Vulkan's clip space, whose Y points down, for a view
    /// `aspect` times as wide as it's tall: in `f32`, so that far from the origin its translation
    /// rounds, as `f32`s do, 6 cm at 1,000 km; one [`relative_to`](Self::relative_to) a point near
    /// it maps the world relative to that point.
    ///
    /// # Panics
    ///
    /// If its projection can't project, as [`Projection`] says.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let view =
            Mat4::from_rotation_translation(self.rotation, self.position.as_vec3()).inverse();
        self.projection_matrix(aspect) * view
    }

    /// The matrix from Vulkan's clip space to the ways the view sees, for a view `aspect` times as
    /// wide as it's tall, as a sky is drawn: with perspective, from a point at the near plane,
    /// where reversed depth is 1, to one whose way from the origin is the way its pixel sees,
    /// whatever the viewpoint's position; without, from every point to the one way it sees.
    ///
    /// # Panics
    ///
    /// If its projection can't project, as [`Projection`] says.
    pub(crate) fn ways_from_clip(&self, aspect: f32) -> Mat4 {
        let projection = self.projection_matrix(aspect);
        match self.projection {
            Projection::Orthographic { .. } => {
                let along = self.rotation * Vec3::NEG_Z;
                Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, along.extend(1.0))
            }
            Projection::Perspective { .. } => {
                (projection * Mat4::from_quat(self.rotation.inverse())).inverse()
            }
        }
    }

    /// The matrix of its projection alone, from its own space to Vulkan's clip space.
    ///
    /// # Panics
    ///
    /// If its projection can't project, as [`Projection`] says.
    pub(crate) fn projection_matrix(&self, aspect: f32) -> Mat4 {
        // glam checks none of it, without its `glam-assert` feature.
        assert!(
            self.projection.projects(),
            "a viewpoint sees a positive height of the world, between a near and a far that \
             differ, or with perspective, more than nothing and less than a half turn from bottom \
             to top, from a near plane in front of it; got {:?}",
            self.projection
        );
        match self.projection {
            Projection::Orthographic { height, near, far } => {
                let top = height / 2.0;
                let right = top * aspect;
                // The nearer as near, whichever is given as it; then swapped, so that depth runs
                // from 1 at the near plane to 0 at the far one, as perspective's does.
                let (near, far) = (near.min(far), near.max(far));
                vulkan::orthographic(-right, right, -top, top, far, near)
            }
            Projection::Perspective { fov, near } => {
                vulkan::perspective_infinite_reverse(fov, aspect, near)
            }
        }
    }

    /// Where lit shading sees from, as the frame's data holds it: its position and 1 with
    /// perspective, or the way back toward it, its own +Z, and 0 without, as an orthographic
    /// projection sees along one way wherever it looks.
    pub(crate) fn eye(&self) -> Vec4 {
        match self.projection {
            Projection::Orthographic { .. } => (self.rotation * Vec3::Z).extend(0.0),
            Projection::Perspective { .. } => self.position.as_vec3().extend(1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;
    use std::panic::{self, AssertUnwindSafe};

    use gg_core::math::Vec4;

    use super::*;

    #[test]
    fn a_viewpoint_sees_its_height_of_the_world_without_stretching_it() {
        // A quarter turn counterclockwise, so its up is the world's -X and its right the world's
        // +Y.
        let viewpoint = Viewpoint {
            position: DVec3::new(3.0, -2.0, 5.0),
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            projection: Projection::Orthographic {
                height: 4.0,
                near: 1.0,
                far: 11.0,
            },
        };
        let clip = viewpoint.view_projection(1.5);
        for (world, expected) in [
            // 1 in front of it: the view's center, as near as it sees, where depth is 1.
            ([3.0, -2.0, 4.0], [0.0, 0.0, 1.0]),
            // Half its height up, and 11 in front: the top edge, where Vulkan's Y is -1, as far as
            // it sees, where depth is 0.
            ([1.0, -2.0, -6.0], [0.0, -1.0, 0.0]),
            // Half its width, 1.5 times half its height, to its right, and 6 in front: the right
            // edge, halfway into what it sees.
            ([3.0, 1.0, -1.0], [1.0, 0.0, 0.5]),
        ] {
            let got = clip.project_point3(Vec3::from(world));
            assert!(
                got.abs_diff_eq(Vec3::from(expected), 1e-5),
                "{world:?} reached {got}, not {expected:?}"
            );
        }
        // Its near and far given the other way round, it sees the same.
        let swapped = Viewpoint {
            projection: Projection::Orthographic {
                height: 4.0,
                near: 11.0,
                far: 1.0,
            },
            ..viewpoint
        };
        assert_eq!(swapped.view_projection(1.5), clip);
    }

    #[test]
    fn a_perspective_viewpoint_sees_its_field_of_view_to_infinity() {
        // A quarter turn counterclockwise, so its up is the world's -X and its right the world's
        // +Y; a quarter turn from bottom to top, so the view spans as far up as it is in front.
        let viewpoint = Viewpoint {
            position: DVec3::new(3.0, -2.0, 5.0),
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            projection: Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.5,
            },
        };
        let clip = viewpoint.view_projection(1.5);
        for (world, expected) in [
            // As near as it sees: the view's center, where depth is 1.
            ([3.0, -2.0, 4.5], [0.0, 0.0, 1.0]),
            // Twice as far, depth halves.
            ([3.0, -2.0, 4.0], [0.0, 0.0, 0.5]),
            // 1 up and 1 in front: the top edge, where Vulkan's Y is -1.
            ([2.0, -2.0, 4.0], [0.0, -1.0, 0.5]),
            // 1.5 times as far to its right as up: the right edge, 2 in front.
            ([3.0, 1.0, 3.0], [1.0, 0.0, 0.25]),
        ] {
            let got = clip.project_point3(Vec3::from(world));
            assert!(
                got.abs_diff_eq(Vec3::from(expected), 1e-5),
                "{world:?} reached {got}, not {expected:?}"
            );
        }
        // A million in front, still in view, at a depth just over 0.
        let far = clip.project_point3(Vec3::new(3.0, -2.0, 5.0 - 1e6));
        assert!(far.z > 0.0 && far.z < 1e-5, "{far}");
        // Behind it, where Vulkan clips what's at a negative w.
        let behind = clip * Vec4::new(3.0, -2.0, 6.0, 1.0);
        assert!(behind.w < 0.0, "{behind}");
    }

    /// A pixel sees the way its point in the world is from the viewpoint, wherever that is: the
    /// center ahead, the top edge 45° up, and the right edge, 1.5 times as far across, toward its
    /// right; and without perspective, every pixel the one way ahead.
    #[test]
    fn each_pixel_sees_the_way_it_shows() {
        let viewpoint = Viewpoint {
            position: DVec3::new(3.0, -2.0, 5.0),
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            projection: Projection::Perspective {
                fov: FRAC_PI_2,
                near: 0.5,
            },
        };
        let way = |viewpoint: &Viewpoint, x: f32, y: f32| {
            let point = viewpoint.ways_from_clip(1.5) * Vec4::new(x, y, 1.0, 1.0);
            (point.truncate() / point.w).normalize()
        };
        for ([x, y], expected) in [
            ([0.0, 0.0], Vec3::NEG_Z),
            ([0.0, -1.0], Vec3::new(-1.0, 0.0, -1.0).normalize()),
            ([1.0, 0.0], Vec3::new(0.0, 1.5, -1.0).normalize()),
        ] {
            let got = way(&viewpoint, x, y);
            assert!(got.abs_diff_eq(expected, 1e-5), "{got}, not {expected}");
        }
        let flat = Viewpoint {
            projection: Projection::Orthographic {
                height: 4.0,
                near: 1.0,
                far: 11.0,
            },
            rotation: Quat::from_rotation_y(FRAC_PI_2),
            ..viewpoint
        };
        for [x, y] in [[0.0, 0.0], [1.0, -1.0]] {
            let got = way(&flat, x, y);
            assert!(got.abs_diff_eq(Vec3::NEG_X, 1e-5), "{got}");
        }
    }

    #[test]
    fn a_projection_that_cant_project_panics() {
        let orthographic = |height, near, far| Projection::Orthographic { height, near, far };
        let perspective = |fov, near| Projection::Perspective { fov, near };
        for projection in [
            // Upside down, flat, of a height that's no number, and seeing no depth.
            orthographic(-2.0, -1.0, 1.0),
            orthographic(-f32::MIN_POSITIVE, -1.0, 1.0),
            orthographic(0.0, -1.0, 1.0),
            orthographic(f32::NAN, -1.0, 1.0),
            orthographic(2.0, 1.0, 1.0),
            perspective(0.0, 0.1),
            perspective(PI, 0.1),
            perspective(f32::NAN, 0.1),
            perspective(-1.0, 0.1),
            perspective(1.0, 0.0),
            perspective(1.0, -0.1),
            perspective(1.0, f32::NAN),
            perspective(1.0, f32::MIN_POSITIVE / 2.0),
        ] {
            assert!(!projection.projects(), "{projection:?} projects");
            let viewpoint = Viewpoint {
                projection,
                ..Viewpoint::default()
            };
            let projected =
                panic::catch_unwind(AssertUnwindSafe(|| viewpoint.view_projection(1.0)));
            assert!(projected.is_err(), "{projection:?} projected");
        }
    }
}
