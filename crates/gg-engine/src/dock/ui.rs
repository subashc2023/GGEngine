//! The dock in a frame: the strips it takes off the main window's edges, the groups it draws as
//! layers show their panels, and the drags that dock, float and tear off panels.

use std::collections::HashSet;
use std::mem;
use std::path::PathBuf;

use egui::{
    Align, Area, Button, Context, CursorIcon, Frame, Id, LayerId, Order, Painter, Panel, Pos2,
    Rect, Sense, StrokeKind, UiBuilder, Vec2, ViewportBuilder, ViewportId, pos2, vec2,
};
use tracing::warn;

use super::file;
use super::tree::{Arrangement, Axis, CENTRAL, Layout, NodeId, Place, Side, Zone};

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
}

impl<'a> Ui<'a> {
    pub(crate) fn new(root: &'a mut egui::Ui, dock: &'a mut Dock) -> Self {
        Self { root, dock }
    }

    /// Shows panel `name` this frame, with what `add_contents` draws in it. The user places
    /// panels: in tab groups docked around the central area, or floating, over the main window
    /// or in windows of their own. A panel shows on the frames that show it, and keeps its place
    /// while it doesn't; `add_contents` runs when its tab is the one showing. `name` titles the
    /// panel and is its identity: the dock keeps panels' places by name, between runs too, and
    /// a second panel of the same name in a frame doesn't show.
    pub fn panel(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        self.dock.panel(self.root, name, add_contents);
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
}

/// Which drop targets a node has.
#[derive(Clone, Copy, PartialEq)]
enum Zones {
    /// A docked group: among its tabs, and beside it.
    All,
    /// The central area: beside it only.
    Sides,
    /// A floating group: among its tabs only.
    Center,
}

impl Dock {
    /// A dock with the layout saved in `file`, or kept in memory only without one. A file that
    /// can't be read logs a warning, and the dock starts empty.
    pub(crate) fn new(file: Option<PathBuf>) -> Self {
        let layout = match &file {
            Some(path) => file::load(path).unwrap_or_else(|error| {
                warn!(code = error.code(), path = %path.display(), "{error:#}; the dock starts empty");
                Layout::new()
            }),
            None => Layout::new(),
        };
        let arrangement = layout.arrange(Rect::NOTHING, &|_| false);
        Self {
            layout,
            file,
            shown: HashSet::new(),
            showing: HashSet::new(),
            arrangement,
            drawn: HashSet::new(),
            new: Vec::new(),
            clicked: None,
            drag: None,
            hover: None,
            splitter_grab: 0.0,
        }
    }

    /// Saves the layout to its file, if it has one. A failure logs a warning.
    pub(crate) fn save(&self) {
        if let Some(path) = &self.file
            && let Err(error) = file::save(path, &self.layout)
        {
            warn!(code = error.code(), path = %path.display(), "{error:#}");
        }
    }

    /// Starts a frame: takes the strips along the main window's edges that docked groups fill
    /// from `root`, leaving it the central area. egui counts the strips as its own, and the
    /// central area as the game's.
    pub(crate) fn begin(&mut self, root: &mut egui::Ui) {
        if root.ctx().embed_viewports() {
            // Without windows of their own, floating groups float over the main window.
            for floating in &mut self.layout.floating {
                if let Place::Window { position, size } = floating.place {
                    let min = from_desktop(root.ctx(), position).unwrap_or(pos2(48.0, 48.0));
                    floating.place = Place::Main {
                        min: min.into(),
                        size,
                    };
                }
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
        self.drawn.clear();
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

    /// Ends a frame: draws what the panels didn't, moves the drag along, and makes the frame's
    /// changes to the layout.
    pub(crate) fn end(&mut self, root: &mut egui::Ui) {
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
            self.body(&mut ui, id, false, add_contents);
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
                self.body(&mut group, id, true, add_contents);
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
        ctx.show_viewport_immediate(viewport, builder, |ui, _| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
            let mut group = ui.new_child(UiBuilder::new().max_rect(rect));
            self.body(&mut group, id, false, |ui| {
                if let Some(add_contents) = add_contents.take() {
                    add_contents(ui);
                }
            });
            if self
                .drag
                .as_ref()
                .is_some_and(|drag| drag.source == viewport)
            {
                ui.interact(rect, drag_id(), Sense::drag());
            }
            if let Some(hover) = self.hover.filter(|hover| hover.window == Some(id)) {
                paint_hover(ui.ctx(), &hover);
            }
        });
    }

    /// Draws a group into `ui`, filling it: its tab bar, with a tab for each of its panels shown
    /// last frame, then what `add_contents` draws of the one showing. Clicking a tab shows its
    /// panel from the next frame, and dragging a tab starts a drag, as does dragging the bar of
    /// a group floating over the main window (`movable`).
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        movable: bool,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        let rect = ui.max_rect();
        let spacing = ui.spacing().clone();
        let bar_height = spacing.interact_size.y + 2.0 * spacing.item_spacing.y;
        let (bar, content) = rect.split_top_bottom_at_y(rect.min.y + bar_height);
        let background =
            movable.then(|| ui.interact(bar, Id::unique(("gg dock bar", id)), Sense::drag()));
        ui.painter()
            .rect_filled(bar, 0.0, ui.visuals().faint_bg_color);
        let shown = &self.shown;
        let Some(tabs) = self.layout.group(id) else {
            return;
        };
        let showing = tabs.showing(&|p| shown.contains(p));
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
        let mut content_ui =
            ui.new_child(UiBuilder::new().max_rect(content.shrink(spacing.item_spacing.x)));
        content_ui.set_clip_rect(content.intersect(ui.clip_rect()));
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
            {
                self.layout.set_ratio(
                    splitter.split,
                    splitter.ratio_at(pointer[i] - self.splitter_grab),
                );
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
        let (local, down, focused) = ctx.input_for(drag.source, |i| {
            (i.pointer.latest_pos(), i.pointer.primary_down(), i.focused)
        });
        let pointer = local.and_then(|local| to_main(ctx, drag.source, local));
        if let (Some(local), Some(pointer)) = (local, pointer) {
            if let Dragging::Tab { panel, bar } = &drag.what
                && !bar.expand(DETACH).contains(local)
            {
                let place = place_at(ctx, pointer, drag.grab, Vec2::ZERO, drag.size);
                drag.what = Dragging::Group(self.layout.detach(panel, place));
            } else if let Dragging::Group(group) = drag.what
                && let Some(floating) = self.layout.floating_mut(group)
            {
                let frame = window_frame(ctx, group);
                floating.place = place_at(ctx, pointer, drag.grab, frame, floating.place.size());
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
                        let px = outer.min * native * zoom;
                        from_desktop(ctx, [px.x.round() as i32, px.y.round() as i32])
                    })
                    .unwrap_or(pos2(48.0, 48.0));
                floating.place = Place::Main {
                    min: min.into(),
                    size,
                };
            } else if Some(floating.tabs.id) != dragged
                && let (Some(outer), Some(inner), Some(native)) = (outer, inner, native)
            {
                let px = outer.min * native * zoom;
                floating.place = Place::Window {
                    position: [px.x.round() as i32, px.y.round() as i32],
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
    let px = (inner.min + pos.to_vec2()) * ppp;
    from_desktop(ctx, [px.x.round() as i32, px.y.round() as i32])
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
    let px = (main.min + pos.to_vec2()) * ppp;
    Some([px.x.round() as i32, px.y.round() as i32])
}

/// Where a group held at `grab` by the pointer at `pointer`, in the main window's points, goes:
/// over the main window while the pointer is, and otherwise in a window of its own, where egui
/// opens them, whose inside is `frame` from its outside corner.
fn place_at(ctx: &Context, pointer: Pos2, grab: Vec2, frame: Vec2, size: Vec2) -> Place {
    let min = pointer - grab;
    let main = ctx.input_for(ViewportId::ROOT, |i| i.viewport_rect());
    if !ctx.embed_viewports()
        && !main.contains(pointer)
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
/// the central area.
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
    };
    let others = || {
        layout
            .floating
            .iter()
            .rev()
            .filter(|f| f.tabs.id != dragged)
    };
    if let Some(px) = to_desktop(ctx, pointer) {
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
    if !ctx
        .input_for(ViewportId::ROOT, |i| i.viewport_rect())
        .contains(pointer)
    {
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
    let central = arrangement.central;
    central
        .contains(pointer)
        .then(|| at(CENTRAL, central, None, Zones::Sides, pointer))
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
    let share = if hover.node == CENTRAL { 0.25 } else { 0.5 };
    if let Some(zone) = hover.zone {
        let rect = preview(hover.rect, zone, share);
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
    use egui::{Event, Modifiers, PointerButton, RawInput, ViewportInfo};

    use super::super::tree::{Node, Tabs};
    use super::*;

    const SCREEN: Vec2 = vec2(800.0, 600.0);

    /// egui and a dock, run a frame at a time on made-up input, with panels A and B showing.
    struct Run {
        ctx: Context,
        dock: Dock,
        frame: u32,
        pointer: Pos2,
        /// Where the windows are on the desktop, for a run with windows of their own.
        viewports: Vec<(ViewportId, ViewportInfo)>,
    }

    impl Run {
        /// Runs until both panels float, A then B, cascaded from the top-left.
        fn new() -> Self {
            let mut run = Self {
                ctx: Context::default(),
                dock: Dock::new(None),
                frame: 0,
                pointer: Pos2::ZERO,
                viewports: Vec::new(),
            };
            run.frames(2);
            run
        }

        /// A run with windows of their own, whose main window's inside starts at (100, 50) on
        /// the desktop, at 1.5 pixels a point.
        fn with_windows() -> Self {
            let mut run = Self::new();
            run.ctx.set_embed_viewports(false);
            let main = ViewportInfo {
                inner_rect: Some(Rect::from_min_size(pos2(100.0, 50.0), SCREEN)),
                native_pixels_per_point: Some(1.5),
                ..ViewportInfo::default()
            };
            run.viewports.push((ViewportId::ROOT, main));
            run
        }

        fn frame(&mut self, events: Vec<Event>) {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(f64::from(self.frame) / 60.0),
                events,
                ..RawInput::default()
            };
            input.viewports.extend(self.viewports.iter().cloned());
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

    /// A point in the main window of [`Run::with_windows`] on the desktop, in pixels.
    fn desktop(pos: Pos2) -> [i32; 2] {
        [
            ((100.0 + pos.x) * 1.5).round() as i32,
            ((50.0 + pos.y) * 1.5).round() as i32,
        ]
    }

    #[test]
    fn a_group_dragged_off_the_main_window_tears_off_into_a_window_of_its_own() {
        let mut run = Run::with_windows();
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
        let mut run = Run::with_windows();
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
}
