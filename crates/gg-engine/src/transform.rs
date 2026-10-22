//! Where entities are in the world.

use gg_core::math::{Mat4, Quat, Vec3};

use crate::Component;

/// Where an entity is in the world, how it's turned and how big it is: its translation, rotation
/// and scale, as glTF's nodes have them, which [`matrix`](Self::matrix) applies scale first, then
/// rotation, then translation. The engine draws the world's entities back to front by
/// `translation.z`, so entities that overlap need different z's: those at one z draw in an order
/// that changes as entities are despawned or change components.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    /// As a unit quaternion.
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    /// At the origin, unturned, at its own size.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// The matrix from the entity's own space to the world's.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Component for Transform {}
