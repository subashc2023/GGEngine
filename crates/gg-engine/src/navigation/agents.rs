//! Agents at the ticks: each plans its path when its destination or the navmesh changes, heads
//! for the next corner, and steers round neighbors, its velocity going to what moves it.

use gg_core::math::{Vec2, Vec3};
use tracing::{debug, warn};

use super::avoid::{Disc, Lines, MAX_NEIGHBORS, TIME_HORIZON};
use super::bake::across;
use super::mesh::NavMesh;
use super::path::Search;
use super::{NavAgent, Navigation};
use crate::ecs::mix64;
use crate::{CharacterController3D, Entity, EntityId, RigidBody3D, Tick, Transform, World};

/// How far apart in height agents are on different floors, with no navmesh to say how tall an
/// agent is. Agents avoid each other only on the same floor.
const FLOOR_APART: f32 = 2.0;

/// An agent as a tick finds it, with the velocities (across) it chose at the last tick, prefers
/// at this one and chooses at this one.
#[derive(Clone, Copy)]
struct Mover {
    entity: Entity,
    id: EntityId,
    position: Vec3,
    velocity: Vec2,
    radius: f32,
    speed: f32,
    preferred: Vec2,
    chosen: Vec2,
}

/// What agents' steps work in, kept between ticks so they allocate nothing once they have room:
/// the agents by ID; an agent's neighbors as (distance squared, ID, place among the agents); and
/// avoidance's lines.
#[derive(Default)]
pub(super) struct Crowd {
    movers: Vec<Mover>,
    near: Vec<(f32, EntityId, u32)>,
    lines: Lines,
}

/// `value` as a speed or radius: 0 if negative or not a number.
fn size_of(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// [`NavAgent::planned`]: the navmesh's fingerprint mixed with the destination's bits; never 0,
/// which stands for no plan.
pub(super) fn plan_key(fingerprint: u64, to: Vec3) -> u64 {
    let [x, y, z] = to.to_array().map(|at| u64::from(at.to_bits()));
    mix64(mix64(fingerprint ^ x) ^ ((y << 32) | z)).max(1)
}

/// Steps `navigation`'s agents a tick: those with a [`NavAgent`] and a [`Transform`], by ID,
/// except those under a parent (warned of once each). Each plans and heads for its next corner,
/// then each steers round its neighbors, then each velocity goes to what moves its agent.
pub(super) fn step(navigation: &mut Navigation, world: &mut World) {
    let Navigation {
        mesh,
        fingerprint,
        search,
        crowd,
        children,
        alone,
        ..
    } = navigation;
    let mesh = mesh.as_deref();
    let Crowd {
        movers,
        near,
        lines,
    } = crowd;
    movers.clear();
    children.retain(|&child| {
        world.get::<NavAgent>(child).is_some() && matches!(world.parent(child), Ok(Some(_)))
    });
    let mut waiting = None;
    for (entity, agent, transform) in world.query::<(Entity, &NavAgent, &Transform)>() {
        let id = world.id(entity).expect("found alive");
        if matches!(world.parent(entity), Ok(Some(_))) {
            if !children.contains(&entity) {
                warn!(
                    code = "navigation.child_agent",
                    entity = %id,
                    "Entity {id}'s NavAgent isn't moved: it's under a parent, whose space its \
                     transform is in"
                );
                children.push(entity);
            }
            continue;
        }
        if agent.destination.is_some() {
            waiting = Some(id);
        }
        movers.push(Mover {
            entity,
            id,
            // Navigation is in `f32`, as precise as that within some 16 km of the origin.
            position: transform.translation.as_vec3(),
            velocity: across(agent.velocity),
            radius: size_of(agent.radius),
            speed: size_of(agent.speed),
            preferred: Vec2::ZERO,
            chosen: Vec2::ZERO,
        });
    }
    if *fingerprint != 0 {
        *alone = false;
    } else if let Some(id) = waiting.filter(|_| !*alone) {
        warn!(
            code = "navigation.no_volume",
            entity = %id,
            "Entity {id}'s NavAgent has a destination, but the world has no NavMeshVolume to bake \
             a navmesh from: its agents stand"
        );
        *alone = true;
    }
    movers.sort_unstable_by_key(|mover| mover.id);

    for mover in movers.iter_mut() {
        let agent = world.get_mut::<NavAgent>(mover.entity);
        let agent = agent.expect("found with one this tick");
        mover.preferred = head(agent, mover, mesh, search, *fingerprint);
    }

    let apart = mesh.map_or(FLOOR_APART, |mesh| mesh.height);
    // Room for every agent, so it grows only as agents come.
    near.clear();
    near.reserve(movers.len());
    // Every pair, O(n²): some 0.2 ms a tick for 100 agents in a release build; the ceiling is
    // some hundreds. Past it, a grid of cells as wide as the neighbors' range, filled each tick,
    // would find neighbors in linear time.
    for i in 0..movers.len() {
        let me = movers[i];
        near.clear();
        for (j, other) in movers.iter().enumerate() {
            // Compared so: a height that isn't a number is on no floor.
            let same_floor = (other.position.y - me.position.y).abs() < apart;
            if j == i || !same_floor {
                continue;
            }
            let distance = across(other.position).distance_squared(across(me.position));
            let range = (me.speed + other.speed) * TIME_HORIZON + me.radius + other.radius;
            if distance < range * range {
                near.push((distance, other.id, j as u32));
            }
        }
        near.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        near.truncate(MAX_NEIGHBORS);
        let disc = |mover: &Mover, side: f32| Disc {
            position: across(mover.position),
            velocity: mover.velocity,
            radius: mover.radius,
            side,
        };
        let neighbors = near.iter().map(|&(_, id, j)| {
            // Which way along X to leave a neighbor in the very same place, by ID order.
            let side = if me.id < id { 1.0 } else { -1.0 };
            disc(&movers[j as usize], side)
        });
        let chosen = lines.velocity(disc(&me, 0.0), neighbors, me.preferred, me.speed);
        movers[i].chosen = chosen;
    }

    for mover in movers.iter() {
        apply(world, mesh, mover);
    }
}

/// How many corners each agent's path has room for when first planned, so steady ticks planning
/// paths of no more allocate nothing. A longer path grows its vector once.
const ROOM: usize = 16;

/// Plans `agent`'s path from where `mover` is if its destination or the navmesh (`fingerprint`)
/// changed, and each tick while it has a destination but no path (it started off the navmesh,
/// falling or knocked off), costing a search a tick.
///
/// Moves it past each corner within reach (its radius or a tick's way, whichever is farther) while
/// standing on the corner's floor, or from which the next is in sight on its floor. So one pushed
/// aside doesn't go back for a corner it no longer needs, and none takes a floor over or under its
/// way for its own.
///
/// Returns the preferred velocity, across: toward the next corner at its speed, slowing to reach
/// the last within a tick; none within its stopping distance of it, on its floor, or without a
/// path. One coming over a corner on another floor (pushed off a platform) plans again next tick.
fn head(
    agent: &mut NavAgent,
    mover: &Mover,
    mesh: Option<&NavMesh>,
    search: &mut Search,
    fingerprint: u64,
) -> Vec2 {
    let planned = agent.destination.map_or(0, |to| plan_key(fingerprint, to));
    let unplanned = agent.destination.is_some() && agent.corners.is_empty();
    if planned != agent.planned || unplanned {
        let afresh = planned != agent.planned;
        agent.planned = planned;
        agent.corner = 0;
        agent.corners.clear();
        agent.corners.reserve(ROOM);
        if let (Some(to), Some(mesh)) = (agent.destination, mesh)
            && !mesh.find_path(search, mover.position, to, &mut agent.corners)
            && afresh
        {
            let id = mover.id;
            debug!(
                code = "navigation.no_path",
                entity = %id,
                "Entity {id}'s NavAgent finds no path to {to}: it stands, searching again each tick"
            );
        }
    }
    let Some(last) = agent.corners.len().checked_sub(1) else {
        agent.arrived = false;
        return Vec2::ZERO;
    };
    let dt = Tick::DT.as_secs_f32();
    let here = across(mover.position);
    let reach = if mover.radius > mover.speed * dt {
        mover.radius
    } else {
        mover.speed * dt
    };
    // Whether the agent stands on a corner's floor: always without a navmesh.
    let on = |corner: Vec3| mesh.is_none_or(|mesh| mesh.stands(mover.position, corner.y));
    let mut corner = (agent.corner as usize).min(last);
    let floor = mesh.and_then(|mesh| Some((mesh, mesh.floor_under(mover.position)?)));
    while corner < last {
        let at = agent.corners[corner];
        let reached = here.distance(across(at)) <= reach && on(at);
        let next = agent.corners[corner + 1];
        let seen = floor.is_some_and(|(mesh, floor)| mesh.in_sight(here, floor, next));
        if !(reached || seen) {
            break;
        }
        corner += 1;
    }
    agent.corner = corner as u32;
    let target = agent.corners[corner];
    let way = across(target) - here;
    let distance = way.length();
    if distance <= reach && !on(target) {
        agent.planned = 0;
        agent.arrived = false;
        return Vec2::ZERO;
    }
    let arrived = corner == last && distance <= agent.stopping_distance && on(target);
    agent.arrived = arrived;
    // Compared so: a way that isn't a number goes nowhere.
    let away = distance > 0.0;
    if arrived || !away {
        return Vec2::ZERO;
    }
    let speed = if corner == last && distance / dt < mover.speed {
        distance / dt
    } else {
        mover.speed
    };
    way * (speed / distance)
}

/// Gives the velocity `mover` chose to its [`NavAgent::velocity`] and to what moves it, across: its
/// [`CharacterController3D`]'s velocity, else its [`RigidBody3D`]'s (the rest is physics'), else
/// its [`Transform`], moved along the navmesh by [`walk`]. Each is written only if it changes.
fn apply(world: &mut World, mesh: Option<&NavMesh>, mover: &Mover) {
    let entity = mover.entity;
    let chosen = mover.chosen;
    let velocity = Vec3::new(chosen.x, 0.0, chosen.y);
    let agent = world
        .get::<NavAgent>(entity)
        .expect("found with one this tick");
    if agent.velocity != velocity {
        let agent = world.get_mut::<NavAgent>(entity).expect("found with one");
        agent.velocity = velocity;
    }
    let changes = |velocity: Vec3| (velocity.x, velocity.z) != (chosen.x, chosen.y);
    if let Some(controller) = world.get::<CharacterController3D>(entity) {
        if changes(controller.velocity) {
            let controller = world.get_mut::<CharacterController3D>(entity);
            let controller = controller.expect("found with one");
            controller.velocity.x = chosen.x;
            controller.velocity.z = chosen.y;
        }
        return;
    }
    if let Some(body) = world.get::<RigidBody3D>(entity) {
        if changes(body.velocity) {
            let body = world
                .get_mut::<RigidBody3D>(entity)
                .expect("found with one");
            body.velocity.x = chosen.x;
            body.velocity.z = chosen.y;
        }
        return;
    }
    if chosen == Vec2::ZERO {
        return;
    }
    let at = mover.position;
    let walked = walk(mesh, at, chosen * Tick::DT.as_secs_f32());
    if walked != at {
        let transform = world.get_mut::<Transform>(entity).expect("found with one");
        transform.translation = walked.as_dvec3();
    }
}

/// Where an agent at `at` that nothing else moves goes by `step` across: along the navmesh, its
/// height following the floor, and kept to the mesh. A step off the mesh goes to the mesh's
/// nearest point within the step's length on a floor it steps onto (sliding along the edge), or
/// stays put if there's none. Without a floor under it, or a mesh, it goes straight on at its
/// height.
fn walk(mesh: Option<&NavMesh>, at: Vec3, step: Vec2) -> Vec3 {
    let there = Vec3::new(at.x + step.x, at.y, at.z + step.y);
    let Some((mesh, floor)) = mesh.and_then(|mesh| Some((mesh, mesh.floor_under(at)?))) else {
        return there;
    };
    if let Some(next) = mesh.floor_near(across(there), floor) {
        return Vec3::new(there.x, at.y + (next - floor), there.z);
    }
    let wanted = Vec3::new(there.x, floor, there.z);
    match mesh.nearest(wanted, step.length()) {
        Some((_, onto)) if (onto.y - floor).abs() <= mesh.climb => {
            Vec3::new(onto.x, at.y + (onto.y - floor), onto.z)
        }
        _ => at,
    }
}
