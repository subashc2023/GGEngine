//! Camera controllers: what steers the game's camera from the keyboard and mouse.

use std::f32::consts::FRAC_PI_2;
use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::math::{self, Quat, Vec3};
use gg_renderer::{Camera, Projection};

use crate::{Context, Event, Key, Scroll, Window};

/// How fast the camera moves, in the view's heights a second.
const SPEED: f32 = 0.5;
/// How fast it turns, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;
/// How much a line of the mouse wheel zooms.
const ZOOM: f32 = 1.1;
/// What a line of the wheel is in points, for scrolls that a touchpad reports in pixels: 40, as
/// egui counts it on the desktop (`InputOptions::line_scroll_speed`).
const POINTS_PER_LINE: f64 = 40.0;

/// Steers an orthographic camera from the keyboard and mouse wheel, as 2D games and tools do.
/// W, A, S and D move it along its own axes, half the view's height a second, so that it crosses
/// the screen as fast however far it's zoomed; Q and E turn it left and right, a quarter turn a
/// second, if [`rotation`](Self::rotation) is set; and the mouse wheel zooms about the view's
/// center, 1.1 times a line, between a hundredth and a hundred times the height it started at.
///
/// A layer keeps one, and calls [`update`](Self::update) from its own update and
/// [`event`](Self::event) from its own event handler, as `self.camera.event(cx, event)?`.
pub struct OrthographicCameraController {
    /// The camera it steers, which the layer can place, turn or zoom itself too.
    pub camera: Camera,
    /// Whether Q and E turn the camera; at first they don't.
    pub rotation: bool,
    /// The least and most of the world the view's height zooms between.
    heights: [f32; 2],
}

impl OrthographicCameraController {
    pub fn new(camera: Camera) -> Self {
        let Projection::Orthographic { height, .. } = camera.projection;
        Self {
            camera,
            rotation: false,
            heights: [height / 100.0, height * 100.0],
        }
    }

    /// Moves and turns the camera by the keys held, for `dt`, and makes it the game's view's
    /// camera.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let held = |key| if cx.input().key_down(key) { 1.0 } else { 0.0 };
        if self.rotation {
            let turn = held(Key::KeyQ) - held(Key::KeyE);
            // About the axis it looks along, counterclockwise as it sees it; normalized, so that
            // rounding doesn't build up frame after frame.
            let by = Quat::from_rotation_z(turn * TURN_SPEED * seconds);
            self.camera.rotation = (self.camera.rotation * by).normalize();
        }
        // Along the camera's own axes, so that W moves up the screen however it's turned, and as
        // fast diagonally as straight.
        let along = Vec3::new(
            held(Key::KeyD) - held(Key::KeyA),
            held(Key::KeyW) - held(Key::KeyS),
            0.0,
        );
        let Projection::Orthographic { height, .. } = self.camera.projection;
        self.camera.position +=
            self.camera.rotation * along.normalize_or_zero() * height * SPEED * seconds;
        cx.set_camera(self.camera);
    }

    /// Zooms the camera for a scroll of the mouse wheel or a touchpad, and makes it the game's
    /// view's camera. `Break` when it zoomed, so that the layers below don't act on the same
    /// scroll.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        let lines = match *event {
            Event::Scroll(Scroll::Lines { y, .. }) => y,
            Event::Scroll(Scroll::Pixels { y, .. }) => {
                let scale = cx.window().map_or(1.0, Window::scale);
                (y / scale / POINTS_PER_LINE) as f32
            }
            _ => return ControlFlow::Continue(()),
        };
        if lines == 0.0 {
            return ControlFlow::Continue(());
        }
        // Scrolling up zooms in.
        let Projection::Orthographic { height, .. } = &mut self.camera.projection;
        let [least, most] = self.heights;
        *height = (*height * math::libm::powf(ZOOM, -lines)).clamp(least, most);
        cx.set_camera(self.camera);
        ControlFlow::Break(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Stack;

    /// At the origin, 4 units high.
    fn camera() -> Camera {
        Camera {
            projection: Projection::Orthographic {
                height: 4.0,
                near: -1.0,
                far: 1.0,
            },
            ..Camera::default()
        }
    }

    fn height(camera: Camera) -> f32 {
        let Projection::Orthographic { height, .. } = camera.projection;
        height
    }

    /// Holds `keys` down, as the window's events would.
    fn hold(cx: &mut Context, keys: &[Key]) {
        let mut stack = Stack::default();
        for &key in keys {
            let press = Event::Key {
                key,
                pressed: true,
                repeat: false,
            };
            let _ = stack.event(cx, &press);
        }
    }

    #[test]
    fn it_moves_half_the_views_height_a_second_along_the_cameras_own_axes() {
        // A quarter turn counterclockwise, so its up is the world's -X and its right the world's
        // +Y.
        let turned = Camera {
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            ..camera()
        };
        let diagonal = Vec3::new(-1.0, 1.0, 0.0).normalize();
        for (keys, expected) in [
            (&[Key::KeyW][..], Vec3::new(-1.0, 0.0, 0.0)),
            // As far diagonally as straight.
            (&[Key::KeyW, Key::KeyD], diagonal),
        ] {
            let mut cx = Context::new(None);
            hold(&mut cx, keys);
            let mut controller = OrthographicCameraController::new(turned);
            controller.update(&mut cx, Duration::from_millis(500));
            let moved = controller.camera.position;
            assert!(
                moved.abs_diff_eq(expected, 1e-5),
                "{keys:?} moved it to {moved}, not {expected}"
            );
            assert_eq!(cx.camera(), controller.camera);
        }
    }

    #[test]
    fn q_and_e_turn_it_only_once_rotation_is_set() {
        let mut cx = Context::new(None);
        hold(&mut cx, &[Key::KeyQ]);
        let mut controller = OrthographicCameraController::new(camera());
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.camera.rotation, Quat::IDENTITY);
        controller.rotation = true;
        controller.update(&mut cx, Duration::from_secs(1));
        let turned = controller.camera.rotation;
        assert!(
            turned.abs_diff_eq(Quat::from_rotation_z(FRAC_PI_2), 1e-6),
            "Q turned it to {turned}, not a quarter counterclockwise"
        );
    }

    /// Scrolls, which the controller should handle, and returns the height it zoomed to.
    fn scroll(controller: &mut OrthographicCameraController, cx: &mut Context, by: Scroll) -> f32 {
        let handled = controller.event(cx, &Event::Scroll(by));
        assert_eq!(handled, ControlFlow::Break(()), "{by:?}");
        assert_eq!(cx.camera(), controller.camera);
        height(controller.camera)
    }

    #[test]
    fn the_wheel_zooms_between_a_hundredth_and_a_hundred_times_the_starting_height() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(camera());
        // A line up zooms in, and 40 pixels down, a line from a touchpad on a display without
        // scaling, zooms back out.
        let zoomed_in = scroll(&mut controller, &mut cx, Scroll::Lines { x: 0.0, y: 1.0 });
        assert!((zoomed_in - 4.0 / 1.1).abs() < 1e-5, "{zoomed_in}");
        let touchpad = Scroll::Pixels { x: 0.0, y: -40.0 };
        let zoomed_out = scroll(&mut controller, &mut cx, touchpad);
        assert!((zoomed_out - 4.0).abs() < 1e-5, "{zoomed_out}");
        let far_in = Scroll::Lines { x: 0.0, y: 1000.0 };
        assert_eq!(scroll(&mut controller, &mut cx, far_in), 0.04);
        let far_out = Scroll::Lines { x: 0.0, y: -1000.0 };
        assert_eq!(scroll(&mut controller, &mut cx, far_out), 400.0);
        // Anything else passes by: a key, and a scroll sideways.
        for other in [
            Event::Key {
                key: Key::KeyW,
                pressed: true,
                repeat: false,
            },
            Event::Scroll(Scroll::Lines { x: 1.0, y: 0.0 }),
        ] {
            assert_eq!(controller.event(&mut cx, &other), ControlFlow::Continue(()));
        }
        assert_eq!(height(controller.camera), 400.0);
    }
}
