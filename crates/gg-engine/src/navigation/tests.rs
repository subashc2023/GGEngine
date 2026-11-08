//! Navigation's tests: the bake, paths, agents, re-bakes and snapshots, and its reference hash.

use gg_core::math::{Quat, Vec2, Vec3, libm};

use super::*;
use crate::{CharacterController3D, step_physics};

/// A static box of `size` around `at`, turned by `rotation`.
fn solid(world: &mut World, at: Vec3, size: Vec3, rotation: Quat) -> Entity {
    let transform = Transform {
        translation: at,
        rotation,
        ..Transform::IDENTITY
    };
    let body = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let boxed = BoxCollider3D {
        size,
        ..BoxCollider3D::default()
    };
    world.spawn((transform, body, boxed))
}

/// A floor `size` across and deep, a unit thick, its top at y 0.
fn floor(world: &mut World, size: f32) -> Entity {
    let at = Vec3::new(0.0, -0.5, 0.0);
    solid(world, at, Vec3::new(size, 1.0, size), Quat::IDENTITY)
}

/// A volume around `at`, of `size`, its agent half a unit in radius and its cells the defaults'.
fn volume(world: &mut World, at: Vec3, size: Vec3) -> Entity {
    let placed = Transform {
        translation: at,
        ..Transform::IDENTITY
    };
    let volume = NavMeshVolume {
        size,
        ..NavMeshVolume::default()
    };
    world.spawn((placed, volume))
}

/// The world's navmesh, once navigation has stepped.
fn baked(world: &mut World) -> Arc<NavMesh> {
    step_navigation(world);
    let navigation = world.resource::<Navigation>().expect("stepped");
    navigation.mesh.clone().expect("baked")
}

/// Whether `point` is on `mesh`, standing on a floor.
fn on(mesh: &NavMesh, point: Vec3) -> bool {
    mesh.nearest(point, 0.0).is_some()
}

/// The floors of the column `point` is over.
fn floors(mesh: &NavMesh, point: Vec3) -> Vec<f32> {
    let x = ((point.x - mesh.min.x) / mesh.cell_size) as u32;
    let z = ((point.z - mesh.min.z) / mesh.cell_size) as u32;
    let cells = mesh.column(x, z).iter();
    cells.map(|cell| mesh.height_of(cell.floor)).collect()
}

fn near(a: f32, b: f32, within: f32) -> bool {
    (a - b).abs() <= within
}

/// A floor bakes walkable as one rectangle, but for its edge, eroded by the agent's radius rounded
/// up to whole cells: 2 of a quarter for a radius of a half, 4 for a radius of 1.
#[test]
fn a_floor_bakes_walkable_but_its_edge_eroded_by_the_radius() {
    for (radius, edge) in [(0.5, 4.5), (1.0, 4.0)] {
        let mut world = World::default();
        floor(&mut world, 10.0);
        let placed = volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
        world
            .get_mut::<NavMeshVolume>(placed)
            .expect("a volume")
            .agent_radius = radius;
        let mesh = baked(&mut world);
        assert_eq!(mesh.polygons.len(), 1, "radius {radius}");
        let floors = floors(&mesh, Vec3::ZERO);
        assert!(
            floors.len() == 1 && near(floors[0], 0.0, 1e-4),
            "{floors:?}"
        );
        for x in [-edge + 0.1, 0.0, edge - 0.1] {
            assert!(on(&mesh, Vec3::new(x, 1.0, 0.0)), "radius {radius}: {x}");
        }
        for x in [-edge - 0.1, edge + 0.1] {
            assert!(!on(&mesh, Vec3::new(x, 1.0, 0.0)), "radius {radius}: {x}");
            let (_, snapped) = mesh.nearest(Vec3::new(x, 1.0, 0.0), 1.0).expect("near");
            assert!(near(snapped.x, x.signum() * edge, 1e-4), "{snapped}");
        }
    }
}

/// A wall across a floor splits it in two: each side's points join, and no path crosses.
#[test]
fn no_path_joins_the_sides_of_a_wall() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    let wall = Vec3::new(0.5, 3.0, 10.0);
    solid(&mut world, Vec3::new(0.0, 1.5, 0.0), wall, Quat::IDENTITY);
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    let mesh = baked(&mut world);
    assert!(
        !on(&mesh, Vec3::new(0.0, 1.0, 0.0)),
        "the wall's top is out of reach"
    );
    let left = Vec3::new(-3.0, 1.0, -2.0);
    let right = Vec3::new(3.0, 1.0, 2.0);
    assert_eq!(world.find_path(left, right), None);
    let across_left = world.find_path(left, Vec3::new(-3.0, 1.0, 3.0));
    assert_eq!(across_left.map(|corners| corners.len()), Some(1));
}

/// A step no higher than the agent climbs joins the floor it stands on, and one higher doesn't,
/// though its top is walkable.
#[test]
fn steps_join_their_floor_up_to_the_climb() {
    for (height, joined) in [(0.2, true), (0.6, false)] {
        let mut world = World::default();
        floor(&mut world, 10.0);
        let step = Vec3::new(3.0, height, 8.0);
        let at = Vec3::new(2.5, height * 0.5, 0.0);
        solid(&mut world, at, step, Quat::IDENTITY);
        volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
        let mesh = baked(&mut world);
        let top = Vec3::new(2.5, height + 1.0, 0.0);
        let (_, stood) = mesh.nearest(top, 0.0).expect("its top is walkable");
        assert!(near(stood.y, height, 0.1), "{stood}");
        let path = world.find_path(Vec3::new(-3.0, 1.0, 0.0), top);
        assert_eq!(path.is_some(), joined, "{height}: {path:?}");
        if let Some(path) = path {
            let end = *path.last().expect("an end");
            assert!(near(end.y, height, 0.1), "{end}");
        }
    }
}

/// A ramp no steeper than the agent's slope is walked, up from the floor, and a steeper one isn't.
#[test]
fn slopes_steeper_than_the_agents_arent_walked() {
    for (degrees, walked) in [(20.0f32, true), (60.0, false)] {
        let mut world = World::default();
        floor(&mut world, 10.0);
        let turned = Quat::from_rotation_z(gg_core::math::to_radians(degrees));
        let ramp = Vec3::new(4.0, 0.2, 3.0);
        solid(&mut world, Vec3::new(1.0, 0.6, 0.0), ramp, turned);
        volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 6.0, 12.0));
        let mesh = baked(&mut world);
        let high = mesh
            .cells
            .iter()
            .filter(|cell| mesh.height_of(cell.floor) > 0.4);
        assert_eq!(high.count() > 0, walked, "{degrees}°");
        if walked {
            let up = world.find_path(Vec3::new(-4.0, 1.0, 0.0), Vec3::new(1.5, 2.0, 0.0));
            let up = up.expect("up the ramp");
            assert!(up.last().expect("an end").y > 0.5, "{up:?}");
        }
    }
}

/// A platform over a floor holds a cell over each of the floor's beneath it, with room to stand
/// under it, and points find the floor they stand on; a lower one leaves no room beneath.
#[test]
fn a_floor_over_a_floor_holds_both() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    let platform = Vec3::new(4.0, 0.2, 4.0);
    solid(
        &mut world,
        Vec3::new(0.0, 3.1, 0.0),
        platform,
        Quat::IDENTITY,
    );
    volume(
        &mut world,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(12.0, 8.0, 12.0),
    );
    let mesh = baked(&mut world);
    let held = floors(&mesh, Vec3::ZERO);
    assert!(
        held.len() == 2 && near(held[0], 0.0, 1e-4) && near(held[1], 3.2, 0.1),
        "{held:?}"
    );
    let (_, below) = mesh
        .nearest(Vec3::new(0.0, 1.0, 0.0), 0.0)
        .expect("under it");
    let (_, above) = mesh.nearest(Vec3::new(0.0, 4.2, 0.0), 0.0).expect("on it");
    assert!(
        near(below.y, 0.0, 1e-4) && near(above.y, 3.2, 0.1),
        "{below} {above}"
    );
    let under = world.find_path(Vec3::new(-4.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    assert!(under.is_some_and(|corners| corners.len() == 1));

    let mut low = World::default();
    floor(&mut low, 10.0);
    solid(&mut low, Vec3::new(0.0, 1.6, 0.0), platform, Quat::IDENTITY);
    volume(
        &mut low,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(12.0, 8.0, 12.0),
    );
    let mesh = baked(&mut low);
    assert_eq!(floors(&mesh, Vec3::ZERO).len(), 1, "on top alone");
    assert!(!on(&mesh, Vec3::new(0.0, 0.5, 0.0)));
}

/// How near, across, the segment from `a` to `b` comes to the box from `min` to `max`.
fn clearance(a: Vec2, b: Vec2, (min, max): (Vec2, Vec2)) -> f32 {
    let points = (0..=64).map(|i| a.lerp(b, i as f32 / 64.0));
    points
        .map(|at| at.distance(at.clamp(min, max)))
        .fold(f32::INFINITY, f32::min)
}

/// A path goes round a wall's end, through the corners the funnel pulls it taut to, every point of
/// it at least the agent's radius from the wall, past its end; and over open floor goes straight,
/// its only corner its end.
#[test]
fn paths_go_round_walls_by_the_funnels_corners() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    // From the floor's near edge to 2 short of its far one.
    solid(
        &mut world,
        Vec3::new(0.0, 1.0, -1.5),
        Vec3::new(0.5, 2.0, 7.0),
        Quat::IDENTITY,
    );
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    step_navigation(&mut world);
    let (from, to) = (Vec3::new(-3.0, 1.0, -3.0), Vec3::new(3.0, 1.0, -3.0));
    let path = world.find_path(from, to).expect("round the wall");
    assert!(path.len() >= 3, "{path:?}");
    let end = *path.last().expect("an end");
    assert!(near(end.x, 3.0, 1e-4) && near(end.z, -3.0, 1e-4), "{end}");
    let wall = (Vec2::new(-0.25, -5.0), Vec2::new(0.25, 2.0));
    let mut length = 0.0;
    let mut at = Vec2::new(from.x, from.z);
    for corner in &path {
        let corner = Vec2::new(corner.x, corner.z);
        let clear = clearance(at, corner, wall);
        assert!(clear >= 0.5 - 1e-4, "{clear} to {corner}: {path:?}");
        length += at.distance(corner);
        at = corner;
    }
    assert!(
        path[..path.len() - 1].iter().all(|turn| turn.z > 2.0),
        "{path:?}"
    );
    assert!(length > 2.0 * 5.0, "{length}");

    let open = world.find_path(Vec3::new(-3.0, 1.0, 3.5), Vec3::new(3.0, 1.0, 3.5));
    assert_eq!(open.map(|corners| corners.len()), Some(1));
}

/// The way between two points is in sight over the mesh where it crosses none of a wall's eroded
/// cells, nor cuts the corner of one, and out of sight where it does, or leaves the mesh.
#[test]
fn sight_over_the_mesh_stops_at_walls() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    let wall = Vec3::new(0.5, 2.0, 7.0);
    solid(&mut world, Vec3::new(0.0, 1.0, -1.5), wall, Quat::IDENTITY);
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    let mesh = baked(&mut world);
    let sight = |from: Vec2, to: Vec2| mesh.in_sight(from, 0.0, Vec3::new(to.x, 0.0, to.y));
    assert!(
        sight(Vec2::new(-3.0, 3.5), Vec2::new(3.0, 3.5)),
        "past the wall's end"
    );
    assert!(
        !sight(Vec2::new(-3.0, -3.0), Vec2::new(3.0, -3.0)),
        "through the wall"
    );
    assert!(
        !sight(Vec2::new(-3.0, 0.0), Vec2::new(3.0, 4.0)),
        "through its end"
    );
    assert!(
        sight(Vec2::new(-3.0, -3.0), Vec2::new(-0.75, 2.25)),
        "up to its corner"
    );
    assert!(
        !sight(Vec2::new(-3.0, 3.5), Vec2::new(-5.0, 3.5)),
        "off the mesh"
    );
}

/// A path's ends off the mesh snap to its nearest point within the agent's height, and farther off
/// find no path; a start above the floor, as an agent's middle is, stands on it.
#[test]
fn ends_off_the_mesh_snap_to_it_within_reach() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    step_navigation(&mut world);
    let from = Vec3::new(-2.0, 1.0, 0.0);
    let snapped = world.find_path(from, Vec3::new(5.5, 0.0, 0.0));
    let end = *snapped.expect("within reach").last().expect("an end");
    assert!(near(end.x, 4.5, 1e-4) && near(end.y, 0.0, 1e-4), "{end}");
    assert_eq!(world.find_path(from, Vec3::new(8.0, 0.0, 0.0)), None);
    assert_eq!(world.find_path(Vec3::new(0.0, 5.0, 0.0), from), None);
    assert!(world.find_path(Vec3::new(0.0, 3.5, 0.0), from).is_some());
}

/// A world without a volume has no navmesh, and finds no path.
#[test]
fn a_world_without_a_volume_finds_no_path() {
    let mut world = World::default();
    floor(&mut world, 10.0);
    world.spawn((Transform::IDENTITY, NavAgent::default()));
    step_navigation(&mut world);
    let navigation = world
        .resource::<Navigation>()
        .expect("an agent's world steps");
    assert!(navigation.mesh.is_none());
    assert_eq!(world.find_path(Vec3::ZERO, Vec3::X), None);
}

/// An agent at `at` heading for `to`, which nothing but navigation moves.
fn walker(world: &mut World, at: Vec3, to: Vec3) -> Entity {
    let placed = Transform {
        translation: at,
        ..Transform::IDENTITY
    };
    let agent = NavAgent {
        destination: Some(to),
        speed: 2.0,
        ..NavAgent::default()
    };
    world.spawn((placed, agent))
}

/// Two agents heading straight at each other steer round each other, never nearer than their radii
/// together, and each arrives where it was going, its velocity none there.
#[test]
fn agents_head_on_keep_apart_and_arrive() {
    let mut world = World::seeded(3);
    floor(&mut world, 12.0);
    volume(&mut world, Vec3::ZERO, Vec3::new(14.0, 4.0, 14.0));
    let (west, east) = (Vec3::new(-4.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0));
    let agents = [
        walker(&mut world, west, east),
        walker(&mut world, east, west),
    ];
    let at = |world: &World, agent| world.get::<Transform>(agent).expect("alive").translation;
    let mut nearest = f32::INFINITY;
    for _ in 0..384 {
        step_navigation(&mut world);
        let apart = at(&world, agents[0]).distance(at(&world, agents[1]));
        nearest = nearest.min(apart);
    }
    assert!(nearest >= 0.95, "came within {nearest}");
    for (agent, to) in agents.into_iter().zip([east, west]) {
        let walked = world.get::<NavAgent>(agent).expect("an agent");
        assert!(walked.arrived, "{walked:?}");
        assert_eq!(walked.velocity, Vec3::ZERO);
        let there = at(&world, agent);
        assert!(
            there.distance(to) <= walked.stopping_distance + 1e-4,
            "{there}"
        );
    }
}

/// The navmesh's polygons and the agents' paths draw while they're shown, and not while they're
/// hidden, as at first: each polygon's outline, and each agent's way through the corners it has left
/// and the mark of its destination.
#[test]
fn the_navmesh_and_paths_draw_while_shown() {
    use gg_renderer::Viewport;

    use crate::Context;
    use crate::render_thread::Extracted;

    let mut cx = Context::seeded(3);
    let world = cx.world_mut();
    floor(world, 10.0);
    solid(
        world,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(2.0, 2.0, 2.0),
        Quat::IDENTITY,
    );
    volume(world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    let agent = walker(world, Vec3::new(-3.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.5));
    step_navigation(world);
    let mesh = world
        .resource::<Navigation>()
        .expect("stepped")
        .mesh
        .clone();
    let mesh = mesh.expect("baked");
    let mut outlined = 0;
    for polygon in 0..mesh.polygons.len() as u32 {
        outline(&mesh, polygon, |_, _| outlined += 1);
    }
    let corners = world
        .get::<NavAgent>(agent)
        .expect("an agent")
        .corners
        .len();
    assert!(mesh.polygons.len() > 1 && corners > 1, "{corners}");
    let viewport = Viewport {
        origin: [0, 0],
        size: [64, 64],
    };
    let mut frame = Extracted::default();
    let mut drawn = Vec::new();
    assert!(!cx.navmesh_shown(), "hidden at first");
    for shown in [false, true] {
        cx.set_navmesh_shown(shown);
        cx.draw_world(1.0, 0.0, 0);
        cx.extract(&mut frame, viewport);
        drawn.push(frame.draws.len());
    }
    assert_eq!(drawn[1] - drawn[0], outlined + corners + 4, "{drawn:?}");
}

/// Moving a wall bakes the navmesh again, under another fingerprint, and the agent heading past it
/// plans its path again, the other way round; the same entities bake the same navmesh.
#[test]
fn moving_a_collider_bakes_again_and_replans() {
    let mut world = World::seeded(5);
    floor(&mut world, 10.0);
    let size = Vec3::new(0.5, 2.0, 7.0);
    let wall = solid(&mut world, Vec3::new(0.0, 1.0, -1.5), size, Quat::IDENTITY);
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    let to = Vec3::new(3.0, 0.0, 0.0);
    let agent = walker(&mut world, Vec3::new(-3.0, 0.0, 0.0), to);
    step_navigation(&mut world);
    let first = world.resource::<Navigation>().expect("stepped").fingerprint;
    let planned = world.get::<NavAgent>(agent).expect("an agent").clone();
    assert!(planned.corners[0].z > 2.0, "round the far end: {planned:?}");
    step_navigation(&mut world);
    assert_eq!(
        world.get::<NavAgent>(agent).expect("an agent").planned,
        planned.planned
    );
    // The gap at the near end instead.
    world
        .get_mut::<Transform>(wall)
        .expect("alive")
        .translation
        .z = 1.5;
    step_navigation(&mut world);
    let navigation = world.resource::<Navigation>().expect("stepped");
    assert_ne!(navigation.fingerprint, first);
    let replanned = world.get::<NavAgent>(agent).expect("an agent");
    assert_ne!(replanned.planned, planned.planned);
    assert!(
        replanned.corners[0].z < -2.0,
        "round the near end: {replanned:?}"
    );
    let mesh = navigation.mesh.clone().expect("baked");
    world
        .get_mut::<Transform>(wall)
        .expect("alive")
        .translation
        .z = 1.5;
    world.remove_resource::<Navigation>();
    let again = baked(&mut world);
    assert_eq!(*again, *mesh, "baked alike from the same entities");
}

/// The step of a reference world's navmesh in which the agents walk: a floor of 16 square, three
/// boxes in the way, one turned, a step up to a block's top and a ramp up to a platform; and eight
/// characters, each crossing to the far side, two of them up to the platform and the block's top.
fn crossing(world: &mut World) -> Vec<Entity> {
    floor(world, 16.0);
    let blocks = [
        (Vec3::new(-2.0, 0.75, 0.0), Vec3::new(1.0, 1.5, 4.0), 0.0),
        (Vec3::new(3.0, 0.75, -2.5), Vec3::new(2.0, 1.5, 1.0), 0.0),
        (Vec3::new(2.5, 0.75, 3.5), Vec3::new(1.0, 1.5, 1.0), 0.52),
        // A step up, and the block it leads onto.
        (Vec3::new(-5.0, 0.1, -3.5), Vec3::new(2.0, 0.2, 1.0), 0.0),
        (Vec3::new(-5.0, 0.2, -5.5), Vec3::new(3.0, 0.4, 3.0), 0.0),
        // A platform, and a ramp up to it from -X.
        (Vec3::new(5.5, 0.5, -5.5), Vec3::new(3.0, 1.0, 3.0), 0.0),
    ];
    for (at, size, turn) in blocks {
        solid(world, at, size, Quat::from_rotation_y(turn));
    }
    let ramp = Quat::from_rotation_z(0.3805);
    solid(
        world,
        Vec3::new(2.85, 0.45, -5.5),
        Vec3::new(2.8, 0.2, 2.0),
        ramp,
    );
    let placed = volume(world, Vec3::new(0.0, 1.5, 0.0), Vec3::new(17.0, 6.0, 17.0));
    let shape = world.get_mut::<NavMeshVolume>(placed).expect("a volume");
    (shape.agent_radius, shape.agent_height) = (0.4, 1.6);
    let mut agents = Vec::new();
    for i in 0..8u8 {
        let (sin, cos) = libm::sincosf(f32::from(i) * std::f32::consts::FRAC_PI_4 + 0.3);
        let at = Vec3::new(cos * 6.5, 0.82, sin * 6.5);
        let to = match i {
            2 => Vec3::new(5.5, 1.0, -5.5),
            5 => Vec3::new(-5.0, 0.4, -5.5),
            _ => Vec3::new(-cos * 6.5, 0.0, -sin * 6.5),
        };
        let placed = Transform {
            translation: at,
            scale: Vec3::new(0.7, 0.8, 0.7),
            ..Transform::IDENTITY
        };
        let agent = NavAgent {
            destination: Some(to),
            speed: 3.0,
            radius: 0.4,
            ..NavAgent::default()
        };
        let character = (
            CharacterController3D::default(),
            CapsuleCollider3D::default(),
        );
        agents.push(world.spawn((placed, agent, character.0, character.1)));
    }
    agents
}

/// Steps `world`'s navigation, then its physics, `ticks` times, as a game's tick does.
fn run(world: &mut World, ticks: u32) {
    for _ in 0..ticks {
        step_navigation(world);
        step_physics(world);
    }
}

/// A snapshot of a world whose characters are walking round each other, restored, steps as the
/// world it was taken from did, bit for bit, sharing its navmesh; and so does one restored without
/// navigation's resource, as from a snapshot file, which bakes the navmesh again and keeps the
/// agents' paths, planned on a navmesh of the same fingerprint.
#[test]
fn a_snapshot_steps_as_its_world_did_and_bakes_again_without_its_navmesh() {
    let mut world = World::seeded(7);
    crossing(&mut world);
    run(&mut world, 40);
    let snapshot = world.clone();
    let shared = |world: &World| {
        let navigation = world.resource::<Navigation>().expect("stepped");
        Arc::as_ptr(navigation.mesh.as_ref().expect("baked"))
    };
    assert_eq!(shared(&snapshot), shared(&world), "shared, not copied");
    run(&mut world, 80);
    let went = world.hash();
    world.clone_from(&snapshot);
    run(&mut world, 80);
    assert_eq!(world.hash(), went, "restored");
    let mut baked_again = snapshot.clone();
    baked_again.remove_resource::<Navigation>();
    run(&mut baked_again, 80);
    assert_eq!(baked_again.hash(), went, "baked again");
}

/// A world whose characters are walking, saved whole to text as a snapshot file holds it and loaded
/// into a world of another process's making, steps as the world it was saved from did, bit for bit:
/// the file holds no navmesh, which the loaded world bakes again as it first steps.
#[test]
#[cfg(feature = "dev")]
fn a_world_saved_whole_steps_as_it_did() {
    use crate::Physics3D;

    let mut world = World::seeded(7);
    world.register_saved::<Physics3D>().expect("saved");
    crossing(&mut world);
    run(&mut world, 40);
    let mut text = String::new();
    world.save_whole(&mut text).expect("saved");
    assert!(!text.contains("Navigation"), "no navmesh in the file");
    let mut loaded = World::seeded(1);
    for registered in [
        loaded.register::<Transform>(),
        loaded.register::<RigidBody3D>(),
        loaded.register::<BoxCollider3D>(),
        loaded.register::<CapsuleCollider3D>(),
        loaded.register::<CharacterController3D>(),
        loaded.register::<NavMeshVolume>(),
        loaded.register::<NavAgent>(),
        loaded.register_saved::<Physics3D>(),
    ] {
        registered.expect("registered");
    }
    let lines = (1..).zip(text.lines());
    let changes = loaded.load_whole(lines, true).expect("loaded");
    assert!(changes.is_empty(), "{changes:?}");
    assert_eq!(loaded.hash(), world.hash(), "loaded");
    run(&mut world, 80);
    run(&mut loaded, 80);
    assert_eq!(loaded.hash(), world.hash(), "stepped on");
}

/// The world's hash after eight characters have crossed a floor among boxes, up a step and a ramp,
/// round each other, for eight seconds, navigation and physics stepping: a reference CI checks on
/// every platform, as the bake, the paths and avoidance must give the same bits everywhere. A
/// difference on some platforms only is a bug in determinism, not a value to change here.
#[test]
fn navigation_hashes_as_its_reference_says() {
    let logged = crate::testing::logged();
    let mut world = World::seeded(7);
    let agents = crossing(&mut world);
    run(&mut world, 512);
    let codes: Vec<String> = logged
        .codes
        .try_iter()
        .filter(|code| !code.is_empty())
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
    let arrived = agents
        .iter()
        .filter(|&&agent| world.get::<NavAgent>(agent).expect("an agent").arrived);
    assert_eq!(arrived.count(), agents.len(), "each where it was going");
    let content = world.hash().content;
    assert_eq!(content, 0x0673_2bf4_fbe9_5a7f, "{content:#018x}");
}

/// A platform 3 high over walkable ground, and a ramp up to it from its far side, along +X.
fn stacked(world: &mut World) {
    floor(world, 20.0);
    let platform = Vec3::new(4.0, 0.2, 4.0);
    solid(world, Vec3::new(0.0, 2.9, 0.0), platform, Quat::IDENTITY);
    // Its top from the platform's edge, 3 high at X 2, down to the ground at X 8.
    let turn = Quat::from_rotation_z(-0.463_647_6);
    solid(
        world,
        Vec3::new(4.955, 1.411, 0.0),
        Vec3::new(6.9, 0.2, 2.0),
        turn,
    );
    volume(world, Vec3::new(0.0, 2.0, 0.0), Vec3::new(22.0, 8.0, 22.0));
}

/// An agent on the ground sent onto a platform over it walks round to the ramp and up it, and
/// arrives on the platform: no corner on it counts as in sight from the ground under it, nor as
/// reached or arrived at from there.
#[test]
fn agents_take_no_floor_over_or_under_their_way_for_their_own() {
    let mut world = World::seeded(8);
    stacked(&mut world);
    let to = Vec3::new(0.0, 3.0, 0.0);
    let agent = walker(&mut world, Vec3::new(-6.0, 0.0, 0.0), to);
    world.get_mut::<NavAgent>(agent).expect("an agent").speed = 4.0;
    let mut highest = f32::NEG_INFINITY;
    for _ in 0..960 {
        step_navigation(&mut world);
        let at = world.get::<Transform>(agent).expect("alive").translation;
        highest = highest.max(at.y);
    }
    let walked = world.get::<NavAgent>(agent).expect("an agent");
    let at = world.get::<Transform>(agent).expect("alive").translation;
    assert!(walked.arrived, "{at} {walked:?}");
    assert!(
        (at.y - 3.0).abs() < 0.15,
        "on the platform, not under it: {at}"
    );
    assert!(highest < 3.2, "{highest}");
    let mesh = world
        .resource::<Navigation>()
        .expect("stepped")
        .mesh
        .clone();
    let mesh = mesh.expect("baked");
    let ground = Vec2::new(-6.0, 0.0);
    assert!(
        !mesh.in_sight(ground, 0.0, to),
        "the platform over the ground"
    );
    assert!(
        mesh.in_sight(ground, 0.0, Vec3::new(0.0, 0.0, 0.0)),
        "the ground under it"
    );
}

/// Agents spawned in one place, at rest, part, each the way the order of their IDs gives, rather
/// than staying in one another.
#[test]
fn agents_in_one_place_part() {
    let mut world = World::seeded(4);
    floor(&mut world, 10.0);
    volume(&mut world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    let agent = NavAgent {
        speed: 2.0,
        radius: 0.4,
        ..NavAgent::default()
    };
    let agents: Vec<Entity> = (0..3)
        .map(|_| world.spawn((Transform::IDENTITY, agent.clone())))
        .collect();
    for _ in 0..64 {
        step_navigation(&mut world);
    }
    let at = |agent| world.get::<Transform>(agent).expect("alive").translation;
    for (i, &a) in agents.iter().enumerate() {
        for &b in &agents[i + 1..] {
            let apart = at(a).distance(at(b));
            assert!(apart >= 0.7, "{apart}: {} {}", at(a), at(b));
        }
    }
}

/// An agent whose path can't be planned, as one falling from above the navmesh by more than its
/// height, plans it again as it lands, and walks it.
#[test]
fn a_plan_that_fails_is_tried_again_as_the_agent_lands() {
    let mut world = World::seeded(6);
    floor(&mut world, 10.0);
    volume(
        &mut world,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(12.0, 8.0, 12.0),
    );
    let to = Vec3::new(3.0, 0.0, 0.0);
    let placed = Transform {
        translation: Vec3::new(-3.0, 5.0, 0.0),
        ..Transform::IDENTITY
    };
    let agent = NavAgent {
        destination: Some(to),
        speed: 3.0,
        ..NavAgent::default()
    };
    let character = (
        CharacterController3D::default(),
        CapsuleCollider3D::default(),
    );
    let agent = world.spawn((placed, agent, character.0, character.1));
    run(&mut world, 1);
    let falling = world.get::<NavAgent>(agent).expect("an agent");
    assert!(falling.corners.is_empty(), "off the navmesh: {falling:?}");
    run(&mut world, 320);
    let landed = world.get::<NavAgent>(agent).expect("an agent");
    let at = world.get::<Transform>(agent).expect("alive").translation;
    assert!(landed.arrived, "{at} {landed:?}");
    assert!(Vec2::new(at.x - to.x, at.z - to.z).length() < 0.2, "{at}");
}

/// A world saved whole and loaded finds a path before navigation steps as the world it was saved
/// from does after, baking its navmesh for the search; and so does a world that keeps another
/// world's navmesh through the load, as a load keeps what the file doesn't hold.
#[test]
#[cfg(feature = "dev")]
fn a_world_loaded_whole_finds_paths_before_it_steps_as_it_did() {
    let wall = |world: &mut World, z: f32| {
        floor(world, 10.0);
        let size = Vec3::new(0.5, 2.0, 7.0);
        solid(world, Vec3::new(0.0, 1.0, z), size, Quat::IDENTITY);
        volume(world, Vec3::ZERO, Vec3::new(12.0, 4.0, 12.0));
    };
    let mut world = World::seeded(7);
    wall(&mut world, -1.5);
    step_navigation(&mut world);
    let (from, to) = (Vec3::new(-3.0, 1.0, -3.0), Vec3::new(3.0, 1.0, -3.0));
    let path = world.find_path(from, to).expect("round the wall");
    let mut text = String::new();
    world.save_whole(&mut text).expect("saved");
    let registered = |world: &mut World| {
        for registered in [
            world.register::<Transform>(),
            world.register::<RigidBody3D>(),
            world.register::<BoxCollider3D>(),
            world.register::<NavMeshVolume>(),
        ] {
            registered.expect("registered");
        }
    };
    let mut loaded = World::seeded(1);
    registered(&mut loaded);
    let changes = loaded.load_whole((1..).zip(text.lines()), true);
    assert!(changes.expect("loaded").is_empty());
    assert!(
        loaded.resource::<Navigation>().is_none(),
        "the file holds none"
    );
    assert_eq!(loaded.find_path(from, to), Some(path.clone()));
    // The gap at the other end, baked, then the saved world loaded over it.
    let mut stale = World::seeded(9);
    wall(&mut stale, 1.5);
    step_navigation(&mut stale);
    let before = navmesh_of(&stale);
    let changes = stale.load_whole((1..).zip(text.lines()), true);
    assert!(changes.expect("loaded").is_empty());
    assert_eq!(navmesh_of(&stale), before, "kept through the load");
    assert_eq!(stale.find_path(from, to), Some(path));
}

/// A volume whose columns, counted, would overflow is refused, as is one of more levels than a
/// span's height counts below the mark of an eroded span; one of the most levels bakes, a floor at
/// its very top kept.
#[test]
fn volumes_too_large_are_refused_without_overflowing() {
    let settings = |max: Vec3, cell_size: f32| bake::Settings {
        min: Vec3::ZERO,
        max,
        cell_size,
        cell_height: 1.0,
        radius: 0.0,
        height: 2.0,
        climb: 0.3,
        max_slope: FRAC_PI_4,
    };
    let code = |settings: bake::Settings, shapes: &[bake::Shape]| {
        bake::bake(&settings, shapes)
            .err()
            .map(|error| error.code())
    };
    let huge = settings(Vec3::new(1e8, 8.0, 1e8), 0.01);
    assert_eq!(code(huge, &[]), Some("navigation.too_large"));
    let tall = settings(Vec3::new(1.0, 65_534.0, 1.0), 0.5);
    assert_eq!(code(tall, &[]), Some("navigation.too_large"));
    let top = 65_533.0;
    let corners =
        [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)].map(|(x, z)| Vec3::new(x, top, z));
    let surface = bake::Shape {
        triangles: vec![
            [corners[0], corners[1], corners[2]],
            [corners[0], corners[2], corners[3]],
        ],
        solid: false,
    };
    let highest = settings(Vec3::new(1.0, top, 1.0), 0.5);
    let mesh = bake::bake(&highest, &[surface]).expect("the most levels bake");
    assert_eq!(mesh.cells.len(), 4, "the floor at its top kept");
}
