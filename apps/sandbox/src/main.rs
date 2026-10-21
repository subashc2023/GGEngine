//! Development playground: test scenes and debug tooling for engine work.

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Duration;

use gg_engine::{Context, Event, Layer, egui};
use tracing::info;

fn main() -> ExitCode {
    gg_engine::run(gg_engine::App::new("Sandbox").layer(Sandbox::new()))
}

/// Clears to a color that drifts slowly, so that a running sandbox visibly animates, and shows a
/// panel with frame times and the color, which can be held or picked. Logs the key and mouse
/// presses that reach it, which those on the panel don't.
struct Sandbox {
    time: Duration,
    drift: bool,
    /// The clear color, in linear RGB.
    color: [f32; 3],
    /// The last frame's time, shown as it is: smoothing would hide uneven frames.
    frame_time: Duration,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            time: Duration::ZERO,
            drift: true,
            color: [0.0; 3],
            frame_time: Duration::ZERO,
        }
    }
}

impl Layer for Sandbox {
    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.time += dt;
        self.frame_time = dt;
        if self.drift {
            let t = self.time.as_secs_f32() * 0.4;
            let wave = |phase: f32| 0.02 + 0.06 * (0.5 + 0.5 * (t + phase).sin());
            self.color = [wave(0.0), wave(2.1), wave(4.2)];
        }
        let [r, g, b] = self.color;
        cx.set_clear_color([r, g, b, 1.0]);
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

    fn ui(&mut self, cx: &mut Context, ui: &egui::Context) {
        egui::Window::new("Sandbox").show(ui, |ui| {
            let ms = self.frame_time.as_secs_f32() * 1000.0;
            let fps = if ms > 0.0 { 1000.0 / ms } else { 0.0 };
            ui.label(format!("{ms:.2} ms a frame, {fps:.0} fps"));
            ui.checkbox(&mut self.drift, "Drift");
            ui.horizontal(|ui| {
                ui.label("Clear color");
                // Picking a color holds it.
                if ui.color_edit_button_rgb(&mut self.color).changed() {
                    self.drift = false;
                }
            });
        });
        let [r, g, b] = self.color;
        cx.set_clear_color([r, g, b, 1.0]);
    }
}
