//! What entities are called.

use crate::Component;

/// What an entity is called, for people.
///
/// The Entities and Inspector panels show it, and scenes save it. It's optional, so entities that
/// come by the thousand (a swarm, bullets) hold none and spawn without allocating. It isn't unique.
/// A model's node without one takes its name from the model's file;
/// [`World::name`](crate::World::name) gives either. Files, operations and diagnostics name an
/// entity by its [`EntityId`](crate::EntityId). The tool API also takes the name of the one entity
/// that has it.
#[derive(Clone, Debug, PartialEq, Eq, Component)]
pub struct Name(pub String);
