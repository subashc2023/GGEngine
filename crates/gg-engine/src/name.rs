//! What entities are called.

use crate::Component;

/// What an entity is called, for people: the Entities and Inspector panels show it, and scenes
/// save it. It's optional, and an entity without one shows by its ID; and it isn't unique, so two
/// entities may share one. Files, operations and diagnostics name an entity by its
/// [`EntityId`](crate::EntityId), never by this.
#[derive(Clone, Debug, PartialEq, Eq, Component)]
pub struct Name(pub String);
