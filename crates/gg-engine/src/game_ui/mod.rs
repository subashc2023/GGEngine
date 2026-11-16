//! A game's UI from a file: `.ui` files under the asset root, each a tree of named nodes, panels,
//! texts, images, buttons and widgets, sliders, toggles, drop-downs and text fields, in rows,
//! columns, stacks and lists that scroll, which a layer adds, getting a handle, and drives each
//! frame, turning input into what happened, a button by its name pressed, a widget's value changed,
//! the focus moved or back asked for, and drawing it on the view's canvas, clipped where a list or
//! a container that clips says, laid out by the engine as its tree, its texts or the view's size
//! change. Its state, the focus, the widgets' values and what code set of its nodes, is the run's
//! rather than the world's, so that each lockstep peer has its own and none of it is hashed,
//! snapshot or recorded; in development builds it's read again as its file changes, each node
//! keeping what code and input set of it by its name.

mod draw;
mod file;
mod input;
mod layout;
#[cfg(test)]
mod tests;
mod uis;
mod widgets;

use std::sync::Arc;

use gg_core::math::{Vec2, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{Shaders, Texture, Viewport};

use crate::canvas::{self, CANVAS_HEIGHT, Canvas};
use crate::font::Fonts;
use crate::{Align, Context, Event, Font, Input, TextStyle};

use draw::over;
pub(crate) use file::read;
use layout::{Laid, share};
pub(crate) use uis::Uis;

/// A UI that [`Context::add_ui`] added from its file: its index among its context's, through which
/// a layer drives it each frame, with [`event`](Self::event) as input comes and
/// [`draw`](Self::draw) as it updates, and sets what its nodes say, whether they show and what its
/// widgets hold, by their names. What it holds is the run's, as ARCHITECTURE.md's Events and
/// layers says.
///
/// Each method panics if the UI is another context's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GameUi(u32);

/// What input did to a UI, as [`GameUi::event`] gives it: a choice that changes the game queues a
/// command, as other input does.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// The button of this name was pressed: by a click, released over the button it was pressed
    /// on; by Enter or Space, or a pad's South, on the button with the focus. A text field is
    /// pressed by Enter, or a pad's South, as a game takes what's typed in it.
    Pressed(Arc<str>),
    /// The focus moved to the button or the widget of this name: by the arrows, a pad's D-pad or
    /// left stick, Tab or Shift+Tab, the cursor coming over it, or a click on a text field; for the
    /// sound a game plays as it moves.
    Focused(Arc<str>),
    /// The widget of this name took this value, by input: a slider stepped or dragged, a toggle
    /// pressed, a drop-down's choice stepped or chosen from its options, or a text field typed
    /// into or cut.
    Changed(Arc<str>, UiValue),
    /// Back was asked for, by Escape or a pad's East.
    Back,
}

/// What a widget holds, as [`UiEvent::Changed`] gives it, and [`GameUi::value`] and
/// [`GameUi::set_value`] read and set it.
#[derive(Clone, Debug, PartialEq)]
pub enum UiValue {
    /// A slider's, from its `min` to its `max`.
    Number(f32),
    /// A toggle's, whether it's on.
    On(bool),
    /// A drop-down's choice, by its place among its options, from 0.
    Choice(usize),
    /// A text field's text.
    Text(String),
}

/// A UI's file as it read: its nodes, each after its parent, and the node focused as it's added.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Document {
    pub(crate) nodes: Vec<Node>,
    pub(crate) focus: Option<u32>,
}

/// A node of a UI, as its file gives it and code and input change it: what [`Kind`] says it is,
/// and the fields its file gives it, which [`file`](mod@file) says which kinds take.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) name: Option<Arc<str>>,
    pub(crate) kind: Kind,
    /// Past its subtree, among its UI's nodes, which are in pre-order.
    pub(crate) end: u32,
    pub(crate) parent: Option<u32>,
    pub(crate) visible: bool,
    pub(crate) disabled: bool,
    /// Across, then down.
    pub(crate) size: [Size; 2],
    pub(crate) margin: Vec2,
    pub(crate) padding: Vec2,
    pub(crate) gap: f32,
    /// Where a container's children go, and a text's lines, across then down.
    pub(crate) align: [Align; 2],
    /// Where it goes in its parent, in place of its parent's alignment, or in the view.
    pub(crate) anchor: Option<[Align; 2]>,
    /// What a text, a button or a toggle says, or what's typed in a text field.
    pub(crate) text: String,
    pub(crate) font: Option<Named<Font>>,
    /// How tall its text's em is, in the canvas's units.
    pub(crate) text_size: f32,
    /// How far its rounded corners reach, in the canvas's units, over its color without a texture.
    pub(crate) radius: f32,
    /// Its texture cut into nine, as `(fraction, units)`: its corners and edges `fraction` of the
    /// texture in, drawn `units` wide, and the rest stretched between them.
    pub(crate) slice: Option<(f32, f32)>,
    /// A button's or a widget's, normal, focused, pressed and disabled; the first alone for other
    /// kinds.
    pub(crate) looks: [Look; 4],
    /// An image's texture's size, in its pixels, once known.
    pub(crate) natural: Option<Vec2>,
    /// What a slider, a toggle, a drop-down or a text field holds.
    pub(crate) widget: Widget,
    /// The color of what's on, chosen or typed in a widget, and of a list's scroll bar.
    pub(crate) accent: Vec4,
    /// Whether it clips what's under it to its rectangle less its padding, as a list does.
    pub(crate) clips: bool,
    /// How far a list's children are scrolled up, in the canvas's units: the run's, kept by its
    /// name as its file is read again.
    pub(crate) scroll: f32,
}

impl Node {
    /// How its text is drawn, in `color`.
    fn style(&self, color: Vec4) -> TextStyle {
        TextStyle {
            font: self.font.as_ref().and_then(|font| font.handle),
            size: self.text_size,
            color,
            align: self.align,
        }
    }
}

/// An asset a UI's file names, by its asset ID or its path from the asset root, and what loaded
/// of it, once it has.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Named<T> {
    pub(crate) name: Arc<str>,
    pub(crate) handle: Option<T>,
}

/// How a panel, an image, a text or one state of a button or a widget draws: its color, which
/// tints its texture, and the color of its text, in linear RGBA, alpha not multiplied in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub(crate) color: Vec4,
    pub(crate) text: Vec4,
    pub(crate) texture: Option<Named<Texture>>,
}

/// How large a node is along an axis, in the canvas's units: fixed, fit to what it holds, or
/// filling its share of its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Size {
    Fixed(f32),
    Fit,
    Fill(f32),
}

/// What a node is: a panel, a background behind the children it stacks; a text; an image; a
/// button, a panel holding its text, which takes the focus; a widget, which takes the focus too
/// and holds a value, a slider, a toggle, a drop-down or a text field; or a container laying its
/// children out in a row, a column, stacked over each other, or in a list, a column that clips
/// them and scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Panel,
    Text,
    Image,
    Button,
    Slider,
    Toggle,
    Dropdown,
    Field,
    Row,
    Column,
    Stack,
    List,
}

impl Kind {
    /// What it's called, with its article, as refusals name it.
    fn called(self) -> &'static str {
        match self {
            Self::Panel => "a panel",
            Self::Text => "a text",
            Self::Image => "an image",
            Self::Button => "a button",
            Self::Slider => "a slider",
            Self::Toggle => "a toggle",
            Self::Dropdown => "a drop-down",
            Self::Field => "a text field",
            Self::Row => "a row",
            Self::Column => "a column",
            Self::Stack => "a stack",
            Self::List => "a list",
        }
    }

    /// Whether it says something, as a text, a button and a toggle do, and a text field, what's
    /// typed in it.
    fn says(self) -> bool {
        matches!(self, Self::Text | Self::Button | Self::Toggle | Self::Field)
    }

    /// Whether it takes the focus, as a button and each widget do.
    fn focuses(self) -> bool {
        matches!(
            self,
            Self::Button | Self::Slider | Self::Toggle | Self::Dropdown | Self::Field
        )
    }
}

/// What a widget holds, as its file gives it and code and input change it, but a text field's
/// text, which is its node's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Widget {
    /// Of a node that isn't a widget.
    None,
    /// Its value, from `min` to `max`, and the steps it's set to, or 0 for none.
    Slider {
        min: f32,
        max: f32,
        step: f32,
        value: f32,
    },
    Toggle {
        on: bool,
    },
    /// Its options, the one chosen, and the color behind its options as they're open.
    Dropdown {
        options: Vec<String>,
        choice: usize,
        backing: Vec4,
    },
    /// What it says while it's empty, and the most characters it holds.
    Field {
        hint: String,
        most: usize,
    },
}

/// How many texels across [`disc`] is.
const DISC: u32 = 64;

/// The texture a rounded panel's or button's corners each show a quarter of: a white disc, [`DISC`]
/// texels across, its edge smoothed over a texel; its size and its rows of RGBA8, top row first.
pub(crate) fn disc() -> ([u32; 2], Vec<u8>) {
    let radius = DISC as f32 / 2.0;
    let mut pixels = Vec::with_capacity((DISC * DISC * 4) as usize);
    for y in 0..DISC {
        for x in 0..DISC {
            let at = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let inside = (radius - at.distance(Vec2::splat(radius)) + 0.5).clamp(0.0, 1.0);
            pixels.extend([255, 255, 255, (inside * 255.0).round() as u8]);
        }
    }
    ([DISC, DISC], pixels)
}

/// What a context lends its UIs to lay out, draw and take input with, as
/// [`Context::ui_parts`] lends it.
pub(crate) struct Parts<'a> {
    pub(crate) uis: &'a mut Uis,
    pub(crate) fonts: &'a mut Fonts,
    pub(crate) drawn: &'a mut canvas::Drawn,
    pub(crate) shaders: &'a mut Shaders,
    /// Where the game's view was as the last frame handed it off, while it had an area.
    pub(crate) view: Option<Viewport>,
    pub(crate) input: &'a Input,
}

/// How a node is held pressed: by the mouse, which presses a button as it's released over it, and
/// drags a slider or a text field's selection while it's down; or by a key or a pad's button, which
/// pressed it as it went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Mouse,
    Key,
    Pad,
}

/// The looks of a button or a widget, by their places among its looks.
const FOCUSED: usize = 1;
const PRESSED: usize = 2;
const DISABLED: usize = 3;

/// What of a node code set, which it keeps as its file is read again: a widget's value set by
/// input too.
#[derive(Clone, Copy, Debug, Default)]
struct Set {
    text: bool,
    visible: bool,
    disabled: bool,
    value: bool,
}

/// The focused text field's caret and the end its selection reaches from, as bytes of its text at
/// its characters' boundaries, the same where nothing's selected; how far its text is scrolled left
/// so that the caret shows, and where the two are along its text, in the canvas's units, as the UI
/// last drew.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Caret {
    at: usize,
    from: usize,
    scroll: f32,
    xs: [f32; 2],
}

/// The drop-down open over the rest: its node, the option lit, and whether the mouse went down on
/// an option, which it chooses as it's let go over one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    node: usize,
    lit: usize,
    held: bool,
}

/// A UI a layer added: its nodes, as its file gave them and code and input changed them, how they
/// lie, and its focus and what input holds of it.
#[cfg_attr(
    not(feature = "dev"),
    allow(dead_code, reason = "only development builds reload or list a UI")
)]
struct State {
    file: usize,
    /// What its layer named it.
    name: String,
    nodes: Vec<Node>,
    /// What code set of each node.
    set: Vec<Set>,
    laid: Laid,
    /// How many of the canvas's units across and down the view was as it was laid out, or `None`
    /// once its tree, its texts, a list's scroll, or a font or a texture it names, reloaded,
    /// changed since.
    laid_for: Option<Vec2>,
    /// The node with the focus, and the one its file names.
    focus: Option<usize>,
    file_focus: Option<usize>,
    pressed: Option<(usize, Press)>,
    /// The node that takes the focus the cursor was over as it last moved, pressed or let go.
    hovered: Option<usize>,
    caret: Caret,
    open: Option<Open>,
    input: bool,
    /// Whether the left stick was pushed far enough to step, until it comes back near rest, so that
    /// a push steps once, whichever way it wavers.
    pushed: bool,
    /// The frame that last drew it, by the number of frames handed off before it.
    drawn: Option<u64>,
    /// What a text field's selection copied or cut, as Ctrl+C and Ctrl+X do, for the event that did
    /// to put on the OS's clipboard, as [`Context::copy`] does.
    copied: Option<String>,
}

impl State {
    fn new(file: usize, name: &str, document: &Document) -> Self {
        let mut state = Self {
            file,
            name: name.to_owned(),
            nodes: document.nodes.clone(),
            set: vec![Set::default(); document.nodes.len()],
            laid: Laid::default(),
            laid_for: None,
            focus: document.focus.map(|focus| focus as usize),
            file_focus: document.focus.map(|focus| focus as usize),
            pressed: None,
            hovered: None,
            caret: Caret::default(),
            open: None,
            input: true,
            pushed: false,
            drawn: None,
            copied: None,
        };
        state.reset_caret();
        state
    }

    /// Puts the caret at the end of the focused text field's text, if a text field has the focus.
    fn reset_caret(&mut self) {
        self.caret = Caret::default();
        if let Some(focus) = self.focus.filter(|&i| self.nodes[i].kind == Kind::Field) {
            let end = self.nodes[focus].text.len();
            (self.caret.at, self.caret.from) = (end, end);
        }
    }

    /// Takes the nodes of `document`, its file read again: each node keeps what code set of it, its
    /// text, its visibility or whether it's disabled, and what code or input set a widget's value
    /// to, where the node of its name had it and it still takes it, and a list its scroll; the
    /// focus stays on the node of its name, or else goes to the one the file names; and the layout
    /// waits for the next frame.
    #[cfg(any(feature = "dev", test))]
    fn refit(&mut self, document: &Document) {
        let mut nodes = document.nodes.clone();
        let mut set = vec![Set::default(); nodes.len()];
        for (node, kept) in nodes.iter_mut().zip(&mut set) {
            let Some(was) = node.name.as_deref().and_then(|name| self.named(name)) else {
                continue;
            };
            let (old, had) = (&self.nodes[was], self.set[was]);
            if had.text && node.kind.says() && old.kind.says() {
                widgets::put_text(node, &old.text);
                kept.text = true;
            }
            if had.visible {
                node.visible = old.visible;
                kept.visible = true;
            }
            if had.disabled && node.kind.focuses() && old.kind.focuses() {
                node.disabled = old.disabled;
                kept.disabled = true;
            }
            if had.value {
                kept.value = widgets::keep_value(&mut node.widget, &old.widget);
            }
            if node.kind == Kind::List && old.kind == Kind::List {
                node.scroll = old.scroll;
            }
        }
        let focused = self.focus.and_then(|focus| self.nodes[focus].name.clone());
        let focus = focused.and_then(|name| {
            (nodes.iter())
                .position(|node| node.kind.focuses() && node.name.as_deref() == Some(&*name))
        });
        self.file_focus = document.focus.map(|focus| focus as usize);
        self.focus = focus.or(self.file_focus);
        self.nodes = nodes;
        self.set = set;
        self.pressed = None;
        self.hovered = None;
        self.open = None;
        self.laid_for = None;
        self.reset_caret();
    }

    /// The node named `name`: found by looking at each, a ceiling for a UI of thousands of nodes
    /// whose code sets many each frame, which a map of the names would lift.
    fn named(&self, name: &str) -> Option<usize> {
        (self.nodes.iter()).position(|node| node.name.as_deref() == Some(name))
    }

    /// The node named `name`, which `takes` says is of a kind that takes what's set, as `what`
    /// names it.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for one of another kind.
    fn settable(&self, name: &str, what: &str, takes: impl Fn(Kind) -> bool) -> Result<usize> {
        let ui = &self.name;
        let Some(found) = self.named(name) else {
            let message = format!("UI `{ui}` has no node named `{name}`");
            return Err(Error::new("ui.no_node", message));
        };
        let kind = self.nodes[found].kind;
        if !takes(kind) {
            let message = format!(
                "node `{name}` of UI `{ui}` is {}, which takes no {what}",
                kind.called()
            );
            return Err(Error::new("ui.wrong_kind", message));
        }
        Ok(found)
    }

    /// Lays it out for a view `view` units across and down, as [`layout::lay_out`] says, unless it
    /// lies so already, each list that shows kept within what it has to scroll; one that's hidden,
    /// laid out as nothing, keeps its scroll for as it shows again.
    fn lay_out(&mut self, fonts: &mut Fonts, view: Vec2) {
        if self.laid_for == Some(view) {
            return;
        }
        let _zone = gg_core::zone!("lay out a UI");
        layout::lay_out(&self.nodes, fonts, view, &mut self.laid);
        for i in 0..self.nodes.len() {
            if self.nodes[i].kind == Kind::List && self.laid.shown[i] {
                let most = self.laid.most_scroll(&self.nodes, i);
                self.nodes[i].scroll = self.nodes[i].scroll.min(most);
            }
        }
        self.laid_for = Some(view);
    }

    /// Whether node `i` shows: it and each node above it visible.
    fn shows(&self, i: usize) -> bool {
        let mut at = Some(i);
        while let Some(node) = at {
            if !self.nodes[node].visible {
                return false;
            }
            at = self.nodes[node].parent.map(|parent| parent as usize);
        }
        true
    }

    /// Whether node `i` takes the focus: a button or a widget that shows and isn't disabled.
    fn focusable(&self, i: usize) -> bool {
        let node = &self.nodes[i];
        node.kind.focuses() && !node.disabled && self.shows(i)
    }

    /// The node with the focus, while it takes it.
    fn focused(&self) -> Option<usize> {
        self.focus.filter(|&focus| self.focusable(focus))
    }

    /// The text field with the focus, if one has it.
    fn typing(&self) -> Option<usize> {
        self.focused()
            .filter(|&focus| self.nodes[focus].kind == Kind::Field)
    }

    /// The name of node `i`, which every node that takes the focus has.
    fn name_of(&self, i: usize) -> Arc<str> {
        let name = self.nodes[i].name.clone();
        name.expect("a node that takes the focus has a name, as its file is checked")
    }

    /// Which of node `i`'s looks it shows: a button's or a widget's disabled, pressed, focused or
    /// normal, a drop-down's pressed while it's open; the first for other kinds.
    fn look(&self, i: usize) -> usize {
        let node = &self.nodes[i];
        if !node.kind.focuses() {
            return 0;
        }
        let held = match self.pressed {
            Some((at, Press::Mouse)) => {
                at == i && (self.hovered == Some(i) || node.kind == Kind::Slider)
            }
            Some((at, Press::Key | Press::Pad)) => at == i,
            None => false,
        };
        if node.disabled {
            DISABLED
        } else if held || self.open.is_some_and(|open| open.node == i) {
            PRESSED
        } else if self.focus == Some(i) {
            FOCUSED
        } else {
            0
        }
    }
}

/// How many of the canvas's units across and down a view at `view` is: 1,080 down, and across as
/// its aspect has it; a view of 1,920 by 1,080 without one, as before the first frame's handed off.
fn units(view: Option<Viewport>) -> Vec2 {
    view.map_or(Vec2::new(1920.0, CANVAS_HEIGHT), |view| {
        let [width, height] = view.size.map(|side| side as f32);
        Vec2::new(width / height * CANVAS_HEIGHT, CANVAS_HEIGHT)
    })
}

/// Where the cursor at `cursor`, in the main window's pixels, is on a view at `view`: in the
/// canvas's units from the view's top left, +Y down; `None` outside the view, or without either.
fn cursor_on(view: Option<Viewport>, cursor: Option<[f64; 2]>) -> Option<Vec2> {
    let view = view?;
    let [x, y] = cursor?;
    let [left, top] = view.origin.map(f64::from);
    let [width, height] = view.size.map(f64::from);
    let [x, y] = [x - left, y - top];
    if !((0.0..width).contains(&x) && (0.0..height).contains(&y)) {
        return None;
    }
    let scale = f64::from(CANVAS_HEIGHT) / height;
    Some(Vec2::new((x * scale) as f32, (y * scale) as f32))
}

impl GameUi {
    /// What `event`, which the layer was sent, does to the UI, while it takes input: the arrows, a
    /// pad's D-pad and its left stick move the focus to the nearest button or widget that takes it
    /// that way, the stick once a push, the way it leans most as it goes past 0.6, until it comes
    /// back under 0.3; Tab to the next in the file's order and Shift+Tab to the one before, coming
    /// round, each scrolling what it focuses into view in the lists it's in; Enter, Space and a
    /// pad's South press the button with the focus, which shows pressed until they're let go, or
    /// focus one if none has it; Escape and a pad's East ask for back; the cursor coming over a
    /// button or a widget focuses it, but while a text field has the focus, and a click presses a
    /// button, as it's let go over the button it went down on. Left and right, by the arrows, the
    /// D-pad or the stick, step the focused slider's value to the next point of the grid of its
    /// steps, or else twentieths of its range, from its `min`, or to its `max` past the grid's
    /// last, Home and End set it to its ends, and a click or a drag along it sets it at the point
    /// nearest the cursor, or its `max`, a drag ending as it's hidden or disabled; they step the
    /// focused drop-down's choice too, and Enter, Space, a South or a click open its options over
    /// the rest, which take every event until it's closed: the up and down arrows, the D-pad and
    /// the stick light them, Enter, Space, a South or a click let go over one choose, and Escape,
    /// an East, a click outside them or Tab close them as they were. Enter, Space, a South or a
    /// click flip the focused toggle. A text field takes the focus by a click, which puts its caret, and a drag
    /// selects; it takes what's typed, at its caret, in place of what's selected, as many
    /// characters as it holds, the arrows moving its caret and Shift selecting, Home, End, Ctrl+A,
    /// Backspace and Delete, and Enter presses it. The mouse's wheel scrolls the list under the
    /// cursor. A node that's hidden, under a node that is, or disabled takes neither the focus nor
    /// a press, and what a list or a container that clips hides takes no click. With the focus on
    /// none, a move focuses the node the file names, or else the first. Each event does one thing
    /// at most, which it gives back, `None` for nothing, so that the layer plays a sound, queues a
    /// command or lets the event go on down the stack.
    ///
    /// A UI that takes input and drew in the last frame takes the mouse's buttons, pressed or let
    /// go, and its wheel, away from the layers below the layer that sent it the event, as that
    /// layer handling it: those over a button, a widget or a list, as
    /// [`pointer_over`](Self::pointer_over) says, those anywhere while a drop-down's options are
    /// open, and a button let go after a press it holds, so that a click on a button isn't a click
    /// on the world behind it; and, while a text field has the focus, the keys and the text typed,
    /// so that typing a name doesn't walk the player. A layer that handles the event itself too
    /// sees it as well, and asks `pointer_over` of its own.
    pub fn event(self, cx: &mut Context, event: &Event<'_>) -> Option<UiEvent> {
        let Parts {
            uis,
            fonts,
            view,
            input,
            ..
        } = cx.ui_parts();
        let frames = uis.frames;
        let state = uis.state(self);
        if !state.input {
            return None;
        }
        state.lay_out(fonts, units(view));
        let cursor = cursor_on(view, input.cursor());
        let takes = state.takes(event, cursor, frames);
        let done = state.event(event, cursor, input, fonts);
        let copied = state.copied.take();
        uis.consumed |= takes;
        if let Some(copied) = copied {
            cx.copy(&copied);
        }
        done
    }

    /// Whether the cursor is over what the UI draws that a pointer works on, a button, a widget or
    /// a list that shows, within what clips it, or anywhere while a drop-down's options are open:
    /// as the mouse's buttons and wheel over it don't reach the layers below, a game keeps its own
    /// clicks and aiming off it. `false` while the UI takes no input, as
    /// [`set_input`](Self::set_input) says, or didn't draw in the last frame, or the cursor is
    /// outside the view; a panel, a text or
    /// an image isn't, so that a HUD of them doesn't take the game's clicks. As the UI is laid out
    /// for the view as the last frame handed it off.
    pub fn pointer_over(self, cx: &mut Context) -> bool {
        let Parts {
            uis,
            fonts,
            view,
            input,
            ..
        } = cx.ui_parts();
        let frames = uis.frames;
        let state = uis.state(self);
        if !state.takes_input(frames) {
            return false;
        }
        let Some(at) = cursor_on(view, input.cursor()) else {
            return false;
        };
        state.lay_out(fonts, units(view));
        state.pointer_over(at)
    }

    /// Draws the UI on the game's view's canvas this frame, as it updates, laid out for the view as
    /// the last frame handed it off, again only once its tree, its texts, a list's scroll, the
    /// view's size, or a font or a texture it names, reloaded, changed: each node at the top from
    /// the point of the view its anchor names, the view's middle by default, so that a resize keeps
    /// it there in the frame it comes in, and those under it in their order, over what's before
    /// them, within what clips them, a button or a widget in the look its state shows; and an open
    /// drop-down's options over it all. It's of no entity, so a pick where it's on top finds none.
    pub fn draw(self, cx: &mut Context) {
        let _zone = gg_core::zone!("draw a UI");
        let Parts {
            uis,
            fonts,
            drawn,
            shaders,
            view,
            input,
        } = cx.ui_parts();
        let (disc, frames) = (uis.disc, uis.frames);
        let state = uis.state(self);
        let units = units(view);
        state.lay_out(fonts, units);
        state.fit_caret(fonts);
        state.drawn = Some(frames);
        let cursor = input.cursor();
        let origin_of = |state: &State, top: usize| {
            let anchor = state.nodes[top].anchor.unwrap_or([Align::Center; 2]);
            (
                anchor,
                units * Vec2::new(share(anchor[0]), share(anchor[1])),
            )
        };
        let mut top = 0;
        while top < state.nodes.len() {
            let end = state.nodes[top].end as usize;
            if state.laid.shown[top] {
                let (anchor, origin) = origin_of(state, top);
                let mut canvas = Canvas::new(drawn, fonts, shaders, anchor, view, cursor);
                for i in top..end {
                    if state.laid.shown[i] {
                        state.draw_node(i, &mut canvas, origin, disc);
                    }
                    state.draw_bars(i, &mut canvas, origin, disc);
                }
            }
            top = end;
        }
        if let Some(open) = state.open.filter(|open| state.focusable(open.node)) {
            let mut top = open.node;
            while let Some(parent) = state.nodes[top].parent {
                top = parent as usize;
            }
            let (anchor, origin) = origin_of(state, top);
            let mut canvas = Canvas::new(drawn, fonts, shaders, anchor, view, cursor);
            state.draw_options(open, &mut canvas, origin, disc);
        }
    }

    /// Has the text, the button, the toggle or the text field named `name` say `text` from now on,
    /// laid out again as the UI's next drawn, if it said something else; kept as the UI's file is
    /// read again. A text field takes its first line, as many characters as it holds, its caret at
    /// the end. A string the node says already changes nothing, so that a layer may set it each
    /// frame.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that says
    /// nothing.
    pub fn set_text(self, cx: &mut Context, name: &str, text: &str) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "text", Kind::says)?;
        if widgets::put_text(&mut state.nodes[found], text) {
            state.laid_for = None;
            if state.focus == Some(found) {
                let end = state.nodes[found].text.len();
                (state.caret.at, state.caret.from) = (end, end);
            }
        }
        state.set[found].text = true;
        Ok(())
    }

    /// Shows or hides the node named `name`, and what's under it, from now on, kept as the UI's
    /// file is read again: a hidden node takes no room, as if it weren't there, and a button or a
    /// widget under it neither the focus nor a press.
    ///
    /// Fails with `ui.no_node` for a name no node has.
    pub fn set_visible(self, cx: &mut Context, name: &str, visible: bool) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "visibility", |_| true)?;
        if state.nodes[found].visible != visible {
            state.nodes[found].visible = visible;
            state.laid_for = None;
        }
        state.set[found].visible = true;
        Ok(())
    }

    /// Disables or enables the button or the widget named `name` from now on, kept as the UI's file
    /// is read again: a disabled one shows its disabled look, and takes neither the focus nor a
    /// press.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that takes no
    /// focus.
    pub fn set_disabled(self, cx: &mut Context, name: &str, disabled: bool) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "disabling", Kind::focuses)?;
        state.nodes[found].disabled = disabled;
        state.set[found].disabled = true;
        Ok(())
    }

    /// Has the UI take input or not, as [`event`](Self::event) says: as a game's menu takes none
    /// while the game plays under it. It takes input as it's added; turned off, it lets go of what
    /// it held pressed and closes a drop-down's options, and keeps its focus.
    pub fn set_input(self, cx: &mut Context, input: bool) {
        let state = cx.ui_parts().uis.state(self);
        state.input = input;
        if !input {
            (state.pressed, state.hovered, state.pushed) = (None, None, false);
            state.open = None;
        }
    }

    /// The name of the button or the widget with the focus, while it takes it: `None` while the
    /// focus is on none, or on one hidden or disabled since.
    pub fn focused(self, cx: &mut Context) -> Option<Arc<str>> {
        let state = cx.ui_parts().uis.state(self);
        state.focused().map(|focused| state.name_of(focused))
    }

    /// Moves the focus to the button or the widget named `name`, as a game does as it shows a page
    /// of its menu, scrolling it into view in the lists it's in, as a move by keys does; a text
    /// field's caret to the end of its text. One hidden or disabled takes it as it shows and is
    /// enabled again. It gives no [`UiEvent::Focused`], which input alone gives.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that takes no
    /// focus.
    pub fn set_focus(self, cx: &mut Context, name: &str) -> Result<()> {
        let Parts {
            uis, fonts, view, ..
        } = cx.ui_parts();
        let state = uis.state(self);
        let found = state.settable(name, "focus", Kind::focuses)?;
        state.lay_out(fonts, units(view));
        state.open = None;
        state.focus_to(found);
        Ok(())
    }

    /// What the widget named `name` holds: a slider's number, a toggle's whether it's on, a
    /// drop-down's choice or a text field's text.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that's no
    /// widget.
    pub fn value(self, cx: &mut Context, name: &str) -> Result<UiValue> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "value", widgets::holds)?;
        Ok(widgets::value_of(&state.nodes[found]).expect("a widget holds a value"))
    }

    /// Has the widget named `name` hold `value` from now on, kept as the UI's file is read again: a
    /// slider's number within its bounds, a toggle's whether it's on, a drop-down's choice, or a
    /// text field's text, as [`set_text`](Self::set_text) sets it. It gives no
    /// [`UiEvent::Changed`], which input alone gives, so that a layer may set it each frame.
    ///
    /// Fails with `ui.no_node` for a name no node has, `ui.wrong_kind` for a node that's no widget,
    /// and `ui.bad_value` for a value of another widget's, a number that isn't finite or a choice
    /// past its options.
    pub fn set_value(self, cx: &mut Context, name: &str, value: UiValue) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "value", widgets::holds)?;
        let text = match value {
            UiValue::Text(text) if matches!(state.nodes[found].widget, Widget::Field { .. }) => {
                text
            }
            value => {
                widgets::put_value(&mut state.nodes[found], &state.name, value)?;
                state.set[found].value = true;
                return Ok(());
            }
        };
        self.set_text(cx, name, &text)
    }
}
