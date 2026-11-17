//! Where entities are in the world.

use gg_core::math::{DAffine3, DVec3, Mat4, Quat, Vec3};

use crate::Component;

/// Where an entity is, how it's turned and how big it is: translation, rotation and scale.
///
/// As glTF's nodes have them, they are in its [`Parent`](crate::Parent)'s space, or the world's
/// without one. [`matrix`](Self::matrix) applies scale, then rotation, then translation.
/// Translation is `f64`, so an entity 1,000 km from the origin is as precise as one at it (about a
/// tenth of a nanometer); rotation and scale are `f32`. A child is placed in the world by its own
/// transform, then by each ancestor's in turn. Draw order is in
/// [`Context::world`](crate::Context::world): opaque meshes, then the rest back to front by world
/// z, ties in entity-ID order, which overlapping entities keep as others come and go. Frames draw
/// each between where a tick found it and where it left it: a tick that makes one jump, like a
/// teleport, calls [`World::reset_interpolation`](crate::World::reset_interpolation) for it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Transform {
    /// Where its origin is, from its parent's, or the world's.
    pub translation: DVec3,
    /// As a unit quaternion.
    pub rotation: Quat,
    /// How big it is along each of its own axes, as a multiple of its own size.
    #[reflect(default = Vec3::ONE)]
    pub scale: Vec3,
}

impl Transform {
    /// At the origin, unturned, at its own size.
    pub const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// The matrix from the entity's space to its parent's (the world's without one), in `f64`.
    pub fn matrix(&self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(
            self.scale.as_dvec3(),
            self.rotation.as_dquat(),
            self.translation,
        )
    }

    /// [`matrix`](Self::matrix) in `f32`, its translation relative to `origin`, as frames draw.
    pub(crate) fn matrix_from(&self, origin: DVec3) -> Mat4 {
        let translation = (self.translation - origin).as_vec3();
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, translation)
    }
}

/// A world `matrix` in `f32`, relative to `origin`, as frames draw.
pub(crate) fn from_origin(matrix: DAffine3, origin: DVec3) -> Mat4 {
    let mut relative = matrix;
    relative.translation -= origin;
    Mat4::from(relative.as_affine3a())
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
