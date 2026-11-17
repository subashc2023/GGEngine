use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use gg_api::Project;
use gg_engine::cook::Kind;

use super::*;

/// The game as it attaches, rooted at its project as the app is, so the world's hash doesn't depend
/// on where the repository is. IDs are drawn from `seed`.
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
    found
        .expect("the player")
        .1
        .translation
        .truncate()
        .as_vec2()
}

/// The path of the game's scene file `name`.
fn scene(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join(name)
}

/// The sample mod, `mods/rainbow`, runs with what the game grants, giving each shard a `Rainbow`
/// that turns its color through the hues. A mod asking for more is refused, and the game runs
/// without it.
#[test]
fn the_sample_mod_cycles_the_shards_colors_and_a_greedy_one_is_refused() {
    let game = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("shards-mods-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for name in ["rainbow", "greedy"] {
        fs::create_dir_all(dir.join(name)).expect("a directory");
    }
    for file in ["mod.ron", "rainbow.luau"] {
        let from = game.join("mods/rainbow").join(file);
        fs::copy(from, dir.join("rainbow").join(file)).expect("copied");
    }
    let greedy = fs::read_to_string(game.join("mods/rainbow/mod.ron")).expect("the manifest");
    let greedy = greedy.replace("\"rainbow\"", "\"greedy\"");
    let greedy = greedy.replace("\"write:Sprite\"", "\"write:Match\"");
    fs::write(dir.join("greedy/mod.ron"), greedy).expect("written");
    fs::copy(
        game.join("mods/rainbow/rainbow.luau"),
        dir.join("greedy/rainbow.luau"),
    )
    .expect("copied");
    let cx = Context::seeded(1).project(game).expect("the project reads");
    let mut cx = cx.mods("Shards", &dir);
    let mut shards = Shards::new();
    shards.load(&mut cx);
    shards.attach(&mut cx);
    for number in 0..16 {
        shards.tick(cx.world_mut(), Tick { number });
    }
    let world = cx.world();
    let shards: Vec<Entity> = world
        .query::<(Entity, &Shard)>()
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(shards.len(), SHARDS);
    // A quarter turn a second, once a tick: the greedy mod, which would turn it too, was refused.
    let hue = 0.25 * 16.0 * Tick::DT.as_secs_f64();
    let colored = Vec4::new(1.0, (hue * 6.0) as f32, 0.0, 1.0);
    for shard in shards {
        let rainbow = world
            .get_field(shard, "Rainbow", "hue")
            .expect("its own Rainbow");
        assert_eq!(rainbow, gg_engine::reflect::Value::F64(hue));
        let color = world.get::<Sprite>(shard).expect("a sprite").color;
        assert!(color.abs_diff_eq(colored, 1e-6), "{color}");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The manifest is canonical, writing back to its own bytes, and starts the game in the arena.
#[test]
fn the_project_is_canonical_and_starts_in_the_arena() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(dir.join(Project::FILE)).expect("the manifest");
    let project = Project::parse(&text).expect("the project reads");
    assert_eq!(project.text(), text);
    assert_eq!(project.start_path(dir), scene("arena.ron"));
}

/// Each texture and model under the game's asset root has a committed ID file beside it, each ID
/// its own, and no ID file lacks its asset. So every checkout gives each asset the repository's ID,
/// rather than its runs writing others.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = Project::read(dir).expect("the project reads");
    let found = gg_api::asset::scan(&project.assets_dir(dir));
    assert!(
        found.assets.len() >= 2,
        "the Pillar's texture and the crystal"
    );
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

/// The arena's scene loads, with the textures and model it names, and saves to its own bytes, so a
/// game type changing under it fails here. Its crystal's root spawns the model's two nodes, which
/// the scene doesn't hold.
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
    let nodes = cx.world().query::<&gg_engine::ModelNode>().count();
    assert_eq!(nodes, 2, "the crystal and its splinter");
}

/// The game builds both players at their starts, and shards apart from them. A touched shard scores
/// and turns up elsewhere; the tenth wins the match, sending the players back to their starts.
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
        assert!(
            at.truncate().distance((STARTS[player]).as_dvec2()) > 1.5,
            "{at}"
        );
    }
    // Player 0 onto the shard.
    let mut players = world.query_mut::<(&Player, &mut Transform)>();
    let (_, disc) = players
        .find(|(player, _)| player.index == 0)
        .expect("player 0");
    disc.translation = at.truncate().extend(f64::from(PLAYER_Z));
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
    disc.translation = moved.truncate().extend(f64::from(PLAYER_Z));
    shards.tick(world, Tick { number: 1 });
    let found = *world.query::<&Match>().next().expect("the match");
    assert_eq!((found.scores, found.wins), ([0, 0], [1, 0]));
    assert!(player_at(world, 0).distance(STARTS[0]) < 0.2);
}

/// Ticks queue the game's sounds: a dash on its disc, none while it cools down; a collect where the
/// shard was; and the fanfare for a match won.
#[test]
fn the_ticks_queue_the_games_sounds() {
    let (mut cx, mut shards) = attached(0x5eed);
    let sounds = shards.sounds.expect("loaded");
    let world = cx.world_mut();
    let mut players = world.query::<(Entity, &Player)>();
    let disc = players
        .find(|(_, player)| player.index == 0)
        .expect("player 0");
    let disc = disc.0;
    for number in 0..2 {
        shards.commands.push(Command::Dash { player: 0 });
        shards.tick(world, Tick { number });
    }
    let queued: Vec<_> = world.sounds_queued().collect();
    assert_eq!(queued, [(sounds.dash, Place::On(disc))]);
    let shard = world.query::<(Entity, &Shard)>().next().expect("a shard").0;
    let at = world.get::<Transform>(shard).expect("placed").translation;
    let found = world.query_mut::<&mut Match>().next().expect("the match");
    found.scores = [WINS_AT - 1, 0];
    let disc = world.get_mut::<Transform>(disc).expect("player 0");
    disc.translation = at.truncate().extend(f64::from(PLAYER_Z));
    shards.tick(world, Tick { number: 2 });
    let queued: Vec<_> = world.sounds_queued().skip(1).collect();
    let won = [
        (sounds.collect, Place::At(at)),
        (sounds.win, Place::Nowhere),
    ];
    assert_eq!(queued, won);
}

/// Keys and the left stick queue steers and dashes for the run's own player as they come, so the
/// frame's ticks apply them: player 0 alone, player 1 as the joiner. A tick steers only the player
/// its commands name.
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

/// Escape opens the menu, letting go of the disc's steer. While it's open, the keys move its focus,
/// wrapping at either end, and choose, rather than steer and dash. Enter on New match queues one
/// and closes the menu; Space on Resume closes it without a dash; each closing steers the disc as
/// the keys are held. Escape also closes it, and Quit asks the run to end.
#[test]
fn the_menu_takes_the_keys_while_its_open() {
    let (mut cx, mut shards) = attached(0x5eed);
    let key = |key, pressed| Event::Key {
        key,
        pressed,
        repeat: false,
    };
    let press = |shards: &mut Shards, cx: &mut Context, name: Key| {
        for pressed in [true, false] {
            assert_eq!(
                shards.event(cx, &key(name, pressed)),
                ControlFlow::Continue(())
            );
        }
    };
    // Held throughout, so each opening lets go of its steer and each closing takes it up.
    let _ = shards.event(&mut cx, &key(Key::KeyD, true));
    press(&mut shards, &mut cx, Key::Escape);
    assert_eq!(shards.menu, Some(0));
    for (name, focus) in [
        (Key::ArrowDown, 1),
        (Key::KeyS, 2),
        (Key::ArrowDown, 0),
        (Key::KeyW, 2),
        (Key::ArrowUp, 1),
    ] {
        press(&mut shards, &mut cx, name);
        assert_eq!(shards.menu, Some(focus), "after {name:?}");
    }
    press(&mut shards, &mut cx, Key::Enter);
    assert_eq!(shards.menu, None);
    press(&mut shards, &mut cx, Key::Escape);
    press(&mut shards, &mut cx, Key::Space);
    assert_eq!(shards.menu, None);
    press(&mut shards, &mut cx, Key::Escape);
    press(&mut shards, &mut cx, Key::Escape);
    assert_eq!(shards.menu, None);
    let player = 0;
    let [steer, stop] = [Vec2::X, Vec2::ZERO].map(|heading| Command::Steer { player, heading });
    let expected = [
        steer.clone(),
        stop.clone(),
        Command::NewMatch,
        steer.clone(),
        stop.clone(),
        steer.clone(),
        stop.clone(),
        steer.clone(),
    ];
    assert_eq!(shards.commands, expected);
    assert!(!cx.exiting());
    press(&mut shards, &mut cx, Key::Escape);
    press(&mut shards, &mut cx, Key::ArrowUp);
    press(&mut shards, &mut cx, Key::Enter);
    assert!(cx.exiting(), "Quit ends the run");
}

/// The music plays from the game's load, quieter while the menu's open, and full again once it
/// closes.
#[test]
fn the_music_plays_quieter_under_the_menu() {
    let (mut cx, mut shards) = attached(0x5eed);
    assert!(cx.music_playing().is_some());
    assert_eq!(cx.volume(Bus::Music), 1.0);
    for (open, volume) in [(true, MUSIC_UNDER_MENU), (false, 1.0)] {
        for pressed in [true, false] {
            let escape = Event::Key {
                key: Key::Escape,
                pressed,
                repeat: false,
            };
            let _ = shards.event(&mut cx, &escape);
        }
        assert_eq!(shards.menu.is_some(), open);
        assert_eq!(cx.volume(Bus::Music), volume);
    }
}

/// A new match puts both players at their starts, at rest, and every score and win at 0.
#[test]
fn a_new_match_starts_afresh() {
    let (mut cx, mut shards) = attached(0x5eed);
    let world = cx.world_mut();
    let found = world.query_mut::<&mut Match>().next().expect("the match");
    (found.scores, found.wins) = ([7, 3], [2, 1]);
    for (_, at, body) in world.query_mut::<(&Player, &mut Transform, &mut RigidBody2D)>() {
        at.translation.y += 2.0;
        body.velocity = Vec2::new(3.0, -1.0);
    }
    shards.commands.push(Command::NewMatch);
    shards.tick(world, Tick { number: 0 });
    let found = *world.query::<&Match>().next().expect("the match");
    assert_eq!((found.scores, found.wins), ([0; 2], [0; 2]));
    for player in [0, 1] {
        let at = player_at(world, player);
        assert!(at.distance(STARTS[usize::from(player)]) < 0.2, "{at}");
    }
}

/// The cursor finds each item over its button, to its corners, and none beside or between them.
#[test]
fn the_menu_finds_the_item_under_the_cursor() {
    for index in 0..ITEMS.len() {
        let middle = item_middle(index);
        let inside = BUTTON / 2.0 - 1.0;
        for at in [middle, middle - inside, middle + inside] {
            assert_eq!(item_at(at), Some(index), "{at}");
        }
        let beside = middle + Vec2::new(BUTTON.x / 2.0 + 1.0, 0.0);
        assert_eq!(item_at(beside), None);
    }
    assert_eq!(item_at((item_middle(0) + item_middle(1)) / 2.0), None);
}

/// A world restored from a snapshot simulates as before, through steers, dashes, bumps and
/// collections: the game keeps the match in the world alone.
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

/// Queues what both players' keys might before tick `number`, as `world` stands. Every 8 ticks,
/// player 0 steers for the nearest shard and player 1 for player 0, dashing now and then, so they
/// collect shards and bump.
fn script(shards: &mut Shards, world: &World, number: u64) {
    if !number.is_multiple_of(8) {
        return;
    }
    let [ours, theirs] = [0, 1].map(|index| player_at(world, index));
    let shard = world
        .query::<(&Shard, &Transform)>()
        .map(|(_, at)| at.translation.truncate());
    let nearest = shard.min_by(|a, b| {
        a.distance(ours.as_dvec2())
            .total_cmp(&b.distance(ours.as_dvec2()))
    });
    let toward = |from: Vec2, to: Vec2| (to - from).normalize_or_zero();
    shards.commands.extend([
        Command::Steer {
            player: 0,
            heading: toward(ours, nearest.unwrap_or(ours.as_dvec2()).as_vec2()),
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

/// The game's simulation, run without a GPU from a seed through a script's commands, hashes at
/// attach and each tick as its reference says. Determinism, and so lockstep, needs it the same run
/// to run, on four threads as on one, in debug and release builds, and on every platform.
#[test]
fn the_simulation_gives_its_reference_hashes() {
    let run = simulate();
    let on_four = gg_engine::jobs::on_threads(4, simulate);
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

/// The game's simulation from a seed, as lines of the world's hash, content then layout: one after
/// attach, and one at each of 512 ticks, between which the script queues commands.
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

/// The live recording, of a windowed run steered by its keys, edited through the tool API and its
/// scene file saved, replays without a GPU to its hashes, in debug and release builds and on every
/// platform. Shards' AGENTS.md says which changes need a new recording instead.
#[test]
fn the_live_recording_replays_to_its_hashes() {
    let path = live_recording();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("can't read {}: {error}", path.display()));
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        let rewritten = gg_engine::recording::Replay::rewrite(app(), &text);
        fs::write(&path, rewritten.unwrap_or_else(|error| panic!("{error:#}"))).expect("written");
        println!("wrote {}", path.display());
        return;
    }
    assert!(text.ends_with("\nend\n"), "the recording has its end");
    let mut replay = gg_engine::recording::Replay::new(app(), &text);
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

/// The game's package, as `tools/cook` cooks it, holds what its runs load but its mods, its assets
/// by their IDs. The live recording replays from it to its hashes, as a build without `dev` runs.
#[test]
fn the_live_recording_replays_from_the_games_package() {
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("shards-package-{}", std::process::id()));
    let package = dir.join("gg-shards.ggpk");
    let game = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cooked = cook::cook(game, &package).unwrap_or_else(|error| panic!("{error:#}"));
    let held: Vec<(&str, Kind, bool)> = (cooked.iter())
        .map(|cooked| (&*cooked.path, cooked.kind, cooked.id.is_some()))
        .collect();
    let expected = [
        ("fonts/Inter-Bold.ttf", Kind::Font, true),
        ("models/crystal.gltf", Kind::Model, true),
        ("music/match.ogg", Kind::File, false),
        ("scenes/arena.ron", Kind::File, false),
        ("scripts/spin.luau", Kind::File, false),
        ("shaders/floor.slang", Kind::Shader, false),
        ("sounds/choose.wav", Kind::File, false),
        ("sounds/collect.wav", Kind::File, false),
        ("sounds/dash.wav", Kind::File, false),
        ("sounds/focus.wav", Kind::File, false),
        ("sounds/win.wav", Kind::File, false),
        ("textures/pillar.png", Kind::Texture, true),
    ];
    assert_eq!(held, expected);
    let text = fs::read_to_string(live_recording()).expect("the live recording");
    let app = app()
        .package(&package)
        .unwrap_or_else(|error| panic!("{error:#}"));
    let mut replay = gg_engine::recording::Replay::new(app, &text);
    let replay = replay.as_mut().unwrap_or_else(|error| panic!("{error:#}"));
    while replay
        .step()
        .unwrap_or_else(|error| panic!("{error:#}"))
        .is_some()
    {}
    let _ = fs::remove_dir_all(dir);
}

/// A copy of the live recording whose first steer goes another way diverges at that command's
/// tick, and the worlds' first difference is the steered player's heading, as a desync between
/// peers would be named.
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
    let mut original = gg_engine::recording::Replay::new(app(), &text).expect("attached");
    let mut copy = gg_engine::recording::Replay::new(app(), &changed).expect("attached");
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
