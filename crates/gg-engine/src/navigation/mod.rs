//! Navigation: a navmesh baked from the world's static 3D colliders ([`NavMeshVolume`]), paths
//! found over it ([`World::find_path`]), and agents that walk them, steering round each other
//! ([`NavAgent`]). [`step_navigation`] steps it. Its state is the agents' components and the
//! world's [`Navigation`]: snapshots copy it, and a run restored from a snapshot file bakes again.
//! The navmesh's polygons and the agents' paths draw as lines.
//!
//! Bits are the same on every platform, as the reference hash in `tests.rs` checks: plain `f32`
//! arithmetic and square roots, libm's cosine and sine, comparisons instead of `min` and `max`
//! (their zeros' signs are the platform's), and everything ordered by entity IDs.

mod agents;
mod avoid;
mod bake;
mod mesh;
mod path;
#[cfg(test)]
mod tests;

use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;
use std::time::Instant;

use gg_core::math::{Mat4, Quat, Vec3, Vec4};
use gg_renderer::{Draws, Line, Shaders};
use tracing::{info, warn};

use crate::draw::Poses;
use crate::ecs::mix64;
use crate::model::{ModelRoot, Surface};
use crate::{
    BodyType, BoxCollider3D, CapsuleCollider3D, Component, Entity, EntityId, MeshCollider3D,
    RigidBody3D, SphereCollider3D, Transform, World,
};
use agents::Crowd;
use bake::{Settings, Shape, bounds};
use mesh::NavMesh;
use path::Search;

/// What the world's navmesh is baked from, for what size of agent: the static 3D colliders inside a
/// box around its entity's origin.
///
/// The box is in its entity's space and moves, turns and scales with it; the bake uses the box
/// bounding it along the world's axes. Colliders are as physics has them.
///
/// Surfaces are voxelized into cells `cell_size` across and `cell_height` tall. Cells are kept
/// where an agent can stand: a surface no steeper than `max_slope`, `agent_height` of room above,
/// neighbors joined where the step is at most `agent_climb`. The area is eroded by `agent_radius`,
/// so a path keeps an agent's middle that far from walls and edges. Finer cells find narrower
/// ways and bake slower.
///
/// A volume of more than 4,194,304 columns or 65,533 levels isn't baked (`navigation.too_large`),
/// nor one without a size (`navigation.bad_volume`). The world bakes one navmesh, from the volume
/// of the least ID, and warns of the others (`navigation.volumes`).
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct NavMeshVolume {
    /// Width, height and depth of its box.
    #[reflect(min = 0.0, default = Vec3::new(20.0, 8.0, 20.0))]
    pub size: Vec3,
    /// How far from walls and edges an agent's middle stays, in units.
    #[reflect(min = 0.0, default = 0.5)]
    pub agent_radius: f32,
    /// Room above a floor an agent needs to stand there, in units.
    #[reflect(min = 0.0, default = 2.0)]
    pub agent_height: f32,
    /// The highest step an agent climbs, in units.
    #[reflect(min = 0.0, default = 0.3)]
    pub agent_climb: f32,
    /// The steepest slope an agent climbs, from level; steeper ones aren't walked.
    #[reflect(degrees, min = 0.0, default = FRAC_PI_4)]
    pub max_slope: f32,
    /// Width and depth of the heightfield's cells, in units.
    #[reflect(min = 0.01, default = 0.25)]
    pub cell_size: f32,
    /// Height of the heightfield's cells, in units: floor heights are its multiples.
    #[reflect(min = 0.01, default = 0.1)]
    pub cell_height: f32,
}

impl Default for NavMeshVolume {
    fn default() -> Self {
        Self {
            size: Vec3::new(20.0, 8.0, 20.0),
            agent_radius: 0.5,
            agent_height: 2.0,
            agent_climb: 0.3,
            max_slope: FRAC_PI_4,
            cell_size: 0.25,
            cell_height: 0.1,
        }
    }
}

/// An agent that walks the world's navmesh to its `destination`, as [`step_navigation`] steps it.
///
/// - It plans its path from its [`Transform`] when its destination or the navmesh changes, and
///   each tick while it has a destination and no path (as while falling onto the navmesh). Both
///   ends snap to the navmesh within the agent's height of the volume.
/// - It heads for the next corner, passing each once within its radius of it on its floor, or
///   once the next is in sight over the navmesh. Coming over a corner on another floor, it plans
///   again.
/// - It slows to stop within `stopping_distance` of the last corner, on its floor, and returns if
///   pushed off.
/// - It steers round near agents by ORCA, each giving way half as much as two need.
///
/// Its velocity, across, goes to its [`CharacterController3D`](crate::CharacterController3D),
/// which slides it along walls and up steps and slopes; else its [`RigidBody3D`] (gravity is
/// physics'); else its transform, moved along the navmesh, its height following the floor. An
/// agent under a parent isn't moved (`navigation.child_agent`). Without a [`NavMeshVolume`] it
/// stands (`navigation.no_volume`). Its rotation is the game's to set, as from `velocity`.
///
/// Its path is simulation state, so snapshots, replays and peers go alike. The steps write
/// `corners`, `corner`, `planned`, `arrived` and `velocity`; code, scripts and tools read them
/// and steer the agent by setting its destination, speed and the rest.
#[derive(Debug, PartialEq, Component)]
pub struct NavAgent {
    /// Where it's going, a point on or near the navmesh; none to stand where it is.
    pub destination: Option<Vec3>,
    /// How fast it walks, in units a second.
    #[reflect(min = 0.0, default = 3.5)]
    pub speed: f32,
    /// How far round it other agents keep, in units; also how near a corner of its path it comes
    /// before heading for the next.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// How near its destination, across, counts as there.
    #[reflect(min = 0.0, default = 0.1)]
    pub stopping_distance: f32,
    /// Whether it was within its stopping distance of its destination at the last step.
    pub arrived: bool,
    /// The velocity it chose at the last step, across, in units a second.
    pub velocity: Vec3,
    /// The corners of its path, points on the navmesh, the last its destination snapped to it;
    /// none without a destination or a path to it.
    pub corners: Vec<Vec3>,
    /// The index in `corners` it heads for.
    pub corner: u32,
    /// What its path was planned from: the navmesh's fingerprint mixed with its destination. It
    /// plans again when either changes; 0 for none.
    pub planned: u64,
}

impl Default for NavAgent {
    fn default() -> Self {
        Self {
            destination: None,
            speed: 3.5,
            radius: 0.5,
            stopping_distance: 0.1,
            arrived: false,
            velocity: Vec3::ZERO,
            corners: Vec::new(),
            corner: 0,
            planned: 0,
        }
    }
}

impl Clone for NavAgent {
    fn clone(&self) -> Self {
        let mut agent = Self {
            corners: Vec::new(),
            ..*self
        };
        agent.corners.clone_from(&self.corners);
        agent
    }

    /// Copies `source` into this one's memory, its corners into its vector, so a snapshot into a
    /// kept world allocates nothing unless its path grew.
    fn clone_from(&mut self, source: &Self) {
        let corners = std::mem::take(&mut self.corners);
        *self = Self { corners, ..*source };
        self.corners.clone_from(&source.corners);
    }
}

/// The world's navigation, a [`Resource`](crate::Resource) [`step_navigation`] adds at a first step.
///
/// A world with a [`NavMeshVolume`] or a [`NavAgent`] gets it. It holds the navmesh and a
/// fingerprint of what it was baked from: the volume and each static body's transform and
/// colliders, by entity ID. A changed fingerprint bakes again; agents' paths are planned on it.
///
/// Snapshots share the navmesh with their world. Snapshot files don't hold it: the same entities
/// and assets bake the same navmesh, bit for bit, so a restored run bakes it at its first step,
/// under the fingerprint its agents planned on.
#[derive(Default)]
pub struct Navigation {
    mesh: Option<Arc<NavMesh>>,
    fingerprint: u64,
    /// The entity of the volume the navmesh was baked from; its lines are picked as it.
    volume: Option<EntityId>,
    /// The surfaces of the models mesh colliders were made of, as last seen, each with its
    /// triangles' hash, which the fingerprint takes.
    surfaces: Vec<(Arc<Vec<Surface>>, u64, bool)>,
    /// The agents under a parent, warned of once each.
    children: Vec<Entity>,
    /// Whether it warned of destinations with no volume since it last had one, and of several
    /// volumes.
    alone: bool,
    volumes: bool,
    /// What searches and agents' steps work in. Nothing carries over, so a copy makes them anew.
    search: Search,
    crowd: Crowd,
}

impl Clone for Navigation {
    fn clone(&self) -> Self {
        Self {
            mesh: self.mesh.clone(),
            fingerprint: self.fingerprint,
            volume: self.volume,
            surfaces: self.surfaces.clone(),
            children: self.children.clone(),
            alone: self.alone,
            volumes: self.volumes,
            search: Search::default(),
            crowd: Crowd::default(),
        }
    }

    /// Copies `source` into this one's memory, keeping its own workspaces.
    fn clone_from(&mut self, source: &Self) {
        self.mesh.clone_from(&source.mesh);
        self.fingerprint = source.fingerprint;
        self.volume = source.volume;
        self.surfaces.clone_from(&source.surfaces);
        self.children.clone_from(&source.children);
        self.alone = source.alone;
        self.volumes = source.volumes;
    }
}

/// Steps `world`'s navigation a tick: updates the navmesh, then each [`NavAgent`] plans and steers.
///
/// Each agent heads for its next corner and steers round the rest. A layer's tick runs it after what sets
/// destinations (such as scripts) and before physics, which moves characters as their agents set.
pub fn step_navigation(world: &mut World) {
    let _zone = gg_core::zone!();
    refresh(world);
    world.resource_scope(|world, navigation: &mut Navigation| agents::step(navigation, world));
}

/// Brings `world`'s navmesh up to date: bakes again if what it's baked from changed, logging
/// `Navmesh baked` with its polygons and the time taken. A world restored from a snapshot file has
/// no navmesh yet; one loaded whole may keep another world's. Gives the world a [`Navigation`] if
/// it has none and has a [`NavMeshVolume`] or a [`NavAgent`].
///
/// Searches and steps go through it, so a path is planned on what the static colliders give now,
/// whichever came first after a load: peers that loaded at different times must agree. So must a
/// client's copy of the server's world, which holds the resources the client's does.
pub(crate) fn refresh(world: &mut World) {
    if world.resource::<Navigation>().is_none() {
        let volumes = world.query::<&NavMeshVolume>().next().is_some();
        if !volumes && world.query::<&NavAgent>().next().is_none() {
            return;
        }
        world.insert_resource(Navigation::default());
    }
    world.resource_scope(|world, navigation: &mut Navigation| navigation.refresh(world));
}

impl Navigation {
    /// Bakes again where what the navmesh is baked from changed.
    fn refresh(&mut self, world: &World) {
        let volume = self.volume_of(world);
        let fingerprint = volume.map_or(0, |volume| self.fingerprint(world, volume));
        if fingerprint != self.fingerprint {
            self.fingerprint = fingerprint;
            self.volume = volume.and_then(|volume| world.id(volume));
            self.mesh = volume.and_then(|volume| bake_from(world, volume).map(Arc::new));
            if let Some(mesh) = &self.mesh {
                self.search.fit(mesh);
            }
        }
    }

    /// The world's volume, the one of the least ID; warns once while there's another.
    fn volume_of(&mut self, world: &World) -> Option<Entity> {
        let mut volumes = world.query::<(Entity, &NavMeshVolume)>();
        let first = volumes.next().map(|(entity, _)| entity)?;
        let id = |entity| world.id(entity).expect("found alive");
        let mut chosen = (id(first), first);
        let mut count = 1;
        for (entity, _) in volumes {
            count += 1;
            if id(entity) < chosen.0 {
                chosen = (id(entity), entity);
            }
        }
        if count > 1 && !self.volumes {
            let id = chosen.0;
            warn!(
                code = "navigation.volumes",
                entity = %id,
                "The world has {count} NavMeshVolumes: its navmesh is baked from entity {id}'s \
                 alone, the least ID's"
            );
        }
        self.volumes = count > 1;
        Some(chosen.1)
    }

    /// The fingerprint of what the navmesh is baked from: the volume's ID, pose and fields, and
    /// each static body's ID, transform and colliders (a mesh collider by its model's triangles'
    /// hash). Bodies' hashes are summed, so find order counts for nothing. Never 0 (no navmesh).
    fn fingerprint(&mut self, world: &World, volume: Entity) -> u64 {
        let floats = |mut hash: u64, floats: &[f32]| {
            for float in floats {
                hash = mix64(hash ^ u64::from(float.to_bits()));
            }
            hash
        };
        let id = |entity| world.id(entity).map_or(0, EntityId::get);
        let shape = world.get::<NavMeshVolume>(volume).expect("found with one");
        let placed = world
            .pose(volume)
            .map_or(Mat4::IDENTITY, |pose| Mat4::from(pose.matrix.as_affine3a()));
        let mut volume_hash = floats(mix64(id(volume)), &placed.to_cols_array());
        volume_hash = floats(volume_hash, &shape.size.to_array());
        volume_hash = floats(
            volume_hash,
            &[
                shape.agent_radius,
                shape.agent_height,
                shape.agent_climb,
                shape.max_slope,
                shape.cell_size,
                shape.cell_height,
            ],
        );
        let mut statics = 0u64;
        for (entity, body, transform) in world.query::<(Entity, &RigidBody3D, &Transform)>() {
            if body.body_type != BodyType::Static || !matches!(world.parent(entity), Ok(None)) {
                continue;
            }
            let Transform {
                translation,
                rotation,
                scale,
            } = *transform;
            let mut hash = floats(mix64(id(entity)), &translation.as_vec3().to_array());
            hash = floats(floats(hash, &rotation.to_array()), &scale.to_array());
            if let Some(boxed) = world.get::<BoxCollider3D>(entity) {
                hash = floats(mix64(hash ^ 1), &boxed.offset.to_array());
                hash = floats(hash, &boxed.size.to_array());
            }
            if let Some(sphere) = world.get::<SphereCollider3D>(entity) {
                hash = floats(mix64(hash ^ 2), &sphere.offset.to_array());
                hash = floats(hash, &[sphere.radius]);
            }
            if let Some(capsule) = world.get::<CapsuleCollider3D>(entity) {
                hash = floats(mix64(hash ^ 3), &capsule.offset.to_array());
                hash = floats(hash, &[capsule.height, capsule.radius]);
            }
            if world.get::<MeshCollider3D>(entity).is_some() {
                hash = mix64(mix64(hash ^ 4) ^ self.surface_hash(world, entity));
            }
            statics = statics.wrapping_add(mix64(hash));
        }
        self.surfaces
            .retain_mut(|(_, _, seen)| std::mem::take(seen));
        mix64(volume_hash ^ statics).max(1)
    }

    /// The hash of the triangles of `entity`'s [`ModelRoot`]'s model, as they are now, hashed once
    /// per read of the surfaces; 0 for none.
    fn surface_hash(&mut self, world: &World, entity: Entity) -> u64 {
        let model = world.get::<ModelRoot>(entity).map(|root| root.0);
        let Some(nodes) = model.and_then(|model| world.assets().nodes(model)) else {
            return 0;
        };
        let same = |(surfaces, ..): &&mut (Arc<Vec<Surface>>, u64, bool)| {
            Arc::ptr_eq(surfaces, &nodes.surfaces)
        };
        if let Some((_, hash, seen)) = self.surfaces.iter_mut().find(same) {
            *seen = true;
            return *hash;
        }
        let (vertices, triangles) = nodes.triangles();
        let mut hash = mix64(triangles.len() as u64);
        for vertex in triangles
            .iter()
            .flatten()
            .map(|&index| vertices[index as usize])
        {
            for coordinate in vertex.to_array() {
                hash = mix64(hash ^ u64::from(coordinate.to_bits()));
            }
        }
        self.surfaces
            .push((Arc::clone(&nodes.surfaces), hash, true));
        hash
    }
}

/// `rotation` normalized as physics does bodies', so shapes keep their sizes; identity if it has
/// no length.
fn unit(rotation: Quat) -> Quat {
    let unit = Vec4::from(rotation).try_normalize();
    unit.map_or(Quat::IDENTITY, Quat::from_vec4)
}

/// Bakes the navmesh of `world`'s static colliders inside `volume`'s box, bodies in entity ID
/// order, so the same entities bake the same navmesh whatever order queries find them in. Logs
/// the bake, or warns of a volume it can't bake.
fn bake_from(world: &World, volume: Entity) -> Option<NavMesh> {
    let _zone = gg_core::zone!();
    let started = Instant::now();
    let id = world.id(volume).expect("found alive");
    let shape = *world.get::<NavMeshVolume>(volume).expect("found with one");
    // Navigation is in `f32`, as precise as that within some 16 km of the origin.
    let placed = world
        .pose(volume)
        .map_or(Mat4::IDENTITY, |pose| Mat4::from(pose.matrix.as_affine3a()));
    let half = shape.size.abs() * 0.5;
    let corners = (0..8).map(|bits: u32| {
        let sign = |bit: u32| if bits & bit == 0 { -1.0 } else { 1.0 };
        placed.transform_point3(half * Vec3::new(sign(1), sign(2), sign(4)))
    });
    let (min, max) = corners
        .fold(None, |held: Option<(Vec3, Vec3)>, at| {
            Some(held.map_or((at, at), |(min, max)| {
                (bounds(min, at).0, bounds(max, at).1)
            }))
        })
        .expect("a box has corners");
    let settings = Settings {
        min,
        max,
        cell_size: shape.cell_size,
        cell_height: shape.cell_height,
        radius: shape.agent_radius,
        height: shape.agent_height,
        climb: shape.agent_climb,
        max_slope: shape.max_slope,
    };
    let mut statics: Vec<(EntityId, Entity)> = world
        .query::<(Entity, &RigidBody3D, &Transform)>()
        .filter(|(entity, body, _)| {
            body.body_type == BodyType::Static && matches!(world.parent(*entity), Ok(None))
        })
        .map(|(entity, ..)| (world.id(entity).expect("found alive"), entity))
        .collect();
    statics.sort_unstable();
    let mut shapes = Vec::new();
    for (_, entity) in statics {
        shapes.extend(shapes_of(world, entity));
    }
    match bake::bake(&settings, &shapes) {
        Ok(mesh) => {
            let milliseconds = started.elapsed().as_secs_f64() * 1e3;
            info!(
                entity = %id,
                polygons = mesh.polygons.len(),
                cells = mesh.cells.len(),
                milliseconds,
                "Navmesh baked"
            );
            Some(mesh)
        }
        Err(error) => {
            warn!(
                code = error.code(),
                entity = %id,
                "Entity {id}'s NavMeshVolume bakes no navmesh: {error}"
            );
            None
        }
    }
}

/// The shapes of a static body's colliders for a bake, in world space as physics places them:
/// box, sphere and capsule as solids, scaled and offset as physics does, and the mesh collider's
/// triangles as a surface, wound the other way if mirrored, so a floor still faces up.
fn shapes_of(world: &World, entity: Entity) -> Vec<Shape> {
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let (rotation, scale) = (unit(transform.rotation), transform.scale);
    let place = |at: Vec3| transform.translation.as_vec3() + rotation * at;
    let mut shapes = Vec::new();
    if let Some(boxed) = world.get::<BoxCollider3D>(entity) {
        let half = (boxed.size * scale).abs() * 0.5;
        let triangles = bake::cuboid(half, boxed.offset * scale, place);
        shapes.push(Shape {
            triangles,
            solid: true,
        });
    }
    if let Some(sphere) = world.get::<SphereCollider3D>(entity) {
        let radius = sphere.radius * scale.abs().max_element();
        let triangles = bake::capsule(radius, 0.0, sphere.offset * scale, place);
        shapes.push(Shape {
            triangles,
            solid: true,
        });
    }
    if let Some(capsule) = world.get::<CapsuleCollider3D>(entity) {
        let (radius, half) = capsule.shape(scale);
        let triangles = bake::capsule(radius, half, capsule.offset * scale, place);
        shapes.push(Shape {
            triangles,
            solid: true,
        });
    }
    let model = world.get::<ModelRoot>(entity).map(|root| root.0);
    let nodes = model.and_then(|model| world.assets().nodes(model));
    if let (Some(_), Some(nodes)) = (world.get::<MeshCollider3D>(entity), nodes) {
        let (vertices, triangles) = nodes.triangles();
        let mirrored = scale.x * scale.y * scale.z < 0.0;
        let triangles = triangles.into_iter().map(|indices| {
            let [a, b, c] = indices.map(|index| place(vertices[index as usize] * scale));
            if mirrored { [a, c, b] } else { [a, b, c] }
        });
        shapes.push(Shape {
            triangles: triangles.collect(),
            solid: false,
        });
    }
    shapes
}

impl World {
    /// The corners of the shortest path over the world's navmesh from `from` to `to`: points where
    /// it turns, past `from`, the last `to`. An end is on the navmesh where it is (an agent's
    /// middle above its floor), else snapped to the nearest point within the agent's height.
    ///
    /// The navmesh is brought up to date first, so a search sees what the static colliders give
    /// now, even in a world restored from a snapshot file before navigation stepped. That costs a
    /// fingerprint per search, and a bake if they changed.
    ///
    /// Nothing for a world without a volume, an end farther off the navmesh, or no path.
    pub fn find_path(&mut self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        refresh(self);
        let found = self.resource_scope(|_, navigation: &mut Navigation| {
            let mesh = Arc::clone(navigation.mesh.as_ref()?);
            let mut corners = Vec::new();
            let search = &mut navigation.search;
            mesh.find_path(search, from, to, &mut corners)
                .then_some(corners)
        });
        found.flatten()
    }
}

/// The navmesh `world` holds, by address, to tell one kept from one baked again.
#[cfg(test)]
pub(crate) fn navmesh_of(world: &World) -> Option<usize> {
    let mesh = world.resource::<Navigation>()?.mesh.as_ref()?;
    Some(Arc::as_ptr(mesh) as usize)
}

/// Polygons' linear color: sky blue, apart from colliders' green.
const POLYGONS: Vec4 = Vec4::new(0.1, 0.55, 1.0, 1.0);

/// Paths' linear color: amber, over the blue.
const PATHS: Vec4 = Vec4::new(1.0, 0.5, 0.05, 1.0);

/// Line width in points, as colliders' outlines.
const WIDTH: f32 = 2.0;

/// How far above floors lines draw, to show over their meshes.
const LIFT: Vec3 = Vec3::new(0.0, 0.04, 0.0);

/// How far each way of a destination its mark reaches, in units.
const MARK: f32 = 0.2;

/// Draws the navmesh (each polygon's outline along its cells' floors, picked as the volume's
/// entity) and each agent's path from where `poses` has it through the corners it has left, its
/// destination marked by a diamond, picked as the agent. Lines are [`WIDTH`] points wide at
/// `scale` pixels a point, a little above the floor. `shaders` adds the line shader on first use.
pub(crate) fn draw_navigation(
    world: &World,
    poses: &Poses<'_>,
    scale: f32,
    draws: &mut Draws,
    shaders: &mut Shaders,
) {
    let _zone = gg_core::zone!();
    let Some(navigation) = world.resource::<Navigation>() else {
        return;
    };
    let width = WIDTH * scale;
    let from_origin = Mat4::from_translation(-poses.origin().as_vec3());
    let mut line = |from: Vec3, to: Vec3, color: Vec4, id: u64| {
        let line = Line {
            from: from + LIFT,
            to: to + LIFT,
            color,
            width,
        };
        // The navmesh is the world's, in `f32`, drawn from the frame's origin.
        draws.push_line(shaders, from_origin, line, id);
    };
    let Some(mesh) = navigation.mesh.as_deref() else {
        return;
    };
    let volume = navigation.volume.map_or(0, EntityId::get);
    for polygon in 0..mesh.polygons.len() as u32 {
        outline(mesh, polygon, |from, to| line(from, to, POLYGONS, volume));
    }
    let (Some(transform), Some(agents)) = (
        world.component::<Transform>(),
        world.component::<NavAgent>(),
    ) else {
        return;
    };
    for (index, table) in world.tables().iter().enumerate() {
        let Some(column) = table.column::<NavAgent>(agents) else {
            continue;
        };
        if !table.has(transform) {
            continue;
        }
        for (row, (&entity, agent)) in table.entities().iter().zip(column).enumerate() {
            let left = agent
                .corners
                .get(agent.corner as usize..)
                .unwrap_or_default();
            let (Some(first), Some(&last)) = (left.first(), left.last()) else {
                continue;
            };
            let id = world.id(entity).expect("a table's rows are alive").get();
            let (at, _) = poses.pose(index, row);
            let at = at.as_vec3();
            let floor = mesh.floor_under(at).unwrap_or(first.y);
            let mut from = Vec3::new(at.x, floor, at.z);
            for &corner in left {
                line(from, corner, PATHS, id);
                from = corner;
            }
            let ways = [Vec3::X, Vec3::Z, Vec3::NEG_X, Vec3::NEG_Z].map(|way| last + way * MARK);
            for (i, &point) in ways.iter().enumerate() {
                line(point, ways[(i + 1) % ways.len()], PATHS, id);
            }
        }
    }
}

/// Gives `each` the lines outlining polygon `polygon`: along each edge from grid corner to grid
/// corner, at the cells' floors (halfway between two cells' floors between them). A run rising
/// evenly, as a flat floor or ramp, is one line.
fn outline(mesh: &NavMesh, polygon: u32, mut each: impl FnMut(Vec3, Vec3)) {
    let rect = mesh.polygons[polygon as usize];
    let floor = |x: u32, z: u32| {
        let cells = mesh.column(x, z).iter();
        let floor = cells
            .filter(|cell| cell.polygon == polygon)
            .map(|cell| cell.floor);
        i32::from(
            floor
                .min()
                .expect("a polygon holds a cell in each of its columns"),
        )
    };
    let (x, z, width, depth) = (rect.x, rect.z, rect.width, rect.depth);
    // Each edge, -X, +Z, +X and -Z: its length in cells, its cell and grid corner at a step.
    for edge in 0..4 {
        let length = if edge % 2 == 0 { depth } else { width };
        let cell = |k: u32| match edge {
            0 => (x, z + k),
            1 => (x + k, z + depth - 1),
            2 => (x + width - 1, z + k),
            _ => (x + k, z),
        };
        let corner = |k: u32| match edge {
            0 => [x, z + k],
            1 => [x + k, z + depth],
            2 => [x + width, z + k],
            _ => [x + k, z],
        };
        // Twice the floor at each grid corner along the edge, in levels, so halves are whole.
        let twice = |k: u32| {
            let below = |k: u32| {
                let (x, z) = cell(k);
                floor(x, z)
            };
            match k {
                0 => 2 * below(0),
                k if k == length => 2 * below(length - 1),
                k => below(k - 1) + below(k),
            }
        };
        let point = |k: u32, twice: i32| {
            let across = mesh.corner(corner(k));
            let y = mesh.min.y + twice as f32 * 0.5 * mesh.cell_height;
            Vec3::new(across.x, y, across.y)
        };
        let (mut start, mut start_twice) = (0, twice(0));
        let mut last = start_twice;
        let mut rise = None;
        for k in 1..=length {
            let height = twice(k);
            let step = height - last;
            match rise {
                None => rise = Some(step),
                Some(rise_by) if rise_by == step => {}
                Some(_) => {
                    each(point(start, start_twice), point(k - 1, last));
                    (start, start_twice) = (k - 1, last);
                    rise = Some(step);
                }
            }
            last = height;
        }
        each(point(start, start_twice), point(length, last));
    }
}
