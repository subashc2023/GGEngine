//! Frame-rate caps for windowed runs, as [`Context::set_max_fps`] says, and embedded ones, as a
//! client's look says; in development builds, the frame rate limits among the title bar's
//! settings, kept across runs, and above the render scale of a game ([`game_scale`]).

use std::num::NonZeroU32;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(feature = "dev")]
use egui::{Align, ComboBox, Layout};
#[cfg(feature = "dev")]
use gg_core::{Error, Result};
#[cfg(feature = "dev")]
use serde::Deserialize;
#[cfg(feature = "dev")]
use tracing::warn;
#[cfg(feature = "dev")]
use winit::monitor::MonitorHandle;

#[cfg(feature = "dev")]
use crate::{Context, Ui, game_scale};

/// The file keeping the settings' limit on the app's own frames for later runs, in RON as
/// [`Limit`] has it: `Rate(60)`, `Uncapped` or `Monitor`. Not `max_fps.ron`, which kept another
/// format, and in the editor a cap from before it embedded games.
#[cfg(feature = "dev")]
const KEPT: &str = "frame_rate.ron";

/// Likewise, in an app that embeds a game, the game's limit.
#[cfg(feature = "dev")]
const KEPT_GAME: &str = "game_frame_rate.ron";

/// The limits offered, in order.
#[cfg(feature = "dev")]
const OFFERED: [Limit; 7] = [
    Limit::rate(30),
    Limit::rate(60),
    Limit::rate(120),
    Limit::rate(144),
    Limit::rate(240),
    Limit::Uncapped,
    Limit::Monitor,
];

#[cfg(feature = "dev")]
const TIP: &str = "The most frames a second this app renders. Match monitor presents at the \
                   display's refresh rate. Kept for its next runs, unless --vsync or --max-fps \
                   sets another";

#[cfg(feature = "dev")]
const GAME_TIP: &str = "The most frames a second the game this app shows renders, which renders \
                        no more than this app shows either way. Match monitor holds it to this \
                        window's monitor's refresh rate. Kept for its next runs";

/// A frame rate limit the title bar's settings offer and keep.
#[cfg(feature = "dev")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum Limit {
    /// At most this many frames a second.
    Rate(NonZeroU32),
    /// As fast as frames render.
    Uncapped,
    /// The display's refresh rate: presenting at it, for the app's own frames; a cap at the
    /// window's monitor's rate, for its game's.
    Monitor,
}

#[cfg(feature = "dev")]
impl Limit {
    /// # Panics
    ///
    /// - If `rate` is 0.
    const fn rate(rate: u32) -> Self {
        Self::Rate(NonZeroU32::new(rate).expect("a rate isn't 0"))
    }

    /// What the settings show for it.
    fn text(self) -> String {
        match self {
            Self::Rate(rate) => format!("{rate} fps"),
            Self::Uncapped => "Uncapped".to_owned(),
            Self::Monitor => "Match monitor".to_owned(),
        }
    }

    /// How it's kept, in RON.
    fn kept_text(self) -> String {
        match self {
            Self::Rate(rate) => format!("Rate({rate})"),
            Self::Uncapped => "Uncapped".to_owned(),
            Self::Monitor => "Monitor".to_owned(),
        }
    }

    /// The app's own frames under it: whether they present at the display's refresh rate, and
    /// their cap.
    fn own(self) -> (bool, Option<NonZeroU32>) {
        match self {
            Self::Rate(rate) => (false, Some(rate)),
            Self::Uncapped => (false, None),
            Self::Monitor => (true, None),
        }
    }
}

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

/// Applies, as a windowed run starts, the limits the settings kept: in an app that embeds a game,
/// the game's to `cx`. Returns whether the app's own frames present at the display's refresh
/// rate, and their cap: as kept, else Match monitor in an app that embeds a game and Uncapped
/// elsewhere. Call it once the window's refresh rate is noted ([`set_refresh_rate`]).
#[cfg(feature = "dev")]
pub(crate) fn start(cx: &mut Context) -> (bool, Option<NonZeroU32>) {
    let embeds = cx.dev.embeds_game;
    if embeds {
        let game = kept(cx, KEPT_GAME).unwrap_or(Limit::Uncapped);
        set_game(cx, game);
    }
    let default = if embeds {
        Limit::Monitor
    } else {
        Limit::Uncapped
    };
    kept(cx, KEPT).unwrap_or(default).own()
}

/// `monitor`'s refresh rate, to the nearest whole frame a second: 59.94 Hz is 60.
#[cfg(feature = "dev")]
pub(crate) fn refresh_rate(monitor: Option<&MonitorHandle>) -> Option<NonZeroU32> {
    let millihertz = monitor?.refresh_rate_millihertz()?;
    NonZeroU32::new((millihertz + 500) / 1000)
}

/// Notes the window's monitor's refresh rate ([`refresh_rate`]), as it opens and moves to another.
/// A game matching the monitor follows it.
#[cfg(feature = "dev")]
pub(crate) fn set_refresh_rate(cx: &mut Context, rate: Option<NonZeroU32>) {
    cx.dev.refresh_rate = rate;
    if cx.dev.game_matches_monitor {
        set_game(cx, Limit::Monitor);
    }
}

/// Caps the game `cx` shows by `limit`: Match monitor at the window's monitor's refresh rate, or
/// uncapped while that's unknown.
#[cfg(feature = "dev")]
fn set_game(cx: &mut Context, limit: Limit) {
    let max = match limit {
        Limit::Rate(rate) => Some(rate),
        Limit::Uncapped => None,
        Limit::Monitor => cx.dev.refresh_rate,
    };
    cx.set_game_max_fps(max);
    cx.dev.game_matches_monitor = limit == Limit::Monitor;
}

/// Adds the frame rate limits to the title bar's settings ([`Ui::settings`]), a line each: the
/// app's own, called the editor's in an app that embeds a game, and there the game's
/// ([`Context::game_max_fps`]), then the game's render scale ([`game_scale::settings`]). A pick
/// applies from the next frame and is kept for the next runs ([`start`]). A cap set elsewhere,
/// such as another client's 10, shows as it is.
#[cfg(feature = "dev")]
pub(crate) fn settings(ui: &mut Ui<'_>, cx: &mut Context) {
    let embeds = cx.dev.embeds_game;
    let own = match (cx.vsync(), cx.max_fps()) {
        (_, Some(rate)) => Limit::Rate(rate),
        (true, None) => Limit::Monitor,
        (false, None) => Limit::Uncapped,
    };
    let game = match (cx.dev.game_matches_monitor, cx.game_max_fps()) {
        (true, _) => Limit::Monitor,
        (false, Some(rate)) => Limit::Rate(rate),
        (false, None) => Limit::Uncapped,
    };
    let (mut own_picked, mut game_picked) = (None, None);
    ui.settings(|ui| {
        let label = if embeds {
            "Editor frame rate"
        } else {
            "Frame rate limit"
        };
        own_picked = line(ui, label, TIP, own);
        if embeds {
            game_picked = line(ui, "Game frame rate", GAME_TIP, game);
            game_scale::settings(ui, cx);
        }
    });
    if let Some(limit) = own_picked {
        let (vsync, max) = limit.own();
        cx.set_vsync(vsync);
        cx.set_max_fps(max);
        keep(cx, KEPT, limit);
    }
    if let Some(limit) = game_picked {
        set_game(cx, limit);
        keep(cx, KEPT_GAME, limit);
    }
}

/// A limit's line: `label`, then a dropdown of the limits offered, showing `shown`. Returns the
/// limit picked, if another.
#[cfg(feature = "dev")]
fn line(ui: &mut egui::Ui, label: &str, tip: &str, shown: Limit) -> Option<Limit> {
    let mut picked = shown;
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(tip);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let dropdown = ComboBox::from_id_salt(label).selected_text(shown.text());
            dropdown.show_ui(ui, |ui| {
                for limit in OFFERED {
                    ui.selectable_value(&mut picked, limit, limit.text());
                }
            });
        });
    });
    (picked != shown).then_some(picked)
}

/// Keeps `limit` in `file` for the next runs, warning `engine.state` if it can't.
#[cfg(feature = "dev")]
fn keep(cx: &Context, file: &str, limit: Limit) {
    if let Err(error) = cx.save_state(file, limit.kept_text().as_bytes()) {
        warn!(code = error.code(), "{error:#}");
    }
}

/// The limit kept in `file`: `None` if none, or, with an `engine.state` warning, if it can't be
/// read.
#[cfg(feature = "dev")]
fn kept(cx: &Context, file: &str) -> Option<Limit> {
    let read = cx.read_state(file).and_then(|bytes| {
        let read = bytes.map(|bytes| read_kept(&bytes, file));
        read.transpose()
    });
    read.unwrap_or_else(|error| {
        warn!(code = error.code(), "{error:#}; the default limit applies");
        None
    })
}

#[cfg(feature = "dev")]
fn read_kept(bytes: &[u8], file: &str) -> Result<Limit> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string());
    let limit = text.and_then(|text| ron::from_str(text).map_err(|e| e.to_string()));
    limit.map_err(|problem| {
        let message = format!("can't read the frame rate limit kept in {file}: {problem}");
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

    /// The gear opens the settings, a line per limit: its label, and right of it a dropdown
    /// showing it. A pick applies and is kept in that limit's own file: a rate caps, Uncapped
    /// uncaps, and Match monitor presents at the display's rate, or caps the game at the monitor's.
    /// In an app that embeds a game, the editor's line and the game's act apart, and the game's
    /// render scale has a line of its own, kept in its own file.
    #[cfg(feature = "dev")]
    #[test]
    fn the_settings_frame_rate_limits_apply_each_pick_and_keep_it() {
        use egui::{Event, Modifiers, PointerButton, Pos2};

        use crate::chrome::{BAR, BUTTON, Chrome, TOOL};

        let rate = NonZeroU32::new;
        for embeds in [false, true] {
            let scratch = crate::dirs::tests::Scratch::new("frame-rate");
            let context = || {
                let mut cx = Context::new(None);
                cx.keep_state_in(scratch.0.clone());
                cx.dev.embeds_game = embeds;
                cx
            };
            let mut cx = context();
            cx.dev.refresh_rate = rate(165);
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
            // The text right of `label` on its line: its dropdown's.
            let dropdown = |shown: &[(String, Pos2)], label: &str| {
                let at = shown.iter().find(|(text, _)| text == label);
                let at = at.unwrap_or_else(|| panic!("{label:?} shows")).1;
                let right = shown.iter().filter(|(_, pos)| (pos.y - at.y).abs() < 1.0);
                let right: Vec<_> = right.filter(|(_, pos)| pos.x > at.x).collect();
                assert_eq!(right.len(), 1, "{label:?}'s line: {right:?}");
                right[0].clone()
            };
            let (own, game) = match embeds {
                false => ("Frame rate limit", None),
                true => ("Editor frame rate", Some("Game frame rate")),
            };
            let gear = egui::pos2(1280.0 - 3.0 * BUTTON - 3.5 * TOOL, BAR / 2.0);
            let shown = click(&mut cx, gear);
            assert_eq!(dropdown(&shown, own).0, "Uncapped");
            // Opens `label`'s dropdown and picks `limit` from its list, below it and off the
            // lines. Returns what the line shows then.
            let mut shown = Some(shown);
            let mut pick = |cx: &mut Context, label: &str, limit: &str| {
                let before = shown.take().expect("the settings show");
                let scale = game.map(|_| game_scale::LABEL);
                let line = |text: &str| {
                    let labels = [own].into_iter().chain(game).chain(scale);
                    labels.into_iter().any(|label| label == text)
                };
                let lines: Vec<_> = before.iter().filter(|(text, _)| line(text)).collect();
                let below = dropdown(&before, label).1;
                let listed = click(cx, below);
                let entry = listed.iter().find(|(text, at)| {
                    let on_line = lines.iter().any(|(_, line)| (line.y - at.y).abs() < 1.0);
                    text == limit && at.y > below.y && !on_line
                });
                let entry = entry
                    .unwrap_or_else(|| panic!("{limit:?} listed: {listed:?}"))
                    .1;
                let after = click(cx, entry);
                let showing = dropdown(&after, label).0;
                shown = Some(after);
                showing
            };
            let own_caps = |cx: &Context| (cx.vsync(), cx.max_fps());
            let kept_in = |file| kept(&context(), file);
            assert_eq!(pick(&mut cx, own, "60 fps"), "60 fps");
            assert_eq!(own_caps(&cx), (false, rate(60)));
            assert_eq!(kept_in(KEPT), Some(Limit::rate(60)));
            assert!(!scratch.0.join(KEPT_GAME).exists(), "{embeds}");
            assert_eq!(pick(&mut cx, own, "Match monitor"), "Match monitor");
            assert_eq!(
                (own_caps(&cx), kept_in(KEPT)),
                ((true, None), Some(Limit::Monitor))
            );
            assert_eq!(pick(&mut cx, own, "Uncapped"), "Uncapped");
            assert_eq!(
                (own_caps(&cx), kept_in(KEPT)),
                ((false, None), Some(Limit::Uncapped))
            );
            assert_eq!(cx.game_max_fps(), None, "{embeds}");
            if let Some(game) = game {
                assert_eq!(pick(&mut cx, game, "Match monitor"), "Match monitor");
                assert_eq!(cx.game_max_fps(), rate(165));
                assert_eq!(kept_in(KEPT_GAME), Some(Limit::Monitor));
                assert_eq!(pick(&mut cx, game, "144 fps"), "144 fps");
                assert_eq!(cx.game_max_fps(), rate(144));
                assert_eq!(kept_in(KEPT_GAME), Some(Limit::rate(144)));
                assert_eq!(
                    (own_caps(&cx), kept_in(KEPT)),
                    ((false, None), Some(Limit::Uncapped))
                );
                // The game's render scale: the game's own until a pick, kept in its own file.
                let kept_scale = || crate::game_scale::kept(&context());
                assert_eq!(cx.game_render_scale(), None);
                assert_eq!(pick(&mut cx, game_scale::LABEL, "50%"), "50%");
                assert_eq!(cx.game_render_scale(), Some(0.5));
                assert_eq!(kept_scale(), Some(Some(0.5)));
                assert_eq!(pick(&mut cx, game_scale::LABEL, "Game's own"), "Game's own");
                assert_eq!(cx.game_render_scale(), None);
                assert_eq!(kept_scale(), Some(None));
                assert_eq!(cx.game_max_fps(), rate(144), "the frame rate is its own");
            } else {
                let shown = shown.expect("the settings show");
                let labels: Vec<_> = shown.iter().map(|(text, _)| text.as_str()).collect();
                assert!(!labels.contains(&game_scale::LABEL), "no game to scale");
            }
        }
    }

    /// A run starts at the limits kept, else at Match monitor in an app that embeds a game and
    /// Uncapped elsewhere. A game matching the monitor follows its refresh rate, through the
    /// editor's sync of the same cap each frame, until another cap replaces it.
    #[cfg(feature = "dev")]
    #[test]
    fn a_run_starts_at_the_kept_limits_and_a_game_matching_the_monitor_follows_it() {
        let scratch = crate::dirs::tests::Scratch::new("frame-rate-start");
        let context = |embeds| {
            let mut cx = Context::new(None);
            cx.keep_state_in(scratch.0.clone());
            cx.dev.embeds_game = embeds;
            cx
        };
        let rate = NonZeroU32::new;
        assert_eq!(start(&mut context(false)), (false, None));
        let mut cx = context(true);
        assert_eq!((start(&mut cx), cx.game_max_fps()), ((true, None), None));
        keep(&cx, KEPT, Limit::rate(30));
        keep(&cx, KEPT_GAME, Limit::Monitor);
        let mut cx = context(true);
        set_refresh_rate(&mut cx, rate(144));
        let started = (start(&mut cx), cx.game_max_fps());
        assert_eq!(started, ((false, rate(30)), rate(144)));
        cx.set_game_max_fps(rate(144));
        set_refresh_rate(&mut cx, rate(60));
        assert_eq!(cx.game_max_fps(), rate(60));
        cx.set_game_max_fps(rate(10));
        set_refresh_rate(&mut cx, rate(240));
        assert_eq!(cx.game_max_fps(), rate(10), "another client's cap holds");
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_kept_limit_reads_back_as_it_was_kept_and_what_isnt_one_is_refused() {
        for limit in OFFERED.into_iter().chain([Limit::rate(1)]) {
            assert_eq!(
                read_kept(limit.kept_text().as_bytes(), KEPT).unwrap(),
                limit
            );
        }
        for bad in [
            "Rate(0)", "Rate(-1)", "Some(60)", "None", "60fps", "", "\u{fffd}",
        ] {
            let error = read_kept(bad.as_bytes(), KEPT).unwrap_err();
            assert_eq!(error.code(), "engine.state", "{bad:?}: {error}");
        }
    }
}
