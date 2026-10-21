//! Development playground: test scenes and debug tooling for engine work.

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::egui::{ViewportCommand, ViewportId};
use gg_engine::{Context, Event, Key, Layer, Mesh, Ui, Vertex};
use tracing::info;

fn main() -> ExitCode {
    let app = gg_engine::App::new("Sandbox").custom_title_bar();
    gg_engine::run(app.layer(Sandbox::new()))
}

/// A triangle in red, green and blue, pointing up, in clip space.
const TRIANGLE: [Vertex; 3] = [
    Vertex {
        position: [0.0, -0.5, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    },
    Vertex {
        position: [0.5, 0.5, 0.0],
        color: [0.0, 1.0, 0.0, 1.0],
    },
    Vertex {
        position: [-0.5, 0.5, 0.0],
        color: [0.0, 0.0, 1.0, 1.0],
    },
];

/// Draws a triangle in the game's view, over a color that drifts slowly, so that a running
/// sandbox visibly animates, and shows two panels to dock: frame times, and the color, which can
/// be held or picked. Logs the key and mouse presses that reach it, which those on the panels
/// don't. It polls input too: holding Space pauses the drift, and a panel shows where the cursor
/// is. The title bar's File menu quits.
struct Sandbox {
    time: Duration,
    drift: bool,
    /// The clear color, in linear RGB.
    color: [f32; 3],
    /// The last frame's time, shown as it is: smoothing would hide uneven frames.
    frame_time: Duration,
    /// Added once the sandbox attaches.
    triangle: Option<Mesh>,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            time: Duration::ZERO,
            drift: true,
            color: [0.0; 3],
            frame_time: Duration::ZERO,
            triangle: None,
        }
    }
}

impl Layer for Sandbox {
    fn attach(&mut self, cx: &mut Context) {
        let triangle = cx.add_mesh("triangle", &TRIANGLE, &[0, 1, 2]);
        self.triangle = Some(triangle.expect("the triangle is a whole one, of its own vertices"));
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        if let Some(triangle) = self.triangle {
            cx.draw(triangle);
        }
        self.frame_time = dt;
        if self.drift && !cx.input().key_down(Key::Space) {
            self.time += dt;
            let t = self.time.as_secs_f32() * 0.4;
            let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * (t + phase).sin());
            self.color = [wave(0.0), wave(2.1), wave(4.2)];
        }
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        match *event {
            Event::Key {
                key,
                pressed: true,
                repeat: false,
            } => info!(?key, "Key pressed"),
            Event::MouseButton {
                button,
                pressed: true,
            } => info!(?button, "Mouse button pressed"),
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        ui.menu("File", |ui| {
            if ui.button("Quit").clicked() {
                ui.ctx()
                    .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
        });
        ui.panel("Stats", |ui| {
            let ms = self.frame_time.as_secs_f32() * 1000.0;
            let fps = if ms > 0.0 { 1000.0 / ms } else { 0.0 };
            ui.label(format!("{ms:.2} ms a frame, {fps:.0} fps"));
            ui.label(match cx.input().cursor() {
                Some([x, y]) => format!("Cursor at {x:.0}, {y:.0}"),
                None => "Cursor outside the window".to_owned(),
            });
        });
        ui.panel("Clear color", |ui| {
            ui.checkbox(&mut self.drift, "Drift");
            ui.horizontal(|ui| {
                ui.label("Color");
                // Picking a color holds it.
                if ui.color_edit_button_rgb(&mut self.color).changed() {
                    self.drift = false;
                }
            });
        });
        // After the panel, which may have changed it.
        let [r, g, b] = self.color;
        cx.set_clear_color([r, g, b, 1.0]);
    }
}
