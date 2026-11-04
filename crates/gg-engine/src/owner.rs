//! Which player an entity is.

use crate::Component;

/// The player whose entity this is, as [`Context::player`](crate::Context::player) numbers them,
/// and its descendants': a client of a server (`--connect`) draws the entities of players other
/// than its own a little behind the server's latest state, between the states it has, rather than
/// where its own ticks left them, which predict the other players as going on as their last
/// commands had them. Its own player's, and the entities no player owns, draw where its ticks left
/// them, as every run's do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub struct Owner {
    /// The player's number, from 0.
    pub player: u8,
}
