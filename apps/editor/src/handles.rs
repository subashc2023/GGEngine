//! The editor's transform handles: what the Move, Rotate and Scale tools show over the entity the
//! Inspector has picked last, at its origin, along its own axes, or Move's and Rotate's along the
//! world's, as the toolbar's space says, and as big on screen however far it is, which the left
//! button drags to move, turn and scale it, and the others picked with it by the same change, each
//! drag one edit of the Inspector's, through its link; and the press over the view off them, which
//! asks for a pick of what's under the cursor.

use std::f32::consts::{FRAC_PI_4, PI, TAU};

use gg_engine::egui::{self, Color32, Painter, Pos2, Rgba, Shape, Stroke};
use gg_engine::grid;
use gg_engine::math::{DMat3, DVec3, Mat3, Mat4, Quat, Vec2, Vec3, Vec4, libm};
use gg_engine::reflect::Value;
use gg_engine::{
    EntityId, Event, Key, MouseButton, Pose, Projection, Transform, Viewpoint, Viewport,
};

use crate::camera::{Screen, Space, Tool};

/// How long the handles are, in points: a segment of their length along the camera's right, at
/// their origin, spans it on screen.
const LENGTH: f32 = 100.0;
/// How near a handle a press or the cursor hits it, in points.
const HIT: f32 = 6.0;
/// How wide the handles' lines are, in points.
const WIDTH: f32 = 2.5;
/// The radius of Move's center handle, and half the side of Scale's, in points.
const CENTER: f32 = 6.0;
/// Half the side of the squares at the ends of Scale's axes, in points.
const SQUARE: f32 = 4.5;
/// How long the heads of Move's arrows are, and half how wide, in points.
const HEAD: (f32, f32) = (14.0, 5.0);
/// Where Move's squares are along each of their plane's two axes, from the origin, in the
/// handles' lengths.
const PLANE: (f32, f32) = (0.2, 0.4);
/// The radius of Rotate's outer ring, in the handles' lengths.
const OUTER: f32 = 1.2;
/// How many segments a ring is drawn and hit in.
const SEGMENTS: usize = 64;
/// How much of a ring's half away from the camera shows, of the half toward it.
const BACK: f32 = 0.35;
/// The angles, from pointing at the camera for an axis and from edge-on for a plane, within which
/// one isn't drawn or hit, and from which it shows whole: 8° and 20°, fading between.
const HIDDEN: f32 = 8.0 * PI / 180.0;
const SHOWN: f32 = 20.0 * PI / 180.0;
/// How squarely a ring's plane faces the camera, as |a · d| for its axis and the way to the
/// camera, below which a drag turns it by the cursor's travel across it rather than by the angle
/// about it: cos 85°, Godot's.
const EDGE_ON: f32 = 0.087_155_74;
/// The least of a ray's direction across a plane, |r · n|, at which it meets the plane.
const GRAZE: f32 = 1e-6;
/// How far snapping steps: half a unit along the handles' axes, 45°, and half the scale a drag
/// started from.
const MOVE_STEP: f32 = 0.5;
const TURN_STEP: f32 = FRAC_PI_4;
const SCALE_STEP: f32 = 0.5;
/// The least a drag scales by, ImGuizmo's: so that none mirrors or flattens what it scales.
const LEAST: f32 = 0.001;
/// The most steps [`orthogonal`] takes, past the handful it settles in however unevenly the matrix
/// scales: a cap for one that isn't finite, which never settles.
const STEPS: usize = 10;
/// The color of the handle under the cursor, or dragged.
const LIT: Color32 = Color32::from_rgb(255, 210, 0);
/// The color of the handles no axis has: Move's center, Scale's and Rotate's outer ring.
const PLAIN: Color32 = Color32::from_gray(220);

/// One of the handles: what a press grabs, and what's lit under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Along one of the entity's axes, 0 to 2 for X to Z: Move's arrows and Scale's axes.
    Axis(usize),
    /// Across the plane square to one of its axes: Move's squares.
    Plane(usize),
    /// At its origin: Move's, in the view's plane, and Scale's, all three ways at once.
    Center,
    /// About one of its axes: Rotate's rings.
    Ring(usize),
    /// About the view's axis: Rotate's outer ring.
    Outer,
}

/// The entity the Inspector has picked last, the active one, while it's alive with a transform,
/// as the handles see it through the Inspector's link: the handles sit at it.
#[derive(Clone, Copy, Debug)]
pub struct Picked {
    pub entity: EntityId,
    /// Its world's serial, which another world changes.
    pub serial: u64,
    /// Its own transform, as the latest tick left it.
    pub local: Transform,
    /// Where its parent carries it: the parent's pose, or the identity for a root.
    pub parent: Pose,
    /// How many undos and redos the Inspector has asked for, at another of which a drag ends.
    pub stepped: u64,
    /// Whether a set made through the link waits for the Inspector's tick.
    pub waiting: bool,
    /// Whether a drag moves it, as it doesn't one under another entity picked, whose drag carries
    /// it: the handles then show it as the link places it, a drag's value aside.
    pub moves: bool,
}

/// Another entity picked, which a drag of the handles moves, turns or scales with the active one,
/// by the same change, as the Inspector's link places it: its own transform, and where its parent
/// carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Along {
    pub entity: EntityId,
    pub local: Transform,
    pub parent: Pose,
}

/// What the handles read of the window as an event comes, beside the event: what the camera
/// reads, the view, the scale and Alt; whether the command key is held, Ctrl, or Cmd on macOS,
/// which snaps, and Shift, which with it has a pick add to what's picked; and where the cursor is,
/// in the main window's pixels.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub screen: Screen,
    pub command: bool,
    pub shift: bool,
    pub cursor: Option<[f64; 2]>,
}

/// What else the handles go by as an event comes: the toolbar's tool, and the space Move's and
/// Rotate's axes are in; what the view on screen sees from, while it sees through the editor's
/// camera; the pick, the active one, and the others a drag moves with it; whether the camera
/// drags, which no drag of theirs nor pick starts while it does; and whether it eases, as Home and
/// the gizmo's snaps have it, which no drag of theirs starts while it does, since the view would
/// turn under it.
#[derive(Clone, Copy, Debug)]
pub struct Aim<'a> {
    pub tool: Tool,
    pub space: Space,
    pub viewpoint: Option<Viewpoint>,
    pub picked: Option<Picked>,
    pub along: &'a [Along],
    pub camera: bool,
    pub easing: bool,
}

/// What the editor does with an event, as the handles say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reply {
    /// Nothing of the handles': the camera has it.
    Pass,
    /// Takes it from the camera: a press that grabs a handle, or another press, the wheel or Home
    /// while a drag lasts.
    Take,
    /// Sets the field of the entity whose ID it is, the pick's, to the value the drag gives it, and
    /// the same field of the others it moves with it, as [`Handles::sets`] gives them all, the
    /// camera still seeing the event, such as the cursor's move.
    Set(EntityId, Field),
    /// Ends the drag's edit, the camera still seeing the event, such as the button's release.
    Seal,
    /// Asks which entity the view shows at the cursor, in the main window's pixels: a press of the
    /// left button over the view off the handles, with a tool that picks, which the camera still
    /// sees.
    Pick([f64; 2]),
    /// Asks what shading took and made at the cursor, in the main window's pixels: a press of the
    /// left button over the view with the Probe tool, which shows no handles; the camera doesn't
    /// see it.
    Probe([f64; 2]),
}

/// A field of a transform, which a drag sets whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    /// In `f64`, as translations are.
    Translation(DVec3),
    /// A unit quaternion.
    Rotation(Quat),
    Scale(Vec3),
}

impl Field {
    /// The field of `transform` that `tool`'s handles drag, if they drag one.
    fn of(tool: Tool, transform: &Transform) -> Option<Self> {
        match tool {
            Tool::Move => Some(Self::Translation(transform.translation)),
            Tool::Rotate => Some(Self::Rotation(transform.rotation)),
            Tool::Scale => Some(Self::Scale(transform.scale)),
            Tool::Select | Tool::Pan | Tool::Orbit | Tool::Probe => None,
        }
    }

    /// Its path in a transform, as the Inspector sets it.
    pub fn path(self) -> &'static str {
        match self {
            Self::Translation(_) => "translation",
            Self::Rotation(_) => "rotation",
            Self::Scale(_) => "scale",
        }
    }

    /// Its value, as reflection has it.
    pub fn value(self) -> Value {
        match self {
            Self::Translation(v) => Value::DVec3(v),
            Self::Scale(v) => Value::Vec3(v),
            Self::Rotation(q) => Value::Quat(q),
        }
    }

    /// `transform` with it set.
    fn set(self, transform: Transform) -> Transform {
        match self {
            Self::Translation(translation) => Transform {
                translation,
                ..transform
            },
            Self::Rotation(rotation) => Transform {
                rotation,
                ..transform
            },
            Self::Scale(scale) => Transform { scale, ..transform },
        }
    }

    /// Whether a transform takes it: finite, and a rotation a unit, as reflection has it.
    fn takes(self) -> bool {
        match self {
            Self::Translation(v) => v.is_finite(),
            Self::Scale(v) => v.is_finite(),
            Self::Rotation(q) => q.is_finite() && q.is_normalized(),
        }
    }
}

/// The transform handles, which the editor's layer feeds its events, as it does its camera, and
/// paints over the view: the drag under way, if one is, and the value the last one set, which the
/// handles show until the Inspector's tick has made it.
#[derive(Default)]
pub struct Handles {
    drag: Option<Drag>,
    kept: Option<Kept>,
}

/// The value a drag set last, for the entity of its world it set it for.
#[derive(Clone, Copy, Debug)]
struct Kept {
    entity: EntityId,
    serial: u64,
    transform: Transform,
}

impl Handles {
    /// Whether a drag of the handles goes on.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Acts on `event`, as the editor reads the window, `reading`, and as `aim` says, and returns
    /// what the editor does with it. A press of the left button over the view grabs a handle under
    /// it, with Move, Rotate or Scale, and drags it until it's let go, or the window loses the
    /// focus; each move of the cursor sets the dragged field to what the drag gives it from where
    /// it started, when that changes and a transform takes it, and pressing or letting go of the
    /// command key, which snaps, sets it again. While it lasts, it takes every other press, the
    /// wheel and Home from the camera. Off the handles, with a tool that picks, the press asks for
    /// a pick of what's under the cursor, and with Probe, which shows none, for a probe of the
    /// pixel; but none starts with Alt held, which orbits, nor while the camera drags; and while
    /// the camera eases, a press on a handle grabs nothing.
    pub fn event(&mut self, event: &Event<'_>, reading: &Reading, aim: &Aim) -> Reply {
        if let Some(drag) = &mut self.drag {
            return match *event {
                Event::CursorMoved { x, y } => {
                    drag.cursor = Vec2::new(x as f32, y as f32);
                    drag.snap = reading.command;
                    self.apply()
                }
                Event::MouseButton {
                    button: MouseButton::Left,
                    pressed: false,
                }
                | Event::Focused(false) => {
                    self.end();
                    Reply::Seal
                }
                Event::MouseButton { pressed: true, .. }
                | Event::Scroll(_)
                | Event::Key {
                    key: Key::Home,
                    pressed: true,
                    ..
                } => Reply::Take,
                Event::Key { .. } if reading.command != drag.snap => {
                    drag.snap = reading.command;
                    self.apply()
                }
                _ => Reply::Pass,
            };
        }
        let Event::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        } = *event
        else {
            return Reply::Pass;
        };
        let picks = matches!(
            aim.tool,
            Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale
        );
        let Some(cursor) = reading.cursor else {
            return Reply::Pass;
        };
        if aim.camera || reading.screen.alt {
            return Reply::Pass;
        }
        if aim.tool == Tool::Probe {
            return Reply::Probe(cursor);
        }
        if !picks {
            return Reply::Pass;
        }
        let at = Vec2::new(cursor[0] as f32, cursor[1] as f32);
        let Some((picked, grabbed)) = self.under(at, reading, aim) else {
            return Reply::Pick(cursor);
        };
        // Taken all the same, rather than a pick of what's behind the handle.
        if aim.easing {
            return Reply::Take;
        }
        // A handle that can't be dragged from there, as past its plane's horizon, takes the press
        // all the same.
        let start = self.shown(&picked);
        self.drag = Drag::start(grabbed, &picked, start, reading.command);
        if let Some(drag) = &mut self.drag {
            drag.along = aim.along.to_vec();
        }
        Reply::Take
    }

    /// Ends the drag, if one goes on, keeping the value it set to show until the Inspector's tick
    /// has made it, for a pick it moved; and returns whether one did, whose edit the editor seals.
    pub fn end(&mut self) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        self.kept = drag.moves.then(|| Kept {
            entity: drag.entity,
            serial: drag.serial,
            transform: drag.last.set(drag.start),
        });
        true
    }

    /// The sets the drag going on made last, as it replied [`Reply::Set`]: the pick's field, if
    /// the drag moves it, and the same field of each of the others it moves with it, changed as
    /// the pick's is from where each was as the drag started: moved by the same move in the world,
    /// under a parent whose 3x3 has an inverse; turned about its own origin by the same turn,
    /// about the world's axis, or under Local about its own axis as the pick's ring is; or scaled
    /// along its own axes by the same factors.
    pub fn sets(&self) -> Vec<(EntityId, Field)> {
        let Some(drag) = &self.drag else {
            return Vec::new();
        };
        let own = drag.moves.then_some((drag.entity, drag.last));
        let Some(change) = drag.change else {
            return own.into_iter().collect();
        };
        let along = drag.along.iter().filter_map(|along| {
            let field = change.of(along)?;
            field.takes().then_some((along.entity, field))
        });
        own.into_iter().chain(along).collect()
    }

    /// Keeps the handles to the pick, `picked`: a drag ends as the pick changes or dies, the world
    /// is another, or the Inspector undoes or redoes, which ends the drag's edit, the handles
    /// showing the world's from then on; and the value a drag set shows, for the pick it set it
    /// for, until the Inspector's tick has made it. Returns whether a drag ended, whose edit the
    /// editor seals.
    pub fn hold(&mut self, picked: Option<&Picked>) -> bool {
        let same = |entity, serial| {
            picked.is_some_and(|picked| (picked.entity, picked.serial) == (entity, serial))
        };
        let waiting = picked.is_some_and(|picked| picked.waiting);
        self.kept = self
            .kept
            .filter(|kept| waiting && same(kept.entity, kept.serial));
        let Some(drag) = &self.drag else {
            return false;
        };
        let stepped = picked.is_some_and(|picked| picked.stepped != drag.stepped);
        if same(drag.entity, drag.serial) && !stepped {
            return false;
        }
        self.drag = None;
        true
    }

    /// Paints the handles `tool` shows for `picked` with `painter`, the view's layer's, as `sight`
    /// sees them, `ppp` egui's pixels per point, Move's and Rotate's along `space`'s axes, the
    /// pair `(tool, space)` as the toolbar has it: lighting the one being dragged, or else the one
    /// at `hover`, in pixels, which the editor gives while the cursor is over the view, the camera
    /// neither drags nor eases, and Alt isn't held.
    pub fn paint(
        &self,
        painter: &Painter,
        ppp: f32,
        sight: &Sight,
        (tool, space): (Tool, Space),
        picked: Option<&Picked>,
        hover: Option<Vec2>,
    ) {
        let Some(picked) = picked else {
            return;
        };
        let Some(placed) = self.placed(sight, tool, space, picked) else {
            return;
        };
        let lit = match &self.drag {
            Some(drag) => (drag.entity == picked.entity).then_some(drag.handle),
            None => hover.and_then(|at| placed.hit(sight, tool, at)),
        };
        let pen = Pen {
            painter,
            ppp,
            scale: sight.scale,
        };
        let hue = |handle: Handle, color: Color32, alpha: f32| {
            let color = if lit == Some(handle) { LIT } else { color };
            color.gamma_multiply(alpha)
        };
        let Placed {
            origin,
            center,
            axes,
            length,
            ..
        } = placed;
        let axis = |i: usize| sight.segment(origin, origin + axes[i] * length);
        match tool {
            Tool::Move => {
                for k in (0..3).filter(|&k| placed.plane[k] > 0.0) {
                    if let Some(corners) = placed.square(sight, k) {
                        let color = hue(Handle::Plane(k), color(k), placed.plane[k]);
                        pen.polygon(corners, color.gamma_multiply(0.35), color);
                    }
                }
                for i in (0..3).filter(|&i| placed.axis[i] > 0.0) {
                    if let Some((line, [_, end])) = axis(i) {
                        let color = hue(Handle::Axis(i), color(i), placed.axis[i]);
                        pen.line(line, color);
                        if end == 1.0 {
                            pen.head(line, color);
                        }
                    }
                }
                let color = hue(Handle::Center, PLAIN, 1.0);
                pen.circle(center, CENTER, color.gamma_multiply(0.3), color);
            }
            Tool::Rotate => {
                let outer = hue(Handle::Outer, PLAIN, 1.0);
                pen.circle(center, OUTER * LENGTH, Color32::TRANSPARENT, outer);
                for i in 0..3 {
                    for (front, line, _) in placed.ring(sight, i) {
                        let alpha = if front { 1.0 } else { BACK };
                        pen.line(line, hue(Handle::Ring(i), color(i), alpha));
                    }
                }
            }
            Tool::Scale => {
                for i in (0..3).filter(|&i| placed.axis[i] > 0.0) {
                    if let Some((line, [_, end])) = axis(i) {
                        let color = hue(Handle::Axis(i), color(i), placed.axis[i]);
                        pen.line(line, color);
                        if end == 1.0 {
                            pen.square(line[1], SQUARE, color);
                        }
                    }
                }
                pen.square(center, CENTER, hue(Handle::Center, PLAIN, 1.0));
            }
            Tool::Select | Tool::Pan | Tool::Orbit | Tool::Probe => {}
        }
    }

    /// The set the drag makes with the cursor where it last was, if the value it gives changed
    /// and a transform takes it.
    fn apply(&mut self) -> Reply {
        let Some(drag) = &mut self.drag else {
            return Reply::Pass;
        };
        match drag.value(drag.cursor, drag.snap) {
            Some((field, change)) if field.takes() && field != drag.last => {
                drag.last = field;
                drag.change = Some(change);
                Reply::Set(drag.entity, field)
            }
            _ => Reply::Pass,
        }
    }

    /// The transform the handles show `picked` at: the drag's value while it lasts, and after it,
    /// the value it set until the Inspector's tick has made it, for a pick the drag moves;
    /// otherwise its own.
    fn shown(&self, picked: &Picked) -> Transform {
        if let Some(drag) = &self.drag
            && drag.entity == picked.entity
            && drag.moves
        {
            return drag.last.set(drag.start);
        }
        match self.kept {
            Some(kept)
                if picked.waiting
                    && (kept.entity, kept.serial) == (picked.entity, picked.serial) =>
            {
                kept.transform
            }
            _ => picked.local,
        }
    }

    /// Where the handles `tool` shows for `picked` are, as `sight` sees them, Move's and Rotate's
    /// along `space`'s axes, and Scale's along the entity's own: none for a tool without handles,
    /// nor for Move while the parent's space has no room to move in.
    fn placed(&self, sight: &Sight, tool: Tool, space: Space, picked: &Picked) -> Option<Placed> {
        let local = self.shown(picked);
        match tool {
            Tool::Move if carried(&picked.parent).is_none() => None,
            Tool::Move | Tool::Rotate => place(sight, space, &picked.parent, &local),
            Tool::Scale => place(sight, Space::Local, &picked.parent, &local),
            Tool::Select | Tool::Pan | Tool::Orbit | Tool::Probe => None,
        }
    }

    /// The handle under `at`, in pixels, of those `aim` shows for its pick as the view on screen
    /// sees them: the pick, and what a press there grabs.
    fn under(&self, at: Vec2, reading: &Reading, aim: &Aim) -> Option<(Picked, Grabbed)> {
        let picked = aim.picked?;
        let sight = Sight::new(aim.viewpoint?, reading.screen.view?, reading.screen.scale)?;
        let placed = self.placed(&sight, aim.tool, aim.space, &picked)?;
        let handle = placed.hit(&sight, aim.tool, at)?;
        let grabbed = Grabbed {
            tool: aim.tool,
            handle,
            at,
            sight,
            placed,
        };
        Some((picked, grabbed))
    }
}

/// What a press grabbed: a handle of `tool`'s at `at`, in pixels, as `sight` saw it `placed`.
struct Grabbed {
    tool: Tool,
    handle: Handle,
    at: Vec2,
    sight: Sight,
    placed: Placed,
}

/// A drag of a handle.
struct Drag {
    /// The entity, its world, and the Inspector's steps, as the drag started.
    entity: EntityId,
    serial: u64,
    stepped: u64,
    tool: Tool,
    handle: Handle,
    /// The entity's transform as the drag started, and where its parent carries it.
    start: Transform,
    parent: Pose,
    /// The view, and the handles, as they were at the press: the camera stays while a drag lasts.
    sight: Sight,
    placed: Placed,
    grab: Grab,
    /// The press, in pixels.
    press: Vec2,
    /// Where the cursor last was, in pixels, and whether the command key was held then, for the
    /// drag to apply again as the command key changes.
    cursor: Vec2,
    snap: bool,
    /// The value it set last, or else the start's, and the change from the start that gave it,
    /// which the others it moves take too.
    last: Field,
    change: Option<Change>,
    /// Whether it moves the pick, and the others it moves with it, as they were as it started.
    moves: bool,
    along: Vec<Along>,
}

/// The change a drag makes, from where it started, which it makes to each entity it moves.
#[derive(Clone, Copy, Debug)]
enum Change {
    /// A move by this, in the world.
    Move(Vec3),
    /// A turn about each entity's own origin by `angle`: about the world's axis `about`, or
    /// about each one's own axis, X to Z, as Local's rings turn.
    Turn { about: About, angle: f32 },
    /// A scale by these factors along each entity's own axes.
    Scale(Vec3),
}

/// The axis of a [`Change::Turn`].
#[derive(Clone, Copy, Debug)]
enum About {
    World(Vec3),
    Own(usize),
}

impl Change {
    /// The field it gives `along` from where it was as the drag started, as
    /// [`Handles::sets`] says: `None` for a move under a parent whose 3x3 has no inverse.
    fn of(self, along: &Along) -> Option<Field> {
        let Along { local, parent, .. } = along;
        Some(match self {
            Self::Move(by) => {
                let into = carried(parent)?.inverse();
                Field::Translation(local.translation + into * by.as_dvec3())
            }
            Self::Turn { about, angle } => {
                let carry = carry(parent);
                let handed = carry.determinant().signum();
                let start = local.rotation.normalize();
                let turned = match about {
                    // As [`Placed::turn`] turns under the world's axes, which a turn of its own
                    // about the axis's projection onto its own axes equals.
                    About::World(axis) => {
                        let own = (carry.transpose() * axis).normalize();
                        Quat::from_axis_angle(own, handed * angle) * start
                    }
                    About::Own(i) => start * Quat::from_axis_angle(Vec3::AXES[i], handed * angle),
                };
                Field::Rotation(turned.normalize())
            }
            Self::Scale(by) => Field::Scale(local.scale * by),
        })
    }
}

/// What a drag goes by, as its press found it.
#[derive(Clone, Copy, Debug)]
enum Grab {
    /// A plane through the origin square to `normal`, which the press's ray met at `from`.
    Plane { normal: Vec3, from: Vec3 },
    /// A ring's point, `grabbed`: where the press's ray met the ring's plane, while the plane
    /// faces the camera more than edge-on and the ray met it; where it met the plane through the
    /// point facing the camera, which an edge-on ring's drag goes by; and which way the point
    /// goes, across the view, as the ring turns.
    Ring {
        open: Option<Vec3>,
        grabbed: Vec3,
        facing: Vec3,
        tangent: Vec3,
    },
    /// The cursor on screen alone: about the origin, for the outer ring, and how far right, for
    /// Scale's center.
    Screen,
}

impl Drag {
    /// The drag of what `grabbed` says, of `picked`, at `start` as the press found it, snapping
    /// if `snap`; `None` where the press's ray doesn't meet the plane it goes by.
    fn start(grabbed: Grabbed, picked: &Picked, start: Transform, snap: bool) -> Option<Self> {
        let Grabbed {
            tool,
            handle,
            at,
            sight,
            placed,
        } = grabbed;
        let ray = sight.ray(at);
        let Placed {
            origin,
            axes,
            toward,
            ..
        } = placed;
        let plane = |normal: Vec3| {
            let from = ray.meet(origin, normal)?;
            Some(Grab::Plane { normal, from })
        };
        let grab = match (tool, handle) {
            (Tool::Move | Tool::Scale, Handle::Axis(i)) => plane(facing(axes[i], toward))?,
            (Tool::Move, Handle::Plane(k)) => plane(axes[k])?,
            (Tool::Move, Handle::Center) => plane(toward)?,
            (Tool::Scale, Handle::Center) | (Tool::Rotate, Handle::Outer) => Grab::Screen,
            (Tool::Rotate, Handle::Ring(i)) => {
                let axis = axes[i];
                let grabbed = placed.grabbed(&sight, i, at)?;
                // The near half's points go the way the ring turns across the view, the far
                // half's the other way.
                let near = (grabbed - origin).dot(toward) >= 0.0;
                let tangent = axis.cross(toward).normalize() * if near { 1.0 } else { -1.0 };
                let open = axis.dot(toward).abs() >= EDGE_ON;
                Grab::Ring {
                    open: open.then(|| ray.meet(origin, axis)).flatten(),
                    grabbed,
                    facing: ray.meet(grabbed, toward)?,
                    tangent,
                }
            }
            _ => return None,
        };
        let last = Field::of(tool, &start)?;
        Some(Self {
            entity: picked.entity,
            serial: picked.serial,
            stepped: picked.stepped,
            tool,
            handle,
            start,
            parent: picked.parent,
            sight,
            placed,
            grab,
            press: at,
            cursor: at,
            snap,
            last,
            change: None,
            moves: picked.moves,
            along: Vec::new(),
        })
    }

    /// The value the drag gives its field with the cursor at `cursor`, in pixels, from where it
    /// started, snapping from there if `snap`, and the change from there that gives it; `None`
    /// where it gives none, as past its plane's horizon, where the drag keeps its last value.
    fn value(&self, cursor: Vec2, snap: bool) -> Option<(Field, Change)> {
        let Placed {
            origin,
            center,
            axes,
            length,
            toward,
            ..
        } = self.placed;
        let ray = self.sight.ray(cursor);
        match (self.tool, self.handle, self.grab) {
            (Tool::Move, handle, Grab::Plane { normal, from }) => {
                let mut by = ray.meet(origin, normal)? - from;
                if let Handle::Axis(i) = handle {
                    by = axes[i] * by.dot(axes[i]);
                }
                if snap {
                    by = axes
                        .iter()
                        .map(|&axis| axis * stepped(by.dot(axis), MOVE_STEP))
                        .sum();
                }
                // Into the parent's space, which the translation is in: in f64, since f32's inverse
                // of a parent turned and scaled very unevenly loses some of the move.
                let into = self.parent.matrix.matrix3.inverse();
                let field = Field::Translation(self.start.translation + into * by.as_dvec3());
                Some((field, Change::Move(by)))
            }
            (Tool::Scale, Handle::Axis(i), Grab::Plane { normal, from }) => {
                let travel = (ray.meet(origin, normal)? - from).dot(axes[i]);
                let mut by = Vec3::ONE;
                by[i] = ratio(1.0 + travel / length, snap);
                Some((Field::Scale(self.start.scale * by), Change::Scale(by)))
            }
            (Tool::Scale, Handle::Center, Grab::Screen) => {
                let travel = (cursor.x - self.press.x) / self.sight.pixels(LENGTH);
                let by = Vec3::splat(ratio(1.0 + travel, snap));
                Some((Field::Scale(self.start.scale * by), Change::Scale(by)))
            }
            (
                Tool::Rotate,
                Handle::Ring(i),
                Grab::Ring {
                    open,
                    grabbed,
                    facing,
                    tangent,
                },
            ) => {
                let axis = axes[i];
                let opened = open.zip(ray.meet(origin, axis));
                let angle = match opened {
                    Some((from, to)) => angle_about(from - origin, to - origin, axis),
                    // Edge-on, or past the ring's plane's horizon: by the travel across it, a
                    // radian for each radius, so that the point grabbed follows the cursor.
                    None => (ray.meet(grabbed, toward)? - facing).dot(tangent) / length,
                };
                let angle = if snap {
                    stepped(angle, TURN_STEP)
                } else {
                    angle
                };
                let about = match self.placed.space {
                    Space::Local => About::Own(i),
                    Space::World => About::World(axis),
                };
                let turned = self.placed.turn(self.start.rotation, axis, angle);
                Some((Field::Rotation(turned), Change::Turn { about, angle }))
            }
            (Tool::Rotate, Handle::Outer, Grab::Screen) => {
                let [from, to] = [self.press, cursor].map(|at| {
                    let by = at - center;
                    libm::atan2f(by.y, by.x)
                });
                let angle = to - from;
                let angle = if snap {
                    stepped(angle, TURN_STEP)
                } else {
                    angle
                };
                // About the view's axis, pointing away from the camera.
                let turned = self.placed.turn(self.start.rotation, -toward, angle);
                let about = About::World(-toward);
                Some((Field::Rotation(turned), Change::Turn { about, angle }))
            }
            _ => None,
        }
    }
}

/// `x` to the nearest multiple of `step`.
fn stepped(x: f32, step: f32) -> f32 {
    (x / step).round() * step
}

/// The ratio a scale goes by, `ratio` from a drag's travel, no less than [`LEAST`], and in steps
/// of [`SCALE_STEP`], at least one, if `snap`.
fn ratio(ratio: f32, snap: bool) -> f32 {
    let ratio = ratio.max(LEAST);
    if snap {
        stepped(ratio, SCALE_STEP).max(SCALE_STEP)
    } else {
        ratio
    }
}

/// The normal of the plane through `axis` that faces `toward` most: `toward` with its part along
/// the axis taken out.
fn facing(axis: Vec3, toward: Vec3) -> Vec3 {
    (toward - axis * toward.dot(axis)).normalize()
}

/// The angle from `from` to `to`, about `axis`, both square to it: counterclockwise as the axis
/// points at the viewer.
fn angle_about(from: Vec3, to: Vec3, axis: Vec3) -> f32 {
    libm::atan2f(from.cross(to).dot(axis), from.dot(to))
}

/// How the view sees the world, for the handles: what it sees from, where it is in the main
/// window, in pixels, and how many pixels a point spans.
#[derive(Clone, Copy, Debug)]
pub struct Sight {
    /// Where its viewpoint is, in the world, which every point it takes and gives is from, in
    /// `f32`, so that what's near the camera is placed as precisely far from the world's origin as
    /// at it.
    eye: DVec3,
    /// Its viewpoint, at `eye`, its position the origin.
    viewpoint: Viewpoint,
    view: Viewport,
    scale: f32,
    /// From the world to Vulkan's clip space, as the renderer has it.
    clip: Mat4,
    aspect: f32,
}

/// A line from the camera through a pixel: where it starts, and which way it goes, a unit vector;
/// in perspective, it goes only forward.
#[derive(Clone, Copy, Debug)]
struct Ray {
    from: Vec3,
    toward: Vec3,
    perspective: bool,
}

impl Ray {
    /// Where it meets the plane through `point` square to `normal`: `None` where it runs along
    /// the plane, and in perspective, behind the camera, which sees only in front of it.
    fn meet(&self, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let across = self.toward.dot(normal);
        let along = (point - self.from).dot(normal) / across;
        // What's no number meets nothing, failing both.
        let meets = across.abs() > GRAZE && (along > 0.0 || !self.perspective);
        let met = self.from + self.toward * along;
        (meets && met.is_finite()).then_some(met)
    }
}

impl Sight {
    /// `viewpoint` seeing in `view`, `scale` pixels a point; `None` for a view without an area, or
    /// a viewpoint whose projection can't project.
    pub fn new(viewpoint: Viewpoint, view: Viewport, scale: f32) -> Option<Self> {
        let [width, height] = view.size;
        let shows = width > 0 && height > 0 && viewpoint.projection.projects();
        if !shows || !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        let aspect = view.aspect();
        let eye = viewpoint.position;
        let viewpoint = viewpoint.relative_to(eye);
        Some(Self {
            eye,
            viewpoint,
            view,
            scale,
            clip: viewpoint.view_projection(aspect),
            aspect,
        })
    }

    /// How many pixels `points` points span.
    fn pixels(&self, points: f32) -> f32 {
        points * self.scale
    }

    /// The near plane's w in clip space, in perspective, short of which nothing shows.
    fn near(&self) -> Option<f32> {
        match self.viewpoint.projection {
            Projection::Perspective { near, .. } => Some(near),
            Projection::Orthographic { .. } => None,
        }
    }

    /// Where clip-space `at`, past the near plane, shows, in pixels.
    fn pixel(&self, at: Vec4) -> Vec2 {
        let [x, y] = self.view.origin.map(|at| at as f32);
        let [width, height] = self.view.size.map(|side| side as f32);
        // Vulkan's clip space has y down, as the window does.
        Vec2::new(
            x + (at.x / at.w + 1.0) / 2.0 * width,
            y + (at.y / at.w + 1.0) / 2.0 * height,
        )
    }

    /// Where `point` shows, in pixels, if it's past the near plane.
    fn project(&self, point: Vec3) -> Option<Vec2> {
        let at = self.clip * point.extend(1.0);
        let past = self.near().is_none_or(|near| at.w >= near);
        past.then(|| self.pixel(at))
    }

    /// Where the segment from `from` to `to` shows, in pixels: its part past the near plane, if
    /// any, and how far along the segment that part's ends are, from 0 to 1. Clipped before the
    /// division by w, so that what's just in front of the camera doesn't show far off.
    fn segment(&self, from: Vec3, to: Vec3) -> Option<([Vec2; 2], [f32; 2])> {
        let (a, b) = (self.clip * from.extend(1.0), self.clip * to.extend(1.0));
        let (mut start, mut end) = (0.0, 1.0);
        if let Some(near) = self.near() {
            let cut = (near - a.w) / (b.w - a.w);
            match (a.w >= near, b.w >= near) {
                (false, false) => return None,
                (true, false) => end = cut,
                (false, true) => start = cut,
                (true, true) => {}
            }
        }
        let ends = [self.pixel(a.lerp(b, start)), self.pixel(a.lerp(b, end))];
        Some((ends, [start, end]))
    }

    /// The point pixel `at` shows on the plane through the world's origin square to `normal`, as
    /// the grid lies, in the world, in `f64`: `None` where its ray runs along the plane, and in
    /// perspective, where it meets it behind the camera.
    pub fn on_plane(&self, at: Vec2, normal: Vec3) -> Option<DVec3> {
        let ray = self.ray(at);
        let (from, toward) = (self.eye + ray.from.as_dvec3(), ray.toward.as_dvec3());
        let normal = normal.as_dvec3();
        let across = toward.dot(normal);
        let along = -from.dot(normal) / across;
        // What's no number meets nothing, failing both.
        let meets = across.abs() > f64::from(GRAZE) && (along > 0.0 || !ray.perspective);
        let met = from + toward * along;
        (meets && met.is_finite()).then_some(met)
    }

    /// The ray through pixel `at`.
    fn ray(&self, at: Vec2) -> Ray {
        let [x0, y0] = self.view.origin.map(|at| at as f32);
        let [width, height] = self.view.size.map(|side| side as f32);
        let x = (at.x - x0) / width * 2.0 - 1.0;
        let y = (at.y - y0) / height * 2.0 - 1.0;
        let Viewpoint {
            position, rotation, ..
        } = self.viewpoint;
        match self.viewpoint.projection {
            Projection::Perspective { fov, .. } => {
                let tan = libm::tanf(fov / 2.0);
                let toward = Vec3::new(x * tan * self.aspect, -y * tan, -1.0);
                Ray {
                    from: position.as_vec3(),
                    toward: (rotation * toward).normalize(),
                    perspective: true,
                }
            }
            Projection::Orthographic { height, .. } => {
                let half = height / 2.0;
                let across = Vec3::new(x * half * self.aspect, -y * half, 0.0);
                Ray {
                    from: position.as_vec3() + rotation * across,
                    toward: rotation * Vec3::NEG_Z,
                    perspective: false,
                }
            }
        }
    }

    /// Which way the camera is from `point`, a unit vector.
    fn toward(&self, point: Vec3) -> Vec3 {
        match self.viewpoint.projection {
            Projection::Perspective { .. } => {
                (self.viewpoint.position.as_vec3() - point).normalize()
            }
            Projection::Orthographic { .. } => self.viewpoint.rotation * Vec3::Z,
        }
    }
}

/// The handles as a sight sees them about an entity.
#[derive(Clone, Copy, Debug)]
struct Placed {
    /// Where its origin is, from its sight's eye, and in pixels.
    origin: Vec3,
    center: Vec2,
    /// Its axes, X to Z, a unit long and square to each other, as [`place`] places them; and 1
    /// where they're right-handed and -1 where they're left-handed, as under a parent that
    /// mirrors: the sign a turn of its own about one of them takes to turn it the same way in the
    /// world.
    axes: [Vec3; 3],
    handed: f32,
    /// Which axes they are: the entity's own, or the world's, which Move's and Rotate's can be;
    /// and the orthogonal factor of the parent's 3x3 that carries the entity's own, which a turn
    /// about the world's axes is taken into the entity's space by. `handed` is its determinant's
    /// sign in either.
    space: Space,
    carry: Mat3,
    /// How long the handles are in the world, at the origin.
    length: f32,
    /// Which way the camera is from the origin.
    toward: Vec3,
    /// How much of each axis's handle shows, from 0, hidden, to 1, and of each plane's, by the
    /// axis square to it.
    axis: [f32; 3],
    plane: [f32; 3],
}

/// The handles placed about an entity whose own transform is `local`, carried by a parent at
/// `parent`, as `sight` sees them: at its origin in the world; along the world's axes in
/// [`Space::World`], and otherwise along its own axes as drawing
/// places them, its rotation carried by the [`orthogonal`] factor of the parent's 3x3, which takes
/// the parent's scale out but keeps its signs; and as long in the world as [`LENGTH`] points along
/// the camera's right there. Under a parent that scales unevenly, a turned entity's drawn axes
/// aren't square to each other, and the handles' are the nearest that are; under a flat one,
/// whose 3x3 has no inverse, its rotation and its ancestors' turn them, their scales aside. `None`
/// while the origin isn't past the near plane, or the origin, the axes or the length isn't finite.
fn place(sight: &Sight, space: Space, parent: &Pose, local: &Transform) -> Option<Placed> {
    let origin = (parent.matrix.transform_point3(local.translation) - sight.eye).as_vec3();
    let carry = carry(parent);
    let frame = match carried(parent) {
        Some(_) => carry * Mat3::from_quat(local.rotation.normalize()),
        None => Mat3::from_quat((parent.rotation * local.rotation).normalize()),
    };
    let axes = match space {
        Space::Local => [frame.x_axis, frame.y_axis, frame.z_axis].map(Vec3::normalize),
        Space::World => [Vec3::X, Vec3::Y, Vec3::Z],
    };
    let handed = frame.determinant().signum();
    let center = sight.project(origin)?;
    let right = sight.viewpoint.rotation * Vec3::X;
    let length = sight.pixels(LENGTH) / sight.project(origin + right)?.distance(center);
    let toward = sight.toward(origin);
    let finite = origin.is_finite()
        && center.is_finite()
        && frame.is_finite()
        && axes.iter().all(|axis| axis.is_finite())
        && toward.is_finite();
    if !(finite && length.is_finite() && length > 0.0) {
        return None;
    }
    // From pointing at the camera, for an axis, and from edge-on, for a plane.
    let axis = axes.map(|axis| fade(libm::acosf(axis.dot(toward).abs().min(1.0))));
    let plane = axes.map(|normal| fade(libm::asinf(normal.dot(toward).abs().min(1.0))));
    Some(Placed {
        origin,
        center,
        axes,
        handed,
        space,
        carry,
        length,
        toward,
        axis,
        plane,
    })
}

/// The parent's 3x3, in f64, which Move takes a move into the parent's space by and the handles'
/// axes are factored from; `None` for a flat parent, whose 3x3 has no inverse. Its determinant
/// decides it in f64, where a parent a ten-thousandth as big each way is far from none.
fn carried(parent: &Pose) -> Option<DMat3> {
    let carries = parent.matrix.matrix3;
    let determinant = carries.determinant();
    (determinant.is_finite() && determinant != 0.0).then_some(carries)
}

/// What carries an entity's own axes under `parent`, as [`place`] places them: the [`orthogonal`]
/// factor of the parent's 3x3, which takes its scale out but keeps its signs; or, for a flat one,
/// whose 3x3 has no inverse, its rotation.
fn carry(parent: &Pose) -> Mat3 {
    match carried(parent) {
        Some(carries) => orthogonal(carries),
        None => Mat3::from_quat(parent.rotation),
    }
}

/// The orthogonal polar factor of `matrix`, which isn't singular: the nearest matrix to it whose
/// columns are a unit long and square to each other, a rotation, and a mirror too where its
/// determinant is negative; of a rotation times a scale, the rotation times the scale's signs, a
/// mirror where one or three are negative and a half turn where two are. Higham's scaled Newton
/// iteration (Computing the polar decomposition, with applications, 1986), which settles in a
/// handful of steps however unevenly the matrix scales; in f64, since f32's inverse of a matrix
/// scaled tens of thousands of times more one way than the others, once turned, can lose its
/// determinant's sign, and the factor an axis.
fn orthogonal(matrix: DMat3) -> Mat3 {
    let squares = |m: DMat3| m.to_cols_array().iter().map(|x| x * x).sum::<f64>();
    let mut x = matrix;
    for _ in 0..STEPS {
        let inverse = x.inverse();
        // The square root of the ratio of their Frobenius norms.
        let gamma = (squares(inverse) / squares(x)).sqrt().sqrt();
        let next = (x * gamma + inverse.transpose() / gamma) / 2.0;
        // Once a step changes no entry by more than f32's epsilon, the next would change them by
        // about its square.
        if next.abs_diff_eq(x, f64::from(f32::EPSILON)) {
            return next.as_mat3();
        }
        x = next;
    }
    x.as_mat3()
}

/// How much of a handle shows `angle` from where it's hidden: none within [`HIDDEN`], and all
/// from [`SHOWN`].
fn fade(angle: f32) -> f32 {
    ((angle - HIDDEN) / (SHOWN - HIDDEN)).clamp(0.0, 1.0)
}

impl Placed {
    /// The rotation `start`, an entity's own, turned in the world by `angle` about `about`, a unit
    /// axis there, counterclockwise as it points at the viewer. With F the matrix that carries the
    /// entity's own axes, whose determinant's sign is `handed`: for an orthogonal F,
    /// F q(n, a) F^T is q(F n, a det F), so a turn of its own about F^T `about` is the turn in the
    /// world, the other way where the axes are left-handed. Under the world's axes, the same
    /// turn is made before the entity's: with P the parent's factor, `carry`, P^T q(n, a) P is
    /// q(P^T n, a det P), and the entity's rotation R becomes that times R.
    fn turn(&self, start: Quat, about: Vec3, angle: f32) -> Quat {
        let turned = match self.space {
            Space::Local => {
                let own = Vec3::from_array(self.axes.map(|axis| axis.dot(about))).normalize();
                start * Quat::from_axis_angle(own, self.handed * angle)
            }
            Space::World => {
                let own = (self.carry.transpose() * about).normalize();
                Quat::from_axis_angle(own, self.handed * angle) * start
            }
        };
        turned.normalize()
    }

    /// The handle `tool` shows at `at`, in pixels, of those within [`HIT`] points of it that show:
    /// for Move and Scale, the center first, then the nearest of the rest; for Rotate, the
    /// nearest ring's half toward the camera, then the outer ring, then the rings' halves away
    /// from it.
    fn hit(&self, sight: &Sight, tool: Tool, at: Vec2) -> Option<Handle> {
        let reach = sight.pixels(HIT);
        // The best so far, by its rank, then how far it is.
        let mut best: Option<((u8, f32), Handle)> = None;
        let mut near = |rank: u8, distance: Option<f32>, handle: Handle| {
            if let Some(distance) = distance.filter(|&distance| distance <= reach)
                && best.is_none_or(|(key, _)| (rank, distance) < key)
            {
                best = Some(((rank, distance), handle));
            }
        };
        let Placed {
            origin,
            center,
            axes,
            length,
            ..
        } = *self;
        let shown = |i: &usize| self.axis[*i] > 0.0;
        let axis = |i: usize| sight.segment(origin, origin + axes[i] * length);
        match tool {
            Tool::Move => {
                let off = at.distance(center) - sight.pixels(CENTER);
                near(0, Some(off.max(0.0)), Handle::Center);
                for i in (0..3).filter(shown) {
                    let distance = axis(i).map(|(line, _)| nearest(at, line).0);
                    near(1, distance, Handle::Axis(i));
                }
                for k in (0..3).filter(|&k| self.plane[k] > 0.0) {
                    let distance = self.square(sight, k).map(|corners| outside(&corners, at));
                    near(1, distance, Handle::Plane(k));
                }
            }
            Tool::Rotate => {
                let off = at.distance(center) - sight.pixels(OUTER * LENGTH);
                near(1, Some(off.abs()), Handle::Outer);
                for i in 0..3 {
                    for (front, line, _) in self.ring(sight, i) {
                        let rank = if front { 0 } else { 2 };
                        near(rank, Some(nearest(at, line).0), Handle::Ring(i));
                    }
                }
            }
            Tool::Scale => {
                near(
                    0,
                    Some(off_square(at, center, sight.pixels(CENTER))),
                    Handle::Center,
                );
                for i in (0..3).filter(shown) {
                    let line = axis(i).map(|(line, _)| nearest(at, line).0);
                    let tip = sight.project(origin + axes[i] * length);
                    let end = tip.map(|tip| off_square(at, tip, sight.pixels(SQUARE)));
                    near(
                        1,
                        line.into_iter().chain(end).reduce(f32::min),
                        Handle::Axis(i),
                    );
                }
            }
            Tool::Select | Tool::Pan | Tool::Orbit | Tool::Probe => {}
        }
        best.map(|(_, handle)| handle)
    }

    /// The corners of Move's square across the plane square to axis `k`, in pixels, if they're
    /// all past the near plane.
    fn square(&self, sight: &Sight, k: usize) -> Option<[Vec2; 4]> {
        let (u, v) = (self.axes[(k + 1) % 3], self.axes[(k + 2) % 3]);
        let (near, far) = (PLANE.0 * self.length, PLANE.1 * self.length);
        let corner = |a: f32, b: f32| sight.project(self.origin + u * a + v * b);
        Some([
            corner(near, near)?,
            corner(far, near)?,
            corner(far, far)?,
            corner(near, far)?,
        ])
    }

    /// The segments of ring `i`, about axis `i`, as far from the origin as the handles are long:
    /// those parts of them past the near plane, each in pixels and in the world, and whether it's
    /// on the half toward the camera. Counterclockwise as the axis points at the viewer, where the
    /// axes are right-handed.
    fn ring<'a>(
        &'a self,
        sight: &'a Sight,
        i: usize,
    ) -> impl Iterator<Item = (bool, [Vec2; 2], [Vec3; 2])> + 'a {
        let (u, v) = (self.axes[(i + 1) % 3], self.axes[(i + 2) % 3]);
        let point = move |k: usize| {
            let (sin, cos) = libm::sincosf(TAU * k as f32 / SEGMENTS as f32);
            self.origin + (u * cos + v * sin) * self.length
        };
        (0..SEGMENTS).filter_map(move |k| {
            let (from, to) = (point(k), point(k + 1));
            let (line, [start, end]) = sight.segment(from, to)?;
            let front = (from.lerp(to, 0.5) - self.origin).dot(self.toward) >= 0.0;
            Some((front, line, [from.lerp(to, start), from.lerp(to, end)]))
        })
    }

    /// The point of ring `i` that a press at `at`, in pixels, grabs: the nearest on screen of its
    /// half toward the camera, if that's within [`HIT`] points, as the press's hit prefers, and
    /// otherwise the nearest of all.
    fn grabbed(&self, sight: &Sight, i: usize, at: Vec2) -> Option<Vec3> {
        let reach = sight.pixels(HIT);
        let mut best: Option<((bool, f32), Vec3)> = None;
        for (front, line, [from, to]) in self.ring(sight, i) {
            let (distance, along) = nearest(at, line);
            let key = (!(front && distance <= reach), distance);
            if best.is_none_or(|(best, _)| key < best) {
                best = Some((key, from.lerp(to, along)));
            }
        }
        best.map(|(_, point)| point)
    }
}

/// How far `at` is from the segment `line`, and how far along it the nearest point is, from 0 to 1.
fn nearest(at: Vec2, [from, to]: [Vec2; 2]) -> (f32, f32) {
    let along = to - from;
    let squared = along.length_squared();
    let t = if squared > 0.0 {
        ((at - from).dot(along) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (at.distance(from + along * t), t)
}

/// How far `at` is outside the convex polygon `corners`, wound either way: 0 inside it.
fn outside(corners: &[Vec2; 4], at: Vec2) -> f32 {
    let edges = (0..4).map(|i| [corners[i], corners[(i + 1) % 4]]);
    let crosses = edges.clone().map(|[a, b]| (b - a).perp_dot(at - a));
    let (left, right) = crosses.fold((false, false), |(left, right), cross| {
        (left || cross > 0.0, right || cross < 0.0)
    });
    if !(left && right) {
        return 0.0;
    }
    edges
        .map(|line| nearest(at, line).0)
        .fold(f32::INFINITY, f32::min)
}

/// How far `at` is outside the square about `center` `half` across from its middle each way: 0
/// inside it.
fn off_square(at: Vec2, center: Vec2, half: f32) -> f32 {
    ((at - center).abs().max_element() - half).max(0.0)
}

/// The color of axis `i`, as the grid and the gizmo draw it: X red, Y green, Z blue.
fn color(i: usize) -> Color32 {
    let linear = [grid::RED, grid::GREEN, grid::BLUE][i];
    Rgba::from_rgb(linear.x, linear.y, linear.z).into()
}

/// Paints shapes given in pixels with a painter, which takes egui's points.
struct Pen<'a> {
    painter: &'a Painter,
    /// egui's pixels per point, which places the shapes.
    ppp: f32,
    /// The window's, which sizes them.
    scale: f32,
}

impl Pen<'_> {
    fn at(&self, pixel: Vec2) -> Pos2 {
        Pos2::new(pixel.x / self.ppp, pixel.y / self.ppp)
    }

    /// `points` points, in egui's.
    fn size(&self, points: f32) -> f32 {
        points * self.scale / self.ppp
    }

    fn line(&self, [from, to]: [Vec2; 2], color: Color32) {
        let stroke = Stroke::new(self.size(WIDTH), color);
        self.painter
            .line_segment([self.at(from), self.at(to)], stroke);
    }

    /// The head of an arrow along `line`, at its end.
    fn head(&self, [from, to]: [Vec2; 2], color: Color32) {
        let along = (to - from).normalize_or_zero();
        let (length, half) = (self.scale * HEAD.0, self.scale * HEAD.1);
        let base = to - along * length;
        let corners = [to, base + along.perp() * half, base - along.perp() * half];
        let corners = corners.map(|corner| self.at(corner)).to_vec();
        self.painter
            .add(Shape::convex_polygon(corners, color, Stroke::NONE));
    }

    fn polygon(&self, corners: [Vec2; 4], fill: Color32, stroke: Color32) {
        let corners = corners.map(|corner| self.at(corner)).to_vec();
        let stroke = Stroke::new(self.size(1.0), stroke);
        self.painter
            .add(Shape::convex_polygon(corners, fill, stroke));
    }

    /// A circle about `center`, `radius` points across from it.
    fn circle(&self, center: Vec2, radius: f32, fill: Color32, stroke: Color32) {
        let stroke = Stroke::new(self.size(WIDTH), stroke);
        self.painter
            .circle(self.at(center), self.size(radius), fill, stroke);
    }

    /// A square about `center`, `half` points across from its middle each way.
    fn square(&self, center: Vec2, half: f32, color: Color32) {
        let side = egui::Vec2::splat(2.0 * self.size(half));
        let rect = egui::Rect::from_center_size(self.at(center), side);
        self.painter.rect_filled(rect, 0.0, color);
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::math::{DAffine3, EulerRot};

    use super::*;
    use crate::tests::{SCALE, VIEW, reading};

    /// A camera `distance` from `target`, turned by `yaw`, `pitch` and `roll`, as the editor's
    /// turns, seeing 10 units from the bottom of the view to its top at the target either way.
    fn camera(
        target: Vec3,
        [yaw, pitch, roll]: [f32; 3],
        distance: f32,
        perspective: bool,
    ) -> Viewpoint {
        let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
        let projection = if perspective {
            Projection::Perspective {
                fov: 2.0 * libm::atanf(5.0 / distance),
                near: 0.01,
            }
        } else {
            Projection::Orthographic {
                height: 10.0,
                near: -1000.0,
                far: 1000.0,
            }
        };
        Viewpoint {
            position: (target - rotation * Vec3::NEG_Z * distance).as_dvec3(),
            rotation,
            projection,
        }
    }

    /// From above and to the side, at 40 units, at whose distance the rings stay well inside the
    /// outer one in perspective too.
    fn aside(target: Vec3, perspective: bool) -> Viewpoint {
        camera(target, [0.6, -0.5, 0.0], 40.0, perspective)
    }

    fn sight(viewpoint: Viewpoint) -> Sight {
        Sight::new(viewpoint, VIEW, SCALE).expect("the view shows")
    }

    /// An entity at `local`, carried by a parent at `parent`.
    fn picked(local: Transform, parent: Pose) -> Picked {
        Picked {
            entity: "0123456789abcdef".parse().expect("an ID"),
            serial: 1,
            local,
            parent,
            stepped: 0,
            waiting: false,
            moves: true,
        }
    }

    /// Off the origin, turned every way and scaled unevenly.
    fn turned() -> Transform {
        Transform {
            translation: DVec3::new(0.5, 0.2, -0.3),
            rotation: Quat::from_euler(EulerRot::XYZ, 0.3, -0.2, 0.5),
            scale: Vec3::new(2.0, 1.0, 0.5),
        }
    }

    /// Where a root at `at` carries its children, as the world's pose of it has it.
    fn carrying(at: Transform) -> Pose {
        Pose {
            matrix: at.matrix(),
            rotation: at.rotation,
        }
    }

    /// Parents that flip what they carry by scaling it negatively, as big each way: along X, a
    /// mirror; along X and Y, a half turn about Z, which their rotation leaves out; along X again,
    /// a ten-thousandth as big, whose 3x3's determinant in f32 is a trillionth; and turned, moved
    /// and twice as big, along Z, a mirror.
    fn flipping() -> [Pose; 4] {
        let along = |scale| {
            carrying(Transform {
                scale,
                ..Transform::IDENTITY
            })
        };
        [
            along(Vec3::new(-1.0, 1.0, 1.0)),
            along(Vec3::new(-1.0, -1.0, 1.0)),
            along(Vec3::new(-1e-4, 1e-4, 1e-4)),
            carrying(Transform {
                translation: DVec3::new(1.0, 2.0, 0.5),
                rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
                scale: Vec3::new(2.0, 2.0, -2.0),
            }),
        ]
    }

    /// The 3x3 that drawing carries an entity turned by `rotation` under `parent` by, but for its
    /// own scale: its columns run along its axes as drawn.
    fn drawn(parent: &Pose, rotation: Quat) -> Mat3 {
        parent.matrix.matrix3.as_mat3() * Mat3::from_quat(rotation)
    }

    fn aim(tool: Tool, viewpoint: Viewpoint, picked: Picked) -> Aim<'static> {
        Aim {
            tool,
            space: Space::Local,
            viewpoint: Some(viewpoint),
            picked: Some(picked),
            along: &[],
            camera: false,
            easing: false,
        }
    }

    fn moved(at: Vec2) -> Event<'static> {
        Event::CursorMoved {
            x: f64::from(at.x),
            y: f64::from(at.y),
        }
    }

    fn left(pressed: bool) -> Event<'static> {
        Event::MouseButton {
            button: MouseButton::Left,
            pressed,
        }
    }

    /// The handles' replies to the cursor moved to `from`, the left button pressed there, the
    /// cursor moved to each of `to` in turn and the button let go, with the command key held if
    /// `command`.
    fn drag(
        handles: &mut Handles,
        aim: &Aim,
        from: Vec2,
        to: &[Vec2],
        command: bool,
    ) -> Vec<Reply> {
        let read = |at: Vec2| Reading {
            command,
            ..reading(at)
        };
        let mut replies = vec![handles.event(&moved(from), &read(from), aim)];
        replies.push(handles.event(&left(true), &read(from), aim));
        let mut at = from;
        for &next in to {
            at = next;
            replies.push(handles.event(&moved(at), &read(at), aim));
        }
        replies.push(handles.event(&left(false), &read(at), aim));
        replies
    }

    /// The values `replies` set, in turn.
    fn sets(replies: &[Reply]) -> Vec<Field> {
        let sets = replies.iter().filter_map(|reply| match *reply {
            Reply::Set(_, field) => Some(field),
            _ => None,
        });
        sets.collect()
    }

    /// The one value a drag's `replies` set, which grabbed a handle and sealed as it ended.
    fn set(replies: &[Reply]) -> Field {
        assert_eq!(replies[1], Reply::Take, "{replies:?}");
        assert_eq!(replies.last(), Some(&Reply::Seal), "{replies:?}");
        let [field] = sets(replies)[..] else {
            panic!("one set: {replies:?}");
        };
        field
    }

    /// A point shows where the renderer's matrix puts it, the ray through that pixel goes through
    /// it, away from where the camera is from it; and a segment that runs behind the camera in
    /// perspective ends at its near plane.
    #[test]
    fn rays_and_pixels_agree_with_the_viewpoints_matrix() {
        for perspective in [false, true] {
            let viewpoint = camera(
                Vec3::new(1.0, -2.0, 0.5),
                [0.4, -0.3, 0.2],
                12.0,
                perspective,
            );
            let sight = sight(viewpoint);
            let clip = viewpoint.view_projection(800.0 / 600.0);
            for point in [
                Vec3::new(1.0, -2.0, 0.5),
                Vec3::new(3.0, 1.0, -2.0),
                Vec3::new(-4.0, 2.5, 3.0),
            ] {
                // As the renderer places it, Vulkan's y down; and from the sight's eye.
                let ndc = clip.project_point3(point);
                let point = point - sight.eye.as_vec3();
                let expected = Vec2::new(
                    100.0 + (ndc.x + 1.0) / 2.0 * 800.0,
                    50.0 + (ndc.y + 1.0) / 2.0 * 600.0,
                );
                let pixel = sight.project(point).expect("in front of it");
                assert!(pixel.abs_diff_eq(expected, 1e-3), "{pixel}, not {expected}");
                let ray = sight.ray(pixel);
                let along = (point - ray.from).dot(ray.toward);
                let off = ray.from + ray.toward * along - point;
                assert!(
                    off.length() < 1e-4,
                    "{perspective}: {point} is {off} off the ray"
                );
                let back = -sight.toward(point);
                assert!(
                    ray.toward.abs_diff_eq(back, 1e-4),
                    "{}, not {back}",
                    ray.toward
                );
            }
        }
        let viewpoint = camera(Vec3::ZERO, [0.4, -0.3, 0.0], 12.0, true);
        let sight = sight(viewpoint);
        let forward = viewpoint.rotation * Vec3::NEG_Z;
        // From the sight's eye, which its points are from: the world's origin, and 3 behind it.
        let (origin, behind) = (-sight.eye.as_vec3(), -forward * 3.0);
        let (_, [start, end]) = sight.segment(origin, behind).expect("partly in front");
        let cut = origin.lerp(behind, end);
        let depth = cut.dot(forward);
        assert!(
            start == 0.0 && (depth - 0.01).abs() < 1e-4,
            "{start}, {depth}"
        );
        assert!(sight.project(behind).is_none());
        assert!(sight.segment(behind, behind - forward).is_none());
    }

    /// A press on each handle hits it, of each tool's, in either projection; one far from them hits
    /// none, nor does one with a tool without handles. An axis pointing at the camera, and a plane
    /// edge-on to it, are hidden, and hit by nothing, fading in between; and Move shows nothing of
    /// an entity whose parent is flat, which no move would take it across.
    #[test]
    fn presses_hit_each_handle_and_none_hidden() {
        for perspective in [false, true] {
            let sight = sight(aside(Vec3::ZERO, perspective));
            let placed = place(&sight, Space::Local, &Pose::IDENTITY, &turned()).expect("in view");
            let shown = placed.axis.iter().chain(&placed.plane);
            assert!(shown.into_iter().all(|&shown| shown > 0.5), "{placed:?}");
            let Placed {
                origin,
                center,
                axes,
                length,
                toward,
                ..
            } = placed;
            let at = |point: Vec3| sight.project(point).expect("in view");
            let hit = |tool, at| placed.hit(&sight, tool, at);
            for i in 0..3 {
                let (u, v) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
                let along = origin + axes[i] * length * 0.7;
                assert_eq!(hit(Tool::Move, at(along)), Some(Handle::Axis(i)), "{i}");
                let middle = origin + (u + v) * (PLANE.0 + PLANE.1) / 2.0 * length;
                assert_eq!(hit(Tool::Move, at(middle)), Some(Handle::Plane(i)), "{i}");
                let end = origin + axes[i] * length;
                assert_eq!(hit(Tool::Scale, at(end)), Some(Handle::Axis(i)), "{i}");
                // The ring's point nearest the camera.
                let near = origin + facing(axes[i], toward) * length;
                assert_eq!(hit(Tool::Rotate, at(near)), Some(Handle::Ring(i)), "{i}");
            }
            assert_eq!(hit(Tool::Move, center), Some(Handle::Center));
            assert_eq!(hit(Tool::Scale, center), Some(Handle::Center));
            let outer = center + Vec2::new(0.0, -sight.pixels(OUTER * LENGTH));
            assert_eq!(hit(Tool::Rotate, outer), Some(Handle::Outer));
            let far = center + Vec2::splat(sight.pixels(2.0 * LENGTH));
            for tool in [Tool::Move, Tool::Rotate, Tool::Scale] {
                assert_eq!(hit(tool, far), None, "{tool:?}");
            }
            for tool in [Tool::Select, Tool::Pan, Tool::Orbit] {
                assert_eq!(hit(tool, center), None, "{tool:?}");
            }
        }
        // From the front, Z points at the camera, and the planes through it are edge-on: a press
        // on their squares, slivers along X and Y, hits those axes; 14° off, Z is half shown.
        let front = sight(camera(Vec3::ZERO, [0.0; 3], 40.0, false));
        let placed =
            place(&front, Space::Local, &Pose::IDENTITY, &Transform::IDENTITY).expect("in view");
        assert_eq!(
            (placed.axis, placed.plane),
            ([1.0, 1.0, 0.0], [0.0, 0.0, 1.0])
        );
        let sliver = |u: Vec3| {
            let at = (u + Vec3::Z) * 0.3 * placed.length;
            front.project(at).expect("in view")
        };
        assert_eq!(
            placed.hit(&front, Tool::Move, sliver(Vec3::Y)),
            Some(Handle::Axis(1))
        );
        assert_eq!(
            placed.hit(&front, Tool::Move, sliver(Vec3::X)),
            Some(Handle::Axis(0))
        );
        let fourteen = sight(camera(
            Vec3::ZERO,
            [14.0 * PI / 180.0, 0.0, 0.0],
            40.0,
            false,
        ));
        let placed = place(
            &fourteen,
            Space::Local,
            &Pose::IDENTITY,
            &Transform::IDENTITY,
        )
        .expect("in view");
        assert!((placed.axis[2] - 0.5).abs() < 1e-3, "{:?}", placed.axis);
        let flat = Pose {
            matrix: DAffine3::from_scale(DVec3::new(1.0, 0.0, 1.0)),
            rotation: Quat::IDENTITY,
        };
        let on_flat = picked(Transform::IDENTITY, flat);
        let handles = Handles::default();
        assert!(
            handles
                .placed(&front, Tool::Move, Space::Local, &on_flat)
                .is_none()
        );
        assert!(
            handles
                .placed(&front, Tool::Rotate, Space::Local, &on_flat)
                .is_some()
        );
        // Nor anything for an entity out at infinity, or turned by what's no rotation.
        let infinite = Transform {
            translation: DVec3::new(f64::from(f32::INFINITY), 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let lost = Transform {
            rotation: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
            ..Transform::IDENTITY
        };
        for local in [infinite, lost] {
            assert!(
                place(&front, Space::Local, &Pose::IDENTITY, &local).is_none(),
                "{local:?}"
            );
        }
    }

    /// Each move puts the point of the handle that the press grabbed under the cursor: along an
    /// axis, wherever on screen the axis goes, and across a plane, or the view's, anywhere; in
    /// either projection.
    #[test]
    fn a_move_keeps_the_point_grabbed_under_the_cursor() {
        for perspective in [false, true] {
            let viewpoint = aside(Vec3::ZERO, perspective);
            let sight = sight(viewpoint);
            let local = turned();
            let picked = picked(local, Pose::IDENTITY);
            let aim = aim(Tool::Move, viewpoint, picked);
            let placed = place(&sight, Space::Local, &picked.parent, &local).expect("in view");
            let Placed {
                origin,
                center,
                axes,
                length,
                toward,
                ..
            } = placed;
            let at = |point: Vec3| sight.project(point).expect("in view");
            for axis in axes {
                let grabbed = origin + axis * length * 0.7;
                let to = at(grabbed + axis * 1.5);
                let replies = drag(&mut Handles::default(), &aim, at(grabbed), &[to], false);
                let Field::Translation(moved) = set(&replies) else {
                    panic!("{replies:?}");
                };
                let expected = local.translation + (axis * 1.5).as_dvec3();
                assert!(
                    moved.abs_diff_eq(expected, 1e-3),
                    "{axis}: {moved}, not {expected}"
                );
            }
            let squares = (0..3).map(|k| {
                let (u, v) = (axes[(k + 1) % 3], axes[(k + 2) % 3]);
                let middle = origin + (u + v) * (PLANE.0 + PLANE.1) / 2.0 * length;
                (at(middle), axes[k])
            });
            for (from, normal) in squares.chain([(center, toward)]) {
                let to = from + Vec2::new(37.0, -23.0);
                let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
                let Field::Translation(moved) = set(&replies) else {
                    panic!("{replies:?}");
                };
                let grabbed = sight.ray(from).meet(origin, normal).expect("on the plane");
                let now = at(grabbed + (moved - local.translation).as_vec3());
                assert!(now.abs_diff_eq(to, 0.05), "{normal}: {now}, not {to}");
            }
        }
    }

    /// Dragged up toward the ground's horizon in perspective, a move goes further the nearer the
    /// cursor gets to it, finite all the way; past it, where the ray would meet the ground behind
    /// the camera, the drag keeps its last value rather than jump.
    #[test]
    fn past_a_planes_horizon_a_drag_keeps_its_last_value() {
        // 15° down, which puts the ground's horizon 15° above the view's middle, in view.
        let distance = 5.0 / libm::tanf(PI / 6.0);
        let viewpoint = camera(Vec3::ZERO, [0.0, -15.0 * PI / 180.0, 0.0], distance, true);
        let sight = sight(viewpoint);
        let picked = picked(Transform::IDENTITY, Pose::IDENTITY);
        let aim = aim(Tool::Move, viewpoint, picked);
        let placed =
            place(&sight, Space::Local, &Pose::IDENTITY, &Transform::IDENTITY).expect("in view");
        let middle = (PLANE.0 + PLANE.1) / 2.0 * placed.length;
        let from = sight
            .project(Vec3::new(middle, 0.0, middle) + placed.origin)
            .expect("in view");
        let horizon = 50.0 + (1.0 - libm::tanf(15.0 * PI / 180.0) / libm::tanf(PI / 6.0)) * 300.0;
        let steps: Vec<Vec2> = (1..60)
            .map(|i| from - Vec2::new(0.0, 10.0 * i as f32))
            .collect();
        let replies = drag(&mut Handles::default(), &aim, from, &steps, false);
        assert_eq!(replies[1], Reply::Take);
        let mut away = 0.0;
        for (reply, at) in replies[2..replies.len() - 1].iter().zip(&steps) {
            match *reply {
                Reply::Set(_, Field::Translation(moved)) => {
                    assert!(at.y > horizon && moved.is_finite(), "{at}: {moved}");
                    assert!(-moved.z > away, "{at}: {moved} goes further");
                    away = -moved.z;
                }
                Reply::Pass => assert!(at.y < horizon, "{at} sets nothing"),
                other => panic!("{other:?}"),
            }
        }
        assert!(away > 10.0, "{away}");
    }

    /// A ring's drag turns about its axis by the angle the cursor goes round it, which brings the
    /// point grabbed along under it; seen edge-on, by the cursor's travel across the view, which
    /// brings the point grabbed on the near half along too: either side of where the one gives way
    /// to the other, in either projection.
    #[test]
    fn a_ring_turns_by_the_angle_or_edge_on_by_the_travel() {
        // Turned off the axes about the world's up, so that ring Y's point nearest the camera,
        // which the presses grab, is on no other ring.
        for perspective in [false, true] {
            let viewpoint = camera(Vec3::ZERO, [0.5, -0.9, 0.0], 40.0, perspective);
            let sight = sight(viewpoint);
            let picked = picked(Transform::IDENTITY, Pose::IDENTITY);
            let aim = aim(Tool::Rotate, viewpoint, picked);
            let placed = place(&sight, Space::Local, &Pose::IDENTITY, &Transform::IDENTITY)
                .expect("in view");
            let grabbed = facing(Vec3::Y, placed.toward) * placed.length;
            let turned = Quat::from_rotation_y(0.6) * grabbed;
            let [from, to] = [grabbed, turned]
                .map(|point| sight.project(placed.origin + point).expect("in view"));
            assert_eq!(
                placed.hit(&sight, Tool::Rotate, from),
                Some(Handle::Ring(1))
            );
            let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
            let Field::Rotation(rotation) = set(&replies) else {
                panic!("{replies:?}");
            };
            assert!((rotation * grabbed).abs_diff_eq(turned, 1e-3), "{rotation}");
        }
        // Nearly level, so that ring Y is nearly edge-on: just inside cos 85° and just past.
        for perspective in [false, true] {
            for degrees in [4.9, 5.1] {
                let pitch = -degrees * PI / 180.0;
                let viewpoint = camera(Vec3::ZERO, [0.5, pitch, 0.0], 40.0, perspective);
                let sight = sight(viewpoint);
                let picked = picked(Transform::IDENTITY, Pose::IDENTITY);
                let aim = aim(Tool::Rotate, viewpoint, picked);
                let placed = place(&sight, Space::Local, &Pose::IDENTITY, &Transform::IDENTITY)
                    .expect("in view");
                let edge_on = Vec3::Y.dot(placed.toward).abs() < EDGE_ON;
                assert_eq!(edge_on, degrees < 5.0, "{degrees}°");
                let near = facing(Vec3::Y, placed.toward) * placed.length;
                let from = sight.project(placed.origin + near).expect("in view");
                assert_eq!(
                    placed.hit(&sight, Tool::Rotate, from),
                    Some(Handle::Ring(1))
                );
                // Along the ring's tangent there, which goes right across the view.
                let to = from + Vec2::new(6.0, 0.0);
                let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
                let Field::Rotation(rotation) = set(&replies) else {
                    panic!("{replies:?}");
                };
                let now = sight
                    .project(placed.origin + rotation * near)
                    .expect("in view");
                assert!(now.distance(to) < 0.1, "{degrees}°: {now}, not {to}");
            }
        }
    }

    /// The outer ring turns about the view's axis by the angle the cursor goes round the origin on
    /// screen, so that a point of the entity's at the press's angle follows the cursor round.
    #[test]
    fn the_outer_ring_turns_about_the_views_axis() {
        for perspective in [false, true] {
            let local = turned();
            // At the view's middle, where a turn about the view's axis turns the image about it.
            let viewpoint = aside(local.translation.as_vec3(), perspective);
            let sight = sight(viewpoint);
            let picked = picked(local, Pose::IDENTITY);
            let aim = aim(Tool::Rotate, viewpoint, picked);
            let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
            let radius = sight.pixels(OUTER * LENGTH);
            let from = placed.center - Vec2::new(0.0, radius);
            // Clockwise on screen, whose y goes down.
            let way = Vec2::new(libm::sinf(0.5), -libm::cosf(0.5));
            let to = placed.center + way * radius;
            let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
            let Field::Rotation(rotation) = set(&replies) else {
                panic!("{replies:?}");
            };
            let by = rotation * local.rotation.inverse();
            let above = viewpoint.rotation * Vec3::Y * 0.5;
            let now = sight.project(placed.origin + by * above).expect("in view") - placed.center;
            assert!(now.normalize().abs_diff_eq(way, 1e-3), "{now}, not {way}");
        }
    }

    /// Scale's axis scales its component by how far the drag goes along it, a handles' length
    /// doubling it, and back past the origin no further than a thousandth; one that's 0 stays 0;
    /// and the middle square scales all three by how far the cursor goes right, a handles' length
    /// doubling them.
    #[test]
    fn scale_goes_by_the_travel() {
        let viewpoint = aside(Vec3::ZERO, false);
        let sight = sight(viewpoint);
        let local = Transform {
            scale: Vec3::new(2.0, 0.0, 0.5),
            ..turned()
        };
        let picked = picked(local, Pose::IDENTITY);
        let aim = aim(Tool::Scale, viewpoint, picked);
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
        let Placed {
            origin,
            center,
            axes,
            length,
            ..
        } = placed;
        let at = |point: Vec3| sight.project(point).expect("in view");
        let end = origin + axes[0] * length;
        for (to, x) in [
            (end + axes[0] * length, 4.0),
            (origin - axes[0] * length, 0.002),
        ] {
            let replies = drag(&mut Handles::default(), &aim, at(end), &[at(to)], false);
            let Field::Scale(scaled) = set(&replies) else {
                panic!("{replies:?}");
            };
            let expected = Vec3::new(x, 0.0, 0.5);
            assert!(
                scaled.abs_diff_eq(expected, 1e-4),
                "{scaled}, not {expected}"
            );
        }
        let y = origin + axes[1] * length;
        let replies = drag(
            &mut Handles::default(),
            &aim,
            at(y),
            &[at(y + axes[1] * length)],
            false,
        );
        assert_eq!(replies[1], Reply::Take);
        assert_eq!(sets(&replies), []);
        let to = center + Vec2::new(sight.pixels(LENGTH), 40.0);
        let replies = drag(&mut Handles::default(), &aim, center, &[to], false);
        let Field::Scale(scaled) = set(&replies) else {
            panic!("{replies:?}");
        };
        let expected = Vec3::new(4.0, 0.0, 1.0);
        assert!(
            scaled.abs_diff_eq(expected, 1e-4),
            "{scaled}, not {expected}"
        );
    }

    /// With the command key held, a drag snaps from where it started: a move to half units along
    /// the handles' axes, a turn to 45° and a scale to halves of where it started, at least one
    /// half; and pressing the key, or letting it go, as a drag goes on sets its value again,
    /// snapped or not.
    #[test]
    fn the_command_key_snaps_from_where_a_drag_started() {
        let viewpoint = aside(Vec3::ZERO, false);
        let sight = sight(viewpoint);
        let local = turned();
        let picked = picked(local, Pose::IDENTITY);
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
        let Placed {
            origin,
            axes,
            length,
            toward,
            ..
        } = placed;
        let at = |point: Vec3| sight.project(point).expect("in view");
        let grabbed = origin + axes[0] * length * 0.7;
        let (from, to) = (at(grabbed), at(grabbed + axes[0] * 1.3));
        let moving = aim(Tool::Move, viewpoint, picked);
        let replies = drag(&mut Handles::default(), &moving, from, &[to], true);
        let Field::Translation(went) = set(&replies) else {
            panic!("{replies:?}");
        };
        assert!(
            went.abs_diff_eq(local.translation + (axes[0] * 1.5).as_dvec3(), 1e-4),
            "{went}"
        );
        // Ring Z, turned 50° round.
        let near = origin + facing(axes[2], toward) * length;
        let turned = origin + Quat::from_axis_angle(axes[2], 50.0 * PI / 180.0) * (near - origin);
        let turning = aim(Tool::Rotate, viewpoint, picked);
        let replies = drag(
            &mut Handles::default(),
            &turning,
            at(near),
            &[at(turned)],
            true,
        );
        let Field::Rotation(rotation) = set(&replies) else {
            panic!("{replies:?}");
        };
        let expected = local.rotation * Quat::from_rotation_z(FRAC_PI_4);
        assert!(
            rotation.abs_diff_eq(expected, 1e-4),
            "{rotation}, not {expected}"
        );
        // X's scale, by 1.3 and by 0.1 of itself.
        let end = origin + axes[0] * length;
        let scaling = aim(Tool::Scale, viewpoint, picked);
        for (by, snapped) in [(0.3, 1.5), (-0.9, 0.5)] {
            let to = at(end + axes[0] * length * by);
            let replies = drag(&mut Handles::default(), &scaling, at(end), &[to], true);
            let Field::Scale(scaled) = set(&replies) else {
                panic!("{replies:?}");
            };
            assert!((scaled.x - 2.0 * snapped).abs() < 1e-4, "{by}: {scaled}");
        }
        // Ctrl pressed as the move goes on, then let go.
        let mut handles = Handles::default();
        handles.event(&moved(from), &reading(from), &moving);
        handles.event(&left(true), &reading(from), &moving);
        let unsnapped = handles.event(&moved(to), &reading(to), &moving);
        let ctrl = |pressed| Event::Key {
            key: Key::ControlLeft,
            pressed,
            repeat: false,
        };
        let held = Reading {
            command: true,
            ..reading(to)
        };
        let snapped = handles.event(&ctrl(true), &held, &moving);
        let again = handles.event(&ctrl(false), &reading(to), &moving);
        let (Reply::Set(_, Field::Translation(first)), Reply::Set(_, Field::Translation(snap))) =
            (unsnapped, snapped)
        else {
            panic!("{unsnapped:?}, {snapped:?}");
        };
        assert!(first.abs_diff_eq(local.translation + (axes[0] * 1.3).as_dvec3(), 1e-4));
        assert!(snap.abs_diff_eq(local.translation + (axes[0] * 1.5).as_dvec3(), 1e-4));
        assert_eq!(again, unsnapped);
    }

    /// A child under a turned parent scaled unevenly moves in the world along its own axis, as the
    /// handles show it, as far as the cursor goes along it: the move is taken into the parent's
    /// space. So does one whose grandparent, turned too, scales a hundred times more one way and a
    /// hundred times less another, which skews the 3x3 that carries it.
    #[test]
    fn a_child_under_a_turned_scaled_parent_moves_along_its_axis_in_the_world() {
        let parent_at = Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
            scale: Vec3::new(2.0, 0.5, 1.5),
        };
        let grandparent_at = Transform {
            rotation: Quat::from_rotation_y(0.4) * Quat::from_rotation_z(-0.5),
            scale: Vec3::new(100.0, 1.0, 0.01),
            ..Transform::IDENTITY
        };
        let skewed = Pose {
            matrix: grandparent_at.matrix() * parent_at.matrix(),
            rotation: grandparent_at.rotation * parent_at.rotation,
        };
        let local = Transform {
            translation: DVec3::new(0.3, -0.4, 0.2),
            rotation: Quat::from_rotation_y(0.7),
            scale: Vec3::ONE,
        };
        for parent in [carrying(parent_at), skewed] {
            moves_along_its_axes(parent, local, Space::Local);
        }
    }

    /// In the world's space, Move's arrows run along the world's X, Y and Z, and a drag of one
    /// moves an entity turned 45° about Z, under a parent turned another way, scaled unevenly,
    /// and under that one's, which skews the 3x3 that carries it, along that axis in the world
    /// alone: the world X arrow changes its world position in X only.
    #[test]
    fn in_the_worlds_space_a_move_goes_along_the_worlds_axes_under_a_turned_parent() {
        let parent_at = Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_y(0.9) * Quat::from_rotation_x(-0.4),
            scale: Vec3::new(2.0, 0.5, 1.5),
        };
        let grandparent_at = Transform {
            rotation: Quat::from_rotation_y(0.4) * Quat::from_rotation_z(-0.5),
            scale: Vec3::new(100.0, 1.0, 0.01),
            ..Transform::IDENTITY
        };
        let skewed = Pose {
            matrix: grandparent_at.matrix() * parent_at.matrix(),
            rotation: grandparent_at.rotation * parent_at.rotation,
        };
        let local = Transform {
            translation: DVec3::new(0.3, -0.4, 0.2),
            rotation: Quat::from_rotation_z(FRAC_PI_4),
            scale: Vec3::ONE,
        };
        for parent in [carrying(parent_at), skewed] {
            moves_along_its_axes(parent, local, Space::World);
        }
    }

    /// Checks that `local`, under `parent`, moves in the world along each axis the handles show in
    /// `space`, as far as the cursor goes along it, in either projection.
    fn moves_along_its_axes(parent: Pose, local: Transform, space: Space) {
        let world = parent.matrix.transform_point3(local.translation);
        for perspective in [false, true] {
            let viewpoint = aside(world.as_vec3(), perspective);
            let sight = sight(viewpoint);
            let picked = picked(local, parent);
            let aim = Aim {
                space,
                ..aim(Tool::Move, viewpoint, picked)
            };
            let placed = place(&sight, space, &parent, &local).expect("in view");
            if space == Space::World {
                assert_eq!(placed.axes, [Vec3::X, Vec3::Y, Vec3::Z]);
            }
            let Placed {
                origin,
                axes,
                length,
                ..
            } = placed;
            assert!(origin.abs_diff_eq((world - sight.eye).as_vec3(), 1e-5));
            let at = |point: Vec3| sight.project(point).expect("in view");
            for axis in axes {
                let grabbed = origin + axis * length * 0.7;
                let to = at(grabbed + axis * 1.5);
                let replies = drag(&mut Handles::default(), &aim, at(grabbed), &[to], false);
                let Field::Translation(moved) = set(&replies) else {
                    panic!("{replies:?}");
                };
                let now = parent.matrix.transform_point3(moved);
                let expected = sight.eye + (origin + axis * 1.5).as_dvec3();
                assert!(
                    now.abs_diff_eq(expected, 1e-3),
                    "{axis}: {now}, not {expected}"
                );
            }
        }
    }

    /// Under a parent that scales negatively, the handles' axes run along the entity's as drawing
    /// places them, mirrored or turned half round, whether it's turned or not. Under a turned
    /// parent that scales unevenly, which skews a turned entity's, they're its own turned by the
    /// parent's rotation, and mirrored where the parent's scale mirrors.
    #[test]
    fn the_axes_run_along_the_drawn_ones() {
        let sight = sight(aside(Vec3::ZERO, false));
        let along = |parent: &Pose, rotation: Quat, expected: Mat3| {
            let local = Transform {
                rotation,
                ..Transform::IDENTITY
            };
            let placed = place(&sight, Space::Local, parent, &local).expect("in view");
            for (i, axis) in placed.axes.into_iter().enumerate() {
                let expected = expected.col(i).normalize();
                assert!(
                    axis.abs_diff_eq(expected, 1e-5),
                    "{parent:?}, {rotation}: {axis}, not {expected}"
                );
            }
        };
        for parent in flipping() {
            for rotation in [
                Quat::IDENTITY,
                Quat::from_rotation_z(PI / 6.0),
                turned().rotation,
            ] {
                along(&parent, rotation, drawn(&parent, rotation));
            }
        }
        let turn = Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3);
        let rotation = turned().rotation;
        for scale in [Vec3::new(2.0, 0.5, 1.5), Vec3::new(-2.0, 0.5, 1.5)] {
            let parent = carrying(Transform {
                rotation: turn,
                scale,
                ..Transform::IDENTITY
            });
            let mirrored = Mat3::from_diagonal(scale.signum());
            let expected = Mat3::from_quat(turn) * mirrored * Mat3::from_quat(rotation);
            along(&parent, rotation, expected);
        }
    }

    /// Under a parent that scales negatively, a ring's drag turns the entity as drawing places it
    /// the way the cursor goes, about the axis the ring shows by the angle the cursor goes round
    /// it, and the outer ring's about the view's axis by the angle it goes round the origin on
    /// screen: whether it's turned or not, in either projection.
    #[test]
    fn under_a_negative_scale_the_rings_turn_it_the_way_the_cursor_goes() {
        for perspective in [false, true] {
            for parent in flipping() {
                for rotation in [
                    Quat::IDENTITY,
                    Quat::from_rotation_z(PI / 6.0),
                    turned().rotation,
                ] {
                    let local = Transform {
                        rotation,
                        ..Transform::IDENTITY
                    };
                    let origin = parent.matrix.transform_point3(local.translation);
                    // As the ring test above sees it, from where no ring is edge-on here; at the
                    // view's middle, where a turn about the view's axis turns the image about it.
                    let viewpoint = camera(origin.as_vec3(), [0.5, -0.9, 0.0], 40.0, perspective);
                    let sight = sight(viewpoint);
                    let origin = (origin - sight.eye).as_vec3();
                    let aim = aim(Tool::Rotate, viewpoint, picked(local, parent));
                    let placed = place(&sight, Space::Local, &parent, &local).expect("in view");
                    let Placed {
                        center,
                        axes,
                        length,
                        toward,
                        ..
                    } = placed;
                    let at = |point: Vec3| sight.project(point).expect("in view");
                    let before = drawn(&parent, rotation);
                    let turns = |from: Vec2, to: Vec2, expected: Mat3| {
                        let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
                        let Field::Rotation(dragged) = set(&replies) else {
                            panic!("{replies:?}");
                        };
                        let now = drawn(&parent, dragged);
                        // As big as the parent is, a ten-thousandth of a unit for one of them.
                        let tolerance = 1e-3 * expected.x_axis.length();
                        assert!(
                            now.abs_diff_eq(expected, tolerance),
                            "{parent:?}, {local:?}: {now}, not {expected}"
                        );
                    };
                    for (i, axis) in axes.into_iter().enumerate() {
                        // From the ring's point nearest the camera, 0.6 round.
                        let turn = Quat::from_axis_angle(axis, 0.6);
                        let grabbed = facing(axis, toward) * length;
                        let from = at(origin + grabbed);
                        assert_eq!(
                            placed.hit(&sight, Tool::Rotate, from),
                            Some(Handle::Ring(i)),
                            "{parent:?}, {local:?}"
                        );
                        let to = at(origin + turn * grabbed);
                        turns(from, to, Mat3::from_quat(turn) * before);
                    }
                    // Clockwise on screen, whose y goes down, from the top: about the view's axis
                    // pointing away from the camera.
                    let radius = sight.pixels(OUTER * LENGTH);
                    let way = Vec2::new(libm::sinf(0.5), -libm::cosf(0.5));
                    let from = center - Vec2::new(0.0, radius);
                    let turn = Quat::from_axis_angle(-toward, 0.5);
                    turns(from, center + way * radius, Mat3::from_quat(turn) * before);
                }
            }
        }
    }

    /// Under a parent that scales negatively, Move's arrows and Scale's axes run along a turned
    /// entity's axes as drawing places them: a move along one keeps the point grabbed under the
    /// cursor, and a scale along one scales the component that drawing scales the entity along it
    /// by; in either projection.
    #[test]
    fn under_a_negative_scale_move_and_scale_go_along_the_drawn_axes() {
        let local = turned();
        for perspective in [false, true] {
            for parent in flipping() {
                let origin = parent.matrix.transform_point3(local.translation);
                let viewpoint = aside(origin.as_vec3(), perspective);
                let sight = sight(viewpoint);
                let origin = (origin - sight.eye).as_vec3();
                let picked = picked(local, parent);
                let length = place(&sight, Space::Local, &parent, &local)
                    .expect("in view")
                    .length;
                let at = |point: Vec3| sight.project(point).expect("in view");
                let frame = drawn(&parent, local.rotation);
                for i in 0..3 {
                    let axis = frame.col(i).normalize();
                    let grabbed = origin + axis * length * 0.7;
                    let to = at(grabbed + axis * 1.5);
                    let moving = aim(Tool::Move, viewpoint, picked);
                    let replies = drag(&mut Handles::default(), &moving, at(grabbed), &[to], false);
                    let Field::Translation(moved) = set(&replies) else {
                        panic!("{replies:?}");
                    };
                    let now = parent.matrix.transform_point3(moved);
                    let expected = sight.eye + (origin + axis * 1.5).as_dvec3();
                    assert!(
                        now.abs_diff_eq(expected, 1e-3),
                        "{parent:?}, {i}: {now}, not {expected}"
                    );
                    // From the axis's end, a handles' length further along it, which doubles it.
                    let end = origin + axis * length;
                    let to = at(end + axis * length);
                    let scaling = aim(Tool::Scale, viewpoint, picked);
                    let replies = drag(&mut Handles::default(), &scaling, at(end), &[to], false);
                    let Field::Scale(scaled) = set(&replies) else {
                        panic!("{replies:?}");
                    };
                    let mut expected = local.scale;
                    expected[i] *= 2.0;
                    assert!(
                        scaled.abs_diff_eq(expected, 1e-4),
                        "{parent:?}, {i}: {scaled}, not {expected}"
                    );
                }
            }
        }
    }

    /// In the world's space, a ring's drag turns an entity about the world's axis the ring shows by
    /// the angle the cursor goes round it, and the outer ring's about the view's axis: so that its
    /// world rotation is the turn times the one it had, under a parent that is turned, mirrored,
    /// or both, whether the entity is turned or not, in either projection.
    #[test]
    fn in_the_worlds_space_the_rings_turn_it_about_the_worlds_axes() {
        let turned_parent = carrying(Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
            scale: Vec3::splat(2.0),
        });
        for perspective in [false, true] {
            for parent in flipping().into_iter().chain([turned_parent]) {
                for rotation in [Quat::IDENTITY, turned().rotation] {
                    let local = Transform {
                        rotation,
                        ..Transform::IDENTITY
                    };
                    let origin = parent.matrix.transform_point3(local.translation);
                    let viewpoint = camera(origin.as_vec3(), [0.5, -0.9, 0.0], 40.0, perspective);
                    let sight = sight(viewpoint);
                    let origin = (origin - sight.eye).as_vec3();
                    let aim = Aim {
                        space: Space::World,
                        ..aim(Tool::Rotate, viewpoint, picked(local, parent))
                    };
                    let placed = place(&sight, Space::World, &parent, &local).expect("in view");
                    assert_eq!(placed.axes, [Vec3::X, Vec3::Y, Vec3::Z]);
                    let (center, length, toward) = (placed.center, placed.length, placed.toward);
                    let at = |point: Vec3| sight.project(point).expect("in view");
                    let before = drawn(&parent, rotation);
                    let turns = |from: Vec2, to: Vec2, expected: Mat3| {
                        let replies = drag(&mut Handles::default(), &aim, from, &[to], false);
                        let Field::Rotation(dragged) = set(&replies) else {
                            panic!("{replies:?}");
                        };
                        let now = drawn(&parent, dragged);
                        let tolerance = 1e-3 * expected.x_axis.length();
                        assert!(
                            now.abs_diff_eq(expected, tolerance),
                            "{parent:?}, {local:?}: {now}, not {expected}"
                        );
                    };
                    for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
                        let turn = Quat::from_axis_angle(axis, 0.6);
                        let grabbed = facing(axis, toward) * length;
                        let from = at(origin + grabbed);
                        assert_eq!(
                            placed.hit(&sight, Tool::Rotate, from),
                            Some(Handle::Ring(i)),
                            "{parent:?}, {local:?}"
                        );
                        let to = at(origin + turn * grabbed);
                        turns(from, to, Mat3::from_quat(turn) * before);
                    }
                    let radius = sight.pixels(OUTER * LENGTH);
                    let way = Vec2::new(libm::sinf(0.5), -libm::cosf(0.5));
                    let from = center - Vec2::new(0.0, radius);
                    let turn = Quat::from_axis_angle(-toward, 0.5);
                    turns(from, center + way * radius, Mat3::from_quat(turn) * before);
                }
            }
        }
    }

    /// A drag changes the others picked with the active one, each under a parent of its own, by the
    /// same change, about its own origin: Move by the same move in the world; Rotate by the same
    /// turn about the world's axis, or under Local about its own axis, as the active one's ring
    /// is; Scale by the same factor along its own axis. A pick the drag doesn't move, as one under
    /// another picked, sets nothing of its own, the others all the same.
    #[test]
    fn a_drag_changes_the_others_picked_by_the_same_change() {
        let parent = carrying(Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
            scale: Vec3::splat(2.0),
        });
        let theirs = carrying(Transform {
            translation: DVec3::new(-3.0, 0.0, 1.0),
            rotation: Quat::from_rotation_y(1.1),
            scale: Vec3::splat(0.5),
        });
        let other = Along {
            entity: "00000000000000ff".parse().expect("an ID"),
            local: turned(),
            parent: theirs,
        };
        let local = Transform::IDENTITY;
        let origin = parent.matrix.transform_point3(local.translation);
        let viewpoint = camera(origin.as_vec3(), [0.5, -0.9, 0.0], 40.0, false);
        let sight = sight(viewpoint);
        let origin = (origin - sight.eye).as_vec3();
        let at = |point: Vec3| sight.project(point).expect("in view");
        // What the drag from `from` to `to` sets, as the cursor reaches `to`: the active one's
        // field, and the other's.
        let dragged = |tool, space, moves, from, to| {
            let picked = Picked {
                moves,
                ..picked(local, parent)
            };
            let aim = Aim {
                space,
                along: &[other],
                ..aim(tool, viewpoint, picked)
            };
            let mut handles = Handles::default();
            handles.event(&moved(from), &reading(from), &aim);
            assert_eq!(
                handles.event(&left(true), &reading(from), &aim),
                Reply::Take
            );
            let reply = handles.event(&moved(to), &reading(to), &aim);
            let Reply::Set(_, own) = reply else {
                panic!("{reply:?}");
            };
            let sets = handles.sets();
            let theirs = sets.iter().find(|(entity, _)| *entity == other.entity);
            let mine = sets.iter().any(|(entity, _)| *entity != other.entity);
            assert_eq!(mine, moves, "{sets:?}");
            (own, theirs.expect("the other's set").1)
        };
        let placed = |space| place(&sight, space, &parent, &local).expect("in view");
        // Along the world's X.
        let world = placed(Space::World);
        let grabbed = origin + Vec3::X * world.length * 0.6;
        let (own, set) = dragged(
            Tool::Move,
            Space::World,
            true,
            at(grabbed),
            at(grabbed + Vec3::X),
        );
        let (Field::Translation(mine), Field::Translation(its)) = (own, set) else {
            panic!("{own:?}, {set:?}");
        };
        let moved_by = parent.matrix.transform_point3(mine)
            - parent.matrix.transform_point3(local.translation);
        let its_by = theirs.matrix.transform_point3(its)
            - theirs.matrix.transform_point3(other.local.translation);
        assert!(moved_by.abs_diff_eq(DVec3::X, 1e-3), "{moved_by}");
        assert!(
            its_by.abs_diff_eq(moved_by, 1e-3),
            "{its_by}, not {moved_by}"
        );
        // About the world's Z, and about each one's own Z, by 0.6.
        for space in [Space::World, Space::Local] {
            let shown = placed(space);
            let axis = shown.axes[2];
            let grabbed = facing(axis, shown.toward) * shown.length;
            let turn = Quat::from_axis_angle(axis, 0.6);
            let (from, to) = (at(origin + grabbed), at(origin + turn * grabbed));
            for moves in [true, false] {
                let (_, set) = dragged(Tool::Rotate, space, moves, from, to);
                let Field::Rotation(its) = set else {
                    panic!("{set:?}");
                };
                let expected = match space {
                    Space::World => {
                        Mat3::from_quat(Quat::from_rotation_z(0.6))
                            * drawn(&theirs, other.local.rotation)
                    }
                    Space::Local => {
                        drawn(&theirs, other.local.rotation * Quat::from_rotation_z(0.6))
                    }
                };
                let now = drawn(&theirs, its);
                assert!(
                    now.abs_diff_eq(expected, 1e-3),
                    "{space:?}: {now}, not {expected}"
                );
            }
        }
        // Along each one's own X, twice as long.
        let shown = placed(Space::Local);
        let along = |by: f32| origin + shown.axes[0] * shown.length * by;
        let (own, set) = dragged(
            Tool::Scale,
            Space::Local,
            true,
            at(along(0.95)),
            at(along(1.9)),
        );
        let (Field::Scale(mine), Field::Scale(its)) = (own, set) else {
            panic!("{own:?}, {set:?}");
        };
        assert!(mine.abs_diff_eq(Vec3::new(1.95, 1.0, 1.0), 1e-2), "{mine}");
        let expected = other.local.scale * mine;
        assert!(its.abs_diff_eq(expected, 1e-4), "{its}, not {expected}");
    }

    /// Scale shows and drags the entity's own axes in the world's space too, as in its own, since
    /// scaling a turned entity along the world's would shear it; while Move's and Rotate's go
    /// along the world's.
    #[test]
    fn in_the_worlds_space_scale_keeps_the_entitys_own_axes() {
        let parent = carrying(Transform {
            rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
            ..Transform::IDENTITY
        });
        let local = turned();
        let origin = parent.matrix.transform_point3(local.translation);
        let viewpoint = aside(origin.as_vec3(), false);
        let sight = sight(viewpoint);
        let picked = picked(local, parent);
        let own = place(&sight, Space::Local, &parent, &local).expect("in view");
        assert_ne!(own.axes, [Vec3::X, Vec3::Y, Vec3::Z]);
        let handles = Handles::default();
        let space = Space::World;
        let scaling = handles.placed(&sight, Tool::Scale, space, &picked);
        assert_eq!(scaling.expect("in view").axes, own.axes);
        for tool in [Tool::Move, Tool::Rotate] {
            let placed = handles
                .placed(&sight, tool, space, &picked)
                .expect("in view");
            assert_eq!(placed.axes, [Vec3::X, Vec3::Y, Vec3::Z], "{tool:?}");
        }
        // From axis Y's end, a handles' length further along it, which doubles it.
        let at = |point: Vec3| sight.project(point).expect("in view");
        let axis = own.axes[1];
        let end = own.origin + axis * own.length;
        let to = at(end + axis * own.length);
        let aim = Aim {
            space,
            ..aim(Tool::Scale, viewpoint, picked)
        };
        let replies = drag(&mut Handles::default(), &aim, at(end), &[to], false);
        let Field::Scale(scaled) = set(&replies) else {
            panic!("{replies:?}");
        };
        let expected = Vec3::new(local.scale.x, 2.0 * local.scale.y, local.scale.z);
        assert!(
            scaled.abs_diff_eq(expected, 1e-4),
            "{scaled}, not {expected}"
        );
    }

    /// Each color `shape` paints with, filling or stroking.
    fn colors(shape: &Shape, found: &mut Vec<Color32>) {
        match shape {
            Shape::Vec(shapes) => {
                for shape in shapes {
                    colors(shape, found);
                }
            }
            Shape::LineSegment { stroke, .. } => found.push(stroke.color),
            Shape::Circle(circle) => found.extend([circle.fill, circle.stroke.color]),
            Shape::Rect(rect) => found.push(rect.fill),
            Shape::Path(path) => {
                found.push(path.fill);
                if let egui::epaint::ColorMode::Solid(color) = path.stroke.color {
                    found.push(color);
                }
            }
            _ => {}
        }
    }

    /// Each tool's handles paint, at 1.5 of egui's pixels a point, as the window's are, the one
    /// under the cursor lit, and none without one.
    #[test]
    fn the_handles_paint_the_one_under_the_cursor_lit() {
        let sight = sight(aside(Vec3::ZERO, true));
        let picked = picked(turned(), Pose::IDENTITY);
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &picked.local).expect("in view");
        let outer = placed.center - Vec2::new(0.0, sight.pixels(OUTER * LENGTH));
        let ctx = egui::Context::default();
        for (tool, under) in [
            (Tool::Move, placed.center),
            (Tool::Rotate, outer),
            (Tool::Scale, placed.center),
        ] {
            for hover in [Some(under), None] {
                let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 500.0));
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    ..egui::RawInput::default()
                };
                let handles = Handles::default();
                let mut output = ctx.run_ui(input, |ui| {
                    let painter = ui.ctx().layer_painter(egui::LayerId::background());
                    let shown = (tool, Space::Local);
                    handles.paint(&painter, 1.5, &sight, shown, Some(&picked), hover);
                });
                output.textures_delta.clear();
                let mut found = Vec::new();
                for clipped in &output.shapes {
                    colors(&clipped.shape, &mut found);
                }
                assert!(found.len() > 3, "{tool:?}: {found:?}");
                assert_eq!(found.contains(&LIT), hover.is_some(), "{tool:?}, {hover:?}");
            }
        }
    }

    /// A drag ends as the Inspector undoes or redoes, as the pick changes or dies, and as the
    /// world is another; and the value a drag set shows, for the pick it set it for, until the
    /// Inspector's tick has made it.
    #[test]
    fn a_drag_ends_with_the_pick_the_world_or_a_step() {
        let viewpoint = aside(Vec3::ZERO, false);
        let sight = sight(viewpoint);
        let local = turned();
        let picked = picked(local, Pose::IDENTITY);
        let aim = aim(Tool::Move, viewpoint, picked);
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
        let from = sight.project(placed.origin + placed.axes[0] * placed.length * 0.7);
        let from = from.expect("in view");
        let to = from + Vec2::new(30.0, 0.0);
        let start = |handles: &mut Handles| {
            handles.event(&moved(from), &reading(from), &aim);
            assert_eq!(
                handles.event(&left(true), &reading(from), &aim),
                Reply::Take
            );
            let set = handles.event(&moved(to), &reading(to), &aim);
            assert!(matches!(set, Reply::Set(..)), "{set:?}");
        };
        let other = Picked {
            entity: "fedcba9876543210".parse().expect("an ID"),
            ..picked
        };
        let stepped = Picked {
            stepped: 1,
            ..picked
        };
        let restored = Picked {
            serial: 2,
            ..picked
        };
        for ending in [Some(stepped), Some(other), Some(restored), None] {
            let mut handles = Handles::default();
            start(&mut handles);
            assert!(!handles.hold(Some(&picked)) && handles.dragging());
            assert!(handles.hold(ending.as_ref()), "{ending:?}");
            assert!(!handles.dragging());
            assert_eq!(
                handles.shown(&picked).translation,
                local.translation,
                "{ending:?}"
            );
        }
        // Let go, it shows what it set while that waits for a tick, and then the world's.
        let mut handles = Handles::default();
        start(&mut handles);
        let dragged = handles.shown(&picked);
        assert_ne!(dragged.translation, local.translation);
        assert_eq!(handles.event(&left(false), &reading(to), &aim), Reply::Seal);
        let waiting = Picked {
            waiting: true,
            ..picked
        };
        assert!(!handles.hold(Some(&waiting)));
        assert_eq!(handles.shown(&waiting), dragged);
        handles.hold(Some(&picked));
        assert_eq!(handles.shown(&waiting), local);
    }

    /// While the camera eases, as after Home, a press on a handle grabs nothing, since the view
    /// turns under it, but is taken all the same; one off the handles still asks for a pick.
    #[test]
    fn a_press_while_the_camera_eases_grabs_nothing() {
        let viewpoint = aside(Vec3::ZERO, false);
        let sight = sight(viewpoint);
        let local = turned();
        let aim = Aim {
            easing: true,
            ..aim(Tool::Move, viewpoint, picked(local, Pose::IDENTITY))
        };
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
        let on = sight.project(placed.origin + placed.axes[0] * placed.length * 0.7);
        let mut handles = Handles::default();
        for (at, reply) in [
            (on.expect("in view"), Reply::Take),
            (Vec2::new(110.0, 60.0), Reply::Pick([110.0, 60.0])),
        ] {
            handles.event(&moved(at), &reading(at), &aim);
            assert_eq!(handles.event(&left(true), &reading(at), &aim), reply);
            assert!(!handles.dragging());
            handles.event(&left(false), &reading(at), &aim);
        }
    }

    /// With Probe, a press over the view asks for a probe of its pixel, where a handle of what's
    /// picked would have been too, since Probe shows none; not with Alt held, which orbits.
    #[test]
    fn a_press_with_probe_asks_for_a_probe_of_its_pixel() {
        let viewpoint = aside(Vec3::ZERO, false);
        let sight = sight(viewpoint);
        let local = turned();
        let aim = aim(Tool::Probe, viewpoint, picked(local, Pose::IDENTITY));
        let placed = place(&sight, Space::Local, &Pose::IDENTITY, &local).expect("in view");
        let on = sight.project(placed.origin + placed.axes[0] * placed.length * 0.7);
        let on = on.expect("in view");
        let mut handles = Handles::default();
        for at in [on, Vec2::new(110.0, 60.0)] {
            handles.event(&moved(at), &reading(at), &aim);
            let reply = handles.event(&left(true), &reading(at), &aim);
            assert_eq!(reply, Reply::Probe([f64::from(at.x), f64::from(at.y)]));
            assert!(!handles.dragging());
            handles.event(&left(false), &reading(at), &aim);
        }
        let at = Vec2::new(110.0, 60.0);
        let alt = Reading {
            screen: Screen {
                alt: true,
                ..reading(at).screen
            },
            ..reading(at)
        };
        assert_eq!(handles.event(&left(true), &alt, &aim), Reply::Pass);
    }
}
