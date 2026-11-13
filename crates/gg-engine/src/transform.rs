//! Where entities are in the world.

use gg_core::math::{DAffine3, DVec3, Mat4, Quat, Vec3};

use crate::Component;

/// Where an entity is, how it's turned and how big it is, in its parent's space if it has a
/// [`Parent`](crate::Parent), and otherwise in the world's: its translation, rotation and scale, as
/// glTF's nodes have them, which [`matrix`](Self::matrix) applies scale first, then rotation, then
/// translation. Its translation is `f64`, so that an entity 1,000 km from the origin is placed as
/// precisely as one at it, to about a tenth of a nanometer; its rotation and scale are `f32`, which
/// are as precise wherever it is. A parent carries its children along: a child is placed in the
/// world by its own transform, then by each of its ancestors' in turn. The engine draws the world's
/// opaque meshes first, then the rest back to front by their z's in the world, as
/// [`Context::world`](crate::Context::world) says, each in the order of their entities' IDs, which
/// overlapping entities keep as others come and go. Frames draw each on its way from where a tick
/// found it to where the tick left it: a tick that moves one so that it jumps, as a teleport does,
/// calls [`World::reset_interpolation`](crate::World::reset_interpolation) for it.
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

    /// The matrix from the entity's own space to its parent's, or to the world's if it has none,
    /// in `f64`, as its translation is.
    pub fn matrix(&self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(
            self.scale.as_dvec3(),
            self.rotation.as_dquat(),
            self.translation,
        )
    }

    /// [`matrix`](Self::matrix) in `f32`, its translation from `origin`, as a frame draws relative
    /// to its own: computed in `f32`, as precise as its rotation and scale are.
    pub(crate) fn matrix_from(&self, origin: DVec3) -> Mat4 {
        let translation = (self.translation - origin).as_vec3();
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, translation)
    }
}

/// `matrix`, a place in the world, in `f32` from `origin`, as a frame draws relative to its own.
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
