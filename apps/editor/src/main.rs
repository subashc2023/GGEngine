//! The editor: the game's view in a panel among the editor's own, in a dock that fills its window.
//! Until the tool API brings it a game, the view shows a world of the editor's own, empty but for a
//! grid, through an orthographic camera, as 2D games see theirs.

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{ViewportCommand, ViewportId};
use gg_engine::math::{Mat4, Vec2};
use gg_engine::{
    App, Blend, Camera, Context, Event, Layer, Mesh, OrthographicCameraController, PerfPanel,
    Projection, Shader, Ui,
};

fn main() -> ExitCode {
    gg_engine::run(app())
}

fn app() -> App {
    let app = App::new("Editor").custom_title_bar().view_panel("Viewport");
    app.layer(Editor::new()).overlay(PerfPanel)
}

/// The view's corners in clip space, which `shaders/grid.slang` finds on the grid's plane.
const CORNERS: [Vec2; 4] = [
    Vec2::new(-1.0, -1.0),
    Vec2::new(1.0, -1.0),
    Vec2::new(1.0, 1.0),
    Vec2::new(-1.0, 1.0),
];

struct Editor {
    camera: OrthographicCameraController,
    /// The grid's mesh and shader, once attached.
    grid: Option<(Mesh<Vec2>, Shader<Vec2, ()>)>,
}

impl Editor {
    fn new() -> Self {
        // Over the origin, seeing 10 units from the view's bottom to its top.
        let camera = Camera {
            projection: Projection::Orthographic {
                height: 10.0,
                near: -1.0,
                far: 1.0,
            },
            ..Camera::default()
        };
        Self {
            camera: OrthographicCameraController::new(camera),
            grid: None,
        }
    }
}

impl Layer for Editor {
    fn attach(&mut self, cx: &mut Context) {
        cx.set_clear_color([0.02, 0.02, 0.025, 1.0]);
        // A grid shader that doesn't compile stops the editor, with what went wrong.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders/grid.slang");
        let shader = cx.add_shader("grid", path, Blend::Alpha);
        let mesh = cx.add_mesh("grid", &CORNERS, &[0, 1, 2, 0, 2, 3]);
        let mesh = mesh.expect("the view's corners are two whole triangles");
        self.grid = Some((mesh, shader.expect("the grid shader compiles")));
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.camera.update(cx, dt);
        if let Some((mesh, shader)) = self.grid {
            // The shader places the corners itself, so the transform goes unread.
            cx.draw(mesh, shader, Mat4::IDENTITY, ());
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        self.camera.event(cx, event)
    }

    fn ui(&mut self, _: &mut Context, ui: &mut Ui<'_>) {
        ui.menu("File", |ui| {
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
    }
}
