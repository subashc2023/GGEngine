//! cards, a game made with GGEngine: one turn-based card fight.
//!
//! Its start scene holds the fight. The UI is `ui/fight.ui`; click a card to play it, End Turn to
//! let the enemy attack. Card effects run in `scripts/effects.luau`.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

mod fight;

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;

use gg_engine::prelude::*;
use gg_engine::{GameUi, UiEvent};

use fight::{Card, Fight, HAND};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("cards")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).dev_panels()
}

/// What the UI's presses do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Plays the hand's card at `slot`.
    Play {
        slot: u32,
    },
    EndTurn,
}

/// The card buttons' names, by hand slot.
const CARDS: [&str; HAND] = ["card0", "card1", "card2", "card3", "card4"];

/// The numbers and hand the UI shows, small enough to copy out each frame without allocating.
#[derive(Clone, Copy, Default)]
struct Shown {
    player_hp: i32,
    player_block: i32,
    energy: i32,
    enemy_hp: i32,
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
}

impl Game {
    fn new() -> Self {
        Self {
            scripts: Scripts::new().expect("Luau starts"),
            commands: Vec::new(),
            ui: None,
            shown: Shown::default(),
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
                }
            }
            if let Some(fight) = world.query_mut::<&mut Fight>().next() {
                fight.settle();
            }
        }
        self.scripts.run(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn update(&mut self, cx: &mut Context, _: std::time::Duration) {
        let Some(ui) = self.ui else {
            return;
        };
        if let Some(fight) = cx.world().query::<&Fight>().next() {
            self.shown = Shown::of(fight);
        }
        let fight = self.shown;
        let _ = ui.set_text_fmt(
            cx,
            "enemy",
            format_args!("ENEMY  {} HP", fight.enemy_hp.max(0)),
        );
        let _ = ui.set_text_fmt(
            cx,
            "intent",
            format_args!("ATTACKS FOR {}", fight::ENEMY_ATTACK),
        );
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
            format_args!("YOU  {} HP   BLOCK {}", fight.player_hp, fight.player_block),
        );
        let _ = ui.set_text_fmt(cx, "energy", format_args!("ENERGY {}", fight.energy));
        let _ = ui.set_text_fmt(
            cx,
            "piles",
            format_args!("DECK {}   DISCARD {}", fight.deck, fight.discard),
        );
        for (slot, name) in CARDS.iter().enumerate() {
            let card = fight.hand[slot];
            let _ = ui.set_visible(cx, name, card.is_some());
            if let Some(card) = card {
                let _ = ui.set_text_fmt(cx, name, card);
                let _ = ui.set_disabled(cx, name, card.cost() > fight.energy);
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
