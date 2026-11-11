//! The frame rates the view shows at its bottom right while the settings' "Show frame rates" is on:
//! the game's, as it publishes frames to the view's memory, and the editor's own, each over the
//! last second. The Performance panel times the editor's frames alone, and the game, which renders
//! a frame once the editor has taken its last, goes no faster and may go slower: what depends on
//! the game's frame rate, as antialiasing's settling does, goes by the game's here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gg_engine::egui::{self, Align2, Response, TextStyle};
use gg_engine::{Context, Ui};
use serde_json::{Value, json};
use tracing::warn;

use crate::overlay;

/// The file the editor keeps whether the rates show in, as [`Context::read_state`] names it.
const FILE: &str = "frame_rates.json";

/// How far back the rates count frames.
const OVER: Duration = Duration::from_secs(1);

/// What the settings' switch is, which its tooltip says.
const TIP: &str = "Show at the view's bottom right how many frames a second the game renders, and \
                   how long each takes, beside the editor's own, over the last second";

/// The frame rates, and whether the view shows them.
#[derive(Default)]
pub struct Rates {
    /// Whether the view shows them.
    pub shown: bool,
    /// The last second's frames of the editor: when each updated, and how many frames the game had
    /// published to the view by then, while a game's view was published.
    seen: VecDeque<(Instant, Option<u64>)>,
}

impl Rates {
    /// Reads whether they show, as `cx` keeps it for the user, warning (`editor.frame_rates`) if it
    /// can't, and then not showing them.
    pub fn read(&mut self, cx: &Context) {
        let read = match cx.read_state(FILE) {
            Ok(None) => return,
            Ok(Some(bytes)) => serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()),
            Err(error) => Err(format!("{error:#}")),
        };
        match read.map(|value| value["shown"].as_bool()) {
            Ok(Some(shown)) => self.shown = shown,
            Ok(None) => warn!(
                code = "editor.frame_rates",
                file = FILE,
                "can't read whether the frame rates show: no `shown`; they don't"
            ),
            Err(why) => warn!(
                code = "editor.frame_rates",
                file = FILE,
                "can't read whether the frame rates show: {why}; they don't"
            ),
        }
    }

    /// Notes an editor frame at `now`, the game having published `published` frames to the view by
    /// then, while a game's view is published, and forgets those more than [`OVER`] before.
    pub fn see(&mut self, now: Instant, published: Option<u64>) {
        self.seen.push_back((now, published));
        while (self.seen.front()).is_some_and(|&(at, _)| now.duration_since(at) > OVER) {
            self.seen.pop_front();
        }
    }

    /// The editor's frames a second over the frames noted, and the game's: `None` before two
    /// frames, and the game's while no game's view was published at the first and the last, or
    /// where the count went back, as it does for a new view.
    pub fn rates(&self) -> (Option<f64>, Option<f64>) {
        let (Some(&(first, from)), Some(&(last, to))) = (self.seen.front(), self.seen.back())
        else {
            return (None, None);
        };
        let seconds = last.duration_since(first).as_secs_f64();
        if seconds <= 0.0 {
            return (None, None);
        }
        let editor = (self.seen.len() - 1) as f64 / seconds;
        let game = from.zip(to).filter(|(from, to)| to >= from);
        (
            Some(editor),
            game.map(|(from, to)| (to - from) as f64 / seconds),
        )
    }

    /// Adds the switch that shows them to the title bar's settings, keeping what it's set to for
    /// the user, warning (`editor.frame_rates`) if it can't.
    pub fn settings(&mut self, ui: &mut Ui<'_>, cx: &Context) {
        let mut changed = false;
        ui.settings(|ui| {
            let switch = ui.checkbox(&mut self.shown, "Show frame rates");
            changed = switch.on_hover_text(TIP).changed();
        });
        let text = format!("{:#}\n", json!({"shown": self.shown}));
        if changed && let Err(error) = cx.save_state(FILE, text.as_bytes()) {
            warn!(code = "editor.frame_rates", file = FILE, "{error:#}");
        }
    }

    /// Shows them at the bottom right of `view`, while they show, as [`overlay::label`] does.
    pub fn paint(&self, egui: &egui::Context, view: &Response) {
        if self.shown {
            let text = text(self.rates());
            overlay::label(egui, view, Align2::RIGHT_BOTTOM, text, TextStyle::Monospace);
        }
    }
}

/// What the view says of the editor's frames a second and the game's, as [`Rates::rates`] gives
/// them, each with its frame time.
fn text((editor, game): (Option<f64>, Option<f64>)) -> String {
    let rate = |rate: Option<f64>| match rate {
        Some(rate) if rate > 0.0 => format!("{rate:5.1} fps {:6.2} ms", 1000.0 / rate),
        Some(_) => "no frames".to_owned(),
        None => "-".to_owned(),
    };
    format!("Game   {}\nEditor {}", rate(game), rate(editor))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of frames `every` apart from `start`, the game publishing a frame each `per` of
    /// them, as `published` counts, or none.
    fn second(every: Duration, per: Option<u64>) -> Rates {
        let mut rates = Rates::default();
        let start = Instant::now();
        let frames = (OVER.as_nanos() / every.as_nanos()) as u32;
        for i in 0..=frames {
            let published = per.map(|per| 100 + u64::from(i) / per);
            rates.see(start + every * i, published);
        }
        rates
    }

    #[test]
    fn the_rates_count_the_last_seconds_frames_of_the_editor_and_the_game() {
        let (editor, game) = second(Duration::from_millis(4), Some(1)).rates();
        assert_eq!((editor, game), (Some(250.0), Some(250.0)));
        // The game a frame in four of the editor's.
        let (editor, game) = second(Duration::from_millis(4), Some(4)).rates();
        assert_eq!((editor, game), (Some(250.0), Some(62.0)));
        // No game's view.
        assert_eq!(
            second(Duration::from_millis(10), None).rates(),
            (Some(100.0), None)
        );
        // Too few frames to say.
        assert_eq!(Rates::default().rates(), (None, None));
        let mut one = Rates::default();
        one.see(Instant::now(), Some(1));
        assert_eq!(one.rates(), (None, None));
    }

    #[test]
    fn frames_older_than_a_second_are_forgotten_and_a_new_view_counts_afresh() {
        let mut rates = second(Duration::from_millis(10), Some(1));
        let (&(last, _), len) = (rates.seen.back().expect("seen"), rates.seen.len());
        // Two seconds on, the old ones are gone.
        rates.see(last + 2 * OVER, Some(5));
        assert_eq!(rates.seen.len(), 1, "of {len}");
        let mut rates = second(Duration::from_millis(10), Some(1));
        rates.see(last + Duration::from_millis(10), Some(0));
        assert_eq!(rates.rates().1, None);
    }

    #[test]
    fn the_text_gives_each_rate_and_its_frame_time() {
        assert_eq!(
            text((Some(240.0), Some(60.0))),
            "Game    60.0 fps  16.67 ms\nEditor 240.0 fps   4.17 ms"
        );
        assert_eq!(
            text((Some(240.0), None)),
            "Game   -\nEditor 240.0 fps   4.17 ms"
        );
        assert_eq!(
            text((Some(240.0), Some(0.0))).lines().next(),
            Some("Game   no frames")
        );
    }
}
