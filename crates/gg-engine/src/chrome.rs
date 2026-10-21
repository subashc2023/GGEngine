//! The frame around the app's windows: who draws it, and what the UI draws when it's the UI's:
//! the main window's title bar, which holds the menus, the buttons that minimize, maximize and
//! close a window, and the edges that resize one.

use egui::{
    Align, Area, Color32, Context, CursorIcon, Frame, Id, Layout, MenuBar, Order, Panel,
    PointerButton, Pos2, Rect, ResizeDirection, Response, Sense, Shape, Stroke, StrokeKind,
    TextStyle, TextWrapMode, UiBuilder, Vec2, ViewportCommand, WidgetText, pos2, vec2,
};
use winit::dpi::LogicalSize;
use winit::window::WindowAttributes;

/// The height of the title bar the UI draws, in points.
pub(crate) const BAR: f32 = 32.0;
/// The height around the traffic lights, which centers them.
const BAR_AROUND_TRAFFIC_LIGHTS: f32 = 28.0;
/// The width of each of a title bar's buttons, in points, as on Windows 11.
pub(crate) const BUTTON: f32 = 46.0;
/// Where the menus start, from the left of the bar, in points.
const INSET: f32 = 6.0;
/// Where they start on macOS, clear of the traffic lights. Not yet checked on a Mac.
const TRAFFIC_LIGHTS: f32 = 72.0;
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
    /// The OS. The menus get a bar of their own, below its title bar.
    Os,
    /// The UI: the title bar, with the menus in it, its buttons and the edges. On Windows, X11
    /// and Wayland.
    Drawn,
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
        } else {
            Self::Drawn
        }
    }
}

/// The main window's `attributes` with the frame `chrome` says.
pub(crate) fn attributes(chrome: Chrome, attributes: WindowAttributes) -> WindowAttributes {
    if chrome == Chrome::Os {
        return attributes;
    }
    // Room for a few menus, the title and the buttons.
    frameless(attributes.with_min_inner_size(LogicalSize::new(480.0, 270.0)))
}

#[cfg(target_os = "windows")]
fn frameless(attributes: WindowAttributes) -> WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    // DWM keeps the window's shadow, at the cost of a line a pixel high along its top.
    attributes
        .with_decorations(false)
        .with_undecorated_shadow(true)
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

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn frameless(attributes: WindowAttributes) -> WindowAttributes {
    attributes.with_decorations(false)
}

/// The main window's menu bar, which is its title bar when the UI draws the frame. The dock owns
/// it, since it comes off the window before the dock arranges the rest.
pub(crate) struct TitleBar {
    chrome: Chrome,
    /// The bar this pass, and the Ui the menus go in until the pass ends. That Ui is made inside
    /// egui's menu bar, so that hovering a menu while another is open switches to it.
    bar: Rect,
    menus: Option<egui::Ui>,
    /// The menus shown this pass, and whether any showed in the last.
    shown: Vec<String>,
    had_menus: bool,
    /// Where the bar was right-clicked, in points, for the OS's window menu.
    window_menu: Option<Pos2>,
}

impl TitleBar {
    pub(crate) fn new(chrome: Chrome) -> Self {
        Self {
            chrome,
            bar: Rect::NOTHING,
            menus: None,
            shown: Vec::new(),
            had_menus: false,
            window_menu: None,
        }
    }

    /// Starts a pass: takes the bar off the top of `root`. Under [`Chrome::Os`] it's a menu bar,
    /// from the pass after a menu first shows.
    pub(crate) fn begin(&mut self, root: &mut egui::Ui) {
        self.had_menus = !self.shown.is_empty();
        self.shown.clear();
        let id = Id::unique("gg title bar");
        if self.chrome == Chrome::Os {
            if self.had_menus {
                Panel::top(id).resizable(false).show(root, |ui| {
                    MenuBar::new().ui(ui, |ui| self.menus = Some(ui.new_child(UiBuilder::new())));
                });
            }
            return;
        }
        let fullscreen = root.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let (height, left, right) = match self.chrome {
            Chrome::AroundTrafficLights if !fullscreen => {
                (BAR_AROUND_TRAFFIC_LIGHTS, TRAFFIC_LIGHTS, 0.0)
            }
            Chrome::AroundTrafficLights => (BAR_AROUND_TRAFFIC_LIGHTS, INSET, 0.0),
            _ => (BAR, INSET, 3.0 * BUTTON),
        };
        let frame = Frame::NONE.fill(root.visuals().panel_fill);
        Panel::top(id)
            .exact_size(height)
            .frame(frame)
            .resizable(false)
            .show(root, |ui| {
                let bar = ui.max_rect();
                let menus = Rect::from_min_max(
                    pos2(bar.min.x + left, bar.min.y),
                    pos2(bar.max.x - right, bar.max.y),
                );
                MenuBar::new().ui(ui, |ui| {
                    let builder = UiBuilder::new()
                        .max_rect(menus)
                        .layout(Layout::left_to_right(Align::Center));
                    self.menus = Some(ui.new_child(builder));
                });
                self.bar = bar;
            });
    }

    /// Adds menu `name` to the bar this pass, with what `add_contents` draws in it while it's
    /// open. A second menu of the same name doesn't show.
    pub(crate) fn menu(&mut self, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        if self.shown.iter().any(|shown| shown == name) {
            return;
        }
        self.shown.push(name.to_owned());
        if let Some(menus) = &mut self.menus {
            // Named by its name rather than its place, so that an open menu stays open as others
            // come and go.
            menus.push_id(name, |ui| ui.menu_button(name, add_contents));
        }
    }

    /// Ends the pass: draws the title and, where the UI draws the frame, the buttons and the
    /// window's edges, and has the bar's free space move the window.
    pub(crate) fn end(&mut self) {
        // Dropped within the pass, since dropping a Ui records its rect in egui.
        let Some(ui) = self.menus.take() else {
            return;
        };
        if self.chrome == Chrome::Os {
            return;
        }
        let ctx = ui.ctx().clone();
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
        let buttons = if self.chrome == Chrome::Drawn {
            3.0 * BUTTON
        } else {
            0.0
        };
        let menus = ui.min_rect();
        let free = Rect::from_min_max(
            pos2(menus.max.x, bar.min.y),
            pos2(bar.max.x - buttons, bar.max.y),
        );

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

        // The free space, and the bar left of the menus, where the traffic lights are on macOS,
        // stand for the OS's title bar. Neither covers a menu, which would then take the press
        // and the free space the drag.
        let left = bar.with_max_x(menus.min.x);
        let held = ctx.input(|i| {
            let pointer = &i.pointer;
            let moved = pointer.latest_pos()? - pointer.press_origin()?;
            pointer.primary_down().then_some(moved)
        });
        for (i, rect) in [left, free].into_iter().enumerate() {
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

        if self.chrome == Chrome::Drawn {
            let maximize = if maximized {
                Icon::Restore
            } else {
                Icon::Maximize
            };
            let commands = [
                (Icon::Minimize, ViewportCommand::Minimized(true)),
                (maximize, ViewportCommand::Maximized(!maximized)),
                (Icon::Close, ViewportCommand::Close),
            ];
            let mut rect = Rect::from_min_size(
                pos2(bar.max.x - 3.0 * BUTTON, bar.min.y),
                vec2(BUTTON, bar.height()),
            );
            for (icon, command) in commands {
                if button(&ui, rect, icon).clicked() {
                    ctx.send_viewport_cmd(command);
                }
                rect = rect.translate(vec2(BUTTON, 0.0));
            }
            edges(&ctx);
        }
    }

    /// Where the bar was right-clicked since this was last asked, in the main window's points,
    /// for the OS's window menu.
    pub(crate) fn take_window_menu(&mut self) -> Option<Pos2> {
        self.window_menu.take()
    }
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

/// A title bar's button over `rect`, drawn with its icon, which lights up under the pointer: red
/// for Close, as on Windows.
pub(crate) fn button(ui: &egui::Ui, rect: Rect, icon: Icon) -> Response {
    let response = ui.interact(
        rect,
        Id::unique(("gg title bar button", icon)),
        Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    let (fill, color) = match icon {
        Icon::Close if response.is_pointer_button_down_on() => (CLOSE_PRESSED, Color32::WHITE),
        Icon::Close if response.hovered() => (CLOSE, Color32::WHITE),
        _ if response.hovered() => (visuals.weak_bg_fill, visuals.fg_stroke.color),
        _ => (Color32::TRANSPARENT, visuals.fg_stroke.color),
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui::{Event, Key, Modifiers, RawInput, ViewportId, ViewportInfo};

    use super::*;

    const SCREEN: Vec2 = vec2(800.0, 600.0);

    /// egui and a title bar, run a frame at a time on made-up input.
    struct Run {
        ctx: Context,
        bar: TitleBar,
        /// The menus layers add each frame.
        menus: &'static [&'static str],
        frame: u32,
        pointer: Pos2,
        /// What egui knows of the window.
        info: ViewportInfo,
        /// Where the bar leaves the rest of the window, and the menus open, as of the last pass.
        top: f32,
        open: Rc<RefCell<Vec<&'static str>>>,
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
                frame: 0,
                pointer: Pos2::ZERO,
                info: ViewportInfo::default(),
                top: 0.0,
                open: Rc::default(),
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
            let mut output = self.ctx.run_ui(input, |root| {
                open.borrow_mut().clear();
                bar.begin(root);
                *top = root.available_rect_before_wrap().min.y;
                for &name in menus {
                    bar.menu(name, |ui| {
                        open.borrow_mut().push(name);
                        ui.label(name);
                    });
                }
                bar.end();
            });
            output.textures_delta.clear();
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
    }

    /// A point on the first menu, File, and one in the bar's free space.
    const FILE: Pos2 = pos2(INSET + 6.0, 16.0);
    const FREE: Pos2 = pos2(400.0, 16.0);

    #[test]
    fn the_bar_takes_the_top_of_the_window() {
        assert_eq!(Run::new(Chrome::Drawn).top, BAR);
        assert_eq!(
            Run::new(Chrome::AroundTrafficLights).top,
            BAR_AROUND_TRAFFIC_LIGHTS
        );
        // Under the OS's frame, it's a menu bar, without menus none.
        let top = Run::new(Chrome::Os).top;
        assert!(top > 0.0 && top < BAR, "{top}");
        assert_eq!(Run::with_menus(Chrome::Os, &[]).top, 0.0);
    }

    #[test]
    fn a_menu_opens_and_hovering_another_switches_to_it() {
        for chrome in [Chrome::Drawn, Chrome::Os] {
            let mut run = Run::new(chrome);
            let file = pos2(FILE.x, run.top / 2.0);
            run.click(file);
            assert_eq!(run.open(), ["File"], "{chrome:?}");
            // Rightwards along the bar, onto Edit.
            let mut at = file;
            while run.open() == ["File"] && at.x < 300.0 {
                at.x += 4.0;
                run.move_to(at);
            }
            run.frames(1);
            assert_eq!(run.open(), ["Edit"], "{chrome:?}, at {at:?}");
        }
    }

    #[test]
    fn dragging_the_free_space_moves_the_window_and_dragging_a_menu_doesnt() {
        let mut run = Run::new(Chrome::Drawn);
        run.info.outer_rect = Some(Rect::from_min_size(pos2(100.0, 50.0), SCREEN));
        // egui starts the drag a step in, (7.5, 5) points from the press, where the window
        // catches up before the OS moves it.
        let (from, to) = (FREE, FREE + vec2(30.0, 20.0));
        let catch_up = ViewportCommand::OuterPosition(pos2(107.5, 55.0));
        assert_eq!(run.drag(from, to), [catch_up, ViewportCommand::StartDrag]);
        assert_eq!(run.drag(FILE, FILE + vec2(30.0, 20.0)), []);
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
        let mut run = Run::new(Chrome::Drawn);
        let at = |i: f32| pos2(SCREEN.x - (3.0 - i) * BUTTON + BUTTON / 2.0, BAR / 2.0);
        assert_eq!(run.click(at(0.0)), [ViewportCommand::Minimized(true)]);
        assert_eq!(run.click(at(1.0)), [ViewportCommand::Maximized(true)]);
        assert_eq!(run.click(at(2.0)), [ViewportCommand::Close]);
        run.info.maximized = Some(true);
        run.frames(30);
        assert_eq!(run.click(at(1.0)), [ViewportCommand::Maximized(false)]);
        // Around the traffic lights, the OS has the buttons.
        let mut run = Run::new(Chrome::AroundTrafficLights);
        assert_eq!(run.click(at(2.0)), []);
    }

    #[test]
    fn a_right_click_on_the_free_space_asks_for_the_window_menu() {
        let mut run = Run::new(Chrome::Drawn);
        run.click_with(PointerButton::Secondary, FILE);
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
        // Around the traffic lights, the OS has the edges.
        let mut run = Run::new(Chrome::AroundTrafficLights);
        assert_eq!(run.click(pos2(2.0, 300.0)), []);
    }
}
