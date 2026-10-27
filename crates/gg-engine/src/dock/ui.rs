//! The dock in a frame: the strips it takes off the main window's edges, the groups it draws as
//! layers show their panels, in a dockspace the panel that shows the game's view, and the drags
//! that dock, float and tear off panels.

use std::any::Any;
use std::collections::HashSet;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{
    Align, Area, Button, CentralPanel, Context, CursorIcon, Frame, Id, LayerId, Order,
    PaintCallback, Painter, Panel, Pos2, Rect, Sense, StrokeKind, UiBuilder, Vec2, ViewportBuilder,
    ViewportClass, ViewportCommand, ViewportId, pos2, vec2,
};
use gg_renderer::ViewCallback;
use tracing::warn;

use super::file;
use super::tree::{Arrangement, Axis, CENTRAL, Layout, NodeId, Place, Side, Zone};
#[cfg(target_os = "windows")]
use crate::chrome::Caption;
use crate::chrome::{self, Chrome, Icon, Look, TitleBar};

/// The size of a new floating group, in points.
const NEW_SIZE: Vec2 = vec2(320.0, 240.0);
/// The smallest a floating group gets, in points.
const MIN_FLOATING: Vec2 = vec2(120.0, 80.0);
/// How far a dragged tab goes past its tab bar before it leaves its group, in points.
const DETACH: f32 = 8.0;
/// Where a group's first tab starts, from the left of its tab bar, in points.
const INSET: f32 = 4.0;
/// The side of a drop target, in points.
const TARGET: f32 = 32.0;
/// The side of a floating group's resize grip, in points.
const GRIP: f32 = 12.0;

/// What layers draw their UI through, in [`Layer::ui`](crate::Layer::ui): panels, which the user
/// docks, and egui itself for everything else.
pub struct Ui<'a> {
    root: &'a mut egui::Ui,
    dock: &'a mut Dock,
    /// The type of the last layer whose `ui` was [`Layer::ui`](crate::Layer::ui)'s default, which
    /// draws none, since the stack last asked.
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

    /// The type of the last layer whose `ui` was the default, since this was last asked.
    pub(crate) fn take_default_ui(&mut self) -> Option<&'static str> {
        self.default_ui.take()
    }

    /// Shows panel `name` this frame, with what `add_contents` draws in it. The user places
    /// panels: in tab groups docked around the central area, or, with
    /// [`App::view_panel`](crate::App::view_panel), filling the main window beside the panel that
    /// shows the game's view; or floating, over the main window or in windows of their own. A
    /// panel shows from the frame after its layer starts showing it, since the dock lays out a
    /// frame before its panels draw, and keeps its place while it isn't shown; `add_contents` runs
    /// when its tab is the one showing. `name` titles the panel and is its identity: the dock
    /// keeps panels' places by name, between runs too, and a second panel of the same name in a
    /// frame doesn't show.
    pub fn panel(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        self.dock.panel(self.root, name, add_contents);
    }

    /// Adds menu `name` to the main window's menu bar this frame, with what `add_contents` draws
    /// in it while it's open. Menus come in the order layers add them, bottom layer first, and a
    /// second menu of the same name in a frame doesn't show. With
    /// [`App::custom_title_bar`](crate::App::custom_title_bar) the menu bar is the title bar;
    /// otherwise it's a bar of its own below the OS's, from the frame after a menu first shows.
    ///
    /// A Quit item closes the main window as the OS's close button does, which the layers get
    /// as [`Event::CloseRequested`](crate::Event::CloseRequested) first:
    /// `ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close)`.
    pub fn menu(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        self.dock.bar.menu(name, add_contents);
    }

    /// egui, for UI that isn't a panel.
    pub fn egui(&self) -> &egui::Context {
        self.root.ctx()
    }
}

/// The dock's layout, and what it needs from one frame to the next.
pub(crate) struct Dock {
    layout: Layout,
    /// Where the layout is saved; `None` keeps it in memory only, as headless runs do.
    file: Option<PathBuf>,
    /// The main window's menu bar, which is its title bar when the UI draws the frame.
    bar: TitleBar,
    /// Who draws the frames of the windows of their own.
    chrome: Chrome,
    /// The panels shown last frame, which tab bars list, and those shown this frame so far.
    shown: HashSet<String>,
    showing: HashSet<String>,
    arrangement: Arrangement,
    /// The groups drawn this frame.
    drawn: HashSet<NodeId>,
    /// Panels shown for the first time this frame, which float from the next.
    new: Vec<String>,
    /// A tab clicked this frame, whose panel shows from the next, so that the layout holds
    /// still while panels draw.
    clicked: Option<String>,
    drag: Option<Drag>,
    /// What a dragged group is over, as of the end of the last frame.
    hover: Option<Hover>,
    /// Where the pointer holds the splitter it drags, from the split, in points.
    splitter_grab: f32,
    /// Where a dockspace's view shows this frame, once its panel draws.
    spot: Option<Spot>,
    /// The renderer's callback that draws the view in its panel.
    callback: Arc<dyn Any + Send + Sync>,
}

/// Where the game's view shows in a dockspace.
#[derive(Clone, Copy)]
struct Spot {
    /// In the main window's points.
    rect: Rect,
    /// The background's when its group is docked, or its group's area's when it floats.
    layer: LayerId,
}

struct Drag {
    /// The window the drag started in, whose pointer it follows: it keeps the pointer until the
    /// button is let go.
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
    /// The window of its own the node is in; `None` for the main window.
    window: Option<NodeId>,
    /// The zones it offers, and the one under the pointer.
    zones: Zones,
    zone: Option<Zone>,
    /// The share of its rect that a group docked beside it takes.
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
    /// A dock with the layout saved in `file`, or kept in memory only without one, in windows
    /// framed as `chrome` says: with a `view`, a dockspace, whose panel of that name shows the
    /// game's view. A file that can't be read logs a warning, and the dock starts as new.
    pub(crate) fn new(file: Option<PathBuf>, chrome: Chrome, view: Option<&'static str>) -> Self {
        let layout = match &file {
            Some(path) => file::load(path, view).unwrap_or_else(|error| {
                warn!(code = error.code(), path = %path.display(), "{error:#}; the dock starts as new");
                Layout::new(view)
            }),
            None => Layout::new(view),
        };
        let arrangement = layout.arrange(Rect::NOTHING, &|_| false);
        Self {
            layout,
            file,
            bar: TitleBar::new(chrome),
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
            callback: Arc::new(ViewCallback),
        }
    }

    /// Saves the layout to its file, if it has one. A failure logs a warning.
    pub(crate) fn save(&self) {
        let _zone = gg_core::zone!();
        if let Some(path) = &self.file
            && let Err(error) = file::save(path, &self.layout.saved())
        {
            warn!(code = error.code(), path = %path.display(), "{error:#}");
        }
    }

    /// Whether the dock fills the main window, with the game's view in a panel of its own.
    pub(crate) fn is_dockspace(&self) -> bool {
        self.layout.view.is_some()
    }

    /// Where the game's view shows this frame, in the main window's points: the central area, or,
    /// in a dockspace, its panel's inside once it draws, and nothing while it doesn't.
    pub(crate) fn view(&self) -> Rect {
        match self.layout.view {
            Some(_) => self.spot.map_or(Rect::NOTHING, |spot| spot.rect),
            None => self.arrangement.central,
        }
    }

    /// Whether `pos`, in the main window's points, is on a dockspace's view as the last frame
    /// drew it: not under a group or a menu over it, nor on the resize grip of the group it floats
    /// in, which lies over its bottom-right corner.
    pub(crate) fn over_view(&self, egui: &Context, pos: Pos2) -> bool {
        let Some(spot) = self.spot else {
            return false;
        };
        let floats = spot.layer.order != Order::Background;
        let grip = Rect::from_min_max(spot.rect.max - Vec2::splat(GRIP), spot.rect.max);
        spot.rect.contains(pos)
            && egui.layer_id_at(pos) == Some(spot.layer)
            && !(floats && grip.contains(pos))
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

    /// How the title bar's maximize button looks from the next frame, as the OS says.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_maximize(&mut self, look: Look) {
        self.bar.set_maximize(look);
    }

    /// Starts a frame: takes the title bar off the top of `root`, then the strips along the main
    /// window's edges that docked groups fill, leaving it the central area. egui counts the bar
    /// and the strips as its own, and the central area as the game's. A dockspace has no central
    /// area: egui counts the whole dock as its own, and the panel that shows the game's view draws
    /// first.
    pub(crate) fn begin(&mut self, root: &mut egui::Ui) {
        self.bar.begin(root);
        let view = self.layout.view;
        let embedded = root.ctx().embed_viewports();
        for floating in &mut self.layout.floating {
            // Without windows of their own, floating groups float over the main window, and the
            // layout keeps their windows' places for runs with them. The view's group never has
            // one, since only the main window's input reaches the layers.
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
        if !self.showing.insert(name.to_owned()) {
            return;
        }
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

    /// Ends a frame: finishes the title bar, draws what the panels didn't, moves the drag along,
    /// and makes the frame's changes to the layout.
    pub(crate) fn end(&mut self, root: &mut egui::Ui) {
        self.bar.end();
        let ctx = root.ctx().clone();
        // The tab bars of groups whose showing panel wasn't shown, so that the user can pick
        // another, and the window a drag started in, which the drag's pointer comes through.
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
        for panel in mem::take(&mut self.new) {
            let n = self.layout.floating.len() % 8;
            let at = 48.0 + 24.0 * n as f32;
            let place = Place::Main {
                min: [at, at],
                size: NEW_SIZE.into(),
            };
            self.layout.float(panel, place);
        }
        mem::swap(&mut self.shown, &mut self.showing);
        self.showing.clear();
    }

    /// Draws group `id` where it is, with `add_contents` drawing the panel showing.
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
        // A docked group none of whose panels showed last frame takes no room until the next.
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

    /// Draws a floating group over the main window, with a grip to resize it.
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

    /// Draws a floating group in a window of its own, at `position` on the desktop in physical
    /// pixels, `size` points inside.
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
        // egui places windows in their own points; one that isn't open yet, in the main
        // window's.
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
            // Inactive, so that the window a drag started in keeps the pointer.
            .with_active(false)
            // Off the taskbar, a minimized window would be hard to find again.
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
        // egui moves or sizes the window whenever these change, which would fight the user
        // moving it: they're set as it opens, and its position while the dock drags it.
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
            // The UI draws the frame of a window of its own, whose tab bar is its title bar, but
            // not of one shown in the main window, whose closing and resizing it would take.
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
            // The targets of what the dragged group is over, and, as in Dear ImGui, the same in
            // the dragged group's window, which would hide them.
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

    /// Draws a group into `ui`, filling it: its tab bar, with a tab for each of its panels shown
    /// last frame, then what `add_contents` draws of the one showing, or a dockspace's view.
    /// Clicking a tab shows its panel from the next frame, and dragging a tab starts a drag, as
    /// does dragging the bar of a group that moves by it (`movable`): one floating over the main
    /// window, or one whose bar is its window's title bar, which then has a button that closes the
    /// window (`close`).
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
        let single = tabs.panels.len() == 1;
        let floating = self.layout.floating(id).is_some();
        let mut clicked = None;
        let mut dragged = None;
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
            if response.drag_started() {
                dragged = Some((panel.clone(), response.rect));
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
                // In the group a tab comes out into, it's the first: the pointer keeps its place
                // on it.
                let grab = vec2(INSET + press.x - tab.min.x, press.y - bar.min.y);
                (Dragging::Tab { panel, bar }, grab)
            };
            let size = if floating { rect.size() } else { NEW_SIZE };
            self.start_drag(ctx, id, drag, size);
        } else if background.is_some_and(|response| response.drag_started()) {
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
    }

    /// Draws a dockspace's view into `ui`, its panel's inside, filling it, through the renderer's
    /// callback, and notes where it is. It senses presses itself, so that egui's hit test takes
    /// one there as the view's, rather than a tab's or a splitter's within a few points of it, and
    /// it can't take egui's focus, so that Tab never lands on it.
    fn show_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.interact(rect, Id::unique("gg dock view"), Sense::CLICK | Sense::DRAG);
        ui.painter().add(PaintCallback {
            rect,
            callback: self.callback.clone(),
        });
        self.spot = Some(Spot {
            rect,
            layer: ui.layer_id(),
        });
    }

    fn start_drag(
        &mut self,
        ctx: &Context,
        from: NodeId,
        (what, grab): (Dragging, Vec2),
        size: Vec2,
    ) {
        // egui keeps the pointer while one of its widgets is dragged, so the layers don't see
        // the drag. The tab may move to another group or window, so a widget of the dock's own
        // holds the drag instead, in the window it started in.
        ctx.set_dragged_id(drag_id());
        self.drag = Some(Drag {
            source: ctx.viewport_id(),
            from,
            what,
            grab,
            size: size.max(MIN_FLOATING),
        });
    }

    /// The handles between the sides of the main tree's splits.
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
            // egui drags a widget that senses only drags from the press, so a click is a drag
            // too: the split keeps its place under the pointer, wherever the handle is held.
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

    /// Moves what's dragged with the pointer: a tab comes out of its bar into a floating group,
    /// which floats over the main window while the pointer's over it, and otherwise in a window
    /// of its own. When the button's let go, or the source window loses the focus, the drag
    /// drops what it carries where it is: into the zone under the pointer, if any.
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
                    // A window moved by its own bar stays one, over the main window too, since
                    // the pointer comes through it. It keeps its place under the pointer in its
                    // own pixels, whatever its monitor's scale, and moves only on a new pointer
                    // sample: an old one is from before its last move.
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
                    // Moved, it's saved where the user put it; a press on its bar alone doesn't
                    // move it.
                    if moved {
                        floating.kept = None;
                    }
                }
            }
            if let Dragging::Group(group) = drag.what {
                self.hover = hover(&self.layout, &self.arrangement, ctx, pointer, group);
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
            self.layout.dock(group, node, zone);
        }
        self.layout.prune();
        self.drag = None;
        self.hover = None;
        self.save();
    }

    /// Keeps the windows of their own where the user put them, and floats a closed one's group
    /// over the main window instead.
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

/// The widget that holds egui's drag while the dock drags.
fn drag_id() -> Id {
    Id::unique("gg dock drag")
}

/// The viewport of floating group `id`'s window.
fn window_id(id: NodeId) -> ViewportId {
    ViewportId::from_hash_of(("gg dock window", id))
}

/// Viewport `viewport`'s inner rect on the desktop, in its points, and its pixels per point,
/// if egui knows them.
fn geometry(ctx: &Context, viewport: ViewportId) -> Option<(Rect, f32)> {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let info = i.raw.viewports.get(&viewport)?;
        Some((info.inner_rect?, info.native_pixels_per_point? * zoom))
    })
}

/// `pos`, in viewport `from`'s points, in the main window's: for a window of its own, through
/// where the two are on the desktop.
fn to_main(ctx: &Context, from: ViewportId, pos: Pos2) -> Option<Pos2> {
    if from == ViewportId::ROOT {
        return Some(pos);
    }
    let (inner, ppp) = geometry(ctx, from)?;
    from_desktop(ctx, rounded((inner.min + pos.to_vec2()) * ppp))
}

/// `rect`, in viewport `from`'s points, in viewport `to`'s: through where the two are on the
/// desktop.
fn between(ctx: &Context, rect: Rect, from: ViewportId, to: ViewportId) -> Option<Rect> {
    let (from, from_ppp) = geometry(ctx, from)?;
    let (to, to_ppp) = geometry(ctx, to)?;
    let point = |pos: Pos2| (from.min + pos.to_vec2()) * (from_ppp / to_ppp) - to.min.to_vec2();
    Some(Rect::from_min_max(point(rect.min), point(rect.max)))
}

/// Where floating group `id`'s window goes, on the desktop in physical pixels, moved `by` its
/// own points.
fn follow(ctx: &Context, id: NodeId, by: Vec2) -> Option<[i32; 2]> {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let info = i.raw.viewports.get(&window_id(id))?;
        let px = (info.outer_rect?.min + by) * info.native_pixels_per_point? * zoom;
        Some(rounded(px))
    })
}

/// How far the inside of floating group `id`'s window is from its outside corner, past its
/// title bar and border, in the main window's points; zero until the window is open.
fn window_frame(ctx: &Context, id: NodeId) -> Vec2 {
    ctx.input(|i| {
        let info = i.raw.viewports.get(&window_id(id))?;
        let main = i.raw.viewports.get(&ViewportId::ROOT)?;
        let px = (info.inner_rect?.min - info.outer_rect?.min) * info.native_pixels_per_point?;
        Some(px / main.native_pixels_per_point?)
    })
    .unwrap_or(Vec2::ZERO)
}

/// A desktop position, in physical pixels, in the main window's points.
fn from_desktop(ctx: &Context, [x, y]: [i32; 2]) -> Option<Pos2> {
    let (main, ppp) = geometry(ctx, ViewportId::ROOT)?;
    Some(pos2(
        x as f32 / ppp - main.min.x,
        y as f32 / ppp - main.min.y,
    ))
}

/// A position in the main window's points on the desktop, in physical pixels.
fn to_desktop(ctx: &Context, pos: Pos2) -> Option<[i32; 2]> {
    let (main, ppp) = geometry(ctx, ViewportId::ROOT)?;
    Some(rounded((main.min + pos.to_vec2()) * ppp))
}

/// A position on the desktop in physical pixels, to the nearest pixel.
fn rounded(px: Pos2) -> [i32; 2] {
    [px.x.round() as i32, px.y.round() as i32]
}

/// Where a group held at `grab` by the pointer at `pointer`, in the main window's points, goes:
/// over the main window while the pointer is over the dock's `area`, and otherwise in a window
/// of its own, where egui opens them, whose inside is `frame` from its outside corner. The next
/// frame floats a dockspace's view's group back over the main window, before it draws.
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

/// What group `dragged`, at `pointer` in the main window's points, is over: the windows of their
/// own, above the main window, then the groups floating over it, then the docked groups and
/// the central area. The group of a dockspace's view stays over the main window, so it's over
/// no window of its own.
fn hover(
    layout: &Layout,
    arrangement: &Arrangement,
    ctx: &Context,
    pointer: Pos2,
    dragged: NodeId,
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
        layout
            .floating
            .iter()
            .rev()
            .filter(|f| f.tabs.id != dragged)
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
    // A dockspace's is the empty dock, which a group fills.
    let zones = match layout.view {
        Some(_) => Zones::Center,
        None => Zones::Sides,
    };
    let central = arrangement.central;
    central
        .contains(pointer)
        .then(|| at(CENTRAL, central, None, zones, pointer))
}

/// The drop targets over `rect`: a cross of squares at its center, the middle one for its tabs
/// and one toward each side.
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

/// The part of `rect` that a group dropped in `zone` takes: all of it among the tabs, and
/// otherwise `share` of it on that side.
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

/// Draws the drop targets over what a dragged group is over, above everything else in its
/// window, and where the group would go.
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
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui::{
        Event, ImmediateViewport, Modifiers, PointerButton, RawInput, ResizeDirection,
        ViewportEvent, ViewportIdMap, ViewportInfo,
    };

    use super::super::tree::{Node, Tabs};
    use super::*;
    use crate::chrome::BAR;

    const SCREEN: Vec2 = vec2(800.0, 600.0);

    /// egui and a dock, run a frame at a time on made-up input, with panels A and B showing.
    struct Run {
        ctx: Context,
        dock: Dock,
        frame: u32,
        pointer: Pos2,
        /// Where the windows are on the desktop, for a run with windows of their own.
        viewports: Vec<(ViewportId, ViewportInfo)>,
        /// What the passes of windows of their own get, when they run as the engine runs them.
        passes: Option<Rc<RefCell<Passes>>>,
        /// What egui asked of each window in the last frame.
        commands: Vec<(ViewportId, ViewportCommand)>,
    }

    /// What each window's next pass gets: where the windows are, and its events.
    #[derive(Default)]
    struct Passes {
        viewports: ViewportIdMap<ViewportInfo>,
        events: ViewportIdMap<Vec<Event>>,
        time: f64,
    }

    impl Run {
        /// Runs until both panels float, A then B, cascaded from the top-left.
        fn new() -> Self {
            Self::with_chrome(Chrome::Os)
        }

        /// Under the frame `chrome` says.
        fn with_chrome(chrome: Chrome) -> Self {
            Self::with_dock(Dock::new(None, chrome, None))
        }

        /// A dockspace under the frame `chrome` says, whose view, panel [`VIEW`], fills the dock
        /// once it shows, under A and B, floating.
        fn dockspace(chrome: Chrome) -> Self {
            Self::with_dock(Dock::new(None, chrome, Some(VIEW)))
        }

        fn with_dock(dock: Dock) -> Self {
            let mut run = Self {
                ctx: crate::egui_context(),
                dock,
                frame: 0,
                pointer: Pos2::ZERO,
                viewports: Vec::new(),
                passes: None,
                commands: Vec::new(),
            };
            run.frames(2);
            run
        }

        /// A run with windows of their own, as [`Run::open_windows`] opens them.
        fn with_windows(chrome: Chrome) -> Self {
            let mut run = Self::with_chrome(chrome);
            run.open_windows();
            run
        }

        /// Has windows of their own open from the next frame, the main window's inside at
        /// (100, 50) on the desktop, at 1.5 pixels a point. Without egui's callback for them,
        /// which the renderer gives it, they show in the main window.
        fn open_windows(&mut self) {
            self.ctx.set_embed_viewports(false);
            let main = ViewportInfo {
                inner_rect: Some(Rect::from_min_size(pos2(100.0, 50.0), SCREEN)),
                native_pixels_per_point: Some(1.5),
                ..ViewportInfo::default()
            };
            self.viewports.push((ViewportId::ROOT, main));
        }

        /// A run with windows of their own, framed by the UI, whose passes run as
        /// [`Run::run_window_passes`] runs them.
        fn with_window_passes() -> Self {
            let mut run = Self::with_windows(Chrome::Drawn);
            run.run_window_passes();
            run
        }

        /// Has the passes of windows of their own run as the engine runs them: through egui's
        /// callback, each with its own input.
        fn run_window_passes(&mut self) {
            let passes = Rc::new(RefCell::new(Passes::default()));
            let shared = passes.clone();
            // The callback is the test thread's.
            Context::set_immediate_viewport_renderer(move |ctx, viewport| {
                let ImmediateViewport {
                    ids,
                    mut viewport_ui_cb,
                    ..
                } = viewport;
                let input = {
                    let mut passes = shared.borrow_mut();
                    let info = passes.viewports.entry(ids.this).or_default();
                    let size = info.inner_rect.map_or(NEW_SIZE, |inner| inner.size());
                    RawInput {
                        viewport_id: ids.this,
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                        time: Some(passes.time),
                        events: passes.events.remove(&ids.this).unwrap_or_default(),
                        viewports: passes.viewports.clone(),
                        ..RawInput::default()
                    }
                };
                let mut output = ctx.run_ui(input, |ui| viewport_ui_cb(ui));
                output.textures_delta.clear();
            });
            self.passes = Some(passes);
        }

        /// Puts `panel`'s group in a window of its own without a frame, whose inside is at `min`
        /// on the desktop, in its points at 1.25 pixels a point. Returns the window's viewport.
        fn tear_off(&mut self, panel: &str, min: Pos2) -> ViewportId {
            let id = self.layout().find(panel).expect("the panel is in the dock");
            self.dock
                .layout
                .floating_mut(id)
                .expect("the panel floats")
                .place = Place::Window {
                position: rounded(min * 1.25),
                size: NEW_SIZE.into(),
            };
            let rect = Rect::from_min_size(min, NEW_SIZE);
            let info = ViewportInfo {
                outer_rect: Some(rect),
                inner_rect: Some(rect),
                native_pixels_per_point: Some(1.25),
                ..ViewportInfo::default()
            };
            self.viewports.push((window_id(id), info));
            self.frames(2);
            window_id(id)
        }

        fn frame(&mut self, events: Vec<Event>) {
            let time = f64::from(self.frame) / 60.0;
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(time),
                events,
                ..RawInput::default()
            };
            input.viewports.extend(self.viewports.iter().cloned());
            if let Some(passes) = &self.passes {
                let mut passes = passes.borrow_mut();
                passes.viewports = input.viewports.clone();
                passes.time = time;
            }
            self.frame += 1;
            let dock = &mut self.dock;
            let mut output = self.ctx.run_ui(input, |root| {
                dock.begin(root);
                let mut ui = Ui::new(root, dock);
                for name in ["A", "B"] {
                    ui.panel(name, |ui| {
                        ui.label(name);
                    });
                }
                dock.end(root);
            });
            output.textures_delta.clear();
            // As the OS and the engine would: a window moves where egui asks, and a close comes
            // back as an event, which the next frame's passes see.
            self.commands.clear();
            for (id, output) in output.viewport_output {
                if let Some((_, info)) = self.viewports.iter_mut().find(|(window, _)| *window == id)
                {
                    info.events.clear();
                    if let (Some(position), Some(outer), Some(inner)) =
                        (output.builder.position, info.outer_rect, info.inner_rect)
                    {
                        let by = position - outer.min;
                        info.outer_rect = Some(outer.translate(by));
                        info.inner_rect = Some(inner.translate(by));
                    }
                    if output.commands.contains(&ViewportCommand::Close) {
                        info.events.push(ViewportEvent::Close);
                    }
                }
                let commands = output.commands.into_iter();
                self.commands.extend(commands.map(|command| (id, command)));
            }
        }

        /// Sends `events` to window `viewport`'s next pass, and runs a frame.
        fn frame_in(&mut self, viewport: ViewportId, events: Vec<Event>) {
            let passes = self
                .passes
                .as_ref()
                .expect("the windows run their own passes");
            let mut passes = passes.borrow_mut();
            passes.events.entry(viewport).or_default().extend(events);
            drop(passes);
            self.frame(Vec::new());
        }

        /// Window `viewport`'s inside on the desktop, in its points, and its pixels per point.
        fn window(&self, viewport: ViewportId) -> (Rect, f32) {
            let (_, info) = (self.viewports.iter())
                .find(|(window, _)| *window == viewport)
                .expect("the window is open");
            (
                info.inner_rect.unwrap(),
                info.native_pixels_per_point.unwrap(),
            )
        }

        /// Point `px` on the desktop, in pixels, in window `viewport`'s points.
        fn local(&self, viewport: ViewportId, px: Pos2) -> Pos2 {
            let (inner, ppp) = self.window(viewport);
            pos2(px.x / ppp, px.y / ppp) - inner.min.to_vec2()
        }

        /// Moves the pointer to `px` on the desktop, in pixels, through window `viewport`.
        fn move_in(&mut self, viewport: ViewportId, px: Pos2) {
            let pos = self.local(viewport, px);
            self.frame_in(viewport, vec![Event::PointerMoved(pos)]);
        }

        /// Presses or lets go at `px` on the desktop, in pixels, through window `viewport`.
        fn button_in(&mut self, viewport: ViewportId, px: Pos2, pressed: bool) {
            let pos = self.local(viewport, px);
            let button = Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            self.frame_in(viewport, vec![button]);
        }

        /// Clicks at `px` on the desktop, in pixels, through window `viewport`.
        fn click_in(&mut self, viewport: ViewportId, px: Pos2) {
            self.move_in(viewport, px);
            self.button_in(viewport, px, true);
            self.button_in(viewport, px, false);
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(Vec::new());
            }
        }

        fn move_to(&mut self, pos: Pos2) {
            self.pointer = pos;
            self.frame(vec![Event::PointerMoved(pos)]);
        }

        fn button(&mut self, pressed: bool) {
            self.frame(vec![Event::PointerButton {
                pos: self.pointer,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }

        /// Presses at `from` and drags to `to` over a few frames, still holding on.
        fn hold(&mut self, from: Pos2, to: Pos2) {
            self.move_to(from);
            self.button(true);
            for step in 1..=8 {
                self.move_to(from + (to - from) * step as f32 / 8.0);
            }
        }

        /// Drags from `from` to `to`, and lets go.
        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.hold(from, to);
            self.button(false);
            self.frames(2);
        }

        fn layout(&self) -> &Layout {
            &self.dock.layout
        }

        fn floating(&self, panel: &str) -> Place {
            let id = self.layout().find(panel).expect("the panel is in the dock");
            self.layout().floating(id).expect("the panel floats").place
        }

        /// The rect of the docked group holding `panel`.
        fn docked(&self, panel: &str) -> Rect {
            let id = self.layout().find(panel).expect("the panel is in the dock");
            let (_, rect) = self
                .dock
                .arrangement
                .groups
                .iter()
                .find(|(group, _)| *group == id)
                .expect("the group is docked");
            *rect
        }
    }

    /// A point on the first tab of a group at `min`, and one on its tab bar's background.
    fn first_tab(min: Pos2) -> Pos2 {
        min + vec2(INSET + 4.0, 12.0)
    }

    fn bar(min: Pos2) -> Pos2 {
        min + vec2(250.0, 6.0)
    }

    fn target(rect: Rect, zone: Zone) -> Pos2 {
        let (_, target) = targets(rect, Zones::All)
            .find(|&(z, _)| z == zone)
            .expect("a target");
        target.center()
    }

    #[test]
    fn the_central_area_is_the_games_and_the_panels_are_eguis() {
        let mut run = Run::new();
        let a = run.layout().find("A").unwrap();
        assert!(run.dock.layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        run.frames(2);
        // A is docked along the left; B floats at (72, 72).
        for (at, egui) in [
            (pos2(600.0, 450.0), false),
            (pos2(100.0, 450.0), true),
            (pos2(200.0, 200.0), true),
        ] {
            run.move_to(at);
            assert_eq!(run.ctx.egui_wants_pointer_input(), egui, "at {at:?}");
        }
    }

    #[test]
    fn the_title_bar_takes_the_top_and_the_dock_arranges_below_it() {
        let mut run = Run::with_chrome(Chrome::Drawn);
        let a = run.layout().find("A").unwrap();
        assert!(run.dock.layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        run.frames(2);
        assert_eq!(run.docked("A").min.y, BAR);
        assert_eq!(run.dock.arrangement.central.min.y, BAR);
    }

    #[test]
    fn a_group_docks_on_the_target_its_dropped_on_and_floats_elsewhere() {
        let mut run = Run::new();
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        // A's tab bar, clear of B's group above it, onto the central area's right.
        run.drag(
            bar(pos2(48.0, 48.0)),
            target(screen, Zone::Side(Side::Right)),
        );
        assert!(
            matches!(&run.layout().main, Node::Split { children, .. } if children[0] == Node::Central)
        );
        let a = run.docked("A");
        // A strip's handle lies in a gap along its inner edge.
        assert_eq!(a, Rect::from_min_max(pos2(608.0, 0.0), pos2(800.0, 600.0)));

        // B's tab, onto A's tabs.
        run.drag(first_tab(pos2(72.0, 72.0)), target(a, Zone::Center));
        let group = run.layout().group(run.layout().find("A").unwrap()).unwrap();
        assert_eq!(
            group,
            &Tabs {
                id: group.id,
                panels: vec!["A".to_owned(), "B".to_owned()],
                active: 1
            }
        );
        assert!(run.layout().floating.is_empty());

        // A's tab, out of its bar and into the central area clear of its targets. The pointer
        // keeps its place on the tab, now the first in a group of its own.
        run.drag(first_tab(a.min), pos2(150.0, 150.0));
        assert!(
            matches!(
                run.floating("A"),
                Place::Main {
                    min: [142.0, 138.0],
                    ..
                }
            ),
            "{:?}",
            run.floating("A")
        );
        assert_eq!(run.docked("B"), a);
    }

    #[test]
    fn a_splitter_keeps_its_place_under_the_pointer_wherever_its_held() {
        let mut run = Run::new();
        let a = run.layout().find("A").unwrap();
        assert!(run.dock.layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
        run.frames(2);
        let before = run.docked("A");
        // On the handle away from the split, which is along its right edge, and below B's group.
        let held = pos2(run.dock.arrangement.splitters[0].handle.min.x + 1.0, 500.0);
        run.drag(held, held);
        assert_eq!(run.docked("A"), before);
        run.drag(held, held + vec2(200.0, 0.0));
        assert_eq!(run.docked("A").max.x, before.max.x + 200.0);
    }

    #[test]
    fn a_group_dragged_off_the_main_window_stays_on_it_without_windows_of_its_own() {
        let mut run = Run::new();
        run.drag(bar(pos2(48.0, 48.0)), pos2(-100.0, 300.0));
        let Place::Main { min, .. } = run.floating("A") else {
            panic!("A floats over the main window: {:?}", run.floating("A"));
        };
        assert!(min[0] >= 0.0, "A is kept on the screen: {min:?}");
    }

    #[test]
    fn groups_floated_for_want_of_windows_save_their_windows_places_until_moved_or_resized() {
        // A and B in windows of their own, over the main window, until a window that can't open
        // has the dock float every group over the main window instead, as Wayland does from the
        // start.
        let mut run = Run::with_windows(Chrome::Os);
        let window = |min| Place::Window {
            position: desktop(min),
            size: NEW_SIZE.into(),
        };
        let (a, b) = (window(pos2(40.0, 60.0)), window(pos2(420.0, 300.0)));
        for (panel, place) in [("A", a), ("B", b)] {
            let id = run.layout().find(panel).unwrap();
            run.dock.layout.floating_mut(id).unwrap().place = place;
        }
        run.ctx.set_embed_viewports(true);
        run.frames(2);
        let saved = |run: &Run, panel: &str| {
            let layout = run.layout().saved();
            layout.floating(layout.find(panel).unwrap()).unwrap().place
        };
        assert_eq!(
            run.floating("A"),
            Place::Main {
                min: [40.0, 60.0],
                size: NEW_SIZE.into()
            }
        );
        assert_eq!((saved(&run, "A"), saved(&run, "B")), (a, b));

        // A press on A's bar doesn't move it; dragging the bar does.
        let held = bar(pos2(40.0, 60.0));
        run.move_to(held);
        run.button(true);
        run.button(false);
        assert_eq!(saved(&run, "A"), a);
        run.drag(held, held + vec2(0.0, 100.0));
        assert!(
            matches!(
                saved(&run, "A"),
                Place::Main {
                    min: [40.0, 160.0],
                    ..
                }
            ),
            "{:?}",
            saved(&run, "A")
        );
        assert_eq!(saved(&run, "B"), b);

        // Resizing B by its grip places it too.
        let grip = pos2(420.0, 300.0) + NEW_SIZE - Vec2::splat(GRIP / 2.0);
        run.drag(grip, grip - vec2(20.0, 20.0));
        assert!(
            matches!(saved(&run, "B"), Place::Main { .. }),
            "{:?}",
            saved(&run, "B")
        );
    }

    /// A point in the main window of [`Run::with_windows`] on the desktop, in pixels.
    fn desktop(pos: Pos2) -> [i32; 2] {
        [
            ((100.0 + pos.x) * 1.5).round() as i32,
            ((50.0 + pos.y) * 1.5).round() as i32,
        ]
    }

    #[test]
    fn a_group_dragged_off_the_main_window_tears_off_into_a_window_of_its_own() {
        let mut run = Run::with_windows(Chrome::Os);
        let (from, to) = (bar(pos2(48.0, 48.0)), pos2(-40.0, 300.0));
        run.drag(from, to);
        let grab = from - pos2(48.0, 48.0);
        assert_eq!(
            run.floating("A"),
            Place::Window {
                position: desktop(to - grab),
                size: NEW_SIZE.into()
            }
        );
    }

    #[test]
    fn a_dragged_window_keeps_the_pointer_where_it_holds_the_group() {
        let mut run = Run::with_windows(Chrome::Os);
        // Once A's window is open, its inside is past a border 8 points wide and a title bar 30
        // points high, at 1.25 pixels a point.
        let a = run.layout().find("A").unwrap();
        let window = ViewportInfo {
            outer_rect: Some(Rect::from_min_size(Pos2::ZERO, NEW_SIZE + vec2(16.0, 38.0))),
            inner_rect: Some(Rect::from_min_size(pos2(8.0, 30.0), NEW_SIZE)),
            native_pixels_per_point: Some(1.25),
            ..ViewportInfo::default()
        };
        run.viewports.push((window_id(a), window));
        let (from, to) = (bar(pos2(48.0, 48.0)), pos2(-40.0, 300.0));
        run.hold(from, to);
        let grab = from - pos2(48.0, 48.0);
        let frame = vec2(8.0, 30.0) * 1.25 / 1.5;
        assert_eq!(
            run.floating("A"),
            Place::Window {
                position: desktop(to - grab - frame),
                size: NEW_SIZE.into()
            }
        );
    }

    #[test]
    fn a_group_dragged_over_the_title_bar_tears_off() {
        let mut run = Run::with_windows(Chrome::Drawn);
        run.drag(bar(pos2(48.0, 48.0)), pos2(400.0, BAR / 2.0));
        assert!(
            matches!(run.floating("A"), Place::Window { .. }),
            "{:?}",
            run.floating("A")
        );
    }

    #[test]
    fn a_group_floating_over_the_main_window_stays_off_the_title_bar() {
        let mut run = Run::with_chrome(Chrome::Drawn);
        let from = bar(pos2(48.0, 48.0));
        run.drag(from, from - vec2(0.0, 40.0));
        let Place::Main { min, .. } = run.floating("A") else {
            panic!("A floats over the main window: {:?}", run.floating("A"));
        };
        assert_eq!(min[1], BAR);
    }

    /// A point on the bar of A's window from [`Run::tear_off`], clear of its tab and its close
    /// button.
    const WINDOW_BAR: Vec2 = vec2(200.0, 12.0);

    #[test]
    fn a_window_moved_by_its_bar_stays_a_window_over_the_main_one_and_docks_on_a_target() {
        let mut run = Run::with_window_passes();
        // Right of the main window, at a scale of its own.
        let a = run.tear_off("A", pos2(1200.0, 100.0));
        let (inner, ppp) = run.window(a);
        let from = (inner.min + WINDOW_BAR) * ppp;
        // The central area's right target, on the desktop.
        let central = Rect::from_min_max(pos2(0.0, BAR), SCREEN.to_pos2());
        let to = (pos2(100.0, 50.0) + target(central, Zone::Side(Side::Right)).to_vec2()) * 1.5;
        run.move_in(a, from);
        run.button_in(a, from, true);
        for step in 1..=8 {
            run.move_in(a, from + (to - from) * step as f32 / 8.0);
        }
        // It stays where it is without a new pointer sample, with the pointer where it holds it.
        let place = run.floating("A");
        run.frames(2);
        assert_eq!(run.floating("A"), place);
        let Place::Window { position, .. } = place else {
            panic!("A stays a window over the main one: {place:?}");
        };
        let held = pos2(position[0] as f32, position[1] as f32) + WINDOW_BAR * ppp;
        assert!((held - to).length() < 1.0, "held at {held:?}, not {to:?}");
        run.button_in(a, to, false);
        run.frames(2);
        assert_eq!(
            run.docked("A"),
            Rect::from_min_max(pos2(608.0, BAR), SCREEN.to_pos2())
        );
    }

    #[test]
    fn a_windows_close_button_floats_its_group_over_the_main_window() {
        let mut run = Run::with_window_passes();
        let a = run.tear_off("A", pos2(1200.0, 100.0));
        let (inner, ppp) = run.window(a);
        let close = (inner.min + vec2(NEW_SIZE.x - chrome::BUTTON / 2.0, 12.0)) * ppp;
        run.click_in(a, close);
        assert!(run.commands.contains(&(a, ViewportCommand::Close)));
        run.frames(2);
        assert!(
            matches!(run.floating("A"), Place::Main { .. }),
            "{:?}",
            run.floating("A")
        );
    }

    #[test]
    fn a_windows_edges_resize_it() {
        let mut run = Run::with_window_passes();
        let a = run.tear_off("A", pos2(1200.0, 100.0));
        let (inner, ppp) = run.window(a);
        let edge = (inner.min + vec2(2.0, 120.0)) * ppp;
        run.move_in(a, edge);
        run.button_in(a, edge, true);
        let resize = ViewportCommand::BeginResize(ResizeDirection::West);
        assert!(run.commands.contains(&(a, resize)), "{:?}", run.commands);
    }

    #[test]
    fn a_window_shown_in_the_main_one_has_no_frame_of_the_uis() {
        // Without egui's callback for windows of their own, A's shows in the main window, which
        // its close button would close. The main window is maximized, without edges of its own.
        let mut run = Run::with_windows(Chrome::Drawn);
        run.viewports[0].1.maximized = Some(true);
        let a = run.layout().find("A").unwrap();
        run.dock.layout.floating_mut(a).unwrap().place = Place::Window {
            position: desktop(pos2(300.0, 200.0)),
            size: NEW_SIZE.into(),
        };
        run.frames(2);
        let window = (run.ctx.memory(|m| m.area_rect(Id::unique(window_id(a)))))
            .expect("A's window shows in the main one");
        for x in [8.0, 23.0, 38.0] {
            for y in [12.0, 20.0] {
                let at = pos2(window.max.x - x, window.min.y + y);
                run.move_to(at);
                run.button(true);
                run.button(false);
                let closes =
                    (run.commands.iter()).any(|(_, command)| *command == ViewportCommand::Close);
                assert!(!closes, "at {at:?}: {:?}", run.commands);
            }
        }
    }

    /// The panel that shows the view of the tests' dockspaces.
    const VIEW: &str = "View";

    /// How tall a group's tab bar is, in points.
    fn tab_bar(run: &Run) -> f32 {
        let spacing = &run.ctx.global_style().spacing;
        spacing.interact_size.y + 2.0 * spacing.item_spacing.y
    }

    /// Floats the view's group over the main window at `min`, `size` points inside.
    fn float_view(run: &mut Run, min: [f32; 2], size: Vec2) -> NodeId {
        let place = Place::Main {
            min,
            size: size.into(),
        };
        let id = run.dock.layout.detach(VIEW, place);
        run.dock.layout.prune();
        run.frames(2);
        id
    }

    #[test]
    fn a_dockspace_s_view_fills_it_and_egui_counts_all_of_it_its_own() {
        let mut run = Run::dockspace(Chrome::Drawn);
        let dock = Rect::from_min_max(pos2(0.0, BAR), SCREEN.to_pos2());
        assert_eq!(run.docked(VIEW), dock);
        assert_eq!(run.dock.view(), dock.with_min_y(BAR + tab_bar(&run)));
        // On the view, clear of A and B, which float over it at (48, 48) and (72, 72); then
        // there, on the empty dock, once the view floats.
        let clear = pos2(600.0, 450.0);
        run.move_to(clear);
        assert!(run.ctx.egui_wants_pointer_input());
        float_view(&mut run, [100.0, 100.0], vec2(200.0, 150.0));
        assert_eq!(run.layout().main, Node::Central);
        run.move_to(clear);
        assert!(run.ctx.egui_wants_pointer_input());
    }

    #[test]
    fn a_tab_showing_over_the_view_leaves_it_nowhere() {
        let mut run = Run::dockspace(Chrome::Os);
        let view = run.layout().find(VIEW).unwrap();
        let a = run.layout().find("A").unwrap();
        assert!(run.dock.layout.dock(a, view, Zone::Center));
        run.frames(2);
        assert_eq!(run.dock.view(), Rect::NOTHING);
        run.dock.layout.activate(VIEW);
        run.frames(1);
        assert!(run.dock.view().is_positive());
    }

    #[test]
    fn over_the_view_is_on_it_alone() {
        let mut run = Run::dockspace(Chrome::Os);
        // egui's first pass of a new area sizes it, and doesn't take input.
        run.frames(1);
        let view = run.dock.view();
        // Clear of A and B, under them, and on the view's tab bar.
        for (at, over) in [
            (pos2(600.0, 450.0), true),
            (pos2(100.0, 100.0), false),
            (pos2(600.0, view.min.y - 2.0), false),
        ] {
            assert_eq!(run.dock.over_view(&run.ctx, at), over, "docked, at {at:?}");
        }
        // Floating at (150, 150), over A and B and the empty dock: clear of them, over them, on its
        // tab bar, on its grip, and off it.
        float_view(&mut run, [150.0, 150.0], vec2(300.0, 200.0));
        let content = run.dock.view();
        for (at, over) in [
            (pos2(420.0, 300.0), true),
            (pos2(200.0, 250.0), true),
            (pos2(300.0, content.min.y - 4.0), false),
            (content.max - Vec2::splat(GRIP / 2.0), false),
            (pos2(700.0, 550.0), false),
        ] {
            assert_eq!(
                run.dock.over_view(&run.ctx, at),
                over,
                "floating, at {at:?}"
            );
        }
    }

    #[test]
    fn a_press_on_the_view_by_its_tab_is_the_view_s() {
        let mut run = Run::dockspace(Chrome::Os);
        let view = run.dock.view();
        // Within egui's reach of the view's tab, whose drag would pull it out of its group.
        let at = pos2(view.min.x + INSET + 4.0, view.min.y + 1.0);
        run.drag(at, at + vec2(0.0, 100.0));
        assert!(
            matches!(&run.layout().main, Node::Tabs(tabs) if tabs.panels == [VIEW]),
            "{:?}",
            run.layout().main
        );
        assert_eq!(run.dock.view(), view);
    }

    #[test]
    fn the_view_s_group_dragged_off_the_main_window_floats_over_it() {
        let mut run = Run::dockspace(Chrome::Os);
        run.open_windows();
        run.drag(first_tab(Pos2::ZERO), pos2(-40.0, 300.0));
        assert!(
            matches!(run.floating(VIEW), Place::Main { .. }),
            "{:?}",
            run.floating(VIEW)
        );
    }

    #[test]
    fn the_view_s_group_docks_into_no_window_of_its_own() {
        let mut run = Run::dockspace(Chrome::Drawn);
        run.open_windows();
        run.run_window_passes();
        let a = run.tear_off("A", pos2(1200.0, 100.0));
        // Its bar clear of B, and in the 600 by 400 points from the dock's top left that egui's
        // first pass of a new area keeps it in, sizing it.
        let min = pos2(150.0, 150.0);
        float_view(&mut run, min.into(), NEW_SIZE);
        assert_eq!(
            run.floating(VIEW),
            Place::Main {
                min: min.into(),
                size: NEW_SIZE.into()
            }
        );
        // The center of A's window, which holds its target among A's tabs, in the main window's
        // points.
        let (inner, ppp) = run.window(a);
        let center = inner.center() * ppp;
        let to = pos2(center.x / 1.5 - 100.0, center.y / 1.5 - 50.0);
        run.drag(bar(min), to);
        assert!(
            matches!(run.floating(VIEW), Place::Main { .. }),
            "{:?}",
            run.floating(VIEW)
        );
        let a = run.layout().find("A").unwrap();
        assert_eq!(run.layout().group(a).unwrap().panels, ["A"]);
    }

    #[test]
    fn the_view_pulled_out_and_dropped_on_the_empty_dock_fills_it_again() {
        let mut run = Run::dockspace(Chrome::Os);
        let dock = Rect::from_min_size(Pos2::ZERO, SCREEN);
        // Its tab, out of its bar, and onto the empty dock's one target, at its center.
        run.drag(first_tab(Pos2::ZERO), dock.center());
        assert_eq!(run.docked(VIEW), dock);
        assert!(run.layout().floating.iter().all(|f| !f.tabs.holds(VIEW)));
    }

    #[test]
    fn a_view_saved_in_a_window_of_its_own_floats_over_the_main_window_for_good() {
        for embedded in [false, true] {
            let mut run = Run::dockspace(Chrome::Os);
            run.open_windows();
            run.ctx.set_embed_viewports(embedded);
            let id = run.dock.layout.detach(
                VIEW,
                Place::Main {
                    min: [0.0, 0.0],
                    size: NEW_SIZE.into(),
                },
            );
            run.dock.layout.prune();
            run.dock.layout.floating_mut(id).unwrap().place = Place::Window {
                position: desktop(pos2(150.0, 120.0)),
                size: NEW_SIZE.into(),
            };
            run.frames(2);
            let floats = Place::Main {
                min: [150.0, 120.0],
                size: NEW_SIZE.into(),
            };
            assert_eq!(run.floating(VIEW), floats, "embedded: {embedded}");
            let saved = run.layout().saved();
            assert_eq!(
                saved.floating(id).unwrap().place,
                floats,
                "embedded: {embedded}"
            );
        }
    }
}
