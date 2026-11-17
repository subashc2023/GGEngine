//! The game's components.

use gg_engine::prelude::*;

/// A player's disc: which player steers it, its heading, and the ticks left of its dash and until
/// the next.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Player {
    pub(crate) index: u8,
    pub(crate) heading: Vec2,
    pub(crate) dash: u32,
    pub(crate) cooldown: u32,
}

/// A shard to collect, and its turn.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Shard {
    #[reflect(degrees)]
    pub(crate) angle: f32,
}

/// The match: each player's shards and matches won, and the state of the random numbers placing
/// shards. The world holds it, so every peer, replay and restored run places them alike.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub(crate) struct Match {
    pub(crate) scores: [u32; 2],
    pub(crate) wins: [u32; 2],
    pub(crate) random: u64,
}

/// Registers the game's components, so its scene and the tool API know them before any entity has
/// one.
pub(crate) fn register(world: &mut World) -> Result<(), Error> {
    world.register_all::<(Player, Shard, Match)>()
}
