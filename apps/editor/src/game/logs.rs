//! The Logs panel: the lines it keeps, from `gg run` and the game, and how it shows them.

use std::collections::VecDeque;
use std::fmt::Write as _;

use gg_engine::egui::{self, Color32, RichText, ScrollArea, TextStyle, TextWrapMode};
use serde_json::Value;

use super::{Game, PLACES};

/// The most lines the Logs panel keeps, past which the oldest go.
pub(super) const LOGGED: usize = 5000;

/// A line of the Logs panel, shown as a row for each line of its text.
pub(super) struct Logged {
    pub(super) level: Level,
    pub(super) text: String,
    /// The row its first line has among those logged since the editor started, so [`show_logs`]
    /// finds the rows it shows without laying out the rest.
    line: usize,
}

/// How a line shows: `Debug`, for the game's debug and trace events, recedes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Game {
    pub(super) fn log(&mut self, level: Level, text: String) {
        if self.logs.len() == LOGGED {
            self.logs.pop_front();
        }
        let line = (self.logs.back()).map_or(0, |last| last.line + last.text.split('\n').count());
        self.logs.push_back(Logged { level, text, line });
    }

    /// Takes the game's events logged since, each a line as [`logged`] makes it, but not `gg run`'s
    /// notes of a failed patch or rebuild, which its own events show.
    pub(super) fn take_logged(&mut self) {
        let Some(talker) = &self.talker else {
            return;
        };
        let reads: Vec<Vec<Value>> = talker.logged.try_iter().collect();
        for event in reads.iter().flatten() {
            let code = event["code"].as_str();
            if event["client"] == "gg"
                && matches!(code, Some("gg.patch_failed" | "gg.rebuild_failed"))
            {
                continue;
            }
            let (level, text) = logged(event);
            self.log(level, text);
        }
    }
}

/// The line for `event`, an event of the game's JSON log, as the console shows it but for where
/// and when it was logged: `[code] message  name=value ...`, a string field quoted if it's empty
/// or has spaces. Its level is `Debug` for a debug or trace event.
fn logged(event: &Value) -> (Level, String) {
    let level = match event["level"].as_str() {
        Some("ERROR") => Level::Error,
        Some("WARN") => Level::Warn,
        Some("INFO") => Level::Info,
        _ => Level::Debug,
    };
    let mut text = match &event["code"] {
        Value::Null => String::new(),
        Value::String(code) => format!("[{code}] "),
        code => format!("[{code}] "),
    };
    text.push_str(event["message"].as_str().unwrap_or_default());
    let mut separator = "  ";
    for (name, value) in event.as_object().into_iter().flatten() {
        // `log.*` fields carry the metadata of events from the `log` crate.
        if PLACES.contains(&name.as_str()) || name.starts_with("log.") {
            continue;
        }
        let _ = match value {
            Value::String(value) if !value.is_empty() && !value.contains(char::is_whitespace) => {
                write!(text, "{separator}{name}={value}")
            }
            value => write!(text, "{separator}{name}={value}"),
        };
        separator = " ";
    }
    (level, text)
}

/// Shows `logs` in `ui`: a row to each line of their text, unwrapped, as rustc renders its errors,
/// the newest showing while scrolled to the bottom. Lays out only the rows in view: all the
/// [`LOGGED`] took 5 ms a frame in a debug build, its egui optimized.
pub(super) fn show_logs(ui: &mut egui::Ui, logs: &VecDeque<Logged>) {
    let first = logs.front().map_or(0, |logged| logged.line);
    let rows = (logs.back()).map_or(0, |last| last.line + last.text.split('\n').count() - first);
    let height = ui.text_style_height(&TextStyle::Monospace);
    ScrollArea::both()
        .stick_to_bottom(true)
        .auto_shrink(false)
        .show_rows(ui, height, rows, |ui, shown| {
            ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
            let (start, end) = (first + shown.start, first + shown.end);
            let from = logs.partition_point(|logged| logged.line <= start);
            for logged in (logs.range(from.saturating_sub(1)..)).take_while(|l| l.line < end) {
                let (skip, take) = (
                    start.saturating_sub(logged.line),
                    end - logged.line.max(start),
                );
                for line in logged.text.split('\n').skip(skip).take(take) {
                    let text = RichText::new(line).monospace();
                    let text = match logged.level {
                        Level::Debug => text.weak(),
                        Level::Info => text,
                        Level::Warn => text.color(Color32::from_rgb(230, 190, 60)),
                        Level::Error => text.color(Color32::from_rgb(240, 90, 80)),
                    };
                    ui.label(text);
                }
            }
        });
}
