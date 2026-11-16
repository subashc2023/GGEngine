//! Frame-rate caps for windowed runs, as [`Context::set_max_fps`] says, and, in development
//! builds, the frame rate limit among the title bar's settings, kept across runs.

use std::num::NonZeroU32;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(feature = "dev")]
use egui::{Button, DragValue};
#[cfg(feature = "dev")]
use gg_core::{Error, Result};
#[cfg(feature = "dev")]
use tracing::warn;

#[cfg(feature = "dev")]
use crate::{Context, Ui};

/// The file keeping the settings' limit for later runs, in RON: `Some(60)`, or `None` if uncapped.
#[cfg(feature = "dev")]
const KEPT: &str = "max_fps.ron";

/// Rates offered at a click, in frames a second.
#[cfg(feature = "dev")]
const RATES: [u32; 6] = [10, 30, 60, 120, 144, 240];

/// The field's most frames a second.
#[cfg(feature = "dev")]
const MOST: u32 = 1000;

#[cfg(feature = "dev")]
const TIP: &str = "The most frames a second this app renders, which a game it embeds, as the \
                   editor does, renders no more of; kept for its next runs, unless --max-fps sets \
                   another";

/// Holds frames to [`Context::set_max_fps`]'s rate by sleeping before each, as Unreal and Godot
/// do. Winit's `ControlFlow::WaitUntil` doesn't wake while the user moves or resizes the window
/// on Windows; a sleep paces those frames too. Input arriving mid-sleep waits one period at most.
#[derive(Default)]
pub(crate) struct Limiter {
    /// When the next frame is due, while capped.
    due: Option<Instant>,
}

impl Limiter {
    /// Sleeps until the next frame is due under `max`, if capped. Returns the wait.
    pub(crate) fn wait(&mut self, max: Option<NonZeroU32>) -> Duration {
        let Some(max) = max else {
            self.due = None;
            return Duration::ZERO;
        };
        let now = Instant::now();
        let due = self.due.unwrap_or(now);
        if due > now {
            thread::sleep(due - now);
        }
        let start = Instant::now();
        self.due = Some(next_due(due, start, Duration::from_secs(1) / max.get()));
        start - now
    }
}

/// When the frame after one due at `due`, started at `start`, is due. A period after `due`, so a
/// late sleep doesn't slow the rate; after `start` once that's a period or more past `due` (a
/// slow frame, a pause), so no burst follows to catch up.
fn next_due(due: Instant, start: Instant, period: Duration) -> Instant {
    let next = due + period;
    if start >= next { start + period } else { next }
}

/// Adds the frame rate limit to the title bar's settings ([`Ui::settings`]): Off or a rate at a
/// click, or, while capped, any rate from 1 in a field, as slow motion. A choice caps `cx`'s
/// frames from the next and is kept for [`kept`]; a field drag is kept as it ends.
#[cfg(feature = "dev")]
pub(crate) fn settings(ui: &mut Ui<'_>, cx: &mut Context) {
    let max = cx.max_fps();
    // What was chosen, and whether to keep it.
    let mut chosen = None;
    ui.settings(|ui| {
        ui.label("Frame rate limit").on_hover_text(TIP);
        ui.horizontal_wrapped(|ui| {
            if ui.add(Button::selectable(max.is_none(), "Off")).clicked() {
                chosen = Some((None, true));
            }
            for rate in RATES {
                let on = max.map(NonZeroU32::get) == Some(rate);
                if ui.add(Button::selectable(on, rate.to_string())).clicked() {
                    chosen = Some((NonZeroU32::new(rate), true));
                }
            }
        });
        if let Some(max) = max {
            ui.horizontal(|ui| {
                ui.label("Any rate");
                let mut rate = max.get();
                let field = DragValue::new(&mut rate).range(1..=MOST).suffix(" fps");
                let field = ui.add(field);
                if field.changed() || field.drag_stopped() {
                    chosen = Some((NonZeroU32::new(rate), !field.dragged()));
                }
            });
        }
    });
    if let Some((max, keep)) = chosen {
        cx.set_max_fps(max);
        if keep && let Err(error) = cx.save_state(KEPT, kept_text(max).as_bytes()) {
            warn!(code = error.code(), "{error:#}");
        }
    }
}

/// The limit the settings last kept. Uncapped if none, or, with an `engine.state` warning, if it
/// can't be read.
#[cfg(feature = "dev")]
pub(crate) fn kept(cx: &Context) -> Option<NonZeroU32> {
    let read = cx.read_state(KEPT).and_then(|bytes| match bytes {
        Some(bytes) => read_kept(&bytes),
        None => Ok(None),
    });
    read.unwrap_or_else(|error| {
        warn!(code = error.code(), "{error:#}; frames go uncapped");
        None
    })
}

#[cfg(feature = "dev")]
fn kept_text(max: Option<NonZeroU32>) -> String {
    match max {
        Some(max) => format!("Some({max})"),
        None => "None".to_owned(),
    }
}

#[cfg(feature = "dev")]
fn read_kept(bytes: &[u8]) -> Result<Option<NonZeroU32>> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string());
    let max = text.and_then(|text| ron::from_str(text).map_err(|e| e.to_string()));
    max.map_err(|problem| {
        let message = format!("can't read the frame rate limit kept in {KEPT}: {problem}");
        Error::new("engine.state", message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capped_frame_is_due_a_share_of_a_second_after_the_last_was_due() {
        let (due, period) = (Instant::now(), Duration::from_millis(16));
        let ms = Duration::from_millis;
        // A frame that started late, as a sleep ends late, keeps the rate.
        assert_eq!(next_due(due, due, period), due + period);
        assert_eq!(next_due(due, due + ms(1), period), due + period);
        assert_eq!(next_due(due, due + ms(15), period), due + period);
        // One a whole period late, after a slow frame or a pause, counts from its own start.
        assert_eq!(next_due(due, due + period, period), due + 2 * period);
        assert_eq!(next_due(due, due + ms(500), period), due + ms(516));
    }

    #[test]
    fn a_limiter_waits_out_the_frames_share_of_a_second_while_capped() {
        let mut limiter = Limiter::default();
        let hundred = NonZeroU32::new(100);
        // The first frame is due at once, and the next a hundredth of a second after.
        let first = Instant::now();
        assert!(limiter.wait(hundred) < Duration::from_millis(1));
        let waited = limiter.wait(hundred);
        assert!(first.elapsed() >= Duration::from_millis(10));
        assert!(waited >= Duration::from_millis(9), "{waited:?}");
        // Uncapped, no frame waits, nor the first after a cap comes back.
        assert_eq!(limiter.wait(None), Duration::ZERO);
        assert_eq!(limiter.wait(None), Duration::ZERO);
        assert!(limiter.wait(hundred) < Duration::from_millis(1));
    }

    /// Runs frame `frame` in a 1280 by 720 point window at 60 frames a second. Returns each text
    /// drawn, with its middle.
    #[cfg(feature = "dev")]
    fn texts(
        (stack, cx, dock, egui): (
            &mut crate::layer::Stack,
            &mut Context,
            &mut crate::dock::Dock,
            &egui::Context,
        ),
        frame: u32,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            time: Some(f64::from(frame) / 60.0),
            events,
            ..Default::default()
        };
        let mut stepper = crate::main_loop::Stepper::default();
        let ui = Some((egui, dock, input));
        let dt = Duration::from_millis(16);
        let passed = crate::main_loop::run_frame(stack, cx, dt, &mut stepper, ui);
        let (mut output, _) = passed.expect("a pass");
        output.textures_delta.clear();
        let painted = crate::testing::painted(&output.shapes).into_iter();
        let texts = painted.filter_map(|shape| match shape {
            egui::Shape::Text(text) => {
                let rect = text.galley.rect.translate(text.pos.to_vec2());
                Some((text.galley.text().to_owned(), rect.center()))
            }
            _ => None,
        });
        texts.collect()
    }

    /// The gear opens the settings; a rate click caps the frames and keeps the cap; Off uncaps
    /// and keeps that.
    #[cfg(feature = "dev")]
    #[test]
    fn the_settings_frame_rate_limit_caps_the_frames_and_keeps_the_cap() {
        use egui::{Event, Modifiers, PointerButton, Pos2};

        use crate::chrome::{BAR, BUTTON, Chrome, TOOL};

        let scratch = crate::dirs::tests::Scratch::new("frame-rate");
        let mut cx = Context::new(None);
        cx.keep_state_in(scratch.0.clone());
        let mut stack = crate::layer::Stack::default();
        let mut dock = crate::dock::Dock::new(None, Chrome::InWindowsFrame, None, true, &[]);
        let egui = crate::egui_context();
        let mut frame = 0;
        let mut run = |cx: &mut Context, events| {
            frame += 1;
            texts((&mut stack, cx, &mut dock, &egui), frame, events)
        };
        // The dock lays out a frame before it draws.
        run(&mut cx, Vec::new());
        run(&mut cx, Vec::new());
        let mut click = |cx: &mut Context, at: Pos2| {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            for events in [
                vec![Event::PointerMoved(at)],
                vec![button(true)],
                vec![button(false)],
            ] {
                run(cx, events);
            }
            // Its sizing pass, and the next that shows it.
            run(cx, Vec::new());
            run(cx, Vec::new())
        };
        assert_eq!(cx.max_fps(), None);
        let gear = egui::pos2(1280.0 - 3.0 * BUTTON - 3.5 * TOOL, BAR / 2.0);
        let shown = click(&mut cx, gear);
        let at = |text: &str| {
            let found = shown.iter().find(|(shown, _)| shown == text);
            found.unwrap_or_else(|| panic!("{text:?} shows")).1
        };
        assert!(shown.iter().any(|(text, _)| text == "Frame rate limit"));
        let (sixty, off) = (at("60"), at("Off"));
        click(&mut cx, sixty);
        assert_eq!(cx.max_fps(), NonZeroU32::new(60));
        let mut next = Context::new(None);
        next.keep_state_in(scratch.0.clone());
        assert_eq!(kept(&next), NonZeroU32::new(60));
        click(&mut cx, off);
        assert_eq!((cx.max_fps(), kept(&next)), (None, None));
    }

    #[cfg(feature = "dev")]
    #[test]
    fn the_kept_limit_reads_back_as_it_was_kept_and_what_isnt_one_is_refused() {
        for max in [None, NonZeroU32::new(1), NonZeroU32::new(60)] {
            assert_eq!(read_kept(kept_text(max).as_bytes()).unwrap(), max);
        }
        for bad in ["Some(0)", "Some(-1)", "60fps", "", "\u{fffd}"] {
            let error = read_kept(bad.as_bytes()).unwrap_err();
            assert_eq!(error.code(), "engine.state", "{bad:?}: {error}");
        }
    }
}
