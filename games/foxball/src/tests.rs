use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use gg_api::Project;
use gg_engine::cook::Kind;
use gg_engine::{BoxCollider3D, NavMeshVolume, Pad, Prefab, PrefabRoot, SphereCollider3D};

use super::*;

/// The game as it attaches, the project's as the app is, so that the world's hash doesn't depend on
/// where the repository is, its world's IDs drawn from `seed`: run alone, its bot queued for player
/// 1.
fn attached(seed: u64) -> (Context, Foxball) {
    let cx = Context::seeded(seed).project(env!("CARGO_MANIFEST_DIR"));
    let mut cx = cx.expect("the project reads");
    let mut game = Foxball::new();
    game.load(&mut cx);
    game.attach(&mut cx);
    (cx, game)
}

/// The game as it attaches, as [`attached`] has it, but with both foxes their players', as in a
/// session: the bot that a run alone queues dropped from the queue.
fn humans(seed: u64) -> (Context, Foxball) {
    let (cx, mut game) = attached(seed);
    let player = BOT_PLAYER;
    assert_eq!(
        game.commands,
        [Command::Bot { player }],
        "a run alone's bot"
    );
    game.commands.clear();
    (cx, game)
}

/// The path of the game's scene file `name`.
fn scene(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join(name)
}

/// Player `index`'s fox: where it is, and its state.
fn fox(world: &World, index: u8) -> (Vec3, Player) {
    let mut foxes = world.query::<(&Player, &Transform)>();
    let found = foxes.find(|(player, _)| player.index == index);
    let (player, at) = found.expect("the player");
    (at.translation, *player)
}

/// The ball: where it is, and how fast it goes.
fn ball(world: &World) -> (Vec3, Vec3) {
    let mut balls = world.query::<(&Ball, &Transform, &RigidBody3D)>();
    let (_, at, body) = balls.next().expect("the ball");
    (at.translation, body.velocity)
}

/// How many times the ball's emitter has burst.
fn ball_bursts(world: &World) -> u32 {
    let mut balls = world.query::<(&Ball, &ParticleEmitter)>();
    balls.next().expect("the ball's emitter").1.bursts
}

/// How many times each goal's mouth's emitter has burst, by the player it scores for.
fn goal_bursts(world: &World) -> [u32; 2] {
    let mut bursts = [0; 2];
    for (goal, emitter) in world.query::<(&Goal, &ParticleEmitter)>() {
        bursts[usize::from(goal.player)] += emitter.bursts;
    }
    bursts
}

/// Puts the ball at `at`, still.
fn put_ball(world: &mut World, at: Vec3) {
    for (_, transform, body) in world.query_mut::<(&Ball, &mut Transform, &mut RigidBody3D)>() {
        transform.translation = at;
        body.velocity = Vec3::ZERO;
    }
}

/// The game's project's manifest is canonical, writing back to its own bytes, and starts the game
/// in the arena, which loads and saves to its own bytes, as its walls' prefab does, so that a type
/// that changes under either fails here; every wall of the arena, each goal's pocket's too, is an
/// instance of that one prefab, colliding as the prefab's box; and one volume bakes its navmesh.
#[test]
fn the_arena_saves_as_it_is() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(dir.join(Project::FILE)).expect("the manifest");
    let project = Project::parse(&manifest).expect("the project reads");
    assert_eq!(project.text(), manifest);
    let path = scene("arena.ron");
    assert_eq!(project.start_path(dir), path);
    let text = fs::read_to_string(&path).expect("the arena");
    let cx = Context::seeded(1).project(dir);
    let mut cx = cx.expect("the project reads");
    Foxball::new().load(&mut cx);
    let roots = cx.load_scene_file(&path).expect("the arena loads");
    let saved = cx.world().save_scene(&roots).expect("saved");
    assert_eq!(saved, text);
    let walls = cx.world().query::<(&PrefabRoot, &BoxCollider3D)>();
    let prefabs: Vec<Prefab> = walls.map(|(&PrefabRoot(prefab), _)| prefab).collect();
    assert_eq!(
        prefabs.len(),
        12,
        "2 sides, 2 beside each mouth, 3 to each pocket"
    );
    assert!(prefabs.iter().all(|&prefab| prefab == prefabs[0]));
    assert_eq!(cx.world().query::<&NavMeshVolume>().count(), 1);
    let path = dir.join("prefabs/wall.prefab");
    let text = fs::read_to_string(&path).expect("the wall's prefab");
    let roots = cx.load_scene_file(&path).expect("the prefab loads");
    let saved = cx.world().save_scene(&roots).expect("saved");
    assert_eq!(saved, text);
}

/// Each texture, model, font, prefab and UI under the game's asset root has its ID file beside it,
/// committed with it, each holding an ID of its own, and no ID file is left without its asset, so
/// that every checkout gives each asset the ID the repository holds, which its scene names it by.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = Project::read(dir).expect("the project reads");
    let found = gg_api::asset::scan(&project.assets_dir(dir));
    assert_eq!(
        found.assets.len(),
        8,
        "the ball, the box, the tiles, the Fox and its texture, the font, the wall and the UI"
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
            None => panic!("{path} has no ID file, {path}.meta: commit it beside its file"),
        }
    }
    assert!(found.orphans.is_empty(), "{:?}", found.orphans);
}

/// The game builds the match and a fox for each player at its start, facing the goal it attacks,
/// owned by its player, its model under its capsule, standing as Survey plays alone, its state
/// machine to enter its first state, Idle, at its first tick.
#[test]
fn each_player_has_a_fox_at_its_start() {
    let (cx, _game) = attached(0x5eed);
    let world = cx.world();
    for (index, &(start, yaw)) in STARTS.iter().enumerate() {
        let (at, player) = fox(world, index as u8);
        assert_eq!((at, player.yaw), (start, yaw));
    }
    let owners: Vec<u8> = world.query::<&Owner>().map(|owner| owner.player).collect();
    assert_eq!(owners.len(), 2);
    let foxes: Vec<_> = world.query::<(&Animator, &AnimationStates)>().collect();
    assert_eq!(foxes.len(), 2);
    for (clips, machine) in foxes {
        assert_eq!(clips.0.len(), 1);
        assert_eq!(clips.0[0].clip, "Survey");
        assert_eq!(
            (&*machine.state, machine.states[0].name.as_str()),
            ("", "Idle")
        );
    }
    let found = world.query::<&Match>().next().expect("the match");
    assert_eq!((found.scores, found.wins), ([0; 2], [0; 2]));
}

/// The keys, a pad's stick and the mouse queue commands for the run's own player: W walks forward,
/// E turns right, Space kicks, and the mouse's motion with the cursor grabbed, by a click, turns,
/// its moves between two ticks adding up to one turn, and looks down, which is the view's alone;
/// a click with the cursor grabbed kicks, and Escape lets it go, after which the mouse turns
/// nothing. As the joiner, the keys walk player 1.
#[test]
fn the_keys_and_the_mouse_queue_commands_for_the_runs_own_player() {
    let (mut cx, mut game) = humans(0x5eed);
    let key = |key, pressed| Event::Key {
        key,
        pressed,
        repeat: false,
    };
    let click = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let motion = |x, y| Event::MouseMotion { x, y };
    for event in [
        key(Key::KeyW, true),
        key(Key::KeyE, true),
        key(Key::Space, true),
        key(Key::Space, false),
        motion(40.0, 0.0),
        click,
        motion(40.0, 20.0),
        motion(-10.0, 0.0),
        click,
        key(Key::Escape, true),
        motion(40.0, 0.0),
    ] {
        assert_eq!(game.event(&mut cx, &event), ControlFlow::Continue(()));
    }
    let player = 0;
    let Some(&Command::Turn { player: 0, by }) = game.commands.get(3) else {
        panic!("a turn fourth: {:?}", game.commands);
    };
    assert!((by - 30.0 * SENSITIVITY).abs() < 1e-6, "{by}");
    let expected = [
        Command::Walk {
            player,
            heading: Vec2::Y,
        },
        Command::Spin { player, rate: 1.0 },
        Command::Kick { player },
        Command::Turn { player, by },
        Command::Kick { player },
    ];
    assert_eq!(game.commands, expected);
    assert!((game.pitch - (VIEW_PITCH + 20.0 * SENSITIVITY)).abs() < 1e-6);
    assert!(!game.grab, "let go at Escape");
    game.commands.clear();
    // As the joiner, player 1, which faces -Z, W let go and the up arrow pressed.
    game.player = 1;
    for event in [key(Key::KeyW, false), key(Key::ArrowUp, true)] {
        let _ = game.event(&mut cx, &event);
    }
    let world = cx.world_mut();
    for number in 0..32 {
        game.tick(world, Tick { number });
    }
    assert!(fox(world, 1).0.z < STARTS[1].0.z - 1.0);
    assert_eq!(fox(world, 0).0.x, STARTS[0].0.x);
}

/// A kick sends the ball forward and up from in front of its fox, bursting the ball's emitter and
/// playing the kick's sound from it, and the fox kicks again only once its cooldown has passed; a
/// ball out of reach isn't kicked.
#[test]
fn a_kick_sends_the_ball_from_in_front_of_its_fox() {
    let (mut cx, mut game) = humans(0x5eed);
    let kick = game.assets.as_ref().expect("loaded").sounds.kick;
    let world = cx.world_mut();
    let (start, _) = STARTS[0];
    assert_eq!(ball_bursts(world), 0);
    put_ball(world, start + Vec3::new(0.0, -0.15, 0.8));
    game.commands.push(Command::Kick { player: 0 });
    game.tick(world, Tick { number: 0 });
    let (_, velocity) = ball(world);
    assert!(velocity.z > KICK_SPEED * 0.9, "{velocity}");
    assert!(velocity.y > 0.0, "{velocity}");
    assert_eq!(ball_bursts(world), 1);
    let ball_entity = world.query::<(Entity, &Ball)>().next().expect("the ball").0;
    let queued: Vec<_> = world.sounds_queued().collect();
    assert_eq!(queued, [(kick, Place::On(ball_entity))]);
    // Again at once: the cooldown holds it.
    put_ball(world, start + Vec3::new(0.0, -0.15, 0.8));
    game.commands.push(Command::Kick { player: 0 });
    game.tick(world, Tick { number: 1 });
    assert!(ball(world).1.z < 1.0, "not kicked again yet");
    // Out of reach, once the cooldown has passed.
    for number in 2..u64::from(KICK_EVERY) + 2 {
        game.tick(world, Tick { number });
    }
    put_ball(world, start + Vec3::new(3.0, -0.15, 0.0));
    game.commands.push(Command::Kick { player: 0 });
    game.tick(world, Tick { number: 100 });
    assert!(ball(world).1.length() < 1.0, "out of reach");
    assert_eq!(ball_bursts(world), 1, "a kick that misses bursts nothing");
}

/// A ball in the far goal scores for player 0, and in the near one for player 1, bursting the
/// emitter of the goal's mouth, playing the goal's sound where it went in and putting the ball and
/// the foxes back at their starts; the fifth goal wins the match, starting another; and a ball out
/// of the arena goes back to its start alone.
#[test]
fn goals_score_and_five_win_a_match() {
    let (mut cx, mut game) = humans(0x5eed);
    let sounds = game.assets.as_ref().expect("loaded").sounds;
    let world = cx.world_mut();
    let score = |world: &World| *world.query::<&Match>().next().expect("the match");
    let mut number = 0;
    let mut tick = |game: &mut Foxball, world: &mut World| {
        game.tick(world, Tick { number });
        number += 1;
    };
    put_ball(world, Vec3::new(0.5, 0.4, FIELD.y + 1.0));
    tick(&mut game, world);
    assert_eq!(score(world).scores, [1, 0]);
    assert_eq!(goal_bursts(world), [1, 0]);
    assert_eq!(ball(world).0, BALL_START);
    assert_eq!(fox(world, 0).0, STARTS[0].0);
    let far =
        |(sound, place)| sound == sounds.goal && matches!(place, Place::At(at) if at.z > FIELD.y);
    assert!(world.sounds_queued().any(far), "from the far goal");
    put_ball(world, Vec3::new(-0.5, 0.4, -FIELD.y - 1.0));
    tick(&mut game, world);
    assert_eq!(score(world).scores, [1, 1]);
    assert_eq!(goal_bursts(world), [1, 1]);
    for goals in 2..=WINS_AT {
        put_ball(world, Vec3::new(0.0, 0.4, FIELD.y + 1.0));
        tick(&mut game, world);
        if goals < WINS_AT {
            assert_eq!(score(world).scores, [goals, 1]);
        }
    }
    let won = score(world);
    assert_eq!((won.scores, won.wins), ([0, 0], [1, 0]));
    assert!(world.sounds_queued().any(|(sound, _)| sound == sounds.win));
    // Over a side wall, and gone.
    put_ball(world, Vec3::new(FIELD.x + 5.0, 0.4, 3.0));
    let (moved, _) = fox(world, 1);
    tick(&mut game, world);
    assert_eq!(ball(world).0, BALL_START);
    assert_eq!(score(world).wins, [1, 0]);
    let across = |at: Vec3| Vec2::new(at.x, at.z);
    assert_eq!(
        across(fox(world, 1).0),
        across(moved),
        "the foxes stay where they are"
    );
}

/// Player `index`'s fox's state machine: the state it's in, its speed, and the plays of its
/// animator, which it drives, each its clip and weight.
fn machine(world: &World, index: u8) -> (String, f32, Vec<(String, f32)>) {
    let mut roots = world.query::<(&Parent, &Animator, &AnimationStates)>();
    let found = roots.find(|&(&Parent(body), ..)| {
        let player = world.get::<Player>(body);
        player.is_some_and(|player| player.index == index)
    });
    let (_, clips, machine) = found.expect("the fox's model's root");
    let speed = match machine.param(SPEED) {
        Some(ParamValue::Float(speed)) => speed,
        other => panic!("its speed is {other:?}"),
    };
    let plays = clips.0.iter().map(|play| (play.clip.clone(), play.weight));
    (machine.state.clone(), speed, plays.collect())
}

/// A fox's state machine goes as fast as the fox does: Idle, Survey alone, as it stands, with no
/// dust; Move as it runs, Run alone once the fade is done, the clips it doesn't play gone from its
/// animator, its dust at its most; and Idle again as it stops, Survey alone once the fade is done,
/// the dust gone.
#[test]
fn a_foxs_state_machine_runs_and_stands_as_it_goes() {
    let (mut cx, mut game) = humans(0x5eed);
    let world = cx.world_mut();
    let dust = |world: &World| {
        let dusts = world.query::<(&Parent, &ParticleEmitter)>();
        dusts.map(|(_, dust)| dust.rate).collect::<Vec<_>>()
    };
    let survey = vec![("Survey".to_owned(), 1.0)];
    game.tick(world, Tick { number: 0 });
    for index in 0..2 {
        assert_eq!(
            machine(world, index),
            ("Idle".to_owned(), 0.0, survey.clone())
        );
    }
    assert_eq!(dust(world), [0.0; 2]);
    let walk = |game: &mut Foxball, heading| {
        let walks = (0..2).map(|player| Command::Walk { player, heading });
        game.commands.extend(walks);
    };
    walk(&mut game, Vec2::Y);
    game.tick(world, Tick { number: 1 });
    let (state, speed, _) = machine(world, 0);
    assert_eq!(state, "Move", "at the tick it runs");
    assert!((speed - RUN_SPEED).abs() < 1e-4, "{speed}");
    // The fade's 0.2 seconds, 13 ticks, and some.
    for number in 2..20 {
        game.tick(world, Tick { number });
    }
    for index in 0..2 {
        let (state, _, plays) = machine(world, index);
        assert_eq!(
            (state.as_str(), plays),
            ("Move", vec![("Run".to_owned(), 1.0)])
        );
    }
    assert_eq!(dust(world), [DUST_RATE; 2]);
    walk(&mut game, Vec2::ZERO);
    game.tick(world, Tick { number: 20 });
    assert_eq!(machine(world, 0).0, "Idle", "at the tick it stops");
    for number in 21..40 {
        game.tick(world, Tick { number });
    }
    for index in 0..2 {
        assert_eq!(
            machine(world, index),
            ("Idle".to_owned(), 0.0, survey.clone())
        );
    }
    assert_eq!(dust(world), [0.0; 2]);
}

/// Run alone, player 1's fox is the game's bot's from the first tick, and player 0's its player's:
/// the bot's agent plans a path over the arena's navmesh, its fox runs to the ball, as its state
/// machine's Move, kicks it, as a player's kick does, and scores in player 0's goal within 20
/// seconds, past player 0's fox standing at its start in front of it, where a shot at the goal's
/// middle would hit it, the ball never touching that fox from the bot's last kick to the goal; and
/// a world taken back with the bot in it queues no other.
#[test]
fn the_bot_runs_to_the_ball_kicks_it_and_scores() {
    let (mut cx, mut game) = attached(0x5eed);
    let world = cx.world_mut();
    game.tick(world, Tick { number: 0 });
    let bots: Vec<u8> = (world.query::<(&Player, &Bot, &NavAgent)>())
        .map(|(player, ..)| player.index)
        .collect();
    assert_eq!(bots, [BOT_PLAYER]);
    let (_, _, agent) = world
        .query::<(&Bot, &Transform, &NavAgent)>()
        .next()
        .expect("its agent");
    assert!(
        agent.destination.is_some() && !agent.corners.is_empty(),
        "{agent:?}"
    );
    let balls = world.query::<(&Ball, &SphereCollider3D, &Transform)>();
    let (_, sphere, at) = balls.into_iter().next().expect("the ball");
    // How near the ball's middle comes to the fox's, across, where they touch.
    let touching = sphere.radius * at.scale.x + CAPSULE.1;
    // How near the ball came to player 0's fox, across, since the bot's last kick.
    let (mut moved, mut kicked, mut kicks, mut nearest) = (false, None, 0, f32::INFINITY);
    let scored = (1..64 * 20).find(|&number| {
        game.tick(world, Tick { number });
        if world.query::<&Match>().next().expect("the match").scores[1] > 0 {
            return true;
        }
        moved |= machine(world, BOT_PLAYER).0 == "Move";
        if ball_bursts(world) > kicks {
            kicks = ball_bursts(world);
            kicked.get_or_insert(number);
            nearest = f32::INFINITY;
        }
        nearest = nearest.min(flat(ball(world).0 - fox(world, 0).0).length());
        false
    });
    assert!(moved, "it ran there");
    let kicked = kicked.expect("it kicked the ball");
    let scored = scored.expect("it scored within 20 seconds");
    assert!(kicked <= scored, "{kicked} {scored}");
    assert!(
        nearest > touching,
        "past player 0's fox: {nearest}, {touching}"
    );
    assert_eq!(
        goal_bursts(world),
        [0, 1],
        "in player 0's goal, for player 1"
    );
    game.commands.clear();
    game.restored(&mut cx);
    let bot = game
        .commands
        .iter()
        .any(|command| matches!(command, Command::Bot { .. }));
    assert!(!bot, "{:?}", game.commands);
}

/// A session's runs, a lockstep host's, its joiner's and a server's, queue no bot as their layers
/// attach or pick up from a world, as a joiner's from its host's, and no fox has one as they tick,
/// both foxes their players'; while a run alone that picks up from a world without one, as the
/// world as edited that Stop brings back, queues it again.
#[test]
fn a_sessions_runs_have_no_bot() {
    let bot = |game: &Foxball| {
        let mut commands = game.commands.iter();
        commands.any(|command| matches!(command, Command::Bot { .. }))
    };
    for player in [Some(0), Some(1), None] {
        let cx = Context::seeded(0x5eed).project(env!("CARGO_MANIFEST_DIR"));
        let mut cx = cx.expect("the project reads").in_session(player);
        let mut game = Foxball::new();
        game.load(&mut cx);
        game.attach(&mut cx);
        assert!(!bot(&game), "{player:?}, attached: {:?}", game.commands);
        game.restored(&mut cx);
        assert!(!bot(&game), "{player:?}, restored: {:?}", game.commands);
        let world = cx.world_mut();
        for number in 0..8 {
            game.tick(world, Tick { number });
        }
        let bots = world.query::<&Bot>().count() + world.query::<&NavAgent>().count();
        assert_eq!(bots, 0, "{player:?}");
    }
    let (mut cx, mut game) = humans(0x5eed);
    game.restored(&mut cx);
    let player = BOT_PLAYER;
    assert!(game.commands.contains(&Command::Bot { player }), "alone");
}

/// Escape with the cursor free, or a pad's Start, opens the menu, this run's own, and closes it,
/// its fox standing while it's open, its walk and turn queued at rest as it opens and as the keys
/// are held as it closes, the keys walking it no more until then; the arrows move the menu's focus
/// and Enter presses: New match queues the player's command, which the next tick applies, starting
/// the match afresh, Resume closes the menu and Quit ends the run. Escape with the cursor grabbed
/// lets it go, opening nothing, and Escape with the menu open closes it.
#[test]
fn the_menu_opens_and_its_choices_queue_commands() {
    let (mut cx, mut game) = humans(0x5eed);
    let ui = game.assets.as_ref().expect("loaded").ui;
    game.update(&mut cx, Duration::from_millis(16));
    let press = |game: &mut Foxball, cx: &mut Context, key| {
        for pressed in [true, false] {
            let event = Event::Key {
                key,
                pressed,
                repeat: false,
            };
            let _ = game.event(cx, &event);
        }
    };
    let player = 0;
    let at_rest = [
        Command::Walk {
            player,
            heading: Vec2::ZERO,
        },
        Command::Spin { player, rate: 0.0 },
    ];
    let held = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    let _ = game.event(&mut cx, &held);
    press(&mut game, &mut cx, Key::Escape);
    assert!(game.menu);
    let walk = Command::Walk {
        player,
        heading: Vec2::Y,
    };
    assert_eq!(game.commands, [&[walk][..], &at_rest].concat());
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("resume"), "the file's");
    game.commands.clear();
    let let_go = Event::Key {
        key: Key::KeyW,
        pressed: false,
        repeat: false,
    };
    let _ = game.event(&mut cx, &let_go);
    press(&mut game, &mut cx, Key::ArrowDown);
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("new_match"));
    assert!(game.commands.is_empty(), "{:?}", game.commands);
    press(&mut game, &mut cx, Key::Space);
    assert!(!game.menu, "closed by its choice");
    let new_match = Command::NewMatch { player };
    assert_eq!(game.commands, [&[new_match][..], &at_rest].concat());
    let world = cx.world_mut();
    if let Some(found) = world.query_mut::<&mut Match>().next() {
        (found.scores, found.wins) = ([3, 2], [1, 0]);
    }
    for (_, at) in world.query_mut::<(&Player, &mut Transform)>() {
        at.translation.x += 2.0;
    }
    game.tick(world, Tick { number: 0 });
    let found = *world.query::<&Match>().next().expect("the match");
    assert_eq!((found.scores, found.wins), ([0; 2], [0; 2]));
    // Back at its start, as the tick began, which physics settles on the floor.
    assert_eq!(flat(fox(world, 0).0), flat(STARTS[0].0));

    // A click grabs the cursor, and Escape lets it go, opening nothing; a pad's Start opens the
    // menu and closes it, as Escape does.
    let click = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let _ = game.event(&mut cx, &click);
    assert!(game.grab);
    press(&mut game, &mut cx, Key::Escape);
    assert!(!game.grab && !game.menu);
    let start = |game: &mut Foxball, cx: &mut Context| {
        for pressed in [true, false] {
            let event = Event::PadButton {
                pad: Pad(0),
                button: PadButton::Start,
                pressed,
            };
            let _ = game.event(cx, &event);
        }
    };
    start(&mut game, &mut cx);
    assert!(game.menu);
    start(&mut game, &mut cx);
    assert!(!game.menu);
    press(&mut game, &mut cx, Key::Escape);
    press(&mut game, &mut cx, Key::Escape);
    assert!(!game.menu, "Escape asks the open menu for back");
    press(&mut game, &mut cx, Key::Escape);
    press(&mut game, &mut cx, Key::ArrowUp);
    press(&mut game, &mut cx, Key::Enter);
    assert!(!game.menu, "resumed");
    assert!(!cx.exiting());
    press(&mut game, &mut cx, Key::Escape);
    for key in [Key::ArrowDown, Key::ArrowDown, Key::Enter] {
        press(&mut game, &mut cx, key);
    }
    assert!(cx.exiting(), "quit");
}

/// A world restored from a snapshot simulates as it did from there, through walks, turns, kicks,
/// goals and the bot's play: the game keeps nothing of the match but in the world.
#[test]
fn a_restored_world_simulates_as_before() {
    let (mut cx, mut game) = attached(0x5eed);
    let world = cx.world_mut();
    for number in 0..96 {
        script(&mut game, world, number);
        game.tick(world, Tick { number });
    }
    let snapshot = world.clone();
    let run = |game: &mut Foxball, world: &mut World| {
        let hashes = (96..288).map(|number| {
            script(game, world, number);
            game.tick(world, Tick { number });
            world.hash()
        });
        hashes.collect::<Vec<_>>()
    };
    let first = run(&mut game, world);
    world.clone_from(&snapshot);
    let again = run(&mut game, world);
    let parted = first.iter().zip(&again).position(|(a, b)| a != b);
    assert_eq!(parted.map(|tick| 96 + tick), None, "the first tick to part");
}

/// The commands a script queues before tick `number`, what player 0's keys and mouse would, as
/// `world` stands, player 1 the bot's: every 4 ticks, its fox turns toward a point just behind the
/// ball, as seen from the goal it attacks, and runs there, a little faster than the bot, so that
/// the two don't meet at the ball each time, or, once it's there, turns toward that goal and runs
/// into the ball, kicking it once it's near; so that they push, kick and score.
fn script(game: &mut Foxball, world: &World, number: u64) {
    if !number.is_multiple_of(4) {
        return;
    }
    let (ball_at, _) = ball(world);
    let player = 0;
    let (at, fox) = fox(world, player);
    let goal = Vec3::new(0.0, ball_at.y, FIELD.y + 1.0);
    let behind = ball_at - (goal - ball_at).normalize_or_zero() * 0.9;
    let to_behind = flat(behind - at);
    let lined_up = to_behind.length() < 0.6;
    let toward = if lined_up { flat(goal - at) } else { to_behind };
    let wanted = atan2f(toward.x, toward.z);
    game.commands.extend([
        Command::Turn {
            player,
            by: wrapped(fox.yaw - wanted),
        },
        Command::Walk {
            player,
            heading: Vec2::Y,
        },
    ]);
    if flat(ball_at - at).length() < 1.2 && lined_up {
        game.commands.push(Command::Kick { player });
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
             or what the hash reads, `GG_UPDATE_REFERENCES=1 cargo test -p foxball` writes it \
             again, but a difference on some platforms alone is their simulations going apart",
            path.display(),
            lines().filter(|(line, other)| line != other).count(),
            run.len()
        );
    }
    assert_eq!(run.len(), reference.lines().count(), "{}", path.display());
}

/// The game's simulation from a seed, run alone, as lines of the world's hash, its content then its
/// layout: one after it attaches, and one at each of 768 ticks, between which the script queues
/// what player 0's keys and mouse would, the bot playing player 1.
fn simulate() -> Vec<String> {
    let (mut cx, mut game) = attached(0x5eed);
    let hashed = |at: &dyn std::fmt::Display, world: &World| format!("{at} {}", world.hash());
    let mut lines = vec![hashed(&"attach", cx.world())];
    let world = cx.world_mut();
    for number in 0..768 {
        script(&mut game, world, number);
        game.tick(world, Tick { number });
        lines.push(hashed(&number, world));
    }
    let found = world.query::<&Match>().next().expect("the match");
    assert!(
        found.scores.iter().sum::<u32>() + found.wins.iter().sum::<u32>() > 0,
        "the script scores a goal, so that the reference covers scoring"
    );
    lines
}

/// The live recording, `tests/references/live.recording`, of a run alone walked, turned and kicked
/// by its keys and its mouse against the bot, started afresh by its menu's New match, and edited
/// through the tool API, replays without a GPU to the hashes it recorded, as determinism needs, in
/// a debug build as in a release one and on every platform. `GG_UPDATE_REFERENCES=1` writes its
/// hashes again, after a change meant to change the simulation or what the hash reads; a change to
/// a command's type that its lines no longer read as needs a new recording.
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

/// The game's package, as `tools/cook` cooks it, holds what its runs load: its scene, its wall's
/// prefab, its UI and its sounds as they are, the Fox's texture compressed, its models
/// cooked and its font baked, its assets by their IDs; and the live recording replays from it to
/// its hashes, as a build without `dev` runs from it.
#[test]
fn the_live_recording_replays_from_the_games_package() {
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("foxball-package-{}", std::process::id()));
    let package = dir.join("gg-foxball.ggpk");
    let game = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cooked = cook::cook(game, &package).unwrap_or_else(|error| panic!("{error:#}"));
    let held: Vec<(&str, Kind, bool)> = (cooked.iter())
        .map(|cooked| (&*cooked.path, cooked.kind, cooked.id.is_some()))
        .collect();
    let expected = [
        ("fonts/Inter-Bold.ttf", Kind::Font, true),
        ("models/ball.gltf", Kind::Model, true),
        ("models/box.gltf", Kind::Model, true),
        ("models/fox/Fox.gltf", Kind::Model, true),
        ("models/fox/Texture.png", Kind::Texture, true),
        ("models/tiles.gltf", Kind::Model, true),
        ("prefabs/wall.prefab", Kind::File, true),
        ("scenes/arena.ron", Kind::File, false),
        ("sounds/goal.wav", Kind::File, false),
        ("sounds/kick.wav", Kind::File, false),
        ("sounds/win.wav", Kind::File, false),
        ("ui/menu.ui", Kind::File, true),
    ];
    assert_eq!(held, expected);
    let text = fs::read_to_string(live_recording()).expect("the live recording");
    let app = app()
        .package(&package)
        .unwrap_or_else(|error| panic!("{error:#}"));
    let mut replay = gg_engine::Replay::new(app, &text);
    let replay = replay.as_mut().unwrap_or_else(|error| panic!("{error:#}"));
    while replay
        .step()
        .unwrap_or_else(|error| panic!("{error:#}"))
        .is_some()
    {}
    let _ = fs::remove_dir_all(dir);
}

/// A copy of the live recording whose first walk goes another way goes another way at that
/// command's tick, and the first difference between the two worlds there is on the fox of the
/// player it walks, the velocity its heading gave its character, as a desync between two peers
/// would be named.
#[test]
fn a_forced_divergence_names_its_tick_entity_and_field() {
    // Its hashes are being written again, by the test above.
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        return;
    }
    let text = fs::read_to_string(live_recording()).expect("the live recording");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let at = lines.iter().position(|line| {
        line.starts_with("> 0 Variant(\"Walk\"") && !line.contains("Vec2(0.0, 0.0)")
    });
    let at = at.expect("the recording walks");
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
    let walked = world
        .query::<(Entity, &Player)>()
        .find(|(_, player)| player.index == 0);
    let walked = world.id(walked.expect("player 0").0);
    let difference = world.difference(copy.world()).expect("a difference");
    assert_eq!(Some(difference.entity), walked);
    // Its character's velocity, which its heading set at the tick, its component's name first.
    assert_eq!(
        difference.component.as_deref(),
        Some("CharacterController3D")
    );
    assert_eq!(difference.path, "velocity.x");
    let heading = |world: &World| fox(world, 0).1.heading;
    let turned = Vec2::new(new, 1.0).clamp_length_max(1.0);
    assert_eq!(heading(copy.world()), turned);
    assert_ne!(heading(world), turned);
}
