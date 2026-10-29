//! The frame around the app's windows: who draws it, and what the UI draws when it's the UI's:
//! the main window's title bar, which holds a button that opens the menus in a drawer, which
//! slides out from the window's left edge, the controls layers put at its middle, the buttons that
//! hide and show the panels docked left of, below and right of the view, and those that minimize,
//! maximize and close a window; and the edges that resize one. On Windows the rest of the frame
//! stays the OS's own, which `windows_frame` gives back.

use std::mem;

use egui::emath::{TSTransform, easing};
use egui::menu::{MenuConfig, MenuState, SubMenuButton};
use egui::{
    Align, Area, Atom, Button, Color32, Context, CornerRadius, CursorIcon, Frame, Id, LayerId,
    Layout, Order, Painter, Panel, PointerButton, Popup, PopupCloseBehavior, PopupKind, Pos2, Rect,
    RectAlign, ResizeDirection, Response, Sense, Shape, Stroke, StrokeKind, TextStyle,
    TextWrapMode, UiBuilder, UiKind, UiStackInfo, Vec2, ViewportCommand, WidgetText, pos2, vec2,
};
use winit::dpi::LogicalSize;
use winit::window::WindowAttributes;

/// The height of the title bar the UI draws, and of the bar the menus get below the OS's, in
/// points.
pub(crate) const BAR: f32 = 32.0;
/// The height around the traffic lights, which centers them.
const BAR_AROUND_TRAFFIC_LIGHTS: f32 = 28.0;
/// The width of each of a title bar's buttons, in points, as on Windows 11.
pub(crate) const BUTTON: f32 = 46.0;
/// The width of each of the bar's own buttons, which open the menus and hide and show panels, and
/// the side of their icons, in points.
pub(crate) const TOOL: f32 = 36.0;
const ICON: f32 = 16.0;
/// Where the menu button starts, from the left of the bar, in points.
pub(crate) const INSET: f32 = 6.0;
/// Where it starts on macOS, clear of the traffic lights. Not yet checked on a Mac.
const TRAFFIC_LIGHTS: f32 = 72.0;
/// How wide the menus' drawer is, in points, and how long it takes to slide out, in seconds.
const DRAWER: f32 = 200.0;
const SLIDE: f32 = 0.15;
/// How thick a window's edges are, and how far along one from either end it resizes diagonally,
/// in points.
const EDGE: f32 = 5.0;
const CORNER: f32 = 16.0;
/// The close button under the pointer, and pressed, as on Windows.
const CLOSE: Color32 = Color32::from_rgb(196, 43, 28);
const CLOSE_PRESSED: Color32 = Color32::from_rgb(166, 36, 24);

/// Who draws the frame around the app's windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chrome {
    /// The OS. The menus and the layers' controls get a bar of their own, below its title bar.
    Os,
    /// The UI: the title bar, with the menus in it, its buttons and the edges. On X11 and
    /// Wayland.
    Drawn,
    /// The UI's title bar, with its buttons, in Windows' own frame: the OS's borders resize the
    /// window, the bar's free space moves it as the OS's own title bar does, and the OS shows
    /// Snap Layouts over the maximize button. On Windows.
    InWindowsFrame,
    /// The UI, around the OS's buttons, the traffic lights, which keeps the edges too. On macOS.
    AroundTrafficLights,
}

impl Chrome {
    /// The frame of an app's windows: the UI's if the app asks for it, with
    /// [`App::custom_title_bar`](crate::App::custom_title_bar), and otherwise the OS's.
    pub(crate) fn new(custom_title_bar: bool) -> Self {
        if !custom_title_bar {
            Self::Os
        } else if cfg!(target_os = "macos") {
            Self::AroundTrafficLights
        } else if cfg!(target_os = "windows") {
            Self::InWindowsFrame
        } else {
            Self::Drawn
        }
    }

    /// Whether the UI draws the title bar's buttons.
    fn draws_buttons(self) -> bool {
        matches!(self, Self::Drawn | Self::InWindowsFrame)
    }
}

/// The main window's `attributes` with the frame `chrome` says. On Windows, the OS's frame comes
/// back once the window is open, around the UI's title bar.
pub(crate) fn attributes(chrome: Chrome, attributes: WindowAttributes) -> WindowAttributes {
    if chrome == Chrome::Os {
        return attributes;
    }
    // Room for the bar's buttons, the controls and the window's buttons.
    frameless(attributes.with_min_inner_size(LogicalSize::new(480.0, 270.0)))
}

#[cfg(target_os = "macos")]
fn frameless(attributes: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    // The content goes under the title bar, of which only the traffic lights show.
    attributes
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
}

#[cfg(not(target_os = "macos"))]
fn frameless(attributes: WindowAttributes) -> WindowAttributes {
    attributes.with_decorations(false)
}

/// The panels docked left of, below or right of the view, which a button of the title bar hides and
/// shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sidebar {
    Left,
    Bottom,
    Right,
}

/// The main window's title bar, or, under the OS's frame, a bar below its title bar: the button
/// that opens the menus' drawer, and the drawer, the layers' controls at its middle, and where the
/// UI draws the frame, the buttons that hide and show the panels left of, below and right of the
/// view, and the window's own. The dock owns it, since it comes off the window before the dock
/// arranges the rest.
pub(crate) struct TitleBar {
    chrome: Chrome,
    /// The bar this pass, and the Ui its buttons go in until the pass ends; where its buttons
    /// start, at the menu button, and where its free space starts, after it.
    bar: Rect,
    ui: Option<egui::Ui>,
    lead: f32,
    start: f32,
    /// The Ui the menus go in, in the drawer the menu button opens, while it's open, until the pass
    /// ends; and how tall they were in the last pass it was open, which the drawer takes, since
    /// they come after it shows.
    drawer: Option<egui::Ui>,
    drawer_height: f32,
    /// Whether a press has come since the drawer opened, whose click may close it.
    pressed: bool,
    /// The Ui the layers' controls go in, at the middle of the bar, until the pass ends; how wide
    /// they were in the last pass, which centers them; and whether any were added this pass and in
    /// the last.
    middle: Option<egui::Ui>,
    middle_width: f32,
    controls: bool,
    had_controls: bool,
    /// The menus shown this pass, and whether any showed in the last.
    shown: Vec<String>,
    had_menus: bool,
    /// Where the bar was right-clicked, in points, for the OS's window menu.
    window_menu: Option<Pos2>,
    /// The parts of the bar that were the OS's last pass, and how the OS says the maximize
    /// button looks.
    caption: Option<Caption>,
    maximize: Look,
}

/// The parts of the main window's title bar that are the OS's under
/// [`Chrome::InWindowsFrame`], in the window's points: the free space, which moves the window as
/// the OS's own title bar does, and the maximize button, over which the OS shows Snap Layouts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caption {
    /// Left of the menu button; and right of it, either side of the controls, or, without any, as
    /// far as the bar's buttons, and nothing.
    pub(crate) drag: [Rect; 3],
    pub(crate) maximize: Rect,
}

impl TitleBar {
    pub(crate) fn new(chrome: Chrome) -> Self {
        Self {
            chrome,
            bar: Rect::NOTHING,
            ui: None,
            lead: 0.0,
            start: 0.0,
            drawer: None,
            drawer_height: 0.0,
            pressed: false,
            middle: None,
            middle_width: 0.0,
            controls: false,
            had_controls: false,
            shown: Vec::new(),
            had_menus: false,
            window_menu: None,
            caption: None,
            maximize: Look::Idle,
        }
    }

    /// Starts a pass: takes the bar off the top of `root`, with the button that opens the menus'
    /// drawer once a menu has shown, the drawer while it's open, and the middle, which the
    /// controls go in. Under [`Chrome::Os`] it's a bar of its own below the OS's title bar, from
    /// the pass after a menu or a control first shows.
    pub(crate) fn begin(&mut self, root: &mut egui::Ui) {
        self.had_menus = !self.shown.is_empty();
        self.shown.clear();
        self.had_controls = mem::take(&mut self.controls);
        let ctx = root.ctx().clone();
        // Every pass, so that the drawer slides out from the edge each time it opens.
        let open = Popup::is_id_open(&ctx, drawer_id());
        let out =
            ctx.animate_bool_with_time_and_easing(drawer_id(), open, SLIDE, easing::cubic_out);
        // So that the release of the click that opens it doesn't close it.
        self.pressed = open && (self.pressed || ctx.input(|i| i.pointer.any_pressed()));
        if self.chrome == Chrome::Os && !self.had_menus && !self.had_controls {
            return;
        }
        let fullscreen = root.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let (height, left) = match self.chrome {
            Chrome::AroundTrafficLights if !fullscreen => {
                (BAR_AROUND_TRAFFIC_LIGHTS, TRAFFIC_LIGHTS)
            }
            Chrome::AroundTrafficLights => (BAR_AROUND_TRAFFIC_LIGHTS, INSET),
            _ => (BAR, INSET),
        };
        let frame = Frame::NONE.fill(root.visuals().panel_fill);
        Panel::top(Id::unique("gg title bar"))
            .exact_size(height)
            .frame(frame)
            .resizable(false)
            .show(root, |ui| {
                let bar = ui.max_rect();
                self.bar = bar;
                self.lead = bar.min.x + left;
                self.start = self.lead;
                let mut ui = ui.new_child(UiBuilder::new().max_rect(bar));
                if self.had_menus {
                    let size = vec2(TOOL, bar.height());
                    let rect = Rect::from_min_size(pos2(self.lead, bar.min.y), size);
                    self.start = rect.max.x;
                    let button = tool(&mut ui, rect, Tool::Menu, open, true).on_hover_text("Menus");
                    if button.clicked() {
                        Popup::toggle_id(&ctx, drawer_id());
                    }
                    self.drawer = self.show_drawer(&ui, out);
                }
                // At the middle of the bar where they fit there, as the title is, by their width
                // last pass.
                let end = bar.max.x - self.right();
                let half = self.middle_width / 2.0;
                let x = (bar.center().x)
                    .min(end - INSET - half)
                    .max(self.start + INSET + half);
                let size = vec2(self.middle_width, bar.height());
                let middle = Rect::from_center_size(pos2(x, bar.center().y), size);
                let layout = Layout::left_to_right(Align::Center);
                self.middle = Some(ui.new_child(UiBuilder::new().max_rect(middle).layout(layout)));
                self.ui = Some(ui);
            });
    }

    /// How much of the bar's right its buttons take: those that hide and show the panels around the
    /// view, where the UI draws the title bar, and the window's, where it draws them.
    fn right(&self) -> f32 {
        let sidebars = if self.chrome == Chrome::Os {
            0.0
        } else {
            3.0 * TOOL
        };
        let window = if self.chrome.draws_buttons() {
            3.0 * BUTTON
        } else {
            0.0
        };
        sidebars + window
    }

    /// Shows the menus' drawer while it's open, against the window's left edge below the bar, `out`
    /// of the way out as it slides from that edge, over the dock rather than pushing it aside,
    /// which would resize the game's view as it slid; and returns the Ui the menus go in. It closes
    /// on its button, Escape, and, as [`TitleBar::end`] says, clicks; its menus, as egui's do.
    fn show_drawer(&self, ui: &egui::Ui, out: f32) -> Option<egui::Ui> {
        let ctx = ui.ctx();
        // Square against the edges it comes from.
        let radius = ui.visuals().menu_corner_radius.se;
        let frame = Frame::menu(ui.style()).corner_radius(CornerRadius {
            se: radius,
            ..CornerRadius::ZERO
        });
        let height = self.drawer_height;
        let mut menus = None;
        let edge = self.bar.with_max_x(self.bar.min.x);
        // egui would check for a click that closes the drawer as it shows, before its menus come,
        // and close it whatever's open in them, which `end` does instead; its menus close on a
        // click as egui's do.
        let config = MenuConfig::new()
            .close_behavior(PopupCloseBehavior::CloseOnClick)
            .style(crate::menu_style);
        let info =
            UiStackInfo::new(UiKind::Menu).with_tag_value(MenuConfig::MENU_CONFIG_TAG, config);
        let shown = Popup::new(drawer_id(), ctx.clone(), edge, ui.layer_id())
            .kind(PopupKind::Menu)
            .info(info)
            .close_behavior(PopupCloseBehavior::IgnoreClicks)
            .layout(Layout::top_down_justified(Align::Min))
            .style(crate::menu_style)
            .frame(frame)
            .align(RectAlign::BOTTOM_START)
            .align_alternatives(&[])
            .gap(0.0)
            .open_memory(None)
            .show(|ui| {
                ui.set_width(DRAWER);
                ui.set_min_height(height);
                menus = Some(ui.new_child(UiBuilder::new()));
            })?;
        let width = shown.response.rect.width();
        let slid = TSTransform::from_translation(vec2(-(1.0 - out) * width, 0.0));
        ctx.set_transform_layer(shown.response.layer_id, slid);
        menus
    }

    /// Adds menu `name` to the drawer this pass, with what `add_contents` draws in it while it's
    /// open: an item, while the drawer's open, whose menu opens beside it as the pointer comes
    /// onto it, as a submenu does. A second menu of the same name doesn't show.
    pub(crate) fn menu(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        if self.shown.iter().any(|shown| shown == name) {
            return;
        }
        self.shown.push(name.to_owned());
        if let Some(drawer) = &mut self.drawer {
            // Named by its name rather than its place, so that an open menu stays open as others
            // come and go.
            drawer.push_id(name, |ui| SubMenuButton::new(name).ui(ui, add_contents));
        }
    }

    /// Adds what `add_contents` draws to the controls at the middle of the bar this pass, after
    /// those added before, in place of the window's title; and returns what it returned, unless
    /// the bar doesn't show. Under [`Chrome::Os`], the bar shows from the pass after the first
    /// control is added.
    pub(crate) fn controls<R>(
        &mut self,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        self.controls = true;
        self.middle.as_mut().map(add_contents)
    }

    /// Ends the pass: draws the window's title at the middle of the bar, unless the layers put
    /// controls there; where the UI draws the title bar, the buttons that hide and show the panels
    /// left of, below and right of the view, as `sidebars` says of each (`None` while none is
    /// docked there, or whether they show), and where it draws the frame, the window's buttons and
    /// edges; and has the bar's free space move the window. Returns the side whose button was
    /// clicked, if any.
    pub(crate) fn end(&mut self, sidebars: impl Fn(Sidebar) -> Option<bool>) -> Option<Sidebar> {
        self.caption = None;
        // Dropped within the pass, since dropping a Ui records its rect in egui.
        let (drawer, middle) = (self.drawer.take(), self.middle.take());
        let mut ui = self.ui.take()?;
        let ctx = ui.ctx().clone();
        if let Some(drawer) = drawer {
            // A pass the menus outgrew or shrank from goes again, rather than show the drawer
            // another size than theirs.
            let height = drawer.min_rect().height();
            if height != self.drawer_height {
                self.drawer_height = height;
                ctx.request_discard("the menus' drawer changed size");
            }
            // Closed as an item chosen closes its menu, or by a click, pressed while it was open,
            // while none of its menus is: one that is takes the click, as egui's menus do.
            let clicked = self.pressed && ctx.input(|i| i.pointer.any_click());
            let menu_open = !MenuState::is_deepest_open_sub_menu(&ctx, drawer_id());
            if drawer.should_close() || (clicked && !menu_open) {
                Popup::close_id(&ctx, drawer_id());
            }
        }
        let controls = middle.filter(|_| self.controls).map(|ui| ui.min_rect());
        if let Some(controls) = controls
            && (controls.width() - self.middle_width).abs() > 0.5
        {
            self.middle_width = controls.width();
            ctx.request_discard("the title bar's controls changed width");
        }
        if self.chrome == Chrome::Os {
            return None;
        }
        let (title, focused, maximized, outer) = ctx.input(|i| {
            let info = i.viewport();
            (
                info.title.clone().unwrap_or_default(),
                info.focused.unwrap_or(true),
                info.maximized.unwrap_or(false),
                info.outer_rect,
            )
        });
        let bar = self.bar;
        let end = bar.max.x - self.right();
        let free = Rect::from_min_max(pos2(self.start, bar.min.y), pos2(end, bar.max.y));

        if controls.is_none() {
            // The title, at the middle of the bar where it fits there, and elided to fit.
            let width = (free.width() - 2.0 * INSET).max(0.0);
            let galley = WidgetText::from(title).into_galley(
                &ui,
                Some(TextWrapMode::Truncate),
                width,
                TextStyle::Body,
            );
            let half = galley.size().x / 2.0;
            let x = (bar.center().x)
                .min(free.max.x - INSET - half)
                .max(free.min.x + INSET + half);
            let visuals = ui.visuals();
            let color = if focused {
                visuals.text_color()
            } else {
                visuals.weak_text_color()
            };
            let at = pos2(x - half, bar.center().y - galley.size().y / 2.0);
            ui.painter().galley(at, galley, color);
        }

        // The bar left of the menu button, where the traffic lights are on macOS, and the free
        // space stand for the OS's title bar. None of it covers a button or a control, which
        // would then take the press, and the free space the drag. In Windows' frame they're the
        // OS's own title bar, with the maximize button, and egui sees the pointer over them only
        // while it takes the whole bar.
        let drag = match controls {
            Some(controls) => [
                bar.with_max_x(self.lead),
                free.with_max_x(controls.min.x),
                free.with_min_x(controls.max.x),
            ],
            None => [bar.with_max_x(self.lead), free, Rect::NOTHING],
        };
        let caption = Caption {
            drag,
            maximize: Rect::from_min_size(
                pos2(bar.max.x - 2.0 * BUTTON, bar.min.y),
                vec2(BUTTON, bar.height()),
            ),
        };
        let held = ctx.input(|i| {
            let pointer = &i.pointer;
            let moved = pointer.latest_pos()? - pointer.press_origin()?;
            pointer.primary_down().then_some(moved)
        });
        let drag = caption.drag.into_iter().enumerate();
        for (i, rect) in drag.filter(|(_, rect)| rect.is_positive()) {
            let id = Id::unique(("gg title bar free space", i));
            let response = ui.interact(rect, id, Sense::click_and_drag());
            // egui starts a drag once the pointer has moved a few points or the button has been
            // held a while, so that a click doesn't move the window. The window catches up with
            // the pointer, keeping the point held under it, and the OS moves it from there while
            // the button stays down. A maximized window, the OS restores under the pointer.
            if response.drag_started_by(PointerButton::Primary)
                && let Some(moved) = held
            {
                if let Some(outer) = outer.filter(|_| !maximized) {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min + moved));
                }
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if response.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            if response.secondary_clicked() {
                self.window_menu = response.interact_pointer_pos();
            }
        }

        // As far right as the window's buttons, which stay the rightmost, as the OS's are.
        let mut toggled = None;
        let all = [Sidebar::Left, Sidebar::Bottom, Sidebar::Right];
        for (i, sidebar) in all.into_iter().enumerate() {
            let at = pos2(end + i as f32 * TOOL, bar.min.y);
            let rect = Rect::from_min_size(at, vec2(TOOL, bar.height()));
            let shows = sidebars(sidebar);
            let side = match sidebar {
                Sidebar::Left => "left of",
                Sidebar::Bottom => "below",
                Sidebar::Right => "right of",
            };
            let tip = match shows {
                Some(true) => format!("Hide the panels {side} the view"),
                Some(false) => format!("Show the panels {side} the view"),
                None => format!("No panels are docked {side} the view"),
            };
            let icon = Tool::Sidebar(sidebar, shows == Some(true));
            let response = tool(&mut ui, rect, icon, false, shows.is_some());
            if response
                .on_hover_text(&tip)
                .on_disabled_hover_text(tip)
                .clicked()
            {
                toggled = Some(sidebar);
            }
        }

        if self.chrome.draws_buttons() {
            let maximize = if maximized {
                Icon::Restore
            } else {
                Icon::Maximize
            };
            // How the OS says the maximize button looks, while egui sees no pointer over it.
            let buttons = [
                (Icon::Minimize, ViewportCommand::Minimized(true), Look::Idle),
                (
                    maximize,
                    ViewportCommand::Maximized(!maximized),
                    self.maximize,
                ),
                (Icon::Close, ViewportCommand::Close, Look::Idle),
            ];
            for (i, (icon, command, os)) in buttons.into_iter().enumerate() {
                let rect = (caption.maximize).translate(vec2((i as f32 - 1.0) * BUTTON, 0.0));
                if button(&ui, rect, icon, os).clicked() {
                    ctx.send_viewport_cmd(command);
                }
            }
        }
        if self.chrome == Chrome::Drawn {
            edges(&ctx);
        }
        if self.chrome == Chrome::InWindowsFrame && !covered(&ctx, bar) {
            self.caption = Some(caption);
        }
        toggled
    }

    /// The parts of the bar that were the OS's last pass: none unless the frame is Windows'
    /// own, nor while egui shows something over the bar.
    #[cfg_attr(
        not(target_os = "windows"),
        allow(dead_code, reason = "only Windows' frame asks")
    )]
    pub(crate) fn caption(&self) -> Option<Caption> {
        self.caption
    }

    /// How the maximize button looks from the next pass, as the OS says where it hit-tests the
    /// button.
    #[cfg_attr(
        not(target_os = "windows"),
        allow(dead_code, reason = "only Windows' frame says")
    )]
    pub(crate) fn set_maximize(&mut self, look: Look) {
        self.maximize = look;
    }

    /// Where the bar was right-clicked since this was last asked, in the main window's points,
    /// for the OS's window menu.
    pub(crate) fn take_window_menu(&mut self) -> Option<Pos2> {
        self.window_menu.take()
    }
}

/// Whether egui shows something over `bar`, such as a window, or has a menu open, which a click
/// on the bar should close: then the UI takes the whole bar, leaving none of it to the OS.
fn covered(ctx: &Context, bar: Rect) -> bool {
    ctx.any_popup_open()
        || ctx.memory(|memory| {
            let over = |layer: LayerId| {
                matches!(layer.order, Order::Middle | Order::Foreground)
                    && (memory.area_rect(layer.id))
                        .is_some_and(|rect| rect.intersect(bar).is_positive())
            };
            memory.areas().visible_layer_ids().into_iter().any(over)
        })
}

/// Has the edges of the window this pass draws resize it: Areas above everything else along
/// them, so that a press there reaches neither the game nor egui's widgets beneath. None while
/// the window is maximized or fullscreen.
pub(crate) fn edges(ctx: &Context) {
    let (window, maximized, fullscreen) = ctx.input(|i| {
        let info = i.viewport();
        (
            i.viewport_rect(),
            info.maximized.unwrap_or(false),
            info.fullscreen.unwrap_or(false),
        )
    });
    if maximized || fullscreen {
        return;
    }
    let (min, max) = (window.min, window.max);
    let edges = [
        Rect::from_min_max(min, pos2(max.x, min.y + EDGE)),
        Rect::from_min_max(pos2(min.x, max.y - EDGE), max),
        Rect::from_min_max(min, pos2(min.x + EDGE, max.y)),
        Rect::from_min_max(pos2(max.x - EDGE, min.y), max),
    ];
    for (i, edge) in edges.into_iter().enumerate() {
        Area::new(Id::unique(("gg window edge", i)))
            .order(Order::Foreground)
            .fixed_pos(edge.min)
            .constrain(false)
            .sense(Sense::hover())
            .show(ctx, |ui| {
                // Only drags, so that what's beneath gets no clicks either.
                let response = ui.allocate_rect(edge, Sense::drag());
                if let Some(pos) = response.hover_pos() {
                    ctx.set_cursor_icon(cursor(direction(window, pos)));
                }
                // egui starts dragging a widget that senses only drags as it's pressed.
                if response.drag_started()
                    && let Some(press) = ctx.input(|i| i.pointer.press_origin())
                {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction(window, press)));
                }
            });
    }
}

/// Which way a press at `pos`, on an edge of `window`, resizes it: diagonally within a corner's
/// length of one.
fn direction(window: Rect, pos: Pos2) -> ResizeDirection {
    let near = |distance: f32| distance < CORNER;
    let north = near(pos.y - window.min.y);
    let south = near(window.max.y - pos.y);
    let west = near(pos.x - window.min.x);
    let east = near(window.max.x - pos.x);
    match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (_, _, true, _) => ResizeDirection::West,
        _ => ResizeDirection::East,
    }
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// What a title bar's button does, which its icon shows.
#[derive(Clone, Copy, Debug, Hash)]
pub(crate) enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// How a title bar's button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Look {
    Idle,
    /// Under the pointer.
    Hovered,
    Pressed,
}

/// A title bar's button over `rect`, drawn with its icon, which lights up under the pointer: red
/// for Close, as on Windows. Where the OS hit-tests the button instead, egui sees no pointer over
/// it, and `os` is how the OS says it looks.
pub(crate) fn button(ui: &egui::Ui, rect: Rect, icon: Icon, os: Look) -> Response {
    let response = ui.interact(
        rect,
        Id::unique(("gg title bar button", icon)),
        Sense::click(),
    );
    let look = if response.is_pointer_button_down_on() {
        Look::Pressed
    } else if response.hovered() {
        Look::Hovered
    } else {
        Look::Idle
    };
    let widgets = &ui.visuals().widgets;
    let (fill, color) = match (icon, look.max(os)) {
        (Icon::Close, Look::Pressed) => (CLOSE_PRESSED, Color32::WHITE),
        (Icon::Close, Look::Hovered) => (CLOSE, Color32::WHITE),
        (_, Look::Pressed) => (widgets.active.weak_bg_fill, widgets.active.fg_stroke.color),
        (_, Look::Hovered) => (
            widgets.hovered.weak_bg_fill,
            widgets.hovered.fg_stroke.color,
        ),
        (_, Look::Idle) => (Color32::TRANSPARENT, widgets.inactive.fg_stroke.color),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, fill);
    // Lines a point wide, on pixels' centers, 10 points across.
    let stroke = Stroke::new(1.0, color);
    let center = rect.center();
    let c = pos2(
        painter.round_to_pixel_center(center.x),
        painter.round_to_pixel_center(center.y),
    );
    let r = 5.0;
    match icon {
        Icon::Minimize => {
            painter.line_segment([c - vec2(r, 0.0), c + vec2(r, 0.0)], stroke);
        }
        Icon::Maximize => {
            let square = Rect::from_center_size(c, Vec2::splat(2.0 * r));
            painter.rect_stroke(square, 0.0, stroke, StrokeKind::Middle);
        }
        Icon::Restore => {
            // A square, and the corner of another behind it.
            let front = Rect::from_min_max(c + vec2(-r, -3.0), c + vec2(3.0, r));
            painter.rect_stroke(front, 0.0, stroke, StrokeKind::Middle);
            let back = vec![c + vec2(-3.0, -r), c + vec2(r, -r), c + vec2(r, 3.0)];
            painter.add(Shape::line(back, stroke));
        }
        Icon::Close => {
            painter.line_segment([c - vec2(r, r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        }
    }
    response
}

/// The id of the menus' drawer, and of its slide.
fn drawer_id() -> Id {
    Id::unique("gg title bar drawer")
}

/// One of the bar's own buttons, rather than the window's.
#[derive(Clone, Copy)]
enum Tool {
    /// Three lines across, as menus behind a button have.
    Menu,
    /// A window with a strip along one side, filled while the panels there show.
    Sidebar(Sidebar, bool),
}

impl Tool {
    /// Paints its icon in `rect`, a square, in `color`.
    fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let scale = rect.width() / ICON;
        // From the top left of a square 16 across.
        let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
        let line = Stroke::new(1.5 * scale, color);
        match self {
            Self::Menu => {
                for y in [4.0, 8.0, 12.0] {
                    painter.line_segment([at(2.0, y), at(14.0, y)], line);
                }
            }
            Self::Sidebar(sidebar, shows) => {
                let window = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
                let outline = Stroke::new(1.25 * scale, color);
                painter.rect_stroke(window, 2.0 * scale, outline, StrokeKind::Middle);
                let strip = match sidebar {
                    Sidebar::Left => window.with_max_x(at(6.0, 0.0).x),
                    Sidebar::Bottom => window.with_min_y(at(0.0, 9.5).y),
                    Sidebar::Right => window.with_min_x(at(10.0, 0.0).x),
                };
                if shows {
                    painter.rect_filled(strip, 2.0 * scale, color);
                } else {
                    // The edge the strip shares with the rest of the window.
                    match sidebar {
                        Sidebar::Left => painter.vline(strip.max.x, window.y_range(), outline),
                        Sidebar::Bottom => painter.hline(window.x_range(), strip.min.y, outline),
                        Sidebar::Right => painter.vline(strip.min.x, window.y_range(), outline),
                    };
                }
            }
        }
    }
}

/// Draws `tool` over `rect`, of the bar's full height: its icon, on a rounded light inset from the
/// bar's edges under the pointer, or while `lit`, as the layers' controls are; off unless
/// `enabled`.
fn tool(ui: &mut egui::Ui, rect: Rect, tool: Tool, lit: bool, enabled: bool) -> Response {
    let icon = Atom::paint(Vec2::splat(ICON), move |ui, args| {
        tool.paint(ui.painter(), args.rect, args.fallback_text_color);
    });
    let inset = rect.shrink2(vec2(4.0, 4.0));
    ui.add_enabled_ui(enabled, |ui| ui.put(inset, Button::selectable(lit, icon)))
        .inner
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui::epaint::ClippedShape;
    use egui::{Event, Key, Modifiers, RawInput, ViewportId, ViewportInfo};

    use super::*;

    const SCREEN: Vec2 = vec2(800.0, 600.0);

    /// egui and a title bar, run a frame at a time on made-up input.
    struct Run {
        ctx: Context,
        bar: TitleBar,
        /// The menus layers add each frame, and the width of a control they add, if they add one.
        menus: &'static [&'static str],
        control: Option<f32>,
        /// What the sidebars' buttons say of the left side, the bottom and the right side.
        sidebars: [Option<bool>; 3],
        /// Where egui shows an area of a layer's each frame, if anywhere.
        area: Option<Rect>,
        frame: u32,
        pointer: Pos2,
        /// What egui knows of the window.
        info: ViewportInfo,
        /// Where the bar leaves the rest of the window, the menus open, the control, the side
        /// whose button was clicked, and what egui drew, as of the last pass.
        top: f32,
        open: Rc<RefCell<Vec<&'static str>>>,
        shown: Option<Rect>,
        toggled: Option<Sidebar>,
        shapes: Vec<ClippedShape>,
    }

    impl Run {
        /// Runs a couple of frames, with File and Edit menus.
        fn new(chrome: Chrome) -> Self {
            Self::with_menus(chrome, &["File", "Edit"])
        }

        fn with_menus(chrome: Chrome, menus: &'static [&'static str]) -> Self {
            let mut run = Self {
                ctx: crate::egui_context(),
                bar: TitleBar::new(chrome),
                menus,
                control: None,
                sidebars: [None; 3],
                area: None,
                frame: 0,
                pointer: Pos2::ZERO,
                info: ViewportInfo::default(),
                top: 0.0,
                open: Rc::default(),
                shown: None,
                toggled: None,
                shapes: Vec::new(),
            };
            run.frames(2);
            run
        }

        /// Runs a frame, and returns the commands it sent the window that move, resize,
        /// minimize, maximize or close it.
        fn frame(&mut self, events: Vec<Event>) -> Vec<ViewportCommand> {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(f64::from(self.frame) / 60.0),
                events,
                ..RawInput::default()
            };
            input.viewports.insert(ViewportId::ROOT, self.info.clone());
            self.frame += 1;
            let (bar, menus, top, open) = (&mut self.bar, self.menus, &mut self.top, &self.open);
            let (control, sidebars, area) = (self.control, self.sidebars, self.area);
            let (shown, toggled) = (&mut self.shown, &mut self.toggled);
            // A pass egui runs again gets no events, so a click is the first pass's.
            *toggled = None;
            let mut output = self.ctx.run_ui(input, |root| {
                open.borrow_mut().clear();
                bar.begin(root);
                *top = root.available_rect_before_wrap().min.y;
                for &name in menus {
                    bar.menu(name, |ui| {
                        open.borrow_mut().push(name);
                        ui.label(format!("{name} item"));
                    });
                }
                *shown = control.and_then(|width| {
                    let size = vec2(width, 20.0);
                    bar.controls(|ui| ui.allocate_exact_size(size, Sense::click()).0)
                });
                if let Some(rect) = area {
                    Area::new(Id::unique("a layer's area"))
                        .fixed_pos(rect.min)
                        .show(root.ctx(), |ui| ui.set_min_size(rect.size()));
                }
                let clicked = bar.end(|sidebar| match sidebar {
                    Sidebar::Left => sidebars[0],
                    Sidebar::Bottom => sidebars[1],
                    Sidebar::Right => sidebars[2],
                });
                *toggled = toggled.or(clicked);
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
            let commands = (output.viewport_output.remove(&ViewportId::ROOT))
                .map(|output| output.commands)
                .unwrap_or_default();
            commands
                .into_iter()
                .filter(|command| {
                    matches!(
                        command,
                        ViewportCommand::StartDrag
                            | ViewportCommand::OuterPosition(_)
                            | ViewportCommand::BeginResize(_)
                            | ViewportCommand::Minimized(_)
                            | ViewportCommand::Maximized(_)
                            | ViewportCommand::Close
                    )
                })
                .collect()
        }

        fn frames(&mut self, n: usize) -> Vec<ViewportCommand> {
            (0..n).flat_map(|_| self.frame(Vec::new())).collect()
        }

        fn move_to(&mut self, pos: Pos2) -> Vec<ViewportCommand> {
            self.pointer = pos;
            self.frame(vec![Event::PointerMoved(pos)])
        }

        fn button(&mut self, button: PointerButton, pressed: bool) -> Vec<ViewportCommand> {
            self.frame(vec![Event::PointerButton {
                pos: self.pointer,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            }])
        }

        /// Clicks at `pos`, and returns the commands sent on the way.
        fn click_with(&mut self, button: PointerButton, pos: Pos2) -> Vec<ViewportCommand> {
            let mut commands = self.move_to(pos);
            commands.extend(self.button(button, true));
            commands.extend(self.button(button, false));
            commands
        }

        fn click(&mut self, pos: Pos2) -> Vec<ViewportCommand> {
            self.click_with(PointerButton::Primary, pos)
        }

        /// Presses at `from`, drags to `to` over a few frames and lets go, and returns the
        /// commands sent on the way.
        fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<ViewportCommand> {
            let mut commands = self.move_to(from);
            commands.extend(self.button(PointerButton::Primary, true));
            for step in 1..=4 {
                commands.extend(self.move_to(from + (to - from) * step as f32 / 4.0));
            }
            commands.extend(self.button(PointerButton::Primary, false));
            commands
        }

        fn open(&self) -> Vec<&'static str> {
            self.open.borrow().clone()
        }

        fn drawer_open(&self) -> bool {
            Popup::is_id_open(&self.ctx, drawer_id())
        }

        /// Presses Escape, which closes what's open.
        fn escape(&mut self) {
            self.frame(vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }]);
        }

        /// Where the last pass drew `text`, topmost first.
        fn texts(&self, text: &str) -> Vec<Rect> {
            let painted = crate::testing::painted(&self.shapes).into_iter();
            let found = painted.filter_map(|shape| match shape {
                Shape::Text(shape) if shape.galley.text() == text => {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            });
            found.collect()
        }

        /// The middle of `text`, which the last pass drew.
        fn text(&self, text: &str) -> Pos2 {
            let found = self.texts(text);
            found
                .first()
                .unwrap_or_else(|| panic!("{text:?} shows"))
                .center()
        }
    }

    /// A point on the menu button, and one in the bar's free space.
    const MENU: Pos2 = pos2(INSET + TOOL / 2.0, 16.0);
    const FREE: Pos2 = pos2(400.0, 16.0);

    #[test]
    fn the_bar_takes_the_top_of_the_window() {
        assert_eq!(Run::new(Chrome::Drawn).top, BAR);
        assert_eq!(Run::new(Chrome::InWindowsFrame).top, BAR);
        assert_eq!(
            Run::new(Chrome::AroundTrafficLights).top,
            BAR_AROUND_TRAFFIC_LIGHTS
        );
        // Under the OS's frame, it's a bar of its own from the frame after a menu or a control
        // first shows, and without either there's none.
        assert_eq!(Run::new(Chrome::Os).top, BAR);
        let mut run = Run::with_menus(Chrome::Os, &[]);
        assert_eq!(run.top, 0.0);
        run.control = Some(40.0);
        run.frames(2);
        assert_eq!(run.top, BAR);
    }

    /// The menu button opens the drawer, which slides out from the window's left edge below the
    /// bar, and lists the menus, each opening beside it as the pointer comes onto it; an item
    /// chosen closes the drawer, as its button and Escape do.
    #[test]
    fn the_menu_button_opens_the_drawer_whose_menus_open_beside_it() {
        for chrome in [Chrome::Drawn, Chrome::Os] {
            let mut run = Run::new(chrome);
            assert!(run.texts("File").is_empty(), "{chrome:?}");
            run.click(MENU);
            assert!(run.drawer_open(), "{chrome:?}");
            run.frames(1);
            let sliding = run.text("File");
            run.frames(12);
            let file = run.text("File");
            assert!(
                sliding.x < file.x,
                "{chrome:?}: from {sliding:?} to {file:?}"
            );
            assert!(
                file.y > run.top && file.x < 2.0 * TOOL,
                "{chrome:?}: {file:?}"
            );
            run.move_to(file);
            run.frames(1);
            assert_eq!(run.open(), ["File"], "{chrome:?}");
            run.move_to(run.text("Edit"));
            run.frames(1);
            assert_eq!(run.open(), ["Edit"], "{chrome:?}");
            run.click(run.text("Edit item"));
            assert!(!run.drawer_open(), "{chrome:?}");
            run.click(MENU);
            run.frames(2);
            run.click(MENU + vec2(1.0, 0.0));
            assert!(!run.drawer_open(), "{chrome:?}");
            run.click(MENU);
            run.frames(2);
            run.escape();
            assert!(!run.drawer_open(), "{chrome:?}");
        }
    }

    /// The layers' controls show at the middle of the bar, centered on it, in place of the
    /// window's title; the free space either side of them is the OS's caption in Windows' frame,
    /// and where the UI draws the frame, a drag on them doesn't move the window.
    #[test]
    fn controls_take_the_middle_of_the_bar_in_place_of_the_title() {
        let mut run = Run::new(Chrome::InWindowsFrame);
        run.info.title = Some("Editor".to_owned());
        run.frames(1);
        assert_eq!(run.texts("Editor").len(), 1);
        run.control = Some(100.0);
        run.frames(2);
        assert!(run.texts("Editor").is_empty());
        let control = run.shown.expect("the control shows");
        assert_eq!(control.center().x, SCREEN.x / 2.0);
        let caption = run.bar.caption().expect("the OS has part of the bar");
        let [_, before, after] = caption.drag;
        assert_eq!((before.max.x, after.min.x), (control.min.x, control.max.x));
        let mut run = Run::new(Chrome::Drawn);
        run.control = Some(100.0);
        run.frames(2);
        let at = run.shown.expect("the control shows").center();
        assert_eq!(run.drag(at, at + vec2(30.0, 20.0)), []);
    }

    /// The sidebars' buttons, left, bottom and right, sit left of the window's buttons, or at the
    /// right end of the bar around the traffic lights; a click on one says which, while panels are
    /// docked on its side. Under the OS's frame, there are none.
    #[test]
    fn the_sidebars_buttons_say_which_was_clicked() {
        let left_of_buttons = SCREEN.x - 3.0 * BUTTON;
        for (chrome, end) in [
            (Chrome::Drawn, left_of_buttons),
            (Chrome::InWindowsFrame, left_of_buttons),
            (Chrome::AroundTrafficLights, SCREEN.x),
        ] {
            let mut run = Run::new(chrome);
            run.sidebars = [Some(true), None, None];
            run.frames(1);
            let y = run.top / 2.0;
            let [left, bottom, right] = [3.0, 2.0, 1.0].map(|n| pos2(end - (n - 0.5) * TOOL, y));
            run.click(left);
            assert_eq!(run.toggled, Some(Sidebar::Left), "{chrome:?}");
            for button in [bottom, right] {
                run.click(button);
                assert_eq!(run.toggled, None, "{chrome:?}");
            }
            run.sidebars = [Some(true), Some(true), Some(false)];
            run.frames(1);
            run.click(bottom + vec2(1.0, 0.0));
            assert_eq!(run.toggled, Some(Sidebar::Bottom), "{chrome:?}");
            run.click(right + vec2(1.0, 0.0));
            assert_eq!(run.toggled, Some(Sidebar::Right), "{chrome:?}");
        }
        let mut run = Run::new(Chrome::Os);
        run.sidebars = [Some(true); 3];
        run.frames(1);
        run.click(pos2(SCREEN.x - TOOL / 2.0, run.top / 2.0));
        assert_eq!(run.toggled, None);
    }

    #[test]
    fn dragging_the_free_space_moves_the_window_and_dragging_a_button_doesnt() {
        let mut run = Run::new(Chrome::Drawn);
        run.info.outer_rect = Some(Rect::from_min_size(pos2(100.0, 50.0), SCREEN));
        // egui starts the drag a step in, (7.5, 5) points from the press, where the window
        // catches up before the OS moves it.
        let (from, to) = (FREE, FREE + vec2(30.0, 20.0));
        let catch_up = ViewportCommand::OuterPosition(pos2(107.5, 55.0));
        assert_eq!(run.drag(from, to), [catch_up, ViewportCommand::StartDrag]);
        assert_eq!(run.drag(MENU, MENU + vec2(30.0, 20.0)), []);
        // A click doesn't move it.
        run.frames(30);
        assert_eq!(run.click(FREE), []);
        // A maximized window, the OS restores under the pointer.
        run.info.maximized = Some(true);
        run.frames(30);
        assert_eq!(run.drag(from, to), [ViewportCommand::StartDrag]);
    }

    #[test]
    fn a_double_click_on_the_free_space_maximizes_the_window_and_another_restores_it() {
        let mut run = Run::new(Chrome::Drawn);
        assert_eq!(run.click(FREE), []);
        assert_eq!(run.click(FREE), [ViewportCommand::Maximized(true)]);
        run.info.maximized = Some(true);
        run.frames(30);
        run.click(FREE);
        assert_eq!(run.click(FREE), [ViewportCommand::Maximized(false)]);
    }

    #[test]
    fn the_buttons_minimize_maximize_or_restore_and_close_the_window() {
        let at = |i: f32| pos2(SCREEN.x - (3.0 - i) * BUTTON + BUTTON / 2.0, BAR / 2.0);
        for chrome in [Chrome::Drawn, Chrome::InWindowsFrame] {
            let mut run = Run::new(chrome);
            assert_eq!(run.click(at(0.0)), [ViewportCommand::Minimized(true)]);
            assert_eq!(run.click(at(1.0)), [ViewportCommand::Maximized(true)]);
            assert_eq!(run.click(at(2.0)), [ViewportCommand::Close]);
            run.info.maximized = Some(true);
            run.frames(30);
            assert_eq!(run.click(at(1.0)), [ViewportCommand::Maximized(false)]);
        }
        // Around the traffic lights, the OS has the buttons.
        let mut run = Run::new(Chrome::AroundTrafficLights);
        assert_eq!(run.click(at(2.0)), []);
    }

    #[test]
    fn in_windows_frame_the_os_has_the_free_space_and_the_maximize_button_unless_egui_covers_the_bar()
     {
        let mut run = Run::new(Chrome::InWindowsFrame);
        let caption = run.bar.caption().expect("the OS has part of the bar");
        let [left, free, none] = caption.drag;
        let bar = run.bar.bar;
        assert_eq!(left, bar.with_max_x(INSET));
        // From the menu button to the sidebars' buttons, without controls between.
        let end = pos2(SCREEN.x - 3.0 * BUTTON - 3.0 * TOOL, bar.max.y);
        assert_eq!(free, Rect::from_min_max(pos2(INSET + TOOL, 0.0), end));
        assert_eq!(none, Rect::NOTHING);
        let maximize = bar
            .with_min_x(SCREEN.x - 2.0 * BUTTON)
            .with_max_x(SCREEN.x - BUTTON);
        assert_eq!(caption.maximize, maximize);
        // A click on the bar closes the menus' drawer, so egui takes the whole bar while it's
        // open.
        run.click(MENU);
        assert!(run.drawer_open());
        assert_eq!(run.bar.caption(), None);
        run.click(MENU + vec2(1.0, 0.0));
        assert!(!run.drawer_open());
        run.frames(1);
        assert_eq!(run.bar.caption(), Some(caption));
        // Or while it shows something over the bar.
        run.area = Some(Rect::from_min_size(pos2(300.0, 20.0), vec2(100.0, 100.0)));
        run.frames(2);
        assert_eq!(run.bar.caption(), None);
        run.area = Some(Rect::from_min_size(pos2(300.0, BAR), vec2(100.0, 100.0)));
        run.frames(2);
        assert_eq!(run.bar.caption(), Some(caption));
        // Where the UI draws the whole frame, none of the bar is the OS's.
        assert_eq!(Run::new(Chrome::Drawn).bar.caption(), None);
    }

    #[test]
    fn the_maximize_button_looks_as_the_os_says() {
        let mut run = Run::new(Chrome::InWindowsFrame);
        let bar = run.bar.bar;
        let maximize = bar
            .with_min_x(SCREEN.x - 2.0 * BUTTON)
            .with_max_x(SCREEN.x - BUTTON);
        let fill = |run: &Run| {
            run.shapes.iter().find_map(|clipped| match &clipped.shape {
                Shape::Rect(shape)
                    if shape.rect == maximize && shape.fill != Color32::TRANSPARENT =>
                {
                    Some(shape.fill)
                }
                _ => None,
            })
        };
        let widgets = run.ctx.global_style().visuals.widgets.clone();
        assert_eq!(fill(&run), None);
        run.bar.set_maximize(Look::Hovered);
        run.frames(1);
        assert_eq!(fill(&run), Some(widgets.hovered.weak_bg_fill));
        run.bar.set_maximize(Look::Pressed);
        run.frames(1);
        assert_eq!(fill(&run), Some(widgets.active.weak_bg_fill));
        run.bar.set_maximize(Look::Idle);
        run.frames(1);
        assert_eq!(fill(&run), None);
    }

    #[test]
    fn a_right_click_on_the_free_space_asks_for_the_window_menu() {
        let mut run = Run::new(Chrome::Drawn);
        run.click_with(PointerButton::Secondary, MENU);
        assert_eq!(run.bar.take_window_menu(), None);
        run.click_with(PointerButton::Secondary, FREE);
        assert_eq!(run.bar.take_window_menu(), Some(FREE));
        assert_eq!(run.bar.take_window_menu(), None);
    }

    #[test]
    fn the_quit_shortcut_is_off() {
        let mut run = Run::new(Chrome::Drawn);
        let quit = Event::Key {
            key: Key::Q,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        assert_eq!(run.frame(vec![quit]), []);
    }

    #[test]
    fn the_edges_resize_the_window_unless_its_maximized() {
        let mut run = Run::new(Chrome::Drawn);
        let (w, h) = (SCREEN.x, SCREEN.y);
        let presses = [
            (pos2(2.0, 300.0), ResizeDirection::West),
            (pos2(w - 2.0, 300.0), ResizeDirection::East),
            (pos2(400.0, 2.0), ResizeDirection::North),
            (pos2(400.0, h - 2.0), ResizeDirection::South),
            (pos2(2.0, 2.0), ResizeDirection::NorthWest),
            (pos2(w - 2.0, 10.0), ResizeDirection::NorthEast),
            (pos2(12.0, h - 2.0), ResizeDirection::SouthWest),
            (pos2(w - 2.0, h - 2.0), ResizeDirection::SouthEast),
        ];
        for (at, direction) in presses {
            let mut commands = run.move_to(at);
            commands.extend(run.button(PointerButton::Primary, true));
            assert_eq!(
                commands,
                [ViewportCommand::BeginResize(direction)],
                "{at:?}"
            );
            run.button(PointerButton::Primary, false);
        }
        // Beside the central area, the edges are egui's, and the game gets no press there.
        run.move_to(pos2(2.0, 300.0));
        assert!(run.ctx.is_pointer_over_egui());
        run.move_to(pos2(12.0, 300.0));
        assert!(!run.ctx.is_pointer_over_egui());

        run.info.maximized = Some(true);
        run.frames(2);
        assert_eq!(run.click(pos2(2.0, 300.0)), []);
        assert!(!run.ctx.is_pointer_over_egui());
        // Around the traffic lights, and in Windows' frame, the OS has the edges.
        for chrome in [Chrome::AroundTrafficLights, Chrome::InWindowsFrame] {
            let mut run = Run::new(chrome);
            assert_eq!(run.click(pos2(2.0, 300.0)), [], "{chrome:?}");
        }
    }
}
