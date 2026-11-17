//! The showcase's UI: the HUD it draws by code, and the menu and widgets from UI files.

use std::fmt::Write as _;

use gg_engine::prelude::*;
use gg_engine::{GameUi, UiValue};

use crate::components::{Hud, Menu, Widgets};

/// The widgets' drop-down's options, as `ui/widgets.ui` names them.
const DIFFICULTIES: [&str; 4] = ["EASY", "NORMAL", "HARD", "NIGHTMARE"];

/// How far the HUD's labels are from the canvas's edges, in its units.
const HUD_INSET: f32 = 36.0;

/// The HUD's edge and corner labels, by anchor, across then down.
const ANCHORED: [([Align; 2], &str); 8] = [
    ([Align::Start, Align::Start], "TOP LEFT"),
    ([Align::Center, Align::Start], "TOP"),
    ([Align::End, Align::Start], "TOP RIGHT"),
    ([Align::Start, Align::Center], "LEFT"),
    ([Align::End, Align::Center], "RIGHT"),
    ([Align::Start, Align::End], "BOTTOM LEFT"),
    ([Align::Center, Align::End], "BOTTOM"),
    ([Align::End, Align::End], "BOTTOM RIGHT"),
];

/// The HUD's button: its middle and its half size, from the canvas's middle.
const BUTTON: (Vec2, Vec2) = (Vec2::new(0.0, -40.0), Vec2::new(190.0, 56.0));

/// Draws the menu's UI as `menu` has it, writing its texts through `text`. A node the UI's file no
/// longer names goes unset: the file says what shows.
pub(crate) fn drive_menu(cx: &mut Context, ui: GameUi, menu: &Menu, text: &mut String) {
    let seconds = (Tick::DT * menu.ticks).as_secs();
    let clock = format_args!("{:02}:{:02}", seconds / 60, seconds % 60);
    let _ = ui.set_text_fmt(cx, "clock", clock);
    let _ = ui.set_text_fmt(cx, "presses", menu.presses);
    let label = |text: &mut String, name: &str| {
        text.clear();
        let capitals = name.chars().map(|c| match c {
            '_' => ' ',
            c => c.to_ascii_uppercase(),
        });
        text.extend(capitals);
    };
    match menu.pressed.as_str() {
        "" => label(text, "nothing yet"),
        pressed => label(text, pressed),
    }
    let _ = ui.set_text(cx, "pressed", text);
    match ui.focused(cx) {
        Some(focused) => label(text, &focused),
        None => label(text, "none"),
    }
    let _ = ui.set_text(cx, "focus", text);
    ui.draw(cx);
}

/// Draws the widgets' UI, its last line `text` as [`write_changed`] wrote it. A node the UI's file
/// no longer names goes unset, as the menu's do.
pub(crate) fn drive_widgets(cx: &mut Context, ui: GameUi, text: &str) {
    let _ = ui.set_text(cx, "changed", text);
    if let Ok(UiValue::Number(volume)) = ui.value(cx, "volume") {
        let percent = format_args!("{:.0}%", volume * 100.0);
        let _ = ui.set_text_fmt(cx, "volume_value", percent);
    }
    ui.draw(cx);
}

/// Writes the widgets' UI's last line into `text`: what changed last, and how many changes there
/// have been.
pub(crate) fn write_changed(widgets: &Widgets, text: &mut String) {
    text.clear();
    if widgets.changed.is_empty() {
        text.push_str("NOTHING CHANGED YET");
    } else {
        let capitals = widgets.changed.chars().map(|c| match c {
            '_' => ' ',
            c => c.to_ascii_uppercase(),
        });
        text.extend(capitals);
        let changes = if widgets.changes == 1 {
            "CHANGE"
        } else {
            "CHANGES"
        };
        let written = write!(text, "  ·  {} {changes}", widgets.changes);
        written.expect("a string takes any text");
    }
}

/// A widget's `value`, as [`Widgets`] records it.
pub(crate) fn said(value: &UiValue) -> String {
    match value {
        UiValue::Number(number) => format!("{number:.2}"),
        UiValue::On(on) => (if *on { "on" } else { "off" }).to_owned(),
        UiValue::Choice(choice) => DIFFICULTIES.get(*choice).unwrap_or(&"?").to_string(),
        UiValue::Text(text) => text.clone(),
    }
}

/// Whether `at`, on the canvas from its middle, is on the HUD's button.
pub(crate) fn on_button(at: Vec2) -> bool {
    let (middle, half) = BUTTON;
    let off = (at - middle).abs();
    off.x <= half.x && off.y <= half.y
}

/// Draws `hud` on the view's canvas in `font`, writing its numbers through `text`.
pub(crate) fn draw_hud(cx: &mut Context, hud: Hud, font: Font, text: &mut String) {
    let style = |size: f32, color: Vec4, align: [Align; 2]| TextStyle {
        font: Some(font),
        size,
        color,
        align,
    };
    let at = |x: f32, y: f32| Mat4::from_translation(Vec3::new(x, y, 0.0));
    let dim = Vec4::new(0.62, 0.66, 0.78, 1.0);
    for (anchor, label) in ANCHORED {
        // Inward from the edges it's anchored to: across from the left or the right, and down from
        // the top or up from the bottom.
        let inward = |align: Align, toward: f32| match align {
            Align::Start => toward * HUD_INSET,
            Align::Center => 0.0,
            Align::End => -toward * HUD_INSET,
        };
        let place = at(inward(anchor[0], 1.0), inward(anchor[1], -1.0));
        cx.canvas(anchor)
            .draw_text(place, label, &style(30.0, dim, anchor));
    }
    let mut canvas = cx.canvas([Align::Center; 2]);
    let middle = [Align::Center; 2];
    let title = style(52.0, Vec4::ONE, middle);
    canvas.draw_text(at(0.0, 260.0), "A GAME'S UI, ON THE VIEW'S CANVAS", &title);
    let write = |text: &mut String, label: &str, value: u32| {
        text.clear();
        write!(text, "{label} {value}").expect("a string takes any text");
    };
    write(text, "TICKS", hud.ticks);
    canvas.draw_text(at(0.0, 150.0), text, &style(40.0, dim, middle));
    let lit = canvas.cursor().is_some_and(on_button);
    let (fill, ink) = if lit {
        (Vec4::new(0.95, 0.55, 0.15, 1.0), Vec4::ONE)
    } else {
        (Vec4::new(1.0, 1.0, 1.0, 0.1), Vec4::new(1.0, 1.0, 1.0, 0.8))
    };
    let (button, half) = BUTTON;
    let rect =
        Mat4::from_translation(button.extend(0.0)) * Mat4::from_scale((half * 2.0).extend(1.0));
    let sprite = Sprite {
        color: fill,
        ..Sprite::default()
    };
    canvas.draw_sprite(rect, sprite);
    canvas.draw_text(
        at(button.x, button.y),
        "CLICK ME",
        &style(40.0, ink, middle),
    );
    write(text, "CLICKS", hud.clicks);
    canvas.draw_text(at(0.0, -170.0), text, &style(40.0, dim, middle));
    let hint =
        "PLAY COUNTS THE TICKS AND THE CLICKS: THE LABELS KEEP TO THE VIEW'S EDGES AS IT RESIZES";
    canvas.draw_text(at(0.0, -260.0), hint, &style(24.0, dim, middle));
}
