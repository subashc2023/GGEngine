//! A game's UI from a `.ui` file under the asset root: a tree of named nodes.
//!
//! # The file
//!
//! RON: `(focus: "play", nodes: [...])`. The optional `focus` names the node focused when the UI
//! is added. Each node is its kind with its fields in the kind's parentheses, as
//! `Button(name: "play", text: "PLAY")`. A left-out field takes its default. An optional field is
//! written as its value: `name: "play"` reads like `Some("play")`.
//!
//! In [`file`](mod@file), `Fields` lists the fields, [`Kind::takes`] which kinds take each, and
//! [`parse`](file::parse) the refusals and their `ui.*` codes; its constants are the limits and
//! defaults.
//!
//! # Nodes
//!
//! - Leaves: panel, text, image.
//! - Focusable: button and the widgets (slider, toggle, drop-down, text field).
//! - Containers: row, column, stack, and list (a column that clips and scrolls).
//!
//! # Use
//!
//! A layer adds a UI ([`Context::add_ui`]), gets a [`GameUi`], and each frame passes it input
//! ([`GameUi::event`] gives a [`UiEvent`]) and draws it ([`GameUi::draw`]) on the view's canvas,
//! clipped where a list or clipping container says. The engine lays it out again when its tree,
//! texts or the view's size change.
//!
//! # State
//!
//! The focus, widgets' values and what code set of nodes are the run's, not the world's: each
//! lockstep peer has its own, and none is hashed, snapshot or recorded. In development builds the
//! file is read again as it changes; each node keeps, by name, what code and input set of it.

mod draw;
mod file;
mod input;
mod layout;
#[cfg(test)]
mod tests;
mod uis;
mod widgets;

use std::fmt::{self, Write as _};
use std::mem;
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

/// A UI [`Context::add_ui`] added: its index among its context's.
///
/// A layer drives it each frame with [`event`](Self::event) and [`draw`](Self::draw), and sets its
/// nodes' text, visibility and widgets' values by name. Its state is the run's (ARCHITECTURE.md's
/// Events and layers).
///
/// Each method panics if the UI is another context's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GameUi(u32);

/// What input did to a UI, from [`GameUi::event`]. A choice that changes the game queues a command,
/// as other input does.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// The button of this name was pressed: a click released over the button it went down on, or
    /// Enter, Space or a pad's South on the focused button. Enter or South on a text field also
    /// presses it, so a game takes what's typed.
    Pressed(Arc<str>),
    /// The focus moved to the button or widget of this name: by the arrows, a pad's D-pad or left
    /// stick, Tab or Shift+Tab, the cursor coming over it, or a click on a text field. For a move
    /// sound.
    Focused(Arc<str>),
    /// The widget of this name took this value from input: a slider stepped or dragged, a toggle
    /// pressed, a drop-down's choice stepped or chosen, or a text field typed into or cut.
    Changed(Arc<str>, UiValue),
    /// Back was asked for: Escape or a pad's East.
    Back,
}

/// What a widget holds: [`UiEvent::Changed`] gives it, [`GameUi::value`] and [`GameUi::set_value`]
/// read and set it.
#[derive(Clone, Debug, PartialEq)]
pub enum UiValue {
    /// A slider's, from its `min` to its `max`.
    Number(f32),
    /// A toggle's: whether it's on.
    On(bool),
    /// A drop-down's choice: its place among its options, from 0.
    Choice(usize),
    /// A text field's text.
    Text(String),
}

/// A UI's file as read: its nodes, each after its parent, and the node focused when added.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Document {
    pub(crate) nodes: Vec<Node>,
    pub(crate) focus: Option<u32>,
}

/// A node of a UI, as its file gives it and code and input change it. [`file`](mod@file) says
/// which kinds take which fields.
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
    /// Where it goes in its parent, instead of its parent's alignment, or in the view.
    pub(crate) anchor: Option<[Align; 2]>,
    /// What a text, button or toggle says, or what's typed in a text field.
    pub(crate) text: String,
    pub(crate) font: Option<Named<Font>>,
    /// Its text's em height, in canvas units.
    pub(crate) text_size: f32,
    /// Its rounded corners' reach, in canvas units, over a color without a texture.
    pub(crate) radius: f32,
    /// Its texture cut into nine, as `(fraction, units)`: corners and edges `fraction` of the
    /// texture in, drawn `units` wide, the rest stretched between.
    pub(crate) slice: Option<(f32, f32)>,
    /// A button's or widget's: normal, focused, pressed, disabled. Other kinds use the first.
    pub(crate) looks: [Look; 4],
    /// An image's texture size in pixels, once known.
    pub(crate) natural: Option<Vec2>,
    /// What a slider, toggle, drop-down or text field holds.
    pub(crate) widget: Widget,
    /// The color of what's on, chosen or typed in a widget, and of a list's scroll bar.
    pub(crate) accent: Vec4,
    /// Whether it clips what's under it to its rectangle less its padding, as a list does.
    pub(crate) clips: bool,
    /// How far a list's children are scrolled up, in canvas units. The run's: kept by name when
    /// the file is read again.
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

/// An asset a UI's file names, by asset ID or path from the asset root, and its handle once
/// loaded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Named<T> {
    pub(crate) name: Arc<str>,
    pub(crate) handle: Option<T>,
}

/// How a panel, image, text or one state of a button or widget draws: its color (tinting its
/// texture) and its text's, in linear RGBA, alpha not multiplied in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub(crate) color: Vec4,
    pub(crate) text: Vec4,
    pub(crate) texture: Option<Named<Texture>>,
}

/// A node's size along an axis, in canvas units: fixed, fit to its content, or its share of its
/// parent's room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Size {
    Fixed(f32),
    Fit,
    Fill(f32),
}

/// What a node is. A panel is a background behind its stacked children. A button is a panel
/// holding its text. Buttons and widgets (slider, toggle, drop-down, text field) take the focus;
/// widgets hold a value. A list is a column that clips and scrolls.
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
    /// Its name with an article, as refusals say it.
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

    /// Whether it has text: a text, button, toggle, or text field (what's typed).
    fn says(self) -> bool {
        matches!(self, Self::Text | Self::Button | Self::Toggle | Self::Field)
    }

    /// Whether it takes the focus: a button or a widget.
    fn focuses(self) -> bool {
        matches!(
            self,
            Self::Button | Self::Slider | Self::Toggle | Self::Dropdown | Self::Field
        )
    }
}

/// What a widget holds, except a text field's text, which is its node's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Widget {
    /// Of a node that isn't a widget.
    None,
    /// Its value, from `min` to `max`, and its step, 0 for none.
    Slider {
        min: f32,
        max: f32,
        step: f32,
        value: f32,
    },
    Toggle {
        on: bool,
    },
    /// Its options, the chosen one, and the color behind the open options.
    Dropdown {
        options: Vec<String>,
        choice: usize,
        backing: Vec4,
    },
    /// What it says while empty, and its most characters.
    Field {
        hint: String,
        most: usize,
    },
}

/// How many texels across [`disc`] is.
const DISC: u32 = 64;

/// The texture each rounded corner shows a quarter of: a white disc [`DISC`] texels across, its
/// edge smoothed over a texel. Returns its size and RGBA8 rows, top first.
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

/// What [`Context::ui_parts`] lends to lay out, draw and take input with.
pub(crate) struct Parts<'a> {
    pub(crate) uis: &'a mut Uis,
    pub(crate) fonts: &'a mut Fonts,
    pub(crate) drawn: &'a mut canvas::Drawn,
    pub(crate) shaders: &'a mut Shaders,
    /// The game's view as the last frame handed it off, while it had an area.
    pub(crate) view: Option<Viewport>,
    pub(crate) input: &'a Input,
}

/// How a node is held pressed. The mouse presses a button on release over it and drags a slider or
/// a text field's selection while down. A key or pad button pressed it as it went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Mouse,
    Key,
    Pad,
}

/// Indices into a node's looks.
const FOCUSED: usize = 1;
const PRESSED: usize = 2;
const DISABLED: usize = 3;

/// What code set of a node, kept when its file is read again; input setting a widget's value too.
#[derive(Clone, Copy, Debug, Default)]
struct Set {
    text: bool,
    visible: bool,
    disabled: bool,
    value: bool,
}

/// The focused text field's caret and the other end of its selection, as byte offsets at
/// character boundaries (equal when nothing is selected); how far its text is scrolled left so the
/// caret shows; and where the two are along its text, in canvas units, as last drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Caret {
    at: usize,
    from: usize,
    scroll: f32,
    xs: [f32; 2],
}

/// The open drop-down: its node, the lit option, and whether the mouse went down on an option,
/// which it chooses on release over one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    node: usize,
    lit: usize,
    held: bool,
}

/// A UI a layer added: its nodes as code and input changed them, their layout, its focus and what
/// input holds.
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
    /// The view's size in canvas units at the last layout. `None` once its tree, texts, a list's
    /// scroll, or a font or texture it names, changed or reloaded since.
    laid_for: Option<Vec2>,
    /// The focused node, and the one its file names.
    focus: Option<usize>,
    file_focus: Option<usize>,
    pressed: Option<(usize, Press)>,
    /// The focusable node under the cursor at its last move, press or release.
    hovered: Option<usize>,
    caret: Caret,
    open: Option<Open>,
    input: bool,
    /// Whether the left stick is pushed far enough to step, until it's back near rest: a push
    /// steps once, however it wavers.
    pushed: bool,
    /// The frame that last drew it, as the number of frames handed off before it.
    drawn: Option<u64>,
    /// What Ctrl+C or Ctrl+X took from a text field, for the event to put on the OS's clipboard
    /// with [`Context::copy`].
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

    /// Puts the caret at the end of the focused text field's text, if one has the focus.
    fn reset_caret(&mut self) {
        self.caret = Caret::default();
        if let Some(focus) = self.focus.filter(|&i| self.nodes[i].kind == Kind::Field) {
            let end = self.nodes[focus].text.len();
            (self.caret.at, self.caret.from) = (end, end);
        }
    }

    /// Takes the nodes of `document`, its file read again. A node of the same name keeps, where it
    /// still takes it: what code set of its text, visibility or disabling; a widget's value set by
    /// code or input; a list's scroll. The focus stays on the node of its name, else goes to the
    /// one the file names. The layout waits for the next frame.
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

    /// The node named `name`, by linear scan. Ceiling: a UI of thousands of nodes with many set per
    /// frame; a name map would lift it.
    fn named(&self, name: &str) -> Option<usize> {
        (self.nodes.iter()).position(|node| node.name.as_deref() == Some(name))
    }

    /// The node named `name`, if `takes` accepts its kind; `what` is the thing set, for messages.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for another kind.
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

    /// Lays it out for a view `view` units across and down ([`layout::lay_out`]), unless it is so
    /// already. Each shown list's scroll is kept within what it can scroll; a hidden list keeps its
    /// scroll for when it shows again.
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

    /// The name of node `i`; every focusable node has one.
    fn name_of(&self, i: usize) -> Arc<str> {
        let name = self.nodes[i].name.clone();
        name.expect("a node that takes the focus has a name, as its file is checked")
    }

    /// Which of node `i`'s looks it shows: disabled, pressed (a drop-down while open), focused or
    /// normal; the first for non-focusable kinds.
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

/// A view's size in canvas units: 1,080 down, across by its aspect; 1,920 by 1,080 before the
/// first frame is handed off.
fn units(view: Option<Viewport>) -> Vec2 {
    view.map_or(Vec2::new(1920.0, CANVAS_HEIGHT), |view| {
        let [width, height] = view.size.map(|side| side as f32);
        Vec2::new(width / height * CANVAS_HEIGHT, CANVAS_HEIGHT)
    })
}

/// The cursor, given in the main window's pixels, on `view` in canvas units from its top left, +Y
/// down. `None` outside the view or without either.
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
    /// What `event` does to the UI while it takes input. Each event does at most one thing, which
    /// it gives back (`None` for nothing), so the layer can play a sound, queue a command or let
    /// the event go on down the stack.
    ///
    /// - Focus: the arrows, a pad's D-pad and left stick go to the nearest button or widget that
    ///   way; the stick once per push (the way it leans most past 0.6, until back under 0.3). Tab
    ///   and Shift+Tab go to the next and previous in file order, coming round, scrolling it into
    ///   view in its lists. The cursor over a button or widget focuses it, except while a text
    ///   field has the focus. With no focus, a move focuses the node the file names, else the
    ///   first.
    /// - A hidden node, one under a hidden node, or a disabled one takes no focus or press; what a
    ///   list or clipping container hides takes no click.
    /// - Enter, Space and a pad's South press the focused button (shown pressed until released),
    ///   or focus one if none is. A click presses a button on release over the one it went down
    ///   on.
    /// - Escape and a pad's East are back.
    /// - Slider: left and right step to the next point of the grid of its steps, else of twentieths
    ///   of its range from `min`, or to `max` past the grid's last. Home and End set `min` and
    ///   `max`. A click or drag sets the point nearest the cursor, or `max`; a drag ends if the
    ///   slider is hidden or disabled.
    /// - Drop-down: left and right step the choice. Enter, Space, South or a click open its
    ///   options over the rest, which take every event until closed. Up and down light one; Enter,
    ///   Space, South or a click released over one chooses it. Escape, East, a click outside or
    ///   Tab close them as they were.
    /// - Toggle: Enter, Space, South or a click flip it.
    /// - Text field: a click focuses it and puts the caret; a drag selects. It takes typing at the
    ///   caret, replacing the selection, up to the characters it holds. The arrows move the
    ///   caret, Shift selects, and Home, End, Ctrl+A, Backspace and Delete work. Enter or South
    ///   presses it.
    /// - The mouse's wheel scrolls the list under the cursor.
    ///
    /// A UI that takes input and drew in the last frame takes from the layers below the layer
    /// that sent the event, as that layer handling it: the mouse's buttons and wheel over a button,
    /// widget or list ([`pointer_over`](Self::pointer_over)) or anywhere while a drop-down is
    /// open, so a click on a button isn't a click on the world behind it; a button's release
    /// after a press it holds; and, while a text field has the focus, the keys and typed text, so
    /// typing a name doesn't walk the player. A layer that handles the event itself too sees it,
    /// and asks `pointer_over` of its own.
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

    /// Whether the cursor is over a shown button, widget or list, within what clips it, or
    /// anywhere while a drop-down's options are open. The layers below don't get the mouse's
    /// buttons and wheel there, so a game keeps its own clicks and aiming off it.
    ///
    /// `false` while the UI takes no input ([`set_input`](Self::set_input)), didn't draw in the
    /// last frame, or the cursor is outside the view. A panel, text or image isn't pointed at, so
    /// a HUD of them doesn't take the game's clicks. The UI is laid out for the view as the last
    /// frame handed it off.
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

    /// Draws the UI on the game's view's canvas this frame, laid out for the view as the last
    /// frame handed it off. It lays out again only when its tree, texts, a list's scroll, the
    /// view's size, or a font or texture it names, changed or reloaded.
    ///
    /// Each top node is placed from the point of the view its anchor names (the middle by
    /// default), so a resize keeps it there in the frame it comes in. Nodes under it draw in order
    /// over what's before, within what clips them, a button or widget in the look its state shows.
    /// An open drop-down's options draw over it all. The UI is of no entity, so a pick where it's
    /// on top finds none.
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

    /// Has the text, button, toggle or text field named `name` say `text` from now on, kept when
    /// the file is read again, and laid out again at the next draw if it changed. A text field
    /// takes the first line, up to the characters it holds, its caret at the end. Setting what the
    /// node says already changes nothing, so a layer may set it each frame.
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

    /// Like [`set_text`](Self::set_text), formatting `text`: `ui.set_text_fmt(cx, "score", score)`.
    ///
    /// It formats into a string the UIs keep, so setting a number each frame allocates nothing.
    ///
    /// Fails as `set_text` does.
    pub fn set_text_fmt(self, cx: &mut Context, name: &str, text: impl fmt::Display) -> Result<()> {
        let mut written = mem::take(&mut cx.ui_parts().uis.written);
        written.clear();
        write!(written, "{text}").expect("a string takes any text");
        let set = self.set_text(cx, name, &written);
        cx.ui_parts().uis.written = written;
        set
    }

    /// Shows or hides the node named `name` and what's under it from now on, kept when the file is
    /// read again. A hidden node takes no room, and a button or widget under it no focus or press.
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

    /// Disables or enables the button or widget named `name` from now on, kept when the file is
    /// read again. A disabled one shows its disabled look and takes no focus or press.
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

    /// Has the UI take input or not ([`event`](Self::event)), as a menu takes none while the game
    /// plays under it. It takes input when added. Turned off, it lets go of what it held pressed,
    /// closes a drop-down's options, and keeps its focus.
    pub fn set_input(self, cx: &mut Context, input: bool) {
        let state = cx.ui_parts().uis.state(self);
        state.input = input;
        if !input {
            (state.pressed, state.hovered, state.pushed) = (None, None, false);
            state.open = None;
        }
    }

    /// The name of the focused button or widget. `None` with no focus, or if it's since hidden or
    /// disabled.
    pub fn focused(self, cx: &mut Context) -> Option<Arc<str>> {
        let state = cx.ui_parts().uis.state(self);
        state.focused().map(|focused| state.name_of(focused))
    }

    /// Moves the focus to the button or widget named `name`, as a game does when it shows a page of
    /// its menu. It scrolls the node into view in its lists, like a move by keys, and puts a text
    /// field's caret at the end. A hidden or disabled node takes the focus once shown and enabled.
    /// It gives no [`UiEvent::Focused`]; only input does.
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

    /// What the widget named `name` holds.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that's no
    /// widget.
    pub fn value(self, cx: &mut Context, name: &str) -> Result<UiValue> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "value", widgets::holds)?;
        Ok(widgets::value_of(&state.nodes[found]).expect("a widget holds a value"))
    }

    /// Has the widget named `name` hold `value` from now on, kept when the file is read again. A
    /// slider's number stays within its bounds; a text field's text is set as
    /// [`set_text`](Self::set_text) does. It gives no [`UiEvent::Changed`]; only input does, so a
    /// layer may set it each frame.
    ///
    /// Fails with `ui.no_node` for a name no node has, `ui.wrong_kind` for a node that's no widget,
    /// and `ui.bad_value` for another widget's kind of value, a number that isn't finite or a
    /// choice past its options.
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
