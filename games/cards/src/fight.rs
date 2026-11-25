//! The fight's rules: plain functions over [`Fight`], so tests need no world.

use gg_engine::prelude::*;

/// The player's health and energy a turn at the start of a fight.
pub(crate) const PLAYER_HP: i32 = 30;
pub(crate) const ENERGY: i32 = 3;
pub(crate) const ENEMY_HP: i32 = 40;
/// What the enemy's fixed intent deals each turn.
pub(crate) const ENEMY_ATTACK: i32 = 8;
/// Cards drawn at the start of each turn.
pub(crate) const HAND: usize = 5;

/// A kind of card. Its effect is `scripts/effects.luau`'s, indexed by its place here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub(crate) enum Card {
    #[default]
    Strike,
    Defend,
    Heavy,
}

impl Card {
    /// Damage to the enemy: the one home of the card's numbers, which labels and effects read.
    pub(crate) fn damage(self) -> i32 {
        match self {
            Self::Strike => 6,
            Self::Defend => 0,
            Self::Heavy => 12,
        }
    }

    pub(crate) fn block(self) -> i32 {
        match self {
            Self::Defend => 5,
            Self::Strike | Self::Heavy => 0,
        }
    }

    pub(crate) fn cost(self) -> i32 {
        match self {
            Self::Strike | Self::Defend => 1,
            Self::Heavy => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Strike => "STRIKE",
            Self::Defend => "DEFEND",
            Self::Heavy => "HEAVY",
        }
    }
}

impl std::fmt::Display for Card {
    /// The card's button text: name, effect and cost.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (title, cost) = (self.title(), self.cost());
        match (self.damage(), self.block()) {
            (damage, 0) => write!(f, "{title}\n{damage} DAMAGE\nCOST {cost}"),
            (_, block) => write!(f, "{title}\n{block} BLOCK\nCOST {cost}"),
        }
    }
}

/// The deck, the hand and the discard pile of a fight, and its numbers.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub(crate) struct Fight {
    pub(crate) player_hp: i32,
    pub(crate) player_block: i32,
    pub(crate) energy: i32,
    pub(crate) enemy_hp: i32,
    /// From 1 as a fight starts; 0 before it has.
    pub(crate) turn: u32,
    /// What shuffles draw from, so a run is the same from its seed.
    pub(crate) rng: Rng,
    /// How the last fight ended, until the script logs it: 1 won, 2 lost, 0 none.
    pub(crate) result: i32,
    pub(crate) wins: u32,
    pub(crate) losses: u32,
    pub(crate) deck: Vec<Card>,
    pub(crate) hand: Vec<Card>,
    pub(crate) discard: Vec<Card>,
}

/// The cards of a new deck: ten of them.
const DECK: [(Card, usize); 3] = [(Card::Strike, 5), (Card::Defend, 3), (Card::Heavy, 2)];

impl Fight {
    /// Starts a fight: the piles rebuilt (keeping their room), the numbers full, the first hand
    /// drawn. The stream, wins and losses carry on.
    pub(crate) fn start(&mut self) {
        self.deck.clear();
        self.hand.clear();
        self.discard.clear();
        for (card, count) in DECK {
            self.deck.extend(std::iter::repeat_n(card, count));
        }
        self.rng.shuffle(&mut self.deck);
        self.player_hp = PLAYER_HP;
        self.enemy_hp = ENEMY_HP;
        self.turn = 1;
        self.begin_turn();
    }

    fn begin_turn(&mut self) {
        self.energy = ENERGY;
        self.player_block = 0;
        for _ in 0..HAND {
            self.draw();
        }
    }

    /// Draws one card, reshuffling the discard pile into the deck if it's empty.
    pub(crate) fn draw(&mut self) {
        if self.deck.is_empty() {
            self.deck.append(&mut self.discard);
            self.rng.shuffle(&mut self.deck);
        }
        if let Some(card) = self.deck.pop() {
            self.hand.push(card);
        }
    }

    /// Plays the hand's card at `slot` if there's energy: it goes to the discard pile and takes
    /// effect. Returns whether it was played.
    pub(crate) fn play(&mut self, slot: usize) -> bool {
        let Some(&card) = self.hand.get(slot) else {
            return false;
        };
        if card.cost() > self.energy {
            return false;
        }
        self.energy -= card.cost();
        self.hand.remove(slot);
        self.discard.push(card);
        self.enemy_hp -= card.damage();
        self.player_block += card.block();
        true
    }

    /// Ends the turn: the hand is discarded, the enemy attacks (block first), a new turn begins.
    pub(crate) fn end_turn(&mut self) {
        self.discard.append(&mut self.hand);
        let through = (ENEMY_ATTACK - self.player_block).max(0);
        self.player_hp -= through;
        self.turn += 1;
        if self.player_hp > 0 {
            self.begin_turn();
        }
    }

    /// Ends the fight if either side is down: records its result for the script to log, and
    /// restarts.
    pub(crate) fn settle(&mut self) {
        let result = if self.enemy_hp <= 0 {
            self.wins += 1;
            1
        } else if self.player_hp <= 0 {
            self.losses += 1;
            2
        } else {
            return;
        };
        self.start();
        self.result = result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> Fight {
        let mut fight = Fight {
            rng: Rng::new(7),
            ..Fight::default()
        };
        fight.start();
        fight
    }

    #[test]
    fn a_fight_starts_with_a_hand_of_five_from_ten() {
        let fight = started();
        assert_eq!(fight.hand.len(), HAND);
        assert_eq!(fight.deck.len(), 5);
        assert_eq!(fight.energy, ENERGY);
    }

    #[test]
    fn playing_costs_energy_and_discards() {
        let mut fight = started();
        fight.hand = vec![Card::Heavy, Card::Strike];
        assert!(fight.play(0));
        assert_eq!((fight.energy, fight.enemy_hp), (1, ENEMY_HP - 12));
        assert_eq!(fight.discard, [Card::Heavy]);
        fight.hand = vec![Card::Heavy];
        assert!(!fight.play(0), "one energy is short of a Heavy");
    }

    #[test]
    fn block_absorbs_the_attack_first() {
        let mut fight = started();
        fight.player_block = 5;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - 3);
        assert_eq!(fight.player_block, 0);
        fight.player_block = 20;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - 3);
    }

    #[test]
    fn the_discard_pile_is_reshuffled_when_the_deck_runs_out() {
        let mut fight = started();
        fight.end_turn();
        assert_eq!(fight.hand.len(), HAND);
        assert_eq!(
            fight.deck.len() + fight.discard.len() + fight.hand.len(),
            10
        );
        fight.end_turn();
        assert_eq!(fight.hand.len(), HAND);
        assert_eq!(
            fight.deck.len() + fight.discard.len() + fight.hand.len(),
            10
        );
    }

    #[test]
    fn the_same_seed_shuffles_alike() {
        assert_eq!(started().hand, started().hand);
    }

    #[test]
    fn a_lost_fight_restarts() {
        let mut fight = started();
        fight.player_hp = 0;
        fight.settle();
        assert_eq!(
            (fight.losses, fight.result, fight.player_hp),
            (1, 2, PLAYER_HP)
        );
    }
}
