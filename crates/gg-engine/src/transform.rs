//! Where entities are in the world.

use gg_core::math::{Mat4, Quat, Vec3};

use crate::Component;

/// Where an entity is, how it's turned and how big it is, in its parent's space if it has a
/// [`Parent`](crate::Parent), and otherwise in the world's: its translation, rotation and scale, as
/// glTF's nodes have them, which [`matrix`](Self::matrix) applies scale first, then rotation, then
/// translation. A parent carries its children along: a child is placed in the world by its own
/// transform, then by each of its ancestors' in turn. The engine draws the world's opaque meshes
/// first, then the rest back to front by their z's in the world, as
/// [`Context::world`](crate::Context::world) says, each in the order of their entities' IDs, which
/// overlapping entities keep as others come and go. Frames draw each on its way from where a tick
/// found it to where the tick left it: a tick that moves one so that it jumps, as a teleport does,
/// calls [`World::reset_interpolation`](crate::World::reset_interpolation) for it.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Transform {
    /// Where its origin is, from its parent's, or the world's.
    pub translation: Vec3,
    /// As a unit quaternion.
    pub rotation: Quat,
    /// How big it is along each of its own axes, as a multiple of its own size.
    #[reflect(default = Vec3::ONE)]
    pub scale: Vec3,
}

impl Transform {
    /// At the origin, unturned, at its own size.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// The matrix from the entity's own space to its parent's, or to the world's if it has none.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
