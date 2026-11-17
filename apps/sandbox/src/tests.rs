use std::collections::HashSet;
use std::fs;

use gg_engine::math::Vec4;

use super::*;

/// A world registered as the app's is.
fn world() -> World {
    let mut world = World::default();
    world.register::<Transform>().expect("registered");
    world.register::<Sprite>().expect("registered");
    register(&mut world).expect("registered");
    world
}

/// The grid's text, from its scene.
fn grid_text() -> String {
    gg_engine::ecs::read_scene(&scene("grid.ron")).unwrap_or_else(|error| panic!("{error:#}"))
}

/// The grid's scene loads and saves to its own bytes, so a sandbox type changing under it fails
/// here.
#[test]
fn the_grid_saves_as_it_is() {
    let text = grid_text();
    let mut world = world();
    let roots = world
        .load_scene(&text)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(world.save_scene(&roots).expect("saved"), text);
    assert_eq!(grid(&world).len(), 100);
}

/// Each texture and model under the sandbox's root has a committed ID file beside it, each ID its
/// own, and no ID file lacks its asset. So every checkout gives each asset the repository's ID,
/// rather than its runs writing others.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let found = gg_api::asset::scan(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(!found.assets.is_empty(), "the swarm's sheet is an asset");
    let mut ids = HashSet::new();
    for asset in &found.assets {
        let path = &asset.path;
        match &asset.meta {
            Some(Ok(meta)) => {
                let id = meta.id;
                assert!(ids.insert(id), "{path}.meta holds {id}, another asset's ID");
            }
            Some(Err(failure)) => panic!("{path}'s ID file: {}: {failure}", failure.code),
            None => panic!(
                "{path} has no ID file, {path}.meta: a run that serves the tool API writes it; \
                 commit it beside its file"
            ),
        }
    }
    assert!(found.orphans.is_empty(), "{:?}", found.orphans);
    assert!(found.unread.is_empty(), "{:?}", found.unread);
}

/// Drift finds exactly the drifters that wrap at each tick, as laps worked out apart from their
/// transforms count them.
#[test]
fn drift_finds_the_drifters_that_wrap_alone() {
    let mut world = world();
    let drifters: Vec<(Entity, Drifter)> = (0..1000)
        .map(|index| {
            let (drifter, _) = Drifter::new(index);
            (world.spawn((drifter.transform(0.0), drifter)), drifter)
        })
        .collect();
    let laps = |drifter: &Drifter, t: f64| {
        let lap = |start: f32, speed: f32, every: f32| {
            (f64::from(start) + f64::from(speed) * t).div_euclid(f64::from(every))
        };
        let (start, velocity) = (drifter.start, drifter.velocity);
        [
            lap(start.x, velocity.x, FIELD.x),
            lap(start.y, velocity.y, FIELD.y),
        ]
    };
    let (mut found, mut seen) = (Mutex::new(Vec::new()), 0);
    for tick in 1..=256 {
        let [before, t] = [tick - 1, tick].map(|tick| Tick::DT.as_secs_f64() * f64::from(tick));
        drift(&mut world, t, &mut found);
        let lapped = drifters
            .iter()
            .filter(|(_, drifter)| laps(drifter, t) != laps(drifter, before));
        let mut want: Vec<Entity> = lapped.map(|&(entity, _)| entity).collect();
        want.sort();
        let wrapped = found.get_mut().expect("no job panicked holding it");
        wrapped.sort();
        assert_eq!(*wrapped, want, "tick {tick}");
        seen += wrapped.len();
        wrapped.clear();
    }
    assert!(seen > 0, "no drifter wrapped");
}

/// A reload replaces the whole grid, nested squares too, or, refused, leaves it as it was.
#[test]
fn a_reload_replaces_the_grid_whole_or_not_at_all() {
    let mut world = world();
    world.load_scene(&grid_text()).expect("loaded");
    let cells = grid(&world);
    world
        .set_parent(cells[1], Some(cells[0]))
        .expect("parented");
    let nested = world.save_scene(&cells).expect("saved");
    replace_grid(&mut world, &nested).expect("replaced");
    assert_eq!(world.save_scene(&grid(&world)).expect("saved"), nested);
    let refused = replace_grid(&mut world, "(entities: {").expect_err("refused");
    assert_eq!(refused.code(), "scene.syntax");
    assert_eq!(world.save_scene(&grid(&world)).expect("saved"), nested);
}

/// The panels' commands wait for the next tick, which applies them in order. What the Inspector
/// sets stays until a panel changes it again.
#[test]
fn commands_change_the_world_at_the_next_tick_in_order() {
    let mut world = world();
    world.load_scene(&grid_text()).expect("loaded");
    let mut sandbox = Sandbox::new();
    sandbox.scene = Some(SwarmShapes {
        shapes: std::array::from_fn(|_| Sprite::default()),
    });
    let colors = [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]];
    let commands = [
        Command::Swarm(5),
        Command::Swarm(3),
        Command::GridColors(colors),
    ];
    sandbox.commands.extend(commands);
    let drifters = |world: &World| world.query::<&Drifter>().count();
    let colored = |world: &World| {
        let cells = world.query::<(&GridCell, &Sprite)>();
        let color = |cell: &GridCell| Vec3::from(colors[cell.color as usize]).extend(1.0);
        cells
            .filter(|(cell, sprite)| sprite.color == color(cell))
            .count()
    };
    assert_eq!((drifters(&world), colored(&world)), (0, 0));
    sandbox.tick(&mut world, Tick { number: 0 });
    assert_eq!((drifters(&world), colored(&world)), (3, 100));
    // A square the Inspector recolors stays so at the next tick.
    let square = world
        .query::<(Entity, &GridCell)>()
        .next()
        .map(|(square, _)| square);
    let sprite = world.get_mut::<Sprite>(square.expect("a square"));
    sprite.expect("a sprite").color = Vec4::ONE;
    sandbox.tick(&mut world, Tick { number: 1 });
    assert_eq!(colored(&world), 99);
    // And through a refused reload, which leaves the grid as it was.
    let text = "(entities: {".to_owned();
    sandbox.commands.push(Command::ReloadGrid { text, colors });
    sandbox.tick(&mut world, Tick { number: 2 });
    assert_eq!(colored(&world), 99);
    // A size dragged away and back between two ticks leaves the drifters be.
    let swarm = |world: &World| {
        let drifters = world.query::<(Entity, &Drifter)>();
        let mut drifters: Vec<(u32, Entity)> = drifters.map(|(e, d)| (d.index, e)).collect();
        drifters.sort_unstable();
        drifters
    };
    let before = swarm(&world);
    assert_eq!(
        before.iter().map(|&(i, _)| i).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    sandbox
        .commands
        .extend([Command::Swarm(1), Command::Swarm(3)]);
    sandbox.tick(&mut world, Tick { number: 3 });
    assert_eq!(swarm(&world), before);
    // Shrunk, it keeps its first.
    sandbox.commands.push(Command::Swarm(2));
    sandbox.tick(&mut world, Tick { number: 4 });
    assert_eq!(swarm(&world), before[..2]);
}

/// A world restored from a snapshot simulates as before, through resizes and a recolor: the world,
/// not the sandbox, holds the swarm.
#[test]
fn a_restored_world_simulates_as_before() {
    let mut cx = Context::seeded(0x5eed).root(env!("CARGO_MANIFEST_DIR"));
    let mut sandbox = Sandbox {
        swarm: 1_000,
        ..Sandbox::new()
    };
    sandbox.load(&mut cx).expect("loaded");
    sandbox.attach(&mut cx);
    let world = cx.world_mut();
    for number in 0..64 {
        sandbox.tick(world, Tick { number });
    }
    let snapshot = world.clone();
    let run = |sandbox: &mut Sandbox, world: &mut World| {
        let hashes = (64..128).map(|number| {
            match number {
                72 => sandbox.commands.push(Command::Swarm(1_200)),
                80 => sandbox.commands.push(Command::GridColors([[0.5; 3]; 2])),
                96 => sandbox.commands.push(Command::Swarm(250)),
                _ => {}
            }
            sandbox.tick(world, Tick { number });
            world.hash()
        });
        hashes.collect::<Vec<_>>()
    };
    let first = run(&mut sandbox, world);
    assert_eq!(world.query::<&Drifter>().count(), 250);
    world.clone_from(&snapshot);
    let again = run(&mut sandbox, world);
    let parted = first.iter().zip(&again).position(|(a, b)| a != b);
    assert_eq!(parted.map(|tick| 64 + tick), None, "the first tick to part");
}

/// The sandbox's simulation, run without a GPU from a seed through a script's commands, hashes at
/// attach and each tick as its reference says. Determinism needs it the same run to run, on four
/// threads as on one, in debug and release builds, and on every platform.
#[test]
fn the_simulation_gives_its_reference_hashes() {
    let run = simulate();
    let again = "; the simulation goes another way each run, or on more threads than one, so no \
                 reference holds";
    let on_four = gg_engine::jobs::on_threads(4, simulate);
    first_difference(
        &run,
        &on_four,
        "a second run in this process, on 4 threads",
        again,
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/references/hashes.txt");
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        fs::create_dir_all(path.parent().expect("a folder")).expect("made");
        fs::write(&path, run.join("\n") + "\n").expect("written");
        println!("wrote {}", path.display());
        return;
    }
    let reference = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "can't read {}: {error}; GG_UPDATE_REFERENCES=1 writes it",
            path.display()
        )
    });
    let reference: Vec<String> = reference.lines().map(str::to_owned).collect();
    let after = "; after a change meant to change the simulation or what the hash reads, \
                 `GG_UPDATE_REFERENCES=1 cargo test -p sandbox` writes it again, but a \
                 difference on some platforms alone is their simulations going apart";
    first_difference(&run, &reference, &path.display().to_string(), after);
}

/// The sandbox's simulation from a seed, as lines of the world's hash, content then layout: one
/// after attach, and one at each of 256 ticks. Between ticks, the script queues what the panels
/// would and sets what the Inspector would.
fn simulate() -> Vec<String> {
    // Rooted as the app is, so the textures' names, which the hash reads, are the same wherever
    // the repository is.
    let mut cx = Context::seeded(0x5eed).root(env!("CARGO_MANIFEST_DIR"));
    // One swarm for debug and release builds, whose own differ.
    let mut sandbox = Sandbox {
        swarm: 1_000,
        ..Sandbox::new()
    };
    sandbox.load(&mut cx).expect("loaded");
    sandbox.attach(&mut cx);
    let hashed = |at: &dyn std::fmt::Display, world: &World| format!("{at} {}", world.hash());
    let mut lines = vec![hashed(&"attach", cx.world())];
    for number in 0..256 {
        let world = cx.world_mut();
        match number {
            32 => sandbox.commands.push(Command::Swarm(1_500)),
            64 => {
                let colors = [[0.9, 0.6, 0.1], [0.1, 0.6, 0.9]];
                sandbox.commands.push(Command::GridColors(colors));
            }
            96 => sandbox.commands.push(Command::Swarm(250)),
            // An infinite spin turns the drifter by a NaN from an invalid operation. Its sign
            // varies by CPU, and the hash takes either as one.
            112 => {
                let mut drifters = world.query_mut::<&mut Drifter>();
                let first = drifters.find(|drifter| drifter.index == 0);
                first.expect("a drifter").spin = f32::INFINITY;
            }
            // A zero whose sign the hash keeps.
            144 => {
                let mut squares = world.query_mut::<(&GridCell, &mut Transform)>();
                let (_, at) = squares.next().expect("a square");
                at.translation.x = -0.0;
            }
            160 => {
                let colors = sandbox.grid_colors;
                let text = grid_text();
                sandbox.commands.push(Command::ReloadGrid { text, colors });
            }
            192 => sandbox.commands.push(Command::Swarm(1_000)),
            _ => {}
        }
        sandbox.tick(world, Tick { number });
        lines.push(hashed(&number, world));
    }
    lines
}

/// Panics at `run`'s first line that differs from `expected`, which `what` names, saying which part
/// of the hash differs and how many lines do, then `after`. If one ends first, it panics at the
/// other's first line past that end.
fn first_difference(run: &[String], expected: &[String], what: &str, after: &str) {
    let differ = |(line, other): &(&String, &String)| line != other;
    let lines = || run.iter().zip(expected);
    if let Some((line, other)) = lines().find(differ) {
        let parts = |line: &str| {
            line.split(' ')
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let (ours, theirs) = (parts(line), parts(other));
        let which: Vec<_> = ["content", "layout"]
            .into_iter()
            .zip(ours.iter().zip(&theirs))
            .filter(|(_, (a, b))| a != b)
            .map(|(part, _)| part)
            .collect();
        let verb = if which.len() == 1 {
            "differs"
        } else {
            "differ"
        };
        panic!(
            "the world's {} {verb} from {what}'s, first at its line `{other}`, which this run \
             has as `{line}`; {} of {} lines differ{after}",
            which.join(" and "),
            lines().filter(differ).count(),
            run.len()
        );
    }
    if let Some(past) = expected.get(run.len()).or(run.get(expected.len())) {
        panic!(
            "{what} has {} lines and this run {}, the first past the other's end being \
             `{past}`{after}",
            expected.len(),
            run.len()
        );
    }
}
