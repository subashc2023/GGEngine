//! The World menu's overlays: lines and labels drawn with the world, over the rest, as colliders'
//! outlines are, so a capture shows them. Colliders, navigation and cells draw as the frame draws
//! them for the game and the look.
//!
//! Lines are [`WIDTH`] points wide and carry their entity's ID, so a pick of a line finds the
//! entity. Like colliders', they show except where an opaque mesh is nearer, so a skeleton inside
//! its mesh shows only where the mesh doesn't hide it.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::fmt::Write;

use gg_core::math::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4, libm};
use gg_renderer::{Draws, Line, Meshes, Projection, Shaders, Viewpoint, Viewport};

use super::{Overlay, State};
use crate::audio::{Audio, HEARD};
use crate::camera::Camera;
use crate::draw::Poses;
use crate::ecs::node_id;
use crate::font::{Align, Fonts, TextStyle};
use crate::light::{Light, LightKind};
use crate::{
    Entity, EntityId, ModelRoot, Parent, RigidBody2D, RigidBody3D, Transform, World, bounds,
};

/// Contacts' color, linear: magenta, apart from colliders' green.
const CONTACTS: Vec4 = Vec4::new(1.0, 0.2, 0.8, 1.0);
/// Velocities' arrows, linear: yellow.
const VELOCITIES: Vec4 = Vec4::new(1.0, 0.95, 0.15, 1.0);
/// Angular velocities' arcs: a paler yellow than the arrows.
const SPINS: Vec4 = Vec4::new(1.0, 1.0, 0.6, 1.0);
/// Bounding spheres' color: violet.
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
/// How far a cross reaches each way of its middle, in units, for contacts and markers.
const CROSS: f32 = 0.1;
/// How long a contact's normal is, in units.
const NORMAL: f32 = 0.4;
/// How tall names' text is, in points.
const NAME_POINTS: f32 = 12.0;
/// How far a perspective camera's frustum reaches, in units: its projection has no far plane.
const FRUSTUM: f32 = 5.0;
/// How long a directional light's arrow is, in units.
const DIRECTION: f32 = 2.0;
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

/// Draws the World menu's overlays `state` has on, but those the frame draws itself: colliders,
/// navigation and cells. `scale` is the window's points to pixels.
///
/// - Bounds and Sounds draw one circle in the XY plane where the viewpoint has no perspective
///   (a 2D game's), else three round the sphere.
/// - Names face the viewpoint and stay about [`NAME_POINTS`] tall wherever it is.
/// - Bounds read where the latest tick left entities; the rest, where they draw.
pub(crate) fn draw(
    state: &State,
    world: &World,
    poses: &Poses<'_>,
    frame: &mut Frame<'_>,
    scale: f32,
    over: &mut Draws,
    shaders: &mut Shaders,
) {
    let _zone = gg_core::zone!();
    let mut out = Out::new(over, shaders, poses.origin(), WIDTH * scale);
    let viewpoint = frame.viewpoint.unwrap_or_default();
    let view = frame.view.filter(|view| view.size[1] > 0);
    let look = Look {
        viewpoint,
        flat: matches!(viewpoint.projection, Projection::Orthographic { .. }),
        aspect: view.map_or(16.0 / 9.0, |view| view.aspect()),
        pixels: view.map_or(1080, |view| view.size[1]) as f32,
        scale,
        from: frame.from,
    };
    if state.on(Overlay::Contacts) {
        contacts(world, &mut out);
    }
    if state.on(Overlay::Bounds) {
        bounds(world, frame.meshes, &look, &mut out);
    }
    if state.on(Overlay::Skeletons) {
        skeletons(world, poses, &mut out);
    }
    if state.on(Overlay::Sounds) {
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
        return;
    }
    let mut text = String::new();
    each_posed(world, poses, |posed| {
        if state.on(Overlay::Velocities) {
            velocity(world, posed, &mut out);
        }
        if state.on(Overlay::Axes) {
            axes(posed, &mut out);
        }
        if state.on(Overlay::Cameras) {
            camera(world, poses, posed, &look, &mut out);
        }
        if state.on(Overlay::Lights) {
            light(world, poses, posed, &mut out);
        }
        if state.on(Overlay::Names) {
            text.clear();
            name(world, posed, &look, &mut text, frame.fonts, &mut out);
        }
    });
}

/// What the frame is seen from, as the overlays read it.
struct Look {
    viewpoint: Viewpoint,
    /// Whether the viewpoint has no perspective.
    flat: bool,
    /// The view's width over its height.
    aspect: f32,
    /// The view's height in pixels.
    pixels: f32,
    /// The window's points to pixels.
    scale: f32,
    from: Option<EntityId>,
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

    /// How far its overlays reach, in units: half its largest scale, so they read at its size,
    /// but not past what's seen or so small it's a dot.
    fn size(&self) -> f32 {
        let axes = [self.matrix.x_axis, self.matrix.y_axis, self.matrix.z_axis];
        let scale = axes.map(|axis| axis.truncate().length());
        (scale.into_iter().fold(0.0, f32::max) * 0.5).clamp(0.15, 2.0)
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

/// Contacts: a cross at each point where colliders touch, and a line along its normal.
fn contacts(world: &World, out: &mut Out<'_>) {
    crate::physics::contact_points(world, &mut |entity, at, normal| {
        let id = entity
            .and_then(|entity| world.id(entity))
            .map_or(0, EntityId::get);
        let at = out.at(at);
        out.cross(at, CROSS, CONTACTS, id);
        out.line(at, at + normal.as_vec3() * NORMAL, CONTACTS, id);
    });
}

/// Velocities: an arrow from each body as far as it goes in [`LOOK_AHEAD`] seconds, and an arc
/// round it as far as it turns, 2D's about Z.
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
}

/// Bounds: each root's bounding sphere, as `meshes` have it.
fn bounds(world: &World, meshes: &Meshes, look: &Look, out: &mut Out<'_>) {
    for (root, _) in world.query::<(Entity, &Transform)>() {
        if world.get::<Parent>(root).is_some() {
            continue;
        }
        let (Some((center, radius)), Some(id)) = (bounds::of(world, meshes, root), world.id(root))
        else {
            continue;
        };
        let center = out.at(center);
        out.sphere(center, radius as f32, look.flat, BOUNDS, id.get());
    }
}

/// Axes: a line along each of an entity's axes as it draws, [`Posed::size`] long.
fn axes(posed: &Posed, out: &mut Out<'_>) {
    let (at, size) = (posed.at(), posed.size());
    let matrix = posed.matrix;
    let directions = [matrix.x_axis, matrix.y_axis, matrix.z_axis];
    for (direction, color) in directions.into_iter().zip(AXES) {
        let direction = direction.truncate().normalize_or_zero();
        if direction != Vec3::ZERO {
            out.line(at, at + direction * size, color, posed.pick());
        }
    }
}

/// Names: each entity's name, else its ID, above where it draws, facing the viewpoint.
fn name(
    world: &World,
    posed: &Posed,
    look: &Look,
    text: &mut String,
    fonts: &mut Fonts,
    out: &mut Out<'_>,
) {
    let viewpoint = &look.viewpoint;
    let at = posed.at();
    let forward = (viewpoint.rotation * Vec3::NEG_Z).as_dvec3();
    let depth = ((out.origin + at.as_dvec3() - viewpoint.position).dot(forward)) as f32;
    let per_pixel = match viewpoint.projection {
        Projection::Orthographic { height, .. } => height / look.pixels,
        Projection::Perspective { fov, near } => {
            if depth.is_nan() || depth <= near {
                return;
            }
            2.0 * depth * libm::tanf(fov / 2.0) / look.pixels
        }
    };
    if !(per_pixel > 0.0 && per_pixel.is_finite()) {
        return;
    }
    match world.name(posed.entity) {
        Some(name) => text.push_str(name),
        None => {
            let _ = write!(text, "{}", posed.id);
        }
    }
    let style = TextStyle {
        font: None,
        size: NAME_POINTS * look.scale * per_pixel,
        color: NAMES,
        align: [Align::Center, Align::End],
    };
    let placed = Mat4::from_rotation_translation(viewpoint.rotation, at);
    let glyphs = fonts.lay_out(placed, text, &style, posed.pick());
    out.draws.extend_glyphs(out.shaders, glyphs);
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

/// Lights: a directional light's arrow along its direction, a point light's sphere of its range,
/// a spot light's cone to its range, or [`SPOT`] units without one; and a cross at each.
fn light(world: &World, poses: &Poses<'_>, posed: &Posed, out: &mut Out<'_>) {
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
        LightKind::Directional => out.arrow(at, at + forward * DIRECTION, None, LIGHTS, id),
        LightKind::Point => {
            if let Some(range) = range {
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
/// as far as it's heard at full volume and at all, where the viewpoint has perspective.
fn sounds(world: &World, audio: &Audio, look: &Look, out: &mut Out<'_>) {
    audio.places(world, &mut |entity, at| {
        let id = entity
            .and_then(|entity| world.id(entity))
            .map_or(0, EntityId::get);
        sound(out.at(at), id, look, out);
    });
}

/// A sound's marks at `at`, picked as `id`.
fn sound(at: Vec3, id: u64, look: &Look, out: &mut Out<'_>) {
    out.cross(at, CROSS, SOUNDS, id);
    for heard in HEARD {
        out.sphere(at, heard, look.flat, SOUNDS, id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gg_core::math::DVec3;
    use gg_renderer::{Circle, Sprite};

    use super::*;
    use crate::animation::Skin;
    use crate::gltf::Part;
    use crate::model::Nodes;
    use crate::render_thread::Extracted;
    use crate::{BoxCollider2D, BoxCollider3D, Context, Name, step_physics};

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

    #[test]
    fn axes_draw_three_lines_an_entity() {
        let mut cx = Context::seeded(1);
        cx.world_mut().spawn(Transform::IDENTITY);
        assert_eq!(adds_lines(&mut cx, Overlay::Axes), 3);
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
    fn bounds_draw_circles_round_what_draws() {
        let mut cx = Context::seeded(1);
        cx.world_mut()
            .spawn((Transform::IDENTITY, Sprite::default()));
        // The default viewpoint has no perspective: one circle.
        assert_eq!(adds_lines(&mut cx, Overlay::Bounds), SEGMENTS);
    }

    #[test]
    fn bounds_draw_three_circles_where_the_view_has_perspective() {
        let mut cx = Context::seeded(1);
        cx.world_mut()
            .spawn((Transform::IDENTITY, Circle::default()));
        seen_from(
            &mut cx,
            Projection::Perspective {
                fov: 0.8,
                near: 0.1,
            },
        );
        assert_eq!(adds_lines(&mut cx, Overlay::Bounds), 3 * SEGMENTS);
    }

    #[test]
    fn names_draw_text_for_named_entities_and_ids_for_the_rest() {
        let mut cx = Context::seeded(1);
        cx.world_mut()
            .spawn((Transform::IDENTITY, Name("hero".into())));
        let named = adds_lines(&mut cx, Overlay::Names);
        cx.world_mut().spawn(Transform::IDENTITY);
        assert!(adds_lines(&mut cx, Overlay::Names) > named);
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
            aspect: 1.0,
            pixels: 64.0,
            scale: 1.0,
            from: None,
        };
        sound(Vec3::ONE, 7, &look(true), &mut out);
        assert_eq!(out.draws.len(), 3 + 2 * SEGMENTS);
        sound(Vec3::ONE, 7, &look(false), &mut out);
        assert_eq!(out.draws.len(), 2 * (3 + 2 * SEGMENTS) + 4 * SEGMENTS);
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
