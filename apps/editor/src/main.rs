//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, empty but for a
//! grid, through the editor's camera, orthographic at first, as 2D games see theirs, with a
//! toolbar and a gizmo over it; the Inspector edits the world, and undoes and redoes its edits.

mod camera;
mod grid;
mod overlay;

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{Align2, CursorIcon, ViewportCommand, ViewportId};
use gg_engine::math::Vec3;
use gg_engine::{App, Context, Docked, Event, Inspector, Key, Layer, PerfPanel, Ui};

use camera::{Move, OrbitCamera, Screen, Side, Tool};
use grid::Grid;
use overlay::Toolbar;

fn main() -> ExitCode {
    gg_engine::run(app())
}

fn app() -> App {
    let app = App::new("Editor").custom_title_bar().view_panel("Viewport");
    let app = app
        .dock("Inspector", Docked::RightOf("Viewport"))
        .dock("Entities", Docked::LeftOf("Viewport"))
        .dock("Performance", Docked::Below("Inspector"));
    let app = app.layer(Editor::new());
    app.overlay(PerfPanel).overlay(Inspector::default())
}

struct Editor {
    camera: OrbitCamera,
    toolbar: Toolbar,
    /// The grid, once attached.
    grid: Option<Grid>,
}

impl Editor {
    fn new() -> Self {
        Self {
            camera: OrbitCamera::new(),
            toolbar: Toolbar::default(),
            grid: None,
        }
    }
}

impl Layer for Editor {
    fn attach(&mut self, cx: &mut Context) {
        cx.set_clear_color([0.02, 0.02, 0.025, 1.0]);
        self.grid = Some(Grid::new(cx));
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(dt);
        let plane = grid::shown(&self.camera.camera(), self.toolbar.plane);
        self.camera.settle(plane.normal());
        let camera = self.camera.camera();
        cx.set_camera(camera);
        // Fitted to the view as the last frame showed it, which has nowhere to fit it at first.
        if self.toolbar.grid
            && let (Some(grid), Some(view)) = (&self.grid, cx.view())
        {
            grid.draw(cx, &camera, view, self.toolbar.plane);
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let input = cx.input();
        let screen = Screen {
            view: cx.view(),
            scale: cx.window().map_or(1.0, |window| window.scale() as f32),
            alt: input.key_down(Key::AltLeft) || input.key_down(Key::AltRight),
        };
        self.camera.event(event, screen, self.toolbar.tool)
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        ui.menu("File", |ui| {
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        let Self {
            camera, toolbar, ..
        } = self;
        ui.over_view("toolbar", Align2::LEFT_TOP, |ui| toolbar.show(ui, camera));
        // The side the camera looks squarely from, which the gizmo names.
        let seen = camera.camera();
        let square = cx.view().is_some_and(|view| grid::square(&seen, view));
        let aligned = square
            .then(|| Side::facing(seen.rotation * Vec3::NEG_Z))
            .flatten();
        ui.over_view("gizmo", Align2::RIGHT_TOP, |ui| {
            overlay::gizmo(ui, camera, aligned);
        });
        // The cursor over the view says what the left button does, and while a drag goes on,
        // what it does, wherever the cursor is.
        let hovered = ui.view().is_some_and(|view| view.hovered());
        let icon = match camera.moving() {
            Some(Move::Pan) => Some(CursorIcon::Move),
            Some(Move::Orbit) => Some(CursorIcon::Grabbing),
            Some(Move::Look) => Some(CursorIcon::Crosshair),
            Some(Move::Dolly) => Some(CursorIcon::ZoomIn),
            None if hovered => match toolbar.tool {
                Tool::Select => None,
                Tool::Pan => Some(CursorIcon::Move),
                Tool::Orbit => Some(CursorIcon::Grab),
            },
            None => None,
        };
        if let Some(icon) = icon {
            ui.egui().set_cursor_icon(icon);
        }
    }
}
