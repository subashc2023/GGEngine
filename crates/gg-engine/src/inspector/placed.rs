//! Where a remote Inspector's pick is, which its link gives: in development builds, and in builds
//! without `dev` too, whose link gives none, so that a layer that reads it builds in both.

use gg_core::math::Vec3;

use crate::{EntityId, Pose, Transform};

/// Where a remote Inspector's pick is in the game's world, as the game's last read of it gave it,
/// which [`InspectorLink::placed`](crate::InspectorLink::placed) gives: for a tool's handles, as
/// [`World::pose`] gives where an entity of a world of the app's own is.
///
/// [`World::pose`]: crate::World::pose
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The entity's ID, the pick's.
    pub id: EntityId,
    /// Its own transform.
    pub local: Transform,
    /// Where the space its transform is in sits in the world: its parent's pose, as the game's
    /// latest tick left it, or the identity for a root.
    pub space: Pose,
    /// The sphere round what it and every entity under it draw, in the world, as the game's last
    /// read of it gave it: for a tool to frame it by. `None` if nothing there draws, or the game
    /// doesn't say. Its center and its radius.
    pub bounds: Option<(Vec3, f32)>,
    /// Whether it's a model's node, which the world spawns from the model under its root, and
    /// tools' operations refuse to change, as [`ModelRoot`](crate::ModelRoot) says: no tool's
    /// handles move it.
    pub node: bool,
    /// How many times the Inspector has been reached, to a game or to none, which goes up at each
    /// reach, its reads of the game before gone, as a world's
    /// [`World::serial`](crate::World::serial) changes as it's another.
    pub generation: u64,
}
