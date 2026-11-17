//! The dock in a frame: strips off the main window's edges, groups drawn as layers show their
//! panels, a dockspace's view panel, the drags that dock, float and tear off panels, the menus that
//! close and open panels and save, load and reset the layout, and the title bar's buttons that hide
//! and show the panels left of, below and right of the view.

use std::any::Any;
use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::menu::{MenuConfig, SubMenuButton};
use egui::{
    Align, Align2, Area, Button, CentralPanel, Context, CursorIcon, Frame, Id, Key, LayerId, Order,
    PaintCallback, Painter, Panel, PointerButton, Popup, PopupCloseBehavior, Pos2, Rect, Response,
    Sense, StrokeKind, TextEdit, UiBuilder, Vec2, ViewportBuilder, ViewportClass, ViewportCommand,
    ViewportId, pos2, vec2,
};
use gg_renderer::{ViewCallback, Viewport};
use tracing::warn;

use super::tree::{Arrangement, Axis, CENTRAL, Layout, NodeId, Place, Side, Zone};
use super::{Docked, file};
#[cfg(target_os = "windows")]
use crate::chrome::Caption;
use crate::chrome::{self, Chrome, Icon, Look, Sidebar, TitleBar};

/// The size of a new floating group, in points.
const NEW_SIZE: Vec2 = vec2(320.0, 240.0);
/// The smallest a floating group gets, in points.
const MIN_FLOATING: Vec2 = vec2(120.0, 80.0);
/// How far a dragged tab goes past its tab bar before leaving its group, in points.
const DETACH: f32 = 8.0;
/// Where a group's first tab starts, from its tab bar's left, in points.
const INSET: f32 = 4.0;
/// The side of a drop target, in points.
const TARGET: f32 = 32.0;
/// The side of a floating group's resize grip, in points.
const GRIP: f32 = 12.0;
/// How far UI over the game's view keeps from its edges, in points.
const OVER_VIEW: f32 = 8.0;
/// The menu the dock adds its items to.
const VIEW_MENU: &str = "View";

/// What [`Layer::ui`](crate::Layer::ui) draws through: panels, which the user docks, and egui for
/// everything else.
pub struct Ui<'a> {
    root: &'a mut egui::Ui,
    dock: &'a mut Dock,
    /// The type of the last layer whose `ui` was the default, which draws none, since the stack
    /// last asked.
    default_ui: Option<&'static str>,
}

impl<'a> Ui<'a> {
    pub(crate) fn new(root: &'a mut egui::Ui, dock: &'a mut Dock) -> Self {
        Self {
            root,
            dock,
            default_ui: None,
        }
    }

    /// Notes that layer type `layer`'s `ui` is [`Layer::ui`](crate::Layer::ui)'s default.
    pub(crate) fn default_ui(&mut self, layer: &'static str) {
        self.default_ui = Some(layer);
    }

    pub(crate) fn take_default_ui(&mut self) -> Option<&'static str> {
        self.default_ui.take()
    }

    /// Shows panel `name` this frame, drawing `add_contents` in it when its tab is showing. Users
    /// dock panels in tab groups around the central area, or beside the game's view panel, which
    /// fills the main window, with [`App::view_panel`](crate::App::view_panel); or float them over
    /// the main window or in windows of their own. A panel shows from the frame after its layer
    /// starts showing it, and keeps its place while not shown. One the layout lacks floats, unless
    /// [`App::dock`](crate::App::dock) docks it. `name` titles the panel and is its identity:
    /// places are kept by name, between runs too, and a second panel of that name in a frame
    /// doesn't show.
    ///
    /// With [`App::custom_title_bar`](crate::App::custom_title_bar), users close any panel but the
    /// game's view from its tab's menu or the View menu, and reopen it from the View menu. Title
    /// bar buttons hide and show the panels docked left of, below and right of the game's area or
    /// view (across a split on the way to it from the root); those above have none. A closed or
    /// hidden panel shows nowhere and keeps its place, between runs too, while its layer keeps
    /// showing it; `add_contents` doesn't run. A hidden one shows again when opened from the View
    /// menu or when another panel docks on its side. A torn-off window whose panels are all closed
    /// closes, and reopens where it was.
    pub fn panel(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        self.dock.panel(self.root, name, add_contents);
    }

    /// Adds menu `name` to the main window's menus this frame, drawing `add_contents` while it's
    /// open. Menus come in the order layers add them, bottom layer first; a second of the same
    /// name in a frame doesn't show. From the frame after the first shows, a button opens a drawer
    /// of the menus, sliding out from the window's left edge below the bar: at the title bar's
    /// left with [`App::custom_title_bar`](crate::App::custom_title_bar), else in a bar below the
    /// OS's. Each menu opens beside the drawer as the pointer comes onto it, like a submenu. A
    /// click outside, the button again, Escape or a chosen item closes it.
    ///
    /// With [`App::custom_title_bar`](crate::App::custom_title_bar), the dock adds a View menu
    /// after the layers': a checkbox opening and closing each panel the layers show but the game's
    /// view; then items that save the layout under a name, load or delete a saved one, and reset
    /// it to the app's first, as [`App::dock`](crate::App::dock) docks it, no panel closed. They
    /// act as the frame ends. Headless runs keep no layout, so their View menu saves, loads and
    /// deletes none. A layer's own View menu gets the dock's items after its own.
    ///
    /// A Quit item closes the main window like the OS's close button; the layers get
    /// [`Event::CloseRequested`](crate::Event::CloseRequested) first:
    /// `ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close)`.
    pub fn menu(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        let dock = &mut *self.dock;
        match &mut dock.menu {
            Some(menu) if name == VIEW_MENU => {
                let (known, layout, saves) = (&dock.known, &dock.layout, dock.file.is_some());
                dock.bar.menu(name, |ui| {
                    add_contents(ui);
                    ui.separator();
                    menu.ui(ui, known, layout, saves);
                });
            }
            _ => dock.bar.menu(name, add_contents),
        }
    }

    /// Adds `add_contents` to the controls in the middle of the main window's title bar this
    /// frame, after lower layers', replacing the window's title: such as an editor's Play, at hand
    /// whichever panels show. Without [`App::custom_title_bar`](crate::App::custom_title_bar) they
    /// sit in a bar below the OS's, from the frame after the first is added. Returns
    /// `add_contents`'s value, unless the bar doesn't show.
    pub fn title_bar<R>(&mut self, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
        self.dock.bar.controls(add_contents)
    }

    /// Adds `add_contents` to the settings this frame. With
    /// [`App::custom_title_bar`](crate::App::custom_title_bar), a gear left of the panel hide
    /// buttons opens them below it: the engine's own first, the frame rate limits
    /// ([`Context::set_max_fps`](crate::Context::set_max_fps), and an embedded game's), then the
    /// layers', bottom layer first, a line between each. `add_contents` runs while they're open;
    /// the gear, Escape or a click outside closes them, but a click that closes a dropdown's list
    /// in them closes the list alone. Without the custom title bar there's no gear and it doesn't
    /// run. A layer keeps its settings' state itself, like
    /// [`Context::save_state`](crate::Context::save_state).
    pub fn settings(&mut self, add_contents: impl FnOnce(&mut egui::Ui)) {
        self.dock.bar.settings(add_contents);
    }

    /// Shows `add_contents` over the game's view, at `align` within it, 8 points in from its
    /// edges, moved by `offset` in points (such as a toolbar grip's drag) but not past the view's
    /// edges. It's above the view and below other UI over it, such as floating panels and menus.
    /// `id` tells it from other UI over the view. A press on it gives the view the keys, like a
    /// press on the view, without reaching the layers; neither does the wheel over it, and keys
    /// follow the cursor over it as over the view. So a toolbar or gizmo leaves the view its keys,
    /// unless it opens a popup, which takes them as egui's UI does. Shows nothing and returns
    /// `None` while the view doesn't show, and in apps without
    /// [`App::view_panel`](crate::App::view_panel).
    pub fn over_view<R>(
        &mut self,
        id: impl Hash + Debug,
        align: Align2,
        offset: Vec2,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        self.dock
            .show_over_view(self.root.ctx(), id, align, offset, add_contents)
    }

    /// Shows `add_contents` over the main window below its title bar this frame, in front of the
    /// dock, whose groups and view take no input meanwhile: such as an editor's start screen,
    /// before it has a project. The title bar, its menus and controls stay above it; popups, menus
    /// and tooltips show in front. Returns `add_contents`'s value.
    pub fn cover<R>(&mut self, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.dock.cover(self.root.ctx(), add_contents)
    }

    /// What egui makes of the game's view this frame: its rect in the main window's points;
    /// whether the pointer is on it rather than what's over it (`hovered`); whether a drag that
    /// started on it goes on, with any button (`dragged`). Such as to set the cursor over it.
    /// `None` while the view doesn't show, and in apps without
    /// [`App::view_panel`](crate::App::view_panel).
    pub fn view(&self) -> Option<Response> {
        self.dock.spot.as_ref().map(|spot| spot.response.clone())
    }

    /// Where the game's view is as this frame renders it: the central area or the
    /// [`App::view_panel`](crate::App::view_panel), in the main window's physical pixels from its
    /// top left, rounded like egui's clip rectangles. It matches
    /// [`Input::cursor`](crate::Input::cursor)'s space and
    /// [`Context::view`](crate::Context::view) once the frame reaches the renderer. Such as to
    /// draw over the view where the frame draws the world. `None` while it has no area.
    pub fn viewport(&self) -> Option<Viewport> {
        let ppp = self.root.ctx().pixels_per_point();
        let viewport = Viewport::from_points(self.dock.view(), ppp);
        (viewport.size[0] > 0 && viewport.size[1] > 0).then_some(viewport)
    }

    /// egui, for UI that isn't a panel.
    pub fn egui(&self) -> &egui::Context {
        self.root.ctx()
    }

    /// Gives the game's view the keys as a press on it does, taking them from any egui widget, as
    /// the frame's UI ends. For a control, such as an editor's Play, that hands the view its input
    /// after its press took the keys. Does nothing in apps without
    /// [`App::view_panel`](crate::App::view_panel).
    pub fn focus_view(&mut self) {
        self.dock.focus_asked = true;
    }

    /// Whether the game's view has the keys, as input before this frame left them: from the start,
    /// a press on it or on what [`over_view`](Self::over_view) shows over it, and
    /// [`focus_view`](Self::focus_view), until a press elsewhere. `false` in apps without
    /// [`App::view_panel`](crate::App::view_panel).
    pub fn view_focused(&self) -> bool {
        self.dock.view_focused
    }
}

/// The dock's layout, and what it needs from one frame to the next.
pub(crate) struct Dock {
    layout: Layout,
    /// Where the layout is saved; `None` keeps it in memory only, as headless runs do.
    file: Option<PathBuf>,
    /// The title bar (or the bar below the OS's), and the main window's rest below it this frame,
    /// in points.
    bar: TitleBar,
    below_bar: Rect,
    /// Draws the frames of windows of their own.
    chrome: Chrome,
    /// The panels shown last frame, which tab bars list, and so far this frame.
    shown: HashSet<String>,
    showing: HashSet<String>,
    arrangement: Arrangement,
    /// The groups drawn this frame.
    drawn: HashSet<NodeId>,
    /// Panels shown this frame that the layout lacks, placed as the frame ends.
    new: Vec<String>,
    /// A tab clicked this frame; its panel shows from the next, so the layout holds still while
    /// panels draw.
    clicked: Option<String>,
    drag: Option<Drag>,
    /// What a dragged group is over, as of the last frame's end.
    hover: Option<Hover>,
    /// Where the pointer holds the splitter it drags, from the split, in points.
    splitter_grab: f32,
    /// Where a dockspace's view shows this frame, once its panel draws.
    spot: Option<Spot>,
    /// The layers of UI over the view so far this frame, and last frame's. Last frame's tell
    /// input what's over the view: egui forgets which layers are another's as a frame ends.
    overlays: Vec<LayerId>,
    overlaid: Vec<LayerId>,
    /// The renderer's callback drawing the view in its panel.
    callback: Arc<dyn Any + Send + Sync>,
    /// The panels the layers showed last frame (closed ones too) in order, and so far this frame.
    known: Vec<String>,
    knowing: Vec<String>,
    /// Where the app docks its panels as they first show, in its order.
    docked: Vec<(&'static str, Docked)>,
    /// The View menu, in apps that have it; there tabs close their panels too.
    menu: Option<ViewMenu>,
    /// Whether a dockspace's view had the keys as the frame started, as the windowed loop tells
    /// it (true from the start); and whether a layer asked this frame to have them, which the
    /// loop takes after the frame's UI.
    view_focused: bool,
    focus_asked: bool,
}

/// The View menu's items that save and load layouts, and what the menus chose this frame.
#[derive(Default)]
struct ViewMenu {
    /// What the View menu and tabs' menus chose, done as the frame ends.
    chosen: Vec<Choice>,
    /// The saved layouts' names, alphabetically, as last read.
    saved: Vec<String>,
    /// What's typed in the save-name field, and why the name last saved from it was refused.
    name: String,
    refused: Option<String>,
}

/// What the user chose from a menu.
enum Choice {
    Close(String),
    Open(String),
    Save(String),
    Load(String),
    Delete(String),
    Reset,
}

/// Where the game's view shows in a dockspace.
#[derive(Clone)]
struct Spot {
    /// In the main window's points.
    rect: Rect,
    /// The background's when its group is docked, else its floating group's area's.
    layer: LayerId,
    /// What egui made of the view's widget.
    response: Response,
}

/// What's at a point of the main window as the last frame drew it; decides which input reaches
/// the layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Under {
    /// A dockspace's view, clear of what's over it.
    View,
    /// UI that [`Ui::over_view`] shows over the view.
    Overlay,
    /// Anything else: egui's.
    Other,
}

struct Drag {
    /// The window the drag started in, whose pointer it follows until the button is let go.
    source: ViewportId,
    /// The group it started in, whose window stays open while the drag lasts.
    from: NodeId,
    what: Dragging,
    /// Where the pointer holds what's dragged, from its top-left corner, in points.
    grab: Vec2,
    /// The size of a group made for a dragged tab, in points.
    size: Vec2,
}

enum Dragging {
    /// A tab still in its bar, whose rect is in the source window's points.
    Tab { panel: String, bar: Rect },
    /// A floating group, which follows the pointer.
    Group(NodeId),
}

/// Where a dragged group would drop.
#[derive(Clone, Copy)]
struct Hover {
    /// The node under the pointer, and its rect.
    node: NodeId,
    rect: Rect,
    /// The node's window of its own; `None` for the main window.
    window: Option<NodeId>,
    /// The zones it offers, and the one under the pointer.
    zones: Zones,
    zone: Option<Zone>,
    /// The share of its rect a group docked beside it takes.
    share: f32,
}

/// Which drop targets a node has.
#[derive(Clone, Copy, PartialEq)]
enum Zones {
    /// A docked group: among its tabs, and beside it.
    All,
    /// The central area: beside it only.
    Sides,
    /// A floating group, or a dockspace's empty dock: among its tabs only.
    Center,
}

impl Dock {
    /// A dock with the layout saved in `file` (in memory only without one). A `view` makes a
    /// dockspace, the panel of that name showing the game's view. With `menu`, the main window has
    /// the View menu and tabs close their panels. An unreadable file logs a warning and the dock
    /// starts new.
    pub(crate) fn new(
        file: Option<PathBuf>,
        chrome: Chrome,
        view: Option<&'static str>,
        menu: bool,
        docked: &[(&'static str, Docked)],
    ) -> Self {
        let mut layout = match &file {
            Some(path) => file::load(path, view).unwrap_or_else(|error| {
                warn!(code = error.code(), path = %path.display(), "{error:#}; the dock starts as new");
                Layout::new(view)
            }),
            None => Layout::new(view),
        };
        // Without the View menu, nothing would open them again.
        if !menu {
            layout.closed.clear();
        }
        let menu = menu.then(|| ViewMenu {
            saved: file.as_deref().map(saved).unwrap_or_default(),
            ..ViewMenu::default()
        });
        let arrangement = layout.arrange(Rect::NOTHING, &|_| false);
        Self {
            layout,
            file,
            bar: TitleBar::new(chrome),
            below_bar: Rect::NOTHING,
            chrome,
            shown: HashSet::new(),
            showing: HashSet::new(),
            arrangement,
            drawn: HashSet::new(),
            new: Vec::new(),
            clicked: None,
            drag: None,
            hover: None,
            splitter_grab: 0.0,
            spot: None,
            overlays: Vec::new(),
            overlaid: Vec::new(),
            callback: Arc::new(ViewCallback),
            known: Vec::new(),
            knowing: Vec::new(),
            docked: docked.to_vec(),
            menu,
            view_focused: view.is_some(),
            focus_asked: false,
        }
    }

    /// Saves the layout if it has a file. A failure logs a warning.
    pub(crate) fn save(&self) {
        let _zone = gg_core::zone!();
        if let Some(path) = &self.file
            && let Err(error) = file::save(path, &self.layout.saved())
        {
            warn!(code = error.code(), path = %path.display(), "{error:#}");
        }
    }

    pub(crate) fn is_dockspace(&self) -> bool {
        self.layout.view.is_some()
    }

    /// Where the game's view shows this frame, in the main window's points: the central area, or
    /// a dockspace's view panel's inside once drawn, else nothing.
    pub(crate) fn view(&self) -> Rect {
        match self.layout.view {
            Some(_) => self.spot.as_ref().map_or(Rect::NOTHING, |spot| spot.rect),
            None => self.arrangement.central,
        }
    }

    /// What's at `pos` in the main window's points as the last frame drew it: [`Ui::over_view`]
    /// UI; the view, clear of groups, menus and its floating group's bottom-right resize grip; or
    /// else egui's.
    pub(crate) fn under(&self, egui: &Context, pos: Pos2) -> Under {
        let Some(spot) = &self.spot else {
            return Under::Other;
        };
        let layer = egui.layer_id_at(pos);
        if layer.is_some_and(|layer| self.overlaid.contains(&layer)) {
            return Under::Overlay;
        }
        let floats = spot.layer.order != Order::Background;
        let grip = Rect::from_min_max(spot.rect.max - Vec2::splat(GRIP), spot.rect.max);
        if spot.rect.contains(pos) && layer == Some(spot.layer) && !(floats && grip.contains(pos)) {
            Under::View
        } else {
            Under::Other
        }
    }

    /// Called as the frame starts: [`Ui::view_focused`]'s answer.
    pub(crate) fn set_view_focused(&mut self, focused: bool) {
        self.view_focused = focused && self.is_dockspace();
    }

    /// Whether [`Ui::focus_view`] was called since this was last asked.
    pub(crate) fn take_focus_asked(&mut self) -> bool {
        mem::take(&mut self.focus_asked)
    }

    /// Where the cursor goes as a grab starts, in the main window's points, from its `cursor`
    /// position: the center of a dockspace's view, unless it's already on the view clear of what's
    /// over it. The grab then holds it there, not over egui's UI, such as the title bar control
    /// whose click started it, which would take its clicks. `None` where it stays: on the view,
    /// and while no dockspace's view shows.
    pub(crate) fn grab_at(&self, egui: &Context, cursor: Option<Pos2>) -> Option<Pos2> {
        let spot = self.spot.as_ref()?;
        let on_view = cursor.is_some_and(|pos| self.under(egui, pos) == Under::View);
        (!on_view).then(|| spot.rect.center())
    }

    /// [`Ui::over_view`]: an area of the view's layer's order, a sublayer egui forgets as each
    /// frame ends.
    fn show_over_view<R>(
        &mut self,
        ctx: &Context,
        id: impl Hash + Debug,
        align: Align2,
        offset: Vec2,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        let spot = self.spot.as_ref()?;
        let (rect, parent) = (spot.rect, spot.layer);
        let shown = Area::new(Id::unique(("gg dock over view", id)))
            .order(parent.order)
            .pivot(align)
            .fixed_pos(align.pos_in_rect(&rect.shrink(OVER_VIEW)) + offset)
            .constrain_to(rect)
            // Its own presses, so egui's hit test doesn't take one near its edge for the view's.
            .sense(Sense::CLICK | Sense::DRAG)
            .show(ctx, add_contents);
        let layer = shown.response.layer_id;
        ctx.set_sublayer(parent, layer);
        self.overlays.push(layer);
        // egui raises a pressed area, which as a sublayer stays over its parent: the view's
        // floating group comes up with it, as a press in the group raises it.
        if parent.order != Order::Background
            && shown.response.contains_pointer()
            && ctx.input(|i| i.pointer.any_pressed())
        {
            ctx.move_to_top(parent);
        }
        Some(shown.inner)
    }

    /// [`Ui::cover`]: an area of the middle order, like floating groups. egui raises it above them
    /// all as a new area, and it stays as nothing under it is pressed. Its own presses stop any
    /// going through.
    fn cover<R>(&self, ctx: &Context, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let rect = self.below_bar;
        let shown = Area::new(Id::unique("gg dock cover"))
            .order(Order::Middle)
            .fixed_pos(rect.min)
            .constrain(false)
            .sense(Sense::CLICK | Sense::DRAG)
            .show(ctx, |ui| {
                ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.set_clip_rect(rect);
                    ui.set_min_size(rect.size());
                    add_contents(ui)
                })
                .inner
            });
        shown.inner
    }

    /// Where the title bar was right-clicked since this was last asked, in the main window's
    /// points, for the OS's window menu.
    pub(crate) fn take_window_menu(&mut self) -> Option<Pos2> {
        self.bar.take_window_menu()
    }

    /// The parts of the title bar that were the OS's last frame, in the main window's points.
    #[cfg(target_os = "windows")]
    pub(crate) fn caption(&self) -> Option<Caption> {
        self.bar.caption()
    }

    /// How the maximize button looks from the next frame, as the OS says.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_maximize(&mut self, look: Look) {
        self.bar.set_maximize(look);
    }

    /// Starts a frame: takes the title bar off the top of `root`, then the strips along the
    /// window's edges that docked groups fill, leaving the central area. egui counts the bar and
    /// strips as its own, the central area as the game's. A dockspace has no strips: egui counts
    /// the whole dock as its own, and the view panel draws first.
    pub(crate) fn begin(&mut self, root: &mut egui::Ui) {
        self.bar.begin(root);
        self.below_bar = root.available_rect_before_wrap();
        let view = self.layout.view;
        let embedded = root.ctx().embed_viewports();
        for floating in &mut self.layout.floating {
            // Without windows of their own, groups float over the main window; the layout keeps
            // their window places for runs with them. The view's group never has a window: only
            // the main window's input reaches the layers.
            let holds_view = view.is_some_and(|view| floating.tabs.holds(view));
            if let Place::Window { position, size } = floating.place
                && (embedded || holds_view)
            {
                let min = from_desktop(root.ctx(), position).unwrap_or(pos2(48.0, 48.0));
                floating.kept = (!holds_view).then_some(floating.place);
                floating.place = Place::Main {
                    min: min.into(),
                    size,
                };
            }
        }
        let shown = &self.shown;
        self.arrangement = self
            .layout
            .arrange(root.available_rect_before_wrap(), &|p| shown.contains(p));
        let fill = Frame::NONE.fill(root.visuals().panel_fill);
        for (i, &(side, size)) in self.arrangement.strips.iter().enumerate() {
            let id = ("gg dock strip", i);
            let panel = match side {
                Side::Left => Panel::left(id),
                Side::Right => Panel::right(id),
                Side::Top => Panel::top(id),
                Side::Bottom => Panel::bottom(id),
            };
            panel
                .exact_size(size)
                .frame(fill)
                .resizable(false)
                .show_separator_line(false)
                .show(root, |_| {});
        }
        // Before any group draws: one drawn before would be drawn again as the frame ends.
        self.drawn.clear();
        self.spot = None;
        if let Some(view) = view {
            // The rest is the empty dock where no group is.
            CentralPanel::default().frame(fill).show(root, |_| {});
            self.panel(root, view, |_| {});
        }
    }

    fn panel(&mut self, root: &mut egui::Ui, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        if self.knowing.iter().any(|known| known == name) {
            return;
        }
        self.knowing.push(name.to_owned());
        if self.layout.is_closed(name) || self.layout.is_hidden(name) {
            return;
        }
        self.showing.insert(name.to_owned());
        let Some(id) = self.layout.find(name) else {
            self.new.push(name.to_owned());
            return;
        };
        let shown = &self.shown;
        let tabs = self.layout.group(id).expect("the group holding the panel");
        if tabs.showing(&|p| shown.contains(p)) == Some(name) {
            self.draw(root, id, add_contents);
        }
    }

    /// Ends a frame: adds the View menu, finishes the title bar (toggling the side whose button
    /// was clicked), draws what the panels didn't, moves the drag, and applies the frame's layout
    /// changes: the menus' choices, and panels the layout lacks, docked as the app says or
    /// floating.
    pub(crate) fn end(&mut self, root: &mut egui::Ui) {
        if let Some(menu) = &mut self.menu {
            let (known, layout, saves) = (&self.known, &self.layout, self.file.is_some());
            self.bar
                .menu(VIEW_MENU, |ui| menu.ui(ui, known, layout, saves));
        }
        // `Some(shown)` if a side has open panels the layers show.
        let (layout, knowing) = (&self.layout, &self.knowing);
        let toggled = self.bar.end(|sidebar| {
            let side = side_of(sidebar);
            let open = knowing.iter().filter(|panel| !layout.is_closed(panel));
            let mut docked = open.filter(|panel| layout.side(panel) == Some(side));
            docked.next().map(|_| !layout.hidden.contains(&side))
        });
        if let Some(sidebar) = toggled {
            let side = side_of(sidebar);
            self.set_hidden(side, !self.layout.hidden.contains(&side));
            self.save();
        }
        let ctx = root.ctx().clone();
        // Tab bars of groups whose showing panel wasn't shown, to pick another; and the window a
        // drag started in, which carries the pointer.
        let shown = &self.shown;
        let from = self.drag.as_ref().map(|drag| drag.from);
        let docked = self.arrangement.groups.iter().map(|&(id, _)| id);
        let rest: Vec<NodeId> = (docked.chain(self.layout.floating.iter().map(|f| f.tabs.id)))
            .filter(|id| !self.drawn.contains(id))
            .filter(|&id| {
                Some(id) == from
                    || self
                        .layout
                        .group(id)
                        .is_some_and(|tabs| tabs.showing(&|p| shown.contains(p)).is_some())
            })
            .collect();
        for id in rest {
            self.draw(root, id, |_| {});
        }
        if let Some(drag) = &self.drag
            && drag.source == ViewportId::ROOT
        {
            root.interact(root.max_rect(), drag_id(), Sense::drag());
        }
        self.splitters(root);
        self.update_drag(&ctx);
        if let Some(hover) = self.hover.filter(|hover| hover.window.is_none()) {
            paint_hover(&ctx, &hover);
        }
        self.follow_windows(&ctx);

        if let Some(panel) = self.clicked.take() {
            self.layout.activate(&panel);
        }
        self.choose();
        // The app's panels first, in its order, so each docks after its target.
        let mut new = mem::take(&mut self.new);
        let docked = &self.docked;
        new.sort_by_key(|panel| {
            let at = docked.iter().position(|&(p, _)| p == panel.as_str());
            at.unwrap_or(docked.len())
        });
        for panel in new {
            let n = self.layout.floating.len() % 8;
            let at = 48.0 + 24.0 * n as f32;
            let place = Place::Main {
                min: [at, at],
                size: NEW_SIZE.into(),
            };
            self.layout.add(panel.clone(), place, &self.docked);
            self.reveal(&panel);
        }
        mem::swap(&mut self.shown, &mut self.showing);
        self.showing.clear();
        mem::swap(&mut self.overlaid, &mut self.overlays);
        self.overlays.clear();
        mem::swap(&mut self.known, &mut self.knowing);
        self.knowing.clear();
    }

    fn choose(&mut self) {
        let chosen = (self.menu.as_mut()).map(|menu| mem::take(&mut menu.chosen));
        let Some(chosen) = chosen.filter(|chosen| !chosen.is_empty()) else {
            return;
        };
        let _zone = gg_core::zone!();
        for choice in chosen {
            // Headless runs offer no named-layout choices.
            let path = self.file.as_deref();
            match choice {
                Choice::Close(panel) => {
                    // Its tab doesn't linger a frame.
                    if self.layout.close(&panel) {
                        self.showing.remove(&panel);
                    }
                }
                Choice::Open(panel) => {
                    self.layout.open(&panel);
                    self.reveal(&panel);
                    // From the next frame, where it was, or placed as this one ends.
                    if self.knowing.contains(&panel) {
                        if self.layout.find(&panel).is_none() {
                            self.new.push(panel.clone());
                        }
                        self.showing.insert(panel);
                    }
                }
                Choice::Reset => self.switch(Layout::new(self.layout.view)),
                Choice::Load(name) => {
                    let loaded = path.map(|path| file::load_named(path, &name, self.layout.view));
                    match loaded {
                        Some(Ok(layout)) => self.switch(layout),
                        Some(Err(error)) => {
                            let stays = "the dock stays as it was";
                            warn!(code = error.code(), layout = %name, "{error:#}; {stays}");
                        }
                        None => {}
                    }
                }
                Choice::Save(name) => {
                    if let Some(path) = path {
                        if let Err(error) = file::save_named(path, &name, &self.layout.saved()) {
                            warn!(code = error.code(), layout = %name, "{error:#}");
                        }
                        if let Some(menu) = &mut self.menu {
                            menu.saved = saved(path);
                        }
                    }
                }
                Choice::Delete(name) => {
                    if let Some(path) = path {
                        if let Err(error) = file::delete_named(path, &name) {
                            warn!(code = error.code(), layout = %name, "{error:#}");
                        }
                        if let Some(menu) = &mut self.menu {
                            menu.saved = saved(path);
                        }
                    }
                }
            }
        }
        self.save();
    }

    /// Takes effect from the next frame.
    fn set_hidden(&mut self, side: Side, hidden: bool) {
        self.layout.set_hidden(side, hidden);
        for panel in &self.knowing {
            if self.layout.side(panel) != Some(side) || self.layout.is_closed(panel) {
                continue;
            }
            if hidden {
                self.showing.remove(panel);
            } else {
                self.showing.insert(panel.clone());
            }
        }
    }

    /// Shows `panel`'s side from the next frame if hidden, as the panel opens or docks there.
    fn reveal(&mut self, panel: &str) {
        if let Some(side) =
            (self.layout.side(panel)).filter(|side| self.layout.hidden.contains(side))
        {
            self.set_hidden(side, false);
        }
    }

    /// Replaces the dock's layout with `layout` as the frame ends, numbering its nodes after the
    /// dock's. From the next frame this frame's panels show where it says, except those it
    /// closes; ones it lacks are placed as the frame ends.
    fn switch(&mut self, layout: Layout) {
        self.layout.replace(layout);
        let open = |panel: &&String| !self.layout.is_closed(panel);
        self.showing = self.knowing.iter().filter(open).cloned().collect();
        let held = |panel: &&String| self.layout.find(panel).is_some();
        self.new = (self.knowing.iter())
            .filter(|panel| open(panel) && !held(panel))
            .cloned()
            .collect();
        // What it dragged was the last layout's.
        self.drag = None;
        self.hover = None;
    }

    /// Draws group `id`, `add_contents` drawing its showing panel.
    fn draw(&mut self, root: &mut egui::Ui, id: NodeId, add_contents: impl FnOnce(&mut egui::Ui)) {
        if !self.drawn.insert(id) {
            return;
        }
        if let Some(&(_, rect)) = self
            .arrangement
            .groups
            .iter()
            .find(|(group, _)| *group == id)
        {
            let builder = UiBuilder::new()
                .max_rect(rect)
                .id_salt(("gg dock group", id));
            let mut ui = root.new_child(builder);
            ui.set_clip_rect(rect.intersect(root.clip_rect()));
            ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
            self.body(&mut ui, id, false, false, add_contents);
            return;
        }
        // A docked group with no panel shown last frame takes no room until the next.
        let Some(floating) = self.layout.floating(id) else {
            return;
        };
        let ctx = root.ctx().clone();
        match floating.place {
            Place::Main { min, size } => {
                self.draw_floating(&ctx, id, min.into(), size.into(), add_contents);
            }
            Place::Window { position, size } => {
                self.draw_window(&ctx, id, position, size.into(), add_contents);
            }
        }
    }

    /// Draws a floating group over the main window, with a resize grip.
    fn draw_floating(
        &mut self,
        ctx: &Context,
        id: NodeId,
        min: Pos2,
        size: Vec2,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        let response = Area::new(Id::unique(("gg dock group", id)))
            .order(Order::Middle)
            .current_pos(min)
            .movable(false)
            // Off the title bar.
            .constrain_to(self.arrangement.area)
            .show(ctx, |ui| {
                let rect = Rect::from_min_size(ui.min_rect().min, size);
                let visuals = ui.visuals();
                let (corner, fill, stroke) = (
                    visuals.window_corner_radius,
                    visuals.window_fill,
                    visuals.window_stroke,
                );
                ui.painter()
                    .rect(rect, corner, fill, stroke, StrokeKind::Inside);
                let mut group = ui.new_child(UiBuilder::new().max_rect(rect));
                group.set_clip_rect(rect);
                self.body(&mut group, id, true, false, add_contents);
                let grip = Rect::from_min_max(rect.max - Vec2::splat(GRIP), rect.max);
                let resize = ui
                    .interact(grip, Id::unique(("gg dock resize", id)), Sense::drag())
                    .on_hover_cursor(CursorIcon::ResizeNwSe);
                let stroke = ui.visuals().widgets.noninteractive.fg_stroke;
                for inset in [3.0, 7.0] {
                    let corner = grip.max - Vec2::splat(2.0);
                    ui.painter().line_segment(
                        [
                            corner - vec2(GRIP - inset, 0.0),
                            corner - vec2(0.0, GRIP - inset),
                        ],
                        stroke,
                    );
                }
                ui.allocate_rect(rect, Sense::hover());
                resize.drag_delta()
            });
        if let Some(floating) = self.layout.floating_mut(id)
            && matches!(floating.place, Place::Main { .. })
        {
            floating.place = Place::Main {
                min: response.response.rect.min.into(),
                size: (size + response.inner).max(MIN_FLOATING).into(),
            };
            if response.inner != Vec2::ZERO {
                floating.kept = None;
            }
        }
    }

    /// Draws a floating group in a window at `position` on the desktop in physical pixels, `size`
    /// points inside.
    fn draw_window(
        &mut self,
        ctx: &Context,
        id: NodeId,
        position: [i32; 2],
        size: Vec2,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        let viewport = window_id(id);
        let open = geometry(ctx, viewport);
        // egui places windows in their own points; one not open yet, in the main window's.
        let ppp = open
            .or_else(|| geometry(ctx, ViewportId::ROOT))
            .map_or(ctx.pixels_per_point(), |(_, ppp)| ppp);
        let shown = &self.shown;
        let title = (self.layout.group(id))
            .and_then(|tabs| tabs.showing(&|p| shown.contains(p)))
            .unwrap_or_default()
            .to_owned();
        let mut builder = ViewportBuilder::default()
            .with_title(title)
            .with_min_inner_size(MIN_FLOATING)
            // Inactive, so the window a drag started in keeps the pointer.
            .with_active(false)
            // Off the taskbar, as a minimized window would be hard to find.
            .with_taskbar(false)
            .with_minimize_button(false);
        builder = match self.chrome {
            Chrome::Os => builder,
            Chrome::Drawn | Chrome::InWindowsFrame => builder.with_decorations(false),
            // The OS keeps the edges.
            Chrome::AroundTrafficLights => builder
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_titlebar_buttons_shown(false),
        };
        // egui moves or sizes the window whenever these change, fighting the user: set them as it
        // opens, and its position while the dock drags it.
        let dragged =
            matches!(&self.drag, Some(Drag { what: Dragging::Group(group), .. }) if *group == id);
        if open.is_none() {
            builder = builder.with_inner_size(size);
        }
        if open.is_none() || dragged {
            builder =
                builder.with_position(pos2(position[0] as f32 / ppp, position[1] as f32 / ppp));
        }
        let mut add_contents = Some(add_contents);
        ctx.show_viewport_immediate(viewport, builder, |ui, class| {
            // The UI frames a window of its own, its tab bar the title bar, but not one shown in
            // the main window, whose closing and resizing it would take over.
            let framed = self.chrome != Chrome::Os && class == ViewportClass::Immediate;
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
            let mut group = ui.new_child(UiBuilder::new().max_rect(rect));
            self.body(&mut group, id, framed, framed, |ui| {
                if let Some(add_contents) = add_contents.take() {
                    add_contents(ui);
                }
            });
            if framed && self.chrome == Chrome::Drawn {
                chrome::edges(ui.ctx());
            }
            if self
                .drag
                .as_ref()
                .is_some_and(|drag| drag.source == viewport)
            {
                ui.interact(rect, drag_id(), Sense::drag());
            }
            // Repeat the targets in the dragged group's window, as Dear ImGui does; it would hide
            // them.
            if let Some(hover) = self.hover {
                let from = hover.window.map_or(ViewportId::ROOT, window_id);
                if from == viewport {
                    paint_hover(ui.ctx(), &hover);
                } else if dragged && let Some(rect) = between(ui.ctx(), hover.rect, from, viewport)
                {
                    paint_hover(ui.ctx(), &Hover { rect, ..hover });
                }
            }
        });
    }

    /// Draws a group filling `ui`: a tab bar with a tab per panel shown last frame, then
    /// `add_contents` for the showing one, or a dockspace's view. A clicked tab's panel shows from
    /// the next frame. Dragging a tab with the primary button drags its panel. In a `movable`
    /// group (floating over the main window, or its bar is its window's title bar), dragging the
    /// bar or the only tab showing drags the group. `close` adds a window title bar's close
    /// button. With the View menu, a tab's right-click menu closes its panel, except the view's.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        movable: bool,
        close: bool,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        let rect = ui.max_rect();
        let spacing = ui.spacing().clone();
        let bar_height = spacing.interact_size.y + 2.0 * spacing.item_spacing.y;
        let (bar, content) = rect.split_top_bottom_at_y(rect.min.y + bar_height);
        ui.painter()
            .rect_filled(bar, 0.0, ui.visuals().faint_bg_color);
        let bar = if close {
            let (bar, button) = bar.split_left_right_at_x(bar.max.x - chrome::BUTTON);
            if chrome::button(ui, button, Icon::Close, Look::Idle).clicked() {
                // The dock floats the group over the main window once its window closes.
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
            bar
        } else {
            bar
        };
        let background =
            movable.then(|| ui.interact(bar, Id::unique(("gg dock bar", id)), Sense::drag()));
        let shown = &self.shown;
        let Some(tabs) = self.layout.group(id) else {
            return;
        };
        let showing = tabs.showing(&|p| shown.contains(p));
        let view = showing.is_some() && showing == self.layout.view;
        let single = tabs.panels.iter().filter(|p| shown.contains(*p)).count() == 1;
        let floating = self.layout.floating(id).is_some();
        // With the View menu, which reopens them; the view stays, as nothing else shows the game.
        let closes = |panel: &str| self.menu.is_some() && Some(panel) != self.layout.view;
        let mut clicked = None;
        let mut dragged = None;
        let mut closed = None;
        let mut tab_bar = ui.new_child(
            UiBuilder::new()
                .max_rect(bar.with_min_x(bar.min.x + INSET))
                .layout(egui::Layout::left_to_right(Align::Center)),
        );
        for panel in tabs.panels.iter().filter(|p| shown.contains(*p)) {
            let tab = Button::selectable(showing == Some(panel.as_str()), panel.as_str())
                .sense(Sense::click_and_drag());
            let response = tab_bar.add(tab);
            if response.clicked() {
                clicked = Some(panel.clone());
            }
            // Primary button only: a right-press flicked off a tab does nothing.
            if response.drag_started_by(PointerButton::Primary) {
                dragged = Some((panel.clone(), response.rect));
            }
            if closes(panel) {
                let menu = Popup::context_menu(&response).style(crate::menu_style);
                menu.show(|ui| {
                    if ui.button("Close").clicked() {
                        closed = Some(panel.clone());
                    }
                });
            }
        }
        // The view fills its panel; other panels' contents keep off its edges.
        let inside = if view {
            content
        } else {
            content.shrink(spacing.item_spacing.x)
        };
        let mut content_ui = ui.new_child(UiBuilder::new().max_rect(inside));
        content_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        if view {
            self.show_view(&mut content_ui);
        }
        add_contents(&mut content_ui);

        let ctx = ui.ctx();
        let press = ctx.input(|i| i.pointer.press_origin()).unwrap_or(rect.min);
        if let Some((panel, tab)) = dragged {
            let drag = if movable && single {
                (Dragging::Group(id), press - rect.min)
            } else {
                // The tab is first in its new group; the pointer keeps its place on it.
                let grab = vec2(INSET + press.x - tab.min.x, press.y - bar.min.y);
                (Dragging::Tab { panel, bar }, grab)
            };
            let size = if floating { rect.size() } else { NEW_SIZE };
            self.start_drag(ctx, id, drag, size);
        } else if background
            .is_some_and(|response| response.drag_started_by(PointerButton::Primary))
        {
            self.start_drag(
                ctx,
                id,
                (Dragging::Group(id), press - rect.min),
                rect.size(),
            );
        }
        if clicked.is_some() {
            self.clicked = clicked;
        }
        if let Some(panel) = closed
            && let Some(menu) = &mut self.menu
        {
            menu.chosen.push(Choice::Close(panel));
        }
    }

    /// Draws a dockspace's view filling `ui` through the renderer's callback, and notes where it
    /// is. It senses presses itself, so egui's hit test gives it one within a few points of a tab
    /// or splitter. It can't take egui's focus, so Tab never lands on it.
    fn show_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, Id::unique("gg dock view"), Sense::CLICK | Sense::DRAG);
        ui.painter().add(PaintCallback {
            rect,
            callback: self.callback.clone(),
        });
        self.spot = Some(Spot {
            rect,
            layer: ui.layer_id(),
            response,
        });
    }

    fn start_drag(
        &mut self,
        ctx: &Context,
        from: NodeId,
        (what, grab): (Dragging, Vec2),
        size: Vec2,
    ) {
        // egui keeps the pointer while its widget is dragged, hiding the drag from the layers. The
        // tab may move to another group or window, so a dock widget in the window the drag started
        // in holds it instead.
        ctx.set_dragged_id(drag_id());
        self.drag = Some(Drag {
            source: ctx.viewport_id(),
            from,
            what,
            grab,
            size: size.max(MIN_FLOATING),
        });
    }

    fn splitters(&mut self, root: &mut egui::Ui) {
        let mut stopped = false;
        for splitter in &self.arrangement.splitters {
            let id = Id::unique(("gg dock splitter", splitter.split));
            let response = root.interact(splitter.handle, id, Sense::drag());
            let (icon, i) = match splitter.axis {
                Axis::Horizontal => (CursorIcon::ResizeHorizontal, 0),
                Axis::Vertical => (CursorIcon::ResizeVertical, 1),
            };
            let widgets = &root.visuals().widgets;
            let stroke = if response.hovered() || response.dragged() {
                root.ctx().set_cursor_icon(icon);
                widgets.active.bg_stroke
            } else {
                widgets.noninteractive.bg_stroke
            };
            let (mut from, mut to) = (splitter.rect.min, splitter.rect.max);
            let middle = splitter.handle.center()[i];
            (from[i], to[i]) = (middle, middle);
            root.painter().line_segment([from, to], stroke);
            // egui drags a drag-only widget from the press, so a click is a drag too: the split
            // keeps its place under the pointer wherever the handle is held.
            if response.drag_started()
                && let Some(press) = root.ctx().input(|i| i.pointer.press_origin())
            {
                self.splitter_grab = press[i] - splitter.at;
            }
            if response.dragged()
                && let Some(pointer) = response.interact_pointer_pos()
                && let Some(ratio) = splitter.ratio_at(pointer[i] - self.splitter_grab)
            {
                self.layout.set_ratio(splitter.split, ratio);
            }
            stopped |= response.drag_stopped();
        }
        if stopped {
            self.save();
        }
    }

    /// Moves what's dragged with the pointer: a tab leaves its bar for a floating group, over the
    /// main window while the pointer's over it, else in a window of its own. On release, or when
    /// the source window loses focus, the drag drops what it carries into the zone under the
    /// pointer, if any.
    fn update_drag(&mut self, ctx: &Context) {
        self.hover = None;
        let Some(drag) = &mut self.drag else {
            return;
        };
        let (local, moved, down, focused) = ctx.input_for(drag.source, |i| {
            let pointer = &i.pointer;
            let moved = pointer.delta() != Vec2::ZERO;
            (
                pointer.latest_pos(),
                moved,
                pointer.primary_down(),
                i.focused,
            )
        });
        let pointer = local.and_then(|local| to_main(ctx, drag.source, local));
        let area = self.arrangement.area;
        if let (Some(local), Some(pointer)) = (local, pointer) {
            if let Dragging::Tab { panel, bar } = &drag.what
                && !bar.expand(DETACH).contains(local)
            {
                let place = place_at(ctx, area, pointer, drag.grab, Vec2::ZERO, drag.size);
                drag.what = Dragging::Group(self.layout.detach(panel, place));
            } else if let Dragging::Group(group) = drag.what
                && let Some(floating) = self.layout.floating_mut(group)
            {
                let size = floating.place.size();
                if drag.source == window_id(group) {
                    // A window moved by its own bar stays one, over the main window too. It keeps
                    // its place under the pointer in its own pixels, whatever its monitor's
                    // scale, and moves only on a new pointer sample: an old one predates its
                    // last move.
                    if moved && let Some(position) = follow(ctx, group, local.to_vec2() - drag.grab)
                    {
                        floating.place = Place::Window {
                            position,
                            size: size.into(),
                        };
                    }
                } else {
                    let frame = window_frame(ctx, group);
                    floating.place = place_at(ctx, area, pointer, drag.grab, frame, size);
                    // A move saves it where the user put it; a bare press on its bar doesn't.
                    if moved {
                        floating.kept = None;
                    }
                }
            }
            if let Dragging::Group(group) = drag.what {
                let shown = &self.shown;
                let shown = |panel: &str| shown.contains(panel);
                self.hover = hover(&self.layout, &self.arrangement, ctx, pointer, group, &shown);
            }
        }
        if down && focused {
            return;
        }
        if let Some(Drag {
            what: Dragging::Group(group),
            ..
        }) = self.drag
            && let Some(Hover {
                node,
                zone: Some(zone),
                ..
            }) = self.hover
        {
            let panel = (self.layout.group(group)).and_then(|tabs| tabs.panels.first().cloned());
            if self.layout.dock(group, node, zone)
                && let Some(panel) = panel
            {
                self.reveal(&panel);
            }
        }
        self.layout.prune();
        self.drag = None;
        self.hover = None;
        self.save();
    }

    /// Tracks windows of their own where the user put them; a closed one's group floats over the
    /// main window.
    fn follow_windows(&mut self, ctx: &Context) {
        let dragged = match &self.drag {
            Some(Drag {
                what: Dragging::Group(group),
                ..
            }) => Some(*group),
            _ => None,
        };
        let zoom = ctx.zoom_factor();
        for floating in &mut self.layout.floating {
            let Place::Window { size, .. } = floating.place else {
                continue;
            };
            let viewport = window_id(floating.tabs.id);
            let info = ctx.input(|i| {
                let info = i.raw.viewports.get(&viewport)?;
                Some((
                    info.close_requested(),
                    info.outer_rect,
                    info.inner_rect,
                    info.native_pixels_per_point,
                ))
            });
            let Some((close, outer, inner, native)) = info else {
                continue;
            };
            if close {
                let min = outer
                    .zip(native)
                    .and_then(|(outer, native)| {
                        from_desktop(ctx, rounded(outer.min * native * zoom))
                    })
                    .unwrap_or(pos2(48.0, 48.0));
                floating.place = Place::Main {
                    min: min.into(),
                    size,
                };
            } else if Some(floating.tabs.id) != dragged
                && let (Some(outer), Some(inner), Some(native)) = (outer, inner, native)
            {
                floating.place = Place::Window {
                    position: rounded(outer.min * native * zoom),
                    size: inner.size().into(),
                };
            }
        }
    }
}

impl ViewMenu {
    /// Draws the dock's items into the View menu's `ui`: a checkbox per `known` panel but a
    /// dockspace's view, alphabetically, checked while it shows; it opens the panel, showing its
    /// side if hidden, or closes it. Then the items that save the layout under a name, and load
    /// and delete saved ones, which need `saves`: a file to save to; then the reset.
    fn ui(&mut self, ui: &mut egui::Ui, known: &[String], layout: &Layout, saves: bool) {
        let panels = known
            .iter()
            .filter(|panel| Some(panel.as_str()) != layout.view);
        let mut panels: Vec<&String> = panels.collect();
        panels.sort_by_cached_key(|panel| (panel.to_lowercase(), *panel));
        for &panel in &panels {
            let mut open = !layout.is_closed(panel) && !layout.is_hidden(panel);
            if ui.checkbox(&mut open, panel.as_str()).changed() {
                let panel = panel.clone();
                let choice = if open {
                    Choice::Open(panel)
                } else {
                    Choice::Close(panel)
                };
                self.chosen.push(choice);
            }
        }
        if !panels.is_empty() {
            ui.separator();
        }
        ui.add_enabled_ui(saves, |ui| {
            // A click in its field leaves it open.
            let config = MenuConfig::new()
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .style(crate::menu_style);
            let save = SubMenuButton::new("Save layout as…").config(config);
            save.ui(ui, |ui| self.save_ui(ui));
            ui.add_enabled_ui(!self.saved.is_empty(), |ui| {
                ui.menu_button("Load layout", |ui| {
                    for name in &self.saved {
                        if ui.button(name.as_str()).clicked() {
                            self.chosen.push(Choice::Load(name.clone()));
                        }
                    }
                });
                ui.menu_button("Delete layout", |ui| {
                    for name in &self.saved {
                        if ui.button(name.as_str()).clicked() {
                            self.chosen.push(Choice::Delete(name.clone()));
                        }
                    }
                });
            });
        });
        if ui.button("Reset layout").clicked() {
            self.chosen.push(Choice::Reset);
        }
    }

    /// The submenu that saves the layout under the name typed in its field, on Save or Enter, and
    /// says why it refuses a name.
    fn save_ui(&mut self, ui: &mut egui::Ui) {
        let field = ui.add(TextEdit::singleline(&mut self.name).hint_text("Name"));
        if field.changed() {
            self.refused = None;
        }
        if let Some(refused) = &self.refused {
            ui.colored_label(ui.visuals().error_fg_color, refused.as_str());
        }
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if !(ui.button("Save").clicked() || entered) {
            return;
        }
        match file::name(&self.name).map(str::to_owned) {
            Ok(name) => {
                self.chosen.push(Choice::Save(name));
                self.name.clear();
                ui.close();
            }
            Err(error) => {
                warn!(code = error.code(), name = %self.name, "{error}");
                self.refused = Some(error.to_string());
            }
        }
    }
}

/// The names of the layouts saved beside `path`; none, with a warning, if unlistable.
fn saved(path: &Path) -> Vec<String> {
    file::names(path).unwrap_or_else(|error| {
        warn!(code = error.code(), "{error:#}");
        Vec::new()
    })
}

fn drag_id() -> Id {
    Id::unique("gg dock drag")
}

fn side_of(sidebar: Sidebar) -> Side {
    match sidebar {
        Sidebar::Left => Side::Left,
        Sidebar::Bottom => Side::Bottom,
        Sidebar::Right => Side::Right,
    }
}

fn window_id(id: NodeId) -> ViewportId {
    ViewportId::from_hash_of(("gg dock window", id))
}

/// `viewport`'s inner rect on the desktop in its points, and its pixels per point, if egui knows
/// them.
fn geometry(ctx: &Context, viewport: ViewportId) -> Option<(Rect, f32)> {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let info = i.raw.viewports.get(&viewport)?;
        Some((info.inner_rect?, info.native_pixels_per_point? * zoom))
    })
}

/// `pos` in viewport `from`'s points, as the main window's, through their desktop places.
fn to_main(ctx: &Context, from: ViewportId, pos: Pos2) -> Option<Pos2> {
    if from == ViewportId::ROOT {
        return Some(pos);
    }
    let (inner, ppp) = geometry(ctx, from)?;
    from_desktop(ctx, rounded((inner.min + pos.to_vec2()) * ppp))
}

/// `rect` in viewport `from`'s points, as `to`'s, through their desktop places.
fn between(ctx: &Context, rect: Rect, from: ViewportId, to: ViewportId) -> Option<Rect> {
    let (from, from_ppp) = geometry(ctx, from)?;
    let (to, to_ppp) = geometry(ctx, to)?;
    let point = |pos: Pos2| (from.min + pos.to_vec2()) * (from_ppp / to_ppp) - to.min.to_vec2();
    Some(Rect::from_min_max(point(rect.min), point(rect.max)))
}

/// Where floating group `id`'s window goes moved `by` its own points, on the desktop in physical
/// pixels.
fn follow(ctx: &Context, id: NodeId, by: Vec2) -> Option<[i32; 2]> {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let info = i.raw.viewports.get(&window_id(id))?;
        let px = (info.outer_rect?.min + by) * info.native_pixels_per_point? * zoom;
        Some(rounded(px))
    })
}

/// How far floating group `id`'s window's inside is from its outer corner, past title bar and
/// border, in the main window's points; zero until the window opens.
fn window_frame(ctx: &Context, id: NodeId) -> Vec2 {
    ctx.input(|i| {
        let info = i.raw.viewports.get(&window_id(id))?;
        let main = i.raw.viewports.get(&ViewportId::ROOT)?;
        let px = (info.inner_rect?.min - info.outer_rect?.min) * info.native_pixels_per_point?;
        Some(px / main.native_pixels_per_point?)
    })
    .unwrap_or(Vec2::ZERO)
}

fn from_desktop(ctx: &Context, [x, y]: [i32; 2]) -> Option<Pos2> {
    let (main, ppp) = geometry(ctx, ViewportId::ROOT)?;
    Some(pos2(
        x as f32 / ppp - main.min.x,
        y as f32 / ppp - main.min.y,
    ))
}

fn to_desktop(ctx: &Context, pos: Pos2) -> Option<[i32; 2]> {
    let (main, ppp) = geometry(ctx, ViewportId::ROOT)?;
    Some(rounded((main.min + pos.to_vec2()) * ppp))
}

fn rounded(px: Pos2) -> [i32; 2] {
    [px.x.round() as i32, px.y.round() as i32]
}

/// Where a group held at `grab` by the pointer at `pointer`, in the main window's points, goes:
/// over the main window while the pointer is over the dock's `area`, else in a window of its own
/// where egui opens them, its inside `frame` from its outer corner. The next frame floats a
/// dockspace's view group back over the main window before it draws.
fn place_at(
    ctx: &Context,
    area: Rect,
    pointer: Pos2,
    grab: Vec2,
    frame: Vec2,
    size: Vec2,
) -> Place {
    let min = pointer - grab;
    if !ctx.embed_viewports()
        && !area.contains(pointer)
        && let Some(position) = to_desktop(ctx, min - frame)
    {
        return Place::Window {
            position,
            size: size.into(),
        };
    }
    Place::Main {
        min: min.into(),
        size: size.into(),
    }
}

/// What group `dragged`, at `pointer` in the main window's points, is over: windows of their own,
/// above the main window, then groups floating over it, then docked groups and the central area.
/// A floating group with no panel shown (`shown`) shows nowhere, so it's none of them. A
/// dockspace's view group stays over the main window: it's over no window of its own.
fn hover(
    layout: &Layout,
    arrangement: &Arrangement,
    ctx: &Context,
    pointer: Pos2,
    dragged: NodeId,
    shown: &impl Fn(&str) -> bool,
) -> Option<Hover> {
    let at = |node, rect: Rect, window, zones, pos| Hover {
        node,
        rect,
        window,
        zones,
        zone: targets(rect, zones)
            .find(|(_, target)| target.contains(pos))
            .map(|(zone, _)| zone),
        share: layout.share(node),
    };
    let others = || {
        let floating = layout.floating.iter().rev();
        floating.filter(|f| f.tabs.id != dragged && f.tabs.showing(shown).is_some())
    };
    if !layout.holds_view(dragged)
        && let Some(px) = to_desktop(ctx, pointer)
    {
        for floating in others() {
            let id = floating.tabs.id;
            let Some((inner, ppp)) = geometry(ctx, window_id(id)) else {
                continue;
            };
            let pos = pos2(px[0] as f32 / ppp, px[1] as f32 / ppp) - inner.min.to_vec2();
            let rect = Rect::from_min_size(Pos2::ZERO, inner.size());
            if matches!(floating.place, Place::Window { .. }) && rect.contains(pos) {
                return Some(at(id, rect, Some(id), Zones::Center, pos));
            }
        }
    }
    if !arrangement.area.contains(pointer) {
        return None;
    }
    for floating in others() {
        if let Place::Main { min, size } = floating.place {
            let rect = Rect::from_min_size(min.into(), size.into());
            if rect.contains(pointer) {
                return Some(at(floating.tabs.id, rect, None, Zones::Center, pointer));
            }
        }
    }
    if let Some(&(id, rect)) = arrangement
        .groups
        .iter()
        .find(|(_, rect)| rect.contains(pointer))
    {
        return Some(at(id, rect, None, Zones::All, pointer));
    }
    // A dockspace's is the empty dock, filled by a group.
    let zones = match layout.view {
        Some(_) => Zones::Center,
        None => Zones::Sides,
    };
    let central = arrangement.central;
    central
        .contains(pointer)
        .then(|| at(CENTRAL, central, None, zones, pointer))
}

/// The drop targets over `rect`: a cross of squares at its center, the middle for its tabs, one
/// toward each side.
fn targets(rect: Rect, zones: Zones) -> impl Iterator<Item = (Zone, Rect)> {
    let step = TARGET + 4.0;
    [
        (Zone::Center, vec2(0.0, 0.0)),
        (Zone::Side(Side::Left), vec2(-step, 0.0)),
        (Zone::Side(Side::Right), vec2(step, 0.0)),
        (Zone::Side(Side::Top), vec2(0.0, -step)),
        (Zone::Side(Side::Bottom), vec2(0.0, step)),
    ]
    .into_iter()
    .filter(move |&(zone, _)| match zones {
        Zones::All => true,
        Zones::Sides => zone != Zone::Center,
        Zones::Center => zone == Zone::Center,
    })
    .map(move |(zone, offset)| {
        let target = Rect::from_center_size(rect.center() + offset, Vec2::splat(TARGET));
        (zone, target)
    })
}

/// The part of `rect` a group dropped in `zone` takes: all of it among the tabs, else `share` of
/// it on that side.
fn preview(rect: Rect, zone: Zone, share: f32) -> Rect {
    let (width, height) = (rect.width() * share, rect.height() * share);
    match zone {
        Zone::Center => rect,
        Zone::Side(Side::Left) => rect.with_max_x(rect.min.x + width),
        Zone::Side(Side::Right) => rect.with_min_x(rect.max.x - width),
        Zone::Side(Side::Top) => rect.with_max_y(rect.min.y + height),
        Zone::Side(Side::Bottom) => rect.with_min_y(rect.max.y - height),
    }
}

fn paint_hover(ctx: &Context, hover: &Hover) {
    let painter: Painter = ctx.layer_painter(LayerId::new(
        Order::Foreground,
        Id::unique("gg dock targets"),
    ));
    let visuals = ctx.global_style().visuals.clone();
    let selection = visuals.selection.bg_fill;
    if let Some(zone) = hover.zone {
        let rect = preview(hover.rect, zone, hover.share);
        painter.rect_filled(rect, 0.0, selection.gamma_multiply(0.35));
    }
    for (zone, target) in targets(hover.rect, hover.zones) {
        let fill = if hover.zone == Some(zone) {
            selection
        } else {
            visuals.window_fill
        };
        let stroke = visuals.window_stroke;
        painter.rect(target, 4.0, fill, stroke, StrokeKind::Inside);
        let icon = preview(target.shrink(7.0), zone, 0.5);
        painter.rect_filled(icon, 1.0, visuals.widgets.active.fg_stroke.color);
    }
}

#[cfg(test)]
mod tests;
