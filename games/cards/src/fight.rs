//! The fight's rules: plain functions over [`Fight`], so tests need no world.

use gg_engine::prelude::*;

/// The player's health and energy a turn at the start of a fight.
pub(crate) const PLAYER_HP: i32 = 30;
pub(crate) const ENERGY: i32 = 3;
pub(crate) const ENEMY_HP: i32 = 40;
/// What the enemy's plain attack deals; its big hit deals twice that.
pub(crate) const ENEMY_ATTACK: i32 = 5;
/// The block the enemy's guard gives it until its next turn.
pub(crate) const ENEMY_GUARD: i32 = 6;
/// Cards drawn at the start of each turn.
pub(crate) const HAND: usize = 6;

/// How a fight stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub(crate) enum Outcome {
    #[default]
    Fighting,
    Won,
    Lost,
}

/// What the enemy will do at the end of the turn, which the player sees to plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub(crate) enum Intent {
    #[default]
    Attack,
    BigHit,
    Guard,
}

impl Intent {
    /// Damage dealt to the player, before their block.
    pub(crate) fn damage(self) -> i32 {
        match self {
            Self::Attack => ENEMY_ATTACK,
            Self::BigHit => ENEMY_ATTACK * 2,
            Self::Guard => 0,
        }
    }

    /// Block the enemy gains, which holds until its next turn.
    pub(crate) fn block(self) -> i32 {
        match self {
            Self::Guard => ENEMY_GUARD,
            Self::Attack | Self::BigHit => 0,
        }
    }
}

impl std::fmt::Display for Intent {
    /// The plan as the UI shows it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.damage(), self.block()) {
            (damage, 0) if *self == Self::BigHit => write!(f, "BIG HIT FOR {damage}"),
            (damage, 0) => write!(f, "ATTACKS FOR {damage}"),
            (_, block) => write!(f, "BLOCKS FOR {block}"),
        }
    }
}

/// A kind of card. Its effect is `scripts/effects.luau`'s, indexed by its place here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub(crate) enum Card {
    #[default]
    Strike,
    Defend,
    Heavy,
    Poison,
}

impl Card {
    /// Damage to the enemy: the one home of the card's numbers, which labels and effects read.
    pub(crate) fn damage(self) -> i32 {
        match self {
            Self::Strike => 6,
            Self::Defend | Self::Poison => 0,
            Self::Heavy => 12,
        }
    }

    pub(crate) fn block(self) -> i32 {
        match self {
            Self::Defend => 5,
            Self::Strike | Self::Heavy | Self::Poison => 0,
        }
    }

    /// Poison put on the enemy.
    pub(crate) fn poison(self) -> i32 {
        match self {
            Self::Poison => 3,
            Self::Strike | Self::Defend | Self::Heavy => 0,
        }
    }

    pub(crate) fn cost(self) -> i32 {
        match self {
            Self::Strike | Self::Defend | Self::Poison => 1,
            Self::Heavy => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Strike => "STRIKE",
            Self::Defend => "DEFEND",
            Self::Heavy => "HEAVY",
            Self::Poison => "POISON",
        }
    }
}

impl std::fmt::Display for Card {
    /// The card's button text: name, effect and cost.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (title, cost) = (self.title(), self.cost());
        match (self.damage(), self.block(), self.poison()) {
            (damage, 0, 0) => write!(f, "{title}\n{damage} DAMAGE\nCOST {cost}"),
            (_, 0, poison) => write!(f, "{title}\n{poison} POISON\nCOST {cost}"),
            (_, block, _) => write!(f, "{title}\n{block} BLOCK\nCOST {cost}"),
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
    /// Health the enemy loses at the start of its turn; it then goes down by one.
    pub(crate) enemy_poison: i32,
    /// Absorbs the player's damage until the enemy's next turn.
    pub(crate) enemy_block: i32,
    /// What the enemy does when the turn ends.
    pub(crate) intent: Intent,
    /// Won or lost fights stay as they are, for the player to see, until a new one starts.
    pub(crate) outcome: Outcome,
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
const DECK: [(Card, usize); 4] = [
    (Card::Strike, 3),
    (Card::Defend, 3),
    (Card::Heavy, 2),
    (Card::Poison, 2),
];

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
        self.enemy_poison = 0;
        self.enemy_block = 0;
        self.outcome = Outcome::Fighting;
        self.plan();
        self.turn = 1;
        self.begin_turn();
    }

    /// Chooses the enemy's next move: mostly plain attacks, sometimes a big hit or a guard.
    fn plan(&mut self) {
        self.intent = match self.rng.below(4) {
            0 | 1 => Intent::Attack,
            2 => Intent::BigHit,
            _ => Intent::Guard,
        };
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
        if self.outcome != Outcome::Fighting {
            return false;
        }
        if card.cost() > self.energy {
            return false;
        }
        self.energy -= card.cost();
        self.hand.remove(slot);
        self.discard.push(card);
        let absorbed = card.damage().min(self.enemy_block);
        self.enemy_block -= absorbed;
        self.enemy_hp -= card.damage() - absorbed;
        self.enemy_poison += card.poison();
        self.player_block += card.block();
        true
    }

    /// Ends the turn: the hand is discarded, the enemy attacks (block first), a new turn begins.
    pub(crate) fn end_turn(&mut self) {
        if self.outcome != Outcome::Fighting {
            return;
        }
        self.discard.append(&mut self.hand);
        // The enemy's turn begins: poison first, and a dead enemy doesn't attack.
        self.enemy_hp -= self.enemy_poison;
        self.enemy_poison = (self.enemy_poison - 1).max(0);
        if self.enemy_hp > 0 {
            self.enemy_block = self.intent.block();
            let through = (self.intent.damage() - self.player_block).max(0);
            self.player_hp -= through;
            self.plan();
        }
        self.turn += 1;
        if self.player_hp > 0 {
            self.begin_turn();
        }
    }

    /// Ends the fight if either side is down: records the outcome, and its result for the script
    /// to log. A new fight starts only with [`start`](Self::start).
    pub(crate) fn settle(&mut self) {
        if self.outcome != Outcome::Fighting {
            return;
        }
        if self.enemy_hp <= 0 {
            self.wins += 1;
            self.outcome = Outcome::Won;
            self.result = 1;
        } else if self.player_hp <= 0 {
            self.losses += 1;
            self.outcome = Outcome::Lost;
            self.result = 2;
        }
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
    fn a_fight_starts_with_a_hand_of_six_from_ten() {
        let fight = started();
        assert_eq!(fight.hand.len(), HAND);
        assert_eq!(fight.deck.len(), 10 - HAND);
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
    fn the_enemy_does_what_it_planned() {
        let mut fight = started();
        fight.intent = Intent::BigHit;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - 2 * ENEMY_ATTACK);
        fight.intent = Intent::Guard;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - 2 * ENEMY_ATTACK);
        assert_eq!(fight.enemy_block, ENEMY_GUARD);
    }

    #[test]
    fn enemy_block_absorbs_damage_until_its_next_turn() {
        let mut fight = started();
        fight.enemy_block = 4;
        fight.hand = vec![Card::Strike, Card::Strike];
        assert!(fight.play(0));
        assert_eq!((fight.enemy_block, fight.enemy_hp), (0, ENEMY_HP - 2));
        assert!(fight.play(0));
        assert_eq!(fight.enemy_hp, ENEMY_HP - 8);
    }

    #[test]
    fn block_absorbs_the_attack_first() {
        let mut fight = started();
        fight.intent = Intent::Attack;
        fight.player_block = 2;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - (ENEMY_ATTACK - 2));
        assert_eq!(fight.player_block, 0);
        fight.intent = Intent::Attack;
        fight.player_block = 20;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP - (ENEMY_ATTACK - 2));
    }

    #[test]
    fn poison_hurts_the_enemy_each_turn_and_wanes() {
        let mut fight = started();
        fight.hand = vec![Card::Poison];
        assert!(fight.play(0));
        assert_eq!(fight.enemy_poison, 3);
        fight.end_turn();
        assert_eq!((fight.enemy_hp, fight.enemy_poison), (ENEMY_HP - 3, 2));
        fight.end_turn();
        assert_eq!((fight.enemy_hp, fight.enemy_poison), (ENEMY_HP - 5, 1));
    }

    #[test]
    fn a_poisoned_enemy_that_falls_does_not_attack() {
        let mut fight = started();
        fight.enemy_hp = 2;
        fight.enemy_poison = 3;
        fight.intent = Intent::BigHit;
        fight.end_turn();
        assert_eq!(fight.player_hp, PLAYER_HP);
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
    fn a_lost_fight_waits_for_a_new_one() {
        let mut fight = started();
        fight.player_hp = 0;
        fight.settle();
        assert_eq!((fight.losses, fight.result), (1, 2));
        assert_eq!(fight.outcome, Outcome::Lost);
        fight.settle();
        assert_eq!(fight.losses, 1, "a lost fight counts once");
        assert!(!fight.play(0), "no card plays after the fight");
        let turn = fight.turn;
        fight.end_turn();
        assert_eq!(fight.turn, turn);
        fight.start();
        assert_eq!(
            (fight.outcome, fight.player_hp),
            (Outcome::Fighting, PLAYER_HP)
        );
    }

    #[test]
    fn a_won_fight_counts_a_win() {
        let mut fight = started();
        fight.enemy_hp = 0;
        fight.settle();
        assert_eq!(
            (fight.wins, fight.result, fight.outcome),
            (1, 1, Outcome::Won)
        );
    }
}
