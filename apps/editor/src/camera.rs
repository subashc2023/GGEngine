//! The editor's camera: it looks at a pivot from a distance, under either projection, so 2D and 3D
//! views steer alike.
//!
//! - Wheel: dollies toward the point under the cursor.
//! - Middle button: trucks and pedestals, keeping what's under the cursor there.
//! - Right button: looks about in perspective, pans in orthographic. While it's held, W, A, S and D
//!   fly and Q and E roll.
//! - Alt and the left button orbit the pivot; Alt and the right dolly.
//! - The left button alone does what the toolbar's tool says.
//!
//! Looking about, orbiting and dollying go by the mouse's raw motion, the cursor grabbed, hidden,
//! and held where the platform locks it. So they go on past the screen's edges, and by the mouse's
//! own steps, which pointer acceleration, as Windows' enhanced pointer precision, makes finer than
//! the cursor's pixels at slow speeds. Panning follows the cursor.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::ControlFlow;
use std::time::Duration;

use gg_engine::math::{DVec3, EulerRot, Quat, Vec2, Vec3, libm};
use gg_engine::{
    Actions, Binding, Change, Event, Key, MouseButton, Projection, Scroll, Source, Viewpoint,
    Viewport,
};

/// How much the view spans from its bottom to its top in perspective: 60°.
const FOV: f32 = PI / 3.0;
/// How much of the world the view spans, bottom to top, at the pivot as it starts.
const START_HEIGHT: f32 = 10.0;
/// The least and most distance from the pivot.
const DISTANCES: [f32; 2] = [0.01, 100_000.0];
/// How much of the view's height a framed sphere fills.
const FRAMED: f32 = 0.8;
/// Looking about's turn per point of mouse motion, in radians: the fly camera controller's per
/// point dragged. Raw motion counts as physical pixels, as Windows moves the cursor by it at its
/// default speed without enhanced pointer precision.
const LOOK: f32 = 0.003;
/// Orbiting's turn per point of mouse motion, in radians.
const ORBIT: f32 = 0.005;
/// How much a line of the wheel dollies.
const ZOOM: f32 = 1.1;
/// Q's and E's roll speed, in radians a second: a quarter turn.
const ROLL_SPEED: f32 = FRAC_PI_2;
/// Flying's speed-up with Shift held.
const FAST: f32 = 3.0;
/// How long an ease to an axis's view, or home, takes, in seconds.
const EASE: f32 = 0.25;
/// Perspective's near plane, as a share of the distance to the pivot.
const NEAR: f32 = 1e-4;
/// How far in front of the camera and behind it orthographic sees, in view heights.
const DEPTH: f32 = 1000.0;
/// The least of the view's direction across a plane for the pivot to slide onto it: a view along
/// the plane would slide it away.
const GRAZING: f32 = 0.002;

/// What the left button does over the view. The toolbar picks it;
/// [`SHORTCUTS`](crate::shortcuts::SHORTCUTS) holds keys for the first four.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Picks what's under the cursor.
    #[default]
    Select,
    /// Moves what's picked by its handles: along its axes, across its planes or in the view's
    /// plane. Picks, off them.
    Move,
    /// Turns what's picked by its rings, about its axes or the view's. Picks, off them.
    Rotate,
    /// Scales what's picked by its handles, along its axes or all three ways at once. Picks, off
    /// them.
    Scale,
    /// Trucks and pedestals, as the middle button does.
    Pan,
    /// Orbits the pivot, as Alt and the left button do.
    Orbit,
    /// Reads what shading took and made at the pixel pressed, as `gg probe` does.
    Probe,
}

/// Which axes Move's and Rotate's handles go along; the toolbar picks it. Scale's are always the
/// entity's own: scaling a turned entity along the world's would shear it, which a transform can't
/// hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Space {
    /// The entity's own axes, as drawing carries them.
    #[default]
    Local,
    /// The world's axes, at the entity's origin.
    World,
}

impl Space {
    /// The other one.
    pub fn flipped(self) -> Self {
        match self {
            Self::Local => Self::World,
            Self::World => Self::Local,
        }
    }
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

/// What the camera reads of the window with an event: where the game's view is, in the main
/// window's pixels; pixels per point; and whether Alt is held.
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

/// Where the camera is: the pivot it looks at and orbits; its facing, as turns about the world's
/// up, then its own right, then where it looks; and its distance back. The pivot is `f64`, as the
/// world's positions are, so the camera flies as smoothly 1,000 km from the origin as at it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    pivot: DVec3,
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
            pivot: DVec3::ZERO,
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

#[derive(Clone, Copy, Debug)]
struct Drag {
    /// The button that started it, whose release ends it.
    button: MouseButton,
    what: Move,
    /// For a dolly, the point it dollies toward: what was under the press, on the plane through
    /// the pivot facing the view.
    toward: DVec3,
}

/// An ease to [`OrbitCamera::pose`], from where the camera was as it started.
#[derive(Clone, Copy, Debug)]
struct Ease {
    pivot: DVec3,
    rotation: Quat,
    distance: f32,
    /// Seconds since it started.
    elapsed: f32,
}

/// The editor's camera. The layer feeds it events, updates it each frame, and makes it the game's
/// view's camera. Orthographic, it sees as tall a piece of the world as perspective sees at the
/// pivot, so a switch keeps what's at the pivot as big, and dollying is the distance in both.
pub struct OrbitCamera {
    /// Where it is, or, while it eases, where it's going.
    pose: Pose,
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
        let bound = actions.bind_all([
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
        ]);
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
            position: pivot - (rotation * Vec3::NEG_Z * distance).as_dvec3(),
            rotation,
            projection,
        }
    }

    /// Sees from `viewpoint`, as [`viewpoint`](Self::viewpoint) gives it back:
    ///
    /// - turned as it is, its yaw, pitch and roll read in the camera's turn order. At the poles,
    ///   where a yaw and a roll turn alike, the roll folds into the yaw, the turn still exact;
    /// - with perspective if it has it, the camera's own field of view kept;
    /// - as far from the pivot as an orthographic height makes it, or in perspective as far as it
    ///   was, within the distances it goes; the pivot that far in front.
    ///
    /// An ease stops where it was.
    pub fn adopt(&mut self, viewpoint: &Viewpoint) {
        let (yaw, pitch, roll) = viewpoint.rotation.to_euler(EulerRot::YXZ);
        let distance = match viewpoint.projection {
            Projection::Orthographic { height, .. } => height / 2.0 / tan_half_fov(),
            Projection::Perspective { .. } => self.current().2,
        };
        let [least, most] = DISTANCES;
        let mut pose = Pose {
            pivot: viewpoint.position,
            yaw,
            pitch,
            roll,
            distance: distance.clamp(least, most),
        };
        pose.pivot += (pose.forward() * pose.distance).as_dvec3();
        self.pose = pose;
        self.perspective = matches!(viewpoint.projection, Projection::Perspective { .. });
        self.ease = None;
    }

    /// What the drag under way does, if one is.
    pub fn moving(&self) -> Option<Move> {
        self.drag.map(|drag| drag.what)
    }

    /// Whether the drag under way wants the cursor grabbed: looking about, orbiting and dollying,
    /// which go by the mouse's raw motion; not panning, which keeps what's under the cursor there.
    pub fn grabs(&self) -> bool {
        self.drag.is_some_and(|drag| drag.what != Move::Pan)
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

    /// Feeds the camera an event, acting as `screen` and `tool` say. `Break` when it took a press,
    /// the wheel or Home, each of which moves it, so the layers below don't act on them too.
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
                        Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale | Tool::Probe => {
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
                if let (Some(drag), Some([from_x, from_y]), Some(view)) = (
                    self.drag.filter(|_| !self.grabs()),
                    self.cursor,
                    screen.view,
                ) {
                    let pixels = Vec2::new((x - from_x) as f32, (y - from_y) as f32);
                    self.drag_by(drag, pixels, view, screen.scale);
                }
                self.cursor = Some([x, y]);
                ControlFlow::Continue(())
            }
            Event::MouseMotion { x, y } => {
                if let (Some(drag), Some(view)) = (self.drag.filter(|_| self.grabs()), screen.view)
                {
                    self.drag_by(drag, Vec2::new(x as f32, y as f32), view, screen.scale);
                }
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

    /// Eases on toward where it's going, for `dt`. While the right button looks about or pans, its
    /// keys fly and roll it: W and S forward and back, the distance a second, or zoom in
    /// orthographic; A and D to the sides; three times as fast with Shift; Q and E roll a quarter
    /// turn a second. Otherwise the keys are left to tools.
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
            self.pose.pivot += (rotation * Vec3::new(fly.x, 0.0, -fly.y) * step).as_dvec3();
        } else {
            self.pose.pivot += (rotation * Vec3::X * fly.x * step).as_dvec3();
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

    /// Eases to look at `point`, keeping its facing and distance, as framing what's picked does
    /// when its size isn't known: things stay as big as the view showed them at the pivot.
    pub fn frame(&mut self, point: DVec3) {
        self.ease_to(Pose {
            pivot: point,
            ..self.pose
        });
    }

    /// Eases to look at the sphere of `radius` round `center`, keeping its facing, from as far back
    /// as the sphere fills [`FRAMED`] of the view's height. In perspective the sphere just subtends
    /// that part of the field of view; orthographically, the view is as tall as its diameter over
    /// [`FRAMED`]. Within the camera's distances. A sphere of no size, or not finite, is framed as
    /// a point is.
    pub fn frame_sphere(&mut self, center: DVec3, radius: f32) {
        if !radius.is_finite() || radius <= 0.0 {
            self.frame(center);
            return;
        }
        let distance = if self.perspective {
            radius / libm::sinf(FRAMED * FOV / 2.0)
        } else {
            radius / FRAMED / tan_half_fov()
        };
        let [least, most] = DISTANCES;
        self.ease_to(Pose {
            pivot: center,
            distance: distance.clamp(least, most),
            ..self.pose
        });
    }

    /// In orthographic, slides the pivot along the view onto the plane through the origin facing
    /// `normal`. The view doesn't change, but the plane stays in its depth however far it was
    /// panned off, so a switch to perspective orbits the plane's point at the view's center. Not
    /// while it eases, nor while the view runs along the plane.
    pub fn settle(&mut self, normal: Vec3) {
        if self.perspective || self.ease.is_some() {
            return;
        }
        let (forward, normal) = (self.pose.forward().as_dvec3(), normal.as_dvec3());
        let across = forward.dot(normal);
        if across.abs() >= f64::from(GRAZING) {
            self.pose.pivot -= forward * (self.pose.pivot.dot(normal) / across);
        }
    }

    /// Where it is, its facing and its distance back. While it eases, on the way from the ease's
    /// start: the turn slerped, the distance changing by the same factor each moment, so it neither
    /// spins nor rushes.
    fn current(&self) -> (DVec3, Quat, f32) {
        let pose = &self.pose;
        let Some(ease) = &self.ease else {
            return (pose.pivot, pose.rotation(), pose.distance);
        };
        let t = (ease.elapsed / EASE).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let distance = ease.distance * libm::powf(pose.distance / ease.distance, s);
        let rotation = ease.rotation.slerp(pose.rotation(), s);
        (
            ease.pivot.lerp(pose.pivot, f64::from(s)),
            rotation,
            distance,
        )
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

    /// Moves the camera as `drag` does, by `pixels` over `view`, in the main window's physical
    /// pixels, `scale` to a point: the cursor's for a pan, the mouse's raw motion for the drags
    /// that grab the cursor.
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
        self.pose.pivot -= (by * per_pixel).as_dvec3();
    }

    /// Turns the camera `by` radians, as the cursor went across and down: about the pivot if
    /// `orbit`, as though grabbing the world, so dragging right or down takes the camera left or
    /// up; else where it is, looking the way the cursor goes. The roll turns the drag first, so it
    /// turns the view the cursor's way however it's rolled.
    fn turn(&mut self, by: Vec2, orbit: bool) {
        let position = self.pose.pivot - (self.pose.forward() * self.pose.distance).as_dvec3();
        let (sin, cos) = libm::sincosf(self.pose.roll);
        let up = -by.y;
        let (across, up) = (by.x * cos - up * sin, by.x * sin + up * cos);
        self.pose.yaw = wrap(self.pose.yaw - across);
        self.pose.pitch = (self.pose.pitch + up).clamp(-FRAC_PI_2, FRAC_PI_2);
        if !orbit {
            self.pose.pivot = position + (self.pose.forward() * self.pose.distance).as_dvec3();
        }
    }

    /// Scales the distance by `by`, as far as it goes, and moves the pivot that much of the way
    /// toward `toward`, on the plane through the pivot facing the view, so that it stays where it
    /// is in the view. At either end of the distances, it moves nothing.
    fn dolly(&mut self, by: f32, toward: DVec3) {
        let [least, most] = DISTANCES;
        let distance = (self.pose.distance * by).clamp(least, most);
        if distance == self.pose.distance {
            return;
        }
        let taken = distance / self.pose.distance;
        self.pose.pivot = toward + (self.pose.pivot - toward) * f64::from(taken);
        self.pose.distance = distance;
    }

    /// The point at `at`, in the main window's pixels, on the plane through the pivot facing the
    /// view that `view` shows.
    pub fn under(&self, at: [f64; 2], view: Viewport) -> DVec3 {
        let [width, height] = view.size.map(|side| side as f32);
        let x = (at[0] - f64::from(view.origin[0])) as f32 / width * 2.0 - 1.0;
        let y = 1.0 - (at[1] - f64::from(view.origin[1])) as f32 / height * 2.0;
        let half = self.half_height();
        let across = Vec3::new(x * half * width / height, y * half, 0.0);
        self.pose.pivot + (self.pose.rotation() * across).as_dvec3()
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
            pivot: DVec3::new(3.0, -2.0, 1.0),
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

    /// The mouse's raw motion by `by`.
    fn motion(camera: &mut OrbitCamera, by: Vec2) {
        let event = Event::MouseMotion {
            x: f64::from(by.x),
            y: f64::from(by.y),
        };
        let _ = send(camera, event, false, Tool::Select);
    }

    /// Drags `button` from `from` to `to` in a few moves, with Alt if `alt`, then lets go. Each
    /// move is raw motion and a cursor move by as much, as a mouse at the OS's default pointer
    /// speed gives.
    fn drag(camera: &mut OrbitCamera, button: MouseButton, alt: bool, from: Vec2, to: Vec2) {
        move_to(camera, from);
        self::button(camera, button, true, alt, Tool::Select);
        let step = (to - from) / 4.0;
        for steps in 1..=4 {
            motion(camera, step);
            move_to(camera, from + step * steps as f32);
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
        camera.viewpoint().position.as_vec3()
    }

    fn assert_near(got: Vec2, want: Vec2, what: &str) {
        assert!(got.abs_diff_eq(want, 0.01), "{what}: {got}, not {want}");
    }

    /// Runs the ease out.
    fn eased(camera: &mut OrbitCamera) {
        camera.update(Duration::from_secs(1));
        assert!(camera.ease.is_none());
    }

    /// Framing a point eases to look at it, its facing and distance kept, in both projections: the
    /// point ends in the view's middle, the camera as far from it as it was from the pivot.
    #[test]
    fn framing_eases_to_look_at_a_point_turned_and_as_far_back_as_it_was() {
        for perspective in [false, true] {
            let mut camera = turned(perspective);
            let before = camera.viewpoint();
            let point = Vec3::new(-4.0, 6.0, 2.5);
            camera.frame(point.as_dvec3());
            assert!(camera.eases());
            eased(&mut camera);
            let after = camera.viewpoint();
            assert!(after.rotation.abs_diff_eq(before.rotation, 1e-6));
            assert_eq!(after.projection, before.projection);
            let middle = Vec2::from(VIEW.origin.map(|at| at as f32))
                + Vec2::from(VIEW.size.map(|side| side as f32)) / 2.0;
            assert_near(pixel(&camera, point), middle, "the point");
            let distance = eye(&camera).distance(point);
            assert!((distance - 12.0).abs() < 1e-4, "{distance}");
        }
    }

    /// Framing a sphere centers it, its facing kept, from as far back as it fills 80% of the view's
    /// height: in perspective, its edge that high above the middle as seen from the eye;
    /// orthographically, the view as tall as its diameter over 0.8, whatever the distance. A sphere
    /// too large or small for the distances goes as far as they allow.
    #[test]
    fn framing_a_sphere_fills_80_percent_of_the_views_height() {
        let center = Vec3::new(-4.0, 6.0, 2.5);
        for perspective in [false, true] {
            let mut camera = turned(perspective);
            let before = camera.viewpoint();
            camera.frame_sphere(center.as_dvec3(), 3.0);
            eased(&mut camera);
            let after = camera.viewpoint();
            assert!(after.rotation.abs_diff_eq(before.rotation, 1e-6));
            let middle = Vec2::from(VIEW.origin.map(|at| at as f32))
                + Vec2::from(VIEW.size.map(|side| side as f32)) / 2.0;
            assert_near(pixel(&camera, center), middle, "the center");
            if !perspective {
                // The sphere's top, a radius above its center in the view's up, is 80% of the way
                // from the middle to the view's top edge. (In perspective the eye's tangent to the
                // sphere is, and a point at the center's depth is nearer the middle.)
                let up = after.rotation * Vec3::Y;
                let top = pixel(&camera, center + up * 3.0);
                let want = middle.y - 0.8 * VIEW.size[1] as f32 / 2.0;
                assert!((top.y - want).abs() < 0.5, "{} vs {want}", top.y);
            }
        }
        let mut camera = turned(false);
        camera.frame_sphere(center.as_dvec3(), 3.0);
        eased(&mut camera);
        let Projection::Orthographic { height, .. } = camera.viewpoint().projection else {
            panic!("orthographic");
        };
        assert!((height - 2.0 * 3.0 / 0.8).abs() < 1e-4, "{height}");
        let mut camera = turned(true);
        camera.frame_sphere(center.as_dvec3(), 3.0);
        eased(&mut camera);
        let distance = eye(&camera).distance(center);
        let want = 3.0 / libm::sinf(0.8 * FOV / 2.0);
        assert!((distance - want).abs() < 1e-3, "{distance} vs {want}");
        for (radius, want) in [
            (1e-9, 0.01),
            (0.0, 12.0),
            (f32::NAN, 12.0),
            (1e12, 100_000.0),
        ] {
            let mut camera = turned(true);
            camera.frame_sphere(center.as_dvec3(), radius);
            eased(&mut camera);
            let distance = eye(&camera).distance(center);
            assert!(
                (distance - want).abs() < want * 1e-4,
                "{radius}: {distance}"
            );
        }
    }

    #[test]
    fn it_starts_from_the_front_seeing_10_units_about_the_origin_orthographically() {
        let camera = OrbitCamera::new().viewpoint();
        assert_eq!(camera.rotation, Quat::IDENTITY);
        assert!(
            camera
                .position
                .abs_diff_eq(DVec3::new(0.0, 0.0, 8.660254), 1e-5),
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
        let top = camera.pose.pivot.as_vec3() + up * camera.half_height();
        let center = pixel(&camera, camera.pose.pivot.as_vec3());
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
            assert_near(pixel(&camera, point.as_vec3()), at, "before");
            for (lines, factor) in [(3.0, 1.0 / 1.331), (-5.0, 1.61051)] {
                let distance = camera.pose.distance;
                assert_eq!(scroll(&mut camera, lines), ControlFlow::Break(()));
                let ratio = camera.pose.distance / distance;
                assert!((ratio - factor).abs() < 1e-4, "{lines} lines: {ratio}");
                assert_near(
                    pixel(&camera, point.as_vec3()),
                    at,
                    &format!("{lines} lines"),
                );
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
            assert_near(pixel(&camera, point.as_vec3()), to, &what);
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
            assert_near(pixel(&camera, point.as_vec3()), from, "the press");
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

    /// Looking about, orbiting and dollying go by raw motion alone and want the cursor grabbed,
    /// which holds it still, so they go on past the screen's edges. Panning goes by the cursor
    /// alone and grabs nothing.
    #[test]
    fn turning_and_dollying_go_by_the_mouses_motion_and_panning_by_the_cursor() {
        for (perspective, button, alt, what) in [
            (true, MouseButton::Right, false, Move::Look),
            (true, MouseButton::Left, true, Move::Orbit),
            (true, MouseButton::Right, true, Move::Dolly),
            (false, MouseButton::Middle, false, Move::Pan),
        ] {
            let mut camera = turned(perspective);
            let from = Vec2::new(400.0, 300.0);
            move_to(&mut camera, from);
            assert!(!camera.grabs(), "{what:?}, before the press");
            self::button(&mut camera, button, true, alt, Tool::Select);
            assert_eq!(camera.moving(), Some(what));
            let grabs = what != Move::Pan;
            assert_eq!(camera.grabs(), grabs, "{what:?}");
            let pose = camera.pose;
            // The cursor held, as a grab holds it, while the mouse goes on.
            motion(&mut camera, Vec2::new(30.0, -20.0));
            assert_eq!(
                camera.pose != pose,
                grabs,
                "{what:?}, by the mouse's motion"
            );
            let pose = camera.pose;
            move_to(&mut camera, from + Vec2::new(30.0, -20.0));
            assert_eq!(camera.pose != pose, !grabs, "{what:?}, by the cursor");
            self::button(&mut camera, button, false, alt, Tool::Select);
            assert!(!camera.grabs(), "{what:?}, let go");
        }
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
            let moved = pixel(&camera, point.as_vec3()) - Vec2::new(600.0, 350.0);
            assert!(
                moved.x < -5.0 && moved.y.abs() < moved.x.abs() / 10.0,
                "looking about, rolled {roll}: {moved}"
            );
            // Orbiting to the right takes what's nearer than the pivot along with the cursor, as
            // though the world were grabbed.
            let mut camera = level(roll);
            let nearer =
                camera.pose.pivot.as_vec3() - camera.pose.forward() * camera.pose.distance / 2.0;
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
        assert!(
            flown.abs_diff_eq((forward * 6.0).as_dvec3(), 1e-4),
            "{flown}"
        );
        key(&mut camera, Key::KeyW, false);
        key(&mut camera, Key::ShiftLeft, true);
        key(&mut camera, Key::KeyD, true);
        let (pivot, right) = (camera.pose.pivot, camera.pose.rotation() * Vec3::X);
        camera.update(Duration::from_millis(500));
        let flown = camera.pose.pivot - pivot;
        assert!(
            flown.abs_diff_eq((right * 18.0).as_dvec3(), 1e-3),
            "{flown}"
        );

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

    /// Asserts that `seen` sees as `from` does, for `what`: from the same place, turned the same
    /// way, through the same projection, except how deep an orthographic one sees, which the
    /// camera's reaches past.
    fn assert_sees_as(seen: &Viewpoint, from: &Viewpoint, what: &str) {
        let close = seen.position.abs_diff_eq(from.position, 1e-4);
        assert!(close, "{what}: at {}, not {}", seen.position, from.position);
        // By the axes the turns give, which a quaternion and its negation give alike.
        for axis in [Vec3::X, Vec3::Y] {
            let (got, want) = (seen.rotation * axis, from.rotation * axis);
            assert!(
                got.abs_diff_eq(want, 1e-5),
                "{what}: {axis} turned to {got}, not {want}"
            );
        }
        match (seen.projection, from.projection) {
            (
                Projection::Orthographic { height, .. },
                Projection::Orthographic { height: want, .. },
            ) => assert!(
                (height - want).abs() < want * 1e-5,
                "{what}: {height}, not {want}"
            ),
            (Projection::Perspective { fov, .. }, Projection::Perspective { fov: want, .. }) => {
                assert_eq!(fov, want, "{what}");
            }
            (got, want) => panic!("{what}: {got:?}, not {want:?}"),
        }
    }

    #[test]
    fn adopting_a_viewpoint_sees_from_it() {
        // Turned every way, rolled, and at both poles, where a roll folds into the yaw.
        let turns = [
            (0.4, -0.3, 0.2),
            (-2.4, 0.9, 2.8),
            (2.0, FRAC_PI_2, 0.0),
            (-1.0, -FRAC_PI_2, 0.7),
        ];
        for perspective in [false, true] {
            for (yaw, pitch, roll) in turns {
                let mut from = turned(perspective);
                from.pose = Pose {
                    yaw,
                    pitch,
                    roll,
                    ..from.pose
                };
                let from = from.viewpoint();
                // Easing elsewhere, from the other projection.
                let mut camera = OrbitCamera::new();
                camera.perspective = !perspective;
                camera.snap(Side::Right);
                camera.adopt(&from);
                assert!(!camera.eases());
                assert_eq!(camera.perspective, perspective);
                let what = format!("({yaw}, {pitch}, {roll}), perspective {perspective}");
                assert_sees_as(&camera.viewpoint(), &from, &what);
            }
        }
        // A game's own, which sees a depth of its own: rolled, 20 units tall about (3, 1).
        let game = Viewpoint {
            position: DVec3::new(3.0, 1.0, 10.0),
            rotation: Quat::from_rotation_z(0.3),
            projection: Projection::Orthographic {
                height: 20.0,
                near: -1.0,
                far: 1.0,
            },
        };
        let mut camera = turned(true);
        camera.adopt(&game);
        assert_sees_as(&camera.viewpoint(), &game, "a game's");
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
