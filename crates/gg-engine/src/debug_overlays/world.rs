//! The World menu's overlays: lines and labels drawn with the world, over the rest, as colliders'
//! outlines are, so a capture shows them. Colliders, navigation and cells draw as the frame draws
//! them for the game and the look.
//!
//! Lines are [`WIDTH`] points wide and carry their entity's ID, so a pick of a line finds the
//! entity. Like colliders', they show except where an opaque mesh is nearer, so a skeleton inside
//! its mesh shows only where the mesh doesn't hide it. A shape holding the eye of a viewpoint with
//! perspective, such as a first-person player's own box, is left out: from inside it would span
//! the view.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::fmt::Write;

use gg_core::math::{DAffine3, DVec3, Mat4, Quat, Vec2, Vec3, Vec4, libm};
use gg_renderer::{Draws, Line, Meshes, Projection, Shaders, Viewpoint, Viewport};

use super::{Overlay, State};
use crate::audio::{Audio, HEARD};
use crate::camera::Camera;
use crate::draw::Poses;
use crate::ecs::node_id;
use crate::font::{Align, Fonts, TextStyle};
use crate::light::{Light, LightKind};
use crate::{
    CharacterController3D, Entity, EntityId, ModelRoot, Parent, RigidBody2D, RigidBody3D,
    Transform, World, bounds,
};

/// Contacts' color, linear: magenta, apart from colliders' green.
const CONTACTS: Vec4 = Vec4::new(1.0, 0.2, 0.8, 1.0);
/// Velocities' arrows, linear: yellow.
const VELOCITIES: Vec4 = Vec4::new(1.0, 0.95, 0.15, 1.0);
/// Angular velocities' arcs: a paler yellow than the arrows.
const SPINS: Vec4 = Vec4::new(1.0, 1.0, 0.6, 1.0);
/// Bounding boxes' color: violet.
const BOUNDS: Vec4 = Vec4::new(0.6, 0.4, 1.0, 1.0);
/// Axes' colors, X, Y and Z: red, green and blue, as the grid's axes and gizmos have them.
const AXES: [Vec4; 3] = [
    Vec4::new(1.0, 0.15, 0.15, 1.0),
    Vec4::new(0.2, 0.9, 0.2, 1.0),
    Vec4::new(0.25, 0.4, 1.0, 1.0),
];
/// Names' text color: white.
const NAMES: Vec4 = Vec4::new(1.0, 1.0, 1.0, 1.0);
/// Skeletons' color: cyan.
const SKELETONS: Vec4 = Vec4::new(0.1, 0.9, 0.9, 1.0);
/// Cameras' frusta color: light gray.
const CAMERAS: Vec4 = Vec4::new(0.8, 0.8, 0.8, 1.0);
/// Lights' color: gold.
const LIGHTS: Vec4 = Vec4::new(1.0, 0.8, 0.35, 1.0);
/// Sounds' color: rose.
const SOUNDS: Vec4 = Vec4::new(1.0, 0.45, 0.55, 1.0);

/// How wide lines draw, in points: pixels of a window at a scale of 1, as colliders' outlines.
const WIDTH: f32 = 2.0;
/// How far ahead a velocity looks, in seconds: the arrow and the arc reach as far as the body
/// goes in it.
const LOOK_AHEAD: f32 = 0.25;
/// The least turn an arc shows, in radians: less is a dot.
const MIN_SPIN: f32 = 0.02;
/// How far a cross reaches each way of its middle, in units, for lights and sounds.
const CROSS: f32 = 0.1;
/// How far a contact's cross reaches each way of its middle, and how long its normal's arrow is,
/// in points: the same size near and far.
const CONTACT_POINTS: f32 = 5.0;
const NORMAL_POINTS: f32 = 28.0;
/// An axis's least and most length, in units: half its entity's scale along it, between them.
const AXIS: [f32; 2] = [0.15, 2.0];
/// How tall names' text is, and how far above what its entity draws it sits, in points.
const NAME_POINTS: f32 = 12.0;
const NAME_GAP: f32 = 4.0;
/// How far a perspective camera's frustum reaches, in units: its projection has no far plane.
const FRUSTUM: f32 = 5.0;
/// How far in front of the viewpoint a directional light's arrow shows, in units, and how long it
/// is, in points: such a light is everywhere, so it shows where it's seen.
const AHEAD: f32 = 2.0;
const DIRECTION_POINTS: f32 = 80.0;
/// How far a spot light's cone reaches without a range, in units.
const SPOT: f32 = 3.0;
/// Lines in a circle: round to within a pixel up to some 500 pixels across.
const SEGMENTS: usize = 48;

/// What the frame has beyond the world, for the overlays that read it.
pub(crate) struct Frame<'a> {
    pub(crate) meshes: &'a Meshes,
    pub(crate) fonts: &'a mut Fonts,
    pub(crate) audio: &'a Audio,
    /// What the frame draws from, if it knows yet.
    pub(crate) viewpoint: Option<Viewpoint>,
    /// The camera the viewpoint comes from, if it does, which has no frustum to show.
    pub(crate) from: Option<EntityId>,
    /// Where the game's view was in the last frame handed off.
    pub(crate) view: Option<Viewport>,
}

/// What each overlay drew in a frame, lines and labels, by [`Overlay`] as a number.
pub(crate) type Drew = [u32; Overlay::ALL.len()];

/// Draws the World menu's overlays `state` has on, but those the frame draws itself: colliders,
/// navigation and cells. `scale` is the window's points to pixels. Returns what each drew.
///
/// - Bounds and Sounds draw flat where the viewpoint has no perspective (a 2D game's).
/// - Contacts, names and a directional light's arrow keep their size in points wherever they are.
/// - Bounds and names' places read where the latest tick left entities; the rest, where they draw.
pub(crate) fn draw(
    state: &State,
    world: &World,
    poses: &Poses<'_>,
    frame: &mut Frame<'_>,
    scale: f32,
    over: &mut Draws,
    shaders: &mut Shaders,
) -> Drew {
    let _zone = gg_core::zone!();
    let mut out = Out::new(over, shaders, poses.origin(), WIDTH * scale);
    let viewpoint = frame.viewpoint.unwrap_or_default();
    let view = frame.view.filter(|view| view.size[1] > 0);
    let flat = matches!(viewpoint.projection, Projection::Orthographic { .. });
    let along_z = (viewpoint.rotation * Vec3::NEG_Z).z.abs() > 0.999;
    let look = Look {
        viewpoint,
        flat,
        plane: (flat && along_z).then_some(Vec3::Z),
        eye: (!flat).then_some(viewpoint.position),
        aspect: view.map_or(16.0 / 9.0, |view| view.aspect()),
        pixels: view.map_or(1080, |view| view.size[1]) as f32,
        scale,
        from: frame.from,
    };
    if state.on(Overlay::Contacts) {
        out.drawing = Overlay::Contacts;
        contacts(world, &look, &mut out);
    }
    if state.on(Overlay::Bounds) {
        out.drawing = Overlay::Bounds;
        bounds(world, frame.meshes, &look, &mut out);
    }
    if state.on(Overlay::Skeletons) {
        out.drawing = Overlay::Skeletons;
        skeletons(world, poses, &mut out);
    }
    if state.on(Overlay::Sounds) {
        out.drawing = Overlay::Sounds;
        sounds(world, frame.audio, &look, &mut out);
    }
    let per_entity = [
        Overlay::Velocities,
        Overlay::Axes,
        Overlay::Names,
        Overlay::Cameras,
        Overlay::Lights,
    ];
    if !per_entity.into_iter().any(|overlay| state.on(overlay)) {
        return out.drew;
    }
    let mut text = String::new();
    each_posed(world, poses, |posed| {
        if state.on(Overlay::Velocities) {
            out.drawing = Overlay::Velocities;
            velocity(world, posed, &mut out);
        }
        if state.on(Overlay::Axes) {
            out.drawing = Overlay::Axes;
            axes(posed, &look, &mut out);
        }
        if state.on(Overlay::Cameras) {
            out.drawing = Overlay::Cameras;
            camera(world, poses, posed, &look, &mut out);
        }
        if state.on(Overlay::Lights) {
            out.drawing = Overlay::Lights;
            light(world, poses, posed, &look, &mut out);
        }
        if state.on(Overlay::Names) {
            out.drawing = Overlay::Names;
            text.clear();
            name(
                world,
                frame.meshes,
                posed,
                &look,
                &mut text,
                frame.fonts,
                &mut out,
            );
        }
    });
    out.drew
}

/// What the frame is seen from, as the overlays read it.
struct Look {
    viewpoint: Viewpoint,
    /// Whether the viewpoint has no perspective.
    flat: bool,
    /// The plane arrows' heads lie in, by its normal: Z's where the viewpoint looks along it with
    /// no perspective, as a 2D game's does; else none, and heads have four barbs.
    plane: Option<Vec3>,
    /// Where the viewpoint is, if it has perspective: what holds it isn't drawn.
    eye: Option<DVec3>,
    /// The view's width over its height.
    aspect: f32,
    /// The view's height in pixels.
    pixels: f32,
    /// The window's points to pixels.
    scale: f32,
    from: Option<EntityId>,
}

impl Look {
    /// How many units a point of the window spans at `at`, at its distance from the viewpoint.
    /// `None` at or behind a perspective viewpoint's near plane.
    fn per_point(&self, at: DVec3) -> Option<f32> {
        let viewpoint = &self.viewpoint;
        let per_pixel = match viewpoint.projection {
            Projection::Orthographic { height, .. } => height / self.pixels,
            Projection::Perspective { fov, near } => {
                let forward = (viewpoint.rotation * Vec3::NEG_Z).as_dvec3();
                let depth = (at - viewpoint.position).dot(forward) as f32;
                if depth.is_nan() || depth <= near {
                    return None;
                }
                2.0 * depth * libm::tanf(fov / 2.0) / self.pixels
            }
        };
        let per_point = per_pixel * self.scale;
        (per_point > 0.0 && per_point.is_finite()).then_some(per_point)
    }
}

/// An entity that has a transform, and where it draws.
struct Posed {
    entity: Entity,
    id: EntityId,
    table: usize,
    row: usize,
    /// From the frame's origin, as [`Poses::matrix`] has it.
    matrix: Mat4,
}

impl Posed {
    fn at(&self) -> Vec3 {
        self.matrix.w_axis.truncate()
    }

    /// What lines of this entity are picked as.
    fn pick(&self) -> u64 {
        self.id.get()
    }

    /// How far its arcs reach, in units: half its largest scale, so they read at its size, but
    /// not past what's seen or so small it's a dot.
    fn size(&self) -> f32 {
        let axes = [self.matrix.x_axis, self.matrix.y_axis, self.matrix.z_axis];
        let scale = axes.map(|axis| axis.truncate().length());
        (scale.into_iter().fold(0.0, f32::max) * 0.5).clamp(AXIS[0], AXIS[1])
    }
}

/// Gives `each` every entity of `world` with a transform, with where `poses` draws it.
fn each_posed(world: &World, poses: &Poses<'_>, mut each: impl FnMut(&Posed)) {
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    for (table, rows) in world.tables().iter().enumerate() {
        if !rows.has(transform) {
            continue;
        }
        for (row, &entity) in rows.entities().iter().enumerate() {
            let Some(id) = world.id(entity) else {
                continue;
            };
            let matrix = poses.matrix(table, row);
            each(&Posed {
                entity,
                id,
                table,
                row,
                matrix,
            });
        }
    }
}

/// Where lines go: the frame's over-draws, from its origin.
struct Out<'a> {
    draws: &'a mut Draws,
    shaders: &'a mut Shaders,
    origin: DVec3,
    width: f32,
    /// A unit circle in the XY plane, from +X counterclockwise and back.
    ring: [Vec2; SEGMENTS + 1],
    /// What each overlay drew so far, and the one drawing.
    drew: Drew,
    drawing: Overlay,
}

impl<'a> Out<'a> {
    fn new(draws: &'a mut Draws, shaders: &'a mut Shaders, origin: DVec3, width: f32) -> Self {
        let ring = std::array::from_fn(|i| {
            let (sin, cos) = libm::sincosf(TAU * i as f32 / SEGMENTS as f32);
            Vec2::new(cos, sin)
        });
        Self {
            draws,
            shaders,
            origin,
            width,
            ring,
            drew: [0; Overlay::ALL.len()],
            drawing: Overlay::Contacts,
        }
    }

    /// `position` of the world as the frame draws it: from the origin.
    fn at(&self, position: DVec3) -> Vec3 {
        (position - self.origin).as_vec3()
    }

    fn line(&mut self, from: Vec3, to: Vec3, color: Vec4, id: u64) {
        let line = Line {
            from,
            to,
            color,
            width: self.width,
        };
        (self.draws).push_line(self.shaders, Mat4::IDENTITY, line, id);
        self.drew[self.drawing as usize] += 1;
    }

    /// A line along each axis, `reach` each way of `at`.
    fn cross(&mut self, at: Vec3, reach: f32, color: Vec4, id: u64) {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.line(at - axis * reach, at + axis * reach, color, id);
        }
    }

    /// The circle round `center` in the plane of `x` and `y`, whose lengths are its radii.
    fn circle(&mut self, center: Vec3, x: Vec3, y: Vec3, color: Vec4, id: u64) {
        for i in 0..SEGMENTS {
            let [from, to] = [self.ring[i], self.ring[i + 1]];
            let at = |p: Vec2| center + x * p.x + y * p.y;
            self.line(at(from), at(to), color, id);
        }
    }

    /// The circles round a sphere: one in the XY plane if `flat`, else one in each plane.
    fn sphere(&mut self, center: Vec3, radius: f32, flat: bool, color: Vec4, id: u64) {
        let (x, y, z) = (Vec3::X * radius, Vec3::Y * radius, Vec3::Z * radius);
        self.circle(center, x, y, color, id);
        if !flat {
            self.circle(center, y, z, color, id);
            self.circle(center, z, x, color, id);
        }
    }

    /// The edges of the box from `low` to `high` that `placed` places, in the world's space: each
    /// once, so a flat box has four and a line one.
    fn cuboid(&mut self, placed: DAffine3, [low, high]: [DVec3; 2], color: Vec4, id: u64) {
        let corner = |bits: usize| {
            let pick = |bit: usize, axis: usize| match bits & bit {
                0 => low[axis],
                _ => high[axis],
            };
            self.at(placed.transform_point3(DVec3::new(pick(1, 0), pick(2, 1), pick(4, 2))))
        };
        let corners: [Vec3; 8] = std::array::from_fn(corner);
        // The axes it has no extent along, as bits: their edges are points, their faces one.
        let none = [1, 2, 4]
            .into_iter()
            .enumerate()
            .filter(|&(axis, _)| low[axis] == high[axis])
            .fold(0, |bits, (_, bit)| bits | bit);
        for bit in [1, 2, 4].into_iter().filter(|bit| none & bit == 0) {
            for bits in (0..8).filter(|bits| bits & (bit | none) == 0) {
                self.line(corners[bits], corners[bits | bit], color, id);
            }
        }
    }

    /// A line from `from` to `to` ending in a head, which is two barbs in the plane `plane` is
    /// the normal of, else four. Nothing for one too short to show.
    fn arrow(&mut self, from: Vec3, to: Vec3, plane: Option<Vec3>, color: Vec4, id: u64) {
        let length = (to - from).length();
        if !(length > 1e-4 && length.is_finite()) {
            return;
        }
        self.line(from, to, color, id);
        let head = (length * 0.2).min(0.5);
        self.head(to, (to - from) / length, head, plane, color, id);
    }

    /// The barbs of a head `size` long at `tip`, pointing along `direction`, a unit vector.
    fn head(
        &mut self,
        tip: Vec3,
        direction: Vec3,
        size: f32,
        plane: Option<Vec3>,
        color: Vec4,
        id: u64,
    ) {
        let sides = match plane {
            Some(normal) => [normal.cross(direction).normalize_or_zero(), Vec3::ZERO],
            None => direction.any_orthonormal_pair().into(),
        };
        for side in sides.into_iter().filter(|&side| side != Vec3::ZERO) {
            for sign in [1.0, -1.0] {
                let back = tip - direction * size + side * (sign * size * 0.5);
                self.line(tip, back, color, id);
            }
        }
    }

    /// An arc round `axis`, a unit vector, through the turn `angle` (counterclockwise seen from
    /// the axis's tip if positive), at `radius` from `center`, ending in a head. A turn of less
    /// than [`MIN_SPIN`] shows nothing.
    fn spin(&mut self, center: Vec3, axis: Vec3, angle: f32, radius: f32, color: Vec4, id: u64) {
        let angle = angle.clamp(-TAU, TAU);
        if angle.is_nan() || angle.abs() <= MIN_SPIN {
            return;
        }
        let start = axis.any_orthonormal_vector() * radius;
        let steps = ((angle.abs() / (PI / 12.0)).ceil() as usize).clamp(1, 24);
        let point = |i: usize| {
            let turn = Quat::from_axis_angle(axis, angle * i as f32 / steps as f32);
            center + turn * start
        };
        for i in 0..steps {
            self.line(point(i), point(i + 1), color, id);
        }
        let tip = point(steps);
        let radial = (tip - center).normalize_or_zero();
        let tangent = axis.cross(radial) * angle.signum();
        self.head(tip, tangent, radius * 0.35, Some(axis), color, id);
    }
}

/// Contacts: a cross at each point where colliders touch, characters among them, and an arrow
/// along its normal, [`CONTACT_POINTS`] and [`NORMAL_POINTS`] points long.
fn contacts(world: &World, look: &Look, out: &mut Out<'_>) {
    crate::physics::contact_points(world, &mut |entity, at, normal| {
        let Some(per_point) = look.per_point(at) else {
            return;
        };
        let id = entity
            .and_then(|entity| world.id(entity))
            .map_or(0, EntityId::get);
        let at = out.at(at);
        out.cross(at, CONTACT_POINTS * per_point, CONTACTS, id);
        let tip = at + normal.as_vec3() * (NORMAL_POINTS * per_point);
        out.arrow(at, tip, look.plane, CONTACTS, id);
    });
}

/// Velocities: an arrow from each body or character as far as it goes in [`LOOK_AHEAD`] seconds,
/// and an arc round a body as far as it turns, 2D's about Z.
fn velocity(world: &World, posed: &Posed, out: &mut Out<'_>) {
    let (at, id, radius) = (posed.at(), posed.pick(), posed.size());
    if let Some(body) = world.get::<RigidBody2D>(posed.entity) {
        let to = at + body.velocity.extend(0.0) * LOOK_AHEAD;
        out.arrow(at, to, Some(Vec3::Z), VELOCITIES, id);
        let angle = body.angular_velocity * LOOK_AHEAD;
        out.spin(at, Vec3::Z, angle, radius, SPINS, id);
    }
    if let Some(body) = world.get::<RigidBody3D>(posed.entity) {
        out.arrow(at, at + body.velocity * LOOK_AHEAD, None, VELOCITIES, id);
        let angle = body.angular_velocity.length() * LOOK_AHEAD;
        let axis = body.angular_velocity.normalize_or_zero();
        if axis != Vec3::ZERO {
            out.spin(at, axis, angle, radius, SPINS, id);
        }
    }
    if let Some(character) = world.get::<CharacterController3D>(posed.entity) {
        let to = at + character.velocity * LOOK_AHEAD;
        out.arrow(at, to, None, VELOCITIES, id);
    }
}

/// Bounds: the box round what each root draws, in its space ([`bounds::boxed`]), but one holding
/// the eye.
fn bounds(world: &World, meshes: &Meshes, look: &Look, out: &mut Out<'_>) {
    for (root, _) in world.query::<(Entity, &Transform)>() {
        if world.get::<Parent>(root).is_some() {
            continue;
        }
        let (Some((placed, corners)), Some(id)) =
            (bounds::boxed(world, meshes, root), world.id(root))
        else {
            continue;
        };
        let [low, high] = corners;
        let holds = |eye: DVec3| {
            let eye = placed.inverse().transform_point3(eye);
            eye.cmpge(low).all() && eye.cmple(high).all()
        };
        if !look.eye.is_some_and(holds) {
            out.cuboid(placed, corners, BOUNDS, id.get());
        }
    }
}

/// Axes: an arrow along each of an entity's axes as it draws, half its scale along it long,
/// within [`AXIS`], but the camera's the view sees from. A 2D game's view leaves out Z, which
/// points along it.
fn axes(posed: &Posed, look: &Look, out: &mut Out<'_>) {
    if look.from == Some(posed.id) {
        return;
    }
    let at = posed.at();
    let matrix = posed.matrix;
    let directions = [matrix.x_axis, matrix.y_axis, matrix.z_axis];
    let shown = if look.plane.is_some() { 2 } else { 3 };
    for (axis, color) in directions.into_iter().zip(AXES).take(shown) {
        let axis = axis.truncate();
        let length = (axis.length() * 0.5).clamp(AXIS[0], AXIS[1]);
        let direction = axis.normalize_or_zero();
        if direction != Vec3::ZERO {
            out.arrow(at, at + direction * length, look.plane, color, posed.pick());
        }
    }
}

/// Names: a root entity's name, else its ID, centered [`NAME_GAP`] points above what it and its
/// descendants draw, else above it, facing the viewpoint. Parts have none: a part's label would
/// land on its root's, as a unit's shape or a model's nodes would.
fn name(
    world: &World,
    meshes: &Meshes,
    posed: &Posed,
    look: &Look,
    text: &mut String,
    fonts: &mut Fonts,
    out: &mut Out<'_>,
) {
    if world.get::<Parent>(posed.entity).is_some() || look.from == Some(posed.id) {
        return;
    }
    let named = world.name(posed.entity);
    // How far above the entity's origin its box's top is, in the world's Y, where the latest
    // tick left them.
    let lift = bounds::boxed(world, meshes, posed.entity).map_or(0.0, |(placed, [low, high])| {
        let top = (0..8).map(|bits: usize| {
            let pick = |bit: usize, axis: usize| match bits & bit {
                0 => low[axis],
                _ => high[axis],
            };
            placed
                .transform_point3(DVec3::new(pick(1, 0), pick(2, 1), pick(4, 2)))
                .y
        });
        let top = top.fold(f64::NEG_INFINITY, f64::max);
        (top - placed.translation.y) as f32
    });
    let at = posed.at() + Vec3::Y * lift.max(0.0);
    let Some(per_point) = look.per_point(out.origin + at.as_dvec3()) else {
        return;
    };
    let at = at + Vec3::Y * (NAME_GAP * per_point);
    match named {
        Some(name) => text.push_str(name),
        None => {
            let _ = write!(text, "{}", posed.id);
        }
    }
    let style = TextStyle {
        font: None,
        size: NAME_POINTS * per_point,
        color: NAMES,
        align: [Align::Center, Align::End],
    };
    let placed = Mat4::from_rotation_translation(look.viewpoint.rotation, at);
    let glyphs = fonts.lay_out(placed, text, &style, posed.pick());
    out.draws.extend_glyphs(out.shaders, glyphs);
    out.drew[Overlay::Names as usize] += 1;
}

/// Skeletons: each joint of a skinned model joined to the joint above it, where they draw.
fn skeletons(world: &World, poses: &Poses<'_>, out: &mut Out<'_>) {
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    let assets = world.assets();
    let point = |entity: Entity| {
        let (table, row) = world.table_row(entity)?;
        let (table, row) = (table as usize, row as usize);
        let has = world.tables()[table].has(transform);
        has.then(|| poses.matrix(table, row).w_axis.truncate())
    };
    for (root, &ModelRoot(model)) in world.query::<(Entity, &ModelRoot)>() {
        let (Some(nodes), Some(id)) = (assets.nodes(model), world.id(root)) else {
            continue;
        };
        let part = |part: usize| node_id(id, part).and_then(|node| world.entity(node));
        for skin in nodes.skins.iter() {
            let joint = |part: usize| skin.joints.contains(&(part as u32));
            for &joint_part in &skin.joints {
                // The nearest ancestor that's a joint too: a node between them has no bone.
                let mut above = nodes.parts.get(joint_part as usize).and_then(|p| p.parent);
                while let Some(at) = above.filter(|&at| !joint(at)) {
                    above = nodes.parts.get(at).and_then(|p| p.parent);
                }
                let (Some(above), Some(entity)) = (above, part(joint_part as usize)) else {
                    continue;
                };
                let ends = (part(above).and_then(point), point(entity));
                if let (Some(from), Some(to), Some(id)) = (ends.0, ends.1, world.id(entity)) {
                    out.line(from, to, SKELETONS, id.get());
                }
            }
        }
    }
}

/// The corners of a frustum's near and far planes, in its camera's space (looking along -Z, +X
/// right), for a view `aspect` across to 1 up: each four around from the lower left.
///
/// A perspective one goes [`FRUSTUM`] units, or twice its near plane, if that's more.
fn frustum(projection: Projection, aspect: f32) -> [[Vec3; 4]; 2] {
    let (planes, half) = match projection {
        Projection::Orthographic { height, near, far } => ([near, far], [height / 2.0; 2]),
        Projection::Perspective { fov, near } => {
            let far = FRUSTUM.max(near * 2.0);
            let tan = libm::tanf(fov / 2.0);
            ([near, far], [near * tan, far * tan])
        }
    };
    [0, 1].map(|plane| {
        let (high, z) = (half[plane], -planes[plane]);
        let wide = high * aspect;
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(x, y)| Vec3::new(x * wide, y * high, z))
    })
}

/// Cameras: the frustum of each camera but the one the view sees from.
fn camera(world: &World, poses: &Poses<'_>, posed: &Posed, look: &Look, out: &mut Out<'_>) {
    let Some(camera) = world.get::<Camera>(posed.entity) else {
        return;
    };
    if look.from == Some(posed.id) || !camera.projection.projects() {
        return;
    }
    let (position, rotation) = poses.pose(posed.table, posed.row);
    let at = out.at(position);
    let [near, far] = frustum(camera.projection, look.aspect)
        .map(|corners| corners.map(|corner| at + rotation * corner));
    let id = posed.pick();
    for i in 0..4 {
        let next = (i + 1) % 4;
        out.line(near[i], near[next], CAMERAS, id);
        out.line(far[i], far[next], CAMERAS, id);
        out.line(near[i], far[i], CAMERAS, id);
    }
}

/// Lights: a cross at each; a directional light's arrow along its direction, [`AHEAD`] units in
/// front of the viewpoint; a point light's sphere of its range, but one holding the eye; a spot
/// light's cone to its range, or [`SPOT`] units without one.
fn light(world: &World, poses: &Poses<'_>, posed: &Posed, look: &Look, out: &mut Out<'_>) {
    let Some(light) = world.get::<Light>(posed.entity) else {
        return;
    };
    let (position, rotation) = poses.pose(posed.table, posed.row);
    let (at, id) = (out.at(position), posed.pick());
    let forward = rotation * Vec3::NEG_Z;
    let range = light
        .range
        .filter(|range| *range > 0.0 && range.is_finite());
    out.cross(at, CROSS, LIGHTS, id);
    match light.kind {
        LightKind::Directional => {
            let viewpoint = &look.viewpoint;
            let ahead = viewpoint.position + (viewpoint.rotation * Vec3::NEG_Z * AHEAD).as_dvec3();
            if let Some(per_point) = look.per_point(ahead) {
                let (middle, half) = (
                    out.at(ahead),
                    forward * (DIRECTION_POINTS * per_point / 2.0),
                );
                out.arrow(middle - half, middle + half, None, LIGHTS, id);
            }
        }
        LightKind::Point => {
            let holds = |eye: DVec3| eye.distance(position) <= f64::from(range.unwrap_or(0.0));
            if let Some(range) = range.filter(|_| !look.eye.is_some_and(holds)) {
                out.sphere(at, range, false, LIGHTS, id);
            }
        }
        LightKind::Spot => {
            let length = range.unwrap_or(SPOT);
            let (sin, cos) = libm::sincosf(light.outer_cone.clamp(0.0, FRAC_PI_2));
            let middle = at + forward * (length * cos);
            let [x, y] = [Vec3::X, Vec3::Y].map(|axis| rotation * axis * (length * sin));
            out.circle(middle, x, y, LIGHTS, id);
            for (a, b) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                out.line(at, middle + x * a + y * b, LIGHTS, id);
            }
            out.line(at, at + forward * length, LIGHTS, id);
        }
    }
}

/// Sounds: a cross where each sound playing from a place in the world is, and circles round it
/// as far as it's heard at full volume and at all, but those holding the eye.
fn sounds(world: &World, audio: &Audio, look: &Look, out: &mut Out<'_>) {
    audio.places(world, &mut |entity, at| {
        let id = entity
            .and_then(|entity| world.id(entity))
            .map_or(0, EntityId::get);
        let eye = look.eye.map(|eye| eye.distance(at) as f32);
        sound(out.at(at), id, look, eye, out);
    });
}

/// A sound's marks at `at`, picked as `id`, `eye` as far from it as the eye is, if it's known.
fn sound(at: Vec3, id: u64, look: &Look, eye: Option<f32>, out: &mut Out<'_>) {
    out.cross(at, CROSS, SOUNDS, id);
    for heard in HEARD
        .into_iter()
        .filter(|&heard| eye.is_none_or(|eye| eye > heard))
    {
        out.sphere(at, heard, look.flat, SOUNDS, id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gg_core::math::DVec3;
    use gg_renderer::Sprite;

    use super::*;
    use crate::animation::Skin;
    use crate::gltf::Part;
    use crate::model::Nodes;
    use crate::render_thread::Extracted;
    use crate::{
        BoxCollider2D, BoxCollider3D, CapsuleCollider3D, Context, Name, Shape, step_physics,
    };

    const VIEWPORT: Viewport = Viewport {
        origin: [0, 0],
        size: [64, 64],
    };

    /// How many draws a frame of `cx` has with only `overlay` on, or none.
    fn drawn(cx: &mut Context, overlay: Option<Overlay>) -> usize {
        cx.dev.overlays.all_off();
        if let Some(overlay) = overlay {
            cx.dev.overlays.set(overlay, true);
        }
        let mut frame = Extracted::default();
        cx.draw_world(1.0, 0.0, 0);
        cx.extract(&mut frame, VIEWPORT);
        frame.draws.len()
    }

    /// How many draws `overlay` adds to the world's: none while off, some while on.
    fn adds_lines(cx: &mut Context, overlay: Overlay) -> usize {
        let off = drawn(cx, None);
        let on = drawn(cx, Some(overlay));
        assert!(on > off, "{overlay:?} draws nothing: {on} draws of {off}");
        assert_eq!(drawn(cx, None), off, "{overlay:?} off draws as before");
        on - off
    }

    /// Spawns a camera of `projection` at `at`, which sees the frame.
    fn seen_from(cx: &mut Context, projection: Projection) {
        let camera = Camera {
            projection,
            primary: true,
        };
        cx.world_mut().spawn((Transform::IDENTITY, camera));
    }

    /// A transform `units` in front of a camera at the origin.
    fn ahead(units: f64) -> Transform {
        Transform {
            translation: DVec3::new(0.0, 0.0, -units),
            ..Transform::IDENTITY
        }
    }

    #[test]
    fn axes_draw_an_arrow_an_axis_and_the_api_says_how_many_lines() {
        let mut cx = Context::seeded(1);
        cx.dev.overlays.engine = true;
        cx.world_mut().spawn(Transform::IDENTITY);
        // The default viewpoint looks along Z without perspective: X and Y, two barbs each.
        assert_eq!(adds_lines(&mut cx, Overlay::Axes), 6);
        let report = cx.dev.overlays.report();
        let axes = report.overlays.iter().find(|o| o.name == "Axes");
        assert_eq!(axes.and_then(|axes| axes.drew), None, "off: nothing to say");
        drawn(&mut cx, Some(Overlay::Axes));
        let report = cx.dev.overlays.report();
        let drew = |name: &str| report.overlays.iter().find(|o| o.name == name)?.drew;
        assert_eq!((drew("Axes"), drew("Bounds")), (Some(6), None));
        // With perspective, Z too, and four barbs each; none for the camera seen from.
        seen_from(
            &mut cx,
            Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        );
        cx.world_mut().spawn(ahead(5.0));
        assert_eq!(adds_lines(&mut cx, Overlay::Axes), 2 * 15);
    }

    #[test]
    fn velocities_draw_arrows_and_arcs_from_bodies() {
        let mut cx = Context::seeded(1);
        let still = RigidBody2D::default();
        cx.world_mut().spawn((Transform::IDENTITY, still));
        assert_eq!(
            drawn(&mut cx, Some(Overlay::Velocities)),
            drawn(&mut cx, None)
        );
        let moving = RigidBody2D {
            velocity: Vec2::new(4.0, 0.0),
            angular_velocity: 2.0,
            ..still
        };
        cx.world_mut().spawn((Transform::IDENTITY, moving));
        // A shaft and two barbs; an arc of two lines and two barbs.
        assert_eq!(adds_lines(&mut cx, Overlay::Velocities), 7);
        let body = RigidBody3D {
            velocity: Vec3::new(0.0, 2.0, 0.0),
            ..RigidBody3D::default()
        };
        let before = drawn(&mut cx, Some(Overlay::Velocities));
        cx.world_mut().spawn((Transform::IDENTITY, body));
        // A shaft and four barbs.
        assert_eq!(drawn(&mut cx, Some(Overlay::Velocities)) - before, 5);
        let walking = CharacterController3D {
            velocity: Vec3::new(3.0, 0.0, 0.0),
            ..CharacterController3D::default()
        };
        let before = drawn(&mut cx, Some(Overlay::Velocities));
        cx.world_mut().spawn((Transform::IDENTITY, walking));
        assert_eq!(drawn(&mut cx, Some(Overlay::Velocities)) - before, 5);
    }

    #[test]
    fn contacts_draw_a_cross_and_a_normal_where_colliders_touch() {
        let mut cx = Context::seeded(1);
        let floor = Transform::IDENTITY;
        let body = Transform {
            translation: DVec3::new(0.0, 0.9, 0.0),
            ..Transform::IDENTITY
        };
        let floor = (
            floor,
            RigidBody2D {
                body_type: crate::BodyType::Static,
                ..RigidBody2D::default()
            },
            BoxCollider2D::default(),
        );
        cx.world_mut().spawn(floor);
        cx.world_mut()
            .spawn((body, RigidBody2D::default(), BoxCollider2D::default()));
        let (a, b) = (
            Transform::IDENTITY,
            Transform {
                translation: DVec3::new(10.0, 0.9, 0.0),
                ..Transform::IDENTITY
            },
        );
        let fixed = RigidBody3D {
            body_type: crate::BodyType::Static,
            ..RigidBody3D::default()
        };
        cx.world_mut().spawn((
            Transform {
                translation: DVec3::new(10.0, 0.0, 0.0),
                ..a
            },
            fixed,
            BoxCollider3D::default(),
        ));
        cx.world_mut()
            .spawn((b, RigidBody3D::default(), BoxCollider3D::default()));
        for _ in 0..2 {
            step_physics(cx.world_mut());
        }
        assert!(cx.world().contacts().count() >= 2, "both pairs touch");
        // At least a cross (three lines) and a normal each.
        assert!(adds_lines(&mut cx, Overlay::Contacts) >= 8);
    }

    #[test]
    fn bounds_draw_a_box_round_what_each_root_draws() {
        let mut cx = Context::seeded(1);
        cx.world_mut()
            .spawn((Transform::IDENTITY, Sprite::default()));
        // A sprite's box is flat: four edges.
        assert_eq!(adds_lines(&mut cx, Overlay::Bounds), 4);
        let cube = cx.shape(Shape::Cube);
        cube.spawn(cx.world_mut(), ahead(5.0));
        assert_eq!(adds_lines(&mut cx, Overlay::Bounds), 4 + 12);
    }

    #[test]
    fn boxes_and_outlines_holding_the_eye_are_left_out() {
        let mut cx = Context::seeded(1);
        seen_from(
            &mut cx,
            Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        );
        // A cube and a collider round the camera, as a first-person player's own.
        let cube = cx.shape(Shape::Cube);
        let round = cube.spawn(cx.world_mut(), Transform::IDENTITY);
        cx.world_mut()
            .insert(round, BoxCollider3D::default())
            .expect("alive");
        assert_eq!(drawn(&mut cx, Some(Overlay::Bounds)), drawn(&mut cx, None));
        assert_eq!(
            drawn(&mut cx, Some(Overlay::Colliders)),
            drawn(&mut cx, None)
        );
        let seen = cube.spawn(cx.world_mut(), ahead(5.0));
        cx.world_mut()
            .insert(seen, BoxCollider3D::default())
            .expect("alive");
        assert_eq!(adds_lines(&mut cx, Overlay::Bounds), 12);
        assert_eq!(adds_lines(&mut cx, Overlay::Colliders), 12);
    }

    #[test]
    fn names_label_roots_by_name_else_id_but_not_parts() {
        let mut cx = Context::seeded(1);
        let unit = cx
            .world_mut()
            .spawn((Transform::IDENTITY, Name("Unit 1".into())));
        let named = adds_lines(&mut cx, Overlay::Names);
        let root = cx.world_mut().spawn(Transform::IDENTITY);
        let both = adds_lines(&mut cx, Overlay::Names);
        assert!(both > named, "a root's ID");
        let part = cx.world_mut().spawn(Transform::IDENTITY);
        cx.world_mut().set_parent(part, Some(root)).expect("alive");
        assert_eq!(adds_lines(&mut cx, Overlay::Names), both, "a part has none");
        // A unit's shape, named, and its model's nodes, named in the model's file: on the unit's
        // label, so none.
        let mut shapes = crate::model::tests::imported();
        for node in &mut shapes.parts {
            node.name = Some("Cube".into());
        }
        let (model, _) = crate::model::tests::added(&mut cx, "m", shapes);
        let shape = model.spawn(cx.world_mut(), Transform::IDENTITY);
        let world = cx.world_mut();
        world
            .insert(shape, Name("Unit Shape".into()))
            .expect("alive");
        world.set_parent(shape, Some(unit)).expect("alive");
        let node = world.children(shape).expect("alive").next();
        assert_eq!(world.name(node.expect("a node")), Some("Cube"));
        assert_eq!(adds_lines(&mut cx, Overlay::Names), both, "parts have none");
    }

    #[test]
    fn contacts_show_where_a_character_stands() {
        let mut cx = Context::seeded(1);
        let floor = Transform {
            scale: Vec3::new(10.0, 1.0, 10.0),
            ..Transform::IDENTITY
        };
        let fixed = RigidBody3D {
            body_type: crate::BodyType::Static,
            ..RigidBody3D::default()
        };
        cx.world_mut()
            .spawn((floor, fixed, BoxCollider3D::default()));
        let standing = Transform {
            translation: DVec3::new(0.0, 1.6, 0.0),
            ..Transform::IDENTITY
        };
        let capsule = CapsuleCollider3D::default();
        // Walking: one standing still moves by nothing, touching nothing.
        let walking = CharacterController3D {
            velocity: Vec3::new(1.0, 0.0, 0.0),
            ..CharacterController3D::default()
        };
        cx.world_mut().spawn((standing, capsule, walking));
        for _ in 0..30 {
            step_physics(cx.world_mut());
        }
        assert_eq!(
            cx.world().contacts().count(),
            0,
            "rapier's leave characters out"
        );
        // A cross and an arrow with two barbs, in the default viewpoint's plane.
        assert!(adds_lines(&mut cx, Overlay::Contacts) >= 6);
    }

    #[test]
    fn cameras_draw_frusta_but_the_one_the_view_sees_from() {
        let mut cx = Context::seeded(1);
        seen_from(
            &mut cx,
            Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        );
        cx.draw_world(1.0, 0.0, 0);
        assert_eq!(drawn(&mut cx, Some(Overlay::Cameras)), drawn(&mut cx, None));
        let other = Camera {
            projection: Projection::Orthographic {
                height: 4.0,
                near: -1.0,
                far: 3.0,
            },
            primary: false,
        };
        cx.world_mut().spawn((Transform::IDENTITY, other));
        assert_eq!(adds_lines(&mut cx, Overlay::Cameras), 12);
    }

    #[test]
    fn lights_draw_arrows_spheres_and_cones() {
        let mut cx = Context::seeded(1);
        let directional = Light::default();
        cx.world_mut().spawn((Transform::IDENTITY, directional));
        // A cross, a shaft and four barbs.
        assert_eq!(adds_lines(&mut cx, Overlay::Lights), 3 + 5);
        let point = Light {
            kind: LightKind::Point,
            range: Some(2.0),
            ..directional
        };
        cx.world_mut().spawn((Transform::IDENTITY, point));
        assert_eq!(adds_lines(&mut cx, Overlay::Lights), 8 + 3 + 3 * SEGMENTS);
        let spot = Light {
            kind: LightKind::Spot,
            ..directional
        };
        cx.world_mut().spawn((Transform::IDENTITY, spot));
        assert_eq!(
            adds_lines(&mut cx, Overlay::Lights),
            8 + 3 + 3 * SEGMENTS + 3 + SEGMENTS + 5
        );
    }

    #[test]
    fn skeletons_join_each_joint_to_the_joint_above() {
        let mut cx = Context::seeded(1);
        let part = |parent| Part {
            name: None,
            transform: Transform::IDENTITY,
            parent,
            primitive: None,
            light: None,
            skin: None,
        };
        // Joints 0 and 2, with a node between them that isn't one, and a third under joint 2.
        let skin = Skin {
            joints: vec![0, 2, 3],
            inverse_binds: vec![Mat4::IDENTITY; 3],
        };
        let nodes = Nodes {
            parts: vec![part(None), part(Some(0)), part(Some(1)), part(Some(2))],
            skins: Arc::new(vec![skin]),
            ..Nodes::default()
        };
        let assets = cx.world_mut().assets_mut();
        let model = assets.add_model("rig".into(), "rig".into(), nodes);
        model.spawn(cx.world_mut(), Transform::IDENTITY);
        assert_eq!(adds_lines(&mut cx, Overlay::Skeletons), 2);
    }

    #[test]
    fn a_sound_draws_a_cross_and_circles_at_the_distances_it_is_heard() {
        let (mut draws, mut shaders) = (Draws::default(), Shaders::default());
        let mut out = Out::new(&mut draws, &mut shaders, DVec3::ZERO, 2.0);
        let viewpoint = Viewpoint::default();
        let look = |flat| Look {
            viewpoint,
            flat,
            plane: None,
            eye: None,
            aspect: 1.0,
            pixels: 64.0,
            scale: 1.0,
            from: None,
        };
        sound(Vec3::ONE, 7, &look(true), None, &mut out);
        assert_eq!(out.draws.len(), 3 + 2 * SEGMENTS);
        sound(Vec3::ONE, 7, &look(false), None, &mut out);
        assert_eq!(out.draws.len(), 2 * (3 + 2 * SEGMENTS) + 4 * SEGMENTS);
        // Heard where the eye is, a sound has no circle round it there.
        let before = out.draws.len();
        sound(Vec3::ONE, 7, &look(false), Some(HEARD[0] + 0.5), &mut out);
        assert_eq!(
            out.draws.len() - before,
            3 + 3 * SEGMENTS,
            "the eye is in the outer sphere"
        );
        assert_eq!(
            out.drew[Overlay::Contacts as usize] as usize,
            out.draws.len()
        );
    }

    #[test]
    fn frusta_span_the_view() {
        let ortho = Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 3.0,
        };
        let [near, far] = frustum(ortho, 2.0);
        assert_eq!(near[0], Vec3::new(-4.0, -2.0, 1.0));
        assert_eq!(far[2], Vec3::new(4.0, 2.0, -3.0));
        let tall = Projection::Perspective {
            fov: FRAC_PI_2,
            near: 1.0,
        };
        let [near, far] = frustum(tall, 1.0);
        assert!(
            (near[2] - Vec3::new(1.0, 1.0, -1.0)).length() < 1e-5,
            "{near:?}"
        );
        assert!(
            (far[2] - Vec3::new(5.0, 5.0, -5.0)).length() < 1e-4,
            "{far:?}"
        );
    }
}
