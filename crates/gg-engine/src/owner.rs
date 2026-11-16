//! Which player an entity is.

use crate::Component;

/// The player whose entity this is, and its descendants', numbered as
/// [`Context::player`](crate::Context::player) does. A client of a server (`--connect`) draws other
/// players' entities a little behind the server's latest state, interpolated between the states
/// it has, not where its own ticks left them (those predict other players as repeating their last
/// commands). Its own player's entities and unowned ones draw where its ticks left them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub struct Owner {
    /// The player's number, from 0.
    pub player: u8,
}
