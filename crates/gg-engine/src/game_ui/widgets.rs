//! Drawing a game's UI's nodes, each within what clips it, and its widgets apart from their input,
//! which [`State::event`] takes: what each widget fits to, holds and draws, a slider's track and
//! thumb, a toggle's switch, a drop-down's choice and its options open over the rest, a text
//! field's text, caret and selection within its box, and a list's scroll bar; and where a text
//! field's caret goes between its characters.

use gg_core::math::libm::atan2f;
use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{Sprite, Texture};

use super::layout::Rect;
use super::{
    Canvas, FOCUSED, Kind, Look, Node, Open, State, UiValue, Widget, corners, on_canvas, paint,
    write,
};
use crate::Align;
use crate::font::Fonts;

/// How wide a text field's caret is, in the canvas's units.
pub(super) const CARET: f32 = 3.0;

/// How tall a line of text is, in its ems: Inter's ascent less its descent, about, as a text
/// field's caret and selection are drawn.
const LINE: f32 = 1.2;

/// A toggle's switch's size, in its text's ems, and the room between its text and its switch, and
/// a drop-down's text and its arrow.
const SWITCH: Vec2 = Vec2::new(1.8, 1.0);
const BESIDE: f32 = 0.6;

/// How wide a drop-down's arrow is, in its text's ems.
const ARROW: f32 = 0.5;

/// How wide a text field fits to, in its text's ems.
const FIELD: f32 = 10.0;

/// How wide a list's scroll bar is, and how short its thumb gets, in the canvas's units.
const BAR: f32 = 6.0;
const SHORTEST: f32 = 24.0;

/// Whether a node of `kind` holds a value: a slider, a toggle, a drop-down or a text field.
pub(super) fn holds(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Slider | Kind::Toggle | Kind::Dropdown | Kind::Field
    )
}

/// What widget `node` fits to, its padding round it, its text measured by `fonts`: a slider, 240
/// units by 48; a toggle, its text beside its switch; a drop-down, its widest option beside its
/// arrow; and a text field, ten ems of its text by a line.
pub(super) fn fit(node: &Node, fonts: &mut Fonts) -> Vec2 {
    let style = node.style(Vec4::ONE);
    let em = node.text_size;
    let content = match &node.widget {
        Widget::Slider { .. } => Vec2::new(240.0, 48.0),
        Widget::Toggle { .. } => {
            let text = fonts.measure(&node.text, &style);
            let switch = SWITCH * em;
            Vec2::new(text.x + BESIDE * em + switch.x, text.y.max(switch.y))
        }
        Widget::Dropdown { options, .. } => {
            let mut widest = Vec2::ZERO;
            for option in options {
                widest = widest.max(fonts.measure(option, &style));
            }
            Vec2::new(widest.x + (BESIDE + ARROW) * em, widest.y)
        }
        Widget::Field { .. } => Vec2::new(FIELD * em, fonts.measure("", &style).y),
        Widget::None => Vec2::ZERO,
    };
    content + node.padding * 2.0
}

/// Where slider `node`, laid at `rect`, has its thumb's middle go: along its middle, from its left
/// less its padding, at its `min`, to its right less its padding, at its `max`; a rectangle of no
/// height.
pub(super) fn track(node: &Node, rect: Rect) -> Rect {
    let room = rect.inside(node.padding);
    Rect {
        at: Vec2::new(room.at.x, room.middle().y),
        size: Vec2::new(room.size.x, 0.0),
    }
}

/// What widget `node` holds, `None` for a node that's no widget.
pub(super) fn value_of(node: &Node) -> Option<UiValue> {
    Some(match &node.widget {
        Widget::Slider { value, .. } => UiValue::Number(*value),
        Widget::Toggle { on } => UiValue::On(*on),
        Widget::Dropdown { choice, .. } => UiValue::Choice(*choice),
        Widget::Field { .. } => UiValue::Text(node.text.clone()),
        Widget::None => return None,
    })
}

/// Has `widget`, as its file was read again, hold what `old`, of its node's name, held, where it's
/// of its kind and takes it, a slider's value within its bounds now; and whether it did.
#[cfg(any(feature = "dev", test))]
pub(super) fn keep_value(widget: &mut Widget, old: &Widget) -> bool {
    match (widget, old) {
        (
            Widget::Slider {
                min, max, value, ..
            },
            Widget::Slider { value: was, .. },
        ) => {
            *value = was.clamp(*min, *max);
            true
        }
        (Widget::Toggle { on }, Widget::Toggle { on: was }) => {
            *on = *was;
            true
        }
        (
            Widget::Dropdown {
                options, choice, ..
            },
            Widget::Dropdown { choice: was, .. },
        ) if *was < options.len() => {
            *choice = *was;
            true
        }
        _ => false,
    }
}

/// Has `node` say `text`, a text field its first line, as many characters as it holds; and whether
/// it said something else.
pub(super) fn put_text(node: &mut Node, text: &str) -> bool {
    let text = match node.widget {
        Widget::Field { most, .. } => {
            let line = text.split(char::is_control).next().unwrap_or_default();
            let end = line
                .char_indices()
                .nth(most)
                .map_or(line.len(), |(end, _)| end);
            &line[..end]
        }
        _ => text,
    };
    if node.text == text {
        return false;
    }
    node.text.clear();
    node.text.push_str(text);
    true
}

/// Has slider, toggle or drop-down `node`, named `name` in UI `ui`, hold `value`, a slider's within
/// its bounds, as [`GameUi::set_value`](super::GameUi::set_value) says.
///
/// Fails with `ui.bad_value` for a value of another widget's, a number that isn't finite or a
/// choice past its options.
pub(super) fn put_value(node: &mut Node, ui: &str, value: UiValue) -> Result<()> {
    let refused = |node: &Node, why: &str| {
        let name = node.name.as_deref().unwrap_or_default();
        let called = node.kind.called();
        let message = format!("node `{name}` of UI `{ui}` is {called}, which {why}");
        Err(Error::new("ui.bad_value", message))
    };
    match (&mut node.widget, value) {
        (
            Widget::Slider {
                min, max, value, ..
            },
            UiValue::Number(number),
        ) => {
            if !number.is_finite() {
                return refused(node, "holds a finite number");
            }
            *value = number.clamp(*min, *max);
        }
        (Widget::Toggle { on }, UiValue::On(to)) => *on = to,
        (
            Widget::Dropdown {
                options, choice, ..
            },
            UiValue::Choice(to),
        ) => {
            let count = options.len();
            if to >= count {
                return refused(node, &format!("has {count} options"));
            }
            *choice = to;
        }
        (widget, _) => {
            let takes = match widget {
                Widget::Slider { .. } => "holds a number",
                Widget::Toggle { .. } => "is on or off",
                Widget::Dropdown { .. } => "holds a choice",
                Widget::Field { .. } | Widget::None => "holds text",
            };
            return refused(node, takes);
        }
    }
    Ok(())
}

/// The boundary of the character before byte `at` of `text`, a boundary, or its start.
pub(super) fn before(text: &str, at: usize) -> usize {
    text[..at]
        .char_indices()
        .next_back()
        .map_or(0, |(at, _)| at)
}

/// The boundary of the character after byte `at` of `text`, a boundary, or its end.
pub(super) fn after(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map_or(at, |next| at + next.len_utf8())
}

/// A look of `color` alone, without a texture, as a widget's parts are drawn in.
fn plain(color: Vec4) -> Look {
    Look {
        color,
        text: color,
        texture: None,
    }
}

/// `color` with its alpha times `by`.
fn faded(color: Vec4, by: f32) -> Vec4 {
    color.with_w(color.w * by)
}

impl State {
    /// Draws node `i` on `canvas`, from `origin`, the point of the view its top node is anchored
    /// to, in the canvas's units from the view's top left, within what clips it, and nothing of it
    /// where it's clipped out whole; rounded corners cut from `disc`.
    pub(super) fn draw_node(
        &self,
        i: usize,
        canvas: &mut Canvas<'_>,
        origin: Vec2,
        disc: Option<Texture>,
    ) {
        let node = &self.nodes[i];
        let rect = self.laid.rects[i];
        let clip = self.laid.clips[i];
        if clip.is_some_and(|clip| rect.within(clip).size.cmple(Vec2::ZERO).any()) {
            return;
        }
        canvas.set_clip(clip.map(|clip| corners(clip, origin)));
        let look = &node.looks[self.look(i)];
        let boxed = |canvas: &mut Canvas<'_>| {
            paint(canvas, rect, origin, look, node.radius, node.slice, disc);
        };
        match node.kind {
            Kind::Panel => boxed(canvas),
            Kind::Image => paint(canvas, rect, origin, look, 0.0, None, disc),
            Kind::Button => {
                boxed(canvas);
                write(canvas, node, &node.text, rect, origin, look.text);
            }
            Kind::Text => write(canvas, node, &node.text, rect, origin, look.text),
            Kind::Slider => {
                boxed(canvas);
                self.draw_slider(i, canvas, origin, look, disc);
            }
            Kind::Toggle => {
                boxed(canvas);
                write(canvas, node, &node.text, rect, origin, look.text);
                self.draw_switch(i, canvas, origin, look, disc);
            }
            Kind::Dropdown => {
                boxed(canvas);
                if let Widget::Dropdown {
                    options, choice, ..
                } = &node.widget
                {
                    write(canvas, node, &options[*choice], rect, origin, look.text);
                }
                self.draw_arrow(i, canvas, origin, look);
            }
            Kind::Field => {
                boxed(canvas);
                self.draw_typed(i, canvas, origin, look);
            }
            // Its scroll bar over its children, once they're drawn, as `draw_bars` draws it.
            Kind::List => paint(canvas, rect, origin, look, node.radius, None, disc),
            Kind::Row | Kind::Column | Kind::Stack => {}
        }
    }

    /// Draws slider `i`'s track, as far as its value in its accent, and its thumb there, in its
    /// look's text's color.
    fn draw_slider(
        &self,
        i: usize,
        canvas: &mut Canvas<'_>,
        origin: Vec2,
        look: &Look,
        disc: Option<Texture>,
    ) {
        let node = &self.nodes[i];
        let Widget::Slider {
            min, max, value, ..
        } = node.widget
        else {
            return;
        };
        let rect = self.laid.rects[i];
        let track = track(node, rect);
        let tall = rect.inside(node.padding).size.y;
        let (thick, thumb) = ((tall * 0.16).max(2.0), tall * 0.6);
        let along = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let x = track.at.x + along * track.size.x;
        let bar = |to: f32| Rect {
            at: Vec2::new(track.at.x, track.at.y - thick / 2.0),
            size: Vec2::new(to - track.at.x, thick),
        };
        let whole = bar(track.at.x + track.size.x);
        paint(
            canvas,
            whole,
            origin,
            &plain(faded(look.text, 0.3)),
            thick / 2.0,
            None,
            disc,
        );
        paint(
            canvas,
            bar(x),
            origin,
            &plain(node.accent),
            thick / 2.0,
            None,
            disc,
        );
        let knob = Rect {
            at: Vec2::new(x - thumb / 2.0, track.at.y - thumb / 2.0),
            size: Vec2::splat(thumb),
        };
        paint(
            canvas,
            knob,
            origin,
            &plain(look.text),
            thumb / 2.0,
            None,
            disc,
        );
    }

    /// Draws toggle `i`'s switch at the end of its room: in its accent, its knob to the right,
    /// while it's on, and faded, its knob to the left, while it's off.
    fn draw_switch(
        &self,
        i: usize,
        canvas: &mut Canvas<'_>,
        origin: Vec2,
        look: &Look,
        disc: Option<Texture>,
    ) {
        let node = &self.nodes[i];
        let Widget::Toggle { on } = node.widget else {
            return;
        };
        let room = self.laid.rects[i].inside(node.padding);
        let size = SWITCH * node.text_size;
        let at = Vec2::new(
            room.at.x + room.size.x - size.x,
            room.middle().y - size.y / 2.0,
        );
        let color = if on {
            node.accent
        } else {
            faded(look.text, 0.25)
        };
        let switch = Rect { at, size };
        paint(
            canvas,
            switch,
            origin,
            &plain(color),
            size.y / 2.0,
            None,
            disc,
        );
        let inset = size.y * 0.15;
        let knob = size.y - inset * 2.0;
        let x = if on {
            at.x + size.x - inset - knob
        } else {
            at.x + inset
        };
        let knob = Rect {
            at: Vec2::new(x, at.y + inset),
            size: Vec2::splat(knob),
        };
        paint(
            canvas,
            knob,
            origin,
            &plain(look.text),
            knob.size.y / 2.0,
            None,
            disc,
        );
    }

    /// Draws drop-down `i`'s arrow at the end of its room, pointing down, in its look's text's
    /// color.
    fn draw_arrow(&self, i: usize, canvas: &mut Canvas<'_>, origin: Vec2, look: &Look) {
        let node = &self.nodes[i];
        let room = self.laid.rects[i].inside(node.padding);
        let wide = ARROW * node.text_size;
        let tip = Vec2::new(
            room.at.x + room.size.x - wide / 2.0,
            room.middle().y + wide / 4.0,
        );
        let thick = node.text_size * 0.09;
        for side in [-1.0, 1.0] {
            let from = tip + Vec2::new(side * wide / 2.0, -wide / 2.0);
            stroke(canvas, from, tip, thick, look.text, origin);
        }
    }

    /// Draws text field `i`'s text, scrolled as its caret has it while it's focused, or its hint,
    /// faded, while it's empty; and while it's focused, its selection behind its text and its
    /// caret, in its accent; all within its room, less its padding, and what clips it.
    fn draw_typed(&self, i: usize, canvas: &mut Canvas<'_>, origin: Vec2, look: &Look) {
        let node = &self.nodes[i];
        let Widget::Field { hint, .. } = &node.widget else {
            return;
        };
        let room = self.laid.rects[i].inside(node.padding);
        let clip = self.laid.clips[i].map_or(room, |clip| room.within(clip));
        if clip.size.cmple(Vec2::ZERO).any() {
            return;
        }
        canvas.set_clip(Some(corners(clip, origin)));
        let focused = self.typing() == Some(i);
        let scroll = if focused { self.caret.scroll } else { 0.0 };
        let (x, middle) = (room.at.x - scroll, room.middle().y);
        let tall = LINE * node.text_size;
        let span = |from: f32, to: f32| Rect {
            at: Vec2::new(x + from, middle - tall / 2.0),
            size: Vec2::new(to - from, tall),
        };
        let [at, from] = self.caret.xs;
        if focused && self.caret.at != self.caret.from {
            let selected = span(at.min(from), at.max(from));
            let over = Sprite {
                color: faded(node.accent, 0.45),
                ..Sprite::default()
            };
            canvas.draw_sprite(super::over(selected, origin), over);
        }
        let (text, color) = match node.text.is_empty() {
            true => (hint.as_str(), faded(look.text, 0.45)),
            false => (node.text.as_str(), look.text),
        };
        if !text.is_empty() {
            let style = crate::TextStyle {
                align: [Align::Start, Align::Center],
                ..node.style(color)
            };
            let place = Mat4::from_translation(on_canvas(Vec2::new(x, middle), origin));
            canvas.draw_text(place, text, &style);
        }
        if focused {
            let caret = Sprite {
                color: node.accent,
                ..Sprite::default()
            };
            canvas.draw_sprite(super::over(span(at, at + CARET), origin), caret);
        }
    }

    /// Draws the scroll bar of each list that shows whose nodes end with node `i`, the innermost
    /// first, over all it holds, as [`draw_bar`](Self::draw_bar) draws it, within what clips the
    /// list: as a UI draws its nodes in their order, after its last.
    pub(super) fn draw_bars(
        &self,
        i: usize,
        canvas: &mut Canvas<'_>,
        origin: Vec2,
        disc: Option<Texture>,
    ) {
        let ends = |node: &usize| self.nodes[*node].end as usize == i + 1;
        let mut at = Some(i);
        while let Some(node) = at.filter(ends) {
            if self.nodes[node].kind == Kind::List && self.laid.shown[node] {
                canvas.set_clip(self.laid.clips[node].map(|clip| corners(clip, origin)));
                self.draw_bar(node, canvas, origin, disc);
            }
            at = self.nodes[node].parent.map(|parent| parent as usize);
        }
    }

    /// Draws list `i`'s scroll bar inside its right edge, its thumb as long of its room as its room
    /// is of what its children reach, and as far down it as it's scrolled, in its accent; and
    /// nothing while it has nothing to scroll.
    fn draw_bar(&self, i: usize, canvas: &mut Canvas<'_>, origin: Vec2, disc: Option<Texture>) {
        let node = &self.nodes[i];
        let most = self.laid.most_scroll(&self.nodes, i);
        if most <= 0.0 {
            return;
        }
        let rect = self.laid.rects[i];
        let room = rect.inside(node.padding);
        let long = (room.size.y * room.size.y / (room.size.y + most))
            .max(SHORTEST)
            .min(room.size.y);
        let down = (room.size.y - long) * (node.scroll / most).clamp(0.0, 1.0);
        let thumb = Rect {
            at: Vec2::new(rect.at.x + rect.size.x - BAR * 1.5, room.at.y + down),
            size: Vec2::new(BAR, long),
        };
        paint(
            canvas,
            thumb,
            origin,
            &plain(node.accent),
            BAR / 2.0,
            None,
            disc,
        );
    }

    /// Draws open drop-down `open`'s options over the rest, unclipped, from `origin`: behind them
    /// its backing, each in its text's look, the lit one over its accent, the chosen one in its
    /// focused look's text.
    pub(super) fn draw_options(
        &self,
        open: Open,
        canvas: &mut Canvas<'_>,
        origin: Vec2,
        disc: Option<Texture>,
    ) {
        let node = &self.nodes[open.node];
        let Widget::Dropdown {
            options,
            choice,
            backing,
        } = &node.widget
        else {
            return;
        };
        let (first, count) = self.options(open.node);
        canvas.set_clip(None);
        let all = Rect {
            at: first.at,
            size: Vec2::new(first.size.x, first.size.y * count as f32),
        };
        paint(
            canvas,
            all,
            origin,
            &plain(*backing),
            node.radius,
            None,
            disc,
        );
        for (k, option) in options.iter().enumerate() {
            let rect = Rect {
                at: first.at + Vec2::new(0.0, first.size.y * k as f32),
                size: first.size,
            };
            if k == open.lit {
                paint(
                    canvas,
                    rect,
                    origin,
                    &plain(node.accent),
                    node.radius,
                    None,
                    disc,
                );
            }
            let look = if k == *choice { FOCUSED } else { 0 };
            write(canvas, node, option, rect, origin, node.looks[look].text);
        }
    }
}

/// Draws a stroke on `canvas` from `from` to `to`, in the canvas's units from the view's top left,
/// `thick` across, in `color`, from `origin`.
fn stroke(canvas: &mut Canvas<'_>, from: Vec2, to: Vec2, thick: f32, color: Vec4, origin: Vec2) {
    let (a, b) = (on_canvas(from, origin), on_canvas(to, origin));
    let way = b - a;
    let turned = Mat4::from_rotation_z(atan2f(way.y, way.x));
    let transform = Mat4::from_translation((a + b) / 2.0)
        * turned
        * Mat4::from_scale(Vec3::new(way.length() + thick, thick, 1.0));
    let sprite = Sprite {
        color,
        ..Sprite::default()
    };
    canvas.draw_sprite(transform, sprite);
}
