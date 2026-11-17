//! Cameras: the component the game's view sees the world through, and which of the world's cameras
//! it uses; and the camera controllers, which steer a layer's own viewpoint from the keyboard,
//! mouse and gamepads: an orthographic one for 2D games, and a flying one for 3D.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
use std::fmt::Display;
use std::mem;
use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::Error;
use gg_core::math::{self, DVec3, Quat, Vec2, Vec3};
use gg_renderer::{Projection, Viewpoint};
use tracing::warn;

use crate::draw::Poses;
use crate::interpolation::Past;
use crate::placed::Placed;
use crate::reflect::{self, Digest, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value};
use crate::{
    Actions, Binding, Component, Context, Entity, EntityId, Event, Key, MouseButton, Source, Stick,
    Transform, Trigger, World, Writable,
};

/// The game's view, while [`primary`](Self::primary) and no layer sets a viewpoint.
///
/// A layer sets one with [`Context::set_viewpoint`]. The entity needs a [`Transform`]. It sees from
/// where the entity draws, between ticks too: ancestors carry and turn it, their scales don't
/// count. That position is drawing's alone: no hash, scene or recording holds it, only the
/// component.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Camera {
    /// How it projects what's in front of it onto the view, fitted to the view's aspect ratio.
    pub projection: Projection,
    /// Whether the view uses it. Making one primary (Inspector checkbox, other operations)
    /// unmakes the rest; undo restores them. If code or a scene makes several primary, the lowest
    /// ID wins with a warning (`engine.camera_tie`), as scenes keep no order. None primary: the
    /// default viewpoint.
    pub primary: bool,
}

/// [`primary`](Camera::primary) is its exclusive field.
impl Component for Camera {
    fn exclusive() -> Option<&'static str> {
        Some("primary")
    }
}

impl Writable for Camera {}

/// An enum of `Orthographic { height, near, far }` and `Perspective { fov, near }`, field of view
/// in degrees, set whole. It refuses a projection that can't project (`engine.bad_projection`),
/// so scenes, operations and the Inspector can't panic the render thread. Defaults are Hazel's
/// scene camera's: 10 units high, 1 behind to 1 in front; 45°, from 0.01.
impl Reflect for Projection {
    fn describe(types: &mut Types) -> Result<Type> {
        let field = |name: &'static str, show, default: f32, doc: &'static str| Field {
            name: name.into(),
            ty: Type::F32,
            meta: Meta {
                show,
                doc: doc.into(),
                ..Meta::default()
            },
            default: Some(default.to_value()),
        };
        let variants = vec![
            reflect::Variant {
                name: "Orthographic".into(),
                fields: vec![
                    field(
                        "height",
                        Show::Plain,
                        10.0,
                        "How much of the world the view spans from bottom to top: more than 0.",
                    ),
                    field(
                        "near",
                        Show::Plain,
                        -1.0,
                        "Where what it shows starts, in front of it; behind it, if negative.",
                    ),
                    field(
                        "far",
                        Show::Plain,
                        1.0,
                        "Where what it shows ends, in front of it, other than its near.",
                    ),
                ],
                tuple: false,
            },
            reflect::Variant {
                name: "Perspective".into(),
                fields: vec![
                    field(
                        "fov",
                        Show::Degrees,
                        FRAC_PI_4,
                        "How much the view spans from bottom to top: more than 0° and less than \
                         180°.",
                    ),
                    field(
                        "near",
                        Show::Plain,
                        0.01,
                        "How far in front of it what it shows starts, more than 0; it shows all \
                         beyond, however far.",
                    ),
                ],
                tuple: false,
            },
        ];
        let doc = "How a camera projects what's in front of it onto the view: without perspective, \
                   as 2D games draw, or with it, as 3D games do.";
        types.add::<Self>("Projection".into(), doc.into(), |_| {
            Ok(Shape::Enum(variants))
        })
    }

    fn to_value(&self) -> Value {
        match *self {
            Projection::Orthographic { height, near, far } => Value::Variant(
                "Orthographic".into(),
                vec![
                    ("height".into(), height.to_value()),
                    ("near".into(), near.to_value()),
                    ("far".into(), far.to_value()),
                ],
            ),
            Projection::Perspective { fov, near } => Value::Variant(
                "Perspective".into(),
                vec![
                    ("fov".into(), fov.to_value()),
                    ("near".into(), near.to_value()),
                ],
            ),
        }
    }

    fn from_value(value: &Value) -> Result<Self> {
        let (name, given) = reflect::variant(value)?;
        let number = |value: &Value, name: &str| {
            f32::from_value(value).map_err(|error| reflect::within(error, name))
        };
        let projection = match name {
            "Orthographic" => {
                let [height, near, far] = reflect::pick(given, ["height", "near", "far"])?;
                Projection::Orthographic {
                    height: number(height, "height")?,
                    near: number(near, "near")?,
                    far: number(far, "far")?,
                }
            }
            "Perspective" => {
                let [fov, near] = reflect::pick(given, ["fov", "near"])?;
                Projection::Perspective {
                    fov: number(fov, "fov")?,
                    near: number(near, "near")?,
                }
            }
            _ => return Err(reflect::no_variant(name, &["Orthographic", "Perspective"])),
        };
        if !projection.projects() {
            let needs = match projection {
                Projection::Orthographic { .. } => {
                    "a camera sees a positive height of the world, between a near and a far that \
                     differ"
                }
                Projection::Perspective { .. } => {
                    "a camera with perspective sees more than nothing and less than a half turn \
                     from bottom to top, from a near plane in front of it"
                }
            };
            return Err(Error::new(
                "engine.bad_projection",
                format!("{needs}; got {projection:?}"),
            ));
        }
        Ok(projection)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }

    /// By its variant's index, then its fields.
    fn digest(&self, to: &mut Digest<'_>) {
        match *self {
            Projection::Orthographic { height, near, far } => {
                0u32.digest(to);
                for field in [height, near, far] {
                    field.digest(to);
                }
            }
            Projection::Perspective { fov, near } => {
                1u32.digest(to);
                for field in [fov, near] {
                    field.digest(to);
                }
            }
        }
    }

    fn entities_mut(&mut self, _: &mut dyn FnMut(&mut Entity)) {}
}

/// Several primary cameras; the view uses the lowest ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tie {
    seen: EntityId,
    /// How many are primary.
    tied: usize,
}

impl Tie {
    /// Whether `now`, this frame's tie if any, differs from `warned`, the last one warned of.
    /// `warned` then holds `now`.
    fn news(now: Option<Self>, warned: &mut Option<Self>) -> bool {
        let news = now.is_some() && now != *warned;
        *warned = now;
        news
    }
}

/// What [`checked`] remembers of the last viewpoint's source `S`: a camera, a layer or the look.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Projected<S> {
    source: Option<S>,
    /// The last projection from that source that could project.
    good: Option<Projection>,
    /// Whether the frames since that couldn't project have warned.
    warned: bool,
}

impl<S> Default for Projected<S> {
    fn default() -> Self {
        Self {
            source: None,
            good: None,
            warned: false,
        }
    }
}

/// `viewpoint` from `source`, as drawn. Code can set a projection that can't project (reflection
/// refuses it) and the renderer panics on it, so such a viewpoint keeps its place but takes
/// `source`'s last good projection, else the default's, and warns once (`engine.bad_projection`)
/// until one projects. `projected` holds one source; another resets it, so projections don't cross
/// sources and each warns.
pub(crate) fn checked<S: Display + PartialEq + Copy>(
    viewpoint: Viewpoint,
    source: S,
    projected: &mut Projected<S>,
) -> Viewpoint {
    if projected.source != Some(source) {
        *projected = Projected {
            source: Some(source),
            ..Projected::default()
        };
    }
    if viewpoint.projection.projects() {
        projected.good = Some(viewpoint.projection);
        projected.warned = false;
        return viewpoint;
    }
    let instead = projected.good.unwrap_or(Viewpoint::default().projection);
    if !mem::replace(&mut projected.warned, true) {
        warn!(
            code = "engine.bad_projection",
            source = %source,
            "{source} has a projection that can't project, {:?}: the view draws through \
             {instead:?} instead",
            viewpoint.projection
        );
    }
    Viewpoint {
        projection: instead,
        ..viewpoint
    }
}

/// The primary camera's ID and viewpoint, as `world` draws `alpha` on from `past` with `placed`,
/// when no layer sets one; the lowest ID of several, `None` without. Ties warn
/// (`engine.camera_tie`) when they arise or change, not every frame; `warned` tracks it.
pub(crate) fn seen(
    world: &World,
    past: &Past,
    placed: &Placed,
    alpha: f32,
    warned: &mut Option<Tie>,
) -> Option<(EntityId, Viewpoint)> {
    let primary = || cameras(world).filter(|(.., camera)| camera.primary);
    let best = primary().min_by_key(|&(_, _, id, _)| id);
    let tie = best.and_then(|(_, _, seen, _)| {
        let tied = primary().count();
        (tied > 1).then_some(Tie { seen, tied })
    });
    if Tie::news(tie, warned)
        && let Some(Tie { seen, tied }) = tie
    {
        let ids: Vec<String> = primary().map(|(_, _, id, _)| id.to_string()).collect();
        let ids = ids.join(", ");
        warn!(
            code = "engine.camera_tie",
            seen = %seen,
            cameras = %ids,
            "{tied} cameras are primary, {ids}: the view sees through the one of the lowest ID, \
             {seen}"
        );
    }
    let (table, row, id, camera) = best?;
    // Where it is alone, which no origin changes.
    let poses = Poses::new(world, past, placed, alpha, DVec3::ZERO)?;
    let (position, rotation) = poses.pose(table, row);
    let viewpoint = Viewpoint {
        position,
        rotation,
        projection: camera.projection,
    };
    Some((id, viewpoint))
}

/// `world`'s cameras, its entities with a [`Camera`] and a [`Transform`]: table, row, ID, camera.
fn cameras(world: &World) -> impl Iterator<Item = (usize, usize, EntityId, &Camera)> {
    let ids = world.component::<Camera>();
    let ids = ids.zip(world.component::<Transform>());
    ids.into_iter().flat_map(move |(camera, transform)| {
        let tables = world.tables().iter().enumerate();
        let with = tables.filter(move |(_, table)| table.has(transform));
        with.flat_map(move |(index, table)| {
            let cameras = table.column::<Camera>(camera).unwrap_or_default();
            let rows = table.entities().iter().zip(cameras).enumerate();
            rows.map(move |(row, (&entity, camera))| {
                let id = world.id(entity).expect("a table's entities are alive");
                (index, row, id, camera)
            })
        })
    })
}

/// Movement, in the view's heights a second.
const SPEED: f32 = 0.5;
/// Turn rate in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;
/// Zoom per mouse wheel line.
const ZOOM: f32 = 1.1;
/// The fly camera's initial speed, in world units a second.
const FLY_SPEED: f32 = 2.0;
/// Look rate at full stick tilt, in radians a second: half a turn.
const LOOK_SPEED: f32 = PI;
/// Look per point of mouse drag, in radians.
const DRAG_LOOK: f32 = 0.003;
/// The most it looks up or down: 89°; at 90° its heading is lost.
const MAX_PITCH: f32 = 89.0 * PI / 180.0;

/// What the camera controller does; [`actions`](OrthographicCameraController::actions) binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAction {
    /// A pair of axes along the camera's own: right and up the view.
    Move,
    /// An axis: counterclockwise as the camera sees it while positive.
    Turn,
}

/// Steers an orthographic viewpoint from the keyboard, mouse wheel and gamepads.
///
/// W, A, S and D, or a gamepad's right stick, move it half the view's height a second along its own axes, so it
/// crosses the screen as fast at any zoom. With [`rotation`](Self::rotation) set, Q and E, or the
/// triggers, turn it a quarter turn a second. The wheel zooms about the view's center, 1.1 times a
/// line, between a hundredth and a hundred times the starting height.
///
/// A layer keeps one, and calls [`update`](Self::update) and [`event`](Self::event) from its own,
/// as `self.camera.event(cx, event)?`.
pub struct OrthographicCameraController {
    /// The viewpoint it steers; the layer can place, turn or zoom it too.
    pub viewpoint: Viewpoint,
    /// Whether turning turns the camera. Off at first.
    pub rotation: bool,
    /// What moves and turns it; rebind it here. At first W, A, S and D and the right stick; Q and
    /// E and the triggers.
    pub actions: Actions<CameraAction>,
    heights: [f32; 2],
}

impl OrthographicCameraController {
    /// A controller that steers `viewpoint`, zooming between a hundredth and a hundred times its
    /// height.
    ///
    /// # Panics
    ///
    /// If the viewpoint isn't orthographic, or its height isn't a positive normal number.
    pub fn new(mut viewpoint: Viewpoint) -> Self {
        let height = *height(&mut viewpoint.projection);
        assert!(
            height.is_normal() && height > 0.0,
            "the orthographic camera controller zooms a camera that sees a positive height of the \
             world; got {:?}",
            viewpoint.projection
        );
        let key = Source::Key;
        let mut actions = Actions::default();
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
            (CameraAction::Move, Binding::WASD),
            (CameraAction::Move, Binding::stick(Stick::Right)),
            (CameraAction::Turn, turn),
            (CameraAction::Turn, triggers),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the controller's own bindings are each of one kind, of other sources");
        Self {
            viewpoint,
            rotation: false,
            actions,
            // Positive normal numbers, as rendering requires, however near the height is to either
            // end of them.
            heights: [
                (height / 100.0).max(f32::MIN_POSITIVE),
                (height * 100.0).min(f32::MAX),
            ],
        }
    }

    /// Moves and turns the viewpoint by its actions for `dt`, and sets it as the game's view this
    /// frame.
    ///
    /// # Panics
    ///
    /// If the viewpoint isn't orthographic.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        if self.rotation {
            let turn = self.actions.axis(CameraAction::Turn);
            // About its look axis, counterclockwise as it sees it; normalized so rounding doesn't
            // build up.
            let by = Quat::from_rotation_z(turn * TURN_SPEED * seconds);
            self.viewpoint.rotation = (self.viewpoint.rotation * by).normalize();
        }
        // Along its own axes, so W moves up the screen however it's turned; a pair of axes goes no
        // further than 1, so diagonals are no faster.
        let along = self.actions.axis2(CameraAction::Move).extend(0.0);
        let height = *height(&mut self.viewpoint.projection);
        let by = self.viewpoint.rotation * along * height * SPEED * seconds;
        self.viewpoint.position += by.as_dvec3();
        cx.set_viewpoint(self.viewpoint);
    }

    /// Feeds its actions `event`, and zooms on a wheel or touchpad scroll, setting the viewpoint
    /// as the game's view. `Break` when it zoomed, so layers below skip the scroll.
    ///
    /// # Panics
    ///
    /// On a scroll, if the viewpoint isn't orthographic.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Polled as it updates.
        self.actions.event(event, |_| {});
        let Event::Scroll(scroll) = *event else {
            return ControlFlow::Continue(());
        };
        let lines = scroll.lines(cx.scale());
        // NaN doesn't zoom: it would leave a height that can't project.
        if lines == 0.0 || lines.is_nan() {
            return ControlFlow::Continue(());
        }
        // Scrolling up zooms in.
        let height = height(&mut self.viewpoint.projection);
        let [least, most] = self.heights;
        *height = (*height * math::libm::powf(ZOOM, -lines)).clamp(least, most);
        cx.set_viewpoint(self.viewpoint);
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

/// What the fly camera controller does; [`actions`](FlyCameraController::actions) binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyAction {
    /// A pair of axes: right, and forward where it looks.
    Move,
    /// An axis: up along the world's +Y while positive.
    Rise,
    /// A pair of axes: right, and up.
    Look,
    /// A button: while held, moving the mouse looks about.
    Drag,
}

/// Steers a flying viewpoint, under either projection.
///
/// W, A, S and D, or a gamepad's left stick, move it forward, where it looks, and sideways, 2 units a second at first. E and Q, or the
/// triggers, raise and lower it along the world's up. The right stick, or the mouse moved with
/// its right button held, looks about: half a turn a second at full tilt, or 0.003 radians a
/// point. Looking keeps it level, its right along the horizon, and no more than 89° up or down.
///
/// A layer keeps one and calls [`update`](Self::update) and [`event`](Self::event) as for the
/// orthographic controller. It keeps nothing of the viewpoint but [`viewpoint`](Self::viewpoint),
/// which the layer can place and turn; a viewpoint rolled about its look axis levels at the next
/// look.
pub struct FlyCameraController {
    pub viewpoint: Viewpoint,
    /// How fast it moves, in world units a second.
    pub speed: f32,
    /// What moves it and looks about; rebind it here. At first W, A, S and D and the left stick;
    /// E and Q and the triggers; the right stick; the right mouse button.
    pub actions: Actions<FlyAction>,
    /// Where the cursor last was in the window, in physical pixels; a drag looks from it.
    cursor: Option<[f64; 2]>,
}

impl FlyCameraController {
    /// A controller that steers `viewpoint` at 2 units a second, with the bindings the type names.
    pub fn new(viewpoint: Viewpoint) -> Self {
        let key = Source::Key;
        let mut actions = Actions::default();
        let rise = Binding::Axis {
            negative: key(Key::KeyQ),
            positive: key(Key::KeyE),
        };
        let triggers = Binding::Axis {
            negative: Source::PadTrigger(Trigger::Left),
            positive: Source::PadTrigger(Trigger::Right),
        };
        let bound = [
            (FlyAction::Move, Binding::WASD),
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
            viewpoint,
            speed: FLY_SPEED,
            actions,
            cursor: None,
        }
    }

    /// Looks about and moves the viewpoint by its actions for `dt`, and sets it as the game's view
    /// this frame.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let look = self.actions.axis2(FlyAction::Look);
        if look != Vec2::ZERO {
            self.look(look * LOOK_SPEED * seconds);
        }
        // Forward is where it looks, up or down too; no faster in any direction.
        let along = self.actions.axis2(FlyAction::Move);
        let rise = self.actions.axis(FlyAction::Rise);
        let way = self.viewpoint.rotation * Vec3::new(along.x, 0.0, -along.y) + Vec3::Y * rise;
        self.viewpoint.position += (way.clamp_length_max(1.0) * self.speed * seconds).as_dvec3();
        cx.set_viewpoint(self.viewpoint);
    }

    /// Feeds its actions `event`, and looks about on a cursor move while [`FlyAction::Drag`] is
    /// held, setting the viewpoint as the game's view. Always `Continue`.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Polled in update, except the drag, which must be held before the move is read.
        self.actions.event(event, |_| {});
        match *event {
            Event::CursorMoved { x, y } => {
                if let Some([from_x, from_y]) = self.cursor
                    && self.actions.held(FlyAction::Drag)
                {
                    let scale = cx.scale();
                    let points = |to: f64, from: f64| ((to - from) / scale) as f32;
                    // The window's y goes down: dragging down looks down.
                    let by = Vec2::new(points(x, from_x), -points(y, from_y));
                    self.look(by * DRAG_LOOK);
                    cx.set_viewpoint(self.viewpoint);
                }
                self.cursor = Some([x, y]);
            }
            Event::CursorLeft => self.cursor = None,
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// Turns the viewpoint `by` radians: right about the world's up by `x`, up about its own right
    /// by `y`, to at most [`MAX_PITCH`]. The result is level, with no roll.
    fn look(&mut self, by: Vec2) {
        let forward = self.viewpoint.rotation * Vec3::NEG_Z;
        // Facing -Z unturned, so a turn to its left is positive about +Y.
        let yaw = math::libm::atan2f(-forward.x, -forward.z) - by.x;
        let pitch = math::libm::asinf(forward.y.clamp(-1.0, 1.0)) + by.y;
        let pitch = pitch.clamp(-MAX_PITCH, MAX_PITCH);
        self.viewpoint.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    }
}

#[cfg(test)]
mod tests {
    use crate::transform::from_origin;

    use std::panic;

    use gg_renderer::{Draws, Shaders, Sprite, Textures};

    use super::*;
    use crate::draw::{Scratch, draw_world};
    use crate::font::Fonts;
    use crate::{Entity, Log, Name, Operation, Scroll};

    /// `height` tall, from 1 behind to 1 in front.
    fn orthographic(height: f32) -> Projection {
        Projection::Orthographic {
            height,
            near: -1.0,
            far: 1.0,
        }
    }

    /// Spawns a camera at `x`, 2 tall, primary or not.
    fn camera_at(world: &mut World, x: f32, primary: bool) -> Entity {
        let at = Transform {
            translation: DVec3::new(f64::from(x), 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let camera = Camera {
            projection: orthographic(2.0),
            primary,
        };
        world.spawn((at, camera))
    }

    /// What the view sees through in a frame of `world` as it is.
    fn seen_now(world: &World) -> Option<Viewpoint> {
        seen(
            world,
            &Past::default(),
            &Placed::of(world, &Past::default(), 1.0),
            1.0,
            &mut None,
        )
        .map(|(_, viewpoint)| viewpoint)
    }

    /// The view sees through the primary camera; without one, through none.
    #[test]
    fn the_view_sees_through_the_primary_camera() {
        let mut world = World::seeded(1);
        let other = camera_at(&mut world, 1.0, false);
        let primary = camera_at(&mut world, 2.0, true);
        // Without a transform it isn't a camera.
        world.spawn(Camera {
            projection: orthographic(9.0),
            primary: true,
        });
        let seen = seen_now(&world).expect("a camera");
        assert_eq!(seen.position, DVec3::new(2.0, 0.0, 0.0));
        assert_eq!(seen.projection, orthographic(2.0));
        world.despawn(primary).expect("alive");
        assert_eq!(seen_now(&world), None);
        world.get_mut::<Camera>(other).expect("alive").primary = true;
        assert_eq!(seen_now(&world).expect("a camera").position, DVec3::X);
    }

    #[test]
    fn of_several_primary_cameras_the_view_sees_through_the_one_of_the_lowest_id() {
        let mut world = World::seeded(1);
        let tied = [1.0, 2.0].map(|x| camera_at(&mut world, x, true));
        camera_at(&mut world, 3.0, false);
        let ids = tied.map(|camera| world.id(camera).expect("alive"));
        let lowest = usize::from(ids[1] < ids[0]);
        let mut warned = None;
        let seen = seen(
            &world,
            &Past::default(),
            &Placed::of(&world, &Past::default(), 1.0),
            1.0,
            &mut warned,
        )
        .expect("a camera")
        .1;
        assert_eq!(seen.position.x, [1.0, 2.0][lowest]);
        let tie = Tie {
            seen: ids[lowest],
            tied: 2,
        };
        assert_eq!(warned, Some(tie));
        // Demoted, the view uses the other, and nothing ties.
        let camera = world.get_mut::<Camera>(tied[lowest]).expect("alive");
        camera.primary = false;
        let seen = super::seen(
            &world,
            &Past::default(),
            &Placed::of(&world, &Past::default(), 1.0),
            1.0,
            &mut warned,
        )
        .expect("a camera")
        .1;
        assert_eq!(seen.position.x, [2.0, 1.0][lowest]);
        assert_eq!(warned, None);
    }

    /// Making a camera primary by an operation, as the Inspector's checkbox does, unmakes the
    /// previous one; undoing it restores that one.
    #[test]
    fn a_camera_made_primary_by_an_operation_is_the_only_one() {
        let mut world = World::seeded(1);
        let before = camera_at(&mut world, 1.0, true);
        let made = camera_at(&mut world, 2.0, false);
        let primary = |world: &World| {
            [before, made].map(|camera| world.get::<Camera>(camera).expect("alive").primary)
        };
        let mut log = Log::default();
        let make = Operation::Set {
            entity: world.id(made).expect("alive"),
            component: "Camera".into(),
            path: "primary".to_owned(),
            value: Value::Bool(true),
        };
        log.apply(&mut world, make).expect("set");
        assert_eq!(primary(&world), [false, true]);
        assert_eq!(seen_now(&world).expect("a camera").position.x, 2.0);
        assert!(log.undo(&mut world).expect("undone"));
        assert_eq!(primary(&world), [true, false]);
        assert_eq!(seen_now(&world).expect("a camera").position.x, 1.0);
        log.clear(&mut world);
    }

    #[test]
    fn a_tie_is_news_as_it_starts_and_as_it_changes_but_not_while_it_lasts() {
        let mut world = World::seeded(1);
        let [a, b] = [0, 1].map(|_| {
            let entity = world.spawn(());
            world.id(entity).expect("alive")
        });
        let tie = |seen, tied| Some(Tie { seen, tied });
        let mut warned = None;
        let frames = [
            None,
            tie(a, 2),
            tie(a, 2),
            tie(a, 3),
            tie(b, 3),
            None,
            tie(b, 3),
        ];
        let news: Vec<bool> = frames
            .into_iter()
            .map(|now| Tie::news(now, &mut warned))
            .collect();
        assert_eq!(news, [false, true, false, true, true, false, true]);
    }

    /// A child camera sees, bit for bit, from where a sprite with the same parent and transform
    /// draws: halfway between ticks, and in a headless frame (alpha 1) where the tick left it. It
    /// faces by its rotation and its parent's.
    #[test]
    fn a_camera_sees_from_where_it_draws_between_ticks() {
        let mut world = World::seeded(1);
        let turned = Transform {
            rotation: Quat::from_rotation_z(0.3),
            ..Transform::IDENTITY
        };
        let parent = world.spawn(turned);
        let local = Transform {
            translation: DVec3::new(1.0, 2.0, 0.5),
            rotation: Quat::from_rotation_x(0.2),
            ..Transform::IDENTITY
        };
        let camera = Camera {
            projection: orthographic(2.0),
            primary: true,
        };
        let children = [
            world.spawn((local, camera)),
            world.spawn((local, Sprite::default())),
        ];
        for child in children {
            world.set_parent(child, Some(parent)).expect("both alive");
        }
        let mut past = Past::default();
        past.capture(&mut world);
        // The tick moves the parent to x = 2.
        let moved = world.get_mut::<Transform>(parent).expect("alive");
        moved.translation.x = 2.0;
        past.resolve(&world);

        // Where the parent at `x` carries its children.
        let carried = |x| {
            let parent = Transform {
                translation: DVec3::new(x, 0.0, 0.0),
                ..turned
            };
            parent.matrix() * local.matrix()
        };
        let mut want = Draws::default();
        want.draw_scene();
        let id = world.id(children[1]).expect("alive").get();
        let drawn = from_origin(carried(1.0), DVec3::ZERO);
        want.extend_sprites(std::iter::once((drawn, Sprite::default(), id)));
        let mut draws = Draws::default();
        let shaders = &mut Shaders::default();
        draw_world(
            &world,
            &past,
            &Placed::of(&world, &past, 0.5),
            (0.5, DVec3::ZERO),
            0,
            &mut draws,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        assert_eq!(draws, want);
        let halfway = seen(
            &world,
            &past,
            &Placed::of(&world, &past, 0.5),
            0.5,
            &mut None,
        )
        .expect("a camera")
        .1;
        assert_eq!(halfway.position, carried(1.0).translation);
        let facing = turned.rotation * local.rotation;
        let rotation = halfway.rotation;
        assert!(rotation.abs_diff_eq(facing, 1e-6), "{rotation}");
        let headless = seen(
            &world,
            &past,
            &Placed::of(&world, &past, 1.0),
            1.0,
            &mut None,
        )
        .expect("a camera")
        .1;
        assert_eq!(headless.position, carried(2.0).translation);
    }

    #[test]
    fn a_camera_reset_sees_from_where_the_tick_left_it() {
        let mut world = World::seeded(1);
        let camera = camera_at(&mut world, 0.0, true);
        let mut past = Past::default();
        let mut tick = |world: &mut World, x: f32, reset: bool| {
            past.capture(world);
            let moved = world.get_mut::<Transform>(camera).expect("alive");
            moved.translation.x = f64::from(x);
            if reset {
                world.reset_interpolation(camera).expect("alive");
            }
            past.resolve(world);
            let halfway = seen(world, &past, &Placed::of(world, &past, 0.5), 0.5, &mut None)
                .expect("a camera")
                .1;
            halfway.position.x
        };
        assert_eq!(tick(&mut world, 4.0, false), 2.0);
        assert_eq!(tick(&mut world, 9.0, true), 9.0);
    }

    /// A camera sees as though unscaled: from its translation, turned by its rotation; under a
    /// scaled and mirrored parent, from where the parent carries it, facing by their rotations.
    #[test]
    fn a_camera_sees_as_though_unscaled() {
        let mut world = World::seeded(1);
        let rotation = Quat::from_rotation_y(0.4);
        let scaled = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            rotation,
            scale: Vec3::new(2.0, 3.0, 4.0),
        };
        let camera = Camera {
            projection: orthographic(2.0),
            primary: true,
        };
        let first = world.spawn((scaled, camera));
        let seen = seen_now(&world).expect("a camera");
        assert_eq!(seen.position, scaled.translation);
        assert_eq!(seen.rotation, rotation);
        world.despawn(first).expect("alive");

        let mirrored = Transform {
            translation: DVec3::Y,
            rotation: Quat::from_rotation_z(0.5),
            scale: Vec3::new(-2.0, 2.0, 2.0),
        };
        let parent = world.spawn(mirrored);
        let local = Transform {
            translation: DVec3::X,
            rotation,
            ..Transform::IDENTITY
        };
        let child = world.spawn((local, camera));
        world.set_parent(child, Some(parent)).expect("both alive");
        let seen = seen_now(&world).expect("a camera");
        assert_eq!(
            seen.position,
            (mirrored.matrix() * local.matrix()).translation
        );
        let facing = mirrored.rotation * rotation;
        assert!(seen.rotation.abs_diff_eq(facing, 1e-6), "{}", seen.rotation);
    }

    #[test]
    fn a_projection_is_described_as_its_variants_and_its_values_come_back_bit_for_bit() {
        let mut world = World::default();
        world.register::<Camera>().expect("described");
        let types = world.types();
        let projection = types.find("Projection").expect("described");
        let Shape::Enum(variants) = &types.info(projection).shape else {
            panic!("a projection is an enum");
        };
        let described: Vec<(&str, Vec<&str>)> = variants
            .iter()
            .map(|variant| {
                let fields = variant.fields.iter().map(|field| &*field.name).collect();
                (&*variant.name, fields)
            })
            .collect();
        assert_eq!(
            described,
            [
                ("Orthographic", vec!["height", "near", "far"]),
                ("Perspective", vec!["fov", "near"]),
            ]
        );
        assert_eq!(variants[1].fields[0].meta.show, Show::Degrees);
        for projection in [
            Projection::Orthographic {
                height: 4.5,
                near: -0.0,
                far: 7.25,
            },
            Projection::Perspective {
                fov: 1.25,
                near: f32::MIN_POSITIVE,
            },
        ] {
            let back = Projection::from_value(&projection.to_value()).expect("it projects");
            assert_eq!(format!("{back:?}"), format!("{projection:?}"));
        }
    }

    /// An added camera's default is orthographic, 10 units high, and projects. It isn't primary,
    /// so adding one changes no view. The perspective default is 45° from 0.01, and projects too.
    #[test]
    fn a_cameras_default_projects_and_isnt_primary() {
        let mut world = World::default();
        world.register::<Camera>().expect("registered");
        let camera = world.default_of("Camera").expect("a default");
        let camera = Camera::from_value(&camera).expect("made");
        let orthographic = Projection::Orthographic {
            height: 10.0,
            near: -1.0,
            far: 1.0,
        };
        assert_eq!(
            camera,
            Camera {
                projection: orthographic,
                primary: false,
            }
        );
        assert!(camera.projection.projects());
        let types = world.types();
        let projection = types.find("Projection").expect("described");
        let Shape::Enum(variants) = &types.info(projection).shape else {
            panic!("a projection is an enum");
        };
        let perspective = types.variant_default(&variants[1]).expect("a default");
        let perspective = Projection::from_value(&perspective).expect("it projects");
        let expected = Projection::Perspective {
            fov: FRAC_PI_4,
            near: 0.01,
        };
        assert_eq!(perspective, expected);
    }

    /// A named camera saves as its fields, its name as `Name: ("Camera")`; saving what a load
    /// made gives the same bytes.
    #[test]
    fn a_named_camera_saves_and_loads_as_it_was() {
        let mut world = World::seeded(1);
        let camera = Camera {
            projection: Projection::Perspective {
                fov: 1.25,
                near: 0.125,
            },
            primary: true,
        };
        let named = Name("Camera".to_owned());
        let entity = world.spawn((named, Transform::IDENTITY, camera));
        let text = world.save_scene(&[entity]).expect("saved");
        assert!(text.contains(r#"Name: ("Camera")"#), "{text}");
        let mut other = World::seeded(2);
        for registered in [
            other.register::<Camera>(),
            other.register::<Name>(),
            other.register::<Transform>(),
        ] {
            registered.expect("registered");
        }
        let roots = other.load_scene(&text).expect("loaded");
        assert_eq!(other.save_scene(&roots).expect("saved"), text);
        let loaded = other.get::<Camera>(roots[0]);
        assert_eq!(loaded, Some(&camera));
    }

    /// What can't project, as rendering's own test has it, plus a non-finite far.
    const UNPROJECTABLE: [Projection; 14] = [
        Projection::Orthographic {
            height: -2.0,
            near: -1.0,
            far: 1.0,
        },
        Projection::Orthographic {
            height: -f32::MIN_POSITIVE,
            near: -1.0,
            far: 1.0,
        },
        Projection::Orthographic {
            height: 0.0,
            near: -1.0,
            far: 1.0,
        },
        Projection::Orthographic {
            height: f32::NAN,
            near: -1.0,
            far: 1.0,
        },
        Projection::Orthographic {
            height: 2.0,
            near: 1.0,
            far: 1.0,
        },
        Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: f32::INFINITY,
        },
        Projection::Perspective {
            fov: 0.0,
            near: 0.1,
        },
        Projection::Perspective { fov: PI, near: 0.1 },
        Projection::Perspective {
            fov: f32::NAN,
            near: 0.1,
        },
        Projection::Perspective {
            fov: -1.0,
            near: 0.1,
        },
        Projection::Perspective {
            fov: 1.0,
            near: 0.0,
        },
        Projection::Perspective {
            fov: 1.0,
            near: -0.1,
        },
        Projection::Perspective {
            fov: 1.0,
            near: f32::NAN,
        },
        Projection::Perspective {
            fov: 1.0,
            near: f32::MIN_POSITIVE / 2.0,
        },
    ];

    /// The error code of `result`, if it failed.
    fn code<T>(result: Result<T>) -> Option<String> {
        result.err().map(|error| error.code().to_owned())
    }

    /// A projection that can't project is refused however reflection would make it: from a value,
    /// set by name, created or added by an operation, saved, loaded from a scene. The camera set
    /// keeps its projection.
    #[test]
    fn a_projection_that_cant_project_is_refused_however_it_comes() {
        let refused = Some("engine.bad_projection".to_owned());
        let mut world = World::seeded(1);
        let good = Camera {
            projection: orthographic(2.0),
            primary: true,
        };
        let entity = world.spawn((Transform::IDENTITY, good));
        let id = world.id(entity).expect("alive");
        let bare = world.spawn(Transform::IDENTITY);
        let bare_id = world.id(bare).expect("alive");
        let mut log = Log::default();
        for bad in UNPROJECTABLE {
            let value = bad.to_value();
            assert_eq!(code(Projection::from_value(&value)), refused, "{bad:?}");
            let set = world.set_field(entity, "Camera", "projection", &value);
            assert_eq!(code(set), refused, "{bad:?} set");
            let camera = Camera {
                projection: bad,
                ..good
            };
            let create = Operation::Create {
                entity: None,
                parent: None,
                components: vec![("Camera".into(), camera.to_value())],
            };
            let created = log.apply(&mut world, create);
            assert_eq!(code(created), refused, "{bad:?} created");
            let add = Operation::Add {
                entity: bare_id,
                component: "Camera".into(),
                value: camera.to_value(),
            };
            assert_eq!(code(log.apply(&mut world, add)), refused, "{bad:?} added");
            let operation = Operation::Set {
                entity: id,
                component: "Camera".into(),
                path: "projection".to_owned(),
                value,
            };
            let operated = log.apply(&mut world, operation);
            assert_eq!(code(operated), refused, "{bad:?} by an operation");
            // Made by code, it can't be saved.
            let made = world.spawn((Transform::IDENTITY, camera));
            assert_eq!(code(world.save_scene(&[made])), refused, "{bad:?} saved");
            world.despawn(made).expect("alive");
        }
        assert_eq!(world.get::<Camera>(entity), Some(&good));
        let text = world.save_scene(&[entity]).expect("saved");
        let flat = text.replace("height: 2.0", "height: 0.0");
        assert_ne!(flat, text, "{text}");
        let mut other = World::seeded(2);
        other.register::<Transform>().expect("registered");
        other.register::<Camera>().expect("registered");
        assert_eq!(code(other.load_scene(&flat)), refused, "{flat}");
    }

    /// At the origin, 4 units high.
    fn viewpoint() -> Viewpoint {
        Viewpoint {
            projection: orthographic(4.0),
            ..Viewpoint::default()
        }
    }

    fn height(mut viewpoint: Viewpoint) -> f32 {
        *super::height(&mut viewpoint.projection)
    }

    /// Presses `keys` through the controller's event handler.
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
        // A quarter turn counterclockwise: its up is world -X, its right +Y.
        let turned = Viewpoint {
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            ..viewpoint()
        };
        let diagonal = Vec3::new(-1.0, 1.0, 0.0).normalize();
        for (keys, expected) in [
            (&[Key::KeyW][..], Vec3::new(-1.0, 0.0, 0.0)),
            // No farther diagonally.
            (&[Key::KeyW, Key::KeyD], diagonal),
        ] {
            let mut cx = Context::new(None);
            let mut controller = OrthographicCameraController::new(turned);
            hold(&mut controller, &mut cx, keys);
            controller.update(&mut cx, Duration::from_millis(500));
            let moved = controller.viewpoint.position;
            assert!(
                moved.abs_diff_eq(expected.as_dvec3(), 1e-5),
                "{keys:?} moved it to {moved}, not {expected}"
            );
            assert_eq!(cx.viewpoint(), Some(controller.viewpoint));
        }
    }

    #[test]
    fn q_and_e_turn_it_only_once_rotation_is_set() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(viewpoint());
        hold(&mut controller, &mut cx, &[Key::KeyQ]);
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.viewpoint.rotation, Quat::IDENTITY);
        controller.rotation = true;
        controller.update(&mut cx, Duration::from_secs(1));
        let turned = controller.viewpoint.rotation;
        assert!(
            turned.abs_diff_eq(Quat::from_rotation_z(FRAC_PI_2), 1e-6),
            "Q turned it to {turned}, not a quarter counterclockwise"
        );
    }

    #[test]
    fn its_keys_bind_again() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(viewpoint());
        controller.actions.unbind(CameraAction::Move);
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let bound = controller.actions.bind(CameraAction::Move, arrows);
        bound.expect("the arrows are free");
        hold(&mut controller, &mut cx, &[Key::KeyW]);
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.viewpoint.position, DVec3::ZERO);
        // Half the view's height in half a second.
        hold(&mut controller, &mut cx, &[Key::ArrowUp]);
        controller.update(&mut cx, Duration::from_millis(500));
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(DVec3::Y, 1e-6), "{moved}");
    }

    #[test]
    fn a_gamepads_right_stick_moves_it_and_its_triggers_turn_it() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(viewpoint());
        let pad = crate::Pad(0);
        let mut send = |controller: &mut OrthographicCameraController, event| {
            assert_eq!(controller.event(&mut cx, &event), ControlFlow::Continue(()));
        };
        // Half up for a second.
        let stick = |value| Event::PadStick {
            pad,
            stick: Stick::Right,
            value,
        };
        send(&mut controller, stick(Vec2::new(0.0, 0.5)));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(DVec3::Y, 1e-6), "{moved}");
        // The left trigger half down for a second.
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
        let turned = controller.viewpoint.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
        // Both triggers cancel.
        send(&mut controller, trigger(Trigger::Right, 0.5));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let turned = controller.viewpoint.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
    }

    /// Scrolls, expecting a zoom, and returns the new height.
    fn scroll(controller: &mut OrthographicCameraController, cx: &mut Context, by: Scroll) -> f32 {
        let handled = controller.event(cx, &Event::Scroll(by));
        assert_eq!(handled, ControlFlow::Break(()), "{by:?}");
        assert_eq!(cx.viewpoint(), Some(controller.viewpoint));
        height(controller.viewpoint)
    }

    #[test]
    fn the_wheel_zooms_between_a_hundredth_and_a_hundred_times_the_starting_height() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(viewpoint());
        // A line up zooms in; 40 pixels down, a touchpad line without display scaling, zooms out.
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
        assert_eq!(height(controller.viewpoint), 400.0);
    }

    #[test]
    #[should_panic(expected = "steers an orthographic camera")]
    fn the_orthographic_controller_refuses_a_camera_with_perspective() {
        let perspective = Viewpoint {
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
            ..Viewpoint::default()
        };
        OrthographicCameraController::new(perspective);
    }

    /// A height that can't project, such as an upside-down one, is refused. The wheel zooms any
    /// projecting height to one that projects, near either float extreme and however far it
    /// scrolls; a NaN scroll passes by.
    #[test]
    fn the_orthographic_controller_zooms_only_heights_that_project() {
        let seeing = |height| Viewpoint {
            projection: orthographic(height),
            ..Viewpoint::default()
        };
        for height in [
            -4.0,
            -f32::MIN_POSITIVE,
            0.0,
            f32::MIN_POSITIVE / 2.0,
            f32::INFINITY,
            f32::NAN,
        ] {
            let made = panic::catch_unwind(|| OrthographicCameraController::new(seeing(height)));
            assert!(made.is_err(), "a height of {height} was taken");
        }
        let mut cx = Context::new(None);
        for height in [f32::MIN_POSITIVE, f32::MAX] {
            let mut controller = OrthographicCameraController::new(seeing(height));
            for y in [1000.0, -1000.0, f32::INFINITY, f32::NEG_INFINITY] {
                let zoomed = scroll(&mut controller, &mut cx, Scroll::Lines { x: 0.0, y });
                assert!(
                    zoomed.is_normal() && zoomed > 0.0,
                    "{height} zoomed to {zoomed}"
                );
            }
            let before = controller.viewpoint;
            let nan = Event::Scroll(Scroll::Lines {
                x: 0.0,
                y: f32::NAN,
            });
            assert_eq!(controller.event(&mut cx, &nan), ControlFlow::Continue(()));
            assert_eq!(controller.viewpoint, before);
        }
    }

    /// At the origin, unturned, looking down -Z, perspective.
    fn flying() -> FlyCameraController {
        FlyCameraController::new(Viewpoint {
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
            ..Viewpoint::default()
        })
    }

    /// Sends `events`, each of which the controller passes on.
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

    /// Where the viewpoint looks, and its right.
    fn facing(controller: &FlyCameraController) -> (Vec3, Vec3) {
        let rotation = controller.viewpoint.rotation;
        (rotation * Vec3::NEG_Z, rotation * Vec3::X)
    }

    #[test]
    fn the_fly_camera_moves_where_it_looks_and_rises_along_the_worlds_up() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        let half = Duration::from_millis(500);
        send(&mut controller, &mut cx, &[key(Key::KeyW, true)]);
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(DVec3::NEG_Z, 1e-6), "{moved}");
        assert_eq!(cx.viewpoint(), Some(controller.viewpoint));
        // A quarter turn to its left, and it goes along -X.
        controller.viewpoint.position = DVec3::ZERO;
        controller.viewpoint.rotation = Quat::from_rotation_y(FRAC_PI_2);
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(DVec3::NEG_X, 1e-6), "{moved}");
        // Looking down an eighth of a turn, E raises it straight up, as fast as W moves.
        send(&mut controller, &mut cx, &[key(Key::KeyW, false)]);
        send(&mut controller, &mut cx, &[key(Key::KeyE, true)]);
        controller.viewpoint.position = DVec3::ZERO;
        controller.viewpoint.rotation = Quat::from_rotation_x(-FRAC_PI_2 / 2.0);
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(DVec3::Y, 1e-6), "{moved}");
        // Both at once, unturned, no faster.
        send(&mut controller, &mut cx, &[key(Key::KeyW, true)]);
        controller.viewpoint.position = DVec3::ZERO;
        controller.viewpoint.rotation = Quat::IDENTITY;
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position.length();
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
        // Full up for a second would pass straight up; it stops at 89°.
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
        // Without the button, nothing; with it, from the last cursor position, 0.003 radians a
        // point (a pixel without window scaling).
        send(
            &mut controller,
            &mut cx,
            &[at(10.0, 10.0), at(500.0, 300.0)],
        );
        assert_eq!(controller.viewpoint.rotation, Quat::IDENTITY);
        send(&mut controller, &mut cx, &[right(true), at(600.0, 300.0)]);
        let turned = controller.viewpoint.rotation;
        let expected = Quat::from_rotation_y(-0.3);
        assert!(turned.abs_diff_eq(expected, 1e-6), "{turned}");
        assert_eq!(cx.viewpoint(), Some(controller.viewpoint));
        // Dragging down looks down.
        send(&mut controller, &mut cx, &[at(600.0, 400.0)]);
        let (forward, _) = facing(&controller);
        let pitch = math::libm::asinf(forward.y);
        assert!((pitch + 0.3).abs() < 1e-5, "{pitch}");
        // Re-entering elsewhere looks from there, not from where it left.
        let left = controller.viewpoint.rotation;
        send(&mut controller, &mut cx, &[Event::CursorLeft, at(0.0, 0.0)]);
        assert_eq!(controller.viewpoint.rotation, left);
        // Let go, nothing.
        send(&mut controller, &mut cx, &[right(false), at(100.0, 100.0)]);
        assert_eq!(controller.viewpoint.rotation, left);
    }

    #[test]
    fn the_fly_camera_levels_a_camera_turned_about_where_it_looks_as_it_looks_about() {
        let mut cx = Context::new(None);
        let mut controller = flying();
        controller.viewpoint.rotation = Quat::from_rotation_z(1.0);
        controller.update(&mut cx, Duration::from_secs(1));
        assert_eq!(controller.viewpoint.rotation, Quat::from_rotation_z(1.0));
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
