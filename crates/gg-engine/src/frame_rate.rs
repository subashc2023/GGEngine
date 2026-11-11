//! Frame-rate caps: holding a windowed run's frames to at most so many a second, as
//! [`Context::set_max_fps`] says, and, in development builds, the frame rate limit among the title
//! bar's settings, which the app keeps for the user from run to run.

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

/// The file the frame rate limit that the settings set is kept in, for the app's later runs, in
/// RON: `Some(60)`, or `None` while uncapped.
#[cfg(feature = "dev")]
const KEPT: &str = "max_fps.ron";

/// The rates the settings offer at a click, in frames a second, beside a field for any other.
#[cfg(feature = "dev")]
const RATES: [u32; 6] = [10, 30, 60, 120, 144, 240];

/// The most frames a second the settings' field takes.
#[cfg(feature = "dev")]
const MOST: u32 = 1000;

/// What the settings' frame rate limit is, which its label's tooltip says.
#[cfg(feature = "dev")]
const TIP: &str = "The most frames a second this app renders, which a game it embeds, as the \
                   editor does, renders no more of; kept for its next runs, unless --max-fps sets \
                   another";

/// Holds a windowed run's frames to at most so many a second, as [`Context::set_max_fps`] says,
/// by sleeping before each, as Unreal's `t.MaxFPS` and Godot's `max_fps` do: unlike waking from
/// winit's `ControlFlow::WaitUntil`, which Windows doesn't do while the user moves or resizes the
/// window, a sleep paces the frames that go on then too. Input that comes during the sleep waits
/// for the frame after, at most a frame's share of a second later.
#[derive(Default)]
pub(crate) struct Limiter {
    /// When the next frame is due, while capped.
    due: Option<Instant>,
}

impl Limiter {
    /// Waits until the next frame is due under at most `max` frames a second, if capped, and
    /// returns how long it waited.
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

/// When the frame after one due at `due`, which started at `start`, is due, at most a frame a
/// `period`: a period after `due`, so that a sleep that ends late doesn't slow the rate, or after
/// `start` once that's a period or more past `due`, as a slow frame or a pause leaves it, so that
/// no burst of frames follows to catch up.
fn next_due(due: Instant, start: Instant, period: Duration) -> Instant {
    let next = due + period;
    if start >= next { start + period } else { next }
}

/// Adds the frame rate limit to the title bar's settings, as [`Ui::settings`] says: Off or a rate
/// at a click, or, while capped, any other from 1 in a field, as slow motion for what settles over
/// frames. A choice caps `cx`'s frames from the next frame on, and is kept for the user, which
/// [`kept`] reads as the app's next run starts; a drag of the field is kept as it ends.
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

/// The frame rate limit the settings last kept for the user, if any: uncapped where they kept
/// none, and, with a warning (`engine.state`), where what they kept can't be read.
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

/// `max` as [`KEPT`] holds it.
#[cfg(feature = "dev")]
fn kept_text(max: Option<NonZeroU32>) -> String {
    match max {
        Some(max) => format!("Some({max})"),
        None => "None".to_owned(),
    }
}

/// What [`KEPT`] holds, from its `bytes`.
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

    /// Runs frame `frame` of `stack` with `cx`, in `dock`, a window 1280 by 720 points, at 60
    /// frames a second, given `events`, and returns each text it drew, with where its middle is.
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

    /// The gear opens the settings, whose frame rate limit caps the frames at a click and keeps
    /// the cap for the app's next run, and whose Off uncaps them, keeping that.
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
