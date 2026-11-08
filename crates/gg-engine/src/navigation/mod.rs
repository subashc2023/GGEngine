//! Navigation: a navmesh baked from the world's static 3D colliders, as a [`NavMeshVolume`] says,
//! over which paths are found ([`World::find_path`]), and agents that walk them to their
//! destinations, steering round each other ([`NavAgent`]); stepped by [`step_navigation`] at the
//! ticks a layer runs it in, its state the agents' components and the world's [`Navigation`], which
//! snapshots copy and a run restored from a snapshot file bakes again; and its lines, the navmesh's
//! polygons and the agents' paths, drawn where they are.

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

/// What the world's navmesh is baked from, and for what size of agent: the static 3D colliders, as
/// physics has them, inside a box around its entity's origin, in its entity's space, so that it
/// moves, turns and scales with its entity, the bake taking the box that bounds it along the
/// world's axes. Their surfaces are voxelized into cells `cell_size` across and `cell_height` tall;
/// the cells an agent stands on are kept, those whose surface is no steeper than `max_slope`, with
/// `agent_height` of room above, joined to their neighbors where the step between them is no
/// higher than `agent_climb`; and the walkable area is eroded by `agent_radius`, so that a path
/// keeps an agent's middle that far from walls and edges. Finer cells find narrower ways, and bake
/// slower. A volume of more than 4,194,304 columns or 65,533 levels isn't baked
/// (`navigation.too_large`), nor one without a size (`navigation.bad_volume`). The world bakes one navmesh, from the volume of the
/// least ID, warning of the others (`navigation.volumes`).
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct NavMeshVolume {
    /// How wide, tall and deep its box is.
    #[reflect(min = 0.0, default = Vec3::new(20.0, 8.0, 20.0))]
    pub size: Vec3,
    /// How far from walls and edges an agent's middle stays, in units.
    #[reflect(min = 0.0, default = 0.5)]
    pub agent_radius: f32,
    /// How much room above a floor an agent needs to stand there, in units.
    #[reflect(min = 0.0, default = 2.0)]
    pub agent_height: f32,
    /// How high a step an agent climbs, in units.
    #[reflect(min = 0.0, default = 0.3)]
    pub agent_climb: f32,
    /// How steep a slope an agent climbs, from level: steeper ones aren't walked.
    #[reflect(degrees, min = 0.0, default = FRAC_PI_4)]
    pub max_slope: f32,
    /// How wide and deep the heightfield's cells are, in units.
    #[reflect(min = 0.01, default = 0.25)]
    pub cell_size: f32,
    /// How tall the heightfield's cells are, in units: the floors' heights are their multiples.
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

/// An agent that walks the world's navmesh to its `destination`, at each tick a layer runs
/// [`step_navigation`] in: it plans its path as its destination or the navmesh changes, from where
/// its [`Transform`] is to its destination, each snapped to the navmesh within the agent's height
/// of the volume, and again at each tick while it has a destination and no path, as while it falls
/// onto the navmesh; heads for its path's next corner, past each it comes within its radius of on
/// the corner's floor, or from which the next is in sight over the navmesh on its own, planning
/// again where it comes over a corner on another floor; and slows to stop within
/// `stopping_distance` of the last, on its floor, coming back to it where it's pushed off; and
/// steers round the agents near it, by ORCA, each giving way half as much as two need. The
/// velocity it chooses goes, across, to its entity's [`CharacterController3D`](crate::CharacterController3D),
/// which then slides it along walls and up steps and slopes, or else to its [`RigidBody3D`],
/// gravity and what's up left to physics, or else to its transform, which moves along the navmesh,
/// its height following the floor, kept to the mesh. An agent under a parent isn't moved
/// (`navigation.child_agent`), and without a [`NavMeshVolume`] in the world it stands
/// (`navigation.no_volume`). Its entity's rotation is the game's to set, as from `velocity`.
///
/// Its path is its state, as the rest of the simulation's is, so that snapshots, replays and peers
/// go alike: `corners`, `corner`, `planned`, `arrived` and `velocity` are the steps', which code,
/// scripts and tools read, and steer it by setting its destination, speed and the rest.
#[derive(Debug, PartialEq, Component)]
pub struct NavAgent {
    /// Where it's going, a point on the navmesh or near it; none to stand where it is.
    pub destination: Option<Vec3>,
    /// How fast it walks, in units a second.
    #[reflect(min = 0.0, default = 3.5)]
    pub speed: f32,
    /// How far round it other agents keep, in units, and how near a corner of its path it comes
    /// before heading for the next.
    #[reflect(min = 0.0, default = 0.5)]
    pub radius: f32,
    /// How near its destination, across, counts as there.
    #[reflect(min = 0.0, default = 0.1)]
    pub stopping_distance: f32,
    /// Whether it's within its stopping distance of its destination, as of the last step.
    pub arrived: bool,
    /// The velocity it chose at the last step, across, in units a second.
    pub velocity: Vec3,
    /// The corners of its path, each a point on the navmesh, the last its destination snapped to
    /// it; none without a destination or a path to it.
    pub corners: Vec<Vec3>,
    /// The corner it heads for, of `corners`.
    pub corner: u32,
    /// What its path was planned from, the navmesh's fingerprint and its destination mixed, which
    /// plans it again as either changes; 0 for none.
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

    /// Copies `source` into the memory this one holds, its corners into its vector, so that a
    /// snapshot taken into a world kept from before allocates nothing for it while its path is no
    /// longer.
    fn clone_from(&mut self, source: &Self) {
        let corners = std::mem::take(&mut self.corners);
        *self = Self { corners, ..*source };
        self.corners.clone_from(&source.corners);
    }
}

/// The world's navigation, a [`Resource`](crate::Resource) that [`step_navigation`] gives a world
/// the first time it steps one with a [`NavMeshVolume`] or a [`NavAgent`]: the navmesh, baked from
/// what's inside the volume, and a fingerprint of what it was baked from, the volume and each
/// static body's transform and colliders by its entity's ID, which bakes it again as it changes and
/// which agents' paths are planned on. Snapshots share the navmesh with the world they're taken
/// from; snapshot files don't hold it, as a bake from the same entities and assets makes the same
/// navmesh, bit for bit, so that a run restored from one bakes it again as it first steps, under
/// the fingerprint its agents planned on.
#[derive(Default)]
pub struct Navigation {
    mesh: Option<Arc<NavMesh>>,
    fingerprint: u64,
    /// The entity of the volume the navmesh was baked from, which its lines are picked as.
    volume: Option<EntityId>,
    /// The surfaces of the models that mesh colliders were made of, as their models' were when
    /// last seen, each with the hash of its triangles, which the fingerprint takes.
    surfaces: Vec<(Arc<Vec<Surface>>, u64, bool)>,
    /// The agents under a parent, which aren't moved, warned of once each.
    children: Vec<Entity>,
    /// Whether it warned that agents have destinations in a world without a volume, since it last
    /// had one; and that it has more than one.
    alone: bool,
    volumes: bool,
    /// What a path search and agents' steps work in, which a copy makes anew, as no step carries
    /// anything in them over.
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

    /// Copies `source` into the memory this one holds, keeping its own workspaces.
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

/// Steps `world`'s navigation a tick, as a system a layer's tick runs, after what sets its agents'
/// destinations, such as scripts, and before physics, which moves its characters as their agents
/// set: brings the navmesh up to date, baking it again if what it's baked from has changed since,
/// as a world restored from a snapshot file has none yet, logging `Navmesh baked` with its polygons
/// and how long it took, then each [`NavAgent`] plans, heads for its next corner and steers round
/// the rest, as it says. A world is given a [`Navigation`] the first time it steps, or searches,
/// with a [`NavMeshVolume`] or an agent and without one.
pub fn step_navigation(world: &mut World) {
    let _zone = gg_core::zone!();
    refresh(world);
    world.resource_scope(|world, navigation: &mut Navigation| agents::step(navigation, world));
}

/// Brings `world`'s navmesh up to date with what it's baked from: bakes it again if that has
/// changed since it was baked, as a world restored from a snapshot file has no navmesh yet and a
/// world loaded whole may keep one of another world, logging `Navmesh baked` with its polygons and
/// how long it took. Gives the world a [`Navigation`] if it has none and has a [`NavMeshVolume`]
/// or a [`NavAgent`]. Searches and steps go through it, so that what a path is planned on is what
/// the world's static colliders give, whichever came first after a load, as peers that loaded the
/// world at different times must agree; and a client's copy of the server's world, which it's taken
/// back to as it corrects, so that it holds the resources the client's world does.
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

    /// The entity of the world's volume, the one of the least ID, warning once while it has
    /// another.
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

    /// The fingerprint of what the navmesh is baked from in `world`, with `volume`: the volume's
    /// ID, where its entity is and its fields, and each static body's ID, transform and colliders,
    /// a mesh collider by the hash of its model's triangles, summed, so that the order they're
    /// found in counts for nothing; never 0, which stands for no navmesh.
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
            .map_or(Mat4::IDENTITY, |pose| pose.matrix);
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
            let mut hash = floats(mix64(id(entity)), &translation.to_array());
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

    /// The hash of the triangles of the model `entity`'s [`ModelRoot`] names, as its model's
    /// surfaces are now, hashed once for each time they're read; 0 for none.
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

/// `rotation` made a unit, as physics makes its bodies', so that shapes keep their sizes: none,
/// where it has no length.
fn unit(rotation: Quat) -> Quat {
    let unit = Vec4::from(rotation).try_normalize();
    unit.map_or(Quat::IDENTITY, Quat::from_vec4)
}

/// Bakes the navmesh of `world`'s static colliders inside `volume`'s box, as [`NavMeshVolume`]
/// says, each body's in the order of their entities' IDs, so that the same entities bake the same
/// navmesh whatever order queries find them in; logging the bake, or warning of a volume it can't
/// bake.
fn bake_from(world: &World, volume: Entity) -> Option<NavMesh> {
    let _zone = gg_core::zone!();
    let started = Instant::now();
    let id = world.id(volume).expect("found alive");
    let shape = *world.get::<NavMeshVolume>(volume).expect("found with one");
    let placed = world
        .pose(volume)
        .map_or(Mat4::IDENTITY, |pose| pose.matrix);
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

/// The shapes of `entity`'s colliders, a static body's, as a bake takes them, in the world's space
/// as physics places them: its box, sphere and capsule as solids, scaled and offset as physics
/// scales them, and its mesh collider's triangles as a surface, wound the other way where its
/// entity is mirrored, so that a floor still faces up.
fn shapes_of(world: &World, entity: Entity) -> Vec<Shape> {
    let transform = *world.get::<Transform>(entity).expect("found with one");
    let (rotation, scale) = (unit(transform.rotation), transform.scale);
    let place = |at: Vec3| transform.translation + rotation * at;
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
    /// The corners of the shortest path over the world's navmesh from `from` to `to`: points on the
    /// navmesh where the path turns, past `from`, the last `to`; each end on the navmesh where it
    /// is, as where an agent's middle is above its floor, or else snapped to the navmesh's nearest
    /// point within the agent's height of the [`NavMeshVolume`]. The navmesh is brought up to date
    /// first, as [`step_navigation`] does, so that a search sees what the world's static colliders
    /// give now, in a world restored from a snapshot file before navigation has stepped as after,
    /// at the cost of a fingerprint of them a search, and a bake where they changed. Nothing for a
    /// world without a volume, for an end off the navmesh by more than that, or where no path joins
    /// them.
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

/// The navmesh `world`'s navigation holds, by its address, which tells one kept from one baked
/// again: for tests.
#[cfg(test)]
pub(crate) fn navmesh_of(world: &World) -> Option<usize> {
    let mesh = world.resource::<Navigation>()?.mesh.as_ref()?;
    Some(Arc::as_ptr(mesh) as usize)
}

/// The color the navmesh's polygons draw in, linear: a sky blue, apart from colliders' green.
const POLYGONS: Vec4 = Vec4::new(0.1, 0.55, 1.0, 1.0);

/// The color agents' paths draw in, linear: an amber, over the navmesh's blue.
const PATHS: Vec4 = Vec4::new(1.0, 0.5, 0.05, 1.0);

/// How wide the navmesh's and the paths' lines draw, in points, as colliders' outlines do.
const WIDTH: f32 = 2.0;

/// How far above the floors the lines draw, so that they show over the meshes they lie on.
const LIFT: Vec3 = Vec3::new(0.0, 0.04, 0.0);

/// How far each way of an agent's destination its mark reaches, in units.
const MARK: f32 = 0.2;

/// Draws into `draws` the world's navmesh, each polygon's outline along its cells' floors, picked as
/// its volume's entity, and each agent's path from where its entity draws this frame, as `poses`
/// has it, through the corners it has left, its destination marked by a diamond, picked as the
/// agent's: each line 2 points wide in a window of `scale` pixels a point, a little above the
/// floor. Lines draw with the engine's line shader, which `shaders` adds the first time any do.
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
    let mut line = |from: Vec3, to: Vec3, color: Vec4, id: u64| {
        let line = Line {
            from: from + LIFT,
            to: to + LIFT,
            color,
            width,
        };
        draws.push_line(shaders, Mat4::IDENTITY, line, id);
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

/// Gives `each` the lines polygon `polygon` of `mesh` is outlined with: along each edge, from grid
/// corner to grid corner at the floors of the cells along it, between two cells halfway between
/// their floors, a run whose height rises evenly as one line, as over a flat floor or a ramp.
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
    // Each edge, -X, +Z, +X and -Z: how many cells long, its cell at a step along it, and its grid
    // corner there.
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
        // Twice the floor at each grid corner along the edge, in levels, so that halves are whole.
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
