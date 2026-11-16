//! Where a remote Inspector's pick is, as its link gives it. Builds without `dev` have the type
//! too, with a link that gives none, so a layer that reads it builds in both.

use gg_core::math::DVec3;

use crate::{EntityId, Pose, Transform};

/// Where a remote Inspector's pick is in the game's world, as the game's last read gave it. For a
/// tool's handles, as [`World::pose`] gives an entity of an app's own world.
///
/// Given by [`InspectorLink::placed`](crate::InspectorLink::placed).
///
/// [`World::pose`]: crate::World::pose
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The pick's ID.
    pub id: EntityId,
    /// Its own transform.
    pub local: Transform,
    /// Where its transform's space sits in the world: its parent's pose as the game's latest tick
    /// left it, or the identity for a root.
    pub space: Pose,
    /// The sphere (center, radius) round what it and every entity under it draw, in the world,
    /// for a tool to frame it by. `None` if nothing there draws, or the game doesn't say.
    pub bounds: Option<(DVec3, f32)>,
    /// Whether it's a model's node. The world spawns it from the model under its root and tools'
    /// operations refuse to change it, as [`ModelRoot`](crate::ModelRoot) says, so no handles move
    /// it.
    pub node: bool,
    /// How many times the Inspector has been reached, to a game or to none. Goes up at each
    /// reach, its reads of the game before gone, as [`World::serial`](crate::World::serial)
    /// changes for another world.
    pub generation: u64,
}
