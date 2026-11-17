use std::fs;
use std::time::SystemTime;

use gg_api::asset::{AssetKind, Meta};
use gg_api::probe::Probe;
use gg_api::view::Writer;
use gg_api::{By, Failure, Look, Looked, Outline, Scene, SceneAction, Status};
use gg_engine::Key;
use gg_engine::grid::Plane;
use gg_engine::math::{DVec2, Quat};
use gg_engine::reflect::{self, Reflect};
use gg_engine::{MouseButton, Name, Projection, Scroll, Transform};

use super::input::forwarded;
use super::logs::LOGGED;
use super::run::gg_run;
use super::*;
use crate::assets::tests::{Scratch, find, texts};
use crate::camera::{Move, Tool};
use crate::file::Choice;
use crate::look::Parts;
use crate::overlay::Mode;
use crate::tests::{SCALE, VIEW, reading};
use crate::view::{OUTLINE, OUTLINE_WIDTH};
use gg_engine::api::api_viewpoint;

/// The game runs embedded and starts stopped either way; `--release-game` adds `--release` before
/// the game's own arguments.
#[test]
fn the_game_runs_optimized_where_the_editor_is_given_release_game() {
    let as_built = ["run", "--embedded", "--", "--stopped"];
    assert_eq!(gg_run(false), as_built);
    let optimized = ["run", "--release", "--embedded", "--", "--stopped"];
    assert_eq!(gg_run(true), optimized);
}

/// The game's ends of [`serving`]'s layer's connections: requests and their answers, picks and
/// theirs.
struct Ends {
    asked: Receiver<(String, Value)>,
    answer: Sender<(String, Result<Value, Failure>)>,
    picks: Receiver<(u64, [f64; 2], Option<EntityId>)>,
    found: Sender<(u64, Result<gg_api::Picked, Failure>)>,
    probes: Receiver<(u64, [f64; 2])>,
    read: Sender<(u64, Result<Probe, Failure>)>,
}

/// A layer with a game serving, as `serve` leaves it, told to play through its camera; and the
/// game's ends.
fn serving() -> (Game, Ends) {
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    let (asks, asked) = mpsc::channel();
    let (answer, answers) = mpsc::channel();
    let (_, logged) = mpsc::channel();
    let (picks, picking) = mpsc::channel();
    let (found, picked) = mpsc::channel();
    let (probes, probing) = mpsc::channel();
    let (read, probed) = mpsc::channel();
    game.talker = Some(Talker {
        asks,
        answers,
        logged,
        picks,
        picked,
        probes,
        probed,
    });
    game.run_state = RunState::Running;
    (game.stopped, game.told, game.served) = (false, true, true);
    let ends = Ends {
        asked,
        answer,
        picks: picking,
        found,
        probes: probing,
        read,
    };
    (game, ends)
}

/// The game's answer to a `status`, `play`, `stop`, `pause`, `resume` or `step`.
fn status(stopped: bool, paused: bool) -> Result<Value, Failure> {
    let status = Status {
        ticks: 0,
        paused,
        stopped,
        frames: 1,
        entities: 0,
        view: None,
        scene: None,
        unsaved: false,
        edited: false,
        cells: None,
    };
    Ok(json!(status))
}

/// The game's answer to a `status` or a `scene`, stopped, its open scene's file `scene`, `unsaved`
/// or not.
fn scene_status(scene: Option<&str>, unsaved: bool) -> Result<Value, Failure> {
    let status = Status {
        ticks: 0,
        paused: false,
        stopped: true,
        frames: 1,
        entities: 0,
        view: None,
        scene: scene.map(str::to_owned),
        unsaved,
        edited: unsaved,
        cells: None,
    };
    Ok(json!(status))
}

fn scenes(ends: &Ends) -> Vec<Scene> {
    let asked = ends
        .asked
        .try_iter()
        .filter(|(method, _)| method == "scene");
    let scenes = asked.map(|(_, scene)| serde_json::from_value(scene).expect("a scene"));
    scenes.collect()
}

/// The `scene` request saving the open scene, to `path` or to its file.
fn save(path: Option<PathBuf>) -> Scene {
    Scene {
        action: SceneAction::Save,
        path,
        discard: false,
    }
}

fn methods(ends: &Ends) -> Vec<String> {
    ends.asked.try_iter().map(|(method, _)| method).collect()
}

fn looks(ends: &Ends) -> Vec<Look> {
    let asked = ends.asked.try_iter().filter(|(method, _)| method == "look");
    let looks = asked.map(|(_, look)| serde_json::from_value(look).expect("a look"));
    looks.collect()
}

/// In Play, all the view gets goes to the game, in the view's pixels, whatever the tool, the view
/// staying on the game's camera. That includes what the editor's camera takes otherwise: its
/// buttons, Alt with the left, the left under Pan or Orbit, the wheel and Home. A press asks for no
/// pick. Stopped, through the game's camera, nothing goes: a press off the handles asks for a pick,
/// and what the editor's camera takes turns the view to it. In Simulate nothing goes either, and Q,
/// W, E and R pick tools. Turning back to the game's camera while it plays gives the game the
/// cursor, if it's over the view.
#[test]
fn in_play_the_game_gets_all_the_views_input() {
    let (mut game, ends) = serving();
    // [`VIEW`]'s top left is at (100, 50).
    let (at, in_game) = ([300.0, 200.0], [200.0, 150.0]);
    let moved = Event::CursorMoved { x: at[0], y: at[1] };
    let w = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    let press = |button, pressed| Event::MouseButton { button, pressed };
    let left = [
        press(MouseButton::Left, true),
        press(MouseButton::Left, false),
    ];
    for event in [moved, w, left[0], left[1]] {
        let routed = game.route(&event, &reading(at), Some(VIEW));
        assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
    }
    let button = |pressed| InputEvent::Button {
        button: gg_api::Button::Left,
        pressed,
    };
    let w_down = InputEvent::Key {
        key: "KeyW".to_owned(),
        pressed: true,
        repeat: false,
    };
    let expected = [
        InputEvent::Cursor(in_game),
        w_down,
        button(true),
        button(false),
    ];
    assert_eq!(game.input, expected);
    assert_eq!(ends.picks.try_iter().collect::<Vec<_>>(), [], "no pick");
    assert_eq!(game.view.asked, None);
    assert_eq!(game.view.toolbar.tool, Tool::Select, "the game's W");
    let home = Event::Key {
        key: Key::Home,
        pressed: true,
        repeat: false,
    };
    let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
    let mut alt = reading(at);
    alt.screen.alt = true;
    for (event, read, tool) in [
        (press(MouseButton::Middle, true), reading(at), Tool::Select),
        (press(MouseButton::Right, true), reading(at), Tool::Select),
        (left[0], alt, Tool::Select),
        (left[0], reading(at), Tool::Pan),
        (left[0], reading(at), Tool::Orbit),
        (wheel, reading(at), Tool::Select),
        (home, reading(at), Tool::Select),
    ] {
        let (mut game, ends) = serving();
        game.view.toolbar.tool = tool;
        let routed = game.route(&event, &read, Some(VIEW));
        assert_eq!(routed, ControlFlow::Continue(()), "{event:?} with {tool:?}");
        assert!(game.view.toolbar.world_camera, "{event:?} with {tool:?}");
        let sent = forwarded(&event, VIEW.origin).expect("the game gets it");
        assert_eq!(game.input, [sent], "{event:?}");
        assert_eq!(ends.picks.try_iter().count(), 0, "{event:?} with {tool:?}");
    }
    // Stopped, through the game's camera.
    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    for event in [moved, w, left[0], left[1]] {
        let routed = game.route(&event, &reading(at), Some(VIEW));
        assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
    }
    assert_eq!(
        ends.picks.try_iter().collect::<Vec<_>>(),
        [(0, in_game, None)]
    );
    assert!(game.view.toolbar.world_camera);
    let routed = game.route(&wheel, &reading(at), Some(VIEW));
    assert_eq!(routed, ControlFlow::Break(()));
    assert!(!game.view.toolbar.world_camera, "the editor's camera");
    assert_eq!(game.input, []);
    // In Simulate, through the editor's camera.
    let (mut game, ends) = serving();
    game.view.toolbar.world_camera = false;
    game.tell(Some(at), Some(VIEW));
    assert_eq!(game.input, [InputEvent::Focused(false)], "the game lets go");
    game.input.clear();
    for event in [moved, w, left[0], left[1], wheel] {
        let _ = game.route(&event, &reading(at), Some(VIEW));
    }
    assert_eq!(game.input, []);
    assert_eq!(game.view.toolbar.tool, Tool::Move, "the editor's W");
    assert_eq!(
        ends.picks.try_iter().collect::<Vec<_>>(),
        [(0, in_game, None)]
    );
    // Back to the game's camera, with the toolbar's switch, for which a test sets the field
    // and tells the game, since it can't run `ui`: the cursor over the view, and off it.
    for (cursor, told) in [
        (at, vec![InputEvent::Cursor(in_game)]),
        ([20.0, 20.0], vec![]),
    ] {
        game.view.toolbar.world_camera = false;
        game.tell(Some(cursor), Some(VIEW));
        game.input.clear();
        game.view.toolbar.world_camera = true;
        game.tell(Some(cursor), Some(VIEW));
        assert_eq!(game.input, told, "at {cursor:?}");
    }
}

/// Play and Simulate each ask the game to play, whatever the editor last knew, Play turning the
/// view to the game's camera and Simulate to the editor's, so a switch carries on. Stop asks it to
/// stop. Until it answers, no view input goes to the game, unsent input included, but its letting
/// go. The answer ends that, refused, playing on, or stopped, the view turning to the editor's
/// camera; not if a Play clicked since awaits its answer.
#[test]
fn play_and_simulate_ask_the_game_to_play_and_stop_to_stop() {
    let (mut game, ends) = serving();
    let at = [300.0, 200.0];
    let w = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    assert_eq!(game.mode(), Mode::Play);
    for (to, through) in [
        (Mode::Simulate, false),
        (Mode::Play, true),
        (Mode::Play, true),
    ] {
        game.run(to);
        assert_eq!(methods(&ends), ["play"], "{to:?}");
        assert_eq!(game.view.toolbar.world_camera, through, "{to:?}");
        assert_eq!(game.mode(), to);
        let played = ("play".to_owned(), status(false, false));
        ends.answer.send(played).expect("sent");
        game.take_answers();
    }
    let _ = game.route(&w, &reading(at), Some(VIEW));
    assert_eq!(game.input.len(), 1, "W goes to the game");
    game.run(Mode::Edit);
    assert_eq!(methods(&ends), ["stop"]);
    assert_eq!(game.mode(), Mode::Edit, "until it answers");
    for _ in 0..2 {
        let _ = game.route(&w, &reading(at), Some(VIEW));
    }
    assert_eq!(
        game.input,
        [InputEvent::Focused(false)],
        "its letting go alone"
    );
    let refused = Failure::new("api.no_stop", "a recorded run plays from start to end");
    ends.answer
        .send(("stop".to_owned(), Err(refused)))
        .expect("sent");
    game.take_answers();
    assert_eq!(game.mode(), Mode::Play, "refused");
    game.run(Mode::Edit);
    let stopped = ("stop".to_owned(), status(true, false));
    ends.answer.send(stopped).expect("sent");
    game.take_answers();
    assert!(!game.stopping);
    assert_eq!(game.mode(), Mode::Edit, "stopped");
    assert!(!game.view.toolbar.world_camera, "the editor's camera");
    // Stop, then Play before Stop's answer, which, to the request before, leaves the view
    // where Play turned it, the play's answer finding it there.
    game.run(Mode::Play);
    let played = ("play".to_owned(), status(false, false));
    ends.answer.send(played).expect("sent");
    game.take_answers();
    game.run(Mode::Edit);
    game.run(Mode::Play);
    for (method, stopped) in [("stop", true), ("play", false)] {
        let answer = (method.to_owned(), status(stopped, false));
        ends.answer.send(answer).expect("sent");
    }
    game.take_answers();
    assert_eq!(game.mode(), Mode::Play, "the last click's");
    assert!(game.view.toolbar.world_camera, "the game's camera");
}

/// The run controls ask for what they show: Pause and Resume while it plays; Play from stopped,
/// which first resumes a game paused since before its stop; Step a step, a stopped game paused and
/// played first, through the camera the view shows.
#[test]
fn the_controls_pause_resume_and_step_the_game() {
    let (mut game, ends) = serving();
    game.click(overlay::Clicked::Pause);
    game.click(overlay::Clicked::Resume);
    assert_eq!(methods(&ends), ["pause", "resume"]);
    game.paused = true;
    game.click(overlay::Clicked::Step);
    assert_eq!(methods(&ends), ["step"], "paused");
    for (paused, asked) in [(true, &["resume", "play"][..]), (false, &["play"])] {
        (game.stopped, game.paused) = (true, paused);
        game.click(overlay::Clicked::To(Mode::Play));
        assert_eq!(methods(&ends), asked, "paused {paused}");
    }
    for world_camera in [true, false] {
        (game.stopped, game.paused) = (true, false);
        game.view.toolbar.world_camera = world_camera;
        game.click(overlay::Clicked::Step);
        assert_eq!(methods(&ends), ["pause", "play", "step"]);
        assert_eq!(
            game.view.toolbar.world_camera, world_camera,
            "the view's camera"
        );
    }
}

/// Another client's `play`, `pause` and `stop` reach the editor in its `status` polls. Playing:
/// Play or Simulate lights as the camera switch says, and in Play the view's input goes to the
/// game. Stopped: the view turns to the editor's camera, unsent input dropped and the game letting
/// go of what's held; the next look sets the editor's viewpoint and the grid, though the user
/// didn't turn the view. A game answering stopped as it starts leaves the view as it was.
#[test]
fn another_clients_play_and_stop_reach_the_view() {
    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"]);
    ends.answer
        .send(("status".to_owned(), status(true, false)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.label(), "Stopped");
    assert!(game.view.toolbar.world_camera, "as it starts");
    ends.answer
        .send(("status".to_owned(), status(false, true)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.mode(), Mode::Play);
    assert_eq!(game.label(), "Playing, paused");
    game.view.toolbar.world_camera = false;
    assert_eq!(game.label(), "Simulating, paused", "by the switch");
    game.view.toolbar.world_camera = true;
    let w = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    let _ = game.route(&w, &reading([300.0, 200.0]), Some(VIEW));
    let w_down = InputEvent::Key {
        key: "KeyW".to_owned(),
        pressed: true,
        repeat: false,
    };
    assert_eq!(game.input, std::slice::from_ref(&w_down));
    ends.answer
        .send(("status".to_owned(), status(true, false)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.mode(), Mode::Edit);
    assert!(!game.view.toolbar.world_camera, "the editor's camera");
    let sent: Vec<_> = ends.asked.try_iter().collect();
    let asked = |method: &str| {
        let request = sent.iter().find(|(asked, _)| asked == method);
        request
            .unwrap_or_else(|| panic!("no {method}: {sent:?}"))
            .1
            .clone()
    };
    let events: Vec<InputEvent> =
        serde_json::from_value(asked("input")["events"].clone()).expect("events");
    assert_eq!(events, [InputEvent::Focused(false)], "the W not sent going");
    let look: Look = serde_json::from_value(asked("look")).expect("a look");
    let expected = Look {
        viewpoint: Some(Some(api_viewpoint(&game.view.camera.viewpoint()))),
        grid: Some(Some(gg_api::Plane::Xy)),
        ..Look::default()
    };
    assert_eq!(look, expected);
}

/// A stop the game answers drops the unsent input meant for it, but the letting go a turn from Play
/// to Simulate queued after it. Not while a Play clicked since awaits its answer: the input then
/// goes to the copy Play starts.
#[test]
fn a_stop_drops_the_input_not_sent_but_the_letting_go() {
    let at = [300.0, 200.0];
    let w = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    let w_down = InputEvent::Key {
        key: "KeyW".to_owned(),
        pressed: true,
        repeat: false,
    };
    // In Play, W, and the camera switch's turn to Simulate, before either is sent.
    let (mut game, ends) = serving();
    let _ = game.route(&w, &reading(at), Some(VIEW));
    game.view.toolbar.world_camera = false;
    game.tell(Some(at), Some(VIEW));
    assert_eq!(game.input, [w_down.clone(), InputEvent::Focused(false)]);
    ends.answer
        .send(("status".to_owned(), status(true, false)))
        .expect("sent");
    game.take_answers();
    assert_eq!(
        game.input,
        [InputEvent::Focused(false)],
        "the W not sent going"
    );
    // In Simulate, Play, and W, before the answer to a status asked before the play, which
    // says another client stopped the game.
    let (mut game, ends) = serving();
    game.view.toolbar.world_camera = false;
    game.tell(Some(at), Some(VIEW));
    game.input.clear();
    game.run(Mode::Play);
    game.tell(Some(at), Some(VIEW));
    let _ = game.route(&w, &reading(at), Some(VIEW));
    ends.answer
        .send(("status".to_owned(), status(true, false)))
        .expect("sent");
    game.take_answers();
    assert!(
        game.input.contains(&w_down),
        "for the copy: {:?}",
        game.input
    );
    assert!(game.view.toolbar.world_camera, "the game's camera");
}

/// The editor asks the game's `status` as a game serves and every quarter second after, one at a
/// time.
#[test]
fn the_game_is_asked_how_it_runs_a_status_at_a_time() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let mut asks = |game: &mut Game| {
        game.update(&mut cx, Duration::ZERO);
        let asked = ends
            .asked
            .try_iter()
            .filter(|(method, _)| method == "status");
        asked.count()
    };
    // As though the last was asked a quarter second earlier.
    let ago = |game: &mut Game| {
        game.polled = game.polled.and_then(|at| at.checked_sub(STATUS_EVERY));
    };
    assert_eq!(asks(&mut game), 1, "at once");
    ago(&mut game);
    assert_eq!(asks(&mut game), 0, "until the last is answered");
    let answered = || ("status".to_owned(), status(false, false));
    ends.answer.send(answered()).expect("sent");
    assert_eq!(asks(&mut game), 1, "a quarter second on");
    ends.answer.send(answered()).expect("sent");
    assert_eq!(asks(&mut game), 0, "until a quarter second on");
    ago(&mut game);
    assert_eq!(asks(&mut game), 1);
}

/// An Open goes at once while the last status said no unsaved edits. Refused for edits made since,
/// it prompts, logged but in no message; Don't Save asks again, discarding them, with the dialog's
/// path.
#[test]
fn a_refusal_for_unsaved_edits_prompts_and_dont_save_discards_them() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let arena = (
        "status".to_owned(),
        scene_status(Some("scenes/arena.ron"), false),
    );
    ends.answer.send(arena).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let level = PathBuf::from("scenes/level2.ron");
    game.open(level.clone());
    assert_eq!(game.prompt, None, "as the status said");
    game.update(&mut cx, Duration::ZERO);
    let open = |discard| Scene {
        action: SceneAction::Open,
        path: Some(level.clone()),
        discard,
    };
    assert_eq!(scenes(&ends), [open(false)]);
    let unsaved = Failure::new("scene.unsaved", "the open scene has unsaved edits");
    ends.answer
        .send(("scene".to_owned(), Err(unsaved)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.prompt, Some(Action::Open(level.clone())));
    assert!(game.message.is_none());
    assert_eq!(
        game.logs.back().map(|logged| logged.level),
        Some(Level::Info)
    );
    assert!(!game.files(), "while it prompts");
    game.answer(Choice::DontSave);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [open(true)]);
}

/// The prompt's Save saves the open scene, then redoes what it asked about, an Open with the
/// dialog's path. A refused save does nothing more, saying why in a message.
#[test]
fn the_prompts_save_saves_then_does_it_again() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let (arena, level2) = (Some("scenes/arena.ron"), Some("scenes/level2.ron"));
    let status = |scene, unsaved| ("status".to_owned(), scene_status(scene, unsaved));
    ends.answer.send(status(arena, true)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let level = PathBuf::from("scenes/level2.ron");
    game.open(level.clone());
    assert_eq!(
        game.prompt,
        Some(Action::Open(level.clone())),
        "as the status said"
    );
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [], "until it's answered");
    game.answer(Choice::Save);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [save(None)]);
    let saved = ("scene".to_owned(), scene_status(arena, false));
    ends.answer.send(saved).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let open = Scene {
        action: SceneAction::Open,
        path: Some(level.clone()),
        discard: false,
    };
    assert_eq!(scenes(&ends), [open]);
    let opened = ("scene".to_owned(), scene_status(level2, false));
    ends.answer.send(opened).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    // Edited, and opened again, the save refused.
    ends.answer.send(status(level2, true)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    game.open(level.clone());
    game.answer(Choice::Save);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [save(None)]);
    let changed = Failure::new("scene.changed", "scenes/level2.ron changed since");
    ends.answer
        .send(("scene".to_owned(), Err(changed)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let message = game.message.take().expect("refused");
    assert_eq!(message.title, "Can't save level2.ron");
    assert_eq!(
        message.text,
        "[scene.changed] scenes/level2.ron changed since"
    );
    assert_eq!(scenes(&ends), [], "nothing more");
    assert!(game.files());
}

/// Save As saves to the dialog's file, adding `.ron` unless it ends in it, in any case, as the XDG
/// portal's dialog may leave it. It refuses, saying so, a name with `.ron` added that a file
/// already has, which the dialog didn't ask about replacing.
#[test]
fn save_as_ends_the_name_in_ron_and_replaces_no_file_unasked() {
    let dir = std::env::temp_dir().join(format!("gg-editor-save-as-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).expect("made");
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    for (picked, saved) in [
        ("level", "level.ron"),
        ("level.1", "level.1.ron"),
        ("Boss.RON", "Boss.RON"),
    ] {
        game.save_as(dir.join(picked));
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(scenes(&ends), [save(Some(dir.join(saved)))], "{picked}");
        let answer = ("scene".to_owned(), scene_status(Some(saved), false));
        ends.answer.send(answer).expect("sent");
        game.update(&mut cx, Duration::ZERO);
    }
    std::fs::write(dir.join("kept.ron"), "kept").expect("written");
    game.save_as(dir.join("kept"));
    let message = game.message.take().expect("refused");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(message.title, "Can't save kept.ron");
    assert!(
        message.text.starts_with("[editor.scene_exists]"),
        "{message:?}"
    );
    assert!(game.request.is_none());
}

/// Save saves the open scene to the file the last status named. Refused as untitled, as after
/// another client's `new`, it goes to Save As, whose dialog the next UI shows, with no message.
#[test]
fn a_save_refused_as_untitled_goes_to_save_as() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let arena = (
        "status".to_owned(),
        scene_status(Some("scenes/arena.ron"), true),
    );
    ends.answer.send(arena).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    game.choose(&mut cx, Item::Save);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [save(None)]);
    let untitled = Failure::new("scene.untitled", "the open scene has no file");
    ends.answer
        .send(("scene".to_owned(), Err(untitled)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.due, Some(Item::SaveAs));
    assert!(game.message.is_none());
    assert!(!game.files(), "until the dialog shows");
}

/// A Save refused as untitled while a close is checked goes to no Save As, which would open in
/// the close's way; the close prompt's own Save, refused so, goes to Save As.
#[test]
fn a_save_refused_as_untitled_mid_close_goes_to_save_as_from_the_prompt_alone() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let status = |unsaved| {
        let scene = scene_status(Some("scenes/arena.ron"), unsaved);
        ("status".to_owned(), scene)
    };
    let untitled = || {
        let untitled = Failure::new("scene.untitled", "the open scene has no file");
        ("scene".to_owned(), Err(untitled))
    };
    ends.answer.send(status(true)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    game.choose(&mut cx, Item::Save);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [save(None)]);
    let close = Event::CloseRequested;
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
    ends.answer.send(untitled()).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.due, None, "while the close is checked");
    // The poll's answer, then the close's status's.
    for _ in 0..2 {
        ends.answer.send(status(true)).expect("sent");
        game.update(&mut cx, Duration::ZERO);
    }
    assert_eq!(game.prompt, Some(Action::Close));
    game.answer(Choice::Save);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(scenes(&ends), [save(None)]);
    ends.answer.send(untitled()).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.due, Some(Item::SaveAs), "the prompt's own Save");
}

/// A status saying the world has cells has each poll ask for them, the answer kept for the Scenes
/// panel. One saying it has none stops that, and forgets them.
#[test]
fn a_worlds_cells_are_asked_with_each_status() {
    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"], "a poll");
    let mut world = scene_status(Some("scenes/world.ron"), false).expect("a status");
    world["cells"] = json!({"in": 1, "kept": 0});
    ends.answer
        .send(("status".to_owned(), Ok(world)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    game.polled = None;
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status", "cells"]);
    let cells = json!({"world": {"entity": "00000000000000a0", "folder": "scenes/cells",
        "size": 64, "viewers": [], "cells": []}});
    ends.answer
        .send(("cells".to_owned(), Ok(cells)))
        .expect("sent");
    let plain = scene_status(Some("scenes/arena.ron"), false);
    ends.answer
        .send(("status".to_owned(), plain))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let folder = |game: &Game| game.cells.as_ref().map(|cells| cells.folder.clone());
    assert_eq!(folder(&game), None, "forgotten");
    game.polled = None;
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"]);
    let mut world = scene_status(Some("scenes/world.ron"), false).expect("a status");
    world["cells"] = json!({"in": 1, "kept": 0});
    ends.answer
        .send(("status".to_owned(), Ok(world)))
        .expect("sent");
    let cells = json!({"world": {"entity": "00000000000000a0", "folder": "scenes/cells",
        "size": 64, "viewers": [], "cells": []}});
    ends.answer
        .send(("cells".to_owned(), Ok(cells)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(folder(&game).as_deref(), Some("scenes/cells"), "kept");
}

/// A close is vetoed while a game serves, until a status asked after it says whether there are
/// unsaved edits; one asked before doesn't count. Then it's prompted for: Cancel drops it, and
/// Don't Save grants it, letting the next close through.
#[test]
fn a_close_asks_a_fresh_status_and_prompts_for_unsaved_edits() {
    let (mut game, ends) = serving();
    // Stopped, as it starts, so that it asks for statuses alone.
    (game.stopped, game.told) = (true, false);
    let mut cx = Context::seeded(1);
    let close = Event::CloseRequested;
    let status = |unsaved| {
        let scene = scene_status(Some("scenes/arena.ron"), unsaved);
        ("status".to_owned(), scene)
    };
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"], "a poll");
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
    game.update(&mut cx, Duration::ZERO);
    assert!(methods(&ends).is_empty(), "a status at a time");
    ends.answer.send(status(false)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.closing, Closing::Checking(true), "the poll's skipped");
    assert_eq!(methods(&ends), ["status"]);
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
    ends.answer.send(status(true)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.prompt, Some(Action::Close));
    game.answer(Choice::Cancel);
    assert_eq!(game.closing, Closing::No);
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"]);
    ends.answer.send(status(true)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    game.answer(Choice::DontSave);
    assert_eq!(game.closing, Closing::Granted);
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
}

/// While no game serves after one has, a close goes by the last status: at once if it said no
/// unsaved edits, else prompted for without Save, Don't Save granting it, as for a close checked as
/// the game goes. Before any game serves, it goes at once.
#[test]
fn a_close_while_no_game_serves_goes_by_the_last_status() {
    let mut cx = Context::seeded(1);
    let close = Event::CloseRequested;
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
    for checking in [false, true] {
        let (mut game, ends) = serving();
        let unsaved = scene_status(Some("scenes/arena.ron"), true);
        ends.answer
            .send(("status".to_owned(), unsaved))
            .expect("sent");
        game.update(&mut cx, Duration::ZERO);
        if checking {
            assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        }
        // As `gg run` restarts the game.
        game.disconnect();
        if !checking {
            assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
        }
        assert_eq!(game.prompt, Some(Action::Close), "checking: {checking}");
        game.answer(Choice::Save);
        assert_eq!(game.prompt, Some(Action::Close), "no game to save");
        game.answer(Choice::DontSave);
        assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
    }
    let (mut game, _ends) = serving();
    game.disconnect();
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
}

/// Neither the File menu's request nor a close's status goes while the Inspector has an edit
/// waiting, which goes on its own connection. Once it has none, both go, the request first.
#[test]
fn nothing_goes_while_the_inspector_has_edits_waiting() {
    let (mut game, ends) = serving();
    // Stopped, as it starts, so that it asks for statuses alone.
    (game.stopped, game.told) = (true, false);
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    let arena = (
        "status".to_owned(),
        scene_status(Some("scenes/arena.ron"), false),
    );
    ends.answer.send(arena).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["status"], "a poll");
    let id: EntityId = "0123456789abcdef".parse().expect("an ID");
    let x = gg_engine::reflect::Value::F32(1.0);
    game.view.link.set(id, "Transform", "translation.x", x);
    game.choose(&mut cx, Item::Save);
    assert_eq!(
        game.event(&mut cx, &Event::CloseRequested),
        ControlFlow::Break(())
    );
    game.update(&mut cx, Duration::ZERO);
    assert!(methods(&ends).is_empty(), "the edit first");
    assert_eq!(game.closing, Closing::Checking(false));
    // The Inspector's pass sends the set, and the game answers it, for which a test gives the
    // layer a link with nothing waiting.
    game.view.link = Inspector::remote(CLIENT).link();
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(methods(&ends), ["scene", "status"]);
    assert_eq!(game.closing, Closing::Checking(true));
}

/// A New or an Open turns the view to the game's camera, as at start, to frame the game's new
/// scene. That holds for one that stopped a copy playing, though the stop turns the view to the
/// editor's camera as another client's `stop` does; and for one from the editor's camera, as a Stop
/// leaves it, the next look giving the viewpoint and the grid back to the game. Not one refused,
/// nor one answered before a Simulate clicked since. The title bar names the open scene's file once
/// a game has served, Untitled after a New, marked while unsaved.
#[test]
fn a_new_or_an_open_turns_the_view_to_the_games_camera() {
    let game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    assert_eq!(game.scene_label(), None, "before a game serves");
    // Playing, through the game's camera.
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let arena = scene_status(Some("scenes/arena.ron"), true);
    let playing = arena.map(|mut status| {
        status["stopped"] = json!(false);
        status
    });
    ends.answer
        .send(("status".to_owned(), playing))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.scene_label().as_deref(), Some("arena.ron*"));
    assert_eq!(game.mode(), Mode::Play);
    game.act(Action::New);
    game.answer(Choice::DontSave);
    game.update(&mut cx, Duration::ZERO);
    let new = Scene {
        action: SceneAction::New,
        path: None,
        discard: true,
    };
    assert_eq!(scenes(&ends), [new]);
    let emptied = ("scene".to_owned(), scene_status(None, false));
    ends.answer.send(emptied).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.mode(), Mode::Edit);
    assert!(game.view.toolbar.world_camera, "the game's camera");
    assert_eq!(game.scene_label().as_deref(), Some("Untitled"));
    // Through the editor's camera, for which a test sets the switch, the game's look holding
    // its viewpoint and the grid.
    game.view.toolbar.world_camera = false;
    game.update(&mut cx, Duration::ZERO);
    let sent = looks(&ends);
    let [sent] = &sent[..] else {
        panic!("a look: {sent:?}");
    };
    let (Some(viewpoint), Some(grid)) = (sent.viewpoint.flatten(), sent.grid.flatten()) else {
        panic!("the editor's viewpoint and grid: {sent:?}");
    };
    let held = Looked {
        viewpoint: Some(viewpoint),
        outline: None,
        grid: Some(grid),
        colliders: None,
        show: None,
        skip: None,
        by: By {
            viewpoint: Some(CLIENT.to_owned()),
            outline: None,
            grid: Some(CLIENT.to_owned()),
            colliders: None,
            show: None,
            skip: None,
        },
        revision: 1,
        run: 1,
        seen: None,
    };
    ends.answer
        .send(("look".to_owned(), Ok(json!(held))))
        .expect("sent");
    let level = PathBuf::from("scenes/level2.ron");
    let opened = || {
        let level2 = scene_status(Some("scenes/level2.ron"), false);
        ("scene".to_owned(), level2)
    };
    game.open(level.clone());
    game.update(&mut cx, Duration::ZERO);
    let missing = Failure::new("scene.no_file", "scenes/level2.ron isn't there");
    ends.answer
        .send(("scene".to_owned(), Err(missing)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert!(game.message.take().is_some(), "refused");
    assert!(!game.view.toolbar.world_camera, "refused");
    // A Simulate clicked after the Open was asked, which its answer comes before.
    game.open(level.clone());
    game.update(&mut cx, Duration::ZERO);
    game.run(Mode::Simulate);
    ends.answer.send(opened()).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert!(!game.view.toolbar.world_camera, "as Simulate turned it");
    ends.answer
        .send(("play".to_owned(), status(false, false)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.mode(), Mode::Simulate);
    game.run(Mode::Edit);
    ends.answer
        .send(("stop".to_owned(), status(true, false)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let _ = looks(&ends);
    game.open(level);
    game.update(&mut cx, Duration::ZERO);
    ends.answer.send(opened()).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert!(game.view.toolbar.world_camera, "the game's camera");
    let back = Look {
        viewpoint: Some(None),
        grid: Some(None),
        ..Look::default()
    };
    assert_eq!(looks(&ends), [back]);
}

/// The game's camera, which the view sees through at start and after a New or an Open, only frames
/// the scene. Stopped, the first frame drawn through it turns the view to the editor's camera from
/// where it saw, the handles and the grid showing, its viewpoint going to the game with the grid.
/// Not a frame drawn before the game gave its viewpoint back, whose answer it waits for; nor
/// through the toolbar's switch, which the user turned.
#[test]
fn the_first_frame_through_the_games_camera_turns_the_view_to_the_editors() {
    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    let reader = Reader::create(4, 4).expect("memory");
    let mut writer = Writer::open(reader.name()).expect("opened");
    game.reader = Some(reader);
    // A viewpoint at `position`, turned as the editor's camera starts, 4 units tall.
    let orthographic = |position| gg_api::Viewpoint {
        position,
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Orthographic {
            height: 4.0,
            near: -40.0,
            far: 40.0,
        },
    };
    let mut frame = |viewpoint, look| {
        let published = writer.publish([4, 4], 0, SystemTime::now(), viewpoint, look, |_| {});
        published.expect("published");
    };
    // The game answers a look whose viewpoint and grid the editor set, or else the game's own,
    // at `revision`.
    let answer = |parts: Parts, revision| {
        let mine = |set: bool| set.then(|| CLIENT.to_owned());
        let looked = Looked {
            by: By {
                viewpoint: mine(parts.viewpoint.is_some()),
                grid: mine(parts.grid.is_some()),
                ..By::default()
            },
            viewpoint: parts.viewpoint,
            outline: None,
            grid: parts.grid,
            colliders: None,
            show: None,
            skip: None,
            revision,
            run: 1,
            seen: None,
        };
        let answer = ("look".to_owned(), Ok(json!(looked)));
        ends.answer.send(answer).expect("sent");
    };
    let mut cx = Context::seeded(1);
    let mut update = |game: &mut Game| game.update(&mut cx, Duration::ZERO);
    assert!(game.view.toolbar.world_camera, "as it starts");
    frame(orthographic([1.0, 2.0, 5.0]), 0);
    update(&mut game);
    assert!(!game.view.toolbar.world_camera, "the editor's camera");
    let seen = game.view.camera.viewpoint();
    assert_eq!(seen.position.truncate(), DVec2::new(1.0, 2.0), "as it saw");
    update(&mut game);
    let mine = Parts {
        viewpoint: Some(api_viewpoint(&game.view.camera.viewpoint())),
        grid: Some(gg_api::Plane::Xy),
        ..Parts::default()
    };
    let sent = Look {
        viewpoint: Some(mine.viewpoint),
        grid: Some(mine.grid),
        ..Look::default()
    };
    assert_eq!(looks(&ends), [sent]);
    answer(mine.clone(), 1);
    update(&mut game);
    // An Open, done as a frame drawn from the editor's viewpoint comes.
    game.open(PathBuf::from("scenes/level2.ron"));
    update(&mut game);
    let opened = scene_status(Some("scenes/level2.ron"), false);
    ends.answer
        .send(("scene".to_owned(), opened))
        .expect("sent");
    frame(mine.viewpoint.expect("a viewpoint"), 1);
    update(&mut game);
    assert!(
        game.view.toolbar.world_camera,
        "until the game gives it back"
    );
    let _ = looks(&ends);
    answer(Parts::default(), 2);
    frame(orthographic([-3.0, 1.0, 4.0]), 2);
    update(&mut game);
    assert!(!game.view.toolbar.world_camera, "the editor's camera");
    let seen = game.view.camera.viewpoint();
    assert_eq!(seen.position.truncate(), DVec2::new(-3.0, 1.0), "as it saw");
    update(&mut game);
    assert_eq!(looks(&ends).len(), 1, "its viewpoint and the grid");
    answer(mine, 3);
    update(&mut game);
    // The toolbar's switch, for which a test sets the field.
    game.view.toolbar.world_camera = true;
    update(&mut game);
    answer(Parts::default(), 4);
    frame(orthographic([0.0, 0.0, 4.0]), 4);
    for _ in 0..2 {
        update(&mut game);
    }
    assert!(game.view.toolbar.world_camera, "the user's turn");
}

/// In Play, a press or repeat of N, O or S with the command key, a File menu shortcut, goes neither
/// to the game nor to the view's tools. Its release does, as do other keys with it, and those keys
/// without it.
#[test]
fn the_file_menus_shortcuts_keys_dont_reach_the_game() {
    let (mut game, _ends) = serving();
    let at = [300.0, 200.0];
    let mut command = reading(at);
    command.command = true;
    let key = |key, pressed, repeat| Event::Key {
        key,
        pressed,
        repeat,
    };
    for event in [
        key(Key::KeyS, true, false),
        key(Key::KeyS, true, true),
        key(Key::KeyN, true, false),
        key(Key::KeyO, true, false),
    ] {
        let routed = game.route(&event, &command, Some(VIEW));
        assert_eq!(routed, ControlFlow::Continue(()), "{event:?}");
    }
    assert_eq!(game.input, []);
    let _ = game.route(&key(Key::KeyS, false, false), &command, Some(VIEW));
    let _ = game.route(&key(Key::KeyW, true, false), &command, Some(VIEW));
    let _ = game.route(&key(Key::KeyS, true, false), &reading(at), Some(VIEW));
    let sent = |key: &str, pressed| InputEvent::Key {
        key: key.to_owned(),
        pressed,
        repeat: false,
    };
    let expected = [sent("KeyS", false), sent("KeyW", true), sent("KeyS", true)];
    assert_eq!(game.input, expected);
}

/// Started outside a project, none opened before, the layer opens and runs nothing: the hub shows,
/// the File menu's scene items and the Scenes panel do nothing, and the Assets and Scenes panels
/// have no folder.
#[test]
fn outside_a_project_it_runs_nothing_and_shows_the_hub() {
    let mut game = Game::new(None, Inspector::remote(CLIENT).link());
    game.attach(&mut Context::seeded(1));
    assert!(game.run.is_none() && game.talker.is_none() && game.dir.is_none());
    assert!(game.hub_shows() && !game.files());
    assert_eq!(game.label(), "No project open");
    assert!(game.assets.is_none() && game.scenes.is_none());
}

/// The hub's pick of another project goes as a close does: at once before any game has served.
/// Otherwise, with unsaved edits by a fresh status, the prompt asks to save them first: Cancel
/// keeps the project, and Don't Save grants the switch rather than the editor's close. Closing the
/// editor while the prompt shows is the editor's own close. Picking the open project opens nothing,
/// the hub giving way to it.
#[test]
fn another_project_opens_as_a_close_goes() {
    let other = PathBuf::from("other");
    let mut fresh = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    fresh.switch_to(other.clone());
    assert_eq!(fresh.closing, Closing::Granted, "no game has served");
    assert_eq!(fresh.switching.as_ref(), Some(&other));
    let mut cx = Context::seeded(1);
    let status = |unsaved| {
        let scene = scene_status(Some("scenes/arena.ron"), unsaved);
        ("status".to_owned(), scene)
    };
    let prompted = |game: &mut Game, ends: &Ends, cx: &mut Context| {
        game.switch_to(other.clone());
        game.update(cx, Duration::ZERO);
        assert_eq!(methods(ends), ["status"]);
        ends.answer.send(status(true)).expect("sent");
        game.update(cx, Duration::ZERO);
        assert_eq!(game.prompt, Some(Action::Close));
    };
    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    game.hub.asked = true;
    game.switch_to(PathBuf::from("."));
    assert!(
        !game.hub.asked && game.closing == Closing::No,
        "the project open"
    );
    prompted(&mut game, &ends, &mut cx);
    let (_, text) = game.asks(&Action::Close);
    assert!(text.contains("the project other opens"), "{text}");
    game.answer(Choice::Cancel);
    assert_eq!((game.closing, game.switching.as_ref()), (Closing::No, None));
    prompted(&mut game, &ends, &mut cx);
    game.answer(Choice::DontSave);
    assert_eq!(game.closing, Closing::Granted);
    assert_eq!(game.switching.as_ref(), Some(&other));

    let (mut game, ends) = serving();
    (game.stopped, game.told) = (true, false);
    prompted(&mut game, &ends, &mut cx);
    let close = Event::CloseRequested;
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Break(()));
    assert_eq!(game.switching, None);
    let (_, text) = game.asks(&Action::Close);
    assert!(text.contains("the editor closes"), "{text}");
    game.answer(Choice::DontSave);
    assert_eq!(game.event(&mut cx, &close), ControlFlow::Continue(()));
}

/// A look's part goes to the game only as the user changes it. Not as the editor's camera adopts
/// the frames' viewpoints through the game's camera, which the toolbar's switch turned the view to;
/// nor as it takes another client's viewpoint while that drives, or settles after; nor as the
/// Inspector makes the pick another client's outline asks for. A wheel, stopped, turns the view to
/// the editor's camera from where the frames left it, its viewpoint going with the grid; the switch
/// back gives both back. A pick of the user's goes as the outline, though the Inspector makes it in
/// the outline's pass.
#[test]
fn the_look_goes_to_the_game_as_the_user_changes_it() {
    let (mut game, ends) = serving();
    // Stopped, so that the wheel is the editor's camera's, through the game's camera by the
    // toolbar's switch, rather than as it starts, whose first frame would turn the view.
    (game.stopped, game.told, game.adopting) = (true, false, false);
    let reader = Reader::create(4, 4).expect("memory");
    let mut writer = Writer::open(reader.name()).expect("opened");
    game.reader = Some(reader);
    // A viewpoint at `position`, turned as the editor's camera starts, 4 units tall: its pivot
    // off the grid's plane, onto which the camera settles it.
    let orthographic = |position| gg_api::Viewpoint {
        position,
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Orthographic {
            height: 4.0,
            near: -40.0,
            far: 40.0,
        },
    };
    let mut frame = |viewpoint, look| {
        let published = writer.publish([4, 4], 0, SystemTime::now(), viewpoint, look, |_| {});
        published.expect("published");
    };
    let mut cx = Context::seeded(1);
    let mut update = |game: &mut Game| game.update(&mut cx, Duration::ZERO);
    // The game answers a look with `parts`, each set by the client `by` names, at `revision`.
    let answer = |parts: Parts, by: [Option<&str>; 3], revision| {
        let [viewpoint, outline, grid] = by.map(|by| by.map(str::to_owned));
        let looked = Looked {
            viewpoint: parts.viewpoint,
            outline: parts.outline,
            grid: parts.grid,
            colliders: None,
            show: None,
            skip: None,
            by: By {
                viewpoint,
                outline,
                grid,
                colliders: None,
                show: None,
                skip: None,
            },
            revision,
            run: 1,
            seen: None,
        };
        let answer = ("look".to_owned(), Ok(json!(looked)));
        ends.answer.send(answer).expect("sent");
    };
    for _ in 0..2 {
        frame(orthographic([1.0, 2.0, 5.0]), 0);
        update(&mut game);
    }
    assert_eq!(looks(&ends), [], "the frames' viewpoints");
    let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: 1.0 });
    let _ = game.route(&wheel, &reading([300.0, 200.0]), Some(VIEW));
    update(&mut game);
    let seen = game.view.camera.viewpoint();
    assert_eq!(
        seen.position.truncate(),
        DVec2::new(1.0, 2.0),
        "from the frames'"
    );
    let mine = Parts {
        viewpoint: Some(api_viewpoint(&seen)),
        grid: Some(gg_api::Plane::Xy),
        ..Parts::default()
    };
    let sent = Look {
        viewpoint: Some(mine.viewpoint),
        grid: Some(mine.grid),
        ..Look::default()
    };
    assert_eq!(looks(&ends), [sent]);
    // An agent sets the viewpoint and an outline, which a frame draws with: the editor reads
    // the look, and takes them.
    answer(mine.clone(), [Some(CLIENT), None, Some(CLIENT)], 1);
    let agents = orthographic([-3.0, 1.0, 4.0]);
    frame(agents, 2);
    // At the update after the one that takes the frame, which sends before it takes.
    for _ in 0..2 {
        update(&mut game);
    }
    assert_eq!(looks(&ends), [Look::default()], "a read");
    let x: EntityId = "0123456789abcdef".parse().expect("an ID");
    let outline = Outline {
        ids: vec![x.to_string()],
        color: [0.0, 1.0, 0.0, 1.0],
        width: 3.0,
    };
    let theirs = Parts {
        viewpoint: Some(agents),
        outline: Some(outline.clone()),
        ..mine
    };
    answer(theirs, [Some("agent"), Some("agent"), Some(CLIENT)], 2);
    for _ in 0..3 {
        update(&mut game);
    }
    let seen = game.view.camera.viewpoint();
    assert_eq!(seen.position.truncate(), DVec2::new(-3.0, 1.0), "taken");
    assert_eq!(looks(&ends), [], "taken, and settled");
    // Back to the game's camera, with the toolbar's switch, for which a test sets the field.
    game.view.toolbar.world_camera = true;
    update(&mut game);
    let back = Look {
        viewpoint: Some(None),
        grid: Some(None),
        ..Look::default()
    };
    assert_eq!(looks(&ends), [back]);
    let outlined = Parts {
        outline: Some(outline.clone()),
        ..Parts::default()
    };
    answer(outlined, [None, Some("agent"), None], 3);
    update(&mut game);
    // The Inspector's pass after the agent's outline is taken, another entity picked before
    // it: it picks the outline's, or unpicks, for an ID a read of the game has found gone,
    // neither the user's doing; but a pick of the user's in the same pass is theirs, whose
    // outline goes.
    let [before, users]: [EntityId; 2] =
        ["fedcba9876543210", "00000000000000ff"].map(|id| id.parse().expect("an ID"));
    let was = Wants {
        picks: vec![before],
        ..game.wants.clone()
    };
    game.wants = was.clone();
    for (picks, sends) in [(vec![x], false), (vec![], false), (vec![users], true)] {
        game.take_look(Look {
            outline: Some(Some(outline.clone())),
            ..Look::default()
        });
        game.changes(Wants {
            picks: picks.clone(),
            ..was.clone()
        });
        let wanted = Parts {
            outline: picks.first().map(|id| Outline {
                ids: vec![id.to_string()],
                color: OUTLINE.to_array(),
                width: OUTLINE_WIDTH,
            }),
            ..Parts::default()
        };
        let sent = sends.then(|| Look {
            outline: Some(wanted.outline.clone()),
            ..Look::default()
        });
        assert_eq!(game.looking.ask(&wanted), sent, "{picks:?}");
    }
}

/// The toolbar's switch to the editor's camera sends its viewpoint, adopted from the frames, with
/// the grid, though the camera hasn't moved: from then on it drives the view.
#[test]
fn the_switch_to_the_editors_camera_sends_its_viewpoint() {
    let (mut game, ends) = serving();
    let reader = Reader::create(4, 4).expect("memory");
    let mut writer = Writer::open(reader.name()).expect("opened");
    game.reader = Some(reader);
    let viewpoint = gg_api::Viewpoint {
        position: [1.0, 2.0, 5.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Orthographic {
            height: 4.0,
            near: -40.0,
            far: 40.0,
        },
    };
    let published = writer.publish([4, 4], 0, SystemTime::now(), viewpoint, 0, |_| {});
    published.expect("published");
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [], "the frame's viewpoint");
    // The switch, for which a test sets the field, since it can't run `ui`.
    game.view.toolbar.world_camera = false;
    game.update(&mut cx, Duration::ZERO);
    let seen = game.view.camera.viewpoint();
    assert_eq!(
        seen.position.truncate(),
        DVec2::new(1.0, 2.0),
        "the frame's"
    );
    let sent = Look {
        viewpoint: Some(Some(api_viewpoint(&seen))),
        grid: Some(Some(gg_api::Plane::Xy)),
        ..Look::default()
    };
    assert_eq!(looks(&ends), [sent]);
}

/// Another client's viewpoint, which turns the view to the editor's camera, sends the toolbar's
/// grid, as the user's turn does. Giving it back turns the view back and gives the grid back. A
/// grid another client set stays as the view turns.
#[test]
fn the_grid_follows_a_turn_another_clients_viewpoint_makes() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [], "all the game's own");
    let agents = gg_api::Viewpoint {
        position: [-3.0, 1.0, 4.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Orthographic {
            height: 4.0,
            near: -40.0,
            far: 40.0,
        },
    };
    // The game answers a look of `viewpoint` and `grid`, each set by the client `by` names, at
    // `revision`; returns the looks the editor asks for in the update that takes it.
    let mut answer = |viewpoint, grid, by: [Option<&str>; 2], revision| {
        let [viewpoint_by, grid_by] = by.map(|by| by.map(str::to_owned));
        let looked = Looked {
            viewpoint,
            outline: None,
            grid,
            colliders: None,
            show: None,
            skip: None,
            by: By {
                viewpoint: viewpoint_by,
                outline: None,
                grid: grid_by,
                colliders: None,
                show: None,
                skip: None,
            },
            revision,
            run: 1,
            seen: None,
        };
        let answer = ("look".to_owned(), Ok(json!(looked)));
        ends.answer.send(answer).expect("sent");
        game.update(&mut cx, Duration::ZERO);
        looks(&ends)
    };
    let grid = |grid| Look {
        grid: Some(grid),
        ..Look::default()
    };
    let (xy, xz) = (gg_api::Plane::Xy, gg_api::Plane::Xz);
    let agent = Some("agent");
    let sent = answer(Some(agents), None, [agent, None], 1);
    assert_eq!(sent, [grid(Some(xy))], "the toolbar's");
    let sent = answer(Some(agents), Some(xy), [agent, Some(CLIENT)], 2);
    assert_eq!(sent, []);
    let sent = answer(None, Some(xy), [None, Some(CLIENT)], 3);
    assert_eq!(sent, [grid(None)], "given back");
    assert_eq!(answer(None, None, [None, None], 4), []);
    let sent = answer(Some(agents), Some(xz), [agent, agent], 5);
    assert_eq!(sent, [], "the agent's grid");
    let sent = answer(None, Some(xz), [None, agent], 6);
    assert_eq!(sent, [], "the agent's grid stays");
}

/// The passes the toolbar's list leaves out go to the game as the user ticks them, null once
/// all are drawn again; and those another client leaves out, or gives back, set the list.
#[test]
fn the_passes_left_out_go_to_the_game_and_are_taken_from_other_clients() {
    use gg_api::Pass;

    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [], "all the game's own");
    // The game answers a look whose passes left out are `skip`, set by `by`, at `revision`;
    // returns the looks the editor asks for in the update that takes it.
    let answer = |game: &mut Game, cx: &mut Context, skip, by: Option<&str>, revision| {
        let looked = Looked {
            viewpoint: None,
            outline: None,
            grid: None,
            colliders: None,
            show: None,
            skip,
            by: By {
                skip: by.map(str::to_owned),
                ..By::default()
            },
            revision,
            run: 1,
            seen: None,
        };
        let answer = ("look".to_owned(), Ok(json!(looked)));
        ends.answer.send(answer).expect("sent");
        game.update(cx, Duration::ZERO);
        looks(&ends)
    };
    let skips = |skip| Look {
        skip: Some(skip),
        ..Look::default()
    };
    // The list's ticks, for which a test sets the field, since it can't run `ui`.
    game.view.toolbar.skip = vec![Pass::Reflections];
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [skips(Some(vec![Pass::Reflections]))]);
    let mine = Some(vec![Pass::Reflections]);
    assert_eq!(answer(&mut game, &mut cx, mine, Some(CLIENT), 1), []);
    let theirs = vec![Pass::Shadows, Pass::Fog];
    let sent = answer(&mut game, &mut cx, Some(theirs.clone()), Some("agent"), 2);
    assert_eq!(
        (sent, &game.view.toolbar.skip),
        (vec![], &theirs),
        "the agent's"
    );
    let sent = answer(&mut game, &mut cx, None, None, 3);
    assert_eq!(sent, [], "given back");
    assert_eq!(game.view.toolbar.skip, [], "all drawn again");
    game.view.toolbar.skip = vec![Pass::Lens];
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [skips(Some(vec![Pass::Lens]))]);
    let mine = Some(vec![Pass::Lens]);
    assert_eq!(answer(&mut game, &mut cx, mine, Some(CLIENT), 4), []);
    game.view.toolbar.skip.clear();
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(looks(&ends), [skips(None)], "the user's null");
}

/// A Probe press asks the game for a probe of its pixel, in the view's pixels, taking the press
/// from the camera. Its answer fills the numbers beside it; an answer to a probe another replaced
/// is dropped. Escape, or another tool, closes them.
#[test]
fn a_probe_is_asked_and_its_numbers_shown_until_escape_or_another_tool() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    // Through the editor's camera, as in Simulate, the tools take the view's input.
    game.view.toolbar.world_camera = false;
    game.view.toolbar.tool = Tool::Probe;
    // [`VIEW`]'s top left is at (100, 50).
    let (at, in_game) = ([300.0, 200.0], [200.0, 150.0]);
    let press = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let probed = |game: &mut Game| {
        let routed = game.route(&press, &reading(at), Some(VIEW));
        assert_eq!(routed, ControlFlow::Break(()), "the camera's not");
        let asked: Vec<_> = ends.probes.try_iter().collect();
        let [(ticket, pixel)] = asked[..] else {
            panic!("one probe asked: {asked:?}");
        };
        assert_eq!(pixel, in_game);
        ticket
    };
    let first = probed(&mut game);
    let waiting = game.probing.as_ref().expect("probing");
    assert_eq!((waiting.at, waiting.answer.is_none()), (at, true));
    let second = probed(&mut game);
    let answer = |ticket| (ticket, Ok(crate::tests::probe(in_game)));
    ends.read.send(answer(first)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let probing = game.probing.as_ref().expect("probing");
    assert!(probing.answer.is_none(), "the first's dropped");
    ends.read.send(answer(second)).expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let probing = game.probing.as_ref().expect("probing");
    let shown = probing
        .answer
        .as_ref()
        .and_then(|answer| answer.as_ref().ok());
    assert_eq!(shown, Some(&crate::tests::probe(in_game)));
    let escape = Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
    };
    let _ = game.route(&escape, &reading(at), Some(VIEW));
    assert!(game.probing.is_none(), "closed by Escape");
    probed(&mut game);
    game.view.toolbar.tool = Tool::Select;
    game.update(&mut cx, Duration::ZERO);
    assert!(game.probing.is_none(), "closed by another tool");
}

/// The images the game's frame makes are asked for as the list of what the view shows opens,
/// and listed as the game answers; a refusal, as an older game's, says why in their place.
#[test]
fn the_images_are_asked_for_as_the_list_opens_and_listed() {
    use gg_api::{FrameImage, ImageKind};

    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    let asked = |ends: &Ends| methods(ends).iter().filter(|m| *m == "images").count();
    assert_eq!(asked(&ends), 0, "not before the list opens");
    // The list's opening, for which a test sets the field, since it can't run `ui`.
    game.view.toolbar.ask_images = true;
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(asked(&ends), 1, "as it opens");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(asked(&ends), 0, "once");
    let depth = FrameImage {
        name: "view depth".to_owned(),
        kind: ImageKind::Depth,
        size: [64, 32],
    };
    let listed = json!({ "images": [depth.clone()] });
    ends.answer
        .send(("images".to_owned(), Ok(listed)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.view.toolbar.images, [depth]);
    let refused = Failure::new("api.unknown_method", "there's no method `images`");
    ends.answer
        .send(("images".to_owned(), Err(refused)))
        .expect("sent");
    game.update(&mut cx, Duration::ZERO);
    let why = "[api.unknown_method] there's no method `images`";
    assert_eq!(game.view.toolbar.images_refused.as_deref(), Some(why));
}

/// While the game hasn't answered the last `input`, as when a script stopped it, queued cursor
/// moves replace each other, the presses between them kept in order, so the queue grows with
/// presses alone.
#[test]
fn the_cursors_moves_queued_for_the_game_take_each_others_places() {
    let (mut game, _ends) = serving();
    game.inputting = true;
    let at = |i: u32| [101.0 + f64::from(i % 400), 60.0];
    let w = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    for i in 0..1_000 {
        if i == 500 {
            let _ = game.route(&w, &reading(at(i)), Some(VIEW));
        }
        let [x, y] = at(i);
        let moved = Event::CursorMoved { x, y };
        let _ = game.route(&moved, &reading(at(i)), Some(VIEW));
    }
    let w_down = InputEvent::Key {
        key: "KeyW".to_owned(),
        pressed: true,
        repeat: false,
    };
    let in_game = |[x, y]: [f64; 2]| InputEvent::Cursor(in_view([x, y], VIEW.origin));
    assert_eq!(game.input, [in_game(at(499)), w_down, in_game(at(999))]);
}

/// A game asking for the cursor grabbed has the editor grab its own in Play. Escape lets go, though
/// the game still asks, until a press in the view; the game gets both. Simulate lets go. Raw motion
/// queued for the game adds up.
#[test]
fn a_games_grab_holds_the_editors_cursor_in_play_until_escape() {
    let (mut game, _ends) = serving();
    let reader = Reader::create(4, 4).expect("memory");
    let writer = Writer::open(reader.name()).expect("opened");
    game.reader = Some(reader);
    let mut cx = Context::seeded(1);
    game.update(&mut cx, Duration::ZERO);
    assert!(!cx.cursor_grabbed(), "before the game asks");
    writer.set_cursor_grabbed(true);
    game.update(&mut cx, Duration::ZERO);
    assert!(cx.cursor_grabbed());
    // As while the game hasn't answered the last `input`, so that what's queued stays.
    game.inputting = true;
    let at = reading([300.0, 200.0]);
    for (x, y) in [(2.0, 1.0), (-0.5, 3.0)] {
        let _ = game.route(&Event::MouseMotion { x, y }, &at, Some(VIEW));
    }
    let escape = Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
    };
    let _ = game.route(&escape, &at, Some(VIEW));
    game.update(&mut cx, Duration::ZERO);
    assert!(!cx.cursor_grabbed(), "let go at Escape");
    let press = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let _ = game.route(&press, &at, Some(VIEW));
    game.update(&mut cx, Duration::ZERO);
    assert!(cx.cursor_grabbed(), "grabbed again at a press in the view");
    let escaped = InputEvent::Key {
        key: "Escape".to_owned(),
        pressed: true,
        repeat: false,
    };
    let pressed = InputEvent::Button {
        button: gg_api::Button::Left,
        pressed: true,
    };
    assert_eq!(
        game.input,
        [InputEvent::Motion([1.5, 4.0]), escaped, pressed]
    );
    game.view.toolbar.world_camera = false;
    game.update(&mut cx, Duration::ZERO);
    assert!(!cx.cursor_grabbed(), "Simulate lets go");
}

/// Play captures the view's input for a game that doesn't ask for a grab: the editor grabs the
/// cursor from the game's answer until Escape, after which a press in the view grabs nothing.
/// Resume in Play captures again, after an Escape too, as Play from Simulate does. Another client's
/// stop lets go for good, as Simulate and Stop do: a switch back to the game's camera after another
/// client's play isn't a click of Play.
#[test]
fn play_captures_the_views_input_for_any_game_until_escape() {
    let (mut game, ends) = serving();
    let mut cx = Context::seeded(1);
    let mut grabbed = |game: &mut Game| {
        game.update(&mut cx, Duration::ZERO);
        cx.cursor_grabbed()
    };
    let answer = |method: &str, stopped| {
        let answer = (method.to_owned(), status(stopped, false));
        ends.answer.send(answer).expect("sent");
    };
    game.stopped = true;
    game.click(overlay::Clicked::To(Mode::Play));
    assert!(!grabbed(&mut game), "before the game plays");
    answer("play", false);
    assert!(grabbed(&mut game), "once it plays");
    let at = reading([300.0, 200.0]);
    let escape = Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
    };
    let _ = game.route(&escape, &at, Some(VIEW));
    assert!(!grabbed(&mut game), "let go at Escape");
    game.click(overlay::Clicked::Pause);
    assert!(!grabbed(&mut game), "Pause");
    game.click(overlay::Clicked::Resume);
    assert!(grabbed(&mut game), "Resume in Play");
    let _ = game.route(&escape, &at, Some(VIEW));
    let press = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let _ = game.route(&press, &at, Some(VIEW));
    assert!(!grabbed(&mut game), "a press in the view after Escape");
    game.click(overlay::Clicked::Resume);
    answer("status", true);
    assert!(!grabbed(&mut game), "another client's stop");
    answer("status", false);
    game.view.toolbar.world_camera = true;
    assert!(
        !grabbed(&mut game),
        "another client's play, through the game's camera"
    );
    game.click(overlay::Clicked::To(Mode::Simulate));
    assert!(!grabbed(&mut game), "Simulate");
    game.click(overlay::Clicked::To(Mode::Play));
    assert!(grabbed(&mut game), "Play from Simulate");
    game.click(overlay::Clicked::To(Mode::Edit));
    assert!(!grabbed(&mut game), "Stop");
}

/// Through the editor's camera, the editor grabs the cursor while the camera looks about, from the
/// right button's press in the view to its release; not while it pans.
#[test]
fn the_editors_camera_grabs_the_cursor_while_it_looks_about() {
    let (mut game, _ends) = serving();
    // Simulate, whose view's input is the editor's camera's.
    game.view.toolbar.world_camera = false;
    game.view.camera.perspective = true;
    let mut cx = Context::seeded(1);
    let at = reading([300.0, 200.0]);
    let button = |button, pressed| Event::MouseButton { button, pressed };
    for (pressed, grabbed) in [(true, true), (false, false)] {
        let _ = game.route(&button(MouseButton::Right, pressed), &at, Some(VIEW));
        game.update(&mut cx, Duration::ZERO);
        assert_eq!(cx.cursor_grabbed(), grabbed, "right pressed: {pressed}");
    }
    let _ = game.route(&button(MouseButton::Middle, true), &at, Some(VIEW));
    game.update(&mut cx, Duration::ZERO);
    assert_eq!(game.view.camera.moving(), Some(Move::Pan));
    assert!(!cx.cursor_grabbed(), "panning");
}

#[test]
fn the_views_input_reaches_the_game_in_the_views_pixels() {
    let origin = [10, 20];
    let key = Event::Key {
        key: Key::KeyW,
        pressed: true,
        repeat: false,
    };
    let moved = Event::CursorMoved { x: 15.5, y: 20.0 };
    let wheel = Event::Scroll(Scroll::Lines { x: 0.0, y: -1.0 });
    let forwarded: Vec<_> = [
        key,
        moved,
        wheel,
        Event::Resized {
            width: 1,
            height: 1,
        },
    ]
    .iter()
    .filter_map(|event| forwarded(event, origin))
    .collect();
    let expected = [
        InputEvent::Key {
            key: "KeyW".to_owned(),
            pressed: true,
            repeat: false,
        },
        InputEvent::Cursor([5.5, 0.0]),
        InputEvent::Scroll(gg_api::Scroll::Lines([0.0, -1.0])),
    ];
    assert_eq!(
        forwarded, expected,
        "the window's own events stay the editor's"
    );
}

#[test]
fn gg_runs_events_say_how_the_run_goes_and_reach_the_logs() {
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    let send = game.send.take().expect("not started");
    let failed = json!({"code": "gg.build", "message": "error[E0308]: mismatched types"});
    let running = json!({"code": "gg.running", "message": "another `gg run` runs shards"});
    let lines = [
        Line::Out(json!({"event": "started", "pid": 7}).to_string()),
        Line::Out(json!({"event": "rebuild_failed", "error": failed}).to_string()),
        Line::Err("   1.000  WARN  engine  Something".to_owned()),
        Line::Err(json!({ "error": running }).to_string()),
    ];
    for line in lines {
        send.send(line).expect("sent");
    }
    game.take_lines();
    let levels: Vec<_> = game.logs.iter().map(|logged| logged.level).collect();
    assert_eq!(
        levels,
        [Level::Info, Level::Error, Level::Warn, Level::Error]
    );
    assert!(
        game.logs[1].text.contains("mismatched types"),
        "rustc's errors show"
    );
    assert_eq!(
        game.run_state,
        RunState::Ended("[gg.running] another `gg run` runs shards".to_owned())
    );
}

/// The Logs panel, holding its most, the oldest gone, lays out only the rows in view, a row per
/// line. It fills with the latest lines in order, entries of several lines among them, the last at
/// its bottom.
#[test]
fn the_logs_lay_out_only_the_rows_in_view_and_show_the_latest() {
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    // An entry of one to three lines, each its own text; but the one before the last, of more
    // than the panel holds, so that its top row is inside an entry.
    let logged = LOGGED + 1000;
    let entry = |at: usize| {
        let lines = if at == logged - 2 { 40 } else { at % 3 + 1 };
        (0..lines).map(move |line| format!("{at}.{line}"))
    };
    for at in 0..logged {
        game.log(Level::Info, entry(at).collect::<Vec<_>>().join("\n"));
    }
    let lines: Vec<String> = (0..logged).flat_map(entry).collect();
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
    let panel = egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(400.0, 300.0));
    let frame = || {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let builder = egui::UiBuilder::new().max_rect(panel);
            ui.scope_builder(builder, |ui| show_logs(ui, &game.logs));
        });
        output.textures_delta.clear();
        output.shapes
    };
    // The first finds how tall its rows are, the bottom of which the second scrolls to, and
    // the third lays out only what the second did.
    frame();
    frame();
    let mut shown = texts(&frame());
    shown.sort_by(|(_, a), (_, b)| a.top().total_cmp(&b.top()));
    assert!(shown.len() > 10, "{shown:?}");
    let laid_out = ctx.fonts(|fonts| fonts.num_galleys_in_cache());
    assert!(laid_out <= 4 * shown.len(), "{laid_out} laid out");
    let texts: Vec<_> = shown.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(texts, lines[lines.len() - shown.len()..]);
    let pitch = shown[1].1.top() - shown[0].1.top();
    let (top, bottom) = (shown[0].1.top(), shown[shown.len() - 1].1.bottom());
    assert!(top < panel.top() + pitch && bottom > panel.bottom() - pitch);
}

#[test]
fn a_build_says_where_it_has_come_to_in_the_title_bar() {
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    let send = game.send.take().expect("not started");
    assert_eq!(game.label(), "Building…");
    let building = |mut event: Value| {
        event["event"] = json!("building");
        event
    };
    let events = [
        (
            building(json!({"stage": "starting", "crates": 293})),
            "Building… starting",
        ),
        (
            building(
                json!({"stage": "compiling", "compiled": 212, "crates": 293, "crate": "gg_engine"}),
            ),
            "Building… 212 of 293 crates",
        ),
        (building(json!({"stage": "linking"})), "Building… linking"),
        (
            json!({"event": "built", "build_ms": 14580}),
            "Building… linking",
        ),
        (json!({"event": "started", "pid": 7}), "Starting…"),
        (
            json!({"event": "rebuilding", "file": "src/main.rs"}),
            "Rebuilding…",
        ),
        (
            building(json!({"stage": "extracting_assets"})),
            "Rebuilding… extracting assets",
        ),
    ];
    let count = events.len();
    for (event, label) in events {
        send.send(Line::Out(event.to_string())).expect("sent");
        game.take_lines();
        assert_eq!(game.label(), label, "after {event}");
    }
    assert_eq!(game.logs.len(), count, "each reaches the Logs too");
}

/// The game's events reach the Logs as they're read, at their level, with code, message and fields,
/// but not where and when they were logged. `gg run`'s notes of a failed patch don't, as its own
/// events show it.
#[test]
fn the_games_events_reach_the_logs_with_their_codes_and_fields() {
    let mut game = Game::new(Some(PathBuf::from(".")), Inspector::remote(CLIENT).link());
    let (asks, _) = mpsc::channel();
    let (_, answers) = mpsc::channel();
    let (log, logged) = mpsc::channel();
    let (picks, _) = mpsc::channel();
    let (_, picked) = mpsc::channel();
    let (probes, _) = mpsc::channel();
    let (_, probed) = mpsc::channel();
    game.talker = Some(Talker {
        asks,
        answers,
        logged,
        picks,
        picked,
        probes,
        probed,
    });
    let warned = r#"{"timestamp":"2026-10-30T12:00:00.000000Z","level":"WARN",
            "code":"asset.missing","path":"assets/hero sprite.png","tries":2,"empty":"",
            "message":"Can't load the texture","target":"shards",
            "filename":"games/shards/src/main.rs","line_number":40,"threadName":"main"}"#;
    let traced = r#"{"timestamp":"2026-10-30T12:00:00.000001Z","level":"TRACE",
            "message":"A tick","target":"gg_engine::main_loop","spans":[{"name":"tick"}],
            "threadName":"main"}"#;
    let failed = r#"{"level":"WARN","code":"gg.patch_failed","client":"gg",
            "message":"A patch of src/main.rs failed"}"#;
    let attached = r#"{"level":"WARN","code":"gg.attach_changed","client":"gg",
            "message":"A patch changed a layer's attach"}"#;
    let events = [warned, traced, failed, attached];
    let events = events.map(|line| serde_json::from_str(line).expect("JSON"));
    log.send(events.to_vec()).expect("sent");
    log.send(Vec::new()).expect("sent");
    game.take_logged();
    let logs: Vec<_> = (game.logs.iter())
        .map(|logged| (logged.level, logged.text.as_str()))
        .collect();
    let expected = [
        (
            Level::Warn,
            r#"[asset.missing] Can't load the texture  empty="" path="assets/hero sprite.png" tries=2"#,
        ),
        (Level::Debug, "A tick"),
        (
            Level::Warn,
            "[gg.attach_changed] A patch changed a layer's attach  client=gg",
        ),
    ];
    assert_eq!(logs, expected);
}

/// A primary-button drag from `from` to `to`, an event a frame: past a click's reach the frame
/// after the press, then to `to`, then let go.
fn drag(from: egui::Pos2, to: egui::Pos2) -> Vec<egui::Event> {
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![
        egui::Event::PointerMoved(from),
        button(from, true),
        egui::Event::PointerMoved(from + egui::vec2(12.0, 0.0)),
        egui::Event::PointerMoved(to),
        egui::Event::PointerMoved(to + egui::vec2(1.0, 1.0)),
        button(to + egui::vec2(1.0, 1.0), false),
    ]
}

/// A texture dragged from the Assets panel outlines the view while it's held over it, until let go
/// there, which asks for a pick there, in the view's pixels. One let go elsewhere asks nothing, nor
/// does a file that isn't a texture or a model, which doesn't drag.
#[test]
fn a_texture_dragged_from_the_assets_panel_is_let_go_on_the_view() {
    let scratch = Scratch::new("dragged");
    for file in ["stone.png", "notes.txt"] {
        fs::write(scratch.0.join(file), "").expect("written");
    }
    let (mut game, ends) = serving();
    game.folders = Some(folders(&scratch.0));
    game.assets = Some(Assets::new(scratch.0.clone()));
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
    let (panel, view) = screen.split_left_right_at_x(400.0);
    let place = Viewport {
        origin: [400, 0],
        size: [400, 400],
    };
    // A frame of the panel beside a view that senses presses as the dock's does.
    let frame = |game: &mut Game, event: Option<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: event.into_iter().collect(),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let sense = egui::Sense::CLICK | egui::Sense::DRAG;
            let response = ui.interact(view, egui::Id::unique("test view"), sense);
            if let Some(assets) = &mut game.assets {
                let builder = egui::UiBuilder::new().max_rect(panel);
                ui.scope_builder(builder, |ui| assets.show(ui));
            }
            game.take_drop(ui.ctx(), &response, Some(place), 1.0);
        });
        output.textures_delta.clear();
        output.shapes
    };
    // The drag from `from` to `to`, and whether each of its frames outlined the view.
    let drop = |game: &mut Game, from: egui::Rect, to| {
        let frames = drag(from.center(), to).into_iter();
        let shapes = frames.map(|event| frame(game, Some(event)));
        let outlined = |shapes: Vec<egui::epaint::ClippedShape>| {
            let outline = |shape: &_| matches!(shape, egui::Shape::Rect(r) if r.rect == view);
            shapes.iter().any(|clipped| outline(&clipped.shape))
        };
        shapes.map(outlined).collect::<Vec<bool>>()
    };
    frame(&mut game, None);
    let shown = texts(&frame(&mut game, None));
    let (stone, notes) = (find(&shown, "stone.png"), find(&shown, "notes.txt"));
    let elsewhere = egui::pos2(200.0, 350.0);
    for (from, to) in [(notes, view.center()), (stone, elsewhere)] {
        assert_eq!(drop(&mut game, from, to), [false; 6]);
        assert!(ends.picks.try_recv().is_err());
    }
    // Outlined once it's over the view, until it's let go.
    let outlined = drop(&mut game, stone, view.center());
    assert_eq!(outlined, [false, false, false, true, true, true]);
    let (ticket, at, _) = ends.picks.try_recv().expect("a pick");
    // Let go a pixel right of and below the view's middle, (600, 200).
    assert_eq!(at, [201.0, 201.0]);
    assert_eq!(game.dropping.map(|dropping| dropping.ticket), Some(ticket));
}

/// A texture let go over the view asks for a pick there. The entity found is picked, and its
/// sprite's texture set through the link, as an edit of its own, to the texture's ID from its ID
/// file, or without one, its path from the asset root. Where no entity draws, nothing is set and
/// the Logs say so. Of two let go before an answer, the latest alone is set.
#[test]
fn a_texture_let_go_on_the_view_sets_the_texture_of_the_sprite_there() {
    let scratch = Scratch::new("texture-let-go");
    let textures = scratch.0.join("textures");
    fs::create_dir(&textures).expect("made");
    for file in ["stone.png", "new.png"] {
        fs::write(textures.join(file), "").expect("written");
    }
    let id = "5f3c9a1e07b2d846";
    let meta = Meta {
        id: id.parse().expect("an ID"),
    };
    fs::write(Meta::path(&textures.join("stone.png")), meta.text()).expect("written");
    let (mut game, ends) = serving();
    game.folders = Some(folders(&scratch.0));
    let texture = |name: &str| Dragged {
        path: textures.join(name),
        kind: AssetKind::Texture,
    };
    let entity: EntityId = "0123456789abcdef".parse().expect("an ID");
    let found = |id: Option<EntityId>| gg_api::Picked {
        id: id.map(|id| id.to_string()),
        name: id.map(|_| "Pillar".to_owned()),
    };
    let set = |name: &str| {
        let texture = reflect::Value::String(name.to_owned());
        let texture = reflect::Value::Option(Some(Box::new(texture)));
        Some((entity, "Sprite".to_owned(), "texture".to_owned(), texture))
    };
    game.let_go(&texture("stone.png"), [300.0, 200.0], Some(VIEW), SCALE);
    let (ticket, at, _) = ends.picks.try_recv().expect("a pick");
    // [`VIEW`]'s top left is at (100, 50).
    assert_eq!(at, [200.0, 150.0]);
    ends.found
        .send((ticket, Ok(found(Some(entity)))))
        .expect("sent");
    game.take_picks();
    assert_eq!(game.view.link.pending(), [set(id), None]);
    assert_eq!(game.dropping, None);
    // Where no entity draws.
    game.let_go(&texture("new.png"), [300.0, 200.0], Some(VIEW), SCALE);
    let (ticket, _, _) = ends.picks.try_recv().expect("a pick");
    ends.found.send((ticket, Ok(found(None)))).expect("sent");
    game.take_picks();
    assert_eq!(game.view.link.pending().len(), 2, "nothing more set");
    let said = game.logs.back().map(|logged| logged.text.as_str());
    assert_eq!(said, Some("new.png was let go where no entity draws"));
    // Two before an answer: the latest's alone.
    game.let_go(&texture("stone.png"), [300.0, 200.0], Some(VIEW), SCALE);
    game.let_go(&texture("new.png"), [300.0, 200.0], Some(VIEW), SCALE);
    let tickets: Vec<u64> = ends.picks.try_iter().map(|(ticket, ..)| ticket).collect();
    for ticket in tickets {
        ends.found
            .send((ticket, Ok(found(Some(entity)))))
            .expect("sent");
    }
    game.take_picks();
    let pending = game.view.link.pending();
    assert_eq!(pending[2..], [set("textures/new.png"), None]);
}

/// A font let go over the view sets the font of the text there through the link, as an edit of its
/// own, by its path from the asset root, without an ID file.
#[test]
fn a_font_let_go_on_the_view_sets_the_font_of_the_text_there() {
    let scratch = Scratch::new("font-let-go");
    let (mut game, ends) = serving();
    game.folders = Some(folders(&scratch.0));
    let title = Dragged {
        path: scratch.0.join("fonts").join("title.ttf"),
        kind: AssetKind::Font,
    };
    let entity: EntityId = "0123456789abcdef".parse().expect("an ID");
    game.let_go(&title, [300.0, 200.0], Some(VIEW), SCALE);
    let (ticket, _, _) = ends.picks.try_recv().expect("a pick");
    let found = gg_api::Picked {
        id: Some(entity.to_string()),
        name: Some("Target".to_owned()),
    };
    ends.found.send((ticket, Ok(found))).expect("sent");
    game.take_picks();
    let font = reflect::Value::String("fonts/title.ttf".to_owned());
    let font = reflect::Value::Option(Some(Box::new(font)));
    let set = Some((entity, "Text".to_owned(), "font".to_owned(), font));
    assert_eq!(game.view.link.pending(), [set, None]);
}

/// A model let go over the view is a root the link creates, named after its file, by its path
/// without an ID file, where the last frame shows the grid's plane there: through the editor's
/// starting camera, the 2D games' plane. In perspective, where the ray meets the ground in front of
/// the camera; above the horizon, on the plane through the pivot facing the view. Before a frame
/// has come, it lands nowhere, and the Logs say so.
#[test]
fn a_model_let_go_on_the_view_lands_where_the_cursor_shows_the_grids_plane() {
    let scratch = Scratch::new("model-let-go");
    let (mut game, _ends) = serving();
    game.folders = Some(folders(&scratch.0));
    let rock = Dragged {
        path: scratch.0.join("models").join("rock.gltf"),
        kind: AssetKind::Model,
    };
    game.let_go(&rock, [560.0, 290.0], Some(VIEW), SCALE);
    assert!(game.view.link.creates().is_empty());
    let said = game.logs.back().map(|logged| logged.text.as_str());
    let why = "the view has shown no frame to place it in yet";
    assert_eq!(
        said,
        Some(format!("rock.gltf was let go, but {why}").as_str())
    );
    // Where a create put it, its name and its model as they're given.
    let landed = |game: &Game| {
        let creates = game.view.link.creates();
        let last = creates.last().expect("a create");
        let [(name, named), (transform, placed), (root, rooted)] = &last[..] else {
            panic!("a name, a transform and a model's root: {creates:?}");
        };
        let fields = [name, transform, root].map(|ident| ident.as_ref());
        assert_eq!(fields, ["Name", "Transform", "ModelRoot"]);
        assert_eq!(*named, Name("rock".to_owned()).to_value());
        let model = reflect::Value::String("models/rock.gltf".to_owned());
        assert_eq!(*rooted, reflect::Value::Struct(vec![("0".into(), model)]));
        Transform::from_value(placed)
            .expect("a transform")
            .translation
    };
    // As the editor's camera starts: the origin at (500, 350), and a unit 60 pixels across.
    game.seen = Some(game.view.camera.viewpoint());
    game.size = Some(VIEW.size);
    game.let_go(&rock, [560.0, 290.0], Some(VIEW), SCALE);
    let at = landed(&game);
    assert!(at.abs_diff_eq(DVec3::new(1.0, 1.0, 0.0), 1e-4), "{at}");
    // Looking along the ground, 1 unit above it.
    let along = Viewpoint {
        position: DVec3::new(0.0, 1.0, 10.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
    };
    game.seen = Some(along);
    game.view.camera.adopt(&along);
    game.view.toolbar.plane = Plane::XZ;
    // Below the view's middle, (500, 350), and above it.
    game.let_go(&rock, [500.0, 500.0], Some(VIEW), SCALE);
    let ground = landed(&game);
    assert!(ground.y.abs() < 1e-4 && ground.z < 10.0, "{ground}");
    game.let_go(&rock, [500.0, 200.0], Some(VIEW), SCALE);
    let screen = game.screen(Some(VIEW)).expect("a frame");
    assert_eq!(
        landed(&game),
        game.view.camera.under([500.0, 200.0], screen)
    );
    assert_eq!(game.view.link.creates().len(), 3);
}

/// A prefab let go over the view is an instance's root the link creates where it lands, as a
/// model's is: its transform and its link, by asset ID where its ID file holds one, unnamed, as it
/// takes its prefab root's name. Before a frame has come, nowhere.
#[test]
fn a_prefab_let_go_on_the_view_is_an_instance_where_it_lands() {
    let scratch = Scratch::new("prefab-let-go");
    let (mut game, _ends) = serving();
    game.folders = Some(folders(&scratch.0));
    let prefabs = scratch.0.join("prefabs");
    fs::create_dir(&prefabs).expect("made");
    let file = prefabs.join("lamp.prefab");
    fs::write(&file, "(\n    entities: {},\n)\n").expect("written");
    let id = "5f3c9a1e07b2d846";
    fs::write(
        Meta::path(&file),
        Meta {
            id: id.parse().expect("an ID"),
        }
        .text(),
    )
    .expect("written");
    let lamp = Dragged {
        path: file,
        kind: AssetKind::Prefab,
    };
    game.let_go(&lamp, [560.0, 290.0], Some(VIEW), SCALE);
    assert!(game.view.link.creates().is_empty(), "no frame yet");
    game.seen = Some(game.view.camera.viewpoint());
    game.size = Some(VIEW.size);
    game.let_go(&lamp, [560.0, 290.0], Some(VIEW), SCALE);
    let creates = game.view.link.creates();
    let [(transform, placed), (root, linked)] = &creates[0][..] else {
        panic!("a transform and a link: {creates:?}");
    };
    assert_eq!(
        [transform, root].map(|i| i.as_ref()),
        ["Transform", "PrefabRoot"]
    );
    let name = reflect::Value::String(id.to_owned());
    assert_eq!(*linked, reflect::Value::Struct(vec![("0".into(), name)]));
    let at = Transform::from_value(placed)
        .expect("a transform")
        .translation;
    assert!(at.abs_diff_eq(DVec3::new(1.0, 1.0, 0.0), 1e-4), "{at}");
}
