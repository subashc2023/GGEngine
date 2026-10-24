//! Camera controllers: what steers the game's camera from the keyboard and mouse.

use std::f32::consts::FRAC_PI_2;
use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::math::{self, Quat};
use gg_renderer::{Camera, Projection};

use crate::{Actions, Binding, Context, Event, Key, Scroll, Source, Window};

/// How fast the camera moves, in the view's heights a second.
const SPEED: f32 = 0.5;
/// How fast it turns, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;
/// How much a line of the mouse wheel zooms.
const ZOOM: f32 = 1.1;
/// What a line of the wheel is in points, for scrolls that a touchpad reports in pixels: 40, as
/// egui counts it on the desktop (`InputOptions::line_scroll_speed`).
const POINTS_PER_LINE: f64 = 40.0;

/// What the camera controller does, which its [`actions`](OrthographicCameraController::actions)
/// bind to keys and buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAction {
    /// A pair of axes: along the camera's own, to its right and up the view.
    Move,
    /// An axis: counterclockwise, as the camera sees it, while positive.
    Turn,
}

/// Steers an orthographic camera from the keyboard and mouse wheel, as 2D games and tools do.
/// W, A, S and D move it along its own axes, half the view's height a second, so that it crosses
/// the screen as fast however far it's zoomed; Q and E turn it left and right, a quarter turn a
/// second, if [`rotation`](Self::rotation) is set; and the mouse wheel zooms about the view's
/// center, 1.1 times a line, between a hundredth and a hundred times the height it started at.
/// [`actions`](Self::actions) binds moving and turning to other keys and buttons.
///
/// A layer keeps one, and calls [`update`](Self::update) from its own update and
/// [`event`](Self::event) from its own event handler, as `self.camera.event(cx, event)?`, which is
/// how the keys reach it.
pub struct OrthographicCameraController {
    /// The camera it steers, which the layer can place, turn or zoom itself too.
    pub camera: Camera,
    /// Whether turning turns the camera; at first it doesn't.
    pub rotation: bool,
    /// What moves and turns it: at first W, A, S and D, and Q and E.
    pub actions: Actions<CameraAction>,
    /// The least and most of the world the view's height zooms between.
    heights: [f32; 2],
}

impl OrthographicCameraController {
    pub fn new(camera: Camera) -> Self {
        let Projection::Orthographic { height, .. } = camera.projection;
        let key = Source::Key;
        let mut actions = Actions::default();
        let move_ = Binding::Axis2 {
            x: [key(Key::KeyA), key(Key::KeyD)],
            y: [key(Key::KeyS), key(Key::KeyW)],
        };
        let turn = Binding::Axis {
            negative: key(Key::KeyE),
            positive: key(Key::KeyQ),
        };
        let bound = actions.bind(CameraAction::Move, move_);
        let bound = bound.and_then(|()| actions.bind(CameraAction::Turn, turn));
        bound.expect("the controller's own bindings are each of one kind, of other keys");
        Self {
            camera,
            rotation: false,
            actions,
            heights: [height / 100.0, height * 100.0],
        }
    }

    /// Moves and turns the camera by its actions, for `dt`, and makes it the game's view's camera.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        if self.rotation {
            let turn = self.actions.axis(CameraAction::Turn);
            // About the axis it looks along, counterclockwise as it sees it; normalized, so that
            // rounding doesn't build up frame after frame.
            let by = Quat::from_rotation_z(turn * TURN_SPEED * seconds);
            self.camera.rotation = (self.camera.rotation * by).normalize();
        }
        // Along the camera's own axes, so that W moves up the screen however it's turned; as fast
        // diagonally as straight, since a pair of axes goes no further than 1.
        let along = self.actions.axis2(CameraAction::Move).extend(0.0);
        let Projection::Orthographic { height, .. } = self.camera.projection;
        self.camera.position += self.camera.rotation * along * height * SPEED * seconds;
        cx.set_camera(self.camera);
    }

    /// Feeds its actions `event`, and zooms the camera for a scroll of the mouse wheel or a
    /// touchpad, making it the game's view's camera. `Break` when it zoomed, so that the layers
    /// below don't act on the same scroll.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Polled as it updates.
        self.actions.event(event, |_| {});
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
    use gg_core::math::Vec3;

    use super::*;

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

    /// Holds `keys` down, as the window's events would, sent on by the controller's layer.
    fn hold(controller: &mut OrthographicCameraController, cx: &mut Context, keys: &[Key]) {
        for &key in keys {
            let press = Event::Key {
                key,
                pressed: true,
                repeat: false,
            };
            assert_eq!(controller.event(cx, &press), ControlFlow::Continue(()));
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
            let mut controller = OrthographicCameraController::new(turned);
            hold(&mut controller, &mut cx, keys);
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
        let mut controller = OrthographicCameraController::new(camera());
        hold(&mut controller, &mut cx, &[Key::KeyQ]);
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

    #[test]
    fn its_keys_bind_again() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(camera());
        controller.actions.unbind(CameraAction::Move);
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let bound = controller.actions.bind(CameraAction::Move, arrows);
        bound.expect("the arrows are free");
        hold(&mut controller, &mut cx, &[Key::KeyW]);
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.camera.position, Vec3::ZERO);
        // Half the view's height in half a second.
        hold(&mut controller, &mut cx, &[Key::ArrowUp]);
        controller.update(&mut cx, Duration::from_millis(500));
        let moved = controller.camera.position;
        assert!(moved.abs_diff_eq(Vec3::Y, 1e-6), "{moved}");
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
