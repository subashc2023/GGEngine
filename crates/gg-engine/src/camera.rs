//! Cameras: the component the game's view sees the world through, from where its entity draws, and
//! which of the world's cameras it sees through; and the camera controllers, which steer a layer's
//! own viewpoint from the keyboard, the mouse and gamepads, an orthographic one as 2D games do, and
//! one that flies, as 3D tools do.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::Error;
use gg_core::math::{self, Quat, Vec2, Vec3};
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

/// What the game's view sees the world through, from where its entity is, which has a
/// [`Transform`] too, while it's [`primary`](Self::primary), in frames no layer sets a viewpoint
/// of its own, through [`Context::set_viewpoint`]. It sees from where its entity draws, between
/// ticks too, carried by its ancestors: from its place in the world, facing as its rotation and
/// theirs turn it, their scales aside.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Camera {
    /// How it projects what's in front of it onto the view, fitted to the view's aspect ratio.
    pub projection: Projection,
    /// Whether the view sees through it. Making a camera primary, as the Inspector's checkbox
    /// or another operation does, makes every other not, and undoing it makes them primary again;
    /// of several made primary otherwise, by code or a scene, the view sees through the one of
    /// the lowest ID, with a warning (`engine.camera_tie`); and without one, from the default
    /// viewpoint.
    pub primary: bool,
}

/// [`primary`](Camera::primary) is its exclusive field.
impl Component for Camera {
    fn exclusive() -> Option<&'static str> {
        Some("primary")
    }
}

impl Writable for Camera {}

/// An enum of `Orthographic { height, near, far }` and `Perspective { fov, near }`, its field of
/// view shown in degrees, set whole. It refuses one that can't project (`engine.bad_projection`),
/// on which rendering would panic, so that nothing reflection makes, as scenes, operations and
/// the Inspector do, panics the render thread. Its fields' defaults, which project, are Hazel's
/// scene camera's: 10 units high, from 1 behind to 1 in front; and 45°, from 0.01 in front.
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

/// Cameras primary at once, of which the view sees through the one of the lowest ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tie {
    /// The camera the view sees through.
    seen: EntityId,
    /// How many are primary.
    tied: usize,
}

impl Tie {
    /// Whether `now`, the tie this frame if there's one, is news: a tie other than `warned`, the
    /// last warned of while they've tied since, which then holds `now`.
    fn news(now: Option<Self>, warned: &mut Option<Self>) -> bool {
        let news = now.is_some() && now != *warned;
        *warned = now;
        news
    }
}

/// What the game's view sees through in frames no layer sets a viewpoint, as `world` draws `alpha`
/// of the way on from `past`, placed as `placed` has it: its primary camera, by its ID, from where
/// it draws, and of several, the one of the lowest ID; `None` without one. Several warn
/// (`engine.camera_tie`) as they come to be and as they change, which `warned` keeps track of,
/// rather than every frame.
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
    let (position, rotation) = Poses::new(world, past, placed, alpha)?.pose(table, row);
    let viewpoint = Viewpoint {
        position,
        rotation,
        projection: camera.projection,
    };
    Some((id, viewpoint))
}

/// Each of `world`'s cameras, its entities with a [`Camera`] and a [`Transform`]: its table, its
/// row there, its ID and its camera.
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

/// How fast the camera moves, in the view's heights a second.
const SPEED: f32 = 0.5;
/// How fast it turns, in radians a second: a quarter turn.
const TURN_SPEED: f32 = FRAC_PI_2;
/// How much a line of the mouse wheel zooms.
const ZOOM: f32 = 1.1;
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

/// Steers an orthographic viewpoint from the keyboard, the mouse wheel and gamepads, as 2D games
/// and tools do. W, A, S and D, or a gamepad's right stick, move it along its own axes, half the
/// view's height a second, so that it crosses the screen as fast however far it's zoomed; Q and E,
/// or the left and right triggers, turn it left and right, a quarter turn a second, if
/// [`rotation`](Self::rotation) is set; and the mouse wheel zooms about the view's center, 1.1
/// times a line, between a hundredth and a hundred times the height it started at.
/// [`actions`](Self::actions) binds moving and turning to other keys and buttons.
///
/// A layer keeps one, and calls [`update`](Self::update) from its own update and
/// [`event`](Self::event) from its own event handler, as `self.camera.event(cx, event)?`, which is
/// how the keys reach it.
pub struct OrthographicCameraController {
    /// The viewpoint it steers, which the layer can place, turn or zoom itself too.
    pub viewpoint: Viewpoint,
    /// Whether turning turns the camera; at first it doesn't.
    pub rotation: bool,
    /// What moves and turns it: at first W, A, S and D and the right stick, and Q and E and the
    /// triggers.
    pub actions: Actions<CameraAction>,
    /// The least and most of the world the view's height zooms between.
    heights: [f32; 2],
}

impl OrthographicCameraController {
    /// A controller that steers `viewpoint`, zooming between a hundredth and a hundred times the
    /// height it sees now.
    ///
    /// # Panics
    ///
    /// If the viewpoint isn't orthographic, or its height isn't a positive normal number, as
    /// rendering requires.
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

    /// Moves and turns the viewpoint by its actions, for `dt`, and has the game's view see from it
    /// this frame.
    ///
    /// # Panics
    ///
    /// If the viewpoint isn't orthographic.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        if self.rotation {
            let turn = self.actions.axis(CameraAction::Turn);
            // About the axis it looks along, counterclockwise as it sees it; normalized, so that
            // rounding doesn't build up frame after frame.
            let by = Quat::from_rotation_z(turn * TURN_SPEED * seconds);
            self.viewpoint.rotation = (self.viewpoint.rotation * by).normalize();
        }
        // Along the camera's own axes, so that W moves up the screen however it's turned; as fast
        // diagonally as straight, since a pair of axes goes no further than 1.
        let along = self.actions.axis2(CameraAction::Move).extend(0.0);
        let height = *height(&mut self.viewpoint.projection);
        self.viewpoint.position += self.viewpoint.rotation * along * height * SPEED * seconds;
        cx.set_viewpoint(self.viewpoint);
    }

    /// Feeds its actions `event`, and zooms the viewpoint for a scroll of the mouse wheel or a
    /// touchpad, having the game's view see from it. `Break` when it zoomed, so that the layers
    /// below don't act on the same scroll.
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
        // Nor does a scroll of NaN zoom, which would leave a height that can't project.
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

/// Steers a viewpoint that flies, as 3D tools' and games' free cameras do, under either
/// projection. W, A, S and D, or a gamepad's left stick, move it forward, where it looks, and to
/// its sides, 2 units a second at first; E and Q, or the right and left triggers, raise and lower
/// it along the world's up; and the right stick, or the mouse moved with its right button held,
/// looks about, half a turn a second at full tilt, or 0.003 radians a point. Looking keeps it
/// level, its right along the world's horizon, and looks no further up or down than 89°.
/// [`actions`](Self::actions) binds them to other keys and buttons.
///
/// A layer keeps one, and calls [`update`](Self::update) from its own update and
/// [`event`](Self::event) from its own event handler, as the orthographic camera controller is.
/// It keeps nothing of the viewpoint but [`viewpoint`](Self::viewpoint), which the layer can place
/// and turn itself: a viewpoint turned about where it looks levels as it next looks about.
pub struct FlyCameraController {
    /// The viewpoint it steers.
    pub viewpoint: Viewpoint,
    /// How fast it moves, in world units a second.
    pub speed: f32,
    /// What moves it and looks about: at first W, A, S and D and the left stick, E and Q and the
    /// triggers, the right stick, and the right mouse button.
    pub actions: Actions<FlyAction>,
    /// Where the cursor last was in the window, in physical pixels, which a drag looks from.
    cursor: Option<[f64; 2]>,
}

impl FlyCameraController {
    /// A controller that steers `viewpoint`, at 2 units a second, with the bindings the type names.
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

    /// Looks about and moves the viewpoint by its actions, for `dt`, and has the game's view see
    /// from it this frame.
    pub fn update(&mut self, cx: &mut Context, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let look = self.actions.axis2(FlyAction::Look);
        if look != Vec2::ZERO {
            self.look(look * LOOK_SPEED * seconds);
        }
        // Forward is where it looks, up or down too; as fast every way, rising too.
        let along = self.actions.axis2(FlyAction::Move);
        let rise = self.actions.axis(FlyAction::Rise);
        let way = self.viewpoint.rotation * Vec3::new(along.x, 0.0, -along.y) + Vec3::Y * rise;
        self.viewpoint.position += way.clamp_length_max(1.0) * self.speed * seconds;
        cx.set_viewpoint(self.viewpoint);
    }

    /// Feeds its actions `event`, and looks about for a move of the cursor while
    /// [`FlyAction::Drag`] is held, having the game's view see from the viewpoint. It handles
    /// nothing for the layers below: `Continue`, always.
    pub fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Polled as it updates, but for the drag, which is held before the move is read.
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

    /// Turns the viewpoint `by` radians: to its right about the world's up by `x`, and up about
    /// its own right by `y`, no further than [`MAX_PITCH`]; level, without a turn about where it
    /// looks.
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
    use std::panic;

    use gg_renderer::{Draws, Shaders, Sprite, Textures};

    use super::*;
    use crate::draw::{Scratch, draw_world};
    use crate::font::Fonts;
    use crate::{Entity, Log, Name, Operation, Scroll};

    /// Without perspective, `height` tall, from 1 behind it to 1 in front.
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
            translation: Vec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        };
        let camera = Camera {
            projection: orthographic(2.0),
            primary,
        };
        world.spawn((at, camera))
    }

    /// What the view sees through in a frame of `world` as it is, with nothing to draw on from.
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

    /// The view sees through the primary camera; without one, through none, though the world has
    /// others.
    #[test]
    fn the_view_sees_through_the_primary_camera() {
        let mut world = World::seeded(1);
        let other = camera_at(&mut world, 1.0, false);
        let primary = camera_at(&mut world, 2.0, true);
        // Primary too, without a transform, it isn't a camera.
        world.spawn(Camera {
            projection: orthographic(9.0),
            primary: true,
        });
        let seen = seen_now(&world).expect("a camera");
        assert_eq!(seen.position, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(seen.projection, orthographic(2.0));
        world.despawn(primary).expect("alive");
        assert_eq!(seen_now(&world), None);
        world.get_mut::<Camera>(other).expect("alive").primary = true;
        assert_eq!(seen_now(&world).expect("a camera").position, Vec3::X);
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
        // That one no longer primary, the view sees through the other, and nothing ties.
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

    /// Made primary by an operation, as the Inspector's checkbox makes it, a camera is the one the
    /// view sees through, and the one before isn't primary; undone, the one before is again, and
    /// the view sees through it.
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

    /// A camera that's a child sees from where a sprite beside it, of the same parent and
    /// transform, draws, bit for bit: halfway on its way between ticks, and in a headless frame,
    /// at alpha 1, where the tick left it; facing as its own rotation and its parent's turn it.
    #[test]
    fn a_camera_sees_from_where_it_draws_between_ticks() {
        let mut world = World::seeded(1);
        let turned = Transform {
            rotation: Quat::from_rotation_z(0.3),
            ..Transform::IDENTITY
        };
        let parent = world.spawn(turned);
        let local = Transform {
            translation: Vec3::new(1.0, 2.0, 0.5),
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
        // The tick moves the parent 2 along X.
        let moved = world.get_mut::<Transform>(parent).expect("alive");
        moved.translation.x = 2.0;
        past.resolve(&world);

        // Where the parent at `x` along X carries its children.
        let carried = |x| {
            let parent = Transform {
                translation: Vec3::new(x, 0.0, 0.0),
                ..turned
            };
            parent.matrix() * local.matrix()
        };
        let mut want = Draws::default();
        want.draw_scene();
        let id = world.id(children[1]).expect("alive").get();
        want.extend_sprites(std::iter::once((carried(1.0), Sprite::default(), id)));
        let mut draws = Draws::default();
        let shaders = &mut Shaders::default();
        draw_world(
            &world,
            &past,
            &Placed::of(&world, &past, 0.5),
            0.5,
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
        assert_eq!(halfway.position, carried(1.0).w_axis.truncate());
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
        assert_eq!(headless.position, carried(2.0).w_axis.truncate());
    }

    #[test]
    fn a_camera_reset_sees_from_where_the_tick_left_it() {
        let mut world = World::seeded(1);
        let camera = camera_at(&mut world, 0.0, true);
        let mut past = Past::default();
        let mut tick = |world: &mut World, x: f32, reset: bool| {
            past.capture(world);
            let moved = world.get_mut::<Transform>(camera).expect("alive");
            moved.translation.x = x;
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

    /// A camera sees as though unscaled: scaled, it sees from its translation, turned by its
    /// rotation; and under a parent scaled and mirrored, from where the parent carries it, facing as
    /// their rotations turn it.
    #[test]
    fn a_camera_sees_as_though_unscaled() {
        let mut world = World::seeded(1);
        let rotation = Quat::from_rotation_y(0.4);
        let scaled = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
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
            translation: Vec3::Y,
            rotation: Quat::from_rotation_z(0.5),
            scale: Vec3::new(-2.0, 2.0, 2.0),
        };
        let parent = world.spawn(mirrored);
        let local = Transform {
            translation: Vec3::X,
            rotation,
            ..Transform::IDENTITY
        };
        let child = world.spawn((local, camera));
        world.set_parent(child, Some(parent)).expect("both alive");
        let seen = seen_now(&world).expect("a camera");
        assert_eq!(
            seen.position,
            (mirrored.matrix() * local.matrix()).w_axis.truncate()
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

    /// A camera's default, which an add makes, is orthographic, 10 units high, and projects; and
    /// it isn't primary, so that adding one changes no view. Switched to perspective by default,
    /// it's 45° from 0.01 in front, and projects too.
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

    /// A named camera saves as its fields, its name as `Name: ("Camera")`, and what a load of it
    /// saves is byte for byte what was loaded.
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

    /// What can't project, as rendering's own test has it, and with a far that isn't finite.
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

    /// The code of the error `result` holds, if it holds one.
    fn code<T>(result: Result<T>) -> Option<String> {
        result.err().map(|error| error.code().to_owned())
    }

    /// A projection that can't project is refused however reflection would make it: from a value,
    /// set by name, created or added by an operation, saved, and loaded from a scene; and the
    /// camera set keeps its projection.
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
        let turned = Viewpoint {
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            ..viewpoint()
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
            let moved = controller.viewpoint.position;
            assert!(
                moved.abs_diff_eq(expected, 1e-5),
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
        assert_eq!(controller.viewpoint.position, Vec3::ZERO);
        // Half the view's height in half a second.
        hold(&mut controller, &mut cx, &[Key::ArrowUp]);
        controller.update(&mut cx, Duration::from_millis(500));
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(Vec3::Y, 1e-6), "{moved}");
    }

    #[test]
    fn a_gamepads_right_stick_moves_it_and_its_triggers_turn_it() {
        let mut cx = Context::new(None);
        let mut controller = OrthographicCameraController::new(viewpoint());
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
        let moved = controller.viewpoint.position;
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
        let turned = controller.viewpoint.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
        // Both triggers as far cancel.
        send(&mut controller, trigger(Trigger::Right, 0.5));
        controller.update(&mut Context::new(None), Duration::from_secs(1));
        let turned = controller.viewpoint.rotation;
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
    }

    /// Scrolls, which the controller should handle, and returns the height it zoomed to.
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

    /// A camera whose height can't project, as one upside down, is refused; and the wheel zooms any
    /// height that projects to one that does, however near either end of the floats it is and
    /// however far it scrolls, while a scroll of NaN passes by.
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

    /// At the origin, unturned, looking down -Z with perspective.
    fn flying() -> FlyCameraController {
        FlyCameraController::new(Viewpoint {
            projection: Projection::Perspective {
                fov: 1.0,
                near: 0.1,
            },
            ..Viewpoint::default()
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
        assert!(moved.abs_diff_eq(Vec3::NEG_Z, 1e-6), "{moved}");
        assert_eq!(cx.viewpoint(), Some(controller.viewpoint));
        // A quarter turn to its left, and it goes along -X.
        controller.viewpoint.position = Vec3::ZERO;
        controller.viewpoint.rotation = Quat::from_rotation_y(FRAC_PI_2);
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(Vec3::NEG_X, 1e-6), "{moved}");
        // Looking down an eighth of a turn, E raises it straight up, as fast as W moves it.
        send(&mut controller, &mut cx, &[key(Key::KeyW, false)]);
        send(&mut controller, &mut cx, &[key(Key::KeyE, true)]);
        controller.viewpoint.position = Vec3::ZERO;
        controller.viewpoint.rotation = Quat::from_rotation_x(-FRAC_PI_2 / 2.0);
        controller.update(&mut cx, half);
        let moved = controller.viewpoint.position;
        assert!(moved.abs_diff_eq(Vec3::Y, 1e-6), "{moved}");
        // Both at once, unturned, no faster.
        send(&mut controller, &mut cx, &[key(Key::KeyW, true)]);
        controller.viewpoint.position = Vec3::ZERO;
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
        // Back into the window elsewhere, it looks from there, not from where it left.
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
