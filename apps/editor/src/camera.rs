//! The editor's camera: one that looks at a point, its pivot, from a distance, under either
//! projection, so that 2D and 3D views steer alike. The wheel dollies toward the point under the
//! cursor; the middle button trucks and pedestals, keeping what's under the cursor there; the right
//! button looks about in perspective and pans in orthographic, and, while it's held, W, A, S and D
//! fly and Q and E roll; Alt with the left button orbits the pivot, and Alt with the right dollies.
//! The left button alone does what the toolbar's tool says.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::ControlFlow;
use std::time::Duration;

use gg_engine::math::{EulerRot, Quat, Vec2, Vec3, libm};
use gg_engine::{
    Actions, Binding, Change, Event, Key, MouseButton, Projection, Scroll, Source, Viewpoint,
    Viewport,
};

/// How much the view spans from its bottom to its top in perspective: 60°.
const FOV: f32 = PI / 3.0;
/// How much of the world the view spans from its bottom to its top at the pivot, as it starts.
const START_HEIGHT: f32 = 10.0;
/// The least and most the camera is from its pivot.
const DISTANCES: [f32; 2] = [0.01, 100_000.0];
/// How far looking about turns it for a point the cursor is dragged, in radians: the fly camera
/// controller's.
const LOOK: f32 = 0.003;
/// How far orbiting turns it for a point the cursor is dragged, in radians.
const ORBIT: f32 = 0.005;
/// How much a line of the wheel dollies it.
const ZOOM: f32 = 1.1;
/// How fast Q and E roll it, in radians a second: a quarter turn.
const ROLL_SPEED: f32 = FRAC_PI_2;
/// How much faster it flies with Shift held.
const FAST: f32 = 3.0;
/// How long it takes to ease to a view of an axis, or back home, in seconds.
const EASE: f32 = 0.25;
/// How far in front of the camera perspective sees from, of its distance from the pivot.
const NEAR: f32 = 1e-4;
/// How far in front of the camera and behind it orthographic sees, in the view's heights.
const DEPTH: f32 = 1000.0;
/// The least of the view's direction across a plane that the pivot slides along onto it: a view
/// that runs along the plane would slide it away.
const GRAZING: f32 = 0.002;

/// What the left button does over the view, which the toolbar picks, and Q, W, E and R pick the
/// first four of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Picks what's under the cursor.
    #[default]
    Select,
    /// Moves what's picked by its handles, along its axes, across its planes or in the view's
    /// plane; and picks, off them.
    Move,
    /// Turns what's picked by its rings, about its axes or the view's; and picks, off them.
    Rotate,
    /// Scales what's picked by its handles, along its axes or all three ways at once; and picks,
    /// off them.
    Scale,
    /// Trucks and pedestals, as the middle button does.
    Pan,
    /// Orbits the pivot, as Alt and the left button do.
    Orbit,
}

/// What a drag does to the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Trucks and pedestals, keeping what's under the cursor there.
    Pan,
    /// Turns the camera where it is.
    Look,
    /// Turns the camera about the pivot.
    Orbit,
    /// Moves the camera toward where the drag started, or away.
    Dolly,
}

/// One of the world's six axes, both ways, which the gizmo shows: each names the view from that
/// side, looking back along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// +X.
    Right,
    /// -X.
    Left,
    /// +Y.
    Top,
    /// -Y.
    Bottom,
    /// +Z.
    Front,
    /// -Z.
    Back,
}

impl Side {
    pub const ALL: [Self; 6] = [
        Self::Right,
        Self::Left,
        Self::Top,
        Self::Bottom,
        Self::Front,
        Self::Back,
    ];

    /// The way it points.
    pub fn axis(self) -> Vec3 {
        match self {
            Self::Right => Vec3::X,
            Self::Left => Vec3::NEG_X,
            Self::Top => Vec3::Y,
            Self::Bottom => Vec3::NEG_Y,
            Self::Front => Vec3::Z,
            Self::Back => Vec3::NEG_Z,
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Right => Self::Left,
            Self::Left => Self::Right,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Front => Self::Back,
            Self::Back => Self::Front,
        }
    }

    /// The side a camera facing `forward`, a unit vector, looks back from, to within about a
    /// thousandth of a radian.
    pub fn facing(forward: Vec3) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|side| forward.dot(-side.axis()) > 1.0 - 1e-6)
    }

    /// The yaw and pitch of a camera on this side, looking back. From the top, the view's up is
    /// the world's -Z, as a front view tilted down shows it; from the bottom, its +Z.
    fn angles(self) -> (f32, f32) {
        match self {
            Self::Right => (FRAC_PI_2, 0.0),
            Self::Left => (-FRAC_PI_2, 0.0),
            Self::Top => (0.0, -FRAC_PI_2),
            Self::Bottom => (0.0, FRAC_PI_2),
            Self::Front => (0.0, 0.0),
            Self::Back => (PI, 0.0),
        }
    }
}

/// What the camera reads of the editor's window as an event comes, besides the event: where the
/// game's view is, in the main window's pixels, how many pixels a point spans, and whether Alt is
/// held.
#[derive(Clone, Copy, Debug)]
pub struct Screen {
    pub view: Option<Viewport>,
    pub scale: f32,
    pub alt: bool,
}

/// What its keys do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// A pair of axes: to the camera's right, and forward.
    Fly,
    /// An axis: counterclockwise, as the camera sees it, while positive.
    Roll,
    /// A button: faster while it's held.
    Fast,
    /// A button: back to where it started.
    Home,
}

/// Where the camera is: the point it looks at and orbits, which way it faces, as turns about the
/// world's up, then about its own right, then about where it looks, and how far back it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    pivot: Vec3,
    yaw: f32,
    /// From straight down to straight up, both included, so that the views from the top and the
    /// bottom are exact; the yaw kept apart keeps the turn about the world's up there.
    pitch: f32,
    roll: f32,
    distance: f32,
}

impl Pose {
    /// Looking at the origin along -Z, seeing [`START_HEIGHT`] of the world at it.
    fn start() -> Self {
        Self {
            pivot: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            distance: START_HEIGHT / 2.0 / tan_half_fov(),
        }
    }

    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, self.roll)
    }

    fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
}

/// A drag under way.
#[derive(Clone, Copy, Debug)]
struct Drag {
    /// The button that started it, whose release ends it.
    button: MouseButton,
    what: Move,
    /// For a dolly, the point it dollies toward: what was under the press, on the plane through
    /// the pivot facing the view.
    toward: Vec3,
}

/// An ease to [`OrbitCamera::pose`], from where the camera was as it started.
#[derive(Clone, Copy, Debug)]
struct Ease {
    pivot: Vec3,
    rotation: Quat,
    distance: f32,
    /// Seconds since it started.
    elapsed: f32,
}

/// The editor's camera, which the editor's layer feeds its events, updates each frame, and makes
/// the game's view's camera. Orthographic, it sees as tall a piece of the world as perspective
/// sees at the pivot, so that switching between them keeps what's at the pivot as big, and
/// dollying is one thing in both: the distance.
pub struct OrbitCamera {
    /// Where it is, or, while it eases, where it's going.
    pose: Pose,
    /// Whether it sees with perspective, rather than orthographically.
    pub perspective: bool,
    ease: Option<Ease>,
    drag: Option<Drag>,
    /// Where the cursor last was in the main window, in physical pixels.
    cursor: Option<[f64; 2]>,
    actions: Actions<Action>,
}

impl OrbitCamera {
    /// Orthographic, from the front, seeing [`START_HEIGHT`] of the world about the origin.
    pub fn new() -> Self {
        let key = Source::Key;
        let mut actions = Actions::default();
        let bound = [
            (Action::Fly, Binding::WASD),
            // Q counterclockwise, as the 2D camera controller turns.
            (
                Action::Roll,
                Binding::Axis {
                    negative: key(Key::KeyE),
                    positive: key(Key::KeyQ),
                },
            ),
            (Action::Fast, Binding::Button(key(Key::ShiftLeft))),
            (Action::Fast, Binding::Button(key(Key::ShiftRight))),
            (Action::Home, Binding::Button(key(Key::Home))),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the camera's bindings are each of one kind, of other keys");
        Self {
            pose: Pose::start(),
            perspective: false,
            ease: None,
            drag: None,
            cursor: None,
            actions,
        }
    }

    /// What the game's view sees from: orthographic, `2 * distance * tan(FOV / 2)` of the world
    /// from bottom to top, as perspective sees at the pivot, and far enough both ways to see any
    /// plane through it; in perspective, from a near plane a ten-thousandth of the distance in
    /// front, which reversed depth keeps precise whatever it is.
    pub fn viewpoint(&self) -> Viewpoint {
        let (pivot, rotation, distance) = self.current();
        let projection = if self.perspective {
            Projection::Perspective {
                fov: FOV,
                near: distance * NEAR,
            }
        } else {
            let height = 2.0 * distance * tan_half_fov();
            Projection::Orthographic {
                height,
                near: -DEPTH * height,
                far: DEPTH * height,
            }
        };
        Viewpoint {
            position: pivot - rotation * Vec3::NEG_Z * distance,
            rotation,
            projection,
        }
    }

    /// What the drag under way does, if one is.
    pub fn moving(&self) -> Option<Move> {
        self.drag.map(|drag| drag.what)
    }

    /// Whether it eases to where Home or a snap sends it.
    pub fn eases(&self) -> bool {
        self.ease.is_some()
    }

    /// Whether it flies: while the right button looks about or pans, as W, A, S and D then fly it
    /// and Q and E roll it; not while another button pans.
    pub fn flies(&self) -> bool {
        matches!(
            self.drag,
            Some(Drag {
                button: MouseButton::Right,
                what: Move::Look | Move::Pan,
                ..
            })
        )
    }

    /// Feeds the camera an event the layer was sent, which it acts on as `screen` and `tool` say.
    /// `Break` when it took a press, the wheel or Home, each of which moves it, so that the layers
    /// below don't act on them too.
    pub fn event(&mut self, event: &Event<'_>, screen: Screen, tool: Tool) -> ControlFlow<()> {
        let mut home = false;
        self.actions.event(event, |change| {
            home |= change == Change::Pressed(Action::Home);
        });
        if home {
            self.home();
            return ControlFlow::Break(());
        }
        match *event {
            Event::MouseButton {
                button,
                pressed: true,
            } => {
                // The button pressed first keeps the drag until it's let go.
                if self.drag.is_some() {
                    return ControlFlow::Continue(());
                }
                let what = match (button, screen.alt) {
                    (MouseButton::Middle, _) => Move::Pan,
                    (MouseButton::Right, true) => Move::Dolly,
                    (MouseButton::Right, false) if self.perspective => Move::Look,
                    (MouseButton::Right, false) => Move::Pan,
                    (MouseButton::Left, true) => Move::Orbit,
                    (MouseButton::Left, false) => match tool {
                        Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale => {
                            return ControlFlow::Continue(());
                        }
                        Tool::Pan => Move::Pan,
                        Tool::Orbit => Move::Orbit,
                    },
                    _ => return ControlFlow::Continue(()),
                };
                // Any move ends an ease where it was going.
                self.ease = None;
                let toward = match (self.cursor, screen.view) {
                    (Some(at), Some(view)) => self.under(at, view),
                    _ => self.pose.pivot,
                };
                self.drag = Some(Drag {
                    button,
                    what,
                    toward,
                });
                ControlFlow::Break(())
            }
            Event::MouseButton {
                button,
                pressed: false,
            } => {
                if self.drag.is_some_and(|drag| drag.button == button) {
                    self.drag = None;
                }
                ControlFlow::Continue(())
            }
            Event::CursorMoved { x, y } => {
                if let (Some(drag), Some([from_x, from_y]), Some(view)) =
                    (self.drag, self.cursor, screen.view)
                {
                    let pixels = Vec2::new((x - from_x) as f32, (y - from_y) as f32);
                    self.drag_by(drag, pixels, view, screen.scale);
                }
                self.cursor = Some([x, y]);
                ControlFlow::Continue(())
            }
            Event::CursorLeft => {
                self.cursor = None;
                ControlFlow::Continue(())
            }
            Event::Scroll(scroll) => {
                let lines = scroll.lines(f64::from(screen.scale));
                let Some(view) = screen.view.filter(|_| lines.is_finite() && lines != 0.0) else {
                    return ControlFlow::Continue(());
                };
                self.ease = None;
                let toward = match self.cursor {
                    Some(at) => self.under(at, view),
                    None => self.pose.pivot,
                };
                // Scrolling up dollies in.
                self.dolly(libm::powf(ZOOM, -lines), toward);
                ControlFlow::Break(())
            }
            Event::Focused(false) => {
                self.drag = None;
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    /// Eases on toward where it's going, for `dt`, and while the right button looks about or pans,
    /// flies and rolls by its keys: W and S forward and back, the distance a second, or zoom in
    /// orthographic, and A and D to the sides, three times as fast with Shift; Q and E roll, a
    /// quarter turn a second. Otherwise the keys leave it be, free for tools.
    pub fn update(&mut self, dt: Duration) {
        let seconds = dt.as_secs_f32();
        if let Some(ease) = &mut self.ease {
            ease.elapsed += seconds;
            if ease.elapsed >= EASE {
                self.ease = None;
            }
        }
        let fly = self.actions.axis2(Action::Fly);
        let roll = self.actions.axis(Action::Roll);
        if !self.flies() || (fly == Vec2::ZERO && roll == 0.0) {
            return;
        }
        self.ease = None;
        let fast = if self.actions.held(Action::Fast) {
            FAST
        } else {
            1.0
        };
        self.pose.roll = wrap(self.pose.roll + roll * ROLL_SPEED * seconds);
        let rotation = self.pose.rotation();
        let step = self.pose.distance * fast * seconds;
        if self.perspective {
            self.pose.pivot += rotation * Vec3::new(fly.x, 0.0, -fly.y) * step;
        } else {
            self.pose.pivot += rotation * Vec3::X * fly.x * step;
            let [least, most] = DISTANCES;
            let distance = self.pose.distance * libm::expf(-fly.y * fast * seconds);
            self.pose.distance = distance.clamp(least, most);
        }
    }

    /// Orbits the pivot as a drag of `points` across and down the view would, as the gizmo's
    /// drag does.
    pub fn orbit(&mut self, points: Vec2) {
        self.ease = None;
        self.turn(points * ORBIT, true);
    }

    /// Eases to look at the pivot from `side`, level, or from the opposite side if it looks from
    /// `side` already, or eases there: so a second click on an axis of the gizmo flips the view.
    pub fn snap(&mut self, side: Side) {
        let side = if Side::facing(self.pose.forward()) == Some(side) {
            side.opposite()
        } else {
            side
        };
        let (yaw, pitch) = side.angles();
        self.ease_to(Pose {
            yaw,
            pitch,
            roll: 0.0,
            ..self.pose
        });
    }

    /// Eases back to where it started, keeping its projection.
    pub fn home(&mut self) {
        self.ease_to(Pose::start());
    }

    /// In orthographic, slides the pivot along the view onto the plane through the origin that
    /// faces `normal`, which changes nothing in the view, but keeps the plane in its depth, however
    /// far it was panned off it; and so a switch to perspective orbits the point of the plane at
    /// the view's center. Not while it eases, nor while the view runs along the plane.
    pub fn settle(&mut self, normal: Vec3) {
        if self.perspective || self.ease.is_some() {
            return;
        }
        let forward = self.pose.forward();
        let across = forward.dot(normal);
        if across.abs() >= GRAZING {
            self.pose.pivot -= forward * (self.pose.pivot.dot(normal) / across);
        }
    }

    /// Where it is, which way it faces, and how far back from the pivot: on the way from where an
    /// ease started to where it's going, while it eases, the turn slerped and the distance
    /// changing by the same factor each moment, so that neither spins nor rushes.
    fn current(&self) -> (Vec3, Quat, f32) {
        let pose = &self.pose;
        let Some(ease) = &self.ease else {
            return (pose.pivot, pose.rotation(), pose.distance);
        };
        let t = (ease.elapsed / EASE).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let distance = ease.distance * libm::powf(pose.distance / ease.distance, s);
        let rotation = ease.rotation.slerp(pose.rotation(), s);
        (ease.pivot.lerp(pose.pivot, s), rotation, distance)
    }

    fn ease_to(&mut self, to: Pose) {
        let (pivot, rotation, distance) = self.current();
        self.pose = to;
        self.ease = Some(Ease {
            pivot,
            rotation,
            distance,
            elapsed: 0.0,
        });
    }

    /// Moves the camera as `drag` does, for the cursor's move by `pixels`, in the main window's
    /// physical pixels, `scale` to a point, over `view`.
    fn drag_by(&mut self, drag: Drag, pixels: Vec2, view: Viewport, scale: f32) {
        let points = pixels / scale;
        match drag.what {
            Move::Pan => self.pan(pixels, view),
            Move::Look => self.turn(points * LOOK, false),
            Move::Orbit => self.turn(points * ORBIT, true),
            // Right or up dollies in: as a line of the wheel does for each
            // `Scroll::POINTS_PER_LINE` points dragged.
            Move::Dolly => {
                let lines = (points.x - points.y) / Scroll::POINTS_PER_LINE as f32;
                self.dolly(libm::powf(ZOOM, -lines), drag.toward);
            }
        }
    }

    /// Trucks and pedestals by `pixels` of `view`'s, so that what's on the plane through the pivot
    /// facing the view follows the cursor.
    fn pan(&mut self, pixels: Vec2, view: Viewport) {
        let per_pixel = 2.0 * self.half_height() / view.size[1] as f32;
        // The window's y goes down.
        let by = self.pose.rotation() * Vec3::new(pixels.x, -pixels.y, 0.0);
        self.pose.pivot -= by * per_pixel;
    }

    /// Turns the camera `by` radians, as the cursor went across and down the view: about the
    /// pivot (`orbit`), as though grabbing the world, so that dragging right or down takes the
    /// camera left or up; or where it is, so that it looks the way the cursor goes. The drag is
    /// turned by the roll first, so that it turns the view the way the cursor goes however the
    /// view is rolled.
    fn turn(&mut self, by: Vec2, orbit: bool) {
        let position = self.pose.pivot - self.pose.forward() * self.pose.distance;
        let (sin, cos) = libm::sincosf(self.pose.roll);
        let up = -by.y;
        let (across, up) = (by.x * cos - up * sin, by.x * sin + up * cos);
        self.pose.yaw = wrap(self.pose.yaw - across);
        self.pose.pitch = (self.pose.pitch + up).clamp(-FRAC_PI_2, FRAC_PI_2);
        if !orbit {
            self.pose.pivot = position + self.pose.forward() * self.pose.distance;
        }
    }

    /// Scales the distance by `by`, as far as it goes, and moves the pivot that much of the way
    /// toward `toward`, on the plane through the pivot facing the view, so that it stays where it
    /// is in the view. At either end of the distances, it moves nothing.
    fn dolly(&mut self, by: f32, toward: Vec3) {
        let [least, most] = DISTANCES;
        let distance = (self.pose.distance * by).clamp(least, most);
        if distance == self.pose.distance {
            return;
        }
        let taken = distance / self.pose.distance;
        self.pose.pivot = toward + (self.pose.pivot - toward) * taken;
        self.pose.distance = distance;
    }

    /// The point `at`, in the main window's pixels, shows, on the plane through the pivot facing
    /// the view, which `view` shows.
    fn under(&self, at: [f64; 2], view: Viewport) -> Vec3 {
        let [width, height] = view.size.map(|side| side as f32);
        let x = (at[0] - f64::from(view.origin[0])) as f32 / width * 2.0 - 1.0;
        let y = 1.0 - (at[1] - f64::from(view.origin[1])) as f32 / height * 2.0;
        let half = self.half_height();
        let across = Vec3::new(x * half * width / height, y * half, 0.0);
        self.pose.pivot + self.pose.rotation() * across
    }

    /// Half the height of the world the view sees at the pivot.
    fn half_height(&self) -> f32 {
        self.pose.distance * tan_half_fov()
    }
}

fn tan_half_fov() -> f32 {
    libm::tanf(FOV / 2.0)
}

/// `angle` as the same turn from -π, not included, to π.
fn wrap(angle: f32) -> f32 {
    angle - TAU * ((angle - PI) / TAU).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{SCALE, VIEW};

    fn screen(alt: bool) -> Screen {
        Screen {
            view: Some(VIEW),
            scale: SCALE,
            alt,
        }
    }

    /// Turned every way, off the origin.
    fn turned(perspective: bool) -> OrbitCamera {
        let mut camera = OrbitCamera::new();
        camera.perspective = perspective;
        camera.pose = Pose {
            pivot: Vec3::new(3.0, -2.0, 1.0),
            yaw: 0.4,
            pitch: -0.3,
            roll: 0.2,
            distance: 12.0,
        };
        camera
    }

    fn send(camera: &mut OrbitCamera, event: Event<'_>, alt: bool, tool: Tool) -> ControlFlow<()> {
        camera.event(&event, screen(alt), tool)
    }

    fn move_to(camera: &mut OrbitCamera, at: Vec2) {
        let to = Event::CursorMoved {
            x: f64::from(at.x),
            y: f64::from(at.y),
        };
        let _ = send(camera, to, false, Tool::Select);
    }

    fn button(camera: &mut OrbitCamera, button: MouseButton, pressed: bool, alt: bool, tool: Tool) {
        let _ = send(camera, Event::MouseButton { button, pressed }, alt, tool);
    }

    fn key(camera: &mut OrbitCamera, key: Key, pressed: bool) {
        let event = Event::Key {
            key,
            pressed,
            repeat: false,
        };
        let _ = send(camera, event, false, Tool::Select);
    }

    fn scroll(camera: &mut OrbitCamera, y: f32) -> ControlFlow<()> {
        let event = Event::Scroll(Scroll::Lines { x: 0.0, y });
        send(camera, event, false, Tool::Select)
    }

    /// Drags `button` from `from` to `to` in a few moves, holding Alt if `alt`, and lets go.
    fn drag(camera: &mut OrbitCamera, button: MouseButton, alt: bool, from: Vec2, to: Vec2) {
        move_to(camera, from);
        self::button(camera, button, true, alt, Tool::Select);
        for step in 1..=4 {
            move_to(camera, from + (to - from) * step as f32 / 4.0);
        }
        self::button(camera, button, false, alt, Tool::Select);
    }

    /// Where `point` shows in [`VIEW`], in the window's pixels, as the renderer projects it.
    fn pixel(camera: &OrbitCamera, point: Vec3) -> Vec2 {
        let [width, height] = VIEW.size.map(|side| side as f32);
        let clip = camera.viewpoint().view_projection(VIEW.aspect());
        let ndc = clip.project_point3(point);
        let [x, y] = VIEW.origin.map(|at| at as f32);
        // Vulkan's clip space has y down, as the window does.
        Vec2::new(
            x + (ndc.x + 1.0) / 2.0 * width,
            y + (ndc.y + 1.0) / 2.0 * height,
        )
    }

    /// Where the camera is.
    fn eye(camera: &OrbitCamera) -> Vec3 {
        camera.viewpoint().position
    }

    fn assert_near(got: Vec2, want: Vec2, what: &str) {
        assert!(got.abs_diff_eq(want, 0.01), "{what}: {got}, not {want}");
    }

    /// Runs the ease out.
    fn eased(camera: &mut OrbitCamera) {
        camera.update(Duration::from_secs(1));
        assert!(camera.ease.is_none());
    }

    #[test]
    fn it_starts_from_the_front_seeing_10_units_about_the_origin_orthographically() {
        let camera = OrbitCamera::new().viewpoint();
        assert_eq!(camera.rotation, Quat::IDENTITY);
        assert!(
            camera
                .position
                .abs_diff_eq(Vec3::new(0.0, 0.0, 8.660254), 1e-5),
            "{}",
            camera.position
        );
        let Projection::Orthographic { height, near, far } = camera.projection else {
            panic!("{:?}", camera.projection);
        };
        assert!((height - 10.0).abs() < 1e-5, "{height}");
        assert!(
            near < -8.7 && far > 8.7,
            "the origin's in its depth: {near}..{far}"
        );
    }

    #[test]
    fn switching_projections_keeps_what_s_at_the_pivot_as_big() {
        let mut camera = turned(false);
        let up = camera.pose.rotation() * Vec3::Y;
        // Half the view's height above the pivot: its top edge, in both.
        let top = camera.pose.pivot + up * camera.half_height();
        let center = pixel(&camera, camera.pose.pivot);
        for perspective in [false, true] {
            camera.perspective = perspective;
            assert_near(pixel(&camera, top), Vec2::new(center.x, 50.0), "the top");
        }
    }

    #[test]
    fn the_wheel_dollies_toward_the_point_under_the_cursor_which_stays_there() {
        for perspective in [false, true] {
            let mut camera = turned(perspective);
            let at = Vec2::new(250.0, 500.0);
            move_to(&mut camera, at);
            let point = camera.under([250.0, 500.0], VIEW);
            assert_near(pixel(&camera, point), at, "before");
            for (lines, factor) in [(3.0, 1.0 / 1.331), (-5.0, 1.61051)] {
                let distance = camera.pose.distance;
                assert_eq!(scroll(&mut camera, lines), ControlFlow::Break(()));
                let ratio = camera.pose.distance / distance;
                assert!((ratio - factor).abs() < 1e-4, "{lines} lines: {ratio}");
                assert_near(pixel(&camera, point), at, &format!("{lines} lines"));
            }
        }
    }

    #[test]
    fn the_wheel_moves_nothing_at_the_ends_of_the_distances_nor_for_scrolls_that_aren_t_numbers() {
        let mut camera = turned(true);
        move_to(&mut camera, Vec2::new(250.0, 500.0));
        for (distance, lines) in [(0.01, 2.0), (100_000.0, -2.0)] {
            camera.pose.distance = distance;
            let pose = camera.pose;
            let _ = scroll(&mut camera, lines);
            assert_eq!(camera.pose, pose, "{lines} lines at {distance}");
        }
        let pose = camera.pose;
        for lines in [0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(scroll(&mut camera, lines), ControlFlow::Continue(()));
            assert_eq!(camera.pose, pose, "{lines} lines");
        }
    }

    #[test]
    fn panning_keeps_the_point_under_the_cursor_there() {
        for (perspective, button, tool) in [
            (false, MouseButton::Middle, Tool::Select),
            (true, MouseButton::Middle, Tool::Select),
            (false, MouseButton::Right, Tool::Select),
            (false, MouseButton::Left, Tool::Pan),
            (true, MouseButton::Left, Tool::Pan),
        ] {
            let mut camera = turned(perspective);
            let from = Vec2::new(300.0, 200.0);
            let point = camera.under([300.0, 200.0], VIEW);
            move_to(&mut camera, from);
            self::button(&mut camera, button, true, false, tool);
            assert_eq!(camera.moving(), Some(Move::Pan));
            let to = from + Vec2::new(37.0, -23.0);
            move_to(&mut camera, to);
            let what = format!("{button:?}, perspective {perspective}");
            assert_near(pixel(&camera, point), to, &what);
        }
    }

    #[test]
    fn alt_and_the_right_button_dolly_toward_the_press_which_stays_there() {
        for perspective in [false, true] {
            let mut camera = turned(perspective);
            let from = Vec2::new(700.0, 150.0);
            let point = camera.under([700.0, 150.0], VIEW);
            let distance = camera.pose.distance;
            // 40 points right and 40 up: two lines in.
            let by = Vec2::new(40.0, -40.0) * SCALE;
            drag(&mut camera, MouseButton::Right, true, from, from + by);
            let ratio = camera.pose.distance / distance;
            assert!((ratio - 1.0 / 1.21).abs() < 1e-4, "{ratio}");
            assert_near(pixel(&camera, point), from, "the press");
        }
    }

    #[test]
    fn looking_about_keeps_the_camera_where_it_is_and_orbiting_keeps_the_pivot() {
        let mut camera = turned(true);
        let (from, to) = (Vec2::new(400.0, 300.0), Vec2::new(460.0, 270.0));
        let (start, pivot) = (eye(&camera), camera.pose.pivot);
        drag(&mut camera, MouseButton::Right, false, from, to);
        assert!(eye(&camera).abs_diff_eq(start, 1e-4), "{}", eye(&camera));
        assert!(!camera.pose.pivot.abs_diff_eq(pivot, 0.1));

        let mut camera = turned(true);
        drag(&mut camera, MouseButton::Left, true, from, to);
        assert_eq!(camera.pose.pivot, pivot);
        assert!(!eye(&camera).abs_diff_eq(start, 0.1));
    }

    #[test]
    fn a_rolled_view_still_turns_the_way_the_drag_goes() {
        // Level, so that a turn about the world's up is one about the view's.
        let level = |roll| {
            let mut camera = turned(true);
            camera.pose.pitch = 0.0;
            camera.pose.roll = roll;
            camera
        };
        for roll in [0.0, FRAC_PI_2, -2.4] {
            let center = Vec2::new(500.0, 350.0);
            let right = Vec2::new(10.0, 0.0);
            // Looking about to the right brings what's right of the center toward it.
            let mut camera = level(roll);
            let point = camera.under([600.0, 350.0], VIEW);
            drag(
                &mut camera,
                MouseButton::Right,
                false,
                center,
                center + right,
            );
            let moved = pixel(&camera, point) - Vec2::new(600.0, 350.0);
            assert!(
                moved.x < -5.0 && moved.y.abs() < moved.x.abs() / 10.0,
                "looking about, rolled {roll}: {moved}"
            );
            // Orbiting to the right takes what's nearer than the pivot along with the cursor, as
            // though the world were grabbed.
            let mut camera = level(roll);
            let nearer = camera.pose.pivot - camera.pose.forward() * camera.pose.distance / 2.0;
            let before = pixel(&camera, nearer);
            drag(&mut camera, MouseButton::Left, true, center, center + right);
            let moved = pixel(&camera, nearer) - before;
            assert!(
                moved.x > 1.0 && moved.y.abs() < moved.x / 10.0,
                "orbiting, rolled {roll}: {moved}"
            );
        }
    }

    #[test]
    fn q_and_e_roll_about_the_pivot_only_while_the_right_button_is_held() {
        let mut camera = turned(true);
        key(&mut camera, Key::KeyQ, true);
        let pose = camera.pose;
        camera.update(Duration::from_secs(1));
        assert_eq!(camera.pose, pose, "without a button");
        assert!(!camera.flies());
        button(&mut camera, MouseButton::Middle, true, false, Tool::Select);
        camera.update(Duration::from_secs(1));
        assert_eq!(camera.pose, pose, "with the middle button");
        assert!(!camera.flies(), "panning with the middle button");
        button(&mut camera, MouseButton::Middle, false, false, Tool::Select);
        button(&mut camera, MouseButton::Right, true, false, Tool::Select);
        assert!(camera.flies());
        camera.update(Duration::from_millis(500));
        let rolled = camera.pose.roll - pose.roll;
        assert!((rolled - FRAC_PI_2 / 2.0).abs() < 1e-5, "{rolled}");
        assert_eq!(camera.pose.pivot, pose.pivot);
        // Alt and the right button dolly, without the keys.
        button(&mut camera, MouseButton::Right, false, false, Tool::Select);
        button(&mut camera, MouseButton::Right, true, true, Tool::Select);
        assert!(!camera.flies(), "dollying");
        let pose = camera.pose;
        camera.update(Duration::from_secs(1));
        assert_eq!(camera.pose, pose, "dollying");
        // Orthographic, the right button pans, and flies.
        button(&mut camera, MouseButton::Right, false, true, Tool::Select);
        camera.perspective = false;
        button(&mut camera, MouseButton::Right, true, false, Tool::Select);
        assert_eq!(camera.moving(), Some(Move::Pan));
        assert!(camera.flies());
    }

    #[test]
    fn w_a_s_and_d_fly_the_distance_a_second_or_three_times_with_shift() {
        let mut camera = turned(true);
        button(&mut camera, MouseButton::Right, true, false, Tool::Select);
        key(&mut camera, Key::KeyW, true);
        let (pivot, forward) = (camera.pose.pivot, camera.pose.forward());
        camera.update(Duration::from_millis(500));
        let flown = camera.pose.pivot - pivot;
        assert!(flown.abs_diff_eq(forward * 6.0, 1e-4), "{flown}");
        key(&mut camera, Key::KeyW, false);
        key(&mut camera, Key::ShiftLeft, true);
        key(&mut camera, Key::KeyD, true);
        let (pivot, right) = (camera.pose.pivot, camera.pose.rotation() * Vec3::X);
        camera.update(Duration::from_millis(500));
        let flown = camera.pose.pivot - pivot;
        assert!(flown.abs_diff_eq(right * 18.0, 1e-3), "{flown}");

        // Orthographic, W zooms in about the view's center: a factor of e a second.
        let mut camera = turned(false);
        button(&mut camera, MouseButton::Right, true, false, Tool::Select);
        key(&mut camera, Key::KeyW, true);
        let pivot = camera.pose.pivot;
        camera.update(Duration::from_secs(1));
        let ratio = camera.pose.distance / 12.0;
        assert!((ratio - libm::expf(-1.0)).abs() < 1e-5, "{ratio}");
        assert_eq!(camera.pose.pivot, pivot);
    }

    #[test]
    fn the_button_pressed_first_drives_the_drag_as_the_tool_and_alt_say() {
        for (perspective, button, alt, tool, what) in [
            (
                false,
                MouseButton::Middle,
                false,
                Tool::Select,
                Some(Move::Pan),
            ),
            (
                true,
                MouseButton::Right,
                false,
                Tool::Select,
                Some(Move::Look),
            ),
            (
                false,
                MouseButton::Right,
                false,
                Tool::Select,
                Some(Move::Pan),
            ),
            (
                true,
                MouseButton::Right,
                true,
                Tool::Select,
                Some(Move::Dolly),
            ),
            (false, MouseButton::Left, false, Tool::Select, None),
            (false, MouseButton::Left, false, Tool::Move, None),
            (true, MouseButton::Left, false, Tool::Rotate, None),
            (false, MouseButton::Left, false, Tool::Scale, None),
            (
                false,
                MouseButton::Left,
                true,
                Tool::Move,
                Some(Move::Orbit),
            ),
            (false, MouseButton::Left, false, Tool::Pan, Some(Move::Pan)),
            (
                true,
                MouseButton::Left,
                false,
                Tool::Orbit,
                Some(Move::Orbit),
            ),
            (false, MouseButton::Left, true, Tool::Pan, Some(Move::Orbit)),
            (false, MouseButton::Back, false, Tool::Pan, None),
        ] {
            let mut camera = turned(perspective);
            self::button(&mut camera, button, true, alt, tool);
            let case = format!("{button:?}, alt {alt}, {tool:?}, perspective {perspective}");
            assert_eq!(camera.moving(), what, "{case}");
        }
        // A second button changes nothing, nor does its release; the first's ends the drag.
        let mut camera = turned(true);
        button(&mut camera, MouseButton::Middle, true, false, Tool::Select);
        button(&mut camera, MouseButton::Right, true, false, Tool::Select);
        button(&mut camera, MouseButton::Right, false, false, Tool::Select);
        assert_eq!(camera.moving(), Some(Move::Pan));
        button(&mut camera, MouseButton::Middle, false, false, Tool::Select);
        assert_eq!(camera.moving(), None);
    }

    #[test]
    fn each_side_puts_the_camera_there_looking_back_and_a_second_snap_the_other_side() {
        for side in Side::ALL {
            let mut camera = turned(false);
            camera.snap(side);
            eased(&mut camera);
            let forward = camera.pose.forward();
            assert!(
                forward.dot(-side.axis()) > 1.0 - 1e-6,
                "{side:?}: {forward}"
            );
            assert_eq!(Side::facing(forward), Some(side));
            assert_eq!(camera.pose.roll, 0.0, "{side:?}");
            camera.snap(side);
            eased(&mut camera);
            assert_eq!(Side::facing(camera.pose.forward()), Some(side.opposite()));
        }
        // From the top, the view's up is -Z.
        let mut camera = turned(false);
        camera.snap(Side::Top);
        eased(&mut camera);
        let up = camera.viewpoint().rotation * Vec3::Y;
        assert!(up.abs_diff_eq(Vec3::NEG_Z, 1e-6), "{up}");
    }

    #[test]
    fn an_ease_turns_straight_there_and_ends_exactly_where_it_was_going() {
        // Straight down, turned about the world's up: easing its angles would spin it there
        // before tilting it up to the front.
        let mut camera = turned(false);
        camera.pose.yaw = 2.0;
        camera.pose.pitch = -FRAC_PI_2;
        camera.pose.roll = 0.0;
        camera.snap(Side::Front);
        let target = camera.pose.rotation();
        let mut at = camera.current().1;
        let straight = at.angle_between(target);
        let mut turned = 0.0;
        for _ in 0..30 {
            camera.update(Duration::from_millis(10));
            let now = camera.current().1;
            assert!(now.angle_between(target) <= at.angle_between(target) + 1e-5);
            turned += now.angle_between(at);
            at = now;
        }
        assert!(
            (turned - straight).abs() < straight * 0.01,
            "turned {turned} on the way, {straight} straight"
        );
        eased(&mut camera);
        let Pose {
            yaw, pitch, roll, ..
        } = camera.pose;
        assert_eq!((yaw, pitch, roll), (0.0, 0.0, 0.0));
        // Home goes back to the start, and keeps the projection.
        camera.perspective = true;
        camera.home();
        eased(&mut camera);
        assert_eq!(camera.pose, Pose::start());
        assert!(camera.perspective);
    }

    #[test]
    fn a_move_ends_an_ease_where_it_was_going() {
        let mut camera = turned(false);
        camera.snap(Side::Right);
        let target = camera.pose;
        camera.update(Duration::from_millis(50));
        move_to(&mut camera, Vec2::new(400.0, 300.0));
        button(&mut camera, MouseButton::Middle, true, false, Tool::Select);
        assert!(camera.ease.is_none());
        assert_eq!(camera.pose, target);
    }

    #[test]
    fn orthographic_the_pivot_stays_on_the_grid_s_plane_through_pans_and_snaps() {
        let mut camera = turned(false);
        let point = Vec3::new(2.0, 1.0, -3.0);
        let before = pixel(&camera, point);
        camera.settle(Vec3::Z);
        assert!(camera.pose.pivot.z.abs() < 1e-5, "{}", camera.pose.pivot);
        assert_near(
            pixel(&camera, point),
            before,
            "sliding moves nothing in the view",
        );
        drag(
            &mut camera,
            MouseButton::Middle,
            false,
            Vec2::new(400.0, 300.0),
            Vec2::new(300.0, 100.0),
        );
        camera.settle(Vec3::Z);
        assert!(camera.pose.pivot.z.abs() < 1e-5, "{}", camera.pose.pivot);
        camera.snap(Side::Top);
        camera.settle(Vec3::Y);
        eased(&mut camera);
        camera.settle(Vec3::Y);
        assert!(camera.pose.pivot.y.abs() < 1e-5, "{}", camera.pose.pivot);
        // Not onto a plane the view runs along, nor in perspective.
        let pivot = camera.pose.pivot;
        camera.settle(Vec3::Z);
        assert_eq!(camera.pose.pivot, pivot);
        camera.perspective = true;
        camera.settle(Vec3::Y + Vec3::Z);
        assert_eq!(camera.pose.pivot, pivot);
    }

    #[test]
    fn angles_wrap_to_a_half_turn_either_way() {
        for (angle, wrapped) in [
            (PI, PI),
            (-PI, PI),
            (3.0 * PI, PI),
            (0.5, 0.5),
            (-4.0, -4.0 + TAU),
        ] {
            assert!(
                (wrap(angle) - wrapped).abs() < 1e-6,
                "{angle}: {}",
                wrap(angle)
            );
        }
    }
}
