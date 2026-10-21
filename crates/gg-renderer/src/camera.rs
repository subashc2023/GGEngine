//! Cameras: where the game's view sees the world from, and how what's in front of them is
//! projected onto it.

use gg_core::math::camera::rh::proj::vulkan;
use gg_core::math::{Mat4, Quat, Vec3};

/// Where the game's view sees the world from. World space is right-handed with +Y up, and a
/// camera without rotation looks down -Z, with +X to its right and +Y up, as glTF's cameras do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// Which way it faces, as a unit quaternion. A 2D game's camera turns about Z, with
    /// [`Quat::from_rotation_z`]: turning it counterclockwise turns the scene clockwise on screen.
    pub rotation: Quat,
    pub projection: Projection,
}

/// How a camera projects what's in front of it onto the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Without perspective, as 2D games draw: the view spans `height` world units from bottom to
    /// top, and as many across as its aspect ratio makes, so nothing stretches as it's resized. It
    /// shows what's from `near` to `far` in front of the camera; either may be negative, to show
    /// what's behind it too, as 2D games' cameras do.
    ///
    /// Rendering panics unless `height` is a normal number, and `near` and `far` are finite and
    /// different: otherwise the view would be blank.
    Orthographic { height: f32, near: f32, far: f32 },
}

impl Default for Camera {
    /// At the origin, without rotation, seeing the 2 units around it from bottom to top, and from
    /// 1 behind it to 1 in front.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Orthographic {
                height: 2.0,
                near: -1.0,
                far: 1.0,
            },
        }
    }
}

impl Camera {
    /// The matrix from world space to Vulkan's clip space, whose Y points down, for a view
    /// `aspect` times as wide as it's tall.
    ///
    /// # Panics
    ///
    /// If its projection's height isn't a normal number, or its near and far aren't finite and
    /// different, which would leave the view blank.
    pub(crate) fn view_projection(&self, aspect: f32) -> Mat4 {
        let Projection::Orthographic { height, near, far } = self.projection;
        assert!(
            height.is_normal() && near.is_finite() && far.is_finite() && near != far,
            "a camera sees a height of the world, between a near and a far that differ; got {:?}",
            self.projection
        );
        let top = height / 2.0;
        let right = top * aspect;
        let projection = vulkan::orthographic(-right, right, -top, top, near, far);
        let view = Mat4::from_rotation_translation(self.rotation, self.position).inverse();
        projection * view
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;

    #[test]
    fn a_camera_sees_its_height_of_the_world_without_stretching_it() {
        // A quarter turn counterclockwise, so its up is the world's -X and its right the world's
        // +Y.
        let camera = Camera {
            position: Vec3::new(3.0, -2.0, 5.0),
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            projection: Projection::Orthographic {
                height: 4.0,
                near: 1.0,
                far: 11.0,
            },
        };
        let clip = camera.view_projection(1.5);
        for (world, expected) in [
            // 1 in front of it: the view's center, as near as it sees.
            ([3.0, -2.0, 4.0], [0.0, 0.0, 0.0]),
            // Half its height up, and 11 in front: the top edge, where Vulkan's Y is -1, as far as
            // it sees.
            ([1.0, -2.0, -6.0], [0.0, -1.0, 1.0]),
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
    }
}
