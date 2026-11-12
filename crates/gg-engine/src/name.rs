//! What entities are called.

use crate::Component;

/// What an entity is called, for people: the Entities and Inspector panels show it, and scenes
/// save it. It's optional, and it isn't unique, so two entities may share one. A model's node
/// without one is called by its node's name in the model's file, as
/// [`World::name`](crate::World::name) gives either. Files, operations and diagnostics name an
/// entity by its [`EntityId`](crate::EntityId), never by this.
#[derive(Clone, Debug, PartialEq, Eq, Component)]
pub struct Name(pub String);
