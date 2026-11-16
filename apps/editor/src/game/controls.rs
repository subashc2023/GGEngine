//! The title bar's run controls, Play, Simulate, Stop and Step, and what the title bar says.

use gg_api::InputEvent;
use serde_json::Value;

use crate::overlay::{self, Mode};

use super::{Game, Grab, RunState};

impl Game {
    /// How the game runs, as the title bar shows it: stopped, or as good as, once the editor's
    /// Stop is asked; or playing, Play through the game's camera, Simulate through the editor's.
    pub(super) fn mode(&self) -> Mode {
        if self.stopped || self.stopping {
            Mode::Edit
        } else if self.view.toolbar.world_camera {
            Mode::Play
        } else {
            Mode::Simulate
        }
    }

    /// Whether the view's input goes to the game: in Play alone.
    pub(super) fn through(&self) -> bool {
        self.mode() == Mode::Play
    }

    /// Does what a click of the title bar's controls asks, while a game serves, as
    /// [`run`](Self::run) and [`step`](Self::step) say, or pauses or resumes it. Play, and Resume
    /// in Play, capture the view's input for the game, as the module says; any other click ends
    /// the capture.
    pub(super) fn click(&mut self, clicked: overlay::Clicked) {
        let resumes_play = clicked == overlay::Clicked::Resume && self.mode() == Mode::Play;
        let captures = clicked == overlay::Clicked::To(Mode::Play) || resumes_play;
        // What Escape let go of, a capture grabs again; any other click ends a capture.
        self.grab = match (captures, self.grab) {
            (true, _) => Grab::Captured,
            (false, Grab::Captured) => Grab::Asks,
            (false, grab) => grab,
        };
        match clicked {
            overlay::Clicked::To(to) => self.run(to),
            overlay::Clicked::Pause => self.ask("pause"),
            overlay::Clicked::Resume => self.ask("resume"),
            overlay::Clicked::Step => self.step(),
        }
    }

    /// Asks the game `method`, without params, while one serves.
    fn ask(&self, method: &str) {
        if let Some(talker) = &self.talker {
            talker.ask(method, Value::Null);
        }
    }

    /// Does what a click of the title bar's `to` asks, while a game serves, whatever the editor
    /// last knew of how it runs: Play and Simulate ask the game to play, as
    /// [`play`](Self::play) says, a stopped game resuming first if it's paused, as Play plays;
    /// Stop asks it to stop, and stops the view's input going to it at once, dropping what wasn't
    /// sent yet but its letting go, until it answers.
    pub(super) fn run(&mut self, to: Mode) {
        if self.talker.is_none() {
            return;
        }
        match to {
            Mode::Play | Mode::Simulate => {
                if self.mode() == Mode::Edit && self.paused {
                    self.ask("resume");
                }
                self.play(to);
            }
            Mode::Edit => {
                self.ask("stop");
                self.stopping = true;
                self.drop_unsent();
            }
        }
    }

    /// Asks the game to play, which changes nothing while it plays, so that a switch between Play
    /// and Simulate carries on, and turns the view to the game's camera for `to`'s Play, or the
    /// editor's for Simulate.
    fn play(&mut self, to: Mode) {
        self.ask("play");
        self.playing += 1;
        self.view.toolbar.world_camera = to == Mode::Play;
        self.adopting = false;
    }

    /// Asks the game for a step, a tick, which pauses it; a stopped game is first paused, then
    /// played, as Play does, or Simulate while the view sees through the editor's camera, so that
    /// no frame plays it in real time before the step.
    fn step(&mut self) {
        if self.talker.is_none() {
            return;
        }
        if self.mode() == Mode::Edit {
            self.ask("pause");
            let world_camera = self.view.toolbar.world_camera;
            self.play(if world_camera {
                Mode::Play
            } else {
                Mode::Simulate
            });
        }
        self.ask("step");
    }

    /// Drops the input meant for the game that wasn't sent, but its letting go of what's held,
    /// which would stay held through a Stop otherwise, the game's input not being the world's.
    pub(super) fn drop_unsent(&mut self) {
        self.input
            .retain(|event| *event == InputEvent::Focused(false));
    }

    /// What the title bar says: while a game serves, how it runs, as [`mode`](Self::mode) says,
    /// and whether it's paused; otherwise how the run goes, or that it waits for the run before it
    /// to end, as [`start`](Self::start) does.
    pub(super) fn label(&self) -> String {
        if self.dir.is_none() {
            return "No project open".to_owned();
        }
        if self.waiting {
            return "Waiting for the last run to end…".to_owned();
        }
        let running = match self.mode() {
            Mode::Edit => "Stopped",
            Mode::Play => "Playing",
            Mode::Simulate => "Simulating",
        };
        match &self.run_state {
            // Its status waits too, so what it last said may be from before.
            RunState::Running if self.debugger.stopped() => {
                "Waiting at a script's stop: see the Debugger".to_owned()
            }
            RunState::Running if self.paused => format!("{running}, paused"),
            RunState::Running => running.to_owned(),
            RunState::Building(None) => "Building…".to_owned(),
            RunState::Building(Some(at)) => format!("Building… {at}"),
            RunState::Starting => "Starting…".to_owned(),
            RunState::Rebuilding(None) => "Rebuilding…".to_owned(),
            RunState::Rebuilding(Some(at)) => format!("Rebuilding… {at}"),
            RunState::Restarting => "Restarting…".to_owned(),
            RunState::Waiting => "Waiting for a rebuild".to_owned(),
            RunState::Ended(why) => format!("Ended: {why}"),
        }
    }
}
