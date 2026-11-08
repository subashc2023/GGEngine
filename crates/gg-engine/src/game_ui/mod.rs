//! A game's UI from a file: `.ui` files under the asset root, each a tree of named nodes, panels,
//! texts, images and buttons in rows, columns and stacks, which a layer adds, getting a handle,
//! and drives each frame, turning input into what happened, a button by its name pressed, the
//! focus moved or back asked for, and drawing it on the view's canvas, laid out by the engine as
//! its tree, its texts or the view's size change. Its state, the focus and what code set of its
//! nodes, is the run's rather than the world's, so that each lockstep peer has its own and none of
//! it is hashed, snapshot or recorded; in development builds it's read again as its file changes,
//! each node keeping what code set of it by its name.

mod file;
mod layout;
#[cfg(test)]
mod tests;

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
/// [`draw`](Self::draw) as it updates, and sets what its nodes say and whether they show, by their
/// names. What it holds is the run's, as ARCHITECTURE.md's Events and layers says.
///
/// Each method panics if the UI is another context's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GameUi(u32);

/// What input did to a UI, as [`GameUi::event`] gives it: a choice that changes the game queues a
/// command, as other input does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// The button of this name was pressed: by a click, released over the button it was pressed
    /// on; by Enter or Space, or a pad's South, on the button with the focus.
    Pressed(Arc<str>),
    /// The focus moved to the button of this name: by the arrows, a pad's D-pad or left stick,
    /// Tab or Shift+Tab, or the cursor coming over it; for the sound a game plays as it moves.
    Focused(Arc<str>),
    /// Back was asked for, by Escape or a pad's East.
    Back,
}

/// A UI's file as it read: its nodes, each after its parent, and the button focused as it's added.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Document {
    pub(crate) nodes: Vec<Node>,
    pub(crate) focus: Option<u32>,
}

/// A node of a UI, as its file gives it and code changes it: what [`Kind`] says it is, and the
/// fields its file gives it, which [`file`](mod@file) says which kinds take.
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
    /// What a text or a button says.
    pub(crate) text: String,
    pub(crate) font: Option<Named<Font>>,
    /// How tall its text's em is, in the canvas's units.
    pub(crate) text_size: f32,
    /// How far its rounded corners reach, in the canvas's units, over its color without a texture.
    pub(crate) radius: f32,
    /// Its texture cut into nine, as `(fraction, units)`: its corners and edges `fraction` of the
    /// texture in, drawn `units` wide, and the rest stretched between them.
    pub(crate) slice: Option<(f32, f32)>,
    /// A button's, normal, focused, pressed and disabled; the first alone for other kinds.
    pub(crate) looks: [Look; 4],
    /// An image's texture's size, in its pixels, once known.
    pub(crate) natural: Option<Vec2>,
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

/// How a panel, an image, a text or one state of a button draws: its color, which tints its
/// texture, and the color of its text, in linear RGBA, alpha not multiplied in.
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
/// button, a panel holding its text, which takes the focus; or a container laying its children
/// out in a row, a column or stacked over each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Panel,
    Text,
    Image,
    Button,
    Row,
    Column,
    Stack,
}

impl Kind {
    /// What it's called, with its article, as refusals name it.
    fn called(self) -> &'static str {
        match self {
            Self::Panel => "a panel",
            Self::Text => "a text",
            Self::Image => "an image",
            Self::Button => "a button",
            Self::Row => "a row",
            Self::Column => "a column",
            Self::Stack => "a stack",
        }
    }

    /// Whether it says something, as a text and a button do.
    fn says(self) -> bool {
        matches!(self, Self::Text | Self::Button)
    }
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
    /// as the file has them, the focus on the button its file names, and taking input.
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
                gg_api::UiNode {
                    name: node.name.as_deref().map(str::to_owned),
                    kind: match node.kind {
                        Kind::Panel => gg_api::UiKind::Panel,
                        Kind::Text => gg_api::UiKind::Text,
                        Kind::Image => gg_api::UiKind::Image,
                        Kind::Button => gg_api::UiKind::Button,
                        Kind::Row => gg_api::UiKind::Row,
                        Kind::Column => gg_api::UiKind::Column,
                        Kind::Stack => gg_api::UiKind::Stack,
                    },
                    parent: node.parent.map(|parent| parent as usize),
                    rect,
                    pixels: rect.map(|units| finite(units * scale)),
                    visible: state.laid.shown[i],
                    text: node.kind.says().then(|| node.text.clone()),
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

/// How a button is held pressed: by the mouse, which presses it as it's released over it, or by a
/// key or a pad's button, which pressed it as it went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Mouse,
    Key,
    Pad,
}

/// The looks of a button, by their places among its looks.
const FOCUSED: usize = 1;
const PRESSED: usize = 2;
const DISABLED: usize = 3;

/// What of a node code set, which it keeps as its file is read again.
#[derive(Clone, Copy, Debug, Default)]
struct Set {
    text: bool,
    visible: bool,
    disabled: bool,
}

/// A UI a layer added: its nodes, as its file gave them and code changed them, how they lie, and
/// its focus and what input holds of it.
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
    /// once its tree, its texts, or a font or a texture it names, reloaded, changed since.
    laid_for: Option<Vec2>,
    /// The button with the focus, and the one its file names.
    focus: Option<usize>,
    file_focus: Option<usize>,
    pressed: Option<(usize, Press)>,
    /// The button the cursor was over as it last moved, pressed or let go.
    hovered: Option<usize>,
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

impl State {
    fn new(file: usize, name: &str, document: &Document) -> Self {
        Self {
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
            input: true,
            pushed: false,
            drawn: None,
        }
    }

    /// Takes the nodes of `document`, its file read again: each node keeps what code set of it, its
    /// text, its visibility or whether it's disabled, where the node of its name had it and it
    /// still takes it; the focus stays on the button of its name, or else goes to the one the file
    /// names; and the layout waits for the next frame.
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
                node.text.clone_from(&old.text);
                kept.text = true;
            }
            if had.visible {
                node.visible = old.visible;
                kept.visible = true;
            }
            if had.disabled && node.kind == Kind::Button && old.kind == Kind::Button {
                node.disabled = old.disabled;
                kept.disabled = true;
            }
        }
        let focused = self.focus.and_then(|focus| self.nodes[focus].name.clone());
        let focus = focused.and_then(|name| {
            (nodes.iter())
                .position(|node| node.kind == Kind::Button && node.name.as_deref() == Some(&*name))
        });
        self.file_focus = document.focus.map(|focus| focus as usize);
        self.focus = focus.or(self.file_focus);
        self.nodes = nodes;
        self.set = set;
        self.pressed = None;
        self.hovered = None;
        self.laid_for = None;
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
    /// lies so already.
    fn lay_out(&mut self, fonts: &mut Fonts, view: Vec2) {
        if self.laid_for == Some(view) {
            return;
        }
        let _zone = gg_core::zone!("lay out a UI");
        layout::lay_out(&self.nodes, fonts, view, &mut self.laid);
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

    /// Whether node `i` takes the focus: a button that shows and isn't disabled.
    fn focusable(&self, i: usize) -> bool {
        let node = &self.nodes[i];
        node.kind == Kind::Button && !node.disabled && self.shows(i)
    }

    /// The button with the focus, while it takes it.
    fn focused(&self) -> Option<usize> {
        self.focus.filter(|&focus| self.focusable(focus))
    }

    /// The name of button `i`, which every button has.
    fn button(&self, i: usize) -> Arc<str> {
        let name = self.nodes[i].name.clone();
        name.expect("a button has a name, as its file is checked")
    }

    /// Which of node `i`'s looks it shows: a button's disabled, pressed, focused or normal; the
    /// first for other kinds.
    fn look(&self, i: usize) -> usize {
        let node = &self.nodes[i];
        if node.kind != Kind::Button {
            return 0;
        }
        let held = match self.pressed {
            Some((at, Press::Mouse)) => at == i && self.hovered == Some(i),
            Some((at, Press::Key | Press::Pad)) => at == i,
            None => false,
        };
        if node.disabled {
            DISABLED
        } else if held {
            PRESSED
        } else if self.focus == Some(i) {
            FOCUSED
        } else {
            0
        }
    }

    /// What `event` does to it, the cursor at `cursor` on the view, in the canvas's units from its
    /// top left, and the keys held as `input` has them, as [`GameUi::event`] says.
    fn event(&mut self, event: &Event<'_>, cursor: Option<Vec2>, input: &Input) -> Option<UiEvent> {
        match *event {
            Event::Key {
                key,
                pressed: true,
                repeat,
            } => match key {
                Key::ArrowLeft => self.step(Toward::Left),
                Key::ArrowRight => self.step(Toward::Right),
                Key::ArrowUp => self.step(Toward::Up),
                Key::ArrowDown => self.step(Toward::Down),
                Key::Tab => {
                    let back = input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight);
                    self.tab(back)
                }
                Key::Enter | Key::NumpadEnter | Key::Space if !repeat => self.press(Press::Key),
                Key::Escape if !repeat => Some(UiEvent::Back),
                _ => None,
            },
            Event::Key {
                key: Key::Enter | Key::NumpadEnter | Key::Space,
                pressed: false,
                ..
            } => self.release(Press::Key),
            Event::PadButton {
                button,
                pressed: true,
                ..
            } => match button {
                PadButton::DPadLeft => self.step(Toward::Left),
                PadButton::DPadRight => self.step(Toward::Right),
                PadButton::DPadUp => self.step(Toward::Up),
                PadButton::DPadDown => self.step(Toward::Down),
                PadButton::South => self.press(Press::Pad),
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
                let length = value.length();
                if length < STICK_RESTS {
                    self.pushed = false;
                    return None;
                }
                if self.pushed || length < STICK_STEPS {
                    return None;
                }
                self.pushed = true;
                let toward = match value.x.abs() > value.y.abs() {
                    true if value.x < 0.0 => Toward::Left,
                    true => Toward::Right,
                    false if value.y < 0.0 => Toward::Down,
                    false => Toward::Up,
                };
                self.step(toward)
            }
            Event::CursorMoved { .. } => {
                self.hovered = cursor.and_then(|at| self.button_at(at));
                match self.hovered {
                    Some(over) if self.focus != Some(over) => self.focus_on(over),
                    _ => None,
                }
            }
            Event::MouseButton {
                button: MouseButton::Left,
                pressed,
            } => {
                self.hovered = cursor.and_then(|at| self.button_at(at));
                if pressed {
                    let over = self.hovered?;
                    self.pressed = Some((over, Press::Mouse));
                    return (self.focus != Some(over))
                        .then(|| self.focus_on(over))
                        .flatten();
                }
                let Some((held, Press::Mouse)) = self.pressed else {
                    return None;
                };
                self.pressed = None;
                (self.hovered == Some(held)).then(|| UiEvent::Pressed(self.button(held)))
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

    /// The topmost button that takes the focus whose rectangle `at` is in.
    fn button_at(&self, at: Vec2) -> Option<usize> {
        (0..self.nodes.len())
            .rev()
            .find(|&i| self.focusable(i) && self.laid.rects[i].contains(at))
    }

    /// Moves the focus to button `i`.
    fn focus_on(&mut self, i: usize) -> Option<UiEvent> {
        self.focus = Some(i);
        Some(UiEvent::Focused(self.button(i)))
    }

    /// Moves the focus to the button its file names, if it takes the focus, or else the first that
    /// does, as a step or a press does with none focused.
    fn focus_first(&mut self) -> Option<UiEvent> {
        let first = self.file_focus.filter(|&i| self.focusable(i));
        let first = first.or_else(|| (0..self.nodes.len()).find(|&i| self.focusable(i)));
        self.focus_on(first?)
    }

    /// Moves the focus `toward` a way, to the nearest button that takes it whose middle is that
    /// way of the focused one's: of those level with it, beside it across the way, as a row's are
    /// to the left and right, the nearest along the way; and else the nearest, as far along as
    /// twice as far off to the side. With none focused, it focuses one, as
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
        self.focus_on(to)
    }

    /// Moves the focus to the next button that takes it in the file's order, or the one before if
    /// `back`, coming round from the last to the first; with none focused, to the first, or the
    /// last.
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
            Some(next) if self.focus != Some(next) => self.focus_on(next),
            _ => None,
        }
    }

    /// Presses the focused button, `by` a key or a pad's button, held until its release; with
    /// none focused, focuses one, as [`focus_first`](Self::focus_first) does.
    fn press(&mut self, by: Press) -> Option<UiEvent> {
        match self.focused() {
            Some(focused) => {
                self.pressed = Some((focused, by));
                Some(UiEvent::Pressed(self.button(focused)))
            }
            None => self.focus_first(),
        }
    }

    /// Lets go of the button held pressed `by` a key or a pad's button.
    fn release(&mut self, by: Press) -> Option<UiEvent> {
        if self.pressed.is_some_and(|(_, held)| held == by) {
            self.pressed = None;
        }
        None
    }

    /// Draws node `i` on `canvas`, from `origin`, the point of the view its top node is anchored
    /// to, in the canvas's units from the view's top left; rounded corners cut from `disc`.
    fn draw_node(&self, i: usize, canvas: &mut Canvas<'_>, origin: Vec2, disc: Option<Texture>) {
        let node = &self.nodes[i];
        let rect = self.laid.rects[i];
        let look = &node.looks[self.look(i)];
        match node.kind {
            Kind::Panel => paint(canvas, rect, origin, look, node.radius, node.slice, disc),
            Kind::Image => paint(canvas, rect, origin, look, 0.0, None, disc),
            Kind::Button => {
                paint(canvas, rect, origin, look, node.radius, node.slice, disc);
                write(canvas, node, rect, origin, look.text);
            }
            Kind::Text => write(canvas, node, rect, origin, look.text),
            Kind::Row | Kind::Column | Kind::Stack => {}
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

/// Where `point`, in the canvas's units from the view's top left, +Y down, is on the canvas from
/// `origin`, likewise, +Y up.
fn on_canvas(point: Vec2, origin: Vec2) -> Vec3 {
    Vec3::new(point.x - origin.x, origin.y - point.y, 0.0)
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

/// Draws `node`'s text in `color` in `rect` less its padding on `canvas`, from `origin`, aligned
/// there as the node says.
fn write(canvas: &mut Canvas<'_>, node: &Node, rect: Rect, origin: Vec2, color: Vec4) {
    if node.text.is_empty() || color.w <= 0.0 {
        return;
    }
    let at = rect.at + node.padding;
    let room = (rect.size - node.padding * 2.0).max(Vec2::ZERO);
    let at = at + room * Vec2::new(share(node.align[0]), share(node.align[1]));
    let place = Mat4::from_translation(on_canvas(at, origin));
    canvas.draw_text(place, &node.text, &node.style(color));
}

impl GameUi {
    /// What `event`, which the layer was sent, does to the UI, while it takes input: the arrows, a
    /// pad's D-pad and its left stick move the focus to the nearest button that takes it that way,
    /// the stick once a push, the way it leans most as it goes past 0.6, until it comes back under
    /// 0.3; Tab to the next in the file's order and Shift+Tab to the one before, coming round;
    /// Enter, Space and a pad's South press the button with the focus, which shows pressed until
    /// they're let go, or focus one if none has it; Escape and a pad's East ask for back; the
    /// cursor coming over a button focuses it, and a click presses it, as it's let go over the
    /// button it went down on. A button that's hidden, under a node that is, or disabled takes
    /// neither the focus nor a press. With the focus on none, a move focuses the button the file
    /// names, or else the first. Each event does one thing at most, which it gives back, `None` for
    /// nothing, so that the layer plays a sound, queues a command or lets the event go on down the
    /// stack.
    pub fn event(self, cx: &mut Context, event: &Event<'_>) -> Option<UiEvent> {
        let Parts {
            uis,
            fonts,
            view,
            input,
            ..
        } = cx.ui_parts();
        let state = uis.state(self);
        if !state.input {
            return None;
        }
        state.lay_out(fonts, units(view));
        state.event(event, cursor_on(view, input.cursor()), input)
    }

    /// Draws the UI on the game's view's canvas this frame, as it updates, laid out for the view as
    /// the last frame handed it off, again only once its tree, its texts, the view's size, or a
    /// font or a texture it names, reloaded, changed: each node at the top from the point of the
    /// view its anchor names, the view's middle by default, so that a resize keeps it there in the
    /// frame it comes in, and those under it in their order, over what's before them, a button in
    /// the look its state shows. It's of no entity, so a pick where it's on top finds none.
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
        state.drawn = Some(frames);
        let cursor = input.cursor();
        let mut top = 0;
        while top < state.nodes.len() {
            let end = state.nodes[top].end as usize;
            if state.laid.shown[top] {
                let anchor = state.nodes[top].anchor.unwrap_or([Align::Center; 2]);
                let origin = units * Vec2::new(share(anchor[0]), share(anchor[1]));
                let mut canvas = Canvas::new(drawn, fonts, shaders, anchor, view, cursor);
                for i in (top..end).filter(|&i| state.laid.shown[i]) {
                    state.draw_node(i, &mut canvas, origin, disc);
                }
            }
            top = end;
        }
    }

    /// Has the text or the button named `name` say `text` from now on, laid out again as the UI's
    /// next drawn, if it said something else; kept as the UI's file is read again. A string the
    /// node says already changes nothing, so that a layer may set it each frame.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that says
    /// nothing.
    pub fn set_text(self, cx: &mut Context, name: &str, text: &str) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "text", Kind::says)?;
        let node = &mut state.nodes[found];
        if node.text != text {
            node.text.clear();
            node.text.push_str(text);
            state.laid_for = None;
        }
        state.set[found].text = true;
        Ok(())
    }

    /// Shows or hides the node named `name`, and what's under it, from now on, kept as the UI's
    /// file is read again: a hidden node takes no room, as if it weren't there, and a button under
    /// it neither the focus nor a press.
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

    /// Disables or enables the button named `name` from now on, kept as the UI's file is read
    /// again: a disabled button shows its disabled look, and takes neither the focus nor a press.
    ///
    /// Fails with `ui.no_node` for a name no node has, and `ui.wrong_kind` for a node that isn't a
    /// button.
    pub fn set_disabled(self, cx: &mut Context, name: &str, disabled: bool) -> Result<()> {
        let state = cx.ui_parts().uis.state(self);
        let found = state.settable(name, "disabling", |kind| kind == Kind::Button)?;
        state.nodes[found].disabled = disabled;
        state.set[found].disabled = true;
        Ok(())
    }

    /// Has the UI take input or not, as [`event`](Self::event) says: as a game's menu takes none
    /// while the game plays under it. It takes input as it's added; turned off, it lets go of what
    /// it held pressed, and keeps its focus.
    pub fn set_input(self, cx: &mut Context, input: bool) {
        let state = cx.ui_parts().uis.state(self);
        state.input = input;
        if !input {
            (state.pressed, state.hovered, state.pushed) = (None, None, false);
        }
    }

    /// The name of the button with the focus, while it takes it: `None` while the focus is on
    /// none, or on a button hidden or disabled since.
    pub fn focused(self, cx: &mut Context) -> Option<Arc<str>> {
        let state = cx.ui_parts().uis.state(self);
        state.focused().map(|focused| state.button(focused))
    }
}
