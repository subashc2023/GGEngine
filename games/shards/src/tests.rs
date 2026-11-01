use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use gg_api::Project;

use super::*;

/// The game as it attaches, the project's as the app is, so that the world's hash doesn't depend on
/// where the repository is, its world's IDs drawn from `seed`.
fn attached(seed: u64) -> (Context, Shards) {
    let cx = Context::seeded(seed).project(env!("CARGO_MANIFEST_DIR"));
    let mut cx = cx.expect("the project reads");
    let mut shards = Shards::new();
    shards.load(&mut cx);
    shards.attach(&mut cx);
    (cx, shards)
}

/// Where player `index`'s disc is.
fn player_at(world: &World, index: u8) -> Vec2 {
    let mut players = world.query::<(&Player, &Transform)>();
    let found = players.find(|(player, _)| player.index == index);
    found.expect("the player").1.translation.truncate()
}

/// The path of the game's scene file `name`.
fn scene(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join(name)
}

/// The game's project's manifest is canonical, writing back to its own bytes, and starts the game
/// in the arena.
#[test]
fn the_project_is_canonical_and_starts_in_the_arena() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(dir.join(Project::FILE)).expect("the manifest");
    let project = Project::parse(&text).expect("the project reads");
    assert_eq!(project.text(), text);
    assert_eq!(project.start_path(dir), scene("arena.ron"));
}

/// Each texture and model under the game's asset root has its ID file beside it, committed with
/// it, each holding an ID of its own, and no ID file is left without its asset, so that every
/// checkout gives each asset the ID the repository holds, rather than its runs writing others.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = Project::read(dir).expect("the project reads");
    // The game has no texture or model yet, so it finds none: it bites once Phase 7's third
    // milestone gives the arena's Pillar a texture.
    let found = gg_api::asset::scan(&project.assets_dir(dir));
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

/// The arena's scene loads, with the textures it names, and saves to its own bytes, so that a type
/// of the game's that changes under it fails here.
#[test]
fn the_arena_saves_as_it_is() {
    let path = scene("arena.ron");
    let text = fs::read_to_string(&path).expect("the arena");
    let mut cx = Context::seeded(1).root(env!("CARGO_MANIFEST_DIR"));
    register(cx.world_mut());
    let roots = cx
        .load_scene_file(&path)
        .unwrap_or_else(|error| panic!("{error:#}"));
    assert_eq!(cx.world().save_scene(&roots).expect("saved"), text);
}

/// The game builds the arena, both players at their starts and its shards, apart from the players;
/// a player touching a shard scores it, the shard turning up elsewhere, and the match's tenth wins
/// it, the players going back to their starts.
#[test]
fn a_touched_shard_scores_and_turns_up_elsewhere() {
    let (mut cx, mut shards) = attached(0x5eed);
    let world = cx.world_mut();
    assert_eq!(world.query::<&Player>().count(), 2);
    assert_eq!(world.query::<&Shard>().count(), SHARDS);
    assert_eq!(player_at(world, 1), STARTS[1]);
    let shard = world.query::<(Entity, &Shard)>().next().expect("a shard").0;
    let at = world.get::<Transform>(shard).expect("placed").translation;
    for player in [0, 1] {
        assert!(at.truncate().distance(STARTS[player]) > 1.5, "{at}");
    }
    // Player 0 onto the shard.
    let mut players = world.query_mut::<(&Player, &mut Transform)>();
    let (_, disc) = players
        .find(|(player, _)| player.index == 0)
        .expect("player 0");
    disc.translation = at.truncate().extend(PLAYER_Z);
    shards.tick(world, Tick { number: 0 });
    let found = *world.query::<&Match>().next().expect("the match");
    assert_eq!(found.scores, [1, 0]);
    let moved = world.get::<Transform>(shard).expect("placed").translation;
    assert_ne!(moved.truncate(), at.truncate());
    // The match's tenth.
    world
        .query_mut::<&mut Match>()
        .next()
        .expect("the match")
        .scores = [WINS_AT - 1, 3];
    let mut players = world.query_mut::<(&Player, &mut Transform)>();
    let (_, disc) = players
        .find(|(player, _)| player.index == 0)
        .expect("player 0");
    disc.translation = moved.truncate().extend(PLAYER_Z);
    shards.tick(world, Tick { number: 1 });
    let found = *world.query::<&Match>().next().expect("the match");
    assert_eq!((found.scores, found.wins), ([0, 0], [1, 0]));
    assert!(player_at(world, 0).distance(STARTS[0]) < 0.2);
}

/// The keys and the left stick queue steers and dashes for the run's own player, as they come, so
/// that the frame's ticks apply them: player 0 alone, and the joiner's for player 1; and a tick
/// steers only the player its commands name.
#[test]
fn the_keys_queue_commands_for_the_runs_own_player() {
    let (mut cx, mut shards) = attached(0x5eed);
    let key = |key, pressed| Event::Key {
        key,
        pressed,
        repeat: false,
    };
    let pad = gg_engine::Pad(0);
    for event in [
        key(Key::KeyD, true),
        key(Key::Space, true),
        key(Key::Space, false),
        key(Key::KeyD, false),
        Event::PadStick {
            pad,
            stick: Stick::Left,
            value: Vec2::new(0.0, -0.5),
        },
    ] {
        assert_eq!(shards.event(&mut cx, &event), ControlFlow::Continue(()));
    }
    let player = 0;
    let expected = [
        Command::Steer {
            player,
            heading: Vec2::X,
        },
        Command::Dash { player },
        Command::Steer {
            player,
            heading: Vec2::ZERO,
        },
        Command::Steer {
            player,
            heading: Vec2::new(0.0, -0.5),
        },
    ];
    assert_eq!(shards.commands, expected);
    shards.commands.clear();
    // As the joiner, player 1.
    shards.player = 1;
    let _ = shards.event(&mut cx, &key(Key::ArrowLeft, true));
    let world = cx.world_mut();
    for number in 0..32 {
        shards.tick(world, Tick { number });
    }
    assert!(player_at(world, 1).x < STARTS[1].x - 1.0);
    assert_eq!(player_at(world, 0), STARTS[0]);
}

/// A world restored from a snapshot simulates as it did from there, through steers, dashes, bumps
/// and shards collected: the game keeps nothing of the match but in the world.
#[test]
fn a_restored_world_simulates_as_before() {
    let (mut cx, mut shards) = attached(0x5eed);
    let world = cx.world_mut();
    for number in 0..64 {
        script(&mut shards, world, number);
        shards.tick(world, Tick { number });
    }
    let snapshot = world.clone();
    let run = |shards: &mut Shards, world: &mut World| {
        let hashes = (64..256).map(|number| {
            script(shards, world, number);
            shards.tick(world, Tick { number });
            world.hash()
        });
        hashes.collect::<Vec<_>>()
    };
    let first = run(&mut shards, world);
    world.clone_from(&snapshot);
    let again = run(&mut shards, world);
    let parted = first.iter().zip(&again).position(|(a, b)| a != b);
    assert_eq!(parted.map(|tick| 64 + tick), None, "the first tick to part");
}

/// The commands a script queues before tick `number`, what both players' keys would, as `world`
/// stands: every 8 ticks, player 0 steers for the nearest shard, and player 1 for player 0, each
/// dashing now and then, so that they collect shards and bump into each other.
fn script(shards: &mut Shards, world: &World, number: u64) {
    if !number.is_multiple_of(8) {
        return;
    }
    let [ours, theirs] = [0, 1].map(|index| player_at(world, index));
    let shard = world
        .query::<(&Shard, &Transform)>()
        .map(|(_, at)| at.translation.truncate());
    let nearest = shard.min_by(|a, b| a.distance(ours).total_cmp(&b.distance(ours)));
    let toward = |from: Vec2, to: Vec2| (to - from).normalize_or_zero();
    shards.commands.extend([
        Command::Steer {
            player: 0,
            heading: toward(ours, nearest.unwrap_or(ours)),
        },
        Command::Steer {
            player: 1,
            heading: toward(theirs, ours),
        },
    ]);
    if number.is_multiple_of(96) {
        shards.commands.push(Command::Dash { player: 1 });
    }
}

/// The game's simulation, run without a GPU from a seed through the commands of a script, hashes at
/// its attach and at each tick as its reference says: the same in one run as in the next, on four
/// threads as on one, in a debug build as in a release one, and on every platform, as determinism,
/// and so lockstep, needs. `GG_UPDATE_REFERENCES=1` writes the reference again, after a change
/// meant to change the simulation or what the hash reads.
#[test]
fn the_simulation_gives_its_reference_hashes() {
    let run = simulate();
    let on_four = gg_engine::on_threads(4, simulate);
    assert_eq!(
        run.iter().zip(&on_four).position(|(a, b)| a != b),
        None,
        "the simulation goes another way on 4 threads, so no reference holds"
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
    let lines = || run.iter().zip(reference.lines());
    if let Some((line, other)) = lines().find(|(line, other)| line != other) {
        panic!(
            "the world's hash differs from {}'s, first at its line `{other}`, which this run has \
             as `{line}`, {} of {} lines differing; after a change meant to change the simulation \
             or what the hash reads, `GG_UPDATE_REFERENCES=1 cargo test -p shards` writes it \
             again, but a difference on some platforms alone is their simulations going apart",
            path.display(),
            lines().filter(|(line, other)| line != other).count(),
            run.len()
        );
    }
    assert_eq!(run.len(), reference.lines().count(), "{}", path.display());
}

/// The game's simulation from a seed, as lines of the world's hash, its content then its layout:
/// one after it attaches, and one at each of 512 ticks, between which the script queues what both
/// players' keys would.
fn simulate() -> Vec<String> {
    let (mut cx, mut shards) = attached(0x5eed);
    let hashed = |at: &dyn std::fmt::Display, world: &World| format!("{at} {}", world.hash());
    let mut lines = vec![hashed(&"attach", cx.world())];
    let world = cx.world_mut();
    for number in 0..512 {
        script(&mut shards, world, number);
        shards.tick(world, Tick { number });
        lines.push(hashed(&number, world));
    }
    let found = world.query::<&Match>().next().expect("the match");
    assert!(
        found.scores.iter().sum::<u32>() + found.wins.iter().sum::<u32>() > 0,
        "the script collects a shard, so that the reference covers collecting"
    );
    lines
}

/// The live recording, `tests/references/live.recording`, of a windowed run steered by its keys,
/// edited through the tool API and its scene file saved, replays without a GPU to the hashes it
/// recorded, as determinism needs, in a debug build as in a release one and on every platform.
/// `GG_UPDATE_REFERENCES=1` writes its hashes again, after a change meant to change the simulation
/// or what the hash reads; a change to a command's type that its lines no longer read as needs a new
/// recording.
#[test]
fn the_live_recording_replays_to_its_hashes() {
    let path = live_recording();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("can't read {}: {error}", path.display()));
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        let rewritten = gg_engine::Replay::rewrite(app(), &text);
        fs::write(&path, rewritten.unwrap_or_else(|error| panic!("{error:#}"))).expect("written");
        println!("wrote {}", path.display());
        return;
    }
    assert!(text.ends_with("\nend\n"), "the recording has its end");
    let mut replay = gg_engine::Replay::new(app(), &text);
    let replay = replay.as_mut().unwrap_or_else(|error| panic!("{error:#}"));
    let mut ticks = 0;
    while let Some(tick) = replay.step().unwrap_or_else(|error| panic!("{error:#}")) {
        ticks = tick.number + 1;
    }
    let hashes = text
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()));
    assert_eq!(ticks, hashes.count() as u64);
}

fn live_recording() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/references/live.recording")
}

/// A copy of the live recording whose first steer goes another way goes another way at that
/// command's tick, and the first difference between the two worlds there is the heading of the
/// player it steers, as a desync between two peers would be named.
#[test]
fn a_forced_divergence_names_its_tick_entity_and_field() {
    // Its hashes are being written again, by the test above.
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        return;
    }
    let text = fs::read_to_string(live_recording()).expect("the live recording");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let at = lines.iter().position(|line| {
        line.starts_with("> 0 Variant(\"Steer\"") && !line.contains("Vec2(0.0, 0.0)")
    });
    let at = at.expect("the recording steers");
    let x = lines[at].find("Vec2(").expect("a heading") + "Vec2(".len();
    let end = x + lines[at][x..].find(',').expect("its x");
    let old: f32 = lines[at][x..end].parse().expect("a float");
    let new = if old > 0.0 { old - 0.25 } else { old + 0.25 };
    lines[at].replace_range(x..end, &format!("{new:?}"));
    let tick = lines[at..]
        .iter()
        .find_map(|line| line.split_once(' ')?.0.parse::<u64>().ok());
    let tick = tick.expect("the command's tick");
    let changed: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let mut original = gg_engine::Replay::new(app(), &text).expect("attached");
    let mut copy = gg_engine::Replay::new(app(), &changed).expect("attached");
    let diverged = loop {
        let ran = original.step().expect("the recording replays");
        let ran = ran.expect("the copy goes another way before the end");
        if let Err(error) = copy.step() {
            break (ran, error);
        }
    };
    let (ran, error) = diverged;
    assert_eq!(error.code(), "replay.diverged", "{error}");
    assert_eq!(ran.number, tick, "{error}");
    let world = original.world();
    let steered = world
        .query::<(Entity, &Player)>()
        .find(|(_, player)| player.index == 0);
    let steered = world.id(steered.expect("player 0").0);
    let difference = world.difference(copy.world()).expect("a difference");
    assert_eq!(Some(difference.entity), steered);
    assert_eq!(difference.component.as_deref(), Some("Player"));
    assert_eq!(difference.path, "heading.x");
    assert_eq!(difference.theirs, Some(gg_engine::reflect::Value::F32(new)));
}
