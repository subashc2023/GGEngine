//! Where a remote Inspector's pick is, which its link gives: in development builds, and in builds
//! without `dev` too, whose link gives none, so that a layer that reads it builds in both.

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
    /// How many times the Inspector has been reached, to a game or to none, which goes up at each
    /// reach, its reads of the game before gone, as a world's
    /// [`World::serial`](crate::World::serial) changes as it's another.
    pub generation: u64,
}
