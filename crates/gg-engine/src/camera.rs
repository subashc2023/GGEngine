//! Camera controllers: what steers the game's camera from the keyboard, the mouse and gamepads, an
//! orthographic one as 2D games do, and one that flies, as 3D tools do.

use std::f32::consts::{FRAC_PI_2, PI};
use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::math::{self, Quat, Vec2, Vec3};
use gg_renderer::{Camera, Projection};

use crate::{
    Actions, Binding, Context, Event, Key, MouseButton, Scroll, Source, Stick, Trigger, Window,
};

/// How fast the camera moves, in the view's heights a second.
const SPEED: f32 = 0.5;
/// How fast it turns, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;
/// How much a line of the mouse wheel zooms.
const ZOOM: f32 = 1.1;
/// What a line of the wheel is in points, for scrolls that a touchpad reports in pixels: 40, as
/// egui counts it on the desktop (`InputOptions::line_scroll_speed`).
const POINTS_PER_LINE: f64 = 40.0;
/// How fast the fly camera moves at first, in world units a second.
const FLY_SPEED: f32 = 2.0;
/// How fast it looks about from a stick at full tilt, in radians a second: half a turn.
const LOOK_SPEED: f32 = PI;
/// How far it looks about for a point the mouse is dragged, in radians.
const DRAG_LOOK: f32 = 0.003;
/// How far up or down it looks at most: 89°, short of straight up or down, where which way it
/// faces about the world's up would be lost.
const MAX_PITCH: f32 = 89.0 * PI / 180.0;

/// What the camera controller does, which its [`actions`](OrthographicCameraController::actions)
/// bind to keys, buttons and gamepads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAction {
    /// A pair of axes: along the camera's own, to its right and up the view.
    Move,
    /// An axis: counterclockwise, as the camera sees it, while positive.
    Turn,
}

/// Steers an orthographic camera from the keyboard, the mouse wheel and gamepads, as 2D games and
/// tools do. W, A, S and D, or a gamepad's right stick, move it along its own axes, half the view's
/// height a second, so that it crosses the screen as fast however far it's zoomed; Q and E, or the
/// left and right triggers, turn it left and right, a quarter turn a second, if
/// [`rotation`](Self::rotation) is set; and the mouse wheel zooms about the view's center, 1.1
/// times a line, between a hundredth and a hundred times the height it started at.
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
    /// What moves and turns it: at first W, A, S and D and the right stick, and Q and E and the
    /// triggers.
    pub actions: Actions<CameraAction>,
    /// The least and most of the world the view's height zooms between.
    heights: [f32; 2],
}

impl OrthographicCameraController {
    /// # Panics
    ///
    /// If the camera isn't orthographic.
    pub fn new(mut camera: Camera) -> Self {
        let height = *height(&mut camera.projection);
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
        // The left trigger counterclockwise, as Q.
        let triggers = Binding::Axis {
            negative: Source::PadTrigger(Trigger::Right),
            positive: Source::PadTrigger(Trigger::Left),
        };
        let bound = [
            (CameraAction::Move, move_),
            (CameraAction::Move, Binding::stick(Stick::Right)),
            (CameraAction::Turn, turn),
            (CameraAction::Turn, triggers),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the controller's own bindings are each of one kind, of other sources");
        Self {
            camera,
            rotation: false,
            actions,
            heights: [height / 100.0, height * 100.0],
        }
    }

    /// Moves and turns the camera by its actions, for `dt`, and makes it the game's view's camera.
    ///
    /// # Panics
    ///
    /// If the camera isn't orthographic.
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
        let height = *height(&mut self.camera.projection);
        self.camera.position += self.camera.rotation * along * height * SPEED * seconds;
        cx.set_camera(self.camera);
    }

    /// Feeds its actions `event`, and zooms the camera for a scroll of the mouse wheel or a
    /// touchpad, making it the game's view's camera. `Break` when it zoomed, so that the layers
    /// below don't act on the same scroll.
    ///
    /// # Panics
    ///
    /// On a scroll, if the camera isn't orthographic.
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
        let height = height(&mut self.camera.projection);
        let [least, most] = self.heights;
        *height = (*height * math::libm::powf(ZOOM, -lines)).clamp(least, most);
        cx.set_camera(self.camera);
        ControlFlow::Break(())
    }
}

/// The height of the world an orthographic camera's view spans.
///
/// # Panics
///
/// If the camera isn't orthographic.
fn height(projection: &mut Projection) -> &mut f32 {
    match projection {
        Projection::Orthographic { height, .. } => height,
        Projection::Perspective { .. } => panic!(
            "the orthographic camera controller steers an orthographic camera; got {projection:?}"
        ),
    }
}

/// What the fly camera controller does, which its [`actions`](FlyCameraController::actions) bind
/// to keys, buttons and gamepads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyAction {
    /// A pair of axes: to the camera's right, and forward, where it looks.
    Move,
    /// An axis: up along the world's +Y while positive.
    Rise,
    /// A pair of axes: to the camera's right, and up.
    Look,
    /// A button: while it's held, moving the mouse looks about.
    Drag,
}

/// Steers a camera that flies, as 3D tools' and games' free cameras do, under either projection.
/// W, A, S and D, or a gamepad's left stick, move it forward, where it looks, and to its sides, 2
/// units a second at first; E and Q, or the right and left triggers, raise and lower it along the
/// world's up; and the right stick, or the mouse moved with its right button held, looks about,
/// half a turn a second at full tilt, or 0.003 radians a point. Looking keeps it level, its right
/// along the world's horizon, and looks no further up or down than 89°.
/// [`actions`](Self::actions) binds them to other keys and buttons.
///
/// A layer keeps one, and calls [`update`](Self::update) from its own update and
/// [`event`](Self::event) from its own event handler, as the orthographic camera controller is.
/// It keeps nothing of the camera but [`camera`](Self::camera), which the layer can place and turn
/// itself: a camera turned about where it looks levels as it next looks about.
pub struct FlyCameraController {
    /// The camera it steers.
    pub camera: Camera,
    /// How fast it moves, in world units a second.
    pub speed: f32,
    /// What moves it and looks about: at first W, A, S and D and the left stick, E and Q and the
    /// triggers, the right stick, and the right mouse button.
    pub actions: Actions<FlyAction>,
    /// Where the cursor last was in the window, in physical pixels, which a drag looks from.
    cursor: Option<[f64; 2]>,
}

impl FlyCameraController {
    pub fn new(camera: Camera) -> Self {
        let key = Source::Key;
        let mut actions = Actions::default();
        let move_ = Binding::Axis2 {
            x: [key(Key::KeyA), key(Key::KeyD)],
            y: [key(Key::KeyS), key(Key::KeyW)],
        };
        let rise = Binding::Axis {
            negative: key(Key::KeyQ),
            positive: key(Key::KeyE),
        };
        let triggers = Binding::Axis {
            negative: Source::PadTrigger(Trigger::Left),
            positive: Source::PadTrigger(Trigger::Right),
        };
        let bound = [
            (FlyAction::Move, move_),
            (FlyAction::Move, Binding::stick(Stick::Left)),
            (FlyAction::Rise, rise),
            (FlyAction::Rise, triggers),
            (FlyAction::Look, Binding::stick(Stick::Right)),
            (
                FlyAction::Drag,
                Binding::Button(Source::Button(MouseButton::Right)),
            ),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the controller's own bindings are each of one kind, of other sources");
        Self {
            camera,
            speed: FLY_SPEED,
            actions,
            cursor: None,
        }
    }

    /// Looks about and moves the camera by its actions, for `dt`, and makes it the game's view's
    /// camera.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let look = self.actions.axis2(FlyAction::Look);
        if look != Vec2::ZERO {
            self.look(look * LOOK_SPEED * seconds);
        }
        // Forward is where it looks, up or down too; as fast every way, rising too.
        let along = self.actions.axis2(FlyAction::Move);
        let rise = self.actions.axis(FlyAction::Rise);
        let way = self.camera.rotation * Vec3::new(along.x, 0.0, -along.y) + Vec3::Y * rise;
        self.camera.position += way.clamp_length_max(1.0) * self.speed * seconds;
        cx.set_camera(self.camera);
    }

    /// Feeds its actions `event`, and looks about for a move of the cursor while
    /// [`FlyAction::Drag`] is held, making the camera the game's view's. It handles nothing for
    /// the layers below: `Continue`, always.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Polled as it updates, but for the drag, which is held before the move is read.
        self.actions.event(event, |_| {});
        match *event {
            Event::CursorMoved { x, y } => {
                if let Some([from_x, from_y]) = self.cursor
                    && self.actions.held(FlyAction::Drag)
                {
                    let scale = cx.window().map_or(1.0, Window::scale);
                    let points = |to: f64, from: f64| ((to - from) / scale) as f32;
                    // The window's y goes down: dragging down looks down.
                    let by = Vec2::new(points(x, from_x), -points(y, from_y));
                    self.look(by * DRAG_LOOK);
                    cx.set_camera(self.camera);
                }
                self.cursor = Some([x, y]);
            }
            Event::CursorLeft => self.cursor = None,
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// Turns the camera `by` radians: to its right about the world's up by `x`, and up about its
    /// own right by `y`, no further than [`MAX_PITCH`]; level, without a turn about where it
    /// looks.
    fn look(&mut self, by: Vec2) {
        let forward = self.camera.rotation * Vec3::NEG_Z;
        // Facing -Z unturned, so a turn to its left is positive about +Y.
        let yaw = math::libm::atan2f(-forward.x, -forward.z) - by.x;
        let pitch = math::libm::asinf(forward.y.clamp(-1.0, 1.0)) + by.y;
        let pitch = pitch.clamp(-MAX_PITCH, MAX_PITCH);
        self.camera.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    }
}

#[cfg(test)]
mod tests {
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

    fn height(mut camera: Camera) -> f32 {
        *super::height(&mut camera.projection)
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

    #[test]
    fn a_gamepads_right_stick_moves_it_and_its_triggers_turn_it() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(camera());
        let pad = crate::Pad(0);
        let mut send = |controller: &mut OrthographicCameraController, event| {
            assert_eq!(controller.event(&mut cx, &event), ControlFlow::Continue(()));
        };
        // Half of the way up for a second.
        let stick = |value| Event::PadStick {
            pad,
            stick: Stick::Right,
            value,
        };
        send(&mut controller, stick(Vec2::new(0.0, 0.5)));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let moved = controller.camera.position;
        assert!(moved.abs_diff_eq(Vec3::Y, 1e-6), "{moved}");
        // The left trigger half of the way for a second.
        send(&mut controller, stick(Vec2::ZERO));
        controller.rotation = true;
        let trigger = |trigger, value| Event::PadTrigger {
            pad,
            trigger,
            value,
        };
        send(&mut controller, trigger(Trigger::Left, 0.5));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let eighth = Quat::from_rotation_z(FRAC_PI_2 / 2.0);
        let turned = controller.camera.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
        // Both triggers as far cancel.
        send(&mut controller, trigger(Trigger::Right, 0.5));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let turned = controller.camera.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
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

    #[test]
    #[should_panic(expected = "steers an orthographic camera")]
    fn the_orthographic_controller_refuses_a_camera_with_perspective() {
        let perspective = Camera {
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
            ..Camera::default()
        };
        OrthographicCameraController::new(perspective);
    }

    /// At the origin, unturned, looking down -Z with perspective.
    fn flying() -> FlyCameraController {
        FlyCameraController::new(Camera {
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
            ..Camera::default()
        })
    }

    /// Sends `events` to the fly camera controller, which passes each on.
    fn send(controller: &mut FlyCameraController, cx: &mut Context, events: &[Event<'_>]) {
        for event in events {
            assert_eq!(controller.event(cx, event), ControlFlow::Continue(()));
        }
    }

    fn key(key: Key, pressed: bool) -> Event<'static> {
        Event::Key {
            key,
            pressed,
            repeat: false,
        }
    }

    /// Where the camera looks, and its right.
    fn facing(controller: &FlyCameraController) -> (Vec3, Vec3) {
        let rotation = controller.camera.rotation;
        (rotation * Vec3::NEG_Z, rotation * Vec3::X)
    }

    #[test]
    fn the_fly_camera_moves_where_it_looks_and_rises_along_the_worlds_up() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        let half = Duration::from_millis(500);
        send(&mut controller, &mut cx, &[key(Key::KeyW, true)]);
        controller.update(&mut cx, half);
        let moved = controller.camera.position;
        assert!(moved.abs_diff_eq(Vec3::NEG_Z, 1e-6), "{moved}");
        assert_eq!(cx.camera(), controller.camera);
        // A quarter turn to its left, and it goes along -X.
        controller.camera.position = Vec3::ZERO;
        controller.camera.rotation = Quat::from_rotation_y(FRAC_PI_2);
        controller.update(&mut cx, half);
        let moved = controller.camera.position;
        assert!(moved.abs_diff_eq(Vec3::NEG_X, 1e-6), "{moved}");
        // Looking down an eighth of a turn, E raises it straight up, as fast as W moves it.
        send(&mut controller, &mut cx, &[key(Key::KeyW, false)]);
        send(&mut controller, &mut cx, &[key(Key::KeyE, true)]);
        controller.camera.position = Vec3::ZERO;
        controller.camera.rotation = Quat::from_rotation_x(-FRAC_PI_2 / 2.0);
        controller.update(&mut cx, half);
        let moved = controller.camera.position;
        assert!(moved.abs_diff_eq(Vec3::Y, 1e-6), "{moved}");
        // Both at once, unturned, no faster.
        send(&mut controller, &mut cx, &[key(Key::KeyW, true)]);
        controller.camera.position = Vec3::ZERO;
        controller.camera.rotation = Quat::IDENTITY;
        controller.update(&mut cx, half);
        let moved = controller.camera.position.length();
        assert!((moved - 1.0).abs() < 1e-6, "{moved}");
    }

    #[test]
    fn the_right_stick_looks_about_and_keeps_the_camera_level() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        let pad = crate::Pad(0);
        let stick = |value| Event::PadStick {
            pad,
            stick: Stick::Right,
            value,
        };
        // Half tilted right for a second: a quarter turn, to face +X.
        send(&mut controller, &mut cx, &[stick(Vec2::new(0.5, 0.0))]);
        controller.update(&mut cx, Duration::from_secs(1));
        let (forward, _) = facing(&controller);
        assert!(forward.abs_diff_eq(Vec3::X, 1e-5), "{forward}");
        // Full up for a second, which would look past straight up, stops at 89°.
        send(&mut controller, &mut cx, &[stick(Vec2::Y)]);
        controller.update(&mut cx, Duration::from_secs(1));
        let (forward, right) = facing(&controller);
        let pitch = math::libm::asinf(forward.y);
        assert!((pitch - MAX_PITCH).abs() < 1e-4, "{pitch}");
        assert!(right.y.abs() < 1e-6, "{right}");
        // Down and around, the camera's right stays along the horizon.
        for value in [
            Vec2::new(0.7, -0.7),
            Vec2::new(-0.3, 0.2),
            Vec2::new(1.0, 0.0),
        ] {
            send(&mut controller, &mut cx, &[stick(value)]);
            controller.update(&mut cx, Duration::from_millis(300));
            let (_, right) = facing(&controller);
            assert!(right.y.abs() < 1e-6, "{value}: {right}");
        }
    }

    #[test]
    fn the_mouse_looks_about_only_while_its_right_button_is_held() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        let at = |x, y| Event::CursorMoved { x, y };
        let right = |pressed| Event::MouseButton {
            button: MouseButton::Right,
            pressed,
        };
        // Without the button, nothing; with it, from where the cursor last was, 0.003 radians a
        // point, which a window without scaling, or none, makes a pixel.
        send(
            &mut controller,
            &mut cx,
            &[at(10.0, 10.0), at(500.0, 300.0)],
        );
        assert_eq!(controller.camera.rotation, Quat::IDENTITY);
        send(&mut controller, &mut cx, &[right(true), at(600.0, 300.0)]);
        let turned = controller.camera.rotation;
        let expected = Quat::from_rotation_y(-0.3);
        assert!(turned.abs_diff_eq(expected, 1e-6), "{turned}");
        assert_eq!(cx.camera(), controller.camera);
        // Dragging down looks down.
        send(&mut controller, &mut cx, &[at(600.0, 400.0)]);
        let (forward, _) = facing(&controller);
        let pitch = math::libm::asinf(forward.y);
        assert!((pitch + 0.3).abs() < 1e-5, "{pitch}");
        // Back into the window elsewhere, it looks from there, not from where it left.
        let left = controller.camera.rotation;
        send(&mut controller, &mut cx, &[Event::CursorLeft, at(0.0, 0.0)]);
        assert_eq!(controller.camera.rotation, left);
        // Let go, nothing.
        send(&mut controller, &mut cx, &[right(false), at(100.0, 100.0)]);
        assert_eq!(controller.camera.rotation, left);
    }

    #[test]
    fn the_fly_camera_levels_a_camera_turned_about_where_it_looks_as_it_looks_about() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        controller.camera.rotation = Quat::from_rotation_z(1.0);
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.camera.rotation, Quat::from_rotation_z(1.0));
        let pad = crate::Pad(0);
        let stick = Event::PadStick {
            pad,
            stick: Stick::Right,
            value: Vec2::new(0.5, 0.0),
        };
        send(&mut controller, &mut cx, &[stick]);
        controller.update(&mut cx, Duration::from_millis(100));
        let (forward, right) = facing(&controller);
        assert!(right.y.abs() < 1e-6, "{right}");
        assert!(forward.y.abs() < 1e-6, "{forward}");
    }
}
