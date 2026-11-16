//! What input does to a UI: its events, the focus, widgets' values and text editing.

use gg_core::math::{Vec2, Vec4};

#[cfg(doc)]
use super::GameUi;
use super::layout::Rect;
use super::{Caret, Kind, Open, Press, State, UiEvent, UiValue, Widget, widgets};
use crate::canvas::CANVAS_HEIGHT;
use crate::font::Fonts;
use crate::{Event, Input, Key, MouseButton, PadButton, Stick, Toward};

impl State {
    /// What `event` does to it, the cursor at `cursor` on the view, in the canvas's units from its
    /// top left, the keys held as `input` has them, and text measured by `fonts`, as
    /// [`GameUi::event`] says.
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
            // Its first line, as a field holds one, without the characters no line holds, as a
            // code copied from a chat may come with a tab or a line's end.
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
                    // A drag ends as what it drags is hidden or disabled under it.
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
                    // A text field takes the focus by a click, and keeps it as the cursor passes
                    // over the rest, so that what's typed goes where it was clicked.
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
                // A touchpad's pixels counted as points.
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

    /// What `event` does to it while drop-down `open` is open over the rest, which takes every
    /// event: the up and down arrows, a pad's D-pad and left stick light the option above or
    /// below; Enter, Space or a pad's South choose the lit one, and so does a click on one, as
    /// it's let go over one, closing it; Escape or a pad's East close it as it was, and so does a
    /// click outside its options, and Tab, moving the focus on.
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

    /// Which way the left stick at `value` steps, once a push, as [`GameUi::event`] says.
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

    /// What an arrow, a pad's D-pad or its stick `toward` a way does: left and right step the
    /// focused slider's value or drop-down's choice, or move a focused text field's caret, and
    /// otherwise each moves the focus, as [`step`](Self::step) does.
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

    /// What Enter, Space or a pad's South, `by` which it's pressed, does to the node with the
    /// focus: presses a button, held until its release, flips a toggle, held so too, opens a
    /// drop-down, or presses a text field; and with none focused, focuses one, as
    /// [`focus_first`](Self::focus_first) does.
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

    /// The change of widget `i` to what it holds now, which input set, kept as its file is read
    /// again.
    fn changed(&mut self, i: usize) -> Option<UiEvent> {
        self.set[i].value = true;
        let value = widgets::value_of(&self.nodes[i])?;
        Some(UiEvent::Changed(self.name_of(i), value))
    }

    /// Steps slider `i`'s value to the next point of its steps' grid, from its `min`, up if `up`
    /// and else down, or to its `max` past the grid's last point, within its bounds: from a value
    /// off the grid, to the grid's nearest point that way. A step a hundred-thousandth of a step
    /// short of a point counts as on it, as the grid's points, multiples of a step that floats
    /// can't hold exactly, may lie.
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

    /// Sets slider `i`'s value to its `min`, or its `max` if `end`.
    fn slide_to_end(&mut self, i: usize, end: bool) -> Option<UiEvent> {
        let Widget::Slider { min, max, .. } = self.nodes[i].widget else {
            return None;
        };
        self.set_slider(i, if end { max } else { min })
    }

    /// Sets slider `i`'s value to where `x`, in the canvas's units from the view's left, is along
    /// its track, at the nearest point of its steps' grid, from its `min`, or its `max`, if that's
    /// nearer, as it is at the track's end.
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
        // A track of no width gives no number.
        if !value.is_finite() {
            return None;
        }
        self.set_slider(i, value)
    }

    /// Sets slider `i`'s value to `to`, a change if it held another.
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

    /// Steps drop-down `i`'s choice `by` options, within them.
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

    /// Sets drop-down `i`'s choice to option `to`, a change if it had another.
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

    /// Turns toggle `i` on, or off if it's on.
    fn flip(&mut self, i: usize) -> Option<UiEvent> {
        let Widget::Toggle { on } = &mut self.nodes[i].widget else {
            return None;
        };
        *on = !*on;
        self.changed(i)
    }

    /// Opens drop-down `i`'s options over the rest, its choice lit.
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

    /// Lights the option `by` options from the lit one of the open drop-down, within them.
    fn light(&mut self, by: isize) -> Option<UiEvent> {
        let open = self.open.as_mut()?;
        let Widget::Dropdown { options, .. } = &self.nodes[open.node].widget else {
            return None;
        };
        open.lit = open.lit.saturating_add_signed(by).min(options.len() - 1);
        None
    }

    /// Chooses option `option` of the open drop-down, closing it.
    fn choose(&mut self, option: usize) -> Option<UiEvent> {
        let open = self.open.take()?;
        self.set_choice(open.node, option)
    }

    /// The options of drop-down `i` as they open: the first's rectangle, each below the one before,
    /// as tall and wide as the drop-down, below it, or above it where they'd reach past the view's
    /// bottom and there's room there; and how many there are.
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

    /// The option of open drop-down `i` that `at` is over, if any.
    fn option_at(&self, i: usize, at: Vec2) -> Option<usize> {
        let (first, count) = self.options(i);
        let all = Rect {
            at: first.at,
            size: Vec2::new(first.size.x, first.size.y * count as f32),
        };
        all.contains(at)
            .then(|| (((at.y - first.at.y) / first.size.y) as usize).min(count - 1))
    }

    /// Whether the UI takes input and drew in the last of `frames` frames handed off, or this one:
    /// one that didn't, as a menu that's no longer drawn, takes nothing from the layers below.
    pub(super) fn takes_input(&self, frames: u64) -> bool {
        self.input && self.drawn.is_some_and(|drawn| drawn + 1 >= frames)
    }

    /// Whether a drop-down's options are open over the rest, which take every event: not those of
    /// one that's hidden or disabled since it opened, which close as the next event comes.
    fn options_open(&self) -> bool {
        self.open.is_some_and(|open| self.focusable(open.node))
    }

    /// Whether `at` is over what the UI draws that a pointer works on: a button or a widget that
    /// shows, disabled or not, or a list, within what clips it; or anywhere, while a drop-down's
    /// options are open, which take every event. Panels, texts and images aren't, so that a HUD
    /// doesn't take the game's clicks.
    pub(super) fn pointer_over(&self, at: Vec2) -> bool {
        self.options_open()
            || self.list_at(at).is_some()
            || (0..self.nodes.len()).any(|i| {
                self.nodes[i].kind.focuses()
                    && self.shows(i)
                    && self.laid.rects[i].contains(at)
                    && self.laid.within(i, at)
            })
    }

    /// Whether it takes `event`, the cursor at `cursor`, away from the layers below: the mouse's
    /// buttons, pressed or let go, and its wheel, over what [`pointer_over`](Self::pointer_over)
    /// says, wherever while a drop-down is open, and a button let go after a press it holds,
    /// wherever; the keys and the text typed, pad aside, while a text field has the focus; nothing
    /// while it takes no input or didn't draw in the last of `frames` frames.
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

    /// The topmost node that takes the focus whose rectangle `at` is in, within what clips it.
    fn widget_at(&self, at: Vec2) -> Option<usize> {
        (0..self.nodes.len()).rev().find(|&i| {
            self.focusable(i) && self.laid.rects[i].contains(at) && self.laid.within(i, at)
        })
    }

    /// The innermost list that shows whose rectangle `at` is in, within what clips it.
    fn list_at(&self, at: Vec2) -> Option<usize> {
        (0..self.nodes.len()).rev().find(|&i| {
            self.nodes[i].kind == Kind::List
                && self.laid.shown[i]
                && self.laid.rects[i].contains(at)
                && self.laid.within(i, at)
        })
    }

    /// Scrolls list `i` `by` units down its children, within what it has to scroll, laid out
    /// again as it next draws if it moved.
    fn scroll_by(&mut self, i: usize, by: f32) {
        let most = self.laid.most_scroll(&self.nodes, i);
        let to = (self.nodes[i].scroll + by).clamp(0.0, most);
        if to != self.nodes[i].scroll {
            self.nodes[i].scroll = to;
            self.laid_for = None;
        }
    }

    /// Scrolls each list node `i` is in as far as shows it whole, or as much of it as fits, the
    /// innermost first, as the focus moving to it by keys or a pad has it.
    fn reveal(&mut self, i: usize) {
        // How far the lists inside the one at hand moved the node up, as they scrolled.
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

    /// Moves the focus to node `i`, a text field's caret to the end of its text.
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

    /// Moves the focus to node `i`, as [`focus_on`](Self::focus_on) does, scrolling it into view,
    /// as [`reveal`](Self::reveal) does.
    pub(super) fn focus_to(&mut self, i: usize) -> Option<UiEvent> {
        self.reveal(i);
        self.focus_on(i)
    }

    /// Moves the focus to the node its file names, if it takes the focus, or else the first that
    /// does, as a step or a press does with none focused.
    fn focus_first(&mut self) -> Option<UiEvent> {
        let first = self.file_focus.filter(|&i| self.focusable(i));
        let first = first.or_else(|| (0..self.nodes.len()).find(|&i| self.focusable(i)));
        self.focus_to(first?)
    }

    /// Moves the focus `toward` a way, to the nearest node that takes it whose middle is that way
    /// of the focused one's: of those level with it, beside it across the way, as a row's are to
    /// the left and right, the nearest along the way; and else the nearest, as far along as twice
    /// as far off to the side, scrolled into view. With none focused, it focuses one, as
    /// [`focus_first`](Self::focus_first) does.
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
        for i in (0..self.nodes.len()).filter(|&i| i != from && self.focusable(i)) {
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

    /// Moves the focus to the next node that takes it in the file's order, or the one before if
    /// `back`, coming round from the last to the first, scrolled into view; with none focused, to
    /// the first, or the last.
    fn tab(&mut self, back: bool) -> Option<UiEvent> {
        let count = self.nodes.len();
        let next = match self.focused() {
            None if back => (0..count).rev().find(|&i| self.focusable(i)),
            None => (0..count).find(|&i| self.focusable(i)),
            Some(at) => (1..=count)
                .map(|by| match back {
                    true => (at + count - by) % count,
                    false => (at + by) % count,
                })
                .find(|&i| self.focusable(i)),
        };
        match next {
            Some(next) if self.focus != Some(next) => self.focus_to(next),
            _ => None,
        }
    }

    /// Lets go of the node held pressed `by` a key or a pad's button.
    fn release(&mut self, by: Press) -> Option<UiEvent> {
        if self.pressed.is_some_and(|(_, held)| held == by) {
            self.pressed = None;
        }
        None
    }

    /// What `key`, pressed, a repeat if `repeat`, the keys held as `input` has them, does to text
    /// field `i`, which has the focus, if it takes the key: the left and right arrows move its
    /// caret a character, past what's selected, Home and End to its start and end, each with
    /// Shift selecting as far, and Ctrl+A selects all of it; Backspace and Delete cut what's
    /// selected, or the character before or after the caret; Ctrl+C copies what's selected, and
    /// Ctrl+X cuts it too, Cmd for Ctrl on macOS, as [`copied`](Self::copied) keeps it, and Ctrl+V
    /// waits for the [`Event::Paste`] that follows; Enter presses it; and Space types, as its text
    /// follows. `None` for a key it leaves to the rest, as Escape, Tab, and the up and down arrows,
    /// which move the focus.
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

    /// Keeps text field `i`'s caret and its selection's other end at its characters' boundaries
    /// within its text, as code or a reload may have cut it since.
    fn fix_caret(&mut self, i: usize) {
        let text = &self.nodes[i].text;
        let valid = |byte: usize| text.is_char_boundary(byte).then_some(byte);
        self.caret.at = valid(self.caret.at).unwrap_or(text.len());
        self.caret.from = valid(self.caret.from).unwrap_or(self.caret.at);
    }

    /// Moves text field `i`'s caret a character `by` either way, past what's selected unless
    /// `select`, which selects as far.
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

    /// Types `typed` into text field `i`, in place of what's selected, as much as it holds, unless
    /// it holds a character no line does.
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

    /// Has text field `i` say `put` in place of `cut`, its caret after it, a change of its text.
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

    /// Where in text field `i`'s text, as a byte at a character's boundary, `x` is, in the canvas's
    /// units from the view's left, as the UI last drew it: the boundary nearest it.
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
        // The first boundary at or past `x`, by halves, as each is past the one before.
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

    /// Has the focused text field's caret show, its text scrolled as little as shows it in the
    /// field's room, and finds where its caret and its selection's other end are along its text,
    /// as `fonts` measures it, for drawing.
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

/// How far a pad's stick goes before it steps the focus, and how near rest it comes back before
/// it steps again.
const STICK_STEPS: f32 = 0.6;
const STICK_RESTS: f32 = 0.3;

/// How far a line of the mouse's wheel scrolls a list, in the canvas's units.
const LINE: f32 = 60.0;

/// How many steps a slider without steps of its own takes from its `min` to its `max` by keys and
/// a pad.
const STEPS: f32 = 20.0;
