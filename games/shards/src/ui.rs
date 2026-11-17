//! What the game draws on the view's canvas: the scores, the hint and the menu.

use std::fmt::Write as _;

use gg_engine::prelude::*;

use crate::components::Match;
use crate::{COLORS, ITEMS};

/// The scores, in canvas units: their inset from the view's top corners, and the em of a score
/// and of its matches won, below it.
const HUD_INSET: f32 = 40.0;
const SCORE_SIZE: f32 = 80.0;
const WINS_SIZE: f32 = 32.0;

/// The menu, in canvas units from the view's middle: each button's size, the first item's middle
/// above the view's, the step down to each next, and the title's middle.
pub(crate) const BUTTON: Vec2 = Vec2::new(440.0, 84.0);
const FIRST_ITEM: f32 = 50.0;
const ITEM_STEP: f32 = 104.0;
const TITLE: f32 = 180.0;

/// Draws each player's shards, and its matches won below them, in its color in the top corner on
/// its side, growing toward the middle, where the arena's Target says how many shards win. Each
/// text is written into `text`.
pub(crate) fn draw_scores(cx: &mut Context, found: &Match, font: Font, text: &mut String) {
    for (index, &color) in COLORS.iter().enumerate() {
        let (corner, inward) = if index == 0 {
            (Align::Start, 1.0)
        } else {
            (Align::End, -1.0)
        };
        let mut canvas = cx.canvas([corner, Align::Start]);
        let at = |down: f32| Mat4::from_translation(Vec3::new(inward * HUD_INSET, -down, 0.0));
        let style = |size: f32, alpha: f32| TextStyle {
            font: Some(font),
            size,
            color: Vec3::from(color).extend(alpha),
            align: [corner, Align::Start],
        };
        let write = |text: &mut String, value: u32, then: &str| {
            text.clear();
            write!(text, "{value}{then}").expect("a string takes any text");
        };
        write(text, found.scores[index], "");
        canvas.draw_text(at(HUD_INSET), text, &style(SCORE_SIZE, 1.0));
        let won = found.wins[index];
        write(text, won, if won == 1 { " WIN" } else { " WINS" });
        let below = HUD_INSET + SCORE_SIZE * 1.25;
        canvas.draw_text(at(below), text, &style(WINS_SIZE, 0.7));
    }
}

/// Draws the keys that open the menu at the view's bottom.
pub(crate) fn draw_hint(cx: &mut Context, font: Font) {
    let style = TextStyle {
        font: Some(font),
        size: 28.0,
        color: Vec4::new(1.0, 1.0, 1.0, 0.45),
        align: [Align::Center, Align::End],
    };
    let at = Mat4::from_translation(Vec3::new(0.0, 24.0, 0.0));
    let mut canvas = cx.canvas([Align::Center, Align::End]);
    canvas.draw_text(at, "ESC / START: MENU", &style);
}

/// Draws the menu in the view's middle: a dark panel, its title, and each item's button, `focus`'s
/// lit in `lit`, linear RGB.
pub(crate) fn draw_menu(cx: &mut Context, font: Font, focus: usize, lit: [f32; 3]) {
    let mut canvas = cx.canvas([Align::Center; 2]);
    let rect = |middle: Vec2, size: Vec2| {
        Mat4::from_translation(middle.extend(0.0)) * Mat4::from_scale(size.extend(1.0))
    };
    let filled = |color: Vec4| Sprite {
        color,
        ..Sprite::default()
    };
    let panel = filled(Vec4::new(0.01, 0.012, 0.02, 0.92));
    canvas.draw_sprite(rect(Vec2::new(0.0, 10.0), Vec2::new(560.0, 480.0)), panel);
    let style = |size: f32, color: Vec4| TextStyle {
        font: Some(font),
        size,
        color,
        align: [Align::Center; 2],
    };
    let at = |middle: Vec2| Mat4::from_translation(middle.extend(0.0));
    let title = style(64.0, Vec4::ONE);
    canvas.draw_text(at(Vec2::new(0.0, TITLE)), "SHARDS", &title);
    for (index, &(_, label)) in ITEMS.iter().enumerate() {
        let middle = item_middle(index);
        let (fill, ink) = if index == focus {
            (Vec3::from(lit).extend(1.0), Vec4::ONE)
        } else {
            (
                Vec4::new(1.0, 1.0, 1.0, 0.06),
                Vec4::new(1.0, 1.0, 1.0, 0.7),
            )
        };
        canvas.draw_sprite(rect(middle, BUTTON), filled(fill));
        canvas.draw_text(at(middle), label, &style(36.0, ink));
    }
}

/// Item `index`'s middle on the canvas, from the view's middle.
pub(crate) fn item_middle(index: usize) -> Vec2 {
    Vec2::new(0.0, FIRST_ITEM - ITEM_STEP * index as f32)
}

/// The item whose button is at `at` on the canvas, from the view's middle.
pub(crate) fn item_at(at: Vec2) -> Option<usize> {
    let over = |index| (at - item_middle(index)).abs().cmple(BUTTON / 2.0).all();
    (0..ITEMS.len()).find(|&index| over(index))
}
