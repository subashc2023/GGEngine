//! The render scale of the game an app embeds, among the title bar's settings, kept across runs.
//!
//! The app sends it to the game in its look ([`gg_api::Look::render_scale`]).

use egui::{Align, ComboBox, Layout};
use gg_core::{Error, Result};
use tracing::warn;

use crate::{Context, MIN_RENDER_SCALE};

/// The file keeping the settings' scale for later runs, in RON as an `Option<f32>`: `None` for
/// the game's own, else `Some(0.5)`.
const KEPT: &str = "game_render_scale.ron";

/// The scales offered after the game's own, in order.
const OFFERED: [Option<f32>; 5] = [None, Some(1.0), Some(0.75), Some(0.5), Some(0.25)];

/// The settings' label for the line, which tells it from the frame rate's.
pub(crate) const LABEL: &str = "Game render scale";

const TIP: &str = "The share of the game's pixels, across and down, that the game this app shows \
                   renders, upscaled to its view. Game's own leaves it to the game. Kept for \
                   its next runs";

/// What the settings show for `scale`.
fn text(scale: Option<f32>) -> String {
    match scale {
        None => "Game's own".to_owned(),
        Some(scale) => format!("{}%", (scale * 100.0).round()),
    }
}

/// Applies, as a windowed run starts, the scale the settings kept to the game `cx` shows, the
/// game's own if none. Does nothing in an app that embeds no game.
pub(crate) fn start(cx: &mut Context) {
    if cx.dev.embeds_game {
        let scale = kept(cx).flatten();
        cx.set_game_render_scale(scale);
    }
}

/// Adds the scale's line to the title bar's settings `ui`, in an app that embeds a game: its
/// label, and right of it a dropdown of the scales offered. A pick applies from the next frame and
/// is kept for the next runs ([`start`]). A scale set elsewhere, such as another client's, shows
/// as it is.
pub(crate) fn settings(ui: &mut egui::Ui, cx: &mut Context) {
    if !cx.dev.embeds_game {
        return;
    }
    let shown = cx.game_render_scale();
    let mut picked = shown;
    ui.horizontal(|ui| {
        ui.label(LABEL).on_hover_text(TIP);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let dropdown = ComboBox::from_id_salt(LABEL).selected_text(text(shown));
            dropdown.show_ui(ui, |ui| {
                for scale in OFFERED {
                    ui.selectable_value(&mut picked, scale, text(scale));
                }
            });
        });
    });
    if picked != shown {
        cx.set_game_render_scale(picked);
        if let Err(error) = cx.save_state(KEPT, kept_text(picked).as_bytes()) {
            warn!(code = error.code(), "{error:#}");
        }
    }
}

/// How `scale` is kept, in RON.
fn kept_text(scale: Option<f32>) -> String {
    match scale {
        None => "None".to_owned(),
        Some(scale) => format!("Some({scale:?})"),
    }
}

/// The scale kept: `None` if none is, or, with an `engine.state` warning, if it can't be read;
/// `Some(None)` if the game's own is.
pub(crate) fn kept(cx: &Context) -> Option<Option<f32>> {
    let read = cx.read_state(KEPT).and_then(|bytes| {
        let read = bytes.map(|bytes| read_kept(&bytes));
        read.transpose()
    });
    read.unwrap_or_else(|error| {
        warn!(
            code = error.code(),
            "{error:#}; the game's own scale applies"
        );
        None
    })
}

fn read_kept(bytes: &[u8]) -> Result<Option<f32>> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string());
    let scale = text.and_then(|text| ron::from_str::<Option<f32>>(text).map_err(|e| e.to_string()));
    let scale = scale.and_then(|scale| match scale {
        Some(scale) if !(MIN_RENDER_SCALE..=1.0).contains(&scale) => {
            Err(format!("{scale} isn't from {MIN_RENDER_SCALE} to 1"))
        }
        scale => Ok(scale),
    });
    scale.map_err(|problem| {
        let message = format!("can't read the render scale kept in {KEPT}: {problem}");
        Error::new("engine.state", message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(dir: &std::path::Path, embeds: bool) -> Context {
        let mut cx = Context::new(None);
        cx.keep_state_in(dir.to_owned());
        cx.dev.embeds_game = embeds;
        cx
    }

    #[test]
    fn a_kept_scale_reads_back_as_it_was_kept_and_what_isnt_one_is_refused() {
        for scale in OFFERED.into_iter().chain([Some(MIN_RENDER_SCALE)]) {
            assert_eq!(read_kept(kept_text(scale).as_bytes()).unwrap(), scale);
        }
        for bad in [
            "Some(0.0)",
            "Some(0.2)",
            "Some(1.5)",
            "Some(-1.0)",
            "Some(NaN)",
            "0.5",
            "Rate(1)",
            "",
            "\u{fffd}",
        ] {
            let error = read_kept(bad.as_bytes()).unwrap_err();
            assert_eq!(error.code(), "engine.state", "{bad:?}: {error}");
        }
    }

    #[test]
    fn a_run_starts_at_the_kept_scale_in_an_app_that_embeds_a_game() {
        let scratch = crate::dirs::tests::Scratch::new("game-scale-start");
        let mut cx = context(&scratch.0, true);
        start(&mut cx);
        assert_eq!(cx.game_render_scale(), None, "nothing kept");
        cx.save_state(KEPT, kept_text(Some(0.5)).as_bytes())
            .unwrap();
        let mut cx = context(&scratch.0, true);
        start(&mut cx);
        assert_eq!(cx.game_render_scale(), Some(0.5));
        let mut cx = context(&scratch.0, false);
        start(&mut cx);
        assert_eq!(cx.game_render_scale(), None, "no game to scale");
        cx.save_state(KEPT, b"Some(7.0)").unwrap();
        let mut cx = context(&scratch.0, true);
        start(&mut cx);
        assert_eq!(
            cx.game_render_scale(),
            None,
            "unreadable, so the game's own"
        );
    }
}
