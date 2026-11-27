//! What input does to a UI: events, the focus, widgets' values and text editing.

use gg_core::math::{Vec2, Vec4};

#[cfg(doc)]
use super::GameUi;
use super::layout::Rect;
use super::{Caret, Kind, Open, Press, State, UiEvent, UiValue, Widget, widgets};
use crate::canvas::CANVAS_HEIGHT;
use crate::font::Fonts;
use crate::{Event, Input, Key, MouseButton, PadButton, Stick, Toward};

impl State {
    /// See [`GameUi::event`]. `cursor` is in canvas units from the view's top left.
    pub(super) fn event(
        &mut self,
        event: &Event<'_>,
        cursor: Option<Vec2>,
        input: &Input,
        fonts: &mut Fonts,
    ) -> Option<UiEvent> {
        if let Some(open) = self.open {
            if self.focusable(open.node) {
                return self.event_open(open, event, cursor, input);
            }
            // Hidden or disabled since it opened.
            self.open = None;
        }
        if let Some(field) = self.typing() {
            self.fix_caret(field);
        }
        let shift = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
        match *event {
            Event::Key {
                key,
                pressed: true,
                repeat,
            } => {
                if let Some(field) = self.typing()
                    && let Some(edited) = self.edit(field, key, repeat, input)
                {
                    return edited;
                }
                match key {
                    Key::ArrowLeft => self.arrow(Toward::Left),
                    Key::ArrowRight => self.arrow(Toward::Right),
                    Key::ArrowUp => self.arrow(Toward::Up),
                    Key::ArrowDown => self.arrow(Toward::Down),
                    Key::Home | Key::End => {
                        let slider = self
                            .focused()
                            .filter(|&i| self.nodes[i].kind == Kind::Slider);
                        self.slide_to_end(slider?, key == Key::End)
                    }
                    Key::Tab => self.tab(shift),
                    Key::Enter | Key::NumpadEnter | Key::Space if !repeat => {
                        self.activate(Press::Key)
                    }
                    Key::Escape if !repeat => Some(UiEvent::Back),
                    _ => None,
                }
            }
            Event::Key {
                key: Key::Enter | Key::NumpadEnter | Key::Space,
                pressed: false,
                ..
            } => self.release(Press::Key),
            Event::Text(typed) => self.type_in(self.typing()?, typed),
            // The first line, minus control characters: a code copied from a chat may carry a
            // tab or a line end.
            Event::Paste(pasted) => {
                let field = self.typing()?;
                let line = pasted.lines().next().unwrap_or_default();
                let line: String = line.chars().filter(|c| !c.is_control()).collect();
                self.type_in(field, &line)
            }
            Event::PadButton {
                button,
                pressed: true,
                ..
            } => match button {
                PadButton::DPadLeft => self.arrow(Toward::Left),
                PadButton::DPadRight => self.arrow(Toward::Right),
                PadButton::DPadUp => self.arrow(Toward::Up),
                PadButton::DPadDown => self.arrow(Toward::Down),
                PadButton::South => self.activate(Press::Pad),
                PadButton::East => Some(UiEvent::Back),
                _ => None,
            },
            Event::PadButton {
                button: PadButton::South,
                pressed: false,
                ..
            } => self.release(Press::Pad),
            Event::PadStick {
                stick: Stick::Left,
                value,
                ..
            } => {
                let toward = self.push(value)?;
                self.arrow(toward)
            }
            Event::CursorMoved { .. } => {
                if let Some((held, Press::Mouse)) = self.pressed {
                    // A drag ends if its widget gets hidden or disabled.
                    if !self.focusable(held) {
                        self.pressed = None;
                    } else if self.nodes[held].kind == Kind::Slider {
                        return self.slide_to(held, cursor?.x);
                    } else if self.nodes[held].kind == Kind::Field {
                        self.caret.at = self.caret_at(held, cursor?.x, fonts);
                        return None;
                    }
                }
                self.hovered = cursor.and_then(|at| self.widget_at(at));
                match self.hovered {
                    // A text field takes the focus by a click and keeps it as the cursor passes
                    // over the rest, so typing goes where it was clicked.
                    Some(over)
                        if self.focus != Some(over)
                            && self.nodes[over].kind != Kind::Field
                            && self.typing().is_none() =>
                    {
                        self.focus_on(over)
                    }
                    _ => None,
                }
            }
            Event::MouseButton {
                button: MouseButton::Left,
                pressed,
            } => {
                self.hovered = cursor.and_then(|at| self.widget_at(at));
                if pressed {
                    let (over, at) = (self.hovered?, cursor?);
                    self.pressed = Some((over, Press::Mouse));
                    let focused = (self.focus != Some(over))
                        .then(|| self.focus_on(over))
                        .flatten();
                    return match self.nodes[over].kind {
                        Kind::Slider => self.slide_to(over, at.x).or(focused),
                        Kind::Field => {
                            let to = self.caret_at(over, at.x, fonts);
                            (self.caret.at, self.caret.from) = (to, to);
                            focused
                        }
                        _ => focused,
                    };
                }
                let Some((held, Press::Mouse)) = self.pressed else {
                    return None;
                };
                self.pressed = None;
                if self.hovered != Some(held) {
                    return None;
                }
                match self.nodes[held].kind {
                    Kind::Button => Some(UiEvent::Pressed(self.name_of(held))),
                    Kind::Toggle => self.flip(held),
                    Kind::Dropdown => self.open_options(held),
                    _ => None,
                }
            }
            Event::Scroll(scroll) => {
                let list = self.list_at(cursor?)?;
                // A touchpad's pixels count as points.
                self.scroll_by(list, -scroll.lines(1.0) * LINE);
                None
            }
            Event::CursorLeft => {
                self.hovered = None;
                None
            }
            Event::Focused(false) => {
                (self.pressed, self.pushed) = (None, false);
                None
            }
            _ => None,
        }
    }

    /// What `event` does while drop-down `open` is open, which takes every event:
    /// - Up and down arrows, D-pad and left stick light the option above or below.
    /// - Enter, Space or pad South choose the lit option, closing it; so does letting go of a
    ///   click over an option.
    /// - Escape or pad East close it unchanged; so does a click outside the options, and Tab, which
    ///   also moves the focus on.
    fn event_open(
        &mut self,
        open: Open,
        event: &Event<'_>,
        cursor: Option<Vec2>,
        input: &Input,
    ) -> Option<UiEvent> {
        match *event {
            Event::Key {
                key,
                pressed: true,
                repeat,
            } => match key {
                Key::ArrowUp => self.light(-1),
                Key::ArrowDown => self.light(1),
                Key::Enter | Key::NumpadEnter | Key::Space if !repeat => self.choose(open.lit),
                Key::Escape if !repeat => {
                    self.open = None;
                    None
                }
                Key::Tab => {
                    self.open = None;
                    let back = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
                    self.tab(back)
                }
                _ => None,
            },
            Event::PadButton {
                button,
                pressed: true,
                ..
            } => match button {
                PadButton::DPadUp => self.light(-1),
                PadButton::DPadDown => self.light(1),
                PadButton::South => self.choose(open.lit),
                PadButton::East => {
                    self.open = None;
                    None
                }
                _ => None,
            },
            Event::PadStick {
                stick: Stick::Left,
                value,
                ..
            } => match self.push(value)? {
                Toward::Up => self.light(-1),
                Toward::Down => self.light(1),
                Toward::Left | Toward::Right => None,
            },
            Event::CursorMoved { .. } => {
                if let Some(option) = cursor.and_then(|at| self.option_at(open.node, at)) {
                    self.open = Some(Open {
                        lit: option,
                        ..open
                    });
                }
                None
            }
            Event::MouseButton {
                button: MouseButton::Left,
                pressed,
            } => {
                let option = cursor.and_then(|at| self.option_at(open.node, at));
                match (pressed, option) {
                    (true, Some(option)) => {
                        self.open = Some(Open {
                            node: open.node,
                            lit: option,
                            held: true,
                        });
                        None
                    }
                    (true, None) => {
                        (self.open, self.pressed) = (None, None);
                        None
                    }
                    (false, Some(option)) if open.held => self.choose(option),
                    (false, _) => {
                        self.open = Some(Open {
                            held: false,
                            ..open
                        });
                        None
                    }
                }
            }
            Event::Focused(false) => {
                self.pushed = false;
                None
            }
            _ => None,
        }
    }

    /// Which way the left stick at `value` steps, once per push. See [`GameUi::event`].
    fn push(&mut self, value: Vec2) -> Option<Toward> {
        let length = value.length();
        if length < STICK_RESTS {
            self.pushed = false;
            return None;
        }
        if self.pushed || length < STICK_STEPS {
            return None;
        }
        self.pushed = true;
        Some(match value.x.abs() > value.y.abs() {
            true if value.x < 0.0 => Toward::Left,
            true => Toward::Right,
            false if value.y < 0.0 => Toward::Down,
            false => Toward::Up,
        })
    }

    /// What an arrow, D-pad or stick `toward` a way does: left and right step the focused
    /// slider's value or drop-down's choice or move a text field's caret; otherwise each
    /// [steps](Self::step) the focus.
    fn arrow(&mut self, toward: Toward) -> Option<UiEvent> {
        let by = match toward {
            Toward::Left => -1,
            Toward::Right => 1,
            Toward::Up | Toward::Down => return self.step(toward),
        };
        let Some(focused) = self.focused() else {
            return self.step(toward);
        };
        match self.nodes[focused].kind {
            Kind::Slider => self.slide(focused, by > 0),
            Kind::Dropdown => self.choose_by(focused, by),
            Kind::Field => {
                self.move_caret(focused, by, false);
                None
            }
            _ => self.step(toward),
        }
    }

    /// What Enter, Space or pad South (`by`) does to the focused node: presses a button or flips a
    /// toggle, each held until release; opens a drop-down; presses a text field. With none
    /// focused, [`focus_first`](Self::focus_first).
    fn activate(&mut self, by: Press) -> Option<UiEvent> {
        let Some(focused) = self.focused() else {
            return self.focus_first();
        };
        match self.nodes[focused].kind {
            Kind::Button => {
                self.pressed = Some((focused, by));
                Some(UiEvent::Pressed(self.name_of(focused)))
            }
            Kind::Toggle => {
                self.pressed = Some((focused, by));
                self.flip(focused)
            }
            Kind::Dropdown => self.open_options(focused),
            Kind::Field => Some(UiEvent::Pressed(self.name_of(focused))),
            _ => None,
        }
    }

    /// Widget `i`'s change to its current value, which input set. Marked so a reload keeps it.
    fn changed(&mut self, i: usize) -> Option<UiEvent> {
        self.set[i].value = true;
        let value = widgets::value_of(&self.nodes[i])?;
        Some(UiEvent::Changed(self.name_of(i), value))
    }

    /// Steps slider `i` to the next point of its step grid (from `min`) up if `up`, else down,
    /// within its bounds; past the last point, to `max`. Off the grid, it goes to the nearest
    /// point that way. A value within 1e-5 of a step of a point counts as on it, since floats
    /// can't hold the multiples exactly.
    fn slide(&mut self, i: usize, up: bool) -> Option<UiEvent> {
        let Widget::Slider {
            min,
            max,
            step,
            value,
        } = self.nodes[i].widget
        else {
            return None;
        };
        let step = if step > 0.0 {
            step
        } else {
            (max - min) / STEPS
        };
        let steps = (value - min) / step;
        let point = match up {
            true => (steps + 1e-5).floor() + 1.0,
            false => (steps - 1e-5).ceil() - 1.0,
        };
        self.set_slider(i, (min + point * step).clamp(min, max))
    }

    fn slide_to_end(&mut self, i: usize, end: bool) -> Option<UiEvent> {
        let Widget::Slider { min, max, .. } = self.nodes[i].widget else {
            return None;
        };
        self.set_slider(i, if end { max } else { min })
    }

    /// Sets slider `i` to where `x` (canvas units from the view's left) is along its track: the
    /// nearest grid point from `min`, or `max` if nearer, as at the track's end.
    fn slide_to(&mut self, i: usize, x: f32) -> Option<UiEvent> {
        let Widget::Slider { min, max, step, .. } = self.nodes[i].widget else {
            return None;
        };
        let track = widgets::track(&self.nodes[i], self.laid.rects[i]);
        let along = ((x - track.at.x) / track.size.x).clamp(0.0, 1.0);
        let value = min + along * (max - min);
        let value = if step > 0.0 {
            let point = (min + ((value - min) / step).round() * step).min(max);
            if max - value < (value - point).abs() {
                max
            } else {
                point
            }
        } else {
            value
        };
        // A zero-width track gives NaN.
        if !value.is_finite() {
            return None;
        }
        self.set_slider(i, value)
    }

    /// Sets slider `i` to `to`; a change if it differs.
    fn set_slider(&mut self, i: usize, to: f32) -> Option<UiEvent> {
        let Widget::Slider { value, .. } = &mut self.nodes[i].widget else {
            return None;
        };
        if *value == to {
            return None;
        }
        *value = to;
        self.changed(i)
    }

    fn choose_by(&mut self, i: usize, by: isize) -> Option<UiEvent> {
        let Widget::Dropdown {
            options, choice, ..
        } = &self.nodes[i].widget
        else {
            return None;
        };
        let to = choice.saturating_add_signed(by).min(options.len() - 1);
        self.set_choice(i, to)
    }

    /// Sets drop-down `i` to option `to`; a change if it differs.
    fn set_choice(&mut self, i: usize, to: usize) -> Option<UiEvent> {
        let Widget::Dropdown { choice, .. } = &mut self.nodes[i].widget else {
            return None;
        };
        if *choice == to {
            return None;
        }
        *choice = to;
        self.changed(i)
    }

    fn flip(&mut self, i: usize) -> Option<UiEvent> {
        let Widget::Toggle { on } = &mut self.nodes[i].widget else {
            return None;
        };
        *on = !*on;
        self.changed(i)
    }

    fn open_options(&mut self, i: usize) -> Option<UiEvent> {
        let Widget::Dropdown { choice, .. } = self.nodes[i].widget else {
            return None;
        };
        self.open = Some(Open {
            node: i,
            lit: choice,
            held: false,
        });
        self.pressed = None;
        None
    }

    fn light(&mut self, by: isize) -> Option<UiEvent> {
        let open = self.open.as_mut()?;
        let Widget::Dropdown { options, .. } = &self.nodes[open.node].widget else {
            return None;
        };
        open.lit = open.lit.saturating_add_signed(by).min(options.len() - 1);
        None
    }

    fn choose(&mut self, option: usize) -> Option<UiEvent> {
        let open = self.open.take()?;
        self.set_choice(open.node, option)
    }

    /// Drop-down `i`'s open options: the first's rectangle and the count. Each is the drop-down's
    /// size, stacked below it, or above where they'd pass the view's bottom and there's room.
    pub(super) fn options(&self, i: usize) -> (Rect, usize) {
        let count = match &self.nodes[i].widget {
            Widget::Dropdown { options, .. } => options.len(),
            _ => 0,
        };
        let rect = self.laid.rects[i];
        let all = rect.size.y * count as f32;
        let below = rect.at.y + rect.size.y;
        let top = if below + all > CANVAS_HEIGHT && rect.at.y >= all {
            rect.at.y - all
        } else {
            below
        };
        let first = Rect {
            at: Vec2::new(rect.at.x, top),
            size: rect.size,
        };
        (first, count)
    }

    fn option_at(&self, i: usize, at: Vec2) -> Option<usize> {
        let (first, count) = self.options(i);
        let all = Rect {
            at: first.at,
            size: Vec2::new(first.size.x, first.size.y * count as f32),
        };
        all.contains(at)
            .then(|| (((at.y - first.at.y) / first.size.y) as usize).min(count - 1))
    }

    /// Whether the UI takes input and drew within the last `frames` frames handed off, or this
    /// one. A UI that didn't, like a menu no longer drawn, takes nothing from the layers below.
    pub(super) fn takes_input(&self, frames: u64) -> bool {
        self.input && self.drawn.is_some_and(|drawn| drawn + 1 >= frames)
    }

    /// Whether a drop-down's options are open and take every event. Not if it was hidden or
    /// disabled since; they close at the next event.
    fn options_open(&self) -> bool {
        self.open.is_some_and(|open| self.focusable(open.node))
    }

    /// Whether `at` is over something a pointer works on: a shown button or widget, disabled or
    /// not, or a list, within its clip; or anywhere while options are open. Panels, texts and
    /// images aren't, so a HUD doesn't take the game's clicks. A panel over one doesn't lift the
    /// pointer off it: it keeps the pointer from the layers below as well as from the one it covers.
    pub(super) fn pointer_over(&self, at: Vec2) -> bool {
        self.options_open()
            || (0..self.nodes.len()).any(|i| {
                (self.nodes[i].kind.focuses() || self.nodes[i].kind == Kind::List)
                    && self.laid.shown[i]
                    && self.laid.rects[i].contains(at)
                    && self.laid.within(i, at)
            })
    }

    /// Whether it takes `event` away from the layers below:
    /// - Mouse buttons and wheel: over [`pointer_over`](Self::pointer_over), anywhere while options
    ///   are open, and a release after a press it holds, anywhere.
    /// - Keys and typed text, not the pad: while a text field has the focus.
    /// - Nothing if it takes no input or didn't draw within the last `frames` frames.
    pub(super) fn takes(&self, event: &Event<'_>, cursor: Option<Vec2>, frames: u64) -> bool {
        if !self.takes_input(frames) {
            return false;
        }
        let held = matches!(self.pressed, Some((_, Press::Mouse)));
        match *event {
            Event::MouseButton { .. } => {
                self.options_open() || held || cursor.is_some_and(|at| self.pointer_over(at))
            }
            Event::Scroll(_) => {
                self.options_open() || cursor.is_some_and(|at| self.pointer_over(at))
            }
            Event::Key { .. } | Event::Text(_) | Event::Paste(_) => self.typing().is_some(),
            _ => false,
        }
    }

    /// The focusable node under `at` that the pointer reaches: the last drawn of those whose
    /// clipped rectangle holds it, none under a panel drawn after it ([`Self::covers`]).
    fn widget_at(&self, at: Vec2) -> Option<usize> {
        self.reached(at, |i| self.focusable(i))
    }

    /// The shown button or widget under `at`, enabled or not, that the pointer reaches. For
    /// [`GameUi::hovered`].
    pub(super) fn pointed_at(&self, at: Vec2) -> Option<usize> {
        self.reached(at, |i| self.nodes[i].kind.focuses() && self.laid.shown[i])
    }

    fn list_at(&self, at: Vec2) -> Option<usize> {
        self.reached(at, |i| {
            self.nodes[i].kind == Kind::List && self.laid.shown[i]
        })
    }

    /// The last drawn node `wanted` accepts whose rectangle, within its clip, holds `at`, unless a
    /// panel drawn after it covers `at` first. One pass from the last node back: a panel found
    /// covers every node before it.
    fn reached(&self, at: Vec2, wanted: impl Fn(usize) -> bool) -> Option<usize> {
        // The last covering panel drawn so far: one past a node's subtree covers it, one within
        // doesn't, as a list's panel rows don't cover the list they scroll in.
        let mut last: Option<usize> = None;
        for i in (0..self.nodes.len()).rev() {
            let covered = last.is_some_and(|panel| panel >= self.nodes[i].end as usize);
            if self.nodes[i].kind == Kind::Panel {
                if last.is_none() && self.covers(i, at) {
                    last = Some(i);
                }
            } else if !covered
                && wanted(i)
                && self.laid.rects[i].contains(at)
                && self.laid.within(i, at)
            {
                return Some(i);
            }
        }
        None
    }

    /// Whether panel `i` is shown and holds `at`, within its clip.
    fn covers(&self, i: usize, at: Vec2) -> bool {
        self.laid.shown[i] && self.laid.rects[i].contains(at) && self.laid.within(i, at)
    }

    /// Whether a panel drawn after node `i`, past its subtree, wholly covers what shows of it, so a
    /// key or pad move skips it. Ceiling: a UI of thousands of nodes moving the focus, a pass for
    /// each node.
    fn covered(&self, i: usize) -> bool {
        let seen = |node: usize| {
            let rect = self.laid.rects[node];
            self.laid.clips[node].map_or(rect, |clip| rect.within(clip))
        };
        let node = seen(i);
        let end = node.at + node.size;
        node.size.cmpgt(Vec2::ZERO).all()
            && (self.nodes[i].end as usize..self.nodes.len()).any(|j| {
                let panel = seen(j);
                self.nodes[j].kind == Kind::Panel
                    && self.laid.shown[j]
                    && panel.at.cmple(node.at).all()
                    && end.cmple(panel.at + panel.size).all()
            })
    }

    /// Whether a key or pad move may focus node `i`: focusable and not wholly covered.
    fn reachable(&self, i: usize) -> bool {
        self.focusable(i) && !self.covered(i)
    }

    /// Scrolls list `i` down `by` units, within its range. If it moved, it's laid out again at the
    /// next draw.
    fn scroll_by(&mut self, i: usize, by: f32) {
        let most = self.laid.most_scroll(&self.nodes, i);
        let to = (self.nodes[i].scroll + by).clamp(0.0, most);
        if to != self.nodes[i].scroll {
            self.nodes[i].scroll = to;
            self.laid_for = None;
        }
    }

    /// Scrolls each list holding node `i`, innermost first, to show it whole, or as much as fits.
    /// For the focus moving by keys or a pad.
    fn reveal(&mut self, i: usize) {
        // How far the inner lists' scrolling moved the node up.
        let mut moved = 0.0;
        let mut at = self.nodes[i].parent;
        while let Some(list) = at.map(|list| list as usize) {
            at = self.nodes[list].parent;
            if self.nodes[list].kind != Kind::List {
                continue;
            }
            let rect = self.laid.rects[i];
            let (top, bottom) = (rect.at.y - moved, rect.at.y + rect.size.y - moved);
            let room = self.laid.rects[list].inside(self.nodes[list].padding);
            let by = if bottom > room.at.y + room.size.y && top > room.at.y {
                (bottom - (room.at.y + room.size.y)).min(top - room.at.y)
            } else if top < room.at.y {
                top - room.at.y
            } else {
                0.0
            };
            let was = self.nodes[list].scroll;
            self.scroll_by(list, by);
            moved += self.nodes[list].scroll - was;
        }
    }

    fn focus_on(&mut self, i: usize) -> Option<UiEvent> {
        self.focus = Some(i);
        if self.nodes[i].kind == Kind::Field {
            let end = self.nodes[i].text.len();
            self.caret = Caret {
                at: end,
                from: end,
                ..Caret::default()
            };
        }
        Some(UiEvent::Focused(self.name_of(i)))
    }

    /// [`focus_on`](Self::focus_on) node `i`, [revealed](Self::reveal).
    pub(super) fn focus_to(&mut self, i: usize) -> Option<UiEvent> {
        self.reveal(i);
        self.focus_on(i)
    }

    /// Focuses the node its file's `focus` names if focusable, else the first that is. For a step
    /// or press with none focused.
    fn focus_first(&mut self) -> Option<UiEvent> {
        let first = self.file_focus.filter(|&i| self.reachable(i));
        let first = first.or_else(|| (0..self.nodes.len()).find(|&i| self.reachable(i)));
        self.focus_to(first?)
    }

    /// Moves the focus `toward` a way, to the nearest focusable node whose middle is that way of
    /// the focused one's, scrolled into view. Nodes level with it across the way (a row's, going
    /// left or right) win by distance along the way; else distance along plus twice the
    /// sideways offset. With none focused, [`focus_first`](Self::focus_first).
    fn step(&mut self, toward: Toward) -> Option<UiEvent> {
        let Some(from) = self.focused() else {
            return self.focus_first();
        };
        let (way, aside) = match toward {
            Toward::Left => (Vec2::NEG_X, 1),
            Toward::Right => (Vec2::X, 1),
            Toward::Up => (Vec2::NEG_Y, 0),
            Toward::Down => (Vec2::Y, 0),
        };
        let here = self.laid.rects[from];
        let mut nearest: Option<(usize, (bool, f32))> = None;
        for i in (0..self.nodes.len()).filter(|&i| i != from && self.reachable(i)) {
            let there = self.laid.rects[i];
            let off = there.middle() - here.middle();
            let along = off.dot(way);
            if along <= 1e-3 {
                continue;
            }
            let level = there.at[aside] < here.at[aside] + here.size[aside]
                && here.at[aside] < there.at[aside] + there.size[aside];
            let far = match level {
                true => (false, along),
                false => (true, along + off[aside].abs() * 2.0),
            };
            if nearest.is_none_or(|(_, nearest)| far < nearest) {
                nearest = Some((i, far));
            }
        }
        let (to, _) = nearest?;
        self.focus_to(to)
    }

    /// Moves the focus to the next focusable node in file order, or the previous if `back`,
    /// wrapping round and scrolled into view. With none focused, to the first, or the last.
    fn tab(&mut self, back: bool) -> Option<UiEvent> {
        let count = self.nodes.len();
        let next = match self.focused() {
            None if back => (0..count).rev().find(|&i| self.reachable(i)),
            None => (0..count).find(|&i| self.reachable(i)),
            Some(at) => (1..=count)
                .map(|by| match back {
                    true => (at + count - by) % count,
                    false => (at + by) % count,
                })
                .find(|&i| self.reachable(i)),
        };
        match next {
            Some(next) if self.focus != Some(next) => self.focus_to(next),
            _ => None,
        }
    }

    fn release(&mut self, by: Press) -> Option<UiEvent> {
        if self.pressed.is_some_and(|(_, held)| held == by) {
            self.pressed = None;
        }
        None
    }

    /// What pressed `key` does to focused text field `i`; `None` for a key it leaves to the rest
    /// (Escape, Tab, up and down arrows, which move the focus).
    /// - Left and right arrows move the caret a character, past a selection; Home and End, to the
    ///   start and end. Shift selects as far. Ctrl+A selects all.
    /// - Backspace and Delete cut the selection, or the character before or after the caret.
    /// - Ctrl+C copies the selection, Ctrl+X cuts it too (Cmd on macOS), kept in
    ///   [`copied`](Self::copied). Ctrl+V waits for the [`Event::Paste`] that follows.
    /// - Enter presses it. Space types, as its text event follows.
    fn edit(&mut self, i: usize, key: Key, repeat: bool, input: &Input) -> Option<Option<UiEvent>> {
        let shift = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
        let control = input.key_down(Key::ControlLeft) || input.key_down(Key::ControlRight);
        let command = input.command_down();
        let text = &self.nodes[i].text;
        let Caret { at, from, .. } = self.caret;
        let (low, high) = (at.min(from), at.max(from));
        match key {
            Key::KeyC | Key::KeyX if command => {
                if low == high {
                    return Some(None);
                }
                self.copied = Some(text[low..high].to_owned());
                Some(if key == Key::KeyX {
                    self.splice(i, low..high, "")
                } else {
                    None
                })
            }
            Key::KeyV if command => Some(None),
            Key::ArrowLeft | Key::ArrowRight => {
                self.move_caret(i, if key == Key::ArrowLeft { -1 } else { 1 }, shift);
                Some(None)
            }
            Key::Home | Key::End => {
                self.caret.at = if key == Key::Home { 0 } else { text.len() };
                if !shift {
                    self.caret.from = self.caret.at;
                }
                Some(None)
            }
            Key::KeyA if control => {
                (self.caret.from, self.caret.at) = (0, text.len());
                Some(None)
            }
            Key::Backspace | Key::NumpadBackspace | Key::Delete => {
                let cut = if low != high {
                    low..high
                } else if key == Key::Delete {
                    at..widgets::after(text, at)
                } else {
                    widgets::before(text, at)..at
                };
                Some(if cut.is_empty() {
                    None
                } else {
                    self.splice(i, cut, "")
                })
            }
            Key::Enter | Key::NumpadEnter => {
                Some((!repeat).then(|| UiEvent::Pressed(self.name_of(i))))
            }
            Key::Space => Some(None),
            _ => None,
        }
    }

    /// Keeps text field `i`'s caret and selection end on character boundaries within its text,
    /// which code or a reload may have cut since.
    fn fix_caret(&mut self, i: usize) {
        let text = &self.nodes[i].text;
        let valid = |byte: usize| text.is_char_boundary(byte).then_some(byte);
        self.caret.at = valid(self.caret.at).unwrap_or(text.len());
        self.caret.from = valid(self.caret.from).unwrap_or(self.caret.at);
    }

    /// Moves text field `i`'s caret a character either way, past a selection unless `select`,
    /// which extends it.
    fn move_caret(&mut self, i: usize, by: isize, select: bool) {
        let text = &self.nodes[i].text;
        let Caret { at, from, .. } = self.caret;
        self.caret.at = match (by < 0, select || at == from) {
            (true, false) => at.min(from),
            (false, false) => at.max(from),
            (true, true) => widgets::before(text, at),
            (false, true) => widgets::after(text, at),
        };
        if !select {
            self.caret.from = self.caret.at;
        }
    }

    /// Types `typed` into text field `i` over the selection, as much as fits; nothing if it has a
    /// control character.
    fn type_in(&mut self, i: usize, typed: &str) -> Option<UiEvent> {
        let Widget::Field { most, .. } = self.nodes[i].widget else {
            return None;
        };
        if typed.chars().any(char::is_control) {
            return None;
        }
        let Caret { at, from, .. } = self.caret;
        let (low, high) = (at.min(from), at.max(from));
        let text = &self.nodes[i].text;
        let kept = text[..low].chars().count() + text[high..].chars().count();
        let room = most.saturating_sub(kept);
        let fits = typed
            .char_indices()
            .nth(room)
            .map_or(typed.len(), |(end, _)| end);
        if fits == 0 && low == high {
            return None;
        }
        self.splice(i, low..high, &typed[..fits])
    }

    fn splice(&mut self, i: usize, cut: std::ops::Range<usize>, put: &str) -> Option<UiEvent> {
        let start = cut.start;
        self.nodes[i].text.replace_range(cut, put);
        let caret = start + put.len();
        (self.caret.at, self.caret.from) = (caret, caret);
        self.set[i].text = true;
        Some(UiEvent::Changed(
            self.name_of(i),
            UiValue::Text(self.nodes[i].text.clone()),
        ))
    }

    /// The character-boundary byte of text field `i`'s text nearest `x` (canvas units from the
    /// view's left), as last drawn.
    fn caret_at(&self, i: usize, x: f32, fonts: &mut Fonts) -> usize {
        let node = &self.nodes[i];
        let room = self.laid.rects[i].inside(node.padding);
        let scroll = if self.focus == Some(i) {
            self.caret.scroll
        } else {
            0.0
        };
        let x = x - room.at.x + scroll;
        let style = node.style(Vec4::ONE);
        let text = &node.text;
        let byte = |k: usize| text.char_indices().nth(k).map_or(text.len(), |(at, _)| at);
        let mut width = |k: usize| fonts.measure(&text[..byte(k)], &style).x;
        // Bisect for the first boundary at or past `x`; widths grow with the boundary.
        let (mut low, mut high) = (0, text.chars().count());
        while low < high {
            let middle = (low + high) / 2;
            if width(middle) < x {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if low > 0 && x - width(low - 1) < width(low) - x {
            low -= 1;
        }
        byte(low)
    }

    /// Scrolls the focused text field's text as little as shows its caret, and measures where the
    /// caret and selection end lie along it, for drawing.
    pub(super) fn fit_caret(&mut self, fonts: &mut Fonts) {
        let Some(i) = self.typing() else {
            return;
        };
        self.fix_caret(i);
        let node = &self.nodes[i];
        let text = &node.text;
        let style = node.style(Vec4::ONE);
        let xs =
            [self.caret.at, self.caret.from].map(|byte| fonts.measure(&text[..byte], &style).x);
        let room = self.laid.rects[i].inside(node.padding).size.x - widgets::CARET;
        let mut scroll = self.caret.scroll;
        if xs[0] - scroll > room {
            scroll = xs[0] - room;
        } else if xs[0] < scroll {
            scroll = xs[0];
        }
        let whole = fonts.measure(text, &style).x;
        self.caret.scroll = scroll.min(whole - room).max(0.0);
        self.caret.xs = xs;
    }
}

/// How far a stick goes to step the focus, and how near rest it returns before stepping again.
const STICK_STEPS: f32 = 0.6;
const STICK_RESTS: f32 = 0.3;

/// A wheel line's scroll of a list, in canvas units.
const LINE: f32 = 60.0;

/// Steps from `min` to `max` by keys and pad for a slider with no `step`.
const STEPS: f32 = 20.0;
