//! The Debugger panel: the game's Luau scripts, debugged through the tool API's `debug`.
//!
//! It has its own connection, since a stopped script holds up the game's main thread, which answers
//! the editor's other requests. It shows:
//!
//! - the source of a script the game runs, from the asset root. A click on a line's number sets or
//!   clears a breakpoint. The breakpoints are the editor's, given to each game as it serves.
//! - while a script is stopped: the stack, the function stopped in first; the picked function's
//!   locals and upvalues, and its source at its line; Continue, Pause and the steps.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use gg_api::{Client, DebugAction, Debugging, Failure, Session, StopReason, Variable};
use gg_engine::egui::{self, Align, Color32, RichText, ScrollArea};
use serde_json::{Value, json};

/// How often the panel polls the scripts' state while it asks nothing else.
const POLL: Duration = Duration::from_millis(250);

/// How long the connection waits to connect and for each answer, which the game's connection thread
/// gives at once.
const WAIT: Duration = Duration::from_secs(10);

/// The source's share of the panel's width; the stack and its variables take the rest.
const SOURCE: f32 = 0.6;

/// The color of a breakpoint's mark and of the line stopped at.
const MARK: Color32 = Color32::from_rgb(220, 70, 60);
const STOPPED: Color32 = Color32::from_rgba_premultiplied(90, 80, 20, 90);

/// The Debugger panel. Its breakpoints are the editor's, by script, and its asset root names
/// scripts as the game does.
pub(crate) struct Debugger {
    root: Option<PathBuf>,
    client: String,
    breakpoints: BTreeMap<String, BTreeSet<u32>>,
    link: Option<Link>,
    seen: Option<Debugging>,
    /// The script shown, as the game names it, and its lines as last read.
    shown: Option<(String, Vec<String>)>,
    /// The function picked, by its place in the stack, and its stop's number, at which the source
    /// scrolls to its line.
    picked: usize,
    scrolled: u64,
    /// Why the game refused the last request, until another is answered.
    refused: Option<String>,
}

/// A connection to the game on its own thread: requests sent in order, and their answers in order.
struct Link {
    asks: Sender<Value>,
    answers: Receiver<Result<Debugging, Failure>>,
}

impl Debugger {
    /// A panel for the game whose asset root is `root`, `None` outside a project, talking to it as
    /// `client`.
    pub(crate) fn new(root: Option<PathBuf>, client: &str) -> Self {
        Self {
            root,
            client: client.to_owned(),
            breakpoints: BTreeMap::new(),
            link: None,
            seen: None,
            shown: None,
            picked: 0,
            scrolled: 0,
            refused: None,
        }
    }

    /// Connects to `session`'s game in place of the ended one, giving it the breakpoints.
    pub(crate) fn serve(&mut self, session: Session) {
        self.reach(connect(session, self.client.clone()));
    }

    /// Talks through `link` instead, giving the game the breakpoints, which a restarted game starts
    /// without.
    fn reach(&mut self, link: Link) {
        self.disconnect();
        for (script, lines) in &self.breakpoints {
            let _ = link.asks.send(breakpoints(script, lines));
        }
        self.link = Some(link);
    }

    /// Drops the gone game's connection and its last answers.
    pub(crate) fn disconnect(&mut self) {
        self.link = None;
        self.seen = None;
        self.refused = None;
    }

    /// Takes the game's answers since.
    pub(crate) fn update(&mut self) {
        let Some(link) = &self.link else {
            return;
        };
        let answers: Vec<_> = link.answers.try_iter().collect();
        for answer in answers {
            match answer {
                Ok(seen) => {
                    self.refused = None;
                    self.seen = Some(seen);
                }
                Err(Failure { code, message }) => {
                    let broke = matches!(code.as_str(), "api.closed" | "api.timeout");
                    self.refused = Some(format!("[{code}] {message}"));
                    if broke {
                        self.link = None;
                        self.seen = None;
                    }
                }
            }
        }
    }

    /// Sets or clears the breakpoint at `line` of `script`, and tells the game.
    fn toggle(&mut self, script: &str, line: u32) {
        let lines = self.breakpoints.entry(script.to_owned()).or_default();
        if !lines.remove(&line) {
            lines.insert(line);
        }
        let ask = breakpoints(script, lines);
        if lines.is_empty() {
            self.breakpoints.remove(script);
        }
        self.ask(ask);
    }

    fn act(&mut self, action: DebugAction) {
        self.ask(json!({ "action": action }));
    }

    fn ask(&self, params: Value) {
        if let Some(link) = &self.link {
            let _ = link.asks.send(params);
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let serves = self.link.is_some();
        let stopped = self.seen.as_ref().and_then(|seen| seen.stopped.clone());
        let mut act = None;
        ui.horizontal(|ui| {
            let buttons = [
                (DebugAction::Continue, "Continue", "Go on to the next stop"),
                (
                    DebugAction::Pause,
                    "Pause",
                    "Stop the next line a script runs",
                ),
                (
                    DebugAction::StepOver,
                    "Step Over",
                    "Stop at the next line of this function, past the calls it makes",
                ),
                (
                    DebugAction::StepInto,
                    "Step Into",
                    "Stop at the next line that runs, in a function it calls too",
                ),
                (
                    DebugAction::StepOut,
                    "Step Out",
                    "Stop at the next line once this function returns",
                ),
            ];
            for (action, label, tip) in buttons {
                let enabled = serves && (stopped.is_some() != (action == DebugAction::Pause));
                let button = ui.add_enabled(enabled, egui::Button::new(label));
                if button.on_hover_text(tip).clicked() {
                    act = Some(action);
                }
            }
            ui.separator();
            ui.label(self.state(serves));
        });
        if let Some(action) = act {
            self.act(action);
        }
        if let Some(refused) = &self.refused {
            ui.colored_label(MARK, refused);
        }
        ui.separator();
        // A new stop picks the function stopped in and shows its script, read again, scrolled to
        // its line. A function picked since shows its own.
        let number = stopped.as_ref().map_or(0, |stop| stop.number);
        let fresh = number != 0 && number != self.scrolled;
        if fresh {
            self.picked = 0;
        }
        let frame = stopped
            .as_ref()
            .and_then(|stop| stop.frames.get(self.picked));
        let at = frame.map(|frame| (frame.script.clone(), frame.line));
        if let Some((script, _)) = &at
            && (fresh || self.shown.as_ref().is_none_or(|(shown, _)| shown != script))
        {
            self.show_script(script);
        }
        let scripts = (self.seen.as_ref()).map_or_else(Vec::new, |seen| seen.scripts.clone());
        if self.shown.is_none()
            && let Some(first) = scripts.first()
        {
            self.show_script(first);
        }
        // The source the wider, as its lines are.
        let size = ui.available_size();
        let wide = SOURCE * size.x;
        ui.horizontal_top(|ui| {
            let source = egui::Layout::top_down(Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(wide, size.y), source, |ui| {
                ui.set_width(wide);
                self.source(ui, &scripts, at.as_ref(), fresh);
            });
            ui.separator();
            ui.vertical(|ui| self.stack(ui, stopped.as_ref()));
        });
        if fresh {
            self.scrolled = number;
        }
    }

    /// Whether a script is stopped, as the game last answered.
    pub(crate) fn stopped(&self) -> bool {
        (self.seen.as_ref()).is_some_and(|seen| seen.stopped.is_some())
    }

    /// What the scripts are doing, as the game last answered.
    fn state(&self, serves: bool) -> String {
        let Some(seen) = self.seen.as_ref().filter(|_| serves) else {
            return "No game serves".to_owned();
        };
        if let Some(stop) = &seen.stopped {
            let why = match stop.why {
                StopReason::Breakpoint => "a breakpoint",
                StopReason::Step => "a step",
                StopReason::Pause => "a pause",
            };
            let at = (stop.frames.first()).map_or(String::new(), |frame| {
                format!(" at {}:{}", frame.script, frame.line)
            });
            return format!("Stopped{at}, for {why}: the game waits");
        }
        match seen.asked {
            Some(DebugAction::Pause) => "Pausing at the next line a script runs".to_owned(),
            Some(_) => "Stepping".to_owned(),
            None => "Running".to_owned(),
        }
    }

    /// Reads `script`'s source from the asset root to show.
    fn show_script(&mut self, script: &str) {
        let text = (self.root.as_ref()).and_then(|root| fs::read_to_string(root.join(script)).ok());
        let lines = text.map_or_else(
            || vec![format!("-- {script} can't be read from the project")],
            |text| text.lines().map(str::to_owned).collect(),
        );
        self.shown = Some((script.to_owned(), lines));
    }

    /// The source, with a pick of the game's scripts. A click on a line's number toggles its
    /// breakpoint. Line `at`, the picked function's, is lit, and scrolled to if `fresh`.
    fn source(
        &mut self,
        ui: &mut egui::Ui,
        scripts: &[String],
        at: Option<&(String, u32)>,
        fresh: bool,
    ) {
        let shown = self.shown.as_ref().map(|(script, _)| script.clone());
        let mut picked = shown.clone();
        let reload = ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("debugger-script")
                .selected_text(shown.as_deref().unwrap_or("No script"))
                .show_ui(ui, |ui| {
                    for script in scripts {
                        ui.selectable_value(&mut picked, Some(script.clone()), script);
                    }
                });
            let reload = ui.small_button("Reload");
            reload
                .on_hover_text("Read the script again, as saved since")
                .clicked()
        });
        // Read again as it's picked, too, so that a save since shows.
        if let Some(script) = picked.filter(|picked| reload.inner || Some(picked) != shown.as_ref())
        {
            self.show_script(&script);
        }
        let Some((script, lines)) = &self.shown else {
            return;
        };
        let marked = self.breakpoints.get(script);
        let placed: BTreeSet<u32> = (self.seen.iter())
            .flat_map(|seen| &seen.breakpoints)
            .filter(|breakpoint| breakpoint.script == *script)
            .filter_map(|breakpoint| breakpoint.placed)
            .collect();
        let stopped_line = at.filter(|(at, _)| at == script).map(|&(_, line)| line);
        let mut toggled = None;
        ScrollArea::both()
            .id_salt("debugger-source")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (index, text) in lines.iter().enumerate() {
                    let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
                    let row = ui.horizontal(|ui| {
                        let asked = marked.is_some_and(|lines| lines.contains(&line));
                        let mark = if asked { "●" } else { " " };
                        let color = if placed.contains(&line) || asked {
                            MARK
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        let number = RichText::new(format!("{mark}{line:>4}"))
                            .monospace()
                            .color(color);
                        let tip = if asked {
                            "Clear the breakpoint"
                        } else {
                            "Set a breakpoint, at this line or the next with code"
                        };
                        let gutter = egui::Label::new(number).sense(egui::Sense::click());
                        let gutter = ui.add(gutter).on_hover_text(tip);
                        if gutter.clicked() {
                            toggled = Some(line);
                        }
                        ui.label(RichText::new(text).monospace());
                        gutter
                    });
                    if stopped_line == Some(line) {
                        let rect = row.response.rect;
                        ui.painter().rect_filled(rect, 2.0, STOPPED);
                        // To the line's number, which keeps the source scrolled to its left.
                        if fresh {
                            row.inner.scroll_to_me(Some(Align::Center));
                        }
                    }
                }
            });
        if let Some(line) = toggled {
            let script = script.clone();
            self.toggle(&script, line);
        }
    }

    /// `stopped`'s stack, a click picking a function, and the picked one's locals and upvalues.
    fn stack(&mut self, ui: &mut egui::Ui, stopped: Option<&gg_api::Stop>) {
        let Some(stop) = stopped else {
            ui.weak("No script has stopped: set a breakpoint, or pause");
            return;
        };
        ui.strong("Stack");
        for (at, frame) in stop.frames.iter().enumerate() {
            let text = format!("{} at {}:{}", frame.function, frame.script, frame.line);
            if ui.selectable_label(at == self.picked, text).clicked() {
                self.picked = at;
            }
        }
        let Some(frame) = stop.frames.get(self.picked) else {
            return;
        };
        ScrollArea::vertical()
            .id_salt("debugger-variables")
            .auto_shrink(false)
            .show(ui, |ui| {
                variables(ui, "Locals", &frame.locals);
                variables(ui, "Upvalues", &frame.upvalues);
            });
    }
}

/// `variables` under `title`: each name, and its value as JSON.
fn variables(ui: &mut egui::Ui, title: &str, variables: &[Variable]) {
    ui.add_space(4.0);
    ui.strong(title);
    if variables.is_empty() {
        ui.weak("None");
        return;
    }
    egui::Grid::new(title).striped(true).show(ui, |ui| {
        for variable in variables {
            ui.monospace(&variable.name);
            let value = match &variable.value {
                Value::String(text) => text.clone(),
                value => value.to_string(),
            };
            ui.monospace(value);
            ui.end_row();
        }
    });
}

/// The `debug` request that sets `script`'s breakpoints at `lines`.
fn breakpoints(script: &str, lines: &BTreeSet<u32>) -> Value {
    json!({ "breakpoints": { "script": script, "lines": lines } })
}

/// Connects to `session`'s game as `client`, on its own thread. The thread sends each request in
/// turn, else polls the scripts every [`POLL`], and passes each answer on, until the connection
/// breaks or the panel drops it.
fn connect(session: Session, client: String) -> Link {
    let (asks, asked) = mpsc::channel::<Value>();
    let (answer, answers) = mpsc::channel();
    thread::spawn(move || {
        let connected = Client::connect(&session, &client, WAIT);
        let mut client = match connected {
            Ok(client) => client,
            Err(failure) => {
                let _ = answer.send(Err(failure));
                return;
            }
        };
        loop {
            let params = match asked.recv_timeout(POLL) {
                Ok(params) => params,
                Err(RecvTimeoutError::Timeout) => json!({}),
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let answered = client.call_as::<Debugging>("debug", params);
            let broke = (answered.as_ref()).is_err_and(|failure| {
                matches!(failure.code.as_str(), "api.closed" | "api.timeout")
            });
            if answer.send(answered).is_err() || broke {
                return;
            }
        }
    });
    Link { asks, answers }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel whose connection is a pair of channels, the game's ends returned.
    fn linked() -> (
        Debugger,
        Receiver<Value>,
        Sender<Result<Debugging, Failure>>,
    ) {
        let mut panel = Debugger::new(None, "editor");
        let (asks, asked) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        panel.link = Some(Link { asks, answers });
        (panel, asked, answer)
    }

    /// Toggling a breakpoint sends the game the script's lines each time, and the editor keeps
    /// them. A click's action goes to the game; an answer is taken, and a refusal shown.
    #[test]
    fn breakpoints_and_actions_go_to_the_game_and_answers_come_back() {
        let (mut panel, asked, answer) = linked();
        panel.toggle("scripts/spin.luau", 10);
        panel.toggle("scripts/spin.luau", 12);
        panel.toggle("scripts/spin.luau", 10);
        let sent: Vec<Value> = asked.try_iter().collect();
        let lines =
            |lines: &[u32]| breakpoints("scripts/spin.luau", &lines.iter().copied().collect());
        assert_eq!(sent, [lines(&[10]), lines(&[10, 12]), lines(&[12])]);
        panel.toggle("scripts/spin.luau", 12);
        assert_eq!(asked.try_iter().collect::<Vec<_>>(), [lines(&[])]);
        assert!(panel.breakpoints.is_empty(), "none kept for the script");
        panel.act(DebugAction::StepOver);
        assert_eq!(asked.try_recv().ok(), Some(json!({"action": "step_over"})));
        let seen = Debugging {
            stopped: None,
            asked: Some(DebugAction::Pause),
            stops: 3,
            breakpoints: Vec::new(),
            scripts: vec!["scripts/spin.luau".to_owned()],
        };
        answer.send(Ok(seen.clone())).expect("sent");
        panel.update();
        assert_eq!(panel.seen.as_ref(), Some(&seen));
        assert_eq!(panel.state(true), "Pausing at the next line a script runs");
        let refused = Failure::new("script.not_stopped", "no script has stopped");
        answer.send(Err(refused)).expect("sent");
        panel.update();
        assert!(
            panel
                .refused
                .as_deref()
                .is_some_and(|refused| refused.contains("not_stopped"))
        );
        assert!(panel.link.is_some(), "a refusal keeps the connection");
        answer
            .send(Err(Failure::new("api.closed", "gone")))
            .expect("sent");
        panel.update();
        assert!(panel.link.is_none() && panel.seen.is_none());
    }

    /// The editor's breakpoints go to each game as it serves, as a restarted game starts without
    /// them.
    #[test]
    fn the_editors_breakpoints_go_to_each_game_as_it_serves() {
        let (mut panel, _asked, _answer) = linked();
        panel.toggle("scripts/a.luau", 3);
        panel.toggle("scripts/b.luau", 7);
        let (asks, asked) = mpsc::channel();
        let (_answer, answers) = mpsc::channel();
        panel.reach(Link { asks, answers });
        let sent: Vec<Value> = asked.try_iter().collect();
        assert_eq!(
            sent,
            [
                breakpoints("scripts/a.luau", &[3].into()),
                breakpoints("scripts/b.luau", &[7].into()),
            ]
        );
    }
}
