//! cards, a game made with GGEngine: one turn-based card fight.
//!
//! Its start scene holds the fight. The UI is `ui/fight.ui`; click a card to play it, End Turn to
//! let the enemy attack. Card effects are `fight.rs`'s; `scripts/effects.luau` logs each result.
//!
//! The enemy shows what it will do next turn and varies it: an attack, a big hit or a guard. A
//! Poison card poisons it. A hovered card you can pay for lifts, and the enemy's panel shakes when
//! hit: both are UI offsets, which move what draws without moving the layout. A won or lost fight
//! covers the hand and End Turn with its screen, whose Play Again button starts a fresh fight.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

mod fight;

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;

use gg_engine::prelude::*;
use gg_engine::{GameUi, UiEvent};

use fight::{Card, Fight, HAND, Intent, Outcome};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, what its recordings replay, and
/// what a restart in place builds again (`App::hot`).
fn app() -> App {
    let app = App::new("cards")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"))
        .hot(app);
    app.layer(Game::new()).debug_overlays()
}

/// What the UI's presses do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Plays the hand's card at `slot`.
    Play {
        slot: u32,
    },
    EndTurn,
    /// Starts a fresh fight.
    PlayAgain,
}

/// The card buttons' names, by hand slot.
const CARDS: [&str; HAND] = ["card0", "card1", "card2", "card3", "card4", "card5"];

/// The invisible buttons that keep each slot's place under the cursor while its card is lifted
/// off it, so the lift doesn't let go of the card.
const SLOTS: [&str; HAND] = ["slot0", "slot1", "slot2", "slot3", "slot4", "slot5"];

/// How far a hovered card lifts, in canvas units.
const LIFT: f32 = 36.0;

/// How long the enemy shakes when hit, how long each side's turn of it lasts, in seconds, and how
/// far it first swings, in canvas units.
const SHAKE: f32 = 0.35;
const SHAKE_STEP: f32 = 0.05;
const SWAY: f32 = 20.0;

/// The numbers and hand the UI shows, small enough to copy out each frame without allocating.
#[derive(Clone, Copy, Default)]
struct Shown {
    player_hp: i32,
    player_block: i32,
    energy: i32,
    enemy_hp: i32,
    enemy_poison: i32,
    enemy_block: i32,
    intent: Intent,
    outcome: Outcome,
    turn: u32,
    wins: u32,
    losses: u32,
    deck: usize,
    discard: usize,
    hand: [Option<Card>; HAND],
}

impl Shown {
    fn of(fight: &Fight) -> Self {
        let mut hand = [None; HAND];
        for (slot, card) in hand.iter_mut().zip(&fight.hand) {
            *slot = Some(*card);
        }
        Self {
            player_hp: fight.player_hp,
            player_block: fight.player_block,
            energy: fight.energy,
            enemy_hp: fight.enemy_hp,
            enemy_poison: fight.enemy_poison,
            enemy_block: fight.enemy_block,
            intent: fight.intent,
            outcome: fight.outcome,
            turn: fight.turn,
            wins: fight.wins,
            losses: fight.losses,
            deck: fight.deck.len(),
            discard: fight.discard.len(),
            hand,
        }
    }
}

/// The game's layer.
struct Game {
    scripts: Scripts,
    commands: Vec<Command>,
    ui: Option<GameUi>,
    /// What the UI shows of the fight, copied out each frame without allocating.
    shown: Shown,
    /// The enemy's health in the last frame, to tell a hit.
    enemy_was: Option<i32>,
    /// Seconds of shaking left.
    shake: f32,
    /// Whether the last frame showed a result screen.
    was_over: bool,
}

impl Game {
    fn new() -> Self {
        Self {
            scripts: Scripts::new().expect("Luau starts"),
            commands: Vec::new(),
            ui: None,
            shown: Shown::default(),
            enemy_was: None,
            shake: 0.0,
            was_over: false,
        }
    }
}

impl Layer for Game {
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // First: the script names the type.
        cx.world_mut().register::<Fight>()?;
        self.scripts.load(cx, root.join("scripts/effects.luau"))?;
        self.ui = Some(cx.add_ui("fight", root.join("ui/fight.ui"))?);
        Ok(())
    }

    fn attach(&mut self, cx: &mut Context) {
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        if let Some(fight) = world.query_mut::<&mut Fight>().next()
            && fight.turn == 0
        {
            fight.start();
        }
        for command in self.commands.drain(..) {
            if let Some(fight) = world.query_mut::<&mut Fight>().next() {
                match command {
                    Command::Play { slot } => {
                        fight.play(slot as usize);
                    }
                    Command::EndTurn => fight.end_turn(),
                    Command::PlayAgain => {
                        if fight.outcome != Outcome::Fighting {
                            fight.start();
                        }
                    }
                }
            }
        }
        // Each tick, so a fight ended by an edit ends as one played out does.
        if let Some(fight) = world.query_mut::<&mut Fight>().next() {
            fight.settle();
        }
        self.scripts.run(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, dt: std::time::Duration) {
        let Some(ui) = self.ui else {
            return;
        };
        if let Some(fight) = cx.world().query::<&Fight>().next() {
            self.shown = Shown::of(fight);
        }
        let fight = self.shown;
        if self.enemy_was.is_some_and(|was| fight.enemy_hp < was) {
            self.shake = SHAKE;
        }
        self.enemy_was = Some(fight.enemy_hp);
        self.shake = (self.shake - dt.as_secs_f32()).max(0.0);
        // Left, right, left..., each swing smaller, then still.
        let step = (self.shake / SHAKE_STEP) as u32;
        let sway = self.shake / SHAKE * SWAY;
        let sway = if step.is_multiple_of(2) { -sway } else { sway };
        let _ = ui.set_offset(cx, "enemy_panel", Vec2::new(sway, 0.0));
        let _ = if fight.enemy_block > 0 {
            ui.set_text_fmt(
                cx,
                "enemy",
                format_args!(
                    "ENEMY  {} HP   BLOCK {}",
                    fight.enemy_hp.max(0),
                    fight.enemy_block
                ),
            )
        } else {
            ui.set_text_fmt(
                cx,
                "enemy",
                format_args!("ENEMY  {} HP", fight.enemy_hp.max(0)),
            )
        };
        let _ = ui.set_text_fmt(cx, "intent", format_args!("NEXT TURN: {}", fight.intent));
        let _ = ui.set_visible(cx, "poison", fight.enemy_poison > 0);
        let _ = ui.set_text_fmt(cx, "poison", format_args!("POISON {}", fight.enemy_poison));
        let _ = ui.set_text_fmt(
            cx,
            "log",
            format_args!(
                "TURN {}   WINS {}   LOSSES {}",
                fight.turn, fight.wins, fight.losses
            ),
        );
        let _ = ui.set_text_fmt(
            cx,
            "player",
            format_args!(
                "YOU  {} HP   BLOCK {}",
                fight.player_hp.max(0),
                fight.player_block
            ),
        );
        let _ = ui.set_text_fmt(cx, "energy", format_args!("ENERGY {}", fight.energy));
        let _ = ui.set_text_fmt(
            cx,
            "piles",
            format_args!("DECK {}   DISCARD {}", fight.deck, fight.discard),
        );
        let hovered = ui.hovered(cx);
        let over = fight.outcome != Outcome::Fighting;
        let _ = ui.set_visible(cx, "victory", fight.outcome == Outcome::Won);
        let _ = ui.set_visible(cx, "defeat", fight.outcome == Outcome::Lost);
        if over && !self.was_over {
            let _ = ui.set_focus(
                cx,
                if fight.outcome == Outcome::Won {
                    "again_win"
                } else {
                    "again_lose"
                },
            );
        }
        self.was_over = over;
        for (slot, name) in CARDS.iter().enumerate() {
            let card = fight.hand[slot];
            // A card the player can't pay for stays down; one under a result screen isn't hovered.
            let affordable = card.is_some_and(|card| card.cost() <= fight.energy);
            let over_it = hovered
                .as_deref()
                .is_some_and(|hovered| hovered == *name || hovered == SLOTS[slot]);
            let lift = if affordable && over_it { LIFT } else { 0.0 };
            let _ = ui.set_offset(cx, name, Vec2::new(0.0, -lift));
            let _ = ui.set_visible(cx, SLOTS[slot], card.is_some());
            let _ = ui.set_visible(cx, name, card.is_some());
            if let Some(card) = card {
                let _ = ui.set_text_fmt(cx, name, card);
                let _ = ui.set_disabled(cx, name, !affordable);
            }
        }
        ui.draw(cx);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies it.
        if let Some(ui) = self.ui
            && let Some(UiEvent::Pressed(button)) = ui.event(cx, event)
        {
            if &*button == "end_turn" {
                self.commands.push(Command::EndTurn);
            } else if &*button == "again_win" || &*button == "again_lose" {
                self.commands.push(Command::PlayAgain);
            } else if let Some(slot) = CARDS.iter().position(|name| *name == &*button) {
                self.commands.push(Command::Play { slot: slot as u32 });
            }
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start scene saves to its own bytes, so a `gg scene save` changes only what was edited.
    #[test]
    fn the_scene_saves_as_it_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut cx = Context::seeded(1).root(dir);
        Game::new().load(&mut cx).expect("loaded");
        let path = dir.join("scenes/main.ron");
        let text = std::fs::read_to_string(&path).expect("the scene");
        let roots = cx.load_scene_file(path).expect("the scene loads");
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text);
    }
}
