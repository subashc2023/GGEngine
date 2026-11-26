use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use egui::epaint::ClippedShape;
use egui::{
    Event, ImmediateViewport, Modifiers, RawInput, ResizeDirection, Shape, ViewportEvent,
    ViewportIdMap, ViewportInfo,
};

use super::super::tree::{Node, Tabs};
use super::*;
use crate::chrome::BAR;
use crate::dirs::tests::Scratch;

const SCREEN: Vec2 = vec2(800.0, 600.0);

/// egui and a dock, run a frame at a time on made-up input, with panels A and B showing, or those
/// it's given, and the menus it's given.
struct Run {
    ctx: Context,
    dock: Dock,
    frame: u32,
    pointer: Pos2,
    /// The panels the layers show each frame, and the menus they add, in order. A panel shows
    /// `In <its name>`, a menu `<its name> item`.
    panels: Vec<&'static str>,
    menus: Vec<&'static str>,
    /// Where the windows are on the desktop.
    viewports: Vec<(ViewportId, ViewportInfo)>,
    /// What windows' passes get when run as the engine runs them.
    passes: Option<Rc<RefCell<Passes>>>,
    /// What egui asked of each window last frame.
    commands: Vec<(ViewportId, ViewportCommand)>,
    /// Whether frames show a 100 by 30 point toolbar at the view's top left, moved by `offset`;
    /// last frame's results follow.
    toolbar: bool,
    offset: Vec2,
    shown: Option<Rect>,
    view: Option<Response>,
    viewport: Option<Viewport>,
    /// Whether frames show a cover holding a button, Covered; what egui made of it last frame.
    cover: bool,
    covered: Option<Response>,
    /// Whether frames add a Setting button to the title bar's settings; what egui made of it
    /// last frame, while they were open.
    setting: bool,
    set: Option<Response>,
    /// What the main window's pass drew last frame, and the windows egui showed with what it
    /// asked of them.
    shapes: Vec<ClippedShape>,
    windows: Vec<(ViewportId, ViewportBuilder)>,
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

    fn with_chrome(chrome: Chrome) -> Self {
        Self::with_dock(Dock::new(None, chrome, None, false, &[]))
    }

    /// A dockspace under `chrome`; its view, panel [`VIEW`], fills the dock under floating A and
    /// B.
    fn dockspace(chrome: Chrome) -> Self {
        Self::with_dock(Dock::new(None, chrome, Some(VIEW), false, &[]))
    }

    /// With the View menu and no layout file, as headless runs have.
    fn with_menu() -> Self {
        Self::with_dock(Dock::new(None, Chrome::Os, None, true, &[]))
    }

    fn with_dock(dock: Dock) -> Self {
        Self::showing(dock, &["A", "B"])
    }

    fn showing(dock: Dock, panels: &[&'static str]) -> Self {
        let mut run = Self {
            ctx: crate::egui_context(),
            dock,
            frame: 0,
            pointer: Pos2::ZERO,
            panels: panels.to_vec(),
            menus: Vec::new(),
            viewports: Vec::new(),
            passes: None,
            commands: Vec::new(),
            toolbar: false,
            offset: Vec2::ZERO,
            shown: None,
            view: None,
            viewport: None,
            cover: false,
            covered: None,
            setting: false,
            set: None,
            shapes: Vec::new(),
            windows: Vec::new(),
        };
        run.frames(2);
        run
    }

    fn with_windows(chrome: Chrome) -> Self {
        let mut run = Self::with_chrome(chrome);
        run.open_windows();
        run
    }

    /// Opens windows from the next frame, the main window's inside at (100, 50) on the desktop,
    /// 1.5 pixels a point. Without egui's renderer callback, they show in the main window.
    fn open_windows(&mut self) {
        self.ctx.set_embed_viewports(false);
        let main = ViewportInfo {
            inner_rect: Some(Rect::from_min_size(pos2(100.0, 50.0), SCREEN)),
            native_pixels_per_point: Some(1.5),
            ..ViewportInfo::default()
        };
        self.viewports.push((ViewportId::ROOT, main));
    }

    /// Windows framed by the UI, with [`Run::run_window_passes`].
    fn with_window_passes() -> Self {
        let mut run = Self::with_windows(Chrome::Drawn);
        run.run_window_passes();
        run
    }

    /// Runs windows' passes as the engine does, each with its own input.
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

    /// Puts `panel`'s group in a frameless window, its inside at `min` on the desktop, 1.25
    /// pixels a point. Returns the viewport.
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
        let (dock, panels, menus) = (&mut self.dock, &self.panels, &self.menus);
        let (toolbar, offset, cover) = (self.toolbar, self.offset, self.cover);
        let (mut shown, mut view, mut viewport, mut covered) = (None, None, None, None);
        let (setting, mut set) = (self.setting, None);
        let mut output = self.ctx.run_ui(input, |root| {
            dock.begin(root);
            let mut ui = Ui::new(root, dock);
            if setting {
                ui.settings(|ui| {
                    // Low enough to be over the view, below its tab bar.
                    ui.add_space(40.0);
                    set = Some(ui.button("Setting"));
                });
            }
            for &name in menus {
                ui.menu(name, |ui| {
                    ui.label(format!("{name} item"));
                });
            }
            for &name in panels {
                ui.panel(name, |ui| {
                    ui.label(format!("In {name}"));
                });
            }
            (view, viewport) = (ui.view(), ui.viewport());
            if toolbar {
                shown = ui.over_view("toolbar", Align2::LEFT_TOP, offset, |ui| {
                    ui.allocate_exact_size(vec2(100.0, 30.0), Sense::click()).0
                });
            }
            if cover {
                covered = Some(ui.cover(|ui| ui.button("Covered")));
            }
            dock.end(root);
        });
        (self.shown, self.view, self.viewport) = (shown, view, viewport);
        (self.covered, self.set) = (covered, set);
        output.textures_delta.clear();
        self.shapes = mem::take(&mut output.shapes);
        let windows = output.viewport_output.iter();
        let windows = windows.filter(|&(&id, _)| id != ViewportId::ROOT);
        self.windows = (windows.map(|(&id, output)| (id, output.builder.clone()))).collect();
        // As the OS and engine would: a window moves where egui asks, and a close comes back as
        // an event the next frame's passes see.
        self.commands.clear();
        for (id, output) in output.viewport_output {
            if let Some((_, info)) = self.viewports.iter_mut().find(|(window, _)| *window == id) {
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

    /// A window's inside on the desktop in its points, and its pixels per point.
    fn window(&self, viewport: ViewportId) -> (Rect, f32) {
        let (_, info) = (self.viewports.iter())
            .find(|(window, _)| *window == viewport)
            .expect("the window is open");
        (
            info.inner_rect.unwrap(),
            info.native_pixels_per_point.unwrap(),
        )
    }

    /// Desktop pixel `px` in window `viewport`'s points.
    fn local(&self, viewport: ViewportId, px: Pos2) -> Pos2 {
        let (inner, ppp) = self.window(viewport);
        pos2(px.x / ppp, px.y / ppp) - inner.min.to_vec2()
    }

    /// The `_in` helpers act at desktop pixel `px` through window `viewport`.
    fn move_in(&mut self, viewport: ViewportId, px: Pos2) {
        let pos = self.local(viewport, px);
        self.frame_in(viewport, vec![Event::PointerMoved(pos)]);
    }

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
        self.press(PointerButton::Primary, pressed);
    }

    fn press(&mut self, button: PointerButton, pressed: bool) {
        self.frame(vec![Event::PointerButton {
            pos: self.pointer,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn click_with(&mut self, button: PointerButton, pos: Pos2) {
        self.move_to(pos);
        self.press(button, true);
        self.press(button, false);
    }

    /// The middle of the topmost `text` the main window's last pass drew.
    fn text(&self, text: &str) -> Pos2 {
        let texts = self.texts(text);
        texts
            .first()
            .unwrap_or_else(|| panic!("{text:?} shows"))
            .center()
    }

    fn texts(&self, text: &str) -> Vec<Rect> {
        self.texts_where(|drawn| drawn == text)
    }

    /// Where the main window's last pass drew the texts `matches` picks, topmost first.
    fn texts_where(&self, matches: impl Fn(&str) -> bool) -> Vec<Rect> {
        let mut found = self.drawn_where(matches);
        found.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
        found
    }

    /// Like [`Run::texts_where`], in painting order: those over others last.
    fn drawn_where(&self, matches: impl Fn(&str) -> bool) -> Vec<Rect> {
        let painted = crate::testing::painted(&self.shapes).into_iter();
        let found = painted.filter_map(|shape| match shape {
            Shape::Text(text) if matches(text.galley.text()) => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            _ => None,
        });
        found.collect()
    }

    /// The middle of `text` as a menu shows it: the last painted in front, as a tab's under a
    /// menu is in front too.
    fn in_menu(&self, text: &str) -> Pos2 {
        let over = |rect: &&Rect| {
            let layer = self.ctx.layer_id_at(rect.center());
            layer.is_some_and(|layer| layer.order == Order::Foreground)
        };
        let drawn = self.drawn_where(|drawn| drawn == text);
        let at = drawn.iter().rev().find(over);
        at.unwrap_or_else(|| panic!("a menu shows {text:?}"))
            .center()
    }

    fn click_in_menu(&mut self, text: &str) {
        self.click_with(PointerButton::Primary, self.in_menu(text));
    }

    /// Opens the menus' drawer and lets it slide out.
    fn open_menus(&mut self) {
        let button = pos2(chrome::INSET + chrome::TOOL / 2.0, chrome::BAR / 2.0);
        self.click_with(PointerButton::Primary, button);
        self.frames(12);
    }

    /// Opens the drawer afresh, then the View menu, then its `submenu` if any.
    fn open_view_menu(&mut self, submenu: Option<&str>) {
        self.escape();
        self.open_menus();
        // Its first pass sizes it, unseen.
        self.move_to(self.in_menu(VIEW_MENU));
        self.frames(1);
        if let Some(submenu) = submenu {
            self.move_to(self.in_menu(submenu));
            self.frames(1);
        }
    }

    fn escape(&mut self) {
        self.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        self.frames(1);
    }

    fn choose(&mut self, choice: Choice) {
        let menu = self.dock.menu.as_mut().expect("the dock has the View menu");
        menu.chosen.push(choice);
        self.frame(Vec::new());
    }

    /// Presses at `from` and drags to `to` over a few frames, still holding.
    fn hold(&mut self, from: Pos2, to: Pos2) {
        self.move_to(from);
        self.button(true);
        for step in 1..=8 {
            self.move_to(from + (to - from) * step as f32 / 8.0);
        }
    }

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

/// Points on the first tab of the group at `min`, and on its tab bar's background.
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

    // Out of the bar, clear of targets: the pointer keeps its place on the tab.
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
    // On the handle away from the split.
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
    // A window that can't open has the dock float every group over the main window, as Wayland
    // does from the start.
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

    // A bare press on the bar doesn't move it; a drag does.
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

    // Resizing places it too.
    let grip = pos2(420.0, 300.0) + NEW_SIZE - Vec2::splat(GRIP / 2.0);
    run.drag(grip, grip - vec2(20.0, 20.0));
    assert!(
        matches!(saved(&run, "B"), Place::Main { .. }),
        "{:?}",
        saved(&run, "B")
    );
}

/// A main window point of [`Run::with_windows`] as desktop pixels.
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
    // Once open, A's window has an 8 point border and 30 point title bar, 1.25 pixels a point.
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

/// A point on the bar of A's window from [`Run::tear_off`], clear of its tab and close button.
const WINDOW_BAR: Vec2 = vec2(200.0, 12.0);

#[test]
fn a_window_moved_by_its_bar_stays_a_window_over_the_main_one_and_docks_on_a_target() {
    let mut run = Run::with_window_passes();
    let a = run.tear_off("A", pos2(1200.0, 100.0));
    let (inner, ppp) = run.window(a);
    let from = (inner.min + WINDOW_BAR) * ppp;
    let central = Rect::from_min_max(pos2(0.0, BAR), SCREEN.to_pos2());
    let to = (pos2(100.0, 50.0) + target(central, Zone::Side(Side::Right)).to_vec2()) * 1.5;
    run.move_in(a, from);
    run.button_in(a, from, true);
    for step in 1..=8 {
        run.move_in(a, from + (to - from) * step as f32 / 8.0);
    }
    // Without a new pointer sample it stays put, the pointer where it holds it.
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
    // Without egui's callback for windows of their own, A's shows in the main window, which its
    // close button would close. The main window is maximized, without edges of its own.
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

/// The view panel of the tests' dockspaces.
const VIEW: &str = "View";

/// A tab bar's height in points.
fn tab_bar(run: &Run) -> f32 {
    let spacing = &run.ctx.global_style().spacing;
    spacing.interact_size.y + 2.0 * spacing.item_spacing.y
}

/// Floats the view's group at `min`, `size` points inside.
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
    // egui's first pass of a new area sizes it and takes no input.
    run.frames(1);
    let view = run.dock.view();
    // Clear of A and B, under them, on the view's tab bar.
    for (at, over) in [
        (pos2(600.0, 450.0), true),
        (pos2(100.0, 100.0), false),
        (pos2(600.0, view.min.y - 2.0), false),
    ] {
        let under = run.dock.under(&run.ctx, at);
        assert_eq!(under == Under::View, over, "docked, at {at:?}");
    }
    // Floating: clear of A and B, over them, on its tab bar, on its grip, off it.
    float_view(&mut run, [150.0, 150.0], vec2(300.0, 200.0));
    let content = run.dock.view();
    for (at, over) in [
        (pos2(420.0, 300.0), true),
        (pos2(200.0, 250.0), true),
        (pos2(300.0, content.min.y - 4.0), false),
        (content.max - Vec2::splat(GRIP / 2.0), false),
        (pos2(700.0, 550.0), false),
    ] {
        let under = run.dock.under(&run.ctx, at);
        assert_eq!(under == Under::View, over, "floating, at {at:?}");
    }
}

/// A grab starting with the cursor off a dockspace's view, over egui's UI or out of the window,
/// moves it to the view's center. One starting on the view leaves it there, as does any in a
/// game's dock, whose central area is the game's.
#[test]
fn a_grab_moves_the_cursor_onto_the_view_from_off_it() {
    let mut run = Run::dockspace(Chrome::Drawn);
    // egui's first pass of a new area sizes it and takes no input.
    run.frames(1);
    let center = Some(run.dock.view().center());
    // On the view; on floating A; on the title bar; out of the window.
    for (cursor, at) in [
        (Some(pos2(600.0, 450.0)), None),
        (Some(pos2(100.0, 100.0)), center),
        (Some(pos2(400.0, BAR / 2.0)), center),
        (None, center),
    ] {
        assert_eq!(run.dock.grab_at(&run.ctx, cursor), at, "at {cursor:?}");
    }
    let run = Run::new();
    assert_eq!(run.dock.grab_at(&run.ctx, Some(pos2(100.0, 100.0))), None);
    assert_eq!(run.dock.grab_at(&run.ctx, None), None);
}

/// A layer asks through its UI for the view to have the keys, which the windowed loop takes once
/// after the pass; and reads whether it has them, as the loop told the dock at the frame's start,
/// in a dockspace alone.
#[test]
fn layers_ask_for_the_view_s_focus_and_read_it_through_the_ui() {
    let ctx = crate::egui_context();
    for view in [Some(VIEW), None] {
        let mut dock = Dock::new(None, Chrome::Os, view, false, &[]);
        for told in [true, false] {
            dock.set_view_focused(told);
            let mut read = None;
            let mut output = ctx.run_ui(RawInput::default(), |root| {
                dock.begin(root);
                let mut ui = Ui::new(root, &mut dock);
                read = Some(ui.view_focused());
                ui.focus_view();
                dock.end(root);
            });
            output.textures_delta.clear();
            let focused = told && view.is_some();
            assert_eq!(read, Some(focused), "{view:?}, told {told}");
            assert!(dock.take_focus_asked(), "{view:?}");
            assert!(!dock.take_focus_asked(), "{view:?}, taken once");
        }
    }
}

fn floating_rect(run: &Run, panel: &str) -> Rect {
    let Place::Main { min, size } = run.floating(panel) else {
        panic!(
            "{panel} floats over the main window: {:?}",
            run.floating(panel)
        );
    };
    Rect::from_min_size(min.into(), size.into())
}

#[test]
fn ui_over_the_view_shows_above_it_and_below_groups_floating_over_it() {
    let mut run = Run::dockspace(Chrome::Os);
    run.toolbar = true;
    // A floats over the toolbar's right end.
    let a = run.layout().find("A").unwrap();
    run.dock.layout.floating_mut(a).unwrap().place = Place::Main {
        min: [48.0, 48.0],
        size: [200.0, 200.0],
    };
    run.frames(2);
    let toolbar = run.shown.expect("the view shows");
    assert_eq!(toolbar.min, run.dock.view().min + Vec2::splat(OVER_VIEW));
    let a = floating_rect(&run, "A");
    let y = toolbar.max.y - 4.0;
    let (clear, covered) = (pos2(toolbar.min.x + 4.0, y), pos2(a.min.x + 4.0, y));
    assert!(!a.contains(clear) && a.contains(covered) && toolbar.contains(covered));
    for (at, under) in [
        (clear, Under::Overlay),
        (covered, Under::Other),
        (pos2(600.0, 450.0), Under::View),
    ] {
        assert_eq!(run.dock.under(&run.ctx, at), under, "docked, at {at:?}");
    }

    float_view(&mut run, [200.0, 200.0], vec2(400.0, 300.0));
    let toolbar = run.shown.expect("the view shows");
    assert_eq!(toolbar.min, run.dock.view().min + Vec2::splat(OVER_VIEW));
    let y = toolbar.min.y + 4.0;
    let (clear, covered) = (pos2(a.max.x + 4.0, y), pos2(a.max.x - 4.0, y));
    assert!(!a.contains(clear) && a.contains(covered) && toolbar.contains(covered));
    assert_eq!(run.dock.under(&run.ctx, covered), Under::Overlay);
    // A press on the view raises its group, and the toolbar with it.
    run.move_to(pos2(450.0, 400.0));
    run.button(true);
    run.button(false);
    assert_eq!(run.dock.under(&run.ctx, covered), Under::Overlay);
    // A raised by a press on its bar is over the toolbar.
    run.move_to(bar(a.min) - vec2(200.0, 0.0) + vec2(20.0, 0.0));
    run.button(true);
    run.button(false);
    assert_eq!(run.dock.under(&run.ctx, covered), Under::Other);
    assert_eq!(run.dock.under(&run.ctx, clear), Under::Overlay);
    // A press on the toolbar raises the view's group, as a press in it does.
    run.move_to(clear);
    run.button(true);
    run.button(false);
    assert_eq!(run.dock.under(&run.ctx, covered), Under::Overlay);
}

/// A cover fills the main window below the title bar, in front of the view and the groups
/// floating over it. The pointer doesn't reach them: what's under the cover is the cover's, and a
/// click reaches what it shows. The bar above stays the bar's; once the cover's gone, the view is
/// the view's again.
#[test]
fn a_cover_shows_over_the_dock_below_the_title_bar() {
    let mut run = Run::dockspace(Chrome::Drawn);
    run.cover = true;
    run.frames(2);
    let button = run.covered.as_ref().expect("the cover shows").rect;
    assert!(button.min.y >= BAR, "below the bar: {button:?}");
    let cover = Id::unique("gg dock cover");
    for at in [pos2(600.0, 450.0), pos2(100.0, 100.0), button.center()] {
        assert_eq!(run.dock.under(&run.ctx, at), Under::Other, "at {at:?}");
        let layer = run.ctx.layer_id_at(at).map(|layer| layer.id);
        assert_eq!(layer, Some(cover), "at {at:?}");
    }
    let on_bar = run.ctx.layer_id_at(pos2(400.0, BAR / 2.0));
    assert_ne!(on_bar.map(|layer| layer.id), Some(cover));
    run.click_with(PointerButton::Primary, button.center());
    assert!(run.covered.as_ref().is_some_and(Response::clicked));
    run.cover = false;
    run.frames(2);
    assert_eq!(run.dock.under(&run.ctx, pos2(600.0, 450.0)), Under::View);
}

/// The title bar's settings open below its gear over the view, which the pointer doesn't reach
/// there: what's under them is the settings', and a click reaches the setting.
#[test]
fn the_settings_show_over_the_view_and_take_its_clicks() {
    use crate::chrome::{BUTTON, TOOL};

    let mut run = Run::dockspace(Chrome::Drawn);
    run.setting = true;
    for (panel, x) in [("A", 48.0), ("B", 200.0)] {
        let id = run.layout().find(panel).unwrap();
        run.dock.layout.floating_mut(id).unwrap().place = Place::Main {
            min: [x, 400.0],
            size: [100.0, 100.0],
        };
    }
    run.frames(2);
    let gear = pos2(SCREEN.x - 3.0 * BUTTON - 3.5 * TOOL, BAR / 2.0);
    run.click_with(PointerButton::Primary, gear);
    // Their sizing pass, then the one that shows them.
    run.frames(2);
    let at = run.set.as_ref().expect("the settings show").rect.center();
    assert!(run.dock.view().contains(at), "over the view: {at:?}");
    for panel in ["A", "B"] {
        assert!(!floating_rect(&run, panel).contains(at), "clear of {panel}");
    }
    assert_eq!(run.dock.under(&run.ctx, at), Under::Other);
    run.click_with(PointerButton::Primary, at);
    assert!(run.set.as_ref().is_some_and(Response::clicked));
}

#[test]
fn ui_over_the_view_moves_by_its_offset_but_no_further_than_the_view_s_edges() {
    let mut run = Run::dockspace(Chrome::Os);
    run.toolbar = true;
    run.offset = vec2(50.0, 20.0);
    run.frames(2);
    let view = run.dock.view();
    let toolbar = run.shown.expect("the view shows");
    assert_eq!(toolbar.min, view.min + Vec2::splat(OVER_VIEW) + run.offset);
    run.offset = vec2(10_000.0, 10_000.0);
    run.frames(2);
    assert_eq!(run.shown.expect("the view shows").max, view.max);
}

#[test]
fn ui_over_the_view_and_the_view_s_response_are_there_while_it_shows() {
    let mut run = Run::dockspace(Chrome::Os);
    run.toolbar = true;
    run.frames(2);
    let view = run.view.clone().expect("the view shows");
    assert_eq!(view.rect, run.dock.view());
    // In pixels, as the frame renders it.
    assert_eq!(run.viewport, Some(Viewport::from_points(view.rect, 1.0)));
    // Hovered on the view, not on the toolbar over it.
    run.move_to(pos2(600.0, 450.0));
    assert!(run.view.as_ref().unwrap().hovered());
    run.move_to(run.shown.unwrap().center());
    assert!(!run.view.as_ref().unwrap().hovered());
    // Nowhere while a tab shows over it.
    let view = run.layout().find(VIEW).unwrap();
    let a = run.layout().find("A").unwrap();
    assert!(run.dock.layout.dock(a, view, Zone::Center));
    run.frames(2);
    assert!(run.shown.is_none() && run.view.is_none() && run.viewport.is_none());
    // Nor in a game's dock, whose view is the central area, rendered all the same.
    let mut run = Run::new();
    run.toolbar = true;
    run.frames(2);
    assert!(run.shown.is_none() && run.view.is_none());
    let central = Viewport::from_points(run.dock.view(), 1.0);
    assert!(central.size[0] > 0 && run.viewport == Some(central));
}

#[test]
fn a_press_on_the_view_by_its_tab_is_the_view_s() {
    let mut run = Run::dockspace(Chrome::Os);
    let view = run.dock.view();
    // Within egui's reach of the view's tab, whose drag would pull it from its group.
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
    // Within the 600 by 400 points from the dock's top left that egui's first pass of a new
    // area keeps it in.
    let min = pos2(150.0, 150.0);
    float_view(&mut run, min.into(), NEW_SIZE);
    assert_eq!(
        run.floating(VIEW),
        Place::Main {
            min: min.into(),
            size: NEW_SIZE.into()
        }
    );
    // The center of A's window, in main window points.
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

/// How far a docked group's edge keeps from a shared line, for the handle, in points.
const GAP: f32 = 4.0;

/// Whether `panel`'s tab or contents show.
fn shows(run: &Run, panel: &str) -> bool {
    let contents = format!("In {panel}");
    !run.texts_where(|text| text == panel || text == contents)
        .is_empty()
}

#[test]
fn a_closed_panel_takes_no_tab_or_room_from_the_frame_it_closes_and_comes_back_where_it_was() {
    let mut run = Run::with_menu();
    let a = run.layout().find("A").unwrap();
    assert!(run.dock.layout.dock(a, CENTRAL, Zone::Side(Side::Left)));
    run.frames(2);
    let docked = run.docked("A");
    run.choose(Choice::Close("A".to_owned()));
    run.frames(1);
    assert!(!shows(&run, "A"));
    let arranged = &run.dock.arrangement;
    assert!(arranged.strips.is_empty(), "{:?}", arranged.strips);
    assert_eq!(arranged.central, arranged.area);
    assert_eq!(run.layout().find("A"), Some(a));
    run.choose(Choice::Open("A".to_owned()));
    run.frames(1);
    assert_eq!(run.docked("A"), docked);
    assert!(shows(&run, "A"));
}

#[test]
fn a_floating_group_whose_only_panel_closes_hides_where_it_was_and_is_no_target() {
    let mut run = Run::with_menu();
    let place = run.floating("A");
    run.choose(Choice::Close("A".to_owned()));
    run.frames(1);
    assert!(!shows(&run, "A"));
    let a = Rect::from_min_size(pos2(48.0, 48.0), NEW_SIZE);
    run.drag(bar(pos2(72.0, 72.0)), target(a, Zone::Center));
    assert_ne!(run.layout().find("B"), run.layout().find("A"));
    run.choose(Choice::Open("A".to_owned()));
    run.frames(1);
    assert_eq!(run.floating("A"), place);
    assert!(shows(&run, "A"));
}

#[test]
fn a_torn_off_window_whose_only_panel_closes_closes_and_opens_again_where_it_was() {
    let mut run = Run::with_dock(Dock::new(None, Chrome::Drawn, None, true, &[]));
    run.open_windows();
    run.run_window_passes();
    let a = run.tear_off("A", pos2(1200.0, 100.0));
    let place = run.floating("A");
    let shown = |run: &Run| run.windows.iter().any(|&(id, _)| id == a);
    assert!(shown(&run));
    run.choose(Choice::Close("A".to_owned()));
    run.frames(1);
    assert!(!shown(&run));
    assert_eq!(run.floating("A"), place);
    // While it's closed, a group dropped where it was doesn't dock in it, though egui still has
    // it there, as the frame it closes.
    let (inner, ppp) = run.window(a);
    let center = inner.center() * ppp;
    let to = pos2(center.x / 1.5 - 100.0, center.y / 1.5 - 50.0);
    run.drag(bar(pos2(72.0, 72.0)), to);
    assert_ne!(run.layout().find("B"), run.layout().find("A"));
    // The engine closed it and reopens it where it was, at the main window's scale.
    run.viewports.retain(|&(id, _)| id != a);
    run.choose(Choice::Open("A".to_owned()));
    run.frames(1);
    let Place::Window { position, .. } = place else {
        panic!("A is in a window of its own: {place:?}");
    };
    let at = pos2(position[0] as f32 / 1.5, position[1] as f32 / 1.5);
    let asked = run.windows.iter().find(|&&(id, _)| id == a);
    assert_eq!(asked.and_then(|(_, builder)| builder.position), Some(at));
    assert_eq!(run.floating("A"), place);
}

#[test]
fn dragging_the_only_tab_showing_moves_its_group_with_its_closed_panels() {
    let mut run = Run::with_menu();
    let (a, b) = (
        run.layout().find("A").unwrap(),
        run.layout().find("B").unwrap(),
    );
    assert!(run.dock.layout.dock(b, a, Zone::Center));
    run.choose(Choice::Close("B".to_owned()));
    run.frames(1);
    let from = first_tab(pos2(48.0, 48.0));
    run.drag(from, from + vec2(100.0, 50.0));
    assert_eq!(run.layout().floating.len(), 1);
    assert_eq!(run.layout().find("B"), run.layout().find("A"));
    assert!(
        matches!(
            run.floating("A"),
            Place::Main {
                min: [148.0, 98.0],
                ..
            }
        ),
        "{:?}",
        run.floating("A")
    );
}

#[test]
fn with_its_docked_panels_closed_and_the_view_floating_the_empty_dock_takes_the_view_back() {
    let mut run = Run::with_dock(Dock::new(None, Chrome::Os, Some(VIEW), true, &[]));
    let view = run.layout().find(VIEW).unwrap();
    let a = run.layout().find("A").unwrap();
    assert!(run.dock.layout.dock(a, view, Zone::Side(Side::Right)));
    float_view(&mut run, [150.0, 150.0], NEW_SIZE);
    run.choose(Choice::Close("A".to_owned()));
    run.frames(1);
    let area = run.dock.arrangement.area;
    assert_eq!(run.dock.arrangement.central, area);
    run.drag(first_tab(pos2(150.0, 150.0)), area.center());
    assert_eq!(run.docked(VIEW), area);
    // A returns beside it, taking a quarter.
    run.choose(Choice::Open("A".to_owned()));
    run.frames(1);
    let quarter = area.min.x + area.width() / 4.0;
    assert_eq!(run.docked("A"), area.with_max_x(quarter - GAP));
    assert_eq!(run.docked(VIEW), area.with_min_x(quarter + GAP));
}

#[test]
fn a_tabs_right_click_closes_its_panel_with_the_view_menu_and_never_the_views() {
    for menu in [false, true] {
        let mut run = Run::with_dock(Dock::new(None, Chrome::Os, None, menu, &[]));
        run.click_with(PointerButton::Secondary, first_tab(pos2(48.0, 48.0)));
        run.frames(1);
        assert_eq!(
            run.texts("Close").is_empty(),
            !menu,
            "with the menu: {menu}"
        );
        if menu {
            run.click_in_menu("Close");
            run.frames(1);
            assert!(run.layout().is_closed("A"));
            assert!(!shows(&run, "A"));
        }
    }
    let mut run = Run::with_dock(Dock::new(None, Chrome::Os, Some(VIEW), true, &[]));
    // The view's tab; the View menu is in the closed drawer.
    let tab = run.text(VIEW);
    run.click_with(PointerButton::Secondary, tab);
    run.frames(1);
    assert!(run.texts("Close").is_empty());
}

#[test]
fn a_right_press_dragged_off_a_tab_starts_no_drag() {
    let mut run = Run::new();
    let place = run.floating("A");
    let from = first_tab(pos2(48.0, 48.0));
    run.move_to(from);
    run.press(PointerButton::Secondary, true);
    for step in 1..=8 {
        run.move_to(from + vec2(30.0, 20.0) * step as f32);
        assert!(run.dock.drag.is_none(), "at step {step}");
    }
    run.press(PointerButton::Secondary, false);
    run.frames(2);
    assert_eq!(run.floating("A"), place);
    assert_eq!(run.layout().floating.len(), 2);
}

#[test]
fn the_view_menu_follows_the_layers_and_lists_their_panels_alphabetically_but_the_view() {
    let dock = Dock::new(None, Chrome::Os, Some("Viewport"), true, &[]);
    let mut run = Run::showing(dock, &["b", "C", "A"]);
    run.menus = vec!["File", "Edit"];
    for panel in ["A", "b", "C"] {
        assert!(run.dock.layout.close(panel));
    }
    run.frames(2);
    run.open_menus();
    let y = |run: &Run, text| run.texts(text)[0].min.y;
    assert!(y(&run, "File") < y(&run, "Edit") && y(&run, "Edit") < y(&run, VIEW_MENU));
    run.open_view_menu(None);
    assert!(y(&run, "A") < y(&run, "b") && y(&run, "b") < y(&run, "C"));
    assert!(y(&run, "C") < y(&run, "Reset layout"));
    assert_eq!(run.texts("Viewport").len(), 1, "the view's tab alone");
    run.click_in_menu("b");
    run.frames(1);
    assert_eq!(run.layout().closed, ["A", "C"]);
    assert!(shows(&run, "b"));
    run.open_view_menu(None);
    run.click_in_menu("b");
    run.frames(1);
    assert!(run.layout().is_closed("b"));
    // Reset: they all show as new panels, and the view fills the dock again.
    let place = Place::Main {
        min: [300.0, 300.0],
        size: NEW_SIZE.into(),
    };
    run.dock.layout.detach("Viewport", place);
    run.dock.layout.prune();
    run.frames(2);
    run.open_view_menu(None);
    run.click_in_menu("Reset layout");
    // egui's first pass of a new group's area sizes it, unseen.
    run.frames(2);
    assert!(run.layout().closed.is_empty());
    assert_eq!(run.docked("Viewport"), run.dock.arrangement.area);
    for panel in ["A", "b", "C"] {
        assert!(
            run.layout()
                .floating(run.layout().find(panel).unwrap())
                .is_some()
        );
        assert!(shows(&run, panel), "{panel}");
    }
}

#[test]
fn a_layers_own_view_menu_gets_the_docks_items_after_its_own() {
    let mut run = Run::with_menu();
    run.menus = vec![VIEW_MENU];
    run.frames(2);
    run.open_menus();
    assert_eq!(run.texts(VIEW_MENU).len(), 1);
    run.open_view_menu(None);
    let y = |run: &Run, text| run.texts(text)[0].min.y;
    assert!(y(&run, "View item") < y(&run, "Save layout as…"));
    assert!(y(&run, "View item") < y(&run, "Reset layout"));
}

#[test]
fn layouts_save_under_names_load_and_delete_from_the_view_menu() {
    let dir = Scratch::new("dock-view-menu");
    let file = dir.0.join("layout.ron");
    let mut run = Run::with_dock(Dock::new(Some(file), Chrome::Os, None, true, &[]));
    let saved = |run: &Run| run.dock.menu.as_ref().unwrap().saved.clone();
    run.choose(Choice::Close("B".to_owned()));
    // A name typed in the field is trimmed and saves the layout on Save.
    run.open_view_menu(Some("Save layout as…"));
    run.click_in_menu("Name");
    run.frame(vec![Event::Text(" Mine ".to_owned())]);
    run.click_in_menu("Save");
    run.frames(1);
    assert_eq!(saved(&run), ["Mine"]);
    assert!(dir.0.join("layouts/Mine.ron").is_file());
    // B shows again; loading the layout closes it again.
    run.choose(Choice::Open("B".to_owned()));
    assert!(!run.layout().is_closed("B"));
    run.open_view_menu(Some("Load layout"));
    run.click_in_menu("Mine");
    run.frames(1);
    assert!(run.layout().is_closed("B"));
    assert!(!shows(&run, "B"));
    run.open_view_menu(Some("Delete layout"));
    run.click_in_menu("Mine");
    run.frames(1);
    assert!(saved(&run).is_empty());
    assert!(!dir.0.join("layouts/Mine.ron").exists());
    // A refused name says why by the field.
    run.open_view_menu(Some("Save layout as…"));
    run.click_in_menu("Name");
    run.frame(vec![Event::Text("con".to_owned())]);
    run.click_in_menu("Save");
    run.frames(1);
    let refused = |text: &str| text.contains("which Windows keeps for a device");
    assert_eq!(run.texts_where(refused).len(), 1);
    assert!(saved(&run).is_empty());
}

#[test]
fn a_layout_that_cant_be_used_is_refused_and_the_dock_stays_as_it_was() {
    let dir = Scratch::new("dock-refused");
    fs::create_dir_all(dir.0.join("layouts")).expect("a directory");
    fs::write(dir.0.join("layouts/Broken.ron"), "not a layout").expect("the file is written");
    let file = dir.0.join("layout.ron");
    let mut run = Run::with_dock(Dock::new(Some(file), Chrome::Os, None, true, &[]));
    assert_eq!(run.dock.menu.as_ref().unwrap().saved, ["Broken"]);
    let layout = run.layout().clone();
    run.choose(Choice::Load("Broken".to_owned()));
    run.frames(1);
    assert_eq!(run.layout(), &layout);
    assert!(shows(&run, "A") && shows(&run, "B"));
}

#[test]
fn without_a_layout_file_the_view_menu_saves_loads_and_deletes_none_but_resets() {
    let mut run = Run::with_menu();
    // As though a layout were saved; the submenus would list it.
    run.dock.menu.as_mut().unwrap().saved = vec!["Mine".to_owned()];
    run.choose(Choice::Close("A".to_owned()));
    for submenu in ["Save layout as…", "Load layout", "Delete layout"] {
        run.open_view_menu(Some(submenu));
        run.frames(1);
        assert!(run.texts("Name").is_empty(), "{submenu}");
        assert!(run.texts("Mine").is_empty(), "{submenu}");
        run.escape();
    }
    run.open_view_menu(None);
    run.click_in_menu("Reset layout");
    assert!(run.layout().closed.is_empty());
}

/// Where an editor docks three panels around the view: beside it a quarter of its place, beside
/// another panel half.
const EDITOR: [(&str, Docked); 3] = [
    ("Inspector", Docked::RightOf(VIEW, 0.25)),
    ("Entities", Docked::LeftOf(VIEW, 0.25)),
    ("Performance", Docked::Below("Inspector", 0.5)),
];

#[test]
fn a_first_run_docks_the_panels_as_the_app_says_whatever_order_they_show_in() {
    let dock = Dock::new(None, Chrome::Os, Some(VIEW), true, &EDITOR);
    let run = Run::showing(dock, &["Performance", "Entities", "Inspector"]);
    assert!(run.layout().floating.is_empty());
    let area = run.dock.arrangement.area;
    // The Inspector takes a quarter of the window, Entities a quarter of the rest.
    let right = area.max.x - area.width() / 4.0;
    let inspector = run.docked("Inspector");
    assert_eq!(inspector.min.x, right + GAP);
    let left = area.min.x + (right - GAP - area.min.x) / 4.0;
    assert_eq!(run.docked("Entities").max.x, left - GAP);
    let performance = run.docked("Performance");
    assert_eq!(performance.x_range(), inspector.x_range());
    assert!(performance.min.y > inspector.max.y);
}

/// The title bar's buttons hide the panels docked left of, below and right of the view, which
/// then neither show nor take room (the view takes it), and show them again where they were. A
/// panel opened from the View menu, where a hidden one is unchecked, or a group docked beside the
/// view, shows its side again. A side without panels has its button off.
#[test]
fn the_title_bars_buttons_hide_and_show_the_panels_around_the_view() {
    let editor = [&EDITOR[..], &[("Assets", Docked::Below(VIEW, 0.25))]].concat();
    let dock = Dock::new(None, Chrome::Drawn, Some(VIEW), true, &editor);
    let showing = ["Performance", "Entities", "Inspector", "Assets", "Log"];
    let mut run = Run::showing(dock, &showing);
    let panels = ["Entities", "Inspector", "Performance", "Assets"];
    let (docked, view) = (panels.map(|panel| run.docked(panel)), run.docked(VIEW));
    let area = run.dock.arrangement.area;
    // Left, bottom, then right side's, left of the window's buttons.
    let button = |n: f32| {
        let x = SCREEN.x - 3.0 * chrome::BUTTON - (n - 0.5) * chrome::TOOL;
        pos2(x, chrome::BAR / 2.0)
    };
    let (left, bottom, right) = (button(3.0), button(2.0), button(1.0));
    run.click_with(PointerButton::Primary, left);
    run.frames(1);
    assert_eq!(run.layout().hidden, [Side::Left]);
    assert!(!shows(&run, "Entities") && shows(&run, "Inspector"));
    assert_eq!(run.docked(VIEW).min.x, area.min.x);
    run.click_with(PointerButton::Primary, right);
    run.frames(1);
    assert!(!shows(&run, "Inspector") && !shows(&run, "Performance"));
    assert!(shows(&run, "Assets"));
    run.click_with(PointerButton::Primary, bottom);
    run.frames(1);
    assert_eq!(run.layout().hidden, [Side::Left, Side::Right, Side::Bottom]);
    assert!(!shows(&run, "Assets"));
    assert_eq!(run.docked(VIEW), area);
    for button in [left, bottom, right] {
        run.click_with(PointerButton::Primary, button + vec2(1.0, 0.0));
    }
    run.frames(1);
    assert!(run.layout().hidden.is_empty());
    assert_eq!(panels.map(|panel| run.docked(panel)), docked);
    assert_eq!(run.docked(VIEW), view);

    // Hidden Entities is unchecked in the View menu; a click there opens it.
    run.click_with(PointerButton::Primary, left);
    run.frames(1);
    run.open_view_menu(None);
    run.click_in_menu("Entities");
    run.escape();
    run.frames(1);
    assert!(!run.layout().is_closed("Entities"));
    assert!(run.layout().hidden.is_empty() && shows(&run, "Entities"));
    run.click_with(PointerButton::Primary, left + vec2(1.0, 0.0));
    run.frames(1);
    // Floating Log's group onto the left of the view.
    let Place::Main { min, .. } = run.floating("Log") else {
        panic!("Log floats over the main window");
    };
    let to = target(run.docked(VIEW), Zone::Side(Side::Left));
    run.drag(bar(min.into()), to);
    assert!(run.layout().hidden.is_empty());
    assert!(shows(&run, "Entities") && shows(&run, "Log"));
    assert_eq!(run.layout().side("Log"), Some(Side::Left));

    // With Assets closed, no panel is below the view; its button hides nothing.
    run.choose(Choice::Close("Assets".to_owned()));
    run.frames(1);
    run.click_with(PointerButton::Primary, bottom + vec2(2.0, 0.0));
    run.frames(1);
    assert!(run.layout().hidden.is_empty());
}

#[test]
fn a_layout_saved_without_a_panel_docks_it_as_the_app_says() {
    let dir = Scratch::new("dock-older");
    let file = dir.0.join("layout.ron");
    let mut older = Layout::new(Some(VIEW));
    let place = Place::Main {
        min: [48.0, 48.0],
        size: NEW_SIZE.into(),
    };
    older.add("Entities".to_owned(), place, &EDITOR);
    file::save(&file, &older).expect("the layout is saved");
    let dock = Dock::new(Some(file), Chrome::Os, Some(VIEW), true, &EDITOR);
    let run = Run::showing(dock, &["Entities", "Inspector"]);
    assert!(run.layout().floating.is_empty());
    assert!(run.docked("Inspector").min.x > run.docked(VIEW).max.x);
    assert!(run.docked("Entities").max.x < run.docked(VIEW).min.x);
}

fn ids(layout: &Layout) -> Vec<NodeId> {
    fn walk(node: &Node, ids: &mut Vec<NodeId>) {
        match node {
            Node::Split { id, children, .. } => {
                ids.push(*id);
                walk(&children[0], ids);
                walk(&children[1], ids);
            }
            Node::Tabs(tabs) => ids.push(tabs.id),
            Node::Central => {}
        }
    }
    let mut ids = Vec::new();
    walk(&layout.main, &mut ids);
    ids.extend(layout.floating.iter().map(|f| f.tabs.id));
    ids
}

#[test]
fn node_ids_keep_increasing_through_a_reset_and_a_load() {
    let dir = Scratch::new("dock-ids");
    let file = dir.0.join("layout.ron");
    let mut run = Run::with_dock(Dock::new(Some(file), Chrome::Os, Some(VIEW), true, &[]));
    run.choose(Choice::Save("Mine".to_owned()));
    let before = ids(run.layout());
    run.choose(Choice::Reset);
    let reset = ids(run.layout());
    let after = |earlier: &[NodeId], later: &[NodeId]| {
        later
            .iter()
            .all(|id| earlier.iter().all(|earlier| id > earlier))
    };
    assert!(after(&before, &reset), "{before:?}, then {reset:?}");
    run.choose(Choice::Load("Mine".to_owned()));
    let loaded = ids(run.layout());
    assert_eq!(loaded.len(), before.len());
    assert!(after(&reset, &loaded), "{reset:?}, then {loaded:?}");
}

#[test]
fn a_panel_a_layer_places_floats_there_once_it_shows_while_the_rest_cascade() {
    let mut dock = Dock::new(None, Chrome::Os, None, false, &[]);
    dock.floats
        .push(("B", Align2::RIGHT_TOP, vec2(-12.0, 48.0)));
    let run = Run::showing(dock, &["A", "B"]);
    let place = |panel| {
        let id = run.layout().find(panel).expect("in the dock");
        run.dock.layout.floating(id).expect("floats").place
    };
    let min = [SCREEN.x - NEW_SIZE.x - 12.0, 48.0];
    let size = NEW_SIZE.into();
    assert_eq!(place("B"), Place::Main { min, size });
    let min = [48.0, 48.0];
    assert_eq!(place("A"), Place::Main { min, size });
}
