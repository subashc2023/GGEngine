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

mod file;
mod layout;
#[cfg(test)]
mod tests;
mod widgets;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{Shaders, Sprite, Texture, Viewport};

pub(crate) use file::read;
use layout::{Laid, Rect, share};

use crate::canvas::{self, CANVAS_HEIGHT, Canvas};
use crate::font::Fonts;
use crate::{
    Align, Context, Event, Font, Input, Key, MouseButton, PadButton, Stick, TextStyle, Toward,
};

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

/// The UIs a context's layers added: each file read once, and each UI, of a file, with what it
/// holds.
#[derive(Default)]
pub(crate) struct Uis {
    files: Vec<UiFile>,
    /// Each file's index, by the file its path resolves to.
    by_file: HashMap<PathBuf, usize>,
    states: Vec<State>,
    /// The texture of [`disc`], which rounded corners are cut from, from the first UI added on.
    pub(crate) disc: Option<Texture>,
    /// How many frames have been handed off, which says whether a UI drew in the last.
    frames: u64,
    /// Whether a UI took the event the layer being sent it is handling, as [`GameUi::event`] says,
    /// which the layer stack reads as the layer handling it.
    consumed: bool,
}

/// A UI's file, as the UIs added from it share it: its name, its asset ID or else its path, as
/// values name a texture; its path from the asset root, with `/` between its parts; and what it
/// holds, as last read.
#[cfg_attr(
    not(feature = "dev"),
    allow(dead_code, reason = "only development builds reload or list a UI")
)]
struct UiFile {
    asset: Arc<str>,
    path: Arc<str>,
    document: Arc<Document>,
}

impl Uis {
    /// The file a UI was added from that `asset` names, as a texture is named, if one was: by its
    /// asset ID, whatever its file has been called since, as by a rename; or else, for one named by
    /// its path, as one without an ID file is, by `file`, the file its path resolves to, which a
    /// file named by its ID left behind as it was renamed doesn't name.
    pub(crate) fn file_of(&self, asset: &str, file: Option<&Path>) -> Option<usize> {
        let by_id = crate::assets::is_id(asset).then(|| self.file_named(asset));
        if let Some(found) = by_id.flatten() {
            return Some(found);
        }
        let found = *self.by_file.get(file?)?;
        (!crate::assets::is_id(&self.files[found].asset)).then_some(found)
    }

    /// Takes the file at `path`, named `asset`, which reads as `document`, and resolves to `file`,
    /// if it does; and returns its index.
    pub(crate) fn add_file(
        &mut self,
        asset: Arc<str>,
        path: Arc<str>,
        document: Document,
        file: Option<PathBuf>,
    ) -> usize {
        let index = self.files.len();
        self.files.push(UiFile {
            asset,
            path,
            document: Arc::new(document),
        });
        if let Some(file) = file {
            self.by_file.insert(file, index);
        }
        index
    }

    /// Adds a UI of file `file`, which its layer named `name`, and hands out its handle: its nodes
    /// as the file has them, the focus on the node its file names, and taking input.
    pub(crate) fn add(&mut self, file: usize, name: &str) -> GameUi {
        let ui = GameUi(u32::try_from(self.states.len()).expect("fewer than 2^32 UIs"));
        self.states
            .push(State::new(file, name, &self.files[file].document));
        ui
    }

    /// The path of file `file`, from the asset root.
    #[cfg(feature = "dev")]
    pub(crate) fn path(&self, file: usize) -> &str {
        &self.files[file].path
    }

    /// Whether file `file` holds `document` already.
    #[cfg(feature = "dev")]
    pub(crate) fn holds(&self, file: usize, document: &Document) -> bool {
        *self.files[file].document == *document
    }

    /// Has file `file` hold `document`, as it read again, and each UI of it take its nodes, as
    /// [`State::refit`] says.
    #[cfg(any(feature = "dev", test))]
    pub(crate) fn reload(&mut self, file: usize, document: Document) {
        let document = Arc::new(document);
        self.files[file].document = Arc::clone(&document);
        for state in self.states.iter_mut().filter(|state| state.file == file) {
            state.refit(&document);
        }
    }

    /// Counts a frame handed off, after its layers drew.
    pub(crate) fn handed_off(&mut self) {
        self.frames += 1;
    }

    /// Forgets that a UI took the event, as a layer is about to be sent one.
    pub(crate) fn forget_consumed(&mut self) {
        self.consumed = false;
    }

    /// Whether a UI took the event since [`forget_consumed`](Self::forget_consumed).
    pub(crate) fn consumed(&self) -> bool {
        self.consumed
    }

    /// Whether the cursor at `cursor`, on a view at `view`, is over what a UI that takes input and
    /// drew in the last frame draws, as [`GameUi::pointer_over`] says.
    pub(crate) fn pointer_over(
        &mut self,
        fonts: &mut Fonts,
        view: Option<Viewport>,
        cursor: Option<[f64; 2]>,
    ) -> bool {
        let Some(at) = cursor_on(view, cursor) else {
            return false;
        };
        let (units, frames) = (units(view), self.frames);
        self.states.iter_mut().any(|state| {
            state.takes_input(frames) && {
                state.lay_out(fonts, units);
                state.pointer_over(at)
            }
        })
    }

    /// The file named `asset` that a UI was added from, if one was.
    fn file_named(&self, asset: &str) -> Option<usize> {
        self.files.iter().position(|file| *file.asset == *asset)
    }

    /// Has each file named by its asset ID take the path from the asset root that `path_of` gives
    /// that ID now, as its file renamed or moved has it.
    #[cfg(feature = "dev")]
    pub(crate) fn follow(&mut self, path_of: impl Fn(&str) -> Option<Arc<str>>) {
        for file in &mut self.files {
            if let Some(path) = path_of(&file.asset) {
                file.path = path;
            }
        }
    }

    /// Has each UI that writes in `font`, which baked again, laid out again as it next draws.
    #[cfg(feature = "dev")]
    pub(crate) fn font_changed(&mut self, font: Font) {
        for state in &mut self.states {
            let writes = |node: &Node| node.font.as_ref().and_then(|font| font.handle);
            if state.nodes.iter().any(|node| writes(node) == Some(font)) {
                state.laid_for = None;
            }
        }
    }

    /// Has each image of `texture` fit `size`, its pixels as it read again, and each UI with one
    /// laid out again as it next draws.
    #[cfg(feature = "dev")]
    pub(crate) fn texture_resized(&mut self, texture: Texture, size: Vec2) {
        let shows = |node: &Node| {
            let shown = node.looks[0]
                .texture
                .as_ref()
                .and_then(|shown| shown.handle);
            node.kind == Kind::Image && shown == Some(texture)
        };
        for file in &mut self.files {
            if file.document.nodes.iter().any(shows) {
                let document = Arc::make_mut(&mut file.document);
                for node in document.nodes.iter_mut().filter(|node| shows(node)) {
                    node.natural = Some(size);
                }
            }
        }
        for state in &mut self.states {
            for node in state.nodes.iter_mut().filter(|node| shows(node)) {
                if node.natural != Some(size) {
                    node.natural = Some(size);
                    state.laid_for = None;
                }
            }
        }
    }

    /// How many files the UIs were added from.
    #[cfg(all(test, feature = "dev"))]
    pub(crate) fn files(&self) -> usize {
        self.files.len()
    }

    /// The UI `ui`.
    ///
    /// # Panics
    ///
    /// If `ui` is another context's.
    fn state(&mut self, ui: GameUi) -> &mut State {
        match self.states.get_mut(ui.0 as usize) {
            Some(state) => state,
            None => panic!("{ui:?} is a UI of another context"),
        }
    }

    /// Each UI, as the tool API's `ui` lists it, laid out for `view`, the view as the last frame
    /// handed it off, with `fonts`, as [`GameUi::draw`] lays it out.
    #[cfg(feature = "dev")]
    pub(crate) fn listed(&mut self, fonts: &mut Fonts, view: Option<Viewport>) -> gg_api::UiList {
        let units = units(view);
        let size = view.map_or([1920, 1080], |view| view.size);
        let scale = size[1] as f32 / CANVAS_HEIGHT;
        let Self {
            files,
            states,
            frames,
            ..
        } = self;
        let uis = states.iter_mut().map(|state| {
            state.lay_out(fonts, units);
            let file = &files[state.file];
            let nodes = (state.nodes.iter().enumerate()).map(|(i, node)| {
                // JSON has no number that isn't finite, for a client to read. A file's numbers and
                // its nodes are bounded so that none comes to one; were one to, the node lists as
                // nothing at the view's top left rather than breaking the answer.
                let finite = |units: f32| if units.is_finite() { units } else { 0.0 };
                let Rect { at, size } = state.laid.rects[i];
                let rect = [at.x, at.y, size.x, size.y].map(finite);
                let (text, value) = match &node.widget {
                    Widget::Slider { value, .. } => (None, Some(gg_api::UiValue::Number(*value))),
                    Widget::Toggle { on } => {
                        (Some(node.text.clone()), Some(gg_api::UiValue::On(*on)))
                    }
                    Widget::Dropdown {
                        options, choice, ..
                    } => (
                        Some(options[*choice].clone()),
                        Some(gg_api::UiValue::Choice(*choice)),
                    ),
                    Widget::Field { .. } | Widget::None => {
                        (node.kind.says().then(|| node.text.clone()), None)
                    }
                };
                gg_api::UiNode {
                    name: node.name.as_deref().map(str::to_owned),
                    kind: match node.kind {
                        Kind::Panel => gg_api::UiKind::Panel,
                        Kind::Text => gg_api::UiKind::Text,
                        Kind::Image => gg_api::UiKind::Image,
                        Kind::Button => gg_api::UiKind::Button,
                        Kind::Slider => gg_api::UiKind::Slider,
                        Kind::Toggle => gg_api::UiKind::Toggle,
                        Kind::Dropdown => gg_api::UiKind::Dropdown,
                        Kind::Field => gg_api::UiKind::Field,
                        Kind::Row => gg_api::UiKind::Row,
                        Kind::Column => gg_api::UiKind::Column,
                        Kind::Stack => gg_api::UiKind::Stack,
                        Kind::List => gg_api::UiKind::List,
                    },
                    parent: node.parent.map(|parent| parent as usize),
                    rect,
                    pixels: rect.map(|units| finite(units * scale)),
                    visible: state.laid.shown[i],
                    text,
                    value,
                    disabled: node.disabled,
                    focused: state.focused() == Some(i),
                    pressed: state.look(i) == PRESSED,
                }
            });
            gg_api::ListedUi {
                name: state.name.clone(),
                asset: file.asset.to_string(),
                path: file.path.to_string(),
                drawn: state.drawn.is_some_and(|drawn| drawn + 1 >= *frames),
                input: state.input,
                focus: (state.focused())
                    .and_then(|i| state.nodes[i].name.as_deref().map(str::to_owned)),
                nodes: nodes.collect(),
            }
        });
        gg_api::UiList {
            view: size,
            uis: uis.collect(),
        }
    }
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

    /// What `event` does to it, the cursor at `cursor` on the view, in the canvas's units from its
    /// top left, the keys held as `input` has them, and text measured by `fonts`, as
    /// [`GameUi::event`] says.
    fn event(
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
    fn options(&self, i: usize) -> (Rect, usize) {
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
    fn takes_input(&self, frames: u64) -> bool {
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
    fn pointer_over(&self, at: Vec2) -> bool {
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
    fn takes(&self, event: &Event<'_>, cursor: Option<Vec2>, frames: u64) -> bool {
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
            Event::Key { .. } | Event::Text(_) => self.typing().is_some(),
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
    fn focus_to(&mut self, i: usize) -> Option<UiEvent> {
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
    /// selected, or the character before or after the caret; Enter presses it; and Space types,
    /// as its text follows. `None` for a key it leaves to the rest, as Escape, Tab, and the up and
    /// down arrows, which move the focus.
    fn edit(&mut self, i: usize, key: Key, repeat: bool, input: &Input) -> Option<Option<UiEvent>> {
        let shift = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
        let control = input.key_down(Key::ControlLeft) || input.key_down(Key::ControlRight);
        let text = &self.nodes[i].text;
        let Caret { at, from, .. } = self.caret;
        let (low, high) = (at.min(from), at.max(from));
        match key {
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
    fn fit_caret(&mut self, fonts: &mut Fonts) {
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

/// Where `point`, in the canvas's units from the view's top left, +Y down, is on the canvas from
/// `origin`, likewise, +Y up.
fn on_canvas(point: Vec2, origin: Vec2) -> Vec3 {
    Vec3::new(point.x - origin.x, origin.y - point.y, 0.0)
}

/// `rect`'s corners on the canvas from `origin`, its least then its greatest, as a clip is.
fn corners(rect: Rect, origin: Vec2) -> [Vec2; 2] {
    let low = on_canvas(Vec2::new(rect.at.x, rect.at.y + rect.size.y), origin);
    let high = on_canvas(Vec2::new(rect.at.x + rect.size.x, rect.at.y), origin);
    [low.truncate(), high.truncate()]
}

/// The transform of a sprite over `rect`, on the canvas from `origin`.
fn over(rect: Rect, origin: Vec2) -> Mat4 {
    Mat4::from_translation(on_canvas(rect.middle(), origin))
        * Mat4::from_scale(rect.size.extend(1.0))
}

/// Draws `look` over `rect` on `canvas`, from `origin`: its texture cut into nine as `slice` says,
/// or else whole, or without a texture, its corners rounded `radius` by quarters of `disc`; and
/// nothing of a color with no alpha.
fn paint(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    origin: Vec2,
    look: &Look,
    radius: f32,
    slice: Option<(f32, f32)>,
    disc: Option<Texture>,
) {
    if look.color.w <= 0.0 || rect.size.cmple(Vec2::ZERO).any() {
        return;
    }
    let texture = look.texture.as_ref().and_then(|texture| texture.handle);
    let sprite = Sprite {
        color: look.color,
        texture,
        ..Sprite::default()
    };
    match (texture, slice, disc) {
        (Some(_), Some((fraction, units)), _) => {
            nine(canvas, rect, origin, sprite, fraction, units)
        }
        (None, _, Some(disc)) if radius > 0.0 => {
            let rounded = Sprite {
                texture: Some(disc),
                ..sprite
            };
            nine(canvas, rect, origin, rounded, 0.5, radius);
        }
        _ => canvas.draw_sprite(over(rect, origin), sprite),
    }
}

/// Draws `sprite`'s texture cut into nine over `rect` on `canvas`, from `origin`: its corners and
/// edges `fraction` of it in from its sides, drawn `units` wide, or half `rect`'s smaller side if
/// that's less, and the rest stretched between them.
fn nine(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    origin: Vec2,
    sprite: Sprite,
    fraction: f32,
    units: f32,
) {
    let border = units.min(rect.size.x / 2.0).min(rect.size.y / 2.0);
    let end = rect.at + rect.size;
    let xs = [rect.at.x, rect.at.x + border, end.x - border, end.x];
    let ys = [rect.at.y, rect.at.y + border, end.y - border, end.y];
    let cuts = [0.0, fraction, 1.0 - fraction, 1.0];
    for row in 0..3 {
        for column in 0..3 {
            let at = Vec2::new(xs[column], ys[row]);
            let size = Vec2::new(xs[column + 1], ys[row + 1]) - at;
            if size.cmple(Vec2::ZERO).any() {
                continue;
            }
            let piece = Sprite {
                rect: [
                    Vec2::new(cuts[column], cuts[row]),
                    Vec2::new(cuts[column + 1], cuts[row + 1]),
                ],
                ..sprite
            };
            canvas.draw_sprite(over(Rect { at, size }, origin), piece);
        }
    }
}

/// Draws `text` of `node`'s style in `color` in `rect` less its padding on `canvas`, from `origin`,
/// aligned there as the node says.
fn write(canvas: &mut Canvas<'_>, node: &Node, text: &str, rect: Rect, origin: Vec2, color: Vec4) {
    if text.is_empty() || color.w <= 0.0 {
        return;
    }
    let room = rect.inside(node.padding);
    let at = room.at + room.size * Vec2::new(share(node.align[0]), share(node.align[1]));
    let place = Mat4::from_translation(on_canvas(at, origin));
    canvas.draw_text(place, text, &node.style(color));
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
        uis.consumed |= takes;
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
