//! The UIs a context's layers added: each file read once, and each UI's state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "dev")]
use gg_core::math::Vec2;
use gg_renderer::{Texture, Viewport};

#[cfg(doc)]
use super::disc;
#[cfg(feature = "dev")]
use super::layout::Rect;
use super::{Document, GameUi, State, cursor_on, units};
#[cfg(feature = "dev")]
use super::{Kind, Node, PRESSED, Widget};
#[cfg(feature = "dev")]
use crate::Font;
#[cfg(feature = "dev")]
use crate::canvas::CANVAS_HEIGHT;
use crate::font::Fonts;

/// The UIs a context's layers added: each file read once, and each UI with its state.
#[derive(Default)]
pub(crate) struct Uis {
    files: Vec<UiFile>,
    /// Each file's index, by the file its path resolves to.
    by_file: HashMap<PathBuf, usize>,
    states: Vec<State>,
    /// The texture of [`disc`], which rounded corners are cut from, once a UI is added.
    pub(crate) disc: Option<Texture>,
    /// Frames handed off, to tell whether a UI drew in the last.
    pub(super) frames: u64,
    /// Whether a UI took the event ([`GameUi::event`]); the layer stack reads it as the layer
    /// handling the event.
    pub(super) consumed: bool,
}

/// A UI's file, shared by the UIs added from it: its name (asset ID, else path, as for a texture),
/// its path from the asset root with `/` between parts, and its document as last read.
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
    /// The file a UI was added from that `asset` names, as for a texture, if any. An asset ID
    /// matches whatever the file was renamed to. A path (a file without an ID file) matches by
    /// `file`, the file it resolves to, but not a file named by ID that a rename left behind.
    pub(crate) fn file_of(&self, asset: &str, file: Option<&Path>) -> Option<usize> {
        let by_id = crate::assets::is_id(asset).then(|| self.file_named(asset));
        if let Some(found) = by_id.flatten() {
            return Some(found);
        }
        let found = *self.by_file.get(file?)?;
        (!crate::assets::is_id(&self.files[found].asset)).then_some(found)
    }

    /// Takes the file at `path`, named `asset`, read as `document` and resolving to `file` if it
    /// does, and returns its index.
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

    /// Adds a UI of file `file`, which its layer named `name`, with the file's nodes and focus,
    /// taking input, and hands out its handle.
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

    /// Has file `file` hold `document`, read again, and each UI of it take its nodes
    /// ([`State::refit`]).
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

    /// Whether the cursor is over what a UI that takes input and drew in the last frame draws
    /// ([`GameUi::pointer_over`]).
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

    /// Has each file named by asset ID take the path `path_of` now gives that ID, after a rename
    /// or move.
    #[cfg(feature = "dev")]
    pub(crate) fn follow(&mut self, path_of: impl Fn(&str) -> Option<Arc<str>>) {
        for file in &mut self.files {
            if let Some(path) = path_of(&file.asset) {
                file.path = path;
            }
        }
    }

    /// Has each UI that writes in `font`, baked again, lay out again at its next draw.
    #[cfg(feature = "dev")]
    pub(crate) fn font_changed(&mut self, font: Font) {
        for state in &mut self.states {
            let writes = |node: &Node| node.font.as_ref().and_then(|font| font.handle);
            if state.nodes.iter().any(|node| writes(node) == Some(font)) {
                state.laid_for = None;
            }
        }
    }

    /// Has each image of `texture` fit `size`, its pixels as read again, and each UI with one lay
    /// out again at its next draw.
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
    pub(super) fn state(&mut self, ui: GameUi) -> &mut State {
        match self.states.get_mut(ui.0 as usize) {
            Some(state) => state,
            None => panic!("{ui:?} is a UI of another context"),
        }
    }

    /// Each UI as the tool API's `ui` lists it, laid out for `view` (as the last frame handed it
    /// off) like [`GameUi::draw`].
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
                // JSON can't carry a non-finite number. A file's numbers and node count are
                // bounded so none arises; if one did, the node would list as nothing at the
                // view's top left rather than break the answer.
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
