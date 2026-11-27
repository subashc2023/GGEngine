use std::cell::Cell;
use std::io::{self, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;

use gg_api::asset::AssetKind;
use gg_api::{
    Applied, AssetList, Captured, Client, EntityList, Framed, ListedOrphan, LogEvents, Picked,
    Pulse, Request, SavedPrefab, Session, Snapshotted,
};
use gg_core::math::{DVec3, Mat4, Vec3, Vec4};
use gg_renderer::{Streaming, Viewpoint};
use serde_json::{Value, json};
use subsecond::JumpTable;
use tracing::Level;

use super::*;

use super::connection::{
    Answer, HELLO_TIMEOUT, InFlight, Keyed, MOST_SAYING_HELLO, granted, needs, note, read_hello,
    same_token,
};
use super::files::{engine_named, prune};
use super::parse::{METHODS, MOST_STEPS, parse};
use super::view::{CAPTURE_WAIT, TRACE_WAIT};
use super::world::{Subject, listed};
use crate::api::Waiting;
use crate::args::Net;
use crate::dirs::tests::Scratch;
use crate::grant::Grant;
use crate::grid::Plane;
use crate::net::Lockstep;
use crate::recording::Recorder;
use crate::render_thread::{Extracted, Shot};
use crate::scene_files::tests as scenes;
use crate::{
    BoxCollider2D, EntityId, Event, Key, MouseButton, Name, PrefabRoot, Sprite, Tick, Transform,
    Viewport,
};

/// Pruning keeps only the newest engine-named files; client-named files, other kinds and
/// directories stay.
#[test]
fn the_newest_captures_alone_are_kept() {
    let dir = std::env::temp_dir().join(format!("gg-prune-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("gg")).expect("a directory");
    let epoch = std::time::SystemTime::UNIX_EPOCH;
    let files = [
        ("sandbox-7-1.png", 1),
        ("sandbox-7-2.png", 4),
        ("sandbox-9-1.png", 2),
        ("sandbox-9-2.png", 3),
        ("view.png", 0),
        ("my-shot-1.png", 0),
    ];
    for (name, secs) in files {
        let file = fs::File::create(dir.join(name)).expect("written");
        file.set_modified(epoch + Duration::from_secs(secs))
            .expect("its time set");
    }
    fs::write(dir.join("notes.txt"), "kept").expect("written");
    prune(&dir, "png", 2);
    let mut left: Vec<String> = (fs::read_dir(&dir).expect("listed").flatten())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        left,
        [
            "gg",
            "my-shot-1.png",
            "notes.txt",
            "sandbox-7-2.png",
            "sandbox-9-2.png",
            "view.png"
        ]
    );
    assert!(engine_named("shards-42-3.snapshot", "snapshot"));
    for name in [
        "shards-42.snapshot",
        "a-1-2.png",
        "-1-2.snapshot",
        "x-1-.snapshot",
    ] {
        assert!(!engine_named(name, "snapshot"), "{name}");
    }
}

/// How long a test waits before failing instead of hanging.
const DEADLINE: Duration = Duration::from_secs(20);

/// Manifest of the project [`started`]'s game belongs to.
const PROJECT: &str = "games/tested/project.ron";

/// Starts the API for app `app` (unique per test, so concurrent tests write their own session
/// files), for a game of [`PROJECT`] keeping no events.
fn started(app: &str) -> (Api, Session) {
    started_keeping(app, Box::leak(Box::default()))
}

/// As [`started`], the run's events kept in `events`.
fn started_keeping(app: &str, events: &'static Events) -> (Api, Session) {
    let project = Some(Path::new(PROJECT));
    let api = Api::start(app, &[], None, events, None, project).expect("the API starts");
    let pid = std::process::id();
    let file = crate::beside_exe("sessions").join(Session::file_name(app, pid));
    let text = fs::read(file).expect("the session file");
    (api, serde_json::from_slice(&text).expect("a session"))
}

/// Where request `id`'s answer goes: `to`, read by the test, with an in-flight place of its own.
fn reply(id: u64, to: &Sender<Answer>) -> Reply {
    Reply {
        id,
        to: to.clone(),
        flight: Arc::new(InFlight::default()).take(),
        keyed: None,
    }
}

/// A connection to `session`'s API, past hello, as raw lines.
fn said_hello(session: &Session) -> (TcpStream, BufReader<TcpStream>) {
    let stream = TcpStream::connect(session.address).expect("it connects");
    stream.set_read_timeout(Some(DEADLINE)).expect("a timeout");
    let hello = json!({"id": 0, "method": "hello", "params": {"token": session.token}});
    (&stream)
        .write_all(format!("{hello}\n").as_bytes())
        .expect("written");
    let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
    let welcome = gg_api::read_line(&mut reader).expect("an answer");
    assert!(welcome.is_some_and(|line| line.contains("\"result\"")));
    (stream, reader)
}

/// Runs `client` against `session`'s API on its own thread while this one serves `api` as frames
/// would (a tick each), then returns `client`'s result.
fn serving<T: Send + 'static>(
    api: &mut Api,
    cx: &mut Context,
    stack: &mut Stack,
    stepper: &mut Stepper,
    session: Session,
    client: impl FnOnce(Client) -> T + Send + 'static,
    mut frame: impl FnMut(&mut Api, &mut Context, &Stepper),
) -> T {
    let client = thread::spawn(move || {
        client(Client::connect(&session, "test", DEADLINE).expect("it connects"))
    });
    let start = Instant::now();
    while !client.is_finished() {
        assert!(start.elapsed() < DEADLINE, "the client never finished");
        crate::scene_files::look(cx, stack);
        api.serve(cx, stack, stepper);
        stepper.step(Tick::DT, |_| true, || Duration::ZERO);
        frame(api, cx, stepper);
        thread::sleep(Duration::from_millis(1));
    }
    client.join().expect("the client didn't panic")
}

/// What [`a_client_pauses_and_steps_the_simulation_and_reads_the_world`]'s client gets.
struct Read {
    paused: Status,
    stepped: Status,
    status: Status,
    page: EntityList,
    rest: EntityList,
    all: EntityList,
    fielded: EntityList,
    entity: Value,
    kid: Value,
    sprite: Value,
    field: f32,
    types: gg_api::Types,
    hash: Hash,
    refused: Vec<String>,
}

/// A client pauses, steps and reads the world: status, entity pages, an entity, a field, types
/// (described by themselves) and the hash. Wrong requests get a code.
#[test]
fn a_client_pauses_and_steps_the_simulation_and_reads_the_world() {
    let (mut api, session) = started("Api Reads");
    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    let at = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        ..Transform::IDENTITY
    };
    let hero = world.spawn((at, Name("Hero".to_owned())));
    let kid = world.spawn(Transform::IDENTITY);
    world.set_parent(kid, Some(hero)).expect("alive");
    let sprites = [(); 3].map(|()| world.spawn((at, Sprite::default())));
    let [hero, kid, sprite] =
        [hero, kid, sprites[0]].map(|entity| world.id(entity).expect("alive").to_string());
    let mut stepper = Stepper::default();
    let asked = [hero.clone(), kid.clone(), sprite];
    let client = move |mut client: Client| {
        let [hero, kid, sprite] = asked;
        let field = json!({"id": hero, "component": "Transform", "path": "translation.y"});
        let refused = [
            ("entity", json!({"id": "0000000000000001"})),
            ("get", json!({"id": hero, "component": "Nope"})),
            ("step", json!({"ticks": 0})),
            ("bogus", Value::Null),
            ("entities", json!({"fields": ["Nope.x"]})),
            ("entities", json!({"fields": ["Transform.nope"]})),
        ];
        let fields = json!({"fields": ["Transform.translation.y", "Name"]});
        Read {
            paused: client.call_as("pause", ()).expect("pauses"),
            stepped: client.call_as("step", json!({"ticks": 3})).expect("steps"),
            status: client.call_as("status", ()).expect("a status"),
            page: (client.call_as("entities", json!({"with": "Sprite", "take": 2})))
                .expect("a page"),
            rest: client
                .call_as("entities", json!({"skip": 4}))
                .expect("the rest"),
            all: client.call_as("entities", ()).expect("them all"),
            fielded: client.call_as("entities", fields).expect("fields"),
            entity: client
                .call_as("entity", json!({"id": hero}))
                .expect("the hero"),
            kid: client
                .call_as("entity", json!({"id": kid}))
                .expect("the kid"),
            sprite: client
                .call_as("entity", json!({"id": sprite}))
                .expect("a sprite"),
            field: client.call_as("get", field).expect("its y"),
            types: client.call_as("types", ()).expect("the types"),
            hash: client.call_as("hash", ()).expect("a hash"),
            refused: (refused.into_iter())
                .map(|(method, params)| client.call(method, params).expect_err(method).code)
                .collect(),
        }
    };
    let read = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert!(read.paused.paused);
    assert_eq!(
        read.stepped.ticks,
        read.paused.ticks + 3,
        "answered once its ticks ran"
    );
    assert_eq!(
        (read.status.ticks, read.status.paused),
        (read.stepped.ticks, true)
    );
    assert_eq!(read.status.entities, 5);
    assert_eq!((read.page.total, read.page.entities.len()), (3, 2));
    assert!(
        read.page.entities[0]
            .components
            .contains(&"Sprite".to_owned())
    );
    assert_eq!((read.rest.total, read.rest.entities.len()), (5, 1));
    let listed = |id: &str| read.all.entities.iter().find(|listed| listed.id == id);
    let listed = |id: &str| listed(id).expect("listed");
    assert_eq!(listed(&hero).children, std::slice::from_ref(&kid));
    assert_eq!(listed(&kid).parent.as_ref(), Some(&hero));
    assert!(
        read.all
            .entities
            .iter()
            .all(|listed| listed.fields.is_empty())
    );
    // A field of a component an entity lacks is left out.
    let fielded = |id: &str| read.fielded.entities.iter().find(|listed| listed.id == id);
    let fields = |id: &str| &fielded(id).expect("listed").fields;
    let hero_y = fields(&hero).get("Transform.translation.y");
    assert_eq!(hero_y, Some(&json!(2.0)));
    assert!(fields(&hero).contains_key("Name"));
    let kid_fields: Vec<&String> = fields(&kid).keys().collect();
    assert_eq!(kid_fields, ["Transform.translation.y"]);
    assert_eq!(read.entity["name"], "Hero");
    assert_eq!(read.entity["parent"], Value::Null);
    assert_eq!(read.entity["children"], json!([kid]));
    let translation = &read.entity["components"]["Transform"]["translation"];
    assert_eq!(*translation, json!([1.0, 2.0, 3.0]));
    // Root: world space; kid: the hero's space.
    let space = |translation: [f32; 3]| {
        let matrix = Mat4::from_translation(Vec3::from_array(translation));
        json!({"matrix": matrix.to_cols_array(), "rotation": [0.0, 0.0, 0.0, 1.0]})
    };
    assert_eq!(read.entity["space"], space([0.0; 3]));
    assert_eq!(read.kid["space"], space([1.0, 2.0, 3.0]));
    assert_eq!(read.field, 2.0);
    // `bounds` (world space) only where something draws, here or below.
    let bounds = &read.sprite["bounds"];
    assert_eq!(bounds["center"], json!([1.0, 2.0, 3.0]));
    let radius = bounds["radius"].as_f64().expect("a radius") as f32;
    assert_eq!(radius, std::f32::consts::FRAC_1_SQRT_2);
    assert!(read.entity.get("bounds").is_none() && read.kid.get("bounds").is_none());
    let types = cx.world().types();
    let names: Vec<&str> = read
        .types
        .components
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, cx.world().component_types().collect::<Vec<_>>());
    let parent = read.types.components.iter().find(|c| c.name == "Parent");
    let refused = parent.expect("a parent's type").refused.as_deref();
    assert!(
        refused.is_some_and(|why| why.contains("entity")),
        "{refused:?}"
    );
    let described = json::described(&read.types).expect("described");
    assert_eq!(described.types.iter().len(), types.iter().len());
    for ((_, ours), (_, theirs)) in types.iter().zip(described.types.iter()) {
        assert_eq!(ours, theirs, "described as it is");
    }
    let transform = described
        .components
        .iter()
        .find(|(name, _)| name == "Transform");
    let transform = transform.map(|(_, default)| default.as_ref().ok());
    let identity = crate::Reflect::to_value(&Transform::IDENTITY);
    assert_eq!(transform, Some(Some(&identity)));
    assert_eq!(
        read.hash.content,
        format!("{:016x}", cx.world().hash().content)
    );
    let codes = [
        "ecs.no_entity",
        "reflect.unknown_type",
        "api.bad_params",
        "api.unknown_method",
        "reflect.unknown_type",
        "reflect.no_field",
    ];
    assert_eq!(read.refused, codes);
}

/// Where a request takes an entity's ID it takes the name of the one entity with it.
#[test]
fn an_entity_is_named_by_its_id_or_by_the_one_entity_with_its_name() {
    let (mut api, session) = started("Api Names");
    let mut cx = Context::seeded(1);
    let world = cx.world_mut();
    let hero = world.spawn((Transform::IDENTITY, Name("Hero".to_owned())));
    let twins = [(); 2].map(|()| world.spawn(Name("Twin".to_owned())));
    let id = |entity| world.id(entity).expect("alive").to_string();
    let hero = id(hero);
    let twins = twins.map(id);
    let mut stepper = Stepper::default();
    let asked = (hero.clone(), twins.clone());
    let client = move |mut client: Client| {
        let (hero, _) = asked;
        let field = |id: &str| json!({"id": id, "component": "Transform", "path": "translation.y"});
        let entity =
            |client: &mut Client, id: &str| client.call_as::<Value>("entity", json!({"id": id}));
        let by_name = entity(&mut client, "Hero").expect("by name");
        let by_id = entity(&mut client, &hero).expect("by ID");
        let got: f64 = client
            .call_as("get", field("Hero"))
            .expect("a field by name");
        let none = entity(&mut client, "Nobody").expect_err("no such name");
        let two = entity(&mut client, "Twin").expect_err("two with the name");
        // A path may follow the type and `.`, as `entities` takes fields.
        let set = json!({"operations": [{"set": {"id": "Hero",
            "component": "Transform.translation", "path": "y", "value": 5.0}}]});
        let applied = client.call("apply", set).is_ok();
        let dotted = json!({"id": "Hero", "component": "Transform.translation.y"});
        let dotted: f64 = client.call_as("get", dotted).expect("a dotted field");
        (by_name, by_id, got, none, two, applied && dotted == 5.0)
    };
    let (by_name, by_id, got, none, two, applied) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(by_name, by_id);
    assert_eq!(by_name["name"], "Hero");
    assert_eq!(got, 0.0);
    assert_eq!(none.code, "ecs.no_entity");
    assert!(none.message.contains("Nobody"), "{}", none.message);
    assert_eq!(two.code, "ecs.ambiguous_name");
    assert!(two.message.contains("Twin"), "{}", two.message);
    for id in &twins {
        assert!(two.message.contains(id.as_str()), "{}", two.message);
    }
    assert!(
        applied,
        "an operation names an entity as well, and its path its field"
    );
}

/// Held by the tests that set or read what the run waits on, which `ping` says: it's the
/// process's, as a window's is ([`Waiting`]).
static WAITS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// [`WAITS`], held until dropped, though a test that held it panicked.
fn waits() -> std::sync::MutexGuard<'static, ()> {
    WAITS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The first request is `hello` with the token; the connection's thread answers `ping` while no
/// frame serves.
#[test]
fn a_connection_says_hello_with_the_token_and_pings_without_frames() {
    let _waits = waits();
    let (_api, session) = started("Api Hello");
    assert_eq!(session.project.as_deref(), Some(Path::new(PROJECT)));
    let wrong = Session {
        token: "0".repeat(32),
        ..session.clone()
    };
    let refused = Client::connect(&wrong, "test", DEADLINE).err();
    let refused = refused.map(|failure| failure.code);
    assert_eq!(refused.as_deref(), Some("api.unauthorized"));
    let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
    assert_eq!(client.welcome().app, "Api Hello");
    let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
    // Another test's load may say `loading`: loads are the process's.
    assert_eq!(pulse.frames, 0);
    assert_ne!(pulse.waiting.as_deref(), Some("menu"));
    let waiting = super::super::waiting(Waiting::Menu);
    let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
    assert_eq!(pulse.waiting.as_deref(), Some("menu"));
    drop(waiting);
}

/// `ping` says `loading` while a load is under way, on any thread, as the render thread's uploads
/// are; a wait that's the user's doing comes first.
#[test]
fn ping_says_loading_while_a_load_is_under_way() {
    let _waits = waits();
    let (_api, session) = started("Api Loading");
    let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
    let loads = std::thread::spawn(super::super::loading)
        .join()
        .expect("a load");
    let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
    assert_eq!(pulse.waiting.as_deref(), Some("loading"));
    let menu = super::super::waiting(Waiting::Menu);
    let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
    assert_eq!(pulse.waiting.as_deref(), Some("menu"));
    drop((menu, loads));
}

/// `hello` grants a connection its capabilities: its thread refuses requests beyond them with
/// `api.ungranted`, and an unknown capability with `grant.unknown`.
#[test]
fn a_connection_is_kept_to_what_its_hello_grants() {
    let (_api, session) = started("Api Grants");
    let grant = |names: &[&str]| Some(names.iter().map(|&name| name.to_owned()).collect());
    let unknown = Client::connect_granted(&session, "test", grant(&["reed"]), DEADLINE);
    assert_eq!(
        unknown.err().map(|failure| failure.code).as_deref(),
        Some("grant.unknown")
    );
    let granted = Client::connect_granted(&session, "test", grant(&["read", "view"]), DEADLINE);
    let mut client = granted.expect("it connects");
    let set = json!({"operations": [{"set": {
        "id": "00000000000000ff", "component": "Transform", "path": "", "value": {},
    }}]});
    let note = json!({"code": "test.noted", "message": "noted"});
    for (method, params) in [("apply", set), ("note", note), ("quit", Value::Null)] {
        let refused = client.call(method, params).expect_err(method);
        assert_eq!(
            refused.code, "api.ungranted",
            "{method}: {}",
            refused.message
        );
    }
    let pulse: Pulse = client.call_as("ping", ()).expect("a pulse");
    assert_eq!(pulse.frames, 0);
}

/// Whether the game closed `stream`: its read ends or resets, well before it would drop a silent
/// connection.
fn closed(stream: &mut TcpStream) -> bool {
    stream
        .set_read_timeout(Some(HELLO_TIMEOUT / 2))
        .expect("a timeout");
    match stream.read(&mut [0]) {
        Ok(read) => read == 0,
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
        ),
    }
}

/// A `hello` of [`gg_api::MAX_HELLO`] bytes is welcomed; a longer one is refused at once, not after
/// the rest arrives.
#[test]
fn a_hello_longer_than_its_bound_is_refused_unread() {
    let (_api, session) = started("Api Long Hello");
    let hello = |client: &str| {
        let params = json!({"token": session.token, "client": client});
        json!({"id": 7, "method": "hello", "params": params}).to_string()
    };
    let longest = hello(&"a".repeat(gg_api::MAX_HELLO - hello("").len()));
    assert_eq!(longest.len(), gg_api::MAX_HELLO);
    let mut stream = TcpStream::connect(session.address).expect("it connects");
    stream.set_read_timeout(Some(DEADLINE)).expect("a timeout");
    stream
        .write_all(format!("{longest}\n").as_bytes())
        .expect("written");
    let answer = gg_api::read_line(&mut BufReader::new(&stream)).expect("an answer");
    let answer: gg_api::Answer = serde_json::from_str(&answer.expect("a line")).expect("JSON");
    assert_eq!((answer.id, answer.error), (7, None));
    let mut stream = TcpStream::connect(session.address).expect("it connects");
    stream
        .write_all(&[b' '; gg_api::MAX_HELLO + 1])
        .expect("written");
    assert!(closed(&mut stream), "the game waited for the rest");
}

/// A `hello` trickling in a byte at a time is cut off at its deadline, though each byte comes in
/// time.
#[test]
fn a_hello_dripped_a_byte_at_a_time_is_cut_off_at_its_deadline() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("it listens");
    let address = listener.local_addr().expect("its address");
    let mut client = TcpStream::connect(address).expect("it connects");
    client.set_nodelay(true).expect("no delay");
    let (served, _) = listener.accept().expect("accepted");
    let token = "0".repeat(32);
    let hello = format!("{{\"id\":0,\"method\":\"hello\",\"params\":{{\"token\":\"{token}\"}}}}\n");
    // 80 bytes take 1.6 s.
    let dripping = thread::spawn(move || {
        for byte in hello.bytes() {
            if client.write_all(&[byte]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let mut reader = BufReader::new(served);
    let deadline = Duration::from_millis(250);
    let start = Instant::now();
    let read = read_hello(&mut reader, start + deadline);
    let took = start.elapsed();
    // Ends the client's writes.
    drop(reader);
    dripping.join().expect("the client didn't panic");
    assert_eq!(read, None);
    assert!(
        took >= deadline && took < Duration::from_secs(1),
        "{took:?}"
    );
}

/// At [`MOST_SAYING_HELLO`] unfinished hellos, the game closes the next unread connection until one
/// has gone.
#[test]
fn connections_past_the_most_saying_hello_are_refused() {
    let (_api, session) = started("Api Hellos");
    let connect = || TcpStream::connect(session.address).expect("it connects");
    let mut saying: Vec<TcpStream> = (0..MOST_SAYING_HELLO).map(|_| connect()).collect();
    assert!(
        closed(&mut connect()),
        "a connection past the most was served"
    );
    let refused = Client::connect(&session, "test", DEADLINE).err();
    assert!(refused.is_some(), "a client past the most was welcomed");
    drop(saying.pop());
    // After the game sees it go.
    let start = Instant::now();
    let client = loop {
        if let Ok(client) = Client::connect(&session, "test", DEADLINE) {
            break client;
        }
        assert!(start.elapsed() < DEADLINE, "no client was welcomed");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(client.welcome().app, "Api Hellos");
}

/// A client sending faster than requests are answered (no frame here) is held back at
/// [`gg_api::MOST_IN_FLIGHT`]: the game stops reading, so the client's writes block once the
/// loopback's buffers fill.
#[test]
fn a_client_that_sends_faster_than_its_answers_go_is_held_back() {
    let (_api, session) = started("Api In Flight");
    let (mut stream, _) = said_hello(&session);
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("a timeout");
    // 64 MiB, far beyond the loopback's buffers, in chunks, so each write waits only for the game.
    let status = "{\"id\":1,\"method\":\"status\"}\n";
    let chunk = status.repeat((64 << 10) / status.len());
    let mut sent = 0;
    let held = loop {
        if sent >= 64 << 20 {
            break None;
        }
        if let Err(error) = stream.write_all(chunk.as_bytes()) {
            break Some(error.kind());
        }
        sent += chunk.len();
    };
    let held = held
        .is_some_and(|kind| matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut));
    assert!(
        held,
        "the game read {sent} bytes of requests it didn't answer"
    );
}

/// Hands events to `events`; until `most` came, has its client send another `input` once the game
/// has passed it to the main loop (`wakes`).
struct Sending {
    stream: TcpStream,
    wakes: Receiver<()>,
    events: Rc<Cell<usize>>,
    most: usize,
}

impl crate::Layer for Sending {
    fn event(&mut self, _: &mut Context, _: &Event<'_>) -> std::ops::ControlFlow<()> {
        self.events.set(self.events.get() + 1);
        if self.events.get() < self.most {
            let input = json!({"id": 1, "method": "input", "params": {"events": ["cursor_left"]}});
            (self.stream)
                .write_all(format!("{input}\n").as_bytes())
                .expect("written");
            self.wakes.recv_timeout(DEADLINE).expect("passed on");
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// A frame answers requests that came before it started; later ones wait for the next, so a client
/// that keeps sending can't stall it.
#[test]
fn a_frame_answers_the_requests_that_came_before_it_started() {
    let (mut api, session) = started("Api Frames Go On");
    let (passed, wakes) = mpsc::channel();
    api.set_waker(Box::new(move || {
        let _ = passed.send(());
    }));
    let (mut stream, mut answers) = said_hello(&session);
    let input = json!({"id": 1, "method": "input", "params": {"events": ["cursor_left"]}});
    stream
        .write_all(format!("{input}\n").as_bytes())
        .expect("written");
    wakes.recv_timeout(DEADLINE).expect("passed on");
    let events = Rc::new(Cell::new(0));
    let mut stack = Stack::default();
    stack.push_layer(Sending {
        stream: stream.try_clone().expect("a clone"),
        wakes,
        events: events.clone(),
        most: 3,
    });
    let mut cx = Context::seeded(13);
    let mut stepper = Stepper::default();
    for frame in 1..=3 {
        api.serve(&mut cx, &mut stack, &mut stepper);
        assert_eq!(
            events.get(),
            frame,
            "the frame answered those that came after it started"
        );
        let answer = gg_api::read_line(&mut answers).expect("an answer");
        assert_eq!(answer.as_deref(), Some("{\"id\":1,\"result\":{}}"));
    }
}

/// Requests need their own capability, or `read` or `write` of the one type named (see gg-api).
#[test]
fn a_request_needs_what_its_method_and_the_types_it_names_need() {
    let grant = Grant::parse(["read:Transform", "view", "write:Sprite"]).expect("a grant");
    let path = std::env::temp_dir().join("granted.png");
    let set = |component: &str| {
        json!({"operations": [{"set": {
            "id": "00000000000000ff", "component": component, "path": "", "value": {},
        }}]})
    };
    let get = |component: &str| json!({"id": "00000000000000ff", "component": component});
    let delete = json!({"operations": [{"delete": {"id": "00000000000000ff"}}]});
    let asked = [
        ("status", Value::Null, None),
        ("get", get("Transform"), None),
        ("get", get("Sprite"), Some("read:Sprite")),
        ("entities", Value::Null, Some("read")),
        ("capture", Value::Null, None),
        ("capture", json!({ "path": path }), Some("files")),
        ("look", Value::Null, None),
        ("apply", set("Sprite"), None),
        ("apply", set("Transform"), Some("write:Transform")),
        ("apply", delete, Some("write")),
        (
            "save_prefab",
            json!({"id": "00000000000000ff"}),
            Some("write"),
        ),
        ("undo", Value::Null, Some("write")),
        ("step", json!({"ticks": 1}), Some("play")),
        ("snapshot", Value::Null, Some("snapshot")),
        ("quit", Value::Null, Some("quit")),
    ];
    for (method, params, needed) in asked {
        let request = Request {
            id: 1,
            method: method.to_owned(),
            params,
            key: None,
            again: false,
        };
        let call = parse(&request).expect("a call");
        let refused = granted(&grant, method, &needs(&call)).err();
        let named = needed.map(|needed| format!("`{method}` needs `{needed}`,"));
        let message = refused.map(|failure| (failure.code, failure.message));
        match (named, message) {
            (None, None) => {}
            (Some(named), Some((code, message))) => {
                assert_eq!(code, "api.ungranted");
                assert!(message.starts_with(&named), "{message}");
            }
            (named, message) => panic!("{method}: {named:?}, {message:?}"),
        }
    }
}

/// What [`a_client_debugs_a_script_while_the_game_waits_at_its_stop`]'s client gets.
struct Debugged {
    before: gg_api::Debugging,
    stopped: gg_api::Debugging,
    pulse: Pulse,
    stepped: gg_api::Debugging,
    gone_on: gg_api::Debugging,
    refused: Vec<String>,
}

/// A client sets a script breakpoint, waits for the game to stop there (`ping` says so while the
/// main thread waits in the tick), steps a line, clears it and resumes. Wrong requests get a code.
#[test]
fn a_client_debugs_a_script_while_the_game_waits_at_its_stop() {
    let _waits = waits();
    let (mut api, session) = started("Api Debug");
    let dir = Scratch::new("api-debug");
    let (mut cx, mut host) = counting(&api, &dir);
    let pinging = session.clone();
    let client = move |mut client: Client| {
        let mut debug = |params: Value| -> gg_api::Debugging {
            client.call_as("debug", params).expect("an answer")
        };
        let before = debug(json!({}));
        let lines = json!({"script": "scripts/count.luau", "lines": [4]});
        let stopped = debug(json!({"breakpoints": lines, "wait_ms": 10_000}));
        let mut pinged = Client::connect(&pinging, "pinger", DEADLINE).expect("it connects");
        let pulse = pinged.call_as("ping", ()).expect("a pulse");
        let stepped = debug(json!({"action": "step_over", "wait_ms": 10_000}));
        let none = json!({"script": "scripts/count.luau", "lines": []});
        let gone_on = debug(json!({"breakpoints": none, "action": "continue"}));
        let refused = [
            json!({"action": "continue"}),
            json!({"breakpoints": {"script": "", "lines": [1]}}),
            json!({"breakpoints": {"script": "scripts/count.luau", "lines": [0]}}),
            json!({"wait_ms": 10_001}),
            json!({"action": "nope"}),
        ];
        let refused = (refused.into_iter())
            .map(|params| client.call("debug", params).expect_err("refused").code)
            .collect();
        Debugged {
            before,
            stopped,
            pulse,
            stepped,
            gone_on,
            refused,
        }
    };
    let got = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut Stepper::default(),
        session,
        client,
        |_, cx, _| {
            cx.hosts.place();
            host.run(cx.world_mut());
        },
    );
    assert_eq!(got.before.scripts, ["scripts/count.luau"]);
    assert_eq!((got.before.stopped, got.before.stops), (None, 0));
    let stop = got.stopped.stopped.expect("stopped");
    let at = |stop: &gg_api::Stop| (stop.why, stop.frames[0].line);
    assert_eq!(at(&stop), (gg_api::StopReason::Breakpoint, 4));
    let placed = &got.stopped.breakpoints;
    assert_eq!((placed[0].line, placed[0].placed), (4, Some(4)));
    assert_eq!(got.pulse.waiting.as_deref(), Some("breakpoint"));
    let stepped = got.stepped.stopped.expect("stopped");
    assert_eq!(at(&stepped), (gg_api::StopReason::Step, 5));
    let n = |stop: &gg_api::Stop| {
        let locals = &stop.frames[0].locals;
        let n = locals
            .iter()
            .find(|local| local.name == "n")
            .expect("a local");
        n.value.as_f64().expect("a number")
    };
    assert_eq!(n(&stepped), n(&stop) + 1.0, "its line ran");
    assert_eq!((got.gone_on.stopped, got.gone_on.stops), (None, 2));
    assert!(got.gone_on.breakpoints.is_empty());
    assert_eq!(
        got.refused,
        [
            "script.not_stopped",
            "api.bad_params",
            "api.bad_params",
            "api.bad_params",
            "api.bad_params",
        ]
    );
}

/// A context rooted in `dir` whose host runs `scripts/count.luau` for an entity, counting at line
/// 4 each tick, its stops `api`'s debugger's.
fn counting(api: &Api, dir: &Scratch) -> (Context, crate::Scripts) {
    fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
    let path = dir.0.join("scripts/count.luau");
    let script = "local Count = gg.declare(\"Count\", { n = 0 })\n\
            function Count.tick(entity, dt)\n\
                local n = entity[Count.n]\n\
                n = n + 1\n\
                entity[Count.n] = n\n\
            end\n\
            return Count\n";
    fs::write(&path, script).expect("written");
    let mut cx = Context::seeded(1).root(&dir.0);
    cx.hosts.debug(api.debugger());
    let mut host = crate::Scripts::new().expect("Luau starts");
    host.load(&mut cx, &path).expect("the script loads");
    let world = cx.world_mut();
    let counted = world.spawn(());
    world.add_default(counted, "Count").expect("added");
    (cx, host)
}

/// What [`a_stop_no_client_asks_about_goes_on_with_its_breakpoints_cleared`]'s client gets.
struct Unattended {
    stopped: gg_api::Debugging,
    pulse: Pulse,
    later: Pulse,
    after: gg_api::Debugging,
    refused: gg_api::Failure,
}

/// A client sets a breakpoint, sees the game stop there, and asks nothing more, as one that ended:
/// once the lease passes, the script goes on and every breakpoint is cleared. `ping` stops saying
/// it waits, frames count on, and a continue says why nothing has stopped.
#[test]
fn a_stop_no_client_asks_about_goes_on_with_its_breakpoints_cleared() {
    let _waits = waits();
    let (mut api, session) = started("Api Unattended");
    api.debugger().lease(Duration::from_millis(300));
    let dir = Scratch::new("api-unattended");
    let (mut cx, mut host) = counting(&api, &dir);
    let pinging = session.clone();
    let client = move |mut client: Client| {
        let lines = json!({"script": "scripts/count.luau", "lines": [4]});
        let asked = json!({"breakpoints": lines, "wait_ms": 10_000});
        let stopped = client.call_as("debug", asked).expect("an answer");
        let mut pinged = Client::connect(&pinging, "pinger", DEADLINE).expect("it connects");
        let pulse: Pulse = pinged.call_as("ping", ()).expect("a pulse");
        std::thread::sleep(Duration::from_millis(1_000));
        let later = pinged.call_as("ping", ()).expect("a pulse");
        let after = client.call_as("debug", json!({})).expect("an answer");
        let go_on = json!({"action": "continue"});
        let refused = client.call("debug", go_on).expect_err("refused");
        Unattended {
            stopped,
            pulse,
            later,
            after,
            refused,
        }
    };
    let got = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut Stepper::default(),
        session,
        client,
        |_, cx, _| {
            cx.hosts.place();
            host.run(cx.world_mut());
        },
    );
    let stop = got.stopped.stopped.expect("stopped");
    assert_eq!((stop.number, stop.frames[0].line), (1, 4));
    assert_eq!(got.pulse.waiting.as_deref(), Some("breakpoint"));
    // Another test's load may say `loading`: loads are the process's.
    assert_ne!(got.later.waiting.as_deref(), Some("breakpoint"));
    assert!(got.later.frames > got.pulse.frames, "frames count on");
    assert_eq!((got.after.stopped, got.after.stops), (None, 1));
    assert!(got.after.breakpoints.is_empty());
    assert_eq!(got.refused.code, "script.not_stopped");
    assert!(
        got.refused.message.contains("no `debug` asked"),
        "{}",
        got.refused.message
    );
}

/// A request with a key is done once: sent again with that key, from another connection too, it
/// gets the first's answer, and nothing more is done. Another key is done again.
#[test]
fn a_request_sent_again_with_its_key_is_answered_not_done_again() {
    let (mut api, session) = started("Api Keyed");
    let mut cx = Context::seeded(1);
    let again = session.clone();
    let client = move |mut client: Client| {
        let status =
            |client: &mut Client| -> Status { client.call_as("status", ()).expect("a status") };
        let _: Status = client.call_as("pause", ()).expect("paused");
        let before = status(&mut client).ticks;
        let step = || json!({"ticks": 2});
        let first = client
            .call_keyed("step", step(), Some("k1"))
            .expect("stepped");
        let mut other = Client::connect(&again, "other", DEADLINE).expect("it connects");
        let second = other
            .call_keyed("step", step(), Some("k1"))
            .expect("answered");
        let after = status(&mut client).ticks;
        let third = client
            .call_keyed("step", step(), Some("k2"))
            .expect("stepped");
        let last = status(&mut client).ticks;
        (
            before,
            first.get().to_owned(),
            second.get().to_owned(),
            after,
            third.get().to_owned(),
            last,
        )
    };
    let (before, first, second, after, third, last) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut Stepper::default(),
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(first, second, "the first's answer");
    assert_eq!(after, before + 2, "stepped once");
    assert_ne!(third, first);
    assert_eq!(last, before + 4, "another key steps again");
}

/// A request whose key is under way waits for that one's answer, and gets it too; one of another
/// method with the key is apart. The oldest answered key goes once [`gg_api::KEPT_KEYS`] more are
/// answered, one under way staying: sent again then, it's refused, doing nothing.
#[test]
fn a_request_whose_key_is_under_way_gets_that_ones_answer() {
    let keyed = Arc::new(Mutex::new(Keyed::default()));
    let (to, answers) = mpsc::channel();
    let take =
        |method: &str, key: &str, id| Keyed::take(&keyed, method, key, false, reply(id, &to));
    let waiting = take("step", "w", 0).expect("not kept yet");
    let first = take("step", "k", 1).expect("not kept yet");
    assert!(take("step", "k", 2).is_none());
    assert!(answers.try_recv().is_err(), "nothing answered yet");
    first.send(Ok("7".to_owned()));
    let mut lines: Vec<String> = answers.try_iter().map(|(line, _)| line).collect();
    lines.sort();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("\"id\":1") && lines[1].contains("\"id\":2"),
        "{lines:?}"
    );
    assert!(
        lines.iter().all(|line| line.contains("\"result\":7")),
        "{lines:?}"
    );
    assert!(take("step", "k", 3).is_none());
    assert!(answers.try_recv().is_ok(), "answered as it was");
    assert!(take("look", "k", 4).is_some(), "another method's");
    for key in 0..gg_api::KEPT_KEYS {
        let reply = take("step", &key.to_string(), 5).expect("not kept yet");
        reply.send(Ok("0".to_owned()));
    }
    let again = Keyed::take(&keyed, "step", "k", true, reply(6, &to));
    assert!(again.is_none(), "not done again");
    let refused = answers
        .try_iter()
        .last()
        .map(|(line, _)| line)
        .unwrap_or_default();
    assert!(refused.contains("api.forgotten"), "{refused}");
    assert!(take("step", "k", 7).is_some(), "forgotten, so done anew");
    assert!(take("step", "w", 7).is_none(), "still under way");
    drop(waiting);
}

/// A token matches only if every byte does: not with a differing first or last byte, a prefix,
/// extra bytes or nothing.
#[test]
fn a_token_given_is_the_token_only_if_every_byte_is() {
    let token = "0123456789abcdef0123456789abcdef";
    assert!(same_token(token, token));
    let longer = format!("{token}0");
    for given in [
        "f123456789abcdef0123456789abcdef",
        "0123456789abcdef0123456789abcdee",
        "0123456789abcdef",
        &longer,
        "",
    ] {
        assert!(!same_token(given, token), "{given:?}");
    }
}

/// The `n` of each event `read` read.
fn numbers(read: &LogEvents) -> Vec<u64> {
    let events = read.events.iter().map(|event| {
        let event: Value = serde_json::from_str(event.get()).expect("an object");
        event["n"].as_u64().expect("its number")
    });
    events.collect()
}

/// Following logs while no frame serves: events at a level and above, from the oldest or last few,
/// plus new ones as they arrive. Another run's cursor reads from the oldest. Wrong requests get a
/// code.
#[test]
fn a_client_follows_the_logs_without_frames() {
    let events: &'static Events = Box::leak(Box::default());
    let (_api, session) = started_keeping("Api Logs", events);
    let line = |n: u64, level: Level| format!(r#"{{"level":"{level}","n":{n}}}"#);
    for (n, level) in [Level::INFO, Level::WARN, Level::DEBUG]
        .into_iter()
        .enumerate()
    {
        events.push(level, &line(n as u64, level));
    }
    let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
    let all: LogEvents = client.call_as("logs", ()).expect("the events");
    assert_eq!(numbers(&all), [0, 1, 2]);
    assert_eq!((all.next, all.end, all.missed), (3, 3, 0));
    let warned: LogEvents =
        (client.call_as("logs", json!({"level": "warn"}))).expect("the warnings");
    assert_eq!((numbers(&warned), warned.next), (vec![1], 3));
    let last: LogEvents = client
        .call_as("logs", json!({"last": 1}))
        .expect("the last");
    assert_eq!(numbers(&last), [2]);
    assert_eq!(last.run, all.run, "one run's");

    let pusher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        events.push(Level::ERROR, &line(3, Level::ERROR));
    });
    let next = json!({"from": all.next, "run": all.run, "level": "error", "wait_ms": 10_000});
    let start = Instant::now();
    let waited: LogEvents = client.call_as("logs", next).expect("the next");
    pusher.join().expect("the pusher didn't panic");
    assert!(start.elapsed() < DEADLINE, "answered as it came");
    assert_eq!((numbers(&waited), waited.next), (vec![3], 4));

    let other = json!({"from": waited.next, "run": all.run ^ 1});
    let other: LogEvents = client.call_as("logs", other).expect("the events");
    assert_eq!(numbers(&other), [0, 1, 2, 3]);
    for refused in [json!({"level": "loud"}), json!({"wait_ms": 10_001})] {
        let code = client
            .call("logs", refused.clone())
            .err()
            .map(|failure| failure.code);
        assert_eq!(code.as_deref(), Some("api.bad_params"), "{refused}");
    }
}

/// A note is answered by the connection's thread while no frame serves. Non-notes get a code: an
/// error's level, another code shape, no message, or too long.
#[test]
fn a_client_notes_an_event_without_frames_or_is_refused() {
    let (_api, session) = started("Api Notes");
    let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
    let note = |code: &str, level: &str, message: &str| {
        json!({
            "code": code,
            "level": level,
            "message": message,
        })
    };
    let mut noted = note("gg.patch_failed", "warn", "A patch didn't build");
    noted["fields"] = json!({"files": ["src/main.rs"]});
    let answer = client.call("note", noted).expect("noted");
    assert_eq!(answer.get(), "{}");
    let long = "x".repeat(gg_api::MOST_NOTE + 1);
    let with = |key: &str, value: Value| {
        let mut note = note("gg.patch_failed", "info", "m");
        note[key] = value;
        note
    };
    for refused in [
        note("gg.patch_failed", "error", "m"),
        note("gg", "info", "m"),
        note("Gg.patch_failed", "info", "m"),
        note("gg..patch_failed", "info", "m"),
        note("gg.patch-failed", "info", "m"),
        note("gg.1patch", "info", "m"),
        note(
            &format!("gg.{}", "x".repeat(gg_api::MOST_NOTE_CODE)),
            "info",
            "m",
        ),
        note("gg.patch_failed", "info", ""),
        note("gg.patch_failed", "info", &long),
        with("fields", json!(["src/main.rs"])),
        with("fields", json!({"files": long})),
        with("by", json!("me")),
        json!({"code": "gg.patch_failed", "level": "info"}),
        Value::Null,
    ] {
        let code = client.call("note", refused.clone()).err();
        let code = code.map(|failure| failure.code);
        assert_eq!(code.as_deref(), Some("api.bad_params"), "{refused}");
    }
}

/// A JSON layer's event lines, as the run's log has them.
#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut written = self.0.lock().expect("no thread panics holding it");
        written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A note is logged like any event: level, code, message, the client, and its fields (if any) as
/// one JSON text field.
#[test]
fn a_note_is_logged_at_its_level_as_its_clients() {
    use tracing_subscriber::layer::SubscriberExt;

    let written = Written::default();
    let writer = written.clone();
    // Two dispatchers, for the reason `testing::logged` gives.
    let _other = tracing::subscriber::set_default(tracing_subscriber::registry());
    let json = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_writer(move || writer.clone());
    let _logging = tracing::subscriber::set_default(tracing_subscriber::registry().with(json));
    let warned = json!({"code": "gg.attach_changed", "level": "warn",
        "message": "A layer's attach changed", "fields": {"files": ["src/main.rs"]}});
    assert_eq!(note(&warned, "gg").as_deref(), Ok("{}"));
    let told = json!({"code": "agent.checked", "level": "info", "message": "Checked"});
    assert_eq!(note(&told, "claude-code").as_deref(), Ok("{}"));
    let text = written
        .0
        .lock()
        .expect("no thread panics holding it")
        .clone();
    let text = String::from_utf8(text).expect("UTF-8");
    let lines: Vec<Value> = (text.lines())
        .map(|line| serde_json::from_str(line).expect("an event"))
        .collect();
    assert_eq!(lines.len(), 2, "{text}");
    let field = |line: &Value, name: &str| line.get(name).cloned().unwrap_or(Value::Null);
    let logged =
        |line: &Value| ["level", "code", "message", "client"].map(|name| field(line, name));
    let expected = [
        "WARN",
        "gg.attach_changed",
        "A layer's attach changed",
        "gg",
    ];
    assert_eq!(logged(&lines[0]), expected.map(Value::from));
    let fields = field(&lines[0], "fields");
    let fields = fields.as_str().expect("JSON text");
    let fields: Value = serde_json::from_str(fields).expect("JSON");
    assert_eq!(fields, json!({"files": ["src/main.rs"]}));
    let expected = ["INFO", "agent.checked", "Checked", "claude-code"];
    assert_eq!(logged(&lines[1]), expected.map(Value::from));
    assert_eq!(field(&lines[1], "fields"), Value::Null, "none given");
}

/// One gesture's sets of one field undo as one edit, of one or several entities; a gesture holds
/// sets of one field, never two of one entity.
#[test]
fn a_gestures_sets_undo_as_one_edit() {
    let (mut api, session) = started("Api Gestures");
    let mut cx = Context::seeded(3);
    let hero = cx.world_mut().spawn(Transform::IDENTITY);
    let foe = cx.world_mut().spawn(Transform::IDENTITY);
    let [hero_id, foe_id] = [hero, foe].map(|e| cx.world().id(e).expect("alive").to_string());
    let mut stepper = Stepper::default();
    stepper.pause();
    let client = move |mut client: Client| {
        let set = |id: &str, path: &str, x: i32| json!({"set": {"id": id, "component": "Transform", "path": path, "value": x}});
        for x in [1, 2, 3] {
            let apply = json!({"operations": [set(&hero_id, "translation.x", x)], "gesture": 9});
            client.call_as::<Applied>("apply", apply).expect("set");
        }
        let history: History = client.call_as("edits", ()).expect("the edits");
        let undone: Option<gg_api::Edit> = client.call_as("undo", ()).expect("undone");
        // Two entities' sets, twice, as one gesture.
        for x in [4, 5] {
            let sets = [
                set(&hero_id, "translation.x", x),
                set(&foe_id, "translation.x", x),
            ];
            let apply = json!({"operations": sets, "gesture": 10});
            client.call_as::<Applied>("apply", apply).expect("sets");
        }
        let both: History = client.call_as("edits", ()).expect("the edits");
        let undone_both: Option<gg_api::Edit> = client.call_as("undo", ()).expect("undone");
        let refuse = |client: &mut Client, operations: Value| {
            let apply = json!({"operations": operations, "gesture": 11});
            client.call("apply", apply).expect_err("refused").code
        };
        let same = refuse(
            &mut client,
            json!([
                set(&hero_id, "translation.x", 6),
                set(&hero_id, "translation.x", 7)
            ]),
        );
        let fields = refuse(
            &mut client,
            json!([
                set(&hero_id, "translation.x", 6),
                set(&foe_id, "translation.y", 7)
            ]),
        );
        let none = refuse(&mut client, json!([]));
        let after: History = client.call_as("edits", ()).expect("the edits");
        (
            (history, undone),
            (both, undone_both),
            [same, fields, none],
            after,
        )
    };
    let (one, two, refused, after) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(one.0.authors[0].done.len(), 1, "one edit: {:?}", one.0);
    assert!(one.1.is_some());
    assert_eq!(two.0.authors[0].done.len(), 1, "one edit: {:?}", two.0);
    assert!(two.1.is_some());
    assert_eq!(refused, ["api.bad_params"; 3]);
    assert_eq!(
        after.authors[0].done.len(),
        0,
        "the refused changed nothing"
    );
    let x = |e| {
        cx.world()
            .get::<Transform>(e)
            .expect("a transform")
            .translation
            .x
    };
    assert_eq!([x(hero), x(foe)], [0.0, 0.0], "both gestures undone whole");
}

/// An edit naming a texture no layer loaded has it loaded from its file under the root before the
/// edit's read; one that can't load is refused with why.
#[test]
fn an_edit_loads_the_textures_it_names() {
    let (mut api, session) = started("Api Textures");
    let dir = Scratch::new("api-textures");
    fs::create_dir_all(dir.0.join("assets")).expect("a directory");
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    fs::write(dir.0.join("assets/stone.png"), png).expect("a PNG");
    let id = "5f3c9a1e07b2d846";
    let meta = gg_api::asset::Meta {
        id: id.parse().expect("an ID"),
    };
    fs::write(dir.0.join("assets/stone.png.meta"), meta.text()).expect("its ID file");
    let mut cx = Context::seeded(3).root(dir.0.clone());
    let hero = cx
        .world_mut()
        .spawn((Transform::IDENTITY, Sprite::default()));
    let hero_id = cx.world().id(hero).expect("alive").to_string();
    let mut stepper = Stepper::default();
    stepper.pause();
    let client = move |mut client: Client| {
        let set = |texture: &str| {
            let set =
                json!({"id": hero_id, "component": "Sprite", "path": "texture", "value": texture});
            json!({"operations": [{"set": set}]})
        };
        let applied = client.call_as::<Applied>("apply", set(id));
        let refused = client.call("apply", set("assets/none.png"));
        (applied, refused.expect_err("no file"))
    };
    let (applied, refused) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    applied.expect("set");
    assert_eq!(refused.code, "api.no_texture", "{}", refused.message);
    assert!(
        refused.message.contains("engine.bad_texture"),
        "{}",
        refused.message
    );
    let sprite = cx.world().get::<Sprite>(hero).expect("a sprite");
    let texture = sprite.texture.expect("a texture");
    assert_eq!(cx.world().assets().path(texture), Some("assets/stone.png"));
}

/// An edit naming a built-in shape has it made before the edit's read, as a file's model is
/// loaded: a created root spawns its node. A `gg:` name that's no shape's is refused with why.
#[test]
fn an_edit_makes_the_shapes_it_names() {
    let (mut api, session) = started("Api Shapes");
    let mut cx = Context::seeded(3);
    let mut stepper = Stepper::default();
    stepper.pause();
    let client = move |mut client: Client| {
        let create = |model: &str| {
            let components = json!({"Transform": {}, "ModelRoot": {"0": model}});
            json!({"operations": [{"create": {"components": components}}]})
        };
        let applied = client.call_as::<Applied>("apply", create("gg:capsule"));
        let refused = client.call("apply", create("gg:cone"));
        (applied, refused.expect_err("no such shape"))
    };
    let (applied, refused) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let applied = applied.expect("created");
    assert_eq!(refused.code, "api.no_model", "{}", refused.message);
    assert!(
        refused.message.contains("asset.unknown_shape"),
        "{}",
        refused.message
    );
    let world = cx.world();
    let id = applied.entities[0].parse::<EntityId>().expect("an ID");
    let root = world.entity(id).expect("created");
    let capsule = world.assets().model("gg:capsule").expect("made");
    assert_eq!(
        world.get::<crate::ModelRoot>(root),
        Some(&crate::ModelRoot(capsule))
    );
    let node = world.children(root).expect("alive").next().expect("a node");
    let drawn = world.get::<crate::MeshDraw>(node);
    assert!(drawn.is_some(), "drawing");
}

/// What [`clients_apply_edits_and_each_undoes_its_own`]'s clients get.
struct Edited {
    set: Applied,
    created: Applied,
    refused: [Failure; 3],
    others: Option<gg_api::Edit>,
    history: History,
    undone: Option<gg_api::Edit>,
    redone: Option<gg_api::Edit>,
    undone_for_them: Option<gg_api::Edit>,
    nothing: Option<gg_api::Edit>,
}

/// Edits apply while paused, each all or nothing. Another client undoes only its own, except for an
/// author it names. Undo and redo step through an edit's operations together.
#[test]
fn clients_apply_edits_and_each_undoes_its_own() {
    let (mut api, session) = started("Api Edits");
    let mut cx = Context::seeded(2);
    let hero = cx
        .world_mut()
        .spawn((Transform::IDENTITY, Name("Hero".to_owned())));
    let hero_id = cx.world().id(hero).expect("alive").to_string();
    let mut stepper = Stepper::default();
    stepper.pause();
    let other = session.clone();
    let asked = hero_id.clone();
    let client = move |mut client: Client| {
        let hero = asked;
        let apply = |client: &mut Client, operations: Value| {
            client.call_as::<Applied>("apply", json!({ "operations": operations }))
        };
        let set = json!([
            {"set": {"id": hero, "component": "Transform", "path": "translation.x", "value": 2}},
            {"set": {"id": hero, "component": "Name", "path": "0", "value": "Ace"}},
        ]);
        let set = apply(&mut client, set).expect("set");
        // Missing fields take their defaults; the parent is the hero.
        let create = json!([{"create": {"parent": hero, "components": {
            "Transform": {"scale": [2, 2, 2]},
            "Name": {"0": "Kid"},
        }}}]);
        let created = apply(&mut client, create).expect("created");
        let refused = [
            // The second's refusal undoes the first.
            json!([
                {"set": {"id": hero, "component": "Transform", "path": "translation.y", "value": 5}},
                {"delete": {"id": "0000000000000001"}},
            ]),
            json!([{"set": {"id": hero, "component": "Transform", "path": "translation", "value": 1}}]),
            json!([{"add": {"id": hero, "component": "Nope"}}]),
        ]
        .map(|operations| apply(&mut client, operations).expect_err("refused"));
        let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
        let others = them.call_as("undo", ()).expect("nothing of theirs");
        let history = client.call_as("edits", ()).expect("the edits");
        let undone = client.call_as("undo", ()).expect("the create undone");
        let redone = client.call_as("redo", ()).expect("redone");
        let undone_for_them = them
            .call_as("undo", json!({"author": "test"}))
            .expect("the create undone for test");
        client
            .call_as::<Option<gg_api::Edit>>("undo", ())
            .expect("the set undone");
        let nothing = client.call_as("undo", ()).expect("nothing left");
        Edited {
            set,
            created,
            refused,
            others,
            history,
            undone,
            redone,
            undone_for_them,
            nothing,
        }
    };
    let edited = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(edited.set.entities, Vec::<String>::new());
    assert_eq!(edited.created.entities.len(), 1);
    let kid = &edited.created.entities[0];
    let codes = edited.refused.map(|failure| failure.code);
    assert_eq!(
        codes,
        [
            "ecs.no_entity",
            "reflect.wrong_kind",
            "reflect.unknown_type"
        ]
    );
    assert_eq!(edited.others, None);
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|&word| word.to_owned())
            .collect::<Vec<_>>()
    };
    let expected = Authored {
        author: "test".to_owned(),
        done: vec![
            words(&[
                &format!("set `Transform.translation.x` of {hero_id}"),
                &format!("set `Name.0` of {hero_id}"),
            ]),
            words(&[&format!("create {kid}")]),
        ],
        undone: Vec::new(),
    };
    assert_eq!(edited.history.authors, [expected]);
    let create = gg_api::Edit {
        author: "test".to_owned(),
        operations: words(&[&format!("create {kid}")]),
    };
    assert_eq!(edited.undone.as_ref(), Some(&create));
    assert_eq!(edited.redone.as_ref(), Some(&create));
    assert_eq!(edited.undone_for_them.as_ref(), Some(&create));
    assert_eq!(edited.nothing, None);
    // Every edit undone; the refused ones never applied.
    let world = cx.world();
    assert_eq!(world.entities(None).expect("all").len(), 1);
    assert_eq!(world.get::<Transform>(hero), Some(&Transform::IDENTITY));
    assert_eq!(
        world.get::<Name>(hero).map(|name| name.0.as_str()),
        Some("Hero")
    );
    assert_eq!(stepper.ticks, 0, "paused, edits apply all the same");
}

/// `quit` is answered, and ends the run once the frame's served.
#[test]
fn quit_is_answered_and_ends_the_run() {
    let (mut api, session) = started("Api Quit");
    let mut cx = Context::seeded(3);
    let mut stepper = Stepper::default();
    let client = thread::spawn(move || {
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        client
            .call("quit", Value::Null)
            .map(|result| result.get().to_owned())
    });
    let start = Instant::now();
    while !api.serve(&mut cx, &mut Stack::default(), &mut stepper) {
        assert!(start.elapsed() < DEADLINE, "no quit came");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(client.join().expect("no panic").as_deref(), Ok("{}"));
}

/// `snapshot` writes the whole world from its own thread, answering path, ticks, entities and
/// bytes; a run restores from it. A relative path or a snapshot before the first tick is refused.
#[test]
fn a_snapshot_is_written_where_its_asked() {
    let (mut api, session) = started("Api Snapshots");
    let mut cx = Context::seeded(5);
    for _ in 0..3 {
        cx.world_mut()
            .spawn((Transform::IDENTITY, Sprite::default()));
    }
    // Paused, so no tick runs before the client asks.
    let mut stepper = Stepper::default();
    stepper.pause();
    let dir = Scratch::new("api-snapshot");
    fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("world.snapshot");
    let asked = path.clone();
    let client = move |mut client: Client| {
        let mut refused = Vec::new();
        for path in [json!("world.snapshot"), json!(asked)] {
            let call = client.call("snapshot", json!({ "path": path }));
            refused.push(call.expect_err("refused").code);
        }
        client.call("step", json!({"ticks": 1})).expect("stepped");
        let taken: Snapshotted =
            (client.call_as("snapshot", json!({"path": asked}))).expect("a snapshot");
        (refused, taken)
    };
    let (refused, taken) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(refused, ["api.bad_params", "api.not_ticked"]);
    assert_eq!((&taken.path, taken.ticks, taken.entities), (&path, 1, 3));
    assert_eq!(taken.bytes, fs::metadata(&path).expect("written").len());
    let snapshot = crate::snapshot::Snapshot::read(&path, "Api Snapshots").expect("read");
    let mut restored = Context::seeded(6);
    let ticks = snapshot
        .restore(restored.world_mut(), &[])
        .expect("restored");
    assert_eq!(ticks, taken.ticks);
    assert_eq!(restored.world().hash(), cx.world().hash());
}

/// A `snapshot` with `quit` is written and answered before the frame ends the run; what follows it
/// in that frame is refused, applying nothing, so a restart loses no answered edit. It takes the
/// pause again.
#[test]
fn a_snapshot_that_quits_is_written_first_and_nothing_after_it_applies() {
    let (mut api, session) = started("Api Snapshot Quits");
    let (passed, wakes) = mpsc::channel();
    api.set_waker(Box::new(move || {
        let _ = passed.send(());
    }));
    let mut cx = Context::seeded(21);
    let hero = cx.world_mut().spawn(Transform::IDENTITY);
    let hero = cx.world().id(hero).expect("alive").to_string();
    let mut stepper = Stepper::default();
    stepper.step(Tick::DT, |_| true, || Duration::ZERO);
    stepper.pause();
    let dir = Scratch::new("api-snapshot-quits");
    fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("world.snapshot");
    let (mut stream, mut answers) = said_hello(&session);
    let set = json!({"set": {"id": hero, "component": "Transform", "path": "translation.x",
                             "value": 5.0}});
    for request in [
        json!({"id": 1, "method": "snapshot", "params": {"path": path, "quit": true}}),
        json!({"id": 2, "method": "apply", "params": {"operations": [set]}}),
    ] {
        stream
            .write_all(format!("{request}\n").as_bytes())
            .expect("written");
        wakes.recv_timeout(DEADLINE).expect("passed on");
    }
    let hash = cx.world().hash();
    let quits = api.serve(&mut cx, &mut Stack::default(), &mut stepper);
    let [taken, refused] = [1, 2].map(|_| {
        let line = gg_api::read_line(&mut answers).expect("an answer");
        serde_json::from_str::<Value>(&line.expect("a line")).expect("JSON")
    });
    assert!(quits, "the run ends");
    assert_eq!(taken["result"]["path"], json!(path));
    assert_eq!(refused["error"]["code"], "api.quitting");
    assert_eq!(cx.world().hash(), hash, "nothing applied after it");
    let snapshot = crate::snapshot::Snapshot::read(&path, "Api Snapshot Quits");
    let snapshot = snapshot.expect("written before the run ended");
    assert!(snapshot.paused);
    let (mut restored, mut again) = (Stepper::default(), Context::new(None));
    let mut stack = Stack::default();
    crate::main_loop::start(
        &mut stack,
        &mut again,
        &mut restored,
        Some(snapshot),
        false,
        false,
        true,
    )
    .expect("restored");
    assert!(restored.paused() && !restored.stopped());
    assert_eq!(restored.ticks, 1);
    // Restored paused, it runs no tick yet is snapshotted, as it attached nothing its first tick
    // would apply.
    drop(api);
    let (mut api, session) = started("Api Snapshot Quits Again");
    let again_path = dir.0.join("again.snapshot");
    let asked = again_path.clone();
    let taken = serving(
        &mut api,
        &mut again,
        &mut stack,
        &mut restored,
        session,
        move |mut client| client.call_as::<Snapshotted>("snapshot", json!({"path": asked})),
        |_, _, _| {},
    );
    let taken = taken.expect("a snapshot of the run restored paused");
    assert_eq!((&taken.path, taken.ticks), (&again_path, 1));
    assert_eq!(restored.ran(), 0, "no tick ran");
}

/// The run's first frame, before any layer's update, answers nothing, though `ping` counts it. A
/// request that came as the run started is answered as the second starts, with what the first
/// update made: a restarted game's HUD as its restored world has it, not as its file does.
#[test]
fn the_runs_first_frame_answers_nothing_and_the_second_reads_its_update() {
    let (api, session) = started("Api First Frame");
    let mut api = Some(api);
    let (passed, wakes) = mpsc::channel();
    crate::api::set_waker(&mut api, move || {
        let _ = passed.send(());
    });
    let (mut cx, mut stack, mut stepper) =
        (Context::seeded(22), Stack::default(), Stepper::default());
    let (mut stream, mut answers) = said_hello(&session);
    let mut ask = |request: Value| {
        let line = format!("{request}\n");
        stream.write_all(line.as_bytes()).expect("written");
    };
    ask(json!({"id": 1, "method": "status"}));
    wakes.recv_timeout(DEADLINE).expect("passed on");
    assert!(!crate::api::serve(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper
    ));
    ask(json!({"id": 2, "method": "ping"}));
    let mut answer = || {
        let line = gg_api::read_line(&mut answers).expect("an answer");
        serde_json::from_str::<Value>(&line.expect("a line")).expect("JSON")
    };
    let pulse = answer();
    assert_eq!(
        (&pulse["id"], &pulse["result"]["frames"]),
        (&json!(2), &json!(1))
    );
    // As the first frame's update would.
    cx.world_mut().spawn(Transform::IDENTITY);
    assert!(!crate::api::serve(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper
    ));
    let status = answer();
    assert_eq!(status["id"], 1, "the status asked before the first frame");
    assert_eq!(status["result"]["entities"], 1, "{status}");
    assert_eq!(status["result"]["frames"], 2);
}

/// What [`stop_brings_back_the_world_as_edited_that_play_copied`]'s client gets.
struct Stopped {
    stopped: Status,
    still: Status,
    snapshot: Snapshotted,
    refused: String,
    edited: Hash,
    played: Status,
    copied: Snapshotted,
    ticking: Status,
    failed: String,
    back: Status,
    hash: Hash,
    history: History,
    undone: Option<gg_api::Edit>,
}

/// `stop` halts ticks (paused or not), refuses `step` and allows a snapshot. `play` runs a copy of
/// the world as edited, which a snapshot may take before its first tick and a client's edits pass
/// through to; the next `stop` fails waiting
/// `step`s, leaves the game paused and restores the world as edited (same ticks, with both edits,
/// which `undo` takes back). Every answer says whether the game is stopped.
#[test]
fn stop_brings_back_the_world_as_edited_that_play_copied() {
    let (mut api, session) = started("Api Play And Stop");
    let mut cx = Context::seeded(8);
    let hero = cx.world_mut().spawn(Transform::IDENTITY);
    let hero_id = cx.world().id(hero).expect("alive").to_string();
    // Paused, so no tick runs before the client asks.
    let mut stepper = Stepper::default();
    stepper.pause();
    let dir = Scratch::new("api-stop");
    fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("stopped.snapshot");
    let (asked, other) = (path.clone(), session.clone());
    let copy = dir.0.join("copy.snapshot");
    let client = move |mut client: Client| {
        let set = |x: i32| {
            let path = "translation.x";
            let set = json!({"id": hero_id, "component": "Transform", "path": path, "value": x});
            json!({"operations": [{ "set": set }]})
        };
        let status = |client: &mut Client| client.call_as::<Status>("status", ());
        let stopped: Status = client.call_as("stop", ()).expect("stopped");
        // Stopped, it runs no ticks, paused or not.
        client.call_as::<Status>("resume", ()).expect("resumed");
        let still = loop {
            let status = status(&mut client).expect("a status");
            if status.frames > stopped.frames + 2 {
                break status;
            }
        };
        let snapshot = (client.call_as("snapshot", json!({ "path": asked })))
            .expect("a snapshot before any tick");
        let step = client.call("step", json!({"ticks": 1}));
        let refused = step.expect_err("refused").code;
        client.call_as::<Applied>("apply", set(1)).expect("set");
        let edited = client.call_as("hash", ()).expect("a hash");
        // Played paused: a copy of a world whose layers' queues `stop` emptied.
        client.call_as::<Status>("pause", ()).expect("paused");
        let played: Status = client.call_as("play", ()).expect("played");
        let copied = (client.call_as("snapshot", json!({ "path": copy })))
            .expect("a snapshot before the copy's first tick");
        client.call_as::<Status>("resume", ()).expect("resumed");
        let ticking = loop {
            let status = status(&mut client).expect("a status");
            if status.ticks > played.ticks {
                break status;
            }
        };
        client
            .call_as::<Applied>("apply", set(2))
            .expect("set in the copy");
        // Another client's step, waiting for its ticks, which pauses the game.
        let stepping = thread::spawn(move || {
            let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
            let step = them.call("step", json!({"ticks": MOST_STEPS}));
            step.expect_err("failed").code
        });
        while !status(&mut client).expect("a status").paused {}
        let back: Status = client.call_as("stop", ()).expect("stopped");
        Stopped {
            stopped,
            still,
            snapshot,
            refused,
            edited,
            played,
            copied,
            ticking,
            failed: stepping.join().expect("the step didn't panic"),
            back,
            hash: client.call_as("hash", ()).expect("a hash"),
            history: client.call_as("edits", ()).expect("the edits"),
            undone: client.call_as("undo", ()).expect("undone"),
        }
    };
    let got = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert!(got.stopped.stopped && got.still.stopped && !got.still.paused);
    assert_eq!((got.stopped.ticks, got.still.ticks), (0, 0));
    assert_eq!(
        (got.snapshot.ticks, got.refused.as_str()),
        (0, "api.stopped")
    );
    let snapshot = crate::snapshot::Snapshot::read(&path, "Api Play And Stop");
    assert!(snapshot.expect("read").stopped, "it says it's stopped");
    assert!(!got.played.stopped && !got.ticking.stopped);
    assert_eq!(got.copied.ticks, got.played.ticks);
    assert_eq!(got.failed, "api.stopped");
    assert!(got.back.stopped && got.back.paused);
    assert_eq!(got.back.ticks, got.played.ticks, "the world as edited's");
    assert_ne!(got.hash, got.edited, "the copy's edit passed through");
    let [authored] = &got.history.authors[..] else {
        panic!("one author: {:?}", got.history);
    };
    assert_eq!(
        authored.done.len(),
        2,
        "the edit before `play`, and the copy's"
    );
    assert!(got.undone.is_some());
    let x = cx.world().get::<Transform>(hero).map(|at| at.translation.x);
    assert_eq!(x, Some(1.0), "the copy's edit undone");
    assert!(cx.dev.edited.is_none());
}

/// A client saves the open scene (the watch's next look applies nothing), saves it elsewhere, opens
/// one in its place (unsaved edits discarded once refused, named textures loaded) and empties it.
/// Each answers with the status: open scene, unsaved or not. What can't be done gets a code.
#[test]
fn a_client_saves_opens_and_empties_the_open_scene() {
    let logged = crate::testing::logged();
    let (mut api, session) = started("Api Scenes");
    let dir = Scratch::new("api-scenes");
    let (_, ids) = scenes::project(&dir.0, 1);
    let scenes_dir = dir.0.join("scenes");
    fs::create_dir_all(dir.0.join("assets")).expect("a directory");
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    fs::write(dir.0.join("assets/stone.png"), png).expect("a PNG");
    // A sprite of a texture no layer adds; opening it loads it.
    let level2 =
        "(entities: {\"00000000000a5001\": (Sprite: (texture: Some(\"assets/stone.png\")))})\n";
    scenes::save(&scenes_dir.join("level2.ron"), level2, 0);
    fs::write(dir.0.join("outside.ron"), "(entities: {})\n").expect("written");
    let mut cx = scenes::started(&dir.0);
    let mut stack = Stack::default();
    let mut stepper = Stepper::default();
    stepper.pause();
    let scene = |client: &mut Client, params: Value| client.call_as::<Status>("scene", params);
    let create = json!({"operations": [{"create": {"components": {"Transform": {}}}}]});
    let client = move |mut client: Client| {
        let loaded = client.call_as("status", ()).expect("a status");
        client.call_as::<Applied>("apply", create).expect("created");
        let edited = client.call_as("status", ()).expect("a status");
        let saved = scene(&mut client, json!({"action": "save"})).expect("saved");
        (loaded, edited, saved)
    };
    let (loaded, edited, saved): (Status, Status, Status) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session.clone(),
        client,
        |_, _, _| {},
    );
    scenes::look_now(&mut cx, &mut stack);
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(
        !codes.iter().any(|code| code == "scene.applied"),
        "{codes:?}"
    );
    let logs = cx
        .authors
        .logs(cx.world())
        .map(|(author, _)| author.to_owned());
    assert_eq!(logs.collect::<Vec<_>>(), ["test"], "no edit of the file's");
    let id = ids[0].to_string();
    let set = json!({"operations": [{"set": {"id": id, "component": "Transform",
        "path": "translation.x", "value": 2}}]});
    let client = move |mut client: Client| {
        let saved_as = (scene(
            &mut client,
            json!({"action": "save", "path": "scenes/copy.ron"}),
        ))
        .expect("saved as");
        let open = json!({"action": "open", "path": "scenes/level2.ron"});
        client.call_as::<Applied>("apply", set).expect("set");
        let unsaved = scene(&mut client, open.clone()).expect_err("unsaved").code;
        let mut discarded = open;
        discarded["discard"] = json!(true);
        let opened = scene(&mut client, discarded).expect("opened");
        let emptied = scene(&mut client, json!({"action": "new"})).expect("emptied");
        let refused = [
            json!({"action": "save"}),
            json!({"action": "open", "path": "outside.ron"}),
            json!({"action": "open", "path": "scenes/none.ron"}),
        ];
        let refused = refused.map(|params| scene(&mut client, params).expect_err("refused").code);
        (saved_as, unsaved, opened, emptied, refused.to_vec())
    };
    let (saved_as, unsaved, opened, emptied, refused) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let named = |status: &Status| (status.scene.clone(), status.unsaved);
    let title = |name: &str| Some(name.to_owned());
    assert_eq!(named(&loaded), (title("scenes/main.ron"), false));
    assert_eq!(named(&edited), (title("scenes/main.ron"), true));
    assert_eq!(named(&saved), (title("scenes/main.ron"), false));
    assert_eq!(named(&saved_as), (title("scenes/copy.ron"), false));
    assert_eq!(unsaved, "scene.unsaved");
    assert_eq!(named(&opened), (title("scenes/level2.ron"), false));
    assert!(cx.world().assets().texture("assets/stone.png").is_some());
    assert_eq!((named(&emptied), emptied.entities), ((None, false), 0));
    assert_eq!(refused, ["scene.untitled", "scene.bad_path", "scene.file"]);
    assert!(scenes_dir.join("copy.ron").is_file());
}

/// `scene`'s `new` while a copy plays stops it, empties the scene and plays a copy of that, the
/// run's ticks going on.
#[test]
fn a_scene_emptied_while_a_copy_plays_keeps_the_runs_ticks() {
    let (mut api, session) = started("Api Scene Playing");
    let dir = Scratch::new("api-scene-playing");
    scenes::project(&dir.0, 1);
    let mut cx = scenes::started(&dir.0);
    let mut stack = Stack::default();
    stack.play(&mut cx, 0);
    let client = move |mut client: Client| {
        let ticked = loop {
            let status: Status = client.call_as("status", ()).expect("a status");
            if status.ticks > 0 {
                break status;
            }
        };
        let new = json!({"action": "new", "discard": true});
        let emptied: Status = client.call_as("scene", new).expect("emptied");
        (ticked, emptied)
    };
    let (ticked, emptied) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut Stepper::default(),
        session,
        client,
        |_, _, _| {},
    );
    assert!(
        emptied.ticks >= ticked.ticks,
        "{emptied:?} after {ticked:?}"
    );
    assert!(!emptied.stopped && emptied.entities == 0);
    assert!(cx.dev.edited.is_some(), "a copy of the empty scene plays");
}

/// `cells` reads a world's cells: those whose files the folder holds, those in, their streamed
/// roots, and the viewers (the look's viewpoint among them while stopped). A world without `Cells`
/// has none.
#[test]
fn a_client_reads_the_worlds_cells() {
    let (mut api, session) = started("Api Cells");
    let dir = Scratch::new("api-cells");
    let root = crate::cells::tests::project(&dir.0);
    let mut cx = Context::seeded(1).root(root);
    let mut stack = Stack::default();
    let mut stepper = Stepper::default();
    stepper.stop();
    let read = |client: &mut Client| {
        let list = client.call_as::<gg_api::CellList>("cells", ());
        list.expect("the cells")
    };
    let none = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session.clone(),
        move |mut client| read(&mut client),
        |_, _, _| {},
    );
    assert_eq!(none, gg_api::CellList::default());
    let world = cx.world_mut();
    let cells = world.spawn(crate::Cells {
        folder: "cells".to_owned(),
        size: 10.0,
    });
    let at = Transform {
        translation: DVec3::new(5.0, 0.0, 0.0),
        ..Transform::IDENTITY
    };
    let viewer = world.spawn((at, crate::Viewer { radius: 10.0 }));
    let ids = [cells, viewer].map(|entity| world.id(entity).expect("alive").to_string());
    let viewpoint = json!({"position": [35, 0, 0], "rotation": [0, 0, 0, 1],
        "projection": {"Orthographic": {"height": 2, "near": -1, "far": 1}}});
    let client = move |mut client: Client| {
        let cells_in = |list: &gg_api::CellList| {
            let world = list.world.as_ref().expect("a world of cells");
            world.cells.iter().filter(|cell| cell.is_in).count()
        };
        let near = loop {
            let list = read(&mut client);
            if cells_in(&list) == 2 {
                break list;
            }
        };
        let look = json!({"viewpoint": viewpoint});
        client.call_as::<Value>("look", look).expect("looked");
        let all = loop {
            let list = read(&mut client);
            if cells_in(&list) == 4 {
                break list;
            }
        };
        (near, all)
    };
    let (near, all) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session,
        client,
        |_, cx, _| cx.stream_stopped(),
    );
    let near = near.world.expect("a world of cells");
    assert_eq!(
        (near.entity.as_str(), near.folder.as_str(), near.size),
        (ids[0].as_str(), "cells", 10.0)
    );
    let viewer = gg_api::ViewerAt {
        entity: Some(ids[1].clone()),
        position: [5.0, 0.0, 0.0],
        radius: 10.0,
    };
    assert_eq!(near.viewers, std::slice::from_ref(&viewer));
    let cell = |x: i32, is_in: bool| gg_api::CellAt {
        cell: [x, 0, 0],
        file: true,
        made: false,
        is_in,
        coming: false,
        refused: false,
        kept: 0,
        roots: usize::from(is_in),
        unsaved: false,
    };
    let expected = [cell(0, true), cell(1, true), cell(2, false), cell(3, false)];
    assert_eq!(near.cells, expected);
    let all = all.world.expect("a world of cells");
    let looked = gg_api::ViewerAt {
        entity: None,
        position: [35.0, 0.0, 0.0],
        radius: 512.0,
    };
    assert_eq!(all.viewers, [viewer, looked]);
}

/// `scene` is refused with `api.no_scene` where it can't work: `open` and `new` in a recorded run
/// (they save), everything in a lockstep peer; with `engine.no_project` outside a project's game.
#[test]
fn scenes_are_refused_where_the_run_cant_have_them() {
    let (mut api, session) = started("Api No Scenes");
    let dir = Scratch::new("api-no-scenes");
    scenes::project(&dir.0, 1);
    let mut stepper = Stepper::default();
    stepper.pause();
    let asked = [
        json!({"action": "new", "discard": true}),
        json!({"action": "open", "path": "scenes/main.ron", "discard": true}),
        json!({"action": "save"}),
    ];
    let ask = move |mut client: Client| {
        let asked = asked.map(|params| client.call("scene", params).err().map(|f| f.code));
        asked.to_vec()
    };
    let mut recorded = Stack::default();
    let recorder = Recorder::create(&dir.0.join("run.recording"), "Tested");
    recorded.record(recorder.expect("created"));
    let mut peer = Stack::default();
    let net = Net::Host(crate::args::Listen::at("127.0.0.1:0"));
    let code = crate::net::TESTED;
    peer.lockstep(Lockstep::start(&net, code, "Tested", crate::net::BUILD, &[]).expect("hosting"));
    let no_scene = Some("api.no_scene".to_owned());
    let no_project = Some("engine.no_project".to_owned());
    for (stack, project, codes) in [
        (
            &mut recorded,
            true,
            [no_scene.clone(), no_scene.clone(), None],
        ),
        (
            &mut peer,
            true,
            [no_scene.clone(), no_scene.clone(), no_scene],
        ),
        (
            &mut Stack::default(),
            false,
            [no_project.clone(), no_project.clone(), no_project],
        ),
    ] {
        let mut cx = if project {
            scenes::started(&dir.0)
        } else {
            Context::seeded(4)
        };
        let got = serving(
            &mut api,
            &mut cx,
            stack,
            &mut stepper,
            session.clone(),
            ask.clone(),
            |_, _, _| {},
        );
        assert_eq!(got, codes, "project: {project}");
    }
}

/// What [`a_client_saves_an_entity_as_a_prefab_linked_in_its_place`]'s client gets.
struct Prefabbed {
    saved: SavedPrefab,
    root: Value,
    bulb: Value,
    renamed: Value,
    reverted: Value,
    refused: Vec<String>,
    again: SavedPrefab,
}

/// A client saves an entity's subtree as a prefab, written with its ID file in `prefabs` and named
/// after the entity. The entity links to it under the subtree's IDs, as an edit `undo` unlinks.
/// `entity` answers how an instance differs from its prefab; `revert` sets it back. Refused with a
/// code: an instance's entity or root, an existing file, a path outside the root or not a prefab's.
/// A taken name is numbered.
#[test]
fn a_client_saves_an_entity_as_a_prefab_linked_in_its_place() {
    let (mut api, session) = started("Api Prefabs");
    let dir = Scratch::new("api-prefabs");
    scenes::project(&dir.0, 0);
    let transform = "Transform: (translation: (2.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, \
                         1.0), scale: (1.0, 1.0, 1.0))";
    let main = format!(
        "(entities: {{\"0000000000000a01\": (Name: (\"Big Lamp\"), {transform}), \
             \"0000000000000a02\": (Name: (\"bulb\"), Parent: (\"0000000000000a01\"))}})\n"
    );
    scenes::save(&dir.0.join("scenes").join("main.ron"), &main, 0);
    let mut cx = scenes::started(&dir.0);
    let mut stack = Stack::default();
    let mut stepper = Stepper::default();
    stepper.pause();
    let (root, bulb) = ("0000000000000a01", "0000000000000a02");
    let client = move |mut client: Client| {
        let entity = |client: &mut Client, id: &str| -> Value {
            let answer = client.call("entity", json!({"id": id})).expect("an entity");
            serde_json::from_str(answer.get()).expect("JSON")
        };
        let save = |client: &mut Client, params: Value| {
            client.call_as::<SavedPrefab>("save_prefab", params)
        };
        let saved = save(&mut client, json!({"id": root})).expect("saved");
        let (root_read, bulb_read) = (entity(&mut client, root), entity(&mut client, bulb));
        let set = json!({"set": {"id": bulb, "component": "Name", "path": "0",
            "value": "globe"}});
        let apply = |client: &mut Client, operation: Value| {
            let operations = json!({"operations": [operation]});
            client
                .call_as::<Applied>("apply", operations)
                .expect("applied");
        };
        apply(&mut client, set);
        let renamed = entity(&mut client, bulb);
        apply(
            &mut client,
            json!({"revert": {"id": bulb, "component": "Name"}}),
        );
        let reverted = entity(&mut client, bulb);
        let mut refused = vec![
            save(&mut client, json!({"id": root})),
            save(&mut client, json!({"id": bulb})),
        ];
        for _ in 0..3 {
            client.call("undo", Value::Null).expect("undone");
        }
        for path in [
            "prefabs/big_lamp.prefab",
            "../outside.prefab",
            "prefabs/x.ron",
        ] {
            refused.push(save(&mut client, json!({"id": root, "path": path})));
        }
        let refused = refused
            .into_iter()
            .map(|refused| refused.expect_err("refused").code);
        let again = save(&mut client, json!({"id": root})).expect("saved again");
        Prefabbed {
            saved,
            root: root_read,
            bulb: bulb_read,
            renamed,
            reverted,
            refused: refused.collect(),
            again,
        }
    };
    let got = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(got.saved.path, "prefabs/big_lamp.prefab");
    let file = dir.0.join("prefabs").join("big_lamp.prefab");
    let meta = gg_api::asset::Meta::read(&gg_api::asset::Meta::path(&file)).expect("its ID");
    assert_eq!(got.saved.name, meta.id.to_string(), "named by its ID");
    let instance = |read: &Value| -> gg_api::Instanced {
        serde_json::from_value(read["instance"].clone()).expect("an instance's")
    };
    let linked = instance(&got.root);
    assert_eq!(
        (&*linked.root, linked.link.as_deref()),
        (root, Some("prefabs/big_lamp.prefab"))
    );
    assert!(
        linked.overrides.is_empty() && linked.added.is_empty(),
        "{linked:?}"
    );
    let given = instance(&got.bulb);
    assert_eq!(
        (&*given.root, &*given.prefab),
        (root, "prefabs/big_lamp.prefab")
    );
    assert_eq!(given.link, None);
    let renamed = instance(&got.renamed).overrides;
    assert_eq!(
        renamed.get("Name").map(Vec::as_slice),
        Some(&[String::new()][..])
    );
    assert!(instance(&got.reverted).overrides.is_empty());
    let expected = [
        "scene.prefab_variant",
        "ecs.prefab_entity",
        "asset.exists",
        "asset.outside_root",
        "api.bad_params",
    ];
    assert_eq!(got.refused, expected);
    assert_eq!(got.again.path, "prefabs/big_lamp-2.prefab");
    // Linked again under the subtree's IDs; the prefab's root at its origin.
    let world = cx.world();
    let [root, bulb] = [root, bulb].map(|id| world.entity(id.parse().unwrap()).unwrap());
    assert!(world.get::<PrefabRoot>(root).is_some());
    assert_eq!(world.parent(bulb).expect("alive"), Some(root));
    let text = fs::read_to_string(&file).expect("written");
    assert!(text.contains("translation: (0.0, 0.0, 0.0)"), "{text}");
    assert!(text.contains("Name: (\"bulb\")"), "{text}");
}

/// A client lists the assets under the root: a page of a kind or all, each with its ID (written
/// where missing, as it scans first) and whether a layer added it; plus ID files whose assets are
/// missing. Refused with a code without a root or where it's missing.
#[test]
fn a_client_lists_the_assets_under_the_root_with_their_ids() {
    use gg_api::asset::{AssetId, Meta};

    let (mut api, session) = started("Api Assets");
    let dir = Scratch::new("api-assets");
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.0.join(name);
        fs::create_dir_all(path.parent().expect("a folder")).expect("a directory");
        fs::write(path, bytes).expect("written");
    };
    let meta = |id: &str| Meta {
        id: id.parse().expect("an ID"),
    };
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    write("assets/a.png", &png);
    write("assets/b.png", &png);
    write(
        "assets/b.png.meta",
        meta("5f3c9a1e07b2d846").text().as_bytes(),
    );
    write("models/m.glb", b"");
    write("scenes/x.ron", b"(entities: {})\n");
    write(
        "assets/gone.png.meta",
        meta("0a1b2c3d4e5f6071").text().as_bytes(),
    );
    let mut cx = Context::seeded(2).root(dir.0.clone());
    cx.add_texture("b", dir.0.join("assets/b.png"))
        .expect("a texture");
    let mut stepper = Stepper::default();
    let ask = |mut client: Client| {
        let mut list = |params: Value| {
            let list = client.call_as::<AssetList>("assets", params);
            list.expect("a list")
        };
        [
            list(Value::Null),
            list(json!({"kind": "model"})),
            list(json!({"skip": 1, "take": 1})),
            list(json!({})),
        ]
    };
    let [all, models, page, again] = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session.clone(),
        ask,
        |_, _, _| {},
    );
    let paths = |list: &AssetList| -> Vec<String> {
        list.assets.iter().map(|asset| asset.path.clone()).collect()
    };
    assert_eq!(all.total, 3);
    assert_eq!(
        paths(&all),
        ["assets/a.png", "assets/b.png", "models/m.glb"]
    );
    let a = all.assets[0].id.as_deref().expect("written");
    let a: AssetId = a.parse().expect("16 hex digits");
    let written = Meta::read(&dir.0.join("assets/a.png.meta")).expect("its ID file");
    assert_eq!(written.id, a);
    assert_eq!(all.assets[1].id.as_deref(), Some("5f3c9a1e07b2d846"));
    let loaded: Vec<bool> = all.assets.iter().map(|asset| asset.loaded).collect();
    assert_eq!(loaded, [false, true, false]);
    assert!(
        all.assets.iter().all(|asset| asset.problem.is_none()),
        "{all:?}"
    );
    let gone = ListedOrphan {
        path: "assets/gone.png.meta".to_owned(),
        kind: AssetKind::Texture,
        id: Some("0a1b2c3d4e5f6071".to_owned()),
    };
    assert_eq!(all.orphans, [gone]);
    assert_eq!(
        (models.total, paths(&models)),
        (1, vec!["models/m.glb".to_owned()])
    );
    assert_eq!(models.assets[0].kind, AssetKind::Model);
    assert!(models.orphans.is_empty(), "the orphan's asset is a texture");
    assert_eq!(
        (page.total, paths(&page)),
        (3, vec!["assets/b.png".to_owned()])
    );
    assert_eq!(again, all, "the same IDs");

    // With a thread watching the root, each answers once it has looked since it came: a renamed
    // asset is listed at its new path with its ID, still loaded; a new one has its ID written.
    cx.start_assets(true, true);
    assert!(cx.watches_assets());
    let [b, c, d] = ["b", "c", "d"].map(|name| dir.0.join(format!("assets/{name}.png")));
    let other =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[7; 4], None);
    let ask = move |mut client: Client| {
        let mut list = || client.call_as::<AssetList>("assets", json!({}));
        let first = list().expect("a list");
        fs::rename(&b, &c).expect("renamed");
        fs::write(&d, other).expect("written");
        (first, list().expect("a list"))
    };
    let (first, after) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session.clone(),
        ask,
        |_, cx, _| cx.take_assets(),
    );
    assert_eq!(first.assets, all.assets);
    let expected = [
        "assets/a.png",
        "assets/c.png",
        "assets/d.png",
        "models/m.glb",
    ];
    assert_eq!(paths(&after), expected);
    let c = &after.assets[1];
    assert_eq!(
        (c.id.as_deref(), c.loaded),
        (Some("5f3c9a1e07b2d846"), true)
    );
    let written = Meta::read(&dir.0.join("assets/d.png.meta")).expect("its ID file");
    assert_eq!(after.assets[2].id, Some(written.id.to_string()));
    assert_eq!(after.orphans, all.orphans, "b.png's ID file moved");

    let refusals = [
        (Context::seeded(3), "engine.no_root"),
        (
            Context::seeded(3).root(dir.0.join("none")),
            "asset.unlisted",
        ),
    ];
    for (mut cx, code) in refusals {
        let refused = serving(
            &mut api,
            &mut cx,
            &mut Stack::default(),
            &mut stepper,
            session.clone(),
            |mut client: Client| client.call("assets", Value::Null).expect_err("refused"),
            |_, _, _| {},
        );
        assert_eq!(refused.code, code, "{}", refused.message);
    }
}

/// An `assets` waiting for a look at the root is answered as frames pause, from the database as is.
#[test]
fn an_assets_waiting_for_a_look_is_answered_as_frames_pause() {
    let (mut api, _) = started("Api Paused Assets");
    let dir = Scratch::new("api-paused-assets");
    fs::create_dir_all(&dir.0).expect("a directory");
    fs::write(dir.0.join("a.png"), "").expect("an asset");
    let cx = Context::seeded(2).root(dir.0.clone());
    let (to, answers) = mpsc::channel();
    api.listing.push(Listing {
        reply: reply(7, &to),
        look: u64::MAX,
        kind: None,
        skip: 0,
        take: 10,
    });
    api.frames_paused(&cx);
    let (answer, _) = answers.try_recv().expect("answered");
    assert!(answer.contains("\"a.png\""), "{answer}");
    assert!(api.listing.is_empty());
}

/// A patch that can't apply is answered with `api.patch`: here, a table reference past any `main`,
/// refused by a test's build either way, as `patch`'s tests say. A recording holds no patch that
/// didn't apply.
#[test]
fn a_patch_that_cant_apply_is_refused() {
    let (mut api, session) = started("Api Patches");
    let mut stepper = Stepper::default();
    let dir = Scratch::new("api-patch-recorded");
    std::fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("run.recording");
    let mut recorded = Stack::default();
    recorded.record(Recorder::create(&path, "Tested").expect("created"));
    let client = |mut client: Client| {
        let table = json!({
            "lib": std::env::temp_dir().join("patch.dll"),
            "map": {},
            "aslr_reference": u64::MAX,
            "new_base_address": 0,
            "ifunc_count": 0,
        });
        let call = client.call("patch", json!({ "table": table }));
        call.expect_err("refused")
    };
    for stack in [&mut Stack::default(), &mut recorded] {
        let refused = serving(
            &mut api,
            &mut Context::seeded(7),
            stack,
            &mut stepper,
            session.clone(),
            client,
            |_, _, _| {},
        );
        assert_eq!(refused.code, "api.patch", "{}", refused.message);
    }
    let text = std::fs::read_to_string(&path).expect("the recording");
    assert!(
        !text.lines().any(|line| line.starts_with("patch ")),
        "{text}"
    );
}

/// A capture rides with handed-off frames until one that rendered brings the view back: an error,
/// or the view, written as a PNG.
#[test]
fn a_capture_rides_with_frames_until_one_brings_the_view_back() {
    let (mut api, session) = started("Api Captures");
    let mut cx = Context::seeded(4);
    let mut stepper = Stepper::default();
    let dir = Scratch::new("api-capture");
    fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("view.png");
    let asked = path.clone();
    let client = move |mut client: Client| {
        let refused = client.call("capture", Value::Null).expect_err("no view");
        let captured: Captured =
            (client.call_as("capture", json!({"path": asked}))).expect("a capture");
        (refused.code, captured)
    };
    let mut frame = Extracted::default();
    let mut rides = 0;
    let (refused, captured) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |api, cx, stepper| {
            // The frame that rode with a capture comes back; the first two rode with each had
            // nothing to render into.
            if frame.capture {
                rides += 1;
                frame.captured = match rides {
                    3 => Some(Err(Error::new("api.no_view", "the view has no area"))),
                    6 => Some(Ok(Shot {
                        size: [2, 1],
                        pixels: vec![255, 0, 0, 255, 0, 128, 255, 9],
                    })),
                    _ => None,
                };
            }
            api.frame_back(&mut frame, cx, stepper.ticks);
        },
    );
    assert_eq!(refused, "api.no_view");
    assert_eq!(captured.path, Some(path.clone()));
    assert_eq!(captured.pixel, None, "none was asked for");
    assert_eq!((captured.width, captured.height), (2, 1));
    let file = fs::File::open(&path).expect("the PNG");
    let mut reader = (png::Decoder::new(std::io::BufReader::new(file)).read_info()).expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("its pixels");
    assert_eq!(info.color_type, png::ColorType::Rgb);
    let rgb = &pixels[..info.buffer_size()];
    assert_eq!(rgb, [255, 0, 0, 0, 128, 255], "alpha goes");
    assert!(!frame.capture, "nothing rides once both are answered");
}

/// The RGB bytes of the PNG at `path`, and its size.
fn png_rgb(path: &Path) -> (Vec<u8>, [u32; 2]) {
    let file = fs::File::open(path).expect("the PNG");
    let mut reader = (png::Decoder::new(std::io::BufReader::new(file)).read_info()).expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("its pixels");
    pixels.truncate(info.buffer_size());
    (pixels, [info.width, info.height])
}

/// A viewpoint at `z`, looking down -Z, as the tool API gives one.
fn at_z(z: f32) -> gg_api::Viewpoint {
    gg_api::Viewpoint {
        position: [0.0, 0.0, f64::from(z)],
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
    }
}

/// A film: refused as its docs say. Frames draw from the first viewpoint while what's on its way
/// delays them, then are read back in turn, none skipped, from the next viewpoint (the last held),
/// cropped to the rect and written as numbered PNGs. The answer waits for all; the look's viewpoint
/// draws again after.
#[test]
fn frames_are_captured_in_a_row_from_their_viewpoints() {
    let (mut api, session) = started("Api Frames");
    let mut cx = Context::seeded(4);
    let mut stepper = Stepper::default();
    let dir = Scratch::new("api-frames");
    fs::create_dir_all(&dir.0).expect("a directory");
    let asked = dir.0.clone();
    let client = move |mut client: Client| {
        let look = json!({"viewpoint": at_z(9.0)});
        client.call("look", look).expect("a look");
        let refused: Vec<String> = [
            json!({"dir": "frames", "count": 3}),
            json!({"dir": asked.join("none"), "count": 3}),
            json!({"dir": asked, "count": 0}),
            json!({"dir": asked, "count": gg_api::MOST_FRAMES + 1}),
            json!({"dir": asked, "count": 3, "rect": [0, 0, 0, 2]}),
            json!({"dir": asked, "count": 1, "rect": [0, 0, 40_000, 40_000]}),
            json!({"dir": asked, "count": 3, "viewpoints": [
                {"position": [0, 0, 1], "rotation": [0, 0, 0, 0],
                "projection": {"Perspective": {"fov": 1.0, "near": 0.1}}}]}),
        ]
        .into_iter()
        .map(|params| client.call("frames", params).expect_err("refused").code)
        .collect();
        let params = json!({"dir": asked, "count": 3, "rect": [1, 0, 2, 2],
            "viewpoints": [at_z(1.0), at_z(2.0)]});
        let framed: Framed = client.call_as("frames", params).expect("the frames");
        (refused, framed)
    };
    let mut frame = Extracted::default();
    // Frames handed off for the film while it waited, and how many rendered.
    let (mut waiting, mut rendered) = (Vec::new(), 0u8);
    let mut drawn_from = Viewpoint::default();
    let (refused, framed) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |api, cx, stepper| {
            // The last frame comes back, drawn from the look's viewpoint as made; each pixel is its
            // place and the frame's number.
            frame.viewpoint = drawn_from;
            if frame.capture {
                rendered += 1;
                let pixels = (0..2u8).flat_map(|y| (0..4u8).map(move |x| (x, y)));
                let pixels = pixels.flat_map(|(x, y)| [x, y, rendered, 255]).collect();
                frame.captured = Some(Ok(Shot {
                    size: [4, 2],
                    pixels,
                }));
            }
            // What a film's first two frames back showed was on its way.
            let film = api.films.first().filter(|film| !film.started);
            if film.is_some() {
                waiting.push(cx.dev.look.viewpoint);
            }
            frame.loading = u32::from(film.is_some() && waiting.len() <= 2);
            api.frame_back(&mut frame, cx, stepper.ticks);
            drawn_from = cx.dev.look.viewpoint.unwrap_or_default();
        },
    );
    assert_eq!(refused, ["api.bad_params"; 7]);
    let first = Some(engine_viewpoint(&at_z(1.0)));
    assert!(waiting.len() >= 3, "it waits out what's on its way");
    assert!(waiting.iter().all(|viewpoint| *viewpoint == first));
    let viewpoints: Vec<[f64; 3]> = (framed.frames.iter())
        .map(|shot| shot.viewpoint.position)
        .collect();
    assert_eq!(
        viewpoints,
        [[0.0, 0.0, 1.0], [0.0, 0.0, 2.0], [0.0, 0.0, 2.0]]
    );
    let mut numbers = Vec::new();
    for (at, shot) in framed.frames.iter().enumerate() {
        let path = dir.0.join(format!("frame-{:04}.png", at + 1));
        assert_eq!(shot.path, path);
        assert_eq!((shot.width, shot.height, shot.streaming), (2, 2, false));
        let (rgb, size) = png_rgb(&path);
        assert_eq!(size, [2, 2]);
        let number = rgb[2];
        assert_eq!(
            rgb,
            [1, 0, number, 2, 0, number, 1, 1, number, 2, 1, number]
        );
        numbers.push(number);
    }
    assert!(
        numbers.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{numbers:?}"
    );
    assert_eq!(cx.dev.look.viewpoint, Some(engine_viewpoint(&at_z(9.0))));
    assert!(!frame.capture, "nothing reads its view back after them");
}

/// A frame of a film that comes back without its view fails the film.
#[test]
fn frames_fail_at_a_frame_back_without_its_view() {
    let (mut api, session) = started("Api Frames Failing");
    let mut cx = Context::seeded(4);
    let mut stepper = Stepper::default();
    let dir = Scratch::new("api-frames-failing");
    fs::create_dir_all(&dir.0).expect("a directory");
    let asked = dir.0.clone();
    let client = move |mut client: Client| {
        let params = json!({"dir": asked, "count": 3});
        client
            .call("frames", params)
            .expect_err("a frame lost")
            .code
    };
    let mut frame = Extracted::default();
    let mut rendered = 0;
    let refused = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |api, cx, stepper| {
            if frame.capture {
                rendered += 1;
                frame.captured = (rendered != 2).then(|| {
                    Ok(Shot {
                        size: [1, 1],
                        pixels: vec![0; 4],
                    })
                });
            }
            api.frame_back(&mut frame, cx, stepper.ticks);
        },
    );
    assert_eq!(refused, "api.no_view");
    assert!(api.films.is_empty());
    assert!(!fs::exists(dir.0.join("frame-0001.png")).expect("a look"));
}

/// A client's input reaches the layers as the window's would, in order, with the cursor over the
/// view taken to window pixels. Each answer, and the status, says what the client's inputs hold
/// down; a press of what's down already is named, as it changes nothing.
#[test]
fn input_reaches_the_layers_as_the_windows_would() {
    let (mut api, session) = started("Api Input");
    let mut cx = Context::seeded(5);
    let view = Viewport {
        origin: [10, 20],
        size: [100, 50],
    };
    cx.extract(&mut Extracted::default(), view);
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let events = json!({"events": [
            {"key": {"key": "KeyW", "pressed": true}},
            {"key": {"key": "KeyA", "pressed": true}},
            {"key": {"key": "KeyA", "pressed": false}},
            {"key": {"key": "MetaLeft", "pressed": true}},
            {"button": {"button": "left", "pressed": true}},
            {"cursor": [3, 4.5]},
        ]});
        let first: gg_api::Handed = client.call_as("input", events).expect("the input");
        let again = json!({"events": [
            {"key": {"key": "KeyS", "pressed": true}},
            {"key": {"key": "KeyW", "pressed": true}},
            {"key": {"key": "KeyW", "pressed": true, "repeat": true}},
            {"key": {"key": "KeyS", "pressed": false}},
        ]});
        let again: gg_api::Handed = client.call_as("input", again).expect("the input");
        let status: Status = client.call_as("status", ()).expect("a status");
        (first, again, status.held)
    };
    let (first, again, held) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(first.held, ["KeyW", "MetaLeft", "mouse left"]);
    assert!(first.already_down.is_empty());
    assert_eq!(again.held, first.held);
    assert_eq!(again.already_down, ["KeyW"], "a repeat isn't a press");
    assert_eq!(held, again.held);
    let input = cx.input();
    assert!(input.key_down(Key::KeyW));
    assert!(!input.key_down(Key::KeyA), "released after its press");
    assert!(input.key_down(Key::SuperLeft), "by its W3C name");
    assert!(input.button_down(MouseButton::Left));
    assert_eq!(input.cursor(), Some([13.0, 24.5]));
}

/// A focus loss lets go of every key and button a client's input held, as the layers' input does,
/// so the next press is one.
#[test]
fn a_focus_loss_lets_go_of_what_input_held() {
    let (mut api, session) = started("Api Input Focus");
    let mut cx = Context::seeded(5);
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let mut input = |events: Value| -> gg_api::Handed {
            client
                .call_as("input", json!({"events": events}))
                .expect("the input")
        };
        let pressed = input(json!([{"key": {"key": "KeyW", "pressed": true}}]));
        let lost = input(json!([{"focused": false}]));
        let again = input(json!([{"key": {"key": "KeyW", "pressed": true}}]));
        (pressed, lost, again)
    };
    let (pressed, lost, again) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    assert_eq!(pressed.held, ["KeyW"]);
    assert_eq!(lost, gg_api::Handed::default());
    assert_eq!(again.held, ["KeyW"]);
    assert!(again.already_down.is_empty(), "a press again");
}

/// A held `input` lets go of what it pressed and left down after exactly its ticks, though each
/// frame owes three: the frame before the release runs fewer. Paused, the hold steps its ticks,
/// and a click's, leaving nothing down, only steps them. Each answers the status as it lets go,
/// with whether layers want the cursor grabbed.
#[test]
fn a_held_input_lets_go_after_exactly_its_ticks() {
    let (mut api, session) = started("Api Held Input");
    let mut cx = Context::seeded(5);
    cx.set_cursor_grabbed(true);
    let mut stack = Stack::default();
    let mut stepper = Stepper::default();
    let client = thread::spawn(move || {
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        let events = json!([
            {"key": {"key": "KeyW", "pressed": true}},
            {"key": {"key": "KeyA", "pressed": true}},
            {"key": {"key": "KeyA", "pressed": false}},
            {"button": {"button": "left", "pressed": true}},
        ]);
        let refused = (client.call("input", json!({"events": events, "hold": 0})))
            .expect_err("refused")
            .code;
        let held = json!({"events": events, "hold": 4});
        let running: Status = client.call_as("input", held).expect("held");
        let _: Status = client.call_as("pause", ()).expect("paused");
        let held = json!({"events": events, "hold": 2});
        let paused: Status = client.call_as("input", held).expect("held while paused");
        let click = json!([
            {"button": {"button": "right", "pressed": true}},
            {"button": {"button": "right", "pressed": false}},
        ]);
        let clicked = json!({"events": click, "hold": 3});
        let clicked: Status = client.call_as("input", clicked).expect("a click stepped");
        (refused, running, paused, clicked)
    });
    // Each tick's number, and whether W was down through it.
    let mut ticks = Vec::new();
    let start = Instant::now();
    while !client.is_finished() {
        assert!(start.elapsed() < DEADLINE, "the client never finished");
        api.serve(&mut cx, &mut stack, &mut stepper);
        let tick = |tick: Tick| {
            ticks.push((tick.number, cx.input().key_down(Key::KeyW)));
            true
        };
        stepper.step(Tick::DT * 3, tick, || Duration::ZERO);
        thread::sleep(Duration::from_millis(1));
    }
    let (refused, running, paused, clicked) = client.join().expect("the client didn't panic");
    assert_eq!(refused, "api.bad_params");
    // The first tick and count of each run of ticks with W down.
    let mut held: Vec<(u64, u64)> = Vec::new();
    for &(number, down) in &ticks {
        match held.last_mut() {
            Some((first, count)) if down && *first + *count == number => *count += 1,
            _ if down => held.push((number, 1)),
            _ => {}
        }
    }
    assert_eq!(
        held.iter().map(|&(_, count)| count).collect::<Vec<_>>(),
        [4, 2]
    );
    assert!(running.held.is_empty(), "let go of: {:?}", running.held);
    assert_eq!((running.ticks, running.paused), (held[0].0 + 4, false));
    assert_eq!((paused.ticks, paused.paused), (held[1].0 + 2, true));
    assert_eq!((clicked.ticks, clicked.paused), (paused.ticks + 3, true));
    assert!(running.cursor_grabbed && paused.cursor_grabbed);
    assert!(!cx.input().key_down(Key::KeyW));
    assert!(!cx.input().button_down(MouseButton::Left));
}

/// An `input`'s waits hand each run of events after exactly its ticks, though each frame owes
/// three, and a wait of 0 waits for none. An aim moves the cursor to where the view shows its
/// entity or point as its run is handed. An aim the view doesn't show fails, handing nothing after
/// it, and a hold lets go of what came before. An aim needs `view` and `read`, as `project` does.
#[test]
fn waits_hand_each_run_after_its_ticks_and_aims_find_their_entity() {
    use std::f32::consts::FRAC_PI_2;

    use gg_core::math::Quat;
    use gg_renderer::Projection;

    let (mut api, session) = started("Api Waits");
    let mut cx = Context::seeded(5);
    let world = cx.world_mut();
    let at = |x, z| Transform {
        translation: DVec3::new(x, 0.0, z),
        ..Transform::IDENTITY
    };
    world.spawn((at(3.0, -10.0), Name("Target".to_owned())));
    world.spawn((at(0.0, 5.0), Name("Behind".to_owned())));
    // As the cast's test: three across at 10 deep is at (57.5, 25) of the view.
    cx.set_viewpoint(Viewpoint {
        position: DVec3::ZERO,
        rotation: Quat::IDENTITY,
        projection: Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        },
    });
    let view = Viewport {
        origin: [10, 20],
        size: [100, 50],
    };
    cx.extract(&mut Extracted::default(), view);
    let mut stack = Stack::default();
    let mut stepper = Stepper::default();
    let granted = session.clone();
    let client = thread::spawn(move || {
        let mut client = Client::connect(&session, "test", DEADLINE).expect("it connects");
        let events = json!([
            {"key": {"key": "KeyW", "pressed": true}},
            {"wait": 0},
            {"wait": 3},
            {"aim": "Target"},
            {"key": {"key": "KeyD", "pressed": true}},
            {"wait": 2},
            {"key": {"key": "KeyW", "pressed": false}},
            {"aim": [-3.0, 0.0, -10.0]},
        ]);
        let held = json!({"events": events, "hold": 2});
        let waited: Status = client.call_as("input", held).expect("waited");
        let mut refused = |params| client.call("input", params).expect_err("refused").code;
        let key = |key, pressed| json!({"key": {"key": key, "pressed": pressed}});
        let refusals = [
            refused(json!({"events": [{"aim": [3.0, 0.0]}]})),
            refused(json!({"events": [{"wait": 9000}], "hold": 1001})),
            refused(json!({"events": [{"aim": "Nobody"}]})),
            refused(json!({"events": [{"aim": "Behind"}]})),
            refused(json!({"events": [
                key("KeyS", true), {"wait": 1}, {"aim": "Behind"}, key("KeyA", true),
            ], "hold": 1})),
        ];
        let grant = Some(vec!["input".to_owned()]);
        let connected = Client::connect_granted(&granted, "test", grant, DEADLINE);
        let mut player = connected.expect("it connects");
        let aimed = player.call("input", json!({"events": [{"aim": "Target"}]}));
        let ungranted = aimed.expect_err("ungranted").code;
        let alone = player.call("input", json!({"events": [key("KeyE", true)]}));
        (waited, refusals, ungranted, alone.is_ok())
    });
    // Each tick's number, whether W and D were down through it, and the cursor.
    let mut ticks = Vec::new();
    let start = Instant::now();
    while !client.is_finished() {
        assert!(start.elapsed() < DEADLINE, "the client never finished");
        api.serve(&mut cx, &mut stack, &mut stepper);
        let tick = |tick: Tick| {
            let input = cx.input();
            let down = [Key::KeyW, Key::KeyD].map(|key| input.key_down(key));
            ticks.push((tick.number, down, input.cursor()));
            true
        };
        stepper.step(Tick::DT * 3, tick, || Duration::ZERO);
        thread::sleep(Duration::from_millis(1));
    }
    let (waited, refusals, ungranted, alone) = client.join().expect("the client didn't panic");
    let first = ticks
        .iter()
        .find(|(_, [w, _], _)| *w)
        .expect("W went down")
        .0;
    let downs = |key: usize| {
        let down = ticks.iter().filter(|(_, down, _)| down[key]);
        down.map(|&(number, ..)| number - first).collect::<Vec<_>>()
    };
    assert_eq!(downs(0), [0, 1, 2, 3, 4], "W for both waits");
    assert_eq!(
        downs(1),
        [3, 4, 5, 6],
        "D after the first, for the second and the hold"
    );
    assert_eq!(waited.ticks, first + 7);
    let aimed = ticks.iter().find(|&&(number, ..)| number == first + 3);
    let [x, y] = aimed.and_then(|&(.., cursor)| cursor).expect("aimed");
    assert!(
        (x - 67.5).abs() < 1e-9 && (y - 45.0).abs() < 1e-9,
        "{x}, {y}"
    );
    let before = ticks.iter().find(|&&(number, ..)| number == first + 2);
    assert_eq!(before.and_then(|&(.., cursor)| cursor), None, "not yet");
    let pointed = ticks.iter().find(|&&(number, ..)| number == first + 5);
    let [x, y] = pointed
        .and_then(|&(.., cursor)| cursor)
        .expect("aimed at the point");
    assert!(
        (x - 52.5).abs() < 1e-9 && (y - 45.0).abs() < 1e-9,
        "{x}, {y}"
    );
    let unseen = "api.unseen".to_owned();
    let expected = ["api.bad_params", "api.bad_params", "ecs.no_entity"].map(str::to_owned);
    assert_eq!(refusals[..3], expected);
    assert_eq!(refusals[3..], [unseen.clone(), unseen]);
    assert!(!cx.input().key_down(Key::KeyS), "let go of");
    assert!(!cx.input().key_down(Key::KeyA), "not handed");
    assert_eq!((ungranted.as_str(), alone), ("api.ungranted", true));
}

/// What [`Controlled`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Run,
    Jump,
    Move,
}

/// Binds actions to keys and a gamepad and keeps each change they give, as a game's layer does.
struct Controlled {
    actions: crate::Actions<Act>,
    changes: Rc<std::cell::RefCell<Vec<crate::Change<Act>>>>,
}

impl crate::Layer for Controlled {
    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> std::ops::ControlFlow<()> {
        let changes = &self.changes;
        self.actions
            .event(event, |change| changes.borrow_mut().push(change));
        std::ops::ControlFlow::Continue(())
    }

    fn actions(&self) -> crate::Controls {
        self.actions.controls()
    }
}

/// `actions` lists the layers' actions with their bindings and what they do, and the controls read
/// apart from them. `input` sets an action by name through the first binding that gives its value,
/// letting go of the others: keys for -1, 0 and 1, the stick between. A name no layer binds, or a
/// value of another kind or that no binding gives, fails, handing nothing, even past a wait. A hold
/// brings a stick back to rest, and a gamepad's own events reach the actions too.
#[test]
fn actions_are_listed_and_set_by_name_through_their_bindings() {
    use crate::{Binding, Change, Control, MouseButton, PadButton, Source, Stick, Toward};
    use gg_api::{ActionKind, ActionList, Pointer, Reads};
    use gg_core::math::Vec2;

    let (mut api, session) = started("Api Actions");
    let mut cx = Context::seeded(5);
    let mut stack = Stack::default();
    let mut actions = crate::Actions::default();
    let keys = |negative, positive| Binding::Axis {
        negative: Source::Key(negative),
        positive: Source::Key(positive),
    };
    let stick = |negative, positive| Binding::Axis {
        negative: Source::PadStick(Stick::Left, negative),
        positive: Source::PadStick(Stick::Left, positive),
    };
    let bound = actions.bind_all([
        (Act::Run, keys(Key::KeyA, Key::KeyD)),
        (Act::Run, stick(Toward::Left, Toward::Right)),
        (Act::Jump, Binding::Button(Source::Key(Key::Space))),
        (
            Act::Jump,
            Binding::Button(Source::PadButton(PadButton::South)),
        ),
        (Act::Move, Binding::ARROWS),
    ]);
    bound.expect("each action's bindings are of one kind");
    actions.describe(Act::Jump, "Jumps");
    actions.describe(Act::Run, "Walks");
    actions.describe(Act::Run, "Runs: 1 is 6 units a second");
    let click = Control::Source(Source::Button(MouseButton::Left));
    actions.describe_control(Control::Motion, "Looks: 0.0025 radians a unit");
    actions.describe_control(click, "Fires");
    let changes = Rc::default();
    stack.push_layer(Controlled {
        actions,
        changes: Rc::clone(&changes),
    });
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let listed: ActionList = client.call_as("actions", ()).expect("listed");
        let mut input = |events: Value| client.call("input", json!({"events": events}));
        let action = |name: &str, value: Value| json!({"action": {"name": name, "value": value}});
        let handed = [
            json!([action("Run", json!(1)), {"action": {"name": "Jump"}}]),
            json!([action("Run", json!(-0.5))]),
            json!([action("Jump", json!(0)), {"pad_button": {"button": "south", "pressed": true}}]),
            json!([action("Move", json!([1, 1]))]),
        ];
        for events in handed {
            input(events).expect("handed");
        }
        let refusals = [
            json!([{"key": {"key": "KeyE", "pressed": true}}, {"action": {"name": "Jmp"}}]),
            json!([action("Move", json!([0.5, 0]))]),
            json!([action("Jump", json!(0.5))]),
            json!([action("Run", json!([1, 0]))]),
            json!([{"action": {"name": "Run"}}]),
            json!([{"pad_stick": {"stick": "left", "value": [1, 1]}}]),
            json!([{"pad_trigger": {"trigger": "left", "value": 2}}]),
        ];
        let mut refused: Vec<String> = (refusals.into_iter())
            .map(|events| input(events).expect_err("refused").code)
            .collect();
        let waited = json!({"events": [
            {"key": {"key": "KeyQ", "pressed": true}}, {"wait": 1}, {"action": {"name": "Nope"}},
        ], "hold": 1});
        refused.push(client.call("input", waited).expect_err("refused").code);
        let held = json!({"events": [action("Run", json!(0.25))], "hold": 2});
        let _: Status = client.call_as("input", held).expect("held");
        (listed, refused)
    };
    let (listed, refused) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let kinds: Vec<(&str, &str, ActionKind, usize)> = (listed.actions.iter())
        .map(|action| {
            let (name, layer) = (action.name.as_str(), action.layer.as_str());
            (name, layer, action.kind, action.bindings.len())
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("Run", "Controlled", ActionKind::Axis, 2),
            ("Jump", "Controlled", ActionKind::Button, 2),
            ("Move", "Controlled", ActionKind::Axis2, 1),
        ]
    );
    let stick = |toward| gg_api::Source::PadStick(gg_api::Stick::Left, toward);
    assert_eq!(
        listed.actions[0].bindings[1],
        [stick(gg_api::Toward::Left), stick(gg_api::Toward::Right)]
    );
    let south = gg_api::Source::PadButton(gg_api::PadButton::South);
    let space = gg_api::Source::Key("Space".to_owned());
    assert_eq!(listed.actions[1].bindings, [[space], [south]]);
    // What was said last, by action; none for Move.
    let does: Vec<Option<&str>> = (listed.actions.iter())
        .map(|action| action.does.as_deref())
        .collect();
    assert_eq!(
        does,
        [Some("Runs: 1 is 6 units a second"), Some("Jumps"), None]
    );
    let left = gg_api::Source::Button(gg_api::Button::Left);
    let controls: Vec<(&str, &Reads, &str)> = (listed.controls.iter())
        .map(|c| (c.layer.as_str(), &c.reads, c.does.as_str()))
        .collect();
    assert_eq!(
        controls,
        [
            (
                "Controlled",
                &Reads::Pointer(Pointer::Motion),
                "Looks: 0.0025 radians a unit"
            ),
            ("Controlled", &Reads::Source(left), "Fires"),
        ]
    );
    assert_eq!(refused[0], "api.no_action");
    assert_eq!(refused[1..7], ["api.bad_params"; 6]);
    assert_eq!(refused[7], "api.no_action");
    assert!(!cx.input().key_down(Key::KeyE) && !cx.input().key_down(Key::KeyQ));
    let diagonal = Vec2::ONE.clamp_length_max(1.0);
    assert_eq!(
        *changes.borrow(),
        [
            Change::Axis(Act::Run, 1.0),
            Change::Pressed(Act::Jump),
            // D let go of, then the stick.
            Change::Axis(Act::Run, 0.0),
            Change::Axis(Act::Run, -0.5),
            Change::Released(Act::Jump),
            Change::Pressed(Act::Jump),
            Change::Axis2(Act::Move, Vec2::X),
            Change::Axis2(Act::Move, diagonal),
            Change::Axis(Act::Run, 0.25),
            Change::Axis(Act::Run, 0.0),
        ]
    );
}

/// Hands its UI each event and keeps the buttons it pressed, as a game's menu does.
struct Menu {
    ui: crate::GameUi,
    pressed: Rc<std::cell::RefCell<Vec<String>>>,
}

impl crate::Layer for Menu {
    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> std::ops::ControlFlow<()> {
        if let Some(crate::UiEvent::Pressed(name)) = self.ui.event(cx, event) {
            self.pressed.borrow_mut().push(name.to_string());
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// `press` clicks a UI's button or widget by name, answering what the UIs gave their layers, and
/// with `ticks` steps after it. It refuses, clicking nothing, what a click wouldn't press.
#[test]
fn a_press_clicks_a_ui_node_by_name_and_says_what_the_ui_gave() {
    let scratch = Scratch::new("api-press");
    fs::create_dir_all(&scratch.0).expect("a directory");
    let path = scratch.0.join("menu.ui");
    let file = r#"(nodes: [
            Column(gap: 20.0, children: [
                Button(name: "play", width: Fixed(400.0), height: Fixed(80.0)),
                Button(name: "quit", width: Fixed(400.0), height: Fixed(80.0)),
                Button(name: "gone", width: Fixed(400.0), height: Fixed(80.0)),
                Toggle(name: "sound", text: "SOUND", width: Fixed(400.0), height: Fixed(80.0)),
                Text(name: "title", text: "TITLE"),
            ]),
        ])"#;
    fs::write(&path, file).expect("written");
    let (mut api, session) = started("Api Press");
    let mut cx = Context::seeded(5);
    let ui = cx.add_ui("menu", &path).expect("added");
    // Of the same file, never drawn.
    cx.add_ui("pause", &path).expect("added");
    ui.set_disabled(&mut cx, "quit", true).expect("disabled");
    ui.set_visible(&mut cx, "gone", false).expect("hidden");
    let pressed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut stack = Stack::default();
    stack.push_layer(Menu {
        ui,
        pressed: pressed.clone(),
    });
    let view = Viewport {
        origin: [0, 0],
        size: [1920, 1080],
    };
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let refused = [
            json!({"node": "play"}),
            json!({"node": "play", "ui": "nope"}),
            json!({"node": "nope"}),
            json!({"node": "title", "ui": "menu"}),
            json!({"node": "play", "ui": "pause"}),
            json!({"node": "gone", "ui": "menu"}),
            json!({"node": "quit", "ui": "menu"}),
        ];
        let refused = refused.map(|params| client.call("press", params).expect_err("refused").code);
        let press =
            |client: &mut Client, params: Value| client.call_as::<gg_api::Pressed>("press", params);
        let play = press(&mut client, json!({"node": "play", "ui": "menu"})).expect("pressed");
        let sound = press(&mut client, json!({"node": "sound", "ui": "menu"})).expect("flipped");
        let paused: Status = client.call_as("pause", ()).expect("paused");
        let stepped = json!({"node": "play", "ui": "menu", "ticks": 2});
        let stepped = press(&mut client, stepped).expect("pressed and stepped");
        (refused, play, sound, paused, stepped)
    };
    let (refused, play, sound, paused, stepped) = serving(
        &mut api,
        &mut cx,
        &mut stack,
        &mut stepper,
        session,
        client,
        |_, cx, _| {
            ui.draw(cx);
            cx.extract(&mut Extracted::default(), view);
        },
    );
    let codes = [
        "ui.ambiguous_node",
        "ui.no_ui",
        "ui.no_node",
        "ui.wrong_kind",
        "ui.no_input",
        "ui.hidden",
        "ui.disabled",
    ];
    assert_eq!(refused, codes);
    assert_eq!((play.ui.as_str(), play.node.as_str()), ("menu", "play"));
    // The top of a column of four 80-unit buttons 20 apart and a text, centered in the view.
    assert_eq!(play.at[0], 960.0);
    let given = |pressed: &gg_api::Pressed| {
        let events = pressed.events.iter();
        events
            .map(|event| (event.kind, event.node.clone(), event.value))
            .collect::<Vec<_>>()
    };
    let named = |name: &str| Some(name.to_owned());
    let focused = (gg_api::UiEventKind::Focused, named("play"), None);
    let clicked = (gg_api::UiEventKind::Pressed, named("play"), None);
    assert_eq!(given(&play), [focused.clone(), clicked.clone()]);
    assert!(play.events.iter().all(|event| event.ui == "menu"));
    let flipped = (
        gg_api::UiEventKind::Changed,
        named("sound"),
        Some(gg_api::UiValue::On(true)),
    );
    assert_eq!(given(&sound).last(), Some(&flipped));
    // The cursor comes back from the toggle, and the press steps its ticks.
    assert_eq!(given(&stepped), [focused, clicked]);
    assert_eq!(stepped.ticks, paused.ticks + 2);
    assert_eq!(*pressed.borrow(), ["play", "play"]);
}

/// Publishing the view to memory the client made: the next frame back takes it to the render thread
/// with what paces frames by it. A name that opens no memory is refused, leaving publishing as it
/// was; `null` stops it.
#[test]
fn the_view_is_published_to_the_memory_a_client_names() {
    let (mut api, session) = started("Api View");
    let mut cx = Context::seeded(6);
    let mut stepper = Stepper::default();
    let mut frame = Extracted::default();
    let client = |mut client: Client| {
        let reader = gg_api::view::Reader::create(8, 4).expect("memory for the view");
        let memory = json!({"memory": reader.name()});
        client
            .call("view", memory.clone())
            .expect("the view published");
        // Again: no change, not a second writer.
        client
            .call("view", memory)
            .expect("the view published still");
        let missing = json!({"memory": "gg-view-0-4294967295"});
        let refused = client.call("view", missing).expect_err("no such memory");
        (reader, refused.code)
    };
    let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
        api.frame_back(&mut frame, cx, stepper.ticks);
    };
    let (reader, refused) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session.clone(),
        client,
        back,
    );
    assert_eq!(refused, "api.view");
    let publish = frame.publish.as_ref().expect("the view published");
    assert_eq!(publish.writer.asked(), Some([8, 4]));
    assert_eq!(
        api.handing(),
        Some(1),
        "the frame handed off next publishes first"
    );
    let client = |mut client: Client| {
        client
            .call("view", Value::Null)
            .expect("publishing stopped");
    };
    let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
        api.frame_back(&mut frame, cx, stepper.ticks);
    };
    serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        back,
    );
    assert!(frame.publish.is_none());
    assert_eq!(api.handing(), None);
    drop(reader);
}

/// What [`clients_set_the_look_part_by_part_the_last_setter_winning`]'s clients get.
struct Looks {
    read: Looked,
    set: Looked,
    again: Looked,
    theirs: Looked,
    given_back: Looked,
    run: u64,
}

/// Clients set the look part by part. A part stays until a client sets it again (last setter wins);
/// `null` gives it back to the game. A `look` changing no value or setter counts no revision.
/// Frames get the look as answered.
#[test]
fn clients_set_the_look_part_by_part_the_last_setter_winning() {
    let (mut api, session) = started("Api Looks");
    let mut cx = Context::seeded(8);
    let mut stepper = Stepper::default();
    let other = session.clone();
    let client = move |mut client: Client| {
        let mut them = Client::connect(&other, "other", DEADLINE).expect("it connects");
        let viewpoint = json!({"position": [0, 0, 5], "rotation": [0, 0, 0, 2],
            "projection": {"Orthographic": {"height": 4, "near": -1, "far": 1}}});
        let look = |client: &mut Client, params: Value| {
            client.call_as::<Looked>("look", params).expect("the look")
        };
        Looks {
            read: look(&mut client, Value::Null),
            set: look(&mut client, json!({"viewpoint": viewpoint, "grid": "xz"})),
            again: look(&mut client, json!({"grid": "xz"})),
            theirs: look(
                &mut them,
                json!({"grid": "xy", "colliders": true, "max_fps": 30}),
            ),
            given_back: look(&mut client, json!({"viewpoint": null, "colliders": null})),
            run: (client.call_as::<LogEvents>("logs", json!({"last": 0})))
                .expect("the logs")
                .run,
        }
    };
    let looks = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let by = |name: &str| Some(name.to_owned());
    assert_eq!((looks.read.by, looks.read.revision), (By::default(), 0));
    assert_eq!(
        looks.read.run, looks.run,
        "the run's number, as `logs` gives it"
    );
    assert_eq!(looks.read.seen, None, "before a frame drew");
    let viewpoint = gg_api::Viewpoint {
        position: [0.0, 0.0, 5.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
    };
    let set = &looks.set;
    assert_eq!(set.viewpoint, Some(viewpoint), "its rotation normalized");
    assert_eq!((set.grid, set.colliders), (Some(gg_api::Plane::Xz), None));
    let mine = By {
        viewpoint: by("test"),
        grid: by("test"),
        ..By::default()
    };
    assert_eq!((&set.by, set.revision), (&mine, 1));
    assert_eq!(looks.again, looks.set, "nothing changed, nor counted");
    let theirs = &looks.theirs;
    assert_eq!(
        (theirs.viewpoint, theirs.grid),
        (Some(viewpoint), Some(gg_api::Plane::Xy))
    );
    let both = By {
        viewpoint: by("test"),
        grid: by("other"),
        colliders: by("other"),
        max_fps: by("other"),
        ..By::default()
    };
    assert_eq!(
        (
            &theirs.by,
            theirs.colliders,
            theirs.max_fps,
            theirs.revision
        ),
        (&both, Some(true), NonZeroU32::new(30), 2)
    );
    let back = &looks.given_back;
    assert_eq!(
        (back.viewpoint, back.grid, back.colliders),
        (None, Some(gg_api::Plane::Xy), None)
    );
    let left = By {
        grid: by("other"),
        max_fps: by("other"),
        ..By::default()
    };
    assert_eq!((&back.by, back.revision), (&left, 3));
    let look = &cx.dev.look;
    assert_eq!(
        (look.viewpoint, look.grid, look.colliders, look.max_fps),
        (None, Some(Plane::XY), None, NonZeroU32::new(30))
    );
    assert_eq!(look.revision, 3);
}

/// A look's render scale, from the least to 1, replaces the game's own while set; `null` gives it
/// back. One outside that range is refused, the look as it was. Frames get the scale as answered.
#[test]
fn a_looks_render_scale_replaces_the_games_own_until_given_back() {
    for scale in [json!(gg_renderer::MIN_RENDER_SCALE), json!(0.5), json!(1)] {
        let params = json!({ "render_scale": scale });
        let request = Request {
            id: 1,
            method: "look".to_owned(),
            params,
            key: None,
            again: false,
        };
        let parsed = parse(&request).expect("a scale in range");
        let Call::Look(look) = parsed else {
            panic!("a look");
        };
        assert!(matches!(look.render_scale, Some(Some(_))), "{scale}");
    }
    let (mut api, session) = started("Api Look Render Scale");
    let mut cx = Context::seeded(8);
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let mut look =
            |params: Value| (client.call_as::<Looked>("look", params)).expect("the look");
        let set = look(json!({"render_scale": 0.5}));
        let again = look(json!({"render_scale": 0.5}));
        let refused = client.call("look", json!({"render_scale": 2}));
        let after = client.call_as::<Looked>("look", Value::Null);
        let given_back = client.call_as::<Looked>("look", json!({"render_scale": null}));
        let given_back = given_back.expect("given back");
        (
            set,
            again,
            refused.err(),
            after.expect("a read"),
            given_back,
        )
    };
    let (set, again, refused, after, given_back) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let test = Some("test".to_owned());
    assert_eq!(set.render_scale, Some(0.5));
    assert_eq!((&set.by.render_scale, set.revision), (&test, 1));
    assert_eq!(again, set, "nothing changed, nor counted");
    assert_eq!(refused.expect("refused").code, "api.bad_params");
    assert_eq!(after, set, "the look as it was");
    let (scale, by) = (given_back.render_scale, &given_back.by.render_scale);
    assert_eq!((scale, by, given_back.revision), (None, &None, 2));
    assert_eq!((cx.dev.look.render_scale, cx.dev.look.revision), (None, 2));
}

/// `overlays` turns debug overlays on and off by name, in any case, and answers each with the
/// Render menu's choices. A name no overlay has waits; one both on and off is refused, the
/// overlays as they were.
#[test]
fn overlays_turn_on_and_off_by_name_and_answer_each() {
    let (mut api, session) = started("Api Overlays");
    let mut cx = Context::seeded(8);
    cx.dev.overlays.engine = true;
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let asked = json!({"on": ["fps", "Enemy_aim"], "channel": "normal", "skip": ["fog"]});
        let set = client.call_as::<gg_api::Overlaid>("overlays", asked);
        let refused = client.call("overlays", json!({"on": ["fps"], "off": ["FPS"]}));
        let read = client.call_as::<gg_api::Overlaid>("overlays", Value::Null);
        (set.expect("set"), refused.err(), read.expect("read"))
    };
    let (set, refused, read) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let on = set.overlays.iter().filter(|overlay| overlay.on);
    assert_eq!(
        on.map(|overlay| &overlay.name[..]).collect::<Vec<_>>(),
        ["FPS"]
    );
    assert_eq!(set.waiting, ["Enemy_aim"]);
    assert_eq!(set.channel, Some(gg_api::Shown::Normal));
    assert_eq!(set.skip, [gg_api::Pass::Fog]);
    assert!(!set.bar);
    assert_eq!(refused.expect("refused").code, "api.bad_params");
    assert_eq!(read, set, "the overlays as they were");
    assert!(
        cx.dev.overlays.asked("enemy aim"),
        "the game's, once it asks"
    );
}

/// A look's left-out passes are a set: any order or repeats answer once each in
/// [`gg_api::Pass::ALL`]'s order, and the same set again is no revision. An empty list gives the
/// part back, as `null` does. Frames get the set as answered.
#[test]
fn the_passes_a_look_leaves_out_are_a_set() {
    use gg_api::Pass;

    let (mut api, session) = started("Api Look Skips");
    let mut cx = Context::seeded(8);
    let mut stepper = Stepper::default();
    let client = |mut client: Client| {
        let mut look =
            |params: Value| (client.call_as::<Looked>("look", params)).expect("the look");
        [
            look(json!({"skip": ["shadows", "reflections", "shadows"]})),
            look(json!({"skip": ["reflections", "shadows"]})),
            look(json!({"skip": []})),
            look(json!({"skip": ["fog"]})),
        ]
    };
    let [set, again, emptied, fog] = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    let test = Some("test".to_owned());
    assert_eq!(set.skip, Some(vec![Pass::Reflections, Pass::Shadows]));
    assert_eq!((&set.by.skip, set.revision), (&test, 1));
    assert_eq!(again, set, "the same set, nor counted");
    assert_eq!(
        (&emptied.skip, &emptied.by.skip),
        (&None, &None),
        "given back"
    );
    assert_eq!(emptied.revision, 2);
    assert_eq!((fog.skip, fog.revision), (Some(vec![Pass::Fog]), 3));
    assert_eq!(
        (&cx.dev.look.skip, cx.dev.look.revision),
        (&vec![Pass::Fog], 3)
    );
}

/// The look draws as a layer that set it would: its viewpoint and outline replace a layer's, its
/// colliders' outlines the game's, and its grid draws before the world. Layers' own calls still say
/// they set; a bare frame draws as the layers set.
#[test]
fn the_look_draws_over_what_the_layers_set_but_in_a_bare_frame() {
    let (mut api, session) = started("Api Look Draws");
    // A sprite with a collider: over the grid, under the colliders' outlines.
    let world = |seed| {
        let mut cx = Context::seeded(seed);
        let shown = (
            Transform::IDENTITY,
            Sprite::default(),
            BoxCollider2D::default(),
        );
        let hero = cx.world_mut().spawn(shown);
        let id = cx.world().id(hero).expect("alive");
        (cx, id)
    };
    let (mut cx, hero) = world(9);
    let view = Viewport {
        origin: [0, 0],
        size: [64, 32],
    };
    cx.extract(&mut Extracted::default(), view);
    let mut stepper = Stepper::default();
    let looked = gg_api::Viewpoint {
        position: [1.0, 2.0, 10.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        projection: gg_api::Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
    };
    let look = json!({
        "viewpoint": looked,
        "outline": {"ids": [hero.to_string()], "color": [1, 0.5, 0, 1], "width": 3},
        "grid": "xy",
        "colliders": true,
        "show": {"image": "view depth"},
    });
    let client = move |mut client: Client| {
        client.call_as::<Looked>("look", look).expect("the look");
    };
    serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        |_, _, _| {},
    );
    // What the layers set this frame.
    let theirs = Viewpoint::default();
    let set = |cx: &mut Context| {
        cx.set_viewpoint(theirs);
        cx.outline(hero, Vec4::ONE, 1.0);
    };
    let drawn = |cx: &mut Context| {
        cx.draw_world(1.0, 0.0, 0);
        let mut frame = Extracted::default();
        cx.extract(&mut frame, view);
        frame
    };
    set(&mut cx);
    let frame = drawn(&mut cx);
    // As a layer that set the look's parts itself.
    let viewpoint = engine_viewpoint(&looked);
    let (mut twin, _) = world(9);
    twin.extract(&mut Extracted::default(), view);
    twin.set_viewpoint(viewpoint);
    twin.outline(hero, Vec4::new(1.0, 0.5, 0.0, 1.0), 3.0);
    twin.draw_grid(&viewpoint, Plane::XY);
    twin.set_colliders_shown(true);
    let expected = drawn(&mut twin);
    assert_eq!(frame.draws, expected.draws);
    assert_eq!(
        (frame.viewpoint, frame.outline),
        (viewpoint, expected.outline)
    );
    assert_eq!((frame.look, cx.dev.look.seen), (1, Some(viewpoint)));
    assert!(!cx.colliders_shown(), "as the layers set");
    // From the look's viewpoint, whose exposure adapts separately.
    assert!(frame.looked && !expected.looked);
    let depth = Shown::Image("view depth".into());
    assert_eq!(frame.show, Some(depth.clone()), "what the look shows");
    // What a riding capture shows instead of the look.
    let rough = Shown::Shading(gg_renderer::Channel::Roughness);
    cx.dev.look.captured = Some(rough.clone());
    set(&mut cx);
    assert_eq!(
        drawn(&mut cx).show,
        Some(rough.clone()),
        "what the capture shows"
    );
    cx.dev.look.captured = None;
    // Bare, as the layers set alone.
    cx.dev.look.bare = true;
    set(&mut cx);
    let frame = drawn(&mut cx);
    let (mut twin, _) = world(9);
    twin.extract(&mut Extracted::default(), view);
    set(&mut twin);
    let expected = drawn(&mut twin);
    assert_eq!(frame.draws, expected.draws);
    assert_eq!((frame.viewpoint, frame.outline), (theirs, expected.outline));
    assert!(!frame.looked, "a bare frame is the game's own view");
    assert_eq!(frame.show, None, "without what the look shows");
    // Bare, but showing what a capture of the game's own view asks.
    cx.dev.look.captured = Some(rough.clone());
    set(&mut cx);
    assert_eq!(drawn(&mut cx).show, Some(rough), "what the capture shows");
}

/// A look's outline of several entity IDs draws the frame's outline over each subtree once, sorted.
/// A non-entity ID is refused, leaving the look. No IDs outline nothing.
#[test]
fn the_looks_outline_covers_the_subtree_of_each_entity() {
    let (mut api, session) = started("Api Look Outlines");
    let mut cx = Context::seeded(11);
    let view = Viewport {
        origin: [0, 0],
        size: [64, 32],
    };
    let [a, under_a, b, other] = [(); 4].map(|()| cx.world_mut().spawn(Transform::IDENTITY));
    cx.world_mut().set_parent(under_a, Some(a)).expect("alive");
    let [a_id, under_id, b_id, other_id] =
        [a, under_a, b, other].map(|entity| cx.world().id(entity).expect("alive"));
    let mut stepper = Stepper::default();
    let mut look = |cx: &mut Context, params: Value| {
        let client = move |mut client: Client| client.call("look", params).is_ok();
        let (api, session) = (&mut api, session.clone());
        serving(
            api,
            cx,
            &mut Stack::default(),
            &mut stepper,
            session,
            client,
            |_, _, _| {},
        )
    };
    let drawn = |cx: &mut Context| {
        let mut frame = Extracted::default();
        cx.extract(&mut frame, view);
        (frame.outline, frame.outline_ids)
    };
    let wanted = [b_id, under_id, a_id].map(|id| id.to_string());
    let params = json!({"outline": {"ids": wanted, "color": [1, 0.5, 0, 1], "width": 3}});
    assert!(look(&mut cx, params), "an outline of IDs");
    let orange = Vec4::new(1.0, 0.5, 0.0, 1.0);
    let mut want = vec![a_id.get(), under_id.get(), b_id.get()];
    want.sort_unstable();
    assert_eq!(drawn(&mut cx), (Some((orange, 3.0)), want.clone()));
    assert_ne!(other_id, a_id, "an entity left out");

    let refused = json!({"outline": {"ids": [b_id.to_string(), "nope"], "color": [1, 1, 1, 1],
        "width": 1}});
    assert!(!look(&mut cx, refused), "an ID that doesn't parse");
    assert_eq!(drawn(&mut cx), (Some((orange, 3.0)), want), "as it was");

    let none = json!({"outline": {"ids": [], "color": [1, 0, 0, 1], "width": 2}});
    assert!(look(&mut cx, none), "no IDs");
    let red = Vec4::new(1.0, 0.0, 0.0, 1.0);
    assert_eq!(drawn(&mut cx), (Some((red, 2.0)), vec![]));
}

/// Captures of the game's own view ride with a bare frame (not published, paced for, or asking a
/// client's pick); of the view as shown, with a non-bare one; of a part of shading, with a frame
/// showing it (also not published or paced for). Captures asked together go oldest first, each with
/// a frame of its kind.
#[test]
fn captures_ride_with_frames_of_their_kind_and_bare_ones_go_unpublished() {
    let (mut api, session) = started("Api Bare Captures");
    let mut cx = Context::seeded(10);
    let mut stepper = Stepper::default();
    let mut frame = Extracted::default();
    let client = |mut client: Client| {
        let reader = gg_api::view::Reader::create(8, 4).expect("memory for the view");
        let memory = json!({"memory": reader.name()});
        client.call("view", memory).expect("the view published");
        reader
    };
    let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
        api.frame_back(&mut frame, cx, stepper.ticks);
    };
    let reader = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        back,
    );
    let dir = Scratch::new("api-bare-capture");
    fs::create_dir_all(&dir.0).expect("a directory");
    let (to, answers) = mpsc::channel();
    let rough = Shown::Shading(gg_renderer::Channel::Roughness);
    for (id, own, show) in [(1, true, None), (2, false, None), (3, false, Some(rough))] {
        api.asked.push(Capturing {
            reply: reply(id, &to),
            path: Some(dir.0.join(format!("{id}.png"))),
            at: None,
            own,
            show,
            since: Instant::now(),
        });
    }
    api.picks.push(Picking {
        at: [0.0, 0.0],
        within: None,
        ticket: None,
        named: true,
        reply: reply(4, &to),
    });
    // Each frame: bare, shows a part of shading, rides with captures, published, paced for, asked
    // the pick.
    let mut frames = Vec::new();
    for _ in 0..7 {
        api.serve(&mut cx, &mut Stack::default(), &mut stepper);
        let (bare, shows) = (cx.dev.look.bare, cx.dev.look.shown().is_some());
        let asked = api.picks[0].ticket.is_some();
        api.frame_back(&mut frame, &cx, stepper.ticks);
        let skip = frame.publish.as_ref().expect("published").skip;
        let paced = api.handing().is_some();
        frames.push((bare, shows, frame.capture, !skip, paced, asked));
        // As the renderer reads back a frame riding with captures: a bare one is 1 pixel wide, one
        // showing a part of shading 3, another 2.
        frame.captured = frame.capture.then(|| {
            let width = if bare {
                1
            } else if shows {
                3
            } else {
                2
            };
            let pixels = vec![0; width * 4];
            Ok(Shot {
                size: [width as u32, 1],
                pixels,
            })
        });
    }
    let expected = [
        (true, false, true, false, false, false),
        // Bare too, as the riding capture decides, before its frame is known rendered.
        (true, false, false, false, false, false),
        (false, false, true, true, true, true),
        (false, false, false, true, true, true),
        (false, true, true, false, false, true),
        (false, true, false, false, false, true),
        (false, false, false, true, true, true),
    ];
    assert_eq!(frames, expected);
    let mut widths = [0; 3];
    for _ in 0..3 {
        let (line, _) = answers.recv_timeout(DEADLINE).expect("an answer");
        let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
        let id = answer["id"].as_u64().expect("its ID") as usize;
        widths[id - 1] = answer["result"]["width"]
            .as_u64()
            .expect("a capture's width");
    }
    assert_eq!(
        widths,
        [1, 2, 3],
        "the game's own view bare, the next as it's shown, the last showing its roughness"
    );
    drop(reader);
}

/// A capture rides on with frames showing a texture still decoding or meshes or textures streaming,
/// until one shows everything, saying nothing was on its way. One that has waited past its bound
/// takes the next frame that brings the view back, saying something was.
#[test]
fn a_capture_waits_within_its_bound_for_assets_to_stream() {
    let (mut api, _) = started("Api Streaming Captures");
    let mut cx = Context::seeded(12);
    let mut stepper = Stepper::default();
    let mut frame = Extracted::default();
    let dir = Scratch::new("api-streaming-capture");
    fs::create_dir_all(&dir.0).expect("a directory");
    let (to, answers) = mpsc::channel();
    let past = CAPTURE_WAIT + Duration::from_secs(1);
    let long_ago = Instant::now()
        .checked_sub(past)
        .expect("a time that long ago");
    for (id, since) in [(1, long_ago), (2, Instant::now())] {
        api.asked.push(Capturing {
            reply: reply(id, &to),
            path: Some(dir.0.join(format!("{id}.png"))),
            at: None,
            own: false,
            show: None,
            since,
        });
    }
    // The next answer: its capture's ID, and whether it says what it shows was still on its way.
    let answer = |timeout| {
        let (line, _) = answers.recv_timeout(timeout).ok()?;
        let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
        let id = answer["id"].as_u64().expect("its ID");
        Some((id, answer["result"]["streaming"].as_bool()))
    };
    // As the renderer reads back each frame riding with captures: three with a texture decoding or
    // meshes streaming, then one with nothing on its way.
    for (loading, streaming) in [(1, 0), (0, 2), (1, 2), (0, 0)] {
        api.serve(&mut cx, &mut Stack::default(), &mut stepper);
        api.frame_back(&mut frame, &cx, stepper.ticks);
        assert!(frame.capture, "a capture rides with the frame");
        frame.loading = loading;
        frame.streaming = Streaming {
            assets: streaming,
            bytes: 64 * streaming as u64,
        };
        frame.captured = Some(Ok(Shot {
            size: [1, 1],
            pixels: vec![0; 4],
        }));
    }
    assert_eq!(answer(DEADLINE), Some((1, Some(true))), "past its bound");
    assert_eq!(answer(Duration::from_millis(200)), None, "waiting still");
    api.frame_back(&mut frame, &cx, stepper.ticks);
    assert_eq!(
        answer(DEADLINE),
        Some((2, Some(false))),
        "once nothing is on its way"
    );
    assert!(!frame.capture, "nothing rides once both are answered");
}

/// A capture of paths traced rides on while the frames' pixels have taken fewer than
/// [`gg_api::PATH_SAMPLES`], and takes the first that has taken them all. One that has waited past
/// its bound fails, saying how many they took.
#[test]
fn a_capture_of_paths_traced_waits_for_their_samples() {
    let (mut api, _) = started("Api Traced Captures");
    let mut cx = Context::seeded(13);
    let mut stepper = Stepper::default();
    let mut frame = Extracted::default();
    let dir = Scratch::new("api-traced-capture");
    fs::create_dir_all(&dir.0).expect("a directory");
    let (to, answers) = mpsc::channel();
    let past = TRACE_WAIT + Duration::from_secs(1);
    let long_ago = Instant::now()
        .checked_sub(past)
        .expect("a time that long ago");
    for (id, since) in [(1, long_ago), (2, Instant::now())] {
        api.asked.push(Capturing {
            reply: reply(id, &to),
            path: Some(dir.0.join(format!("{id}.png"))),
            at: None,
            own: false,
            show: Some(Shown::PathTraced),
            since,
        });
    }
    // The next answer: its capture's ID, and its error's code if it failed.
    let answer = |timeout| {
        let (line, _) = answers.recv_timeout(timeout).ok()?;
        let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
        let id = answer["id"].as_u64().expect("its ID");
        let code = answer["error"]["code"].as_str().map(str::to_owned);
        Some((
            id,
            code,
            answer["error"]["message"].as_str().map(str::to_owned),
        ))
    };
    // As the renderer reads back each frame riding with captures: paths short of their samples,
    // then all of them.
    for taken in [4, 512, gg_api::PATH_SAMPLES] {
        api.serve(&mut cx, &mut Stack::default(), &mut stepper);
        api.frame_back(&mut frame, &cx, stepper.ticks);
        assert!(frame.capture, "a capture rides with the frame");
        frame.path_traced = Some(taken);
        frame.captured = Some(Ok(Shot {
            size: [1, 1],
            pixels: vec![0; 4],
        }));
    }
    let (id, code, message) = answer(DEADLINE).expect("an answer past its bound");
    assert_eq!((id, code.as_deref()), (1, Some("api.path_traced")));
    let message = message.expect("a message");
    assert!(message.contains("took 4 of 1024"), "{message}");
    assert_eq!(answer(Duration::from_millis(200)), None, "waiting still");
    api.frame_back(&mut frame, &cx, stepper.ticks);
    let (id, code, _) = answer(DEADLINE).expect("an answer once all are taken");
    assert_eq!((id, code), (2, None));
    assert!(!frame.capture, "nothing rides once both are answered");
}

/// An `images` has frames list their images, riding on past a frame that didn't render until one
/// brings its images back and answers it as the API names them. Then no frame lists.
#[test]
fn images_ride_with_frames_until_one_brings_them_back() {
    let (mut api, _) = started("Api Images");
    let mut cx = Context::seeded(14);
    let mut stepper = Stepper::default();
    stepper.pause();
    let mut frame = Extracted::default();
    let (to, answers) = mpsc::channel();
    api.images_asked.push(reply(1, &to));
    api.serve(&mut cx, &mut Stack::default(), &mut stepper);
    assert!(!api.is_idle(&stepper), "waiting for a frame");
    api.frame_back(&mut frame, &cx, stepper.ticks);
    assert!(frame.list_images, "the frame lists");
    // That frame didn't render, and brings none back.
    api.frame_back(&mut frame, &cx, stepper.ticks);
    assert!(frame.list_images, "the next lists too");
    let waiting = answers.recv_timeout(Duration::from_millis(200));
    assert!(waiting.is_err(), "unanswered");
    frame.images = Some(vec![gg_renderer::FrameImage {
        name: "bloom 1",
        kind: gg_renderer::ImageKind::Hdr,
        size: [32, 16],
    }]);
    api.frame_back(&mut frame, &cx, stepper.ticks);
    let (line, _) = answers.recv_timeout(DEADLINE).expect("an answer");
    let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
    assert_eq!(answer["id"], 1);
    let listed = json!({"images": [{"name": "bloom 1", "kind": "hdr", "size": [32, 16]}]});
    assert_eq!(answer["result"], listed);
    assert!(!frame.list_images, "nothing rides once it's answered");
    assert!(api.is_idle(&stepper), "nothing waits");
}

/// A capture with `at` and no path answers the shot's bytes at that pixel (`None` outside the
/// view), writing no PNG and numbering no engine one. With a path it writes it too.
#[test]
fn a_capture_with_a_pixel_answers_its_bytes_and_writes_nothing() {
    let (mut api, _) = started("Api Probes");
    let mut cx = Context::seeded(13);
    let mut stepper = Stepper::default();
    let mut frame = Extracted::default();
    let dir = Scratch::new("api-probe");
    fs::create_dir_all(&dir.0).expect("a directory");
    let (to, answers) = mpsc::channel();
    let written = dir.0.join("both.png");
    let asks = [
        (None, [1.5, 0.0]),
        (None, [2.0, 0.0]),
        (None, [0.0, 1.0]),
        (Some(written.clone()), [1.0, 1.0]),
    ];
    for (id, (path, at)) in asks.into_iter().enumerate() {
        api.asked.push(Capturing {
            reply: reply(id as u64, &to),
            path,
            at: Some(at),
            own: false,
            show: None,
            since: Instant::now(),
        });
    }
    api.serve(&mut cx, &mut Stack::default(), &mut stepper);
    api.frame_back(&mut frame, &cx, stepper.ticks);
    assert!(frame.capture, "the captures ride with the frame");
    // A 2 by 2 view: top row, then bottom.
    frame.captured = Some(Ok(Shot {
        size: [2, 2],
        pixels: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    }));
    api.frame_back(&mut frame, &cx, stepper.ticks);
    let mut answered = [const { None }; 4];
    for _ in 0..4 {
        let (line, _) = answers.recv_timeout(DEADLINE).expect("an answer");
        let answer: Value = serde_json::from_str(&line).expect("an answer's JSON");
        let id = answer["id"].as_u64().expect("its ID") as usize;
        answered[id] = Some(answer["result"].clone());
    }
    let [a, b, c, d] = answered.map(|answer| answer.expect("answered"));
    assert_eq!(a["pixel"], json!([5, 6, 7, 8]), "the pixel 1.5 falls in");
    assert_eq!(b["pixel"], Value::Null, "outside the view");
    assert_eq!(c["pixel"], json!([9, 10, 11, 12]), "the bottom row");
    for probe in [&a, &b, &c] {
        assert_eq!(probe["path"], Value::Null, "no PNG without a path");
    }
    assert_eq!(api.numbered, 0, "none named for the engine's directory");
    assert_eq!(d["pixel"], json!([13, 14, 15, 16]));
    assert_eq!(d["path"], json!(written));
    assert!(written.is_file(), "a path asked for is written");
}

/// A pick answers the entity drawn at its pixel, named, and its parent; none where nothing is or
/// outside the view. An entity gone by the answer, or any for a client with `view` but not `read`
/// of `Name`, is given by ID alone.
#[test]
fn a_pick_is_answered_with_the_entity_drawn_there() {
    let (mut api, session) = started("Api Picks");
    let mut cx = Context::seeded(11);
    let world = cx.world_mut();
    let hero = world.spawn((Transform::IDENTITY, Name("Hero".to_owned())));
    let ghost = world.spawn((Transform::IDENTITY, Name("Ghost".to_owned())));
    let squad = world.spawn((Transform::IDENTITY, Name("Squad".to_owned())));
    world.set_parent(hero, Some(squad)).expect("alive");
    let [hero, ghost, squad] = [hero, ghost, squad].map(|entity| world.id(entity).expect("alive"));
    world
        .despawn(world.entity(ghost).expect("alive"))
        .expect("despawned");
    let view = Viewport {
        origin: [10, 20],
        size: [100, 50],
    };
    let mut frame = Extracted::default();
    cx.extract(&mut frame, view);
    let mut stepper = Stepper::default();
    let granted = session.clone();
    let client = move |mut client: Client| {
        let pick = |client: &mut Client, at: [f64; 2]| {
            (client.call_as::<Picked>("pick", json!({ "at": at }))).expect("an answer")
        };
        let ats = [[3.5, 4.0], [60.0, 4.0], [80.0, 4.0], [-1.0, 4.0]];
        let picked = ats.map(|at| pick(&mut client, at));
        let names = [&["view"][..], &["view", "read:Name"]].map(|grant| {
            let grant = Some(grant.iter().map(|&name| name.to_owned()).collect());
            let connected = Client::connect_granted(&granted, "test", grant, DEADLINE);
            let mut client = connected.expect("it connects");
            let picked = pick(&mut client, [3.5, 4.0]);
            (picked.name, picked.parent_name)
        });
        (picked, names)
    };
    // As the renderer answers: the hero draws at columns up to 50, the ghost's pixels from 70,
    // nothing between.
    let mut answers = Vec::new();
    let back = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
        cx.take_answers(&answers);
        api.frame_back(&mut frame, cx, stepper.ticks);
        cx.extract(&mut frame, view);
        answers.clear();
        answers.extend(frame.picks.iter().map(|pick| {
            let id = match pick.pixel[0] {
                0..50 => hero.get(),
                70.. => ghost.get(),
                _ => 0,
            };
            (pick.tag, id)
        }));
    };
    let (picked, names) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut stepper,
        session,
        client,
        back,
    );
    let picked = picked.map(|picked| (picked.id, picked.name, picked.parent, picked.parent_name));
    let squad = (Some(squad.to_string()), Some("Squad".to_owned()));
    let expected = [
        (
            Some(hero.to_string()),
            Some("Hero".to_owned()),
            squad.0,
            squad.1,
        ),
        (None, None, None, None),
        (Some(ghost.to_string()), None, None, None),
        (None, None, None, None),
    ];
    assert_eq!(picked, expected);
    let named = (Some("Hero".to_owned()), Some("Squad".to_owned()));
    assert_eq!(names, [(None, None), named], "`view`, then `read:Name` too");
    assert!(api.picks.is_empty());
}

/// A cast from the view's middle hits the nearest 3D collider, past the one the camera is inside,
/// or the next past one excluded, named for a client that reads names; through a pixel beside them, or outside the view, it hits
/// nothing. A projection gives an entity's pixel, the camera's turn to it and its distance; a point
/// behind the camera has no pixel. Both fail before a frame has drawn the view.
#[test]
fn a_cast_hits_the_collider_through_a_pixel_and_a_projection_finds_a_point() {
    use std::f32::consts::FRAC_PI_2;

    use gg_api::{Hit, Projected};
    use gg_core::math::Quat;
    use gg_renderer::Projection;

    use crate::{BodyType, RigidBody3D, SphereCollider3D};

    let undrawn = Context::seeded(1);
    let failed = view::cast(&undrawn, None, None, true)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(failed.code, "api.no_view");
    let point = super::parse::Projecting::Point([0.0; 3]);
    let failed = view::project(&undrawn, point).map(|_| ()).unwrap_err();
    assert_eq!(failed.code, "api.no_view");

    let (mut api, session) = started("Api Casts");
    let mut cx = Context::seeded(12);
    let world = cx.world_mut();
    let at = |x, z| Transform {
        translation: DVec3::new(x, 0.0, z),
        ..Transform::IDENTITY
    };
    let fixed = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let ball = SphereCollider3D::default();
    let named = |name: &str| Name(name.to_owned());
    // The camera stands inside it, as in a first-person player's.
    world.spawn((at(0.0, 0.0), fixed, ball, named("Eye")));
    let near = world.spawn((at(0.0, -5.0), fixed, ball, named("Near")));
    let far = world.spawn((at(0.0, -10.0), fixed, ball, named("Far")));
    world.spawn((at(3.0, -10.0), named("Beside")));
    let [near, far] = [near, far].map(|entity| world.id(entity).expect("alive").to_string());
    crate::physics::step_physics(world);
    // A right angle from bottom to top, twice as wide: at 10 deep, 10 up and 20 across.
    cx.set_viewpoint(Viewpoint {
        position: DVec3::ZERO,
        rotation: Quat::IDENTITY,
        projection: Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        },
    });
    let view = Viewport {
        origin: [10, 20],
        size: [100, 50],
    };
    cx.extract(&mut Extracted::default(), view);
    let granted = session.clone();
    let client = move |mut client: Client| {
        let mut cast = |params| client.call_as::<Hit>("cast", params).expect("an answer");
        let hits = [
            cast(json!({})),
            cast(json!({"exclude": "Near"})),
            cast(json!({"at": [90, 25]})),
            cast(json!({"at": [-1, 25]})),
        ];
        let grant = Some(vec!["view".to_owned()]);
        let connected = Client::connect_granted(&granted, "test", grant, DEADLINE);
        let mut viewer = connected.expect("it connects");
        let unnamed = viewer.call_as::<Hit>("cast", json!({})).expect("an answer");
        let ungranted = viewer.call_as::<Projected>("project", json!({"entity": "Beside"}));
        let mut project =
            |params| (client.call_as::<Projected>("project", params)).expect("an answer");
        let beside = project(json!({"entity": "Beside"}));
        let behind = project(json!({"point": [0, 0, 5]}));
        (hits, unnamed, ungranted.map(|_| ()), beside, behind)
    };
    let (hits, unnamed, ungranted, beside, behind) = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut Stepper::default(),
        session,
        client,
        |_, _, _| {},
    );
    let [first, past, aside, outside] = hits;
    let hit = |id: &str, name: &str, z: f64, distance: f32| Hit {
        id: Some(id.to_owned()),
        name: Some(name.to_owned()),
        point: Some([0.0, 0.0, z]),
        normal: Some([0.0, 0.0, 1.0]),
        distance: Some(distance),
    };
    let close = |a: &Hit, b: &Hit| {
        let points = a
            .point
            .zip(b.point)
            .is_some_and(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4));
        let distances = a
            .distance
            .zip(b.distance)
            .is_some_and(|(a, b)| (a - b).abs() < 1e-4);
        a.id == b.id && a.name == b.name && points && distances
    };
    assert!(close(&first, &hit(&near, "Near", -4.5, 4.5)), "{first:?}");
    assert!(close(&past, &hit(&far, "Far", -9.5, 9.5)), "{past:?}");
    let nothing = Hit {
        id: None,
        name: None,
        point: None,
        normal: None,
        distance: None,
    };
    assert_eq!((aside, outside), (nothing.clone(), nothing));
    assert_eq!(
        (unnamed.id, unnamed.name),
        (Some(near), None),
        "`view` alone"
    );
    assert_eq!(
        ungranted.map_err(|error| error.code),
        Err("api.ungranted".to_owned())
    );
    // Three across at 10 deep is 3/20 of half the view right of its middle.
    assert_eq!(beside.point, [3.0, 0.0, -10.0]);
    let [x, y] = beside.at.expect("in the view");
    assert!(
        (x - 57.5).abs() < 1e-9 && (y - 25.0).abs() < 1e-9,
        "{beside:?}"
    );
    let [right, up] = beside.turn.expect("a perspective");
    assert!(
        (right - gg_core::math::libm::atan(0.3)).abs() < 1e-9 && up.abs() < 1e-9,
        "{beside:?}"
    );
    assert!(
        (beside.distance - 109.0_f64.sqrt()).abs() < 1e-9,
        "{beside:?}"
    );
    assert_eq!((behind.at, behind.distance), (None, 5.0));
    let [right, _] = behind.turn.expect("a perspective");
    assert!(
        (right.abs() - std::f64::consts::PI).abs() < 1e-9,
        "{behind:?}"
    );
}

/// A cast meets a 2D collider where it holds the point the ray crosses z = 0, facing the camera.
/// `entity` answers what an entity touched at the last step, each way, and the box along the
/// world's axes round what it draws.
#[test]
fn a_cast_hits_2d_colliders_and_an_entity_says_what_it_touches_and_its_box() {
    use gg_core::math::{Quat, Vec2};
    use gg_renderer::{Projection, Sprite};

    use crate::{BodyType, BoxCollider2D, RigidBody2D};

    let mut cx = Context::seeded(13);
    let world = cx.world_mut();
    let at = |x, y| Transform {
        translation: DVec3::new(x, y, 0.0),
        ..Transform::IDENTITY
    };
    let body = |body_type| RigidBody2D {
        body_type,
        ..RigidBody2D::default()
    };
    let size = |width, height| BoxCollider2D {
        size: Vec2::new(width, height),
        ..BoxCollider2D::default()
    };
    let named = |name: &str| Name(name.to_owned());
    let ground = (at(0.0, 0.0), body(BodyType::Static), size(8.0, 1.0));
    let ground = world.spawn((ground.0, ground.1, ground.2, named("Ground")));
    // Sunk into the ground a little, to touch it at the first step.
    let crate_ = (at(0.0, 0.95), body(BodyType::Dynamic), size(1.0, 1.0));
    let crate_ = world.spawn((crate_.0, crate_.1, crate_.2, named("Crate")));
    let wall = Transform {
        scale: Vec3::new(4.0, 1.0, 1.0),
        ..at(20.0, 0.0)
    };
    let wall = world.spawn((wall, Sprite::default()));
    crate::physics::step_physics(world);
    let [ground, crate_, wall] =
        [ground, crate_, wall].map(|entity| world.id(entity).expect("alive"));
    // From 10 up, 5 pixels a unit: the view's middle is over the origin.
    cx.set_viewpoint(Viewpoint {
        position: DVec3::new(0.0, 0.0, 10.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Orthographic {
            height: 10.0,
            near: 0.1,
            far: 100.0,
        },
    });
    let view = Viewport {
        origin: [0, 0],
        size: [100, 50],
    };
    cx.extract(&mut Extracted::default(), view);
    let cast = |at, exclude: Option<&str>| {
        let exclude = exclude.map(Subject::of);
        view::cast(&cx, at, exclude, true).expect("a cast")
    };
    let middle = cast(None, None);
    assert_eq!(
        (middle.id, middle.name.as_deref()),
        (Some(ground.to_string()), Some("Ground"))
    );
    assert_eq!(
        (middle.point, middle.normal),
        (Some([0.0; 3]), Some([0.0, 0.0, 1.0]))
    );
    assert!(middle.distance.is_some_and(|distance| distance > 0.0));
    assert_eq!(cast(None, Some("Ground")).id, None, "past it, nothing");
    // A unit up, over the crate alone.
    let above = cast(Some([50.0, 20.0]), None);
    assert_eq!(above.id, Some(crate_.to_string()));
    let answer = |id| {
        let text = super::world::entity(cx.world(), cx.meshes(), id).expect("an entity");
        serde_json::from_str::<Value>(&text).expect("JSON")
    };
    let touching =
        |id: EntityId, name| json!([{"id": id.to_string(), "name": name, "sensor": false}]);
    assert_eq!(answer(crate_)["touching"], touching(ground, "Ground"));
    assert_eq!(answer(ground)["touching"], touching(crate_, "Crate"));
    let wall = answer(wall);
    assert_eq!(wall["touching"], json!([]));
    assert_eq!(
        wall["box"],
        json!({"min": [18.0, -0.5, 0.0], "max": [22.0, 0.5, 0.0]})
    );
    assert!(answer(crate_).get("box").is_none(), "nothing drawn");
}

/// A pick of a model's node is answered with the model's root, but within the model: the node drawn
/// there, however deep, named as `entities` and `entity` name it. Within another entity or one
/// gone, it gives the root still; an entity in no model gives itself.
#[test]
fn a_pick_within_a_model_is_answered_with_its_node_drawn_there() {
    let (mut api, session) = started("Api Picks Within");
    let mut cx = Context::seeded(12);
    // Nodes: part 0 holds 1 (named) and 2, each drawing; part 3 draws.
    let mut imported = crate::model::tests::imported();
    imported.parts[1].name = Some(std::sync::Arc::from("seat"));
    let (model, _) = crate::model::tests::added(&mut cx, "m", imported);
    let world = cx.world_mut();
    let root = model.spawn(world, Transform::IDENTITY);
    world.insert(root, Name("Model".to_owned())).expect("alive");
    let part = |world: &crate::World, n: u32| {
        let mut next = vec![root];
        while let Some(at) = next.pop() {
            let children: Vec<crate::Entity> = world.children(at).expect("alive").collect();
            let of = |child: &&crate::Entity| {
                world.get::<crate::ModelNode>(**child) == Some(&crate::ModelNode(n))
            };
            if let Some(&node) = children.iter().find(of) {
                return node;
            }
            next.extend(children);
        }
        panic!("no part {n}");
    };
    let (seat, back) = (part(world, 1), part(world, 3));
    assert_eq!(listed(cx.world(), seat).name.as_deref(), Some("seat"));
    let seat_id = cx.world().id(seat).expect("alive");
    let answered = entity(cx.world(), cx.meshes(), seat_id).expect("an entity");
    let answered: Value = serde_json::from_str(&answered).expect("JSON");
    assert_eq!(answered["name"], "seat");
    let world = cx.world_mut();
    let other = world.spawn((Transform::IDENTITY, Name("Other".to_owned())));
    let gone = world.spawn(Transform::IDENTITY);
    let id = |entity| world.id(entity).expect("alive");
    let [root, seat, back, other, gone] = [root, seat, back, other, gone].map(id);
    world
        .despawn(world.entity(gone).expect("alive"))
        .expect("despawned");
    let view = Viewport {
        origin: [0, 0],
        size: [100, 50],
    };
    let mut frame = Extracted::default();
    cx.extract(&mut frame, view);
    let client = move |mut client: Client| {
        let asks = [
            (5.0, None),
            (5.0, Some(root)),
            (5.0, Some(back)),
            (35.0, Some(seat)),
            (5.0, Some(other)),
            (5.0, Some(gone)),
            (65.0, Some(root)),
            (65.0, Some(other)),
        ];
        asks.map(|(x, within)| {
            let within = within.map(|id: EntityId| id.to_string());
            let params = json!({ "at": [x, 4.0], "within": within });
            let picked = client.call_as::<Picked>("pick", params);
            let picked = picked.expect("an answer");
            (picked.id, picked.name)
        })
    };
    // As the renderer answers: the seat draws at columns up to 30, the back's up to 60, the other
    // entity's from there.
    let mut answers = Vec::new();
    let back_frame = |api: &mut Api, cx: &mut Context, stepper: &Stepper| {
        cx.take_answers(&answers);
        api.frame_back(&mut frame, cx, stepper.ticks);
        cx.extract(&mut frame, view);
        answers.clear();
        answers.extend(frame.picks.iter().map(|pick| {
            let id = match pick.pixel[0] {
                0..30 => seat,
                30..60 => back,
                _ => other,
            };
            (pick.tag, id.get())
        }));
    };
    let picked = serving(
        &mut api,
        &mut cx,
        &mut Stack::default(),
        &mut Stepper::default(),
        session,
        client,
        back_frame,
    );
    let answer = |id: EntityId, name: Option<&str>| (Some(id.to_string()), name.map(str::to_owned));
    // The seat by its model name; the back, unnamed, by none.
    let expected = [
        answer(root, Some("Model")),
        answer(seat, Some("seat")),
        answer(seat, Some("seat")),
        answer(back, None),
        answer(root, Some("Model")),
        answer(root, Some("Model")),
        answer(other, Some("Other")),
        answer(other, Some("Other")),
    ];
    assert_eq!(picked, expected);
}

#[test]
fn requests_parse_into_calls_or_are_refused_with_a_code() {
    let request = |method: &str, params: Value| Request {
        id: 1,
        method: method.to_owned(),
        params,
        key: None,
        again: false,
    };
    assert_eq!(parse(&request("step", Value::Null)), Ok(Call::Step(1)));
    assert_eq!(parse(&request("play", Value::Null)), Ok(Call::Play));
    assert_eq!(parse(&request("stop", json!({}))), Ok(Call::Stop));
    let entities = Call::Entities {
        with: Some("Sprite".to_owned()),
        skip: 0,
        take: 100,
        fields: Vec::new(),
    };
    let given = json!({"with": "Sprite"});
    assert_eq!(parse(&request("entities", given)), Ok(entities));
    let given = json!({"fields": ["Transform", "Sprite.rect[1]", "A.b.c"]});
    let Ok(Call::Entities { fields, .. }) = parse(&request("entities", given)) else {
        panic!("fields parse");
    };
    let split = |component: &str, path: &str| (component.to_owned(), path.to_owned());
    let expected = [
        split("Transform", ""),
        split("Sprite", "rect[1]"),
        split("A", "b.c"),
    ];
    assert_eq!(fields, expected);
    for field in ["", ".x", "Transform."] {
        let refused = parse(&request("entities", json!({"fields": [field]})));
        assert!(
            refused.is_err_and(|failure| failure.code == "api.bad_params"),
            "{field:?}"
        );
    }
    let given = json!({"node": "play", "ticks": 2});
    let press = Call::Press {
        node: "play".to_owned(),
        ui: None,
        ticks: Some(2),
    };
    assert_eq!(parse(&request("press", given)), Ok(press));
    for ticks in [0, MOST_STEPS + 1] {
        let refused = parse(&request("press", json!({"node": "play", "ticks": ticks})));
        assert!(refused.is_err_and(|failure| failure.code == "api.bad_params"));
    }
    let assets = Call::Assets {
        kind: Some(AssetKind::Model),
        skip: 2,
        take: 100,
    };
    let given = json!({"kind": "model", "skip": 2});
    assert_eq!(parse(&request("assets", given)), Ok(assets));
    assert_eq!(parse(&request("ui", Value::Null)), Ok(Call::Ui));
    assert_eq!(parse(&request("actions", Value::Null)), Ok(Call::Actions));
    assert_eq!(parse(&request("cells", json!({}))), Ok(Call::Cells));
    // As `dx tools hotpatch` prints it: string keys, the library a path from the root.
    let lib = std::env::temp_dir().join("patch.dll");
    let table = json!({
        "lib": lib,
        "map": {"4096": 8192},
        "aslr_reference": 1,
        "new_base_address": 2,
        "ifunc_count": 0,
    });
    let jump = JumpTable {
        lib,
        map: [(4096, 8192)].into_iter().collect(),
        aslr_reference: 1,
        new_base_address: 2,
        ifunc_count: 0,
    };
    let given = json!({ "table": table });
    assert_eq!(parse(&request("patch", given)), Ok(Call::Patch(jump)));
    let own = Call::Capture {
        path: None,
        at: None,
        own: true,
        show: None,
    };
    assert_eq!(parse(&request("capture", json!({"own": true}))), Ok(own));
    let shown = Call::Capture {
        path: None,
        at: None,
        own: false,
        show: Some(gg_api::Shown::Image("view depth".to_owned())),
    };
    let given = json!({"show": {"image": "view depth"}});
    assert_eq!(parse(&request("capture", given)), Ok(shown));
    let probed = Call::Capture {
        path: None,
        at: Some([3.0, 4.5]),
        own: false,
        show: None,
    };
    assert_eq!(
        parse(&request("capture", json!({"at": [3, 4.5]}))),
        Ok(probed)
    );
    let pick = parse(&request("pick", json!({"at": [3, 4.5]})));
    assert_eq!(pick, Ok(Call::Pick([3.0, 4.5], None)));
    assert_eq!(
        parse(&request("cast", Value::Null)),
        Ok(Call::Cast(None, None))
    );
    let cast = json!({"at": [3, 4.5], "exclude": "Hero"});
    let hero = Some(Subject::Name("Hero".to_owned()));
    assert_eq!(
        parse(&request("cast", cast)),
        Ok(Call::Cast(Some([3.0, 4.5]), hero))
    );
    let projecting = |given| match parse(&request("project", given)) {
        Ok(Call::Project(projecting)) => Some(projecting),
        _ => None,
    };
    let entity = super::parse::Projecting::Entity(Subject::Name("Hero".to_owned()));
    assert_eq!(projecting(json!({"entity": "Hero"})), Some(entity));
    let point = super::parse::Projecting::Point([1.0, 2.0, 3.0]);
    assert_eq!(projecting(json!({"point": [1, 2, 3]})), Some(point));
    let within = json!({"at": [3, 4.5], "within": "00000000000000ff"});
    let id = Subject::Id("00000000000000ff".parse().expect("an ID"));
    assert_eq!(
        parse(&request("pick", within)),
        Ok(Call::Pick([3.0, 4.5], Some(id)))
    );
    let within = json!({"at": [3, 4.5], "within": "Hero"});
    assert_eq!(
        parse(&request("pick", within)),
        Ok(Call::Pick(
            [3.0, 4.5],
            Some(Subject::Name("Hero".to_owned()))
        ))
    );
    let saved = |path: Option<&str>| {
        Ok(Call::SavePrefab {
            id: Subject::Id("00000000000000ff".parse().expect("an ID")),
            path: path.map(PathBuf::from),
        })
    };
    let given = json!({"id": "00000000000000ff"});
    assert_eq!(parse(&request("save_prefab", given)), saved(None));
    let given = json!({"id": "00000000000000ff", "path": "prefabs/Lamp.PREFAB"});
    assert_eq!(
        parse(&request("save_prefab", given)),
        saved(Some("prefabs/Lamp.PREFAB"))
    );
    for (params, code) in [
        (
            json!({"id": "00000000000000ff", "path": "lamp.ron"}),
            "api.bad_params",
        ),
        (json!({"path": "lamp.prefab"}), "api.bad_params"),
    ] {
        let refused = parse(&request("save_prefab", params.clone())).err();
        assert_eq!(
            refused.map(|failure| failure.code),
            Some(code.to_owned()),
            "{params}"
        );
    }
    let scene = |action, path: Option<&str>, discard| {
        let path = path.map(PathBuf::from);
        Ok(Call::Scene(Scene {
            action,
            path,
            discard,
        }))
    };
    for (params, call) in [
        (
            json!({"action": "new"}),
            scene(SceneAction::New, None, false),
        ),
        (
            json!({"action": "new", "discard": true}),
            scene(SceneAction::New, None, true),
        ),
        (
            json!({"action": "open", "path": "scenes/Level2.RON", "discard": true}),
            scene(SceneAction::Open, Some("scenes/Level2.RON"), true),
        ),
        (
            json!({"action": "save"}),
            scene(SceneAction::Save, None, false),
        ),
        (
            json!({"action": "save", "path": "scenes/level2.ron"}),
            scene(SceneAction::Save, Some("scenes/level2.ron"), false),
        ),
    ] {
        assert_eq!(parse(&request("scene", params.clone())), call, "{params}");
    }
    assert_eq!(
        parse(&request("look", Value::Null)),
        Ok(Call::Look(gg_api::Look::default())),
        "reads"
    );
    let viewpoint = |rotation: [f32; 4], projection: Value| json!({"position": [0, 0, 0], "rotation": rotation, "projection": projection});
    let orthographic = json!({"Orthographic": {"height": 2, "near": -1, "far": 1}});
    let turned = viewpoint([0.0, 0.0, 3.0, 4.0], orthographic.clone());
    let Ok(Call::Look(look)) = parse(&request("look", json!({ "viewpoint": turned }))) else {
        panic!("a look");
    };
    let rotation = look.viewpoint.flatten().map(|viewpoint| viewpoint.rotation);
    assert_eq!(rotation, Some([0.0, 0.0, 0.6, 0.8]), "normalized");
    let outline = |ids: &[&str], color: [f32; 4], width: f32| json!({"outline": {"ids": ids, "color": color, "width": width}});
    let id = "5f3a9c0e12b47d68";
    let half = json!({"table": {"lib": "patch.dll"}});
    let refused = parse(&request("patch", half.clone())).err();
    let message = refused.map(|failure| failure.message).unwrap_or_default();
    assert!(message.contains("`map`"), "says what's missing: {message}");
    let extra = json!({"table": table, "lib": "x"});
    let mut relative = table.clone();
    relative["lib"] = json!("patch.dll");
    for (method, params, code) in [
        ("status", json!({"x": 1}), "api.bad_params"),
        ("stop", json!({"x": 1}), "api.bad_params"),
        ("step", json!({"ticks": 0}), "api.bad_params"),
        ("step", json!({"ticks": 10_001}), "api.bad_params"),
        ("step", json!({"tick": 2}), "api.bad_params"),
        ("capture", json!({"path": "relative.png"}), "api.bad_params"),
        ("capture", json!({"bare": true}), "api.bad_params"),
        ("capture", json!({"at": [-1, 0]}), "api.bad_params"),
        ("capture", json!({"at": [0, -0.5]}), "api.bad_params"),
        // An image's name, empty or longer than names are.
        (
            "capture",
            json!({"show": {"image": "x".repeat(gg_api::MOST_IMAGE_NAME + 1)}}),
            "api.bad_params",
        ),
        ("look", json!({"show": {"image": ""}}), "api.bad_params"),
        ("look", json!({"show": "albedo"}), "api.bad_params"),
        (
            "overlays",
            json!({"on": ["a"], "off": ["A"]}),
            "api.bad_params",
        ),
        ("overlays", json!({"on": [""]}), "api.bad_params"),
        (
            "overlays",
            json!({"channel": "path_traced"}),
            "api.bad_params",
        ),
        (
            "overlays",
            json!({"channel": {"image": "view depth"}}),
            "api.bad_params",
        ),
        ("overlays", json!({"shown": true}), "api.bad_params"),
        ("scene", Value::Null, "api.bad_params"),
        ("scene", json!({"action": "close"}), "api.bad_params"),
        ("scene", json!({"action": "open"}), "api.bad_params"),
        (
            "scene",
            json!({"action": "new", "path": "scenes/a.ron"}),
            "api.bad_params",
        ),
        (
            "scene",
            json!({"action": "save", "discard": true}),
            "api.bad_params",
        ),
        (
            "scene",
            json!({"action": "open", "path": "scenes/a.txt"}),
            "api.bad_params",
        ),
        (
            "scene",
            json!({"action": "save", "path": "scenes/ron"}),
            "api.bad_params",
        ),
        ("view", json!({"memory": 3}), "api.bad_params"),
        // A viewpoint with a zero-length rotation, a position that isn't three numbers, or that
        // can't project; an outline of no ID, a non-finite color, or no width.
        (
            "look",
            json!({"viewpoint": viewpoint([0.0; 4], orthographic.clone())}),
            "api.bad_params",
        ),
        (
            "look",
            json!({"viewpoint": {"position": [1, 0], "rotation": [0, 0, 0, 1],
                "projection": orthographic.clone()}}),
            "api.bad_params",
        ),
        (
            "look",
            json!({"viewpoint": viewpoint([0.0, 0.0, 0.0, 1.0],
                json!({"Perspective": {"fov": 4, "near": 0.1}}))}),
            "api.bad_params",
        ),
        (
            "look",
            outline(&[id, "5F3A9C0E12B47D68"], [1.0; 4], 2.0),
            "api.bad_params",
        ),
        (
            "look",
            json!({"outline": {"ids": [id], "color": [1e39, 0, 0, 1], "width": 2}}),
            "api.bad_params",
        ),
        ("look", outline(&[id], [1.0; 4], 0.0), "api.bad_params"),
        ("look", json!({"grid": "xw"}), "api.bad_params"),
        // A render scale under the least, past 1, or not a number.
        ("look", json!({"render_scale": 0.0}), "api.bad_params"),
        ("look", json!({"render_scale": 0.24}), "api.bad_params"),
        ("look", json!({"render_scale": 1.01}), "api.bad_params"),
        ("look", json!({"render_scale": -0.5}), "api.bad_params"),
        ("look", json!({"render_scale": 1e39}), "api.bad_params"),
        ("look", json!({"render_scale": "half"}), "api.bad_params"),
        ("pick", Value::Null, "api.bad_params"),
        ("pick", json!({"at": [1]}), "api.bad_params"),
        ("cast", json!({"at": [1]}), "api.bad_params"),
        ("cast", json!({"within": "Hero"}), "api.bad_params"),
        ("project", Value::Null, "api.bad_params"),
        ("project", json!({"point": [1, 2]}), "api.bad_params"),
        (
            "project",
            json!({"entity": "Hero", "point": [1, 2, 3]}),
            "api.bad_params",
        ),
        ("images", json!({"all": true}), "api.bad_params"),
        ("input", Value::Null, "api.bad_params"),
        (
            "input",
            json!({"events": [{"key": {"key": "W", "pressed": true}}]}),
            "api.bad_params",
        ),
        ("entities", json!({"take": 10_001}), "api.bad_params"),
        ("assets", json!({"take": 10_001}), "api.bad_params"),
        ("assets", json!({"kind": "sound"}), "api.bad_params"),
        ("ui", json!({"name": "menu"}), "api.bad_params"),
        ("patch", Value::Null, "api.bad_params"),
        ("patch", json!({ "table": relative }), "api.bad_params"),
        ("patch", extra, "api.bad_params"),
        ("patch", half, "api.bad_params"),
        ("hello", json!({"token": "x"}), "api.bad_request"),
        ("bogus", Value::Null, "api.unknown_method"),
    ] {
        let refused = parse(&request(method, params)).err();
        let refused = refused.map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some(code), "{method}");
    }
}

/// `METHODS`, which `api.unknown_method` lists, matches the methods there are: none is unknown to a
/// connection, and it holds each one the parser matches or the connection answers itself.
#[test]
fn each_method_is_listed_and_each_one_listed_is_answered() {
    let (_api, session) = started("Api Methods");
    let (mut stream, mut answers) = said_hello(&session);
    for (id, method) in (1..).zip(METHODS) {
        // Parameters no method takes: the parser refuses each rather than passing it to the main
        // loop, which no frame serves here.
        let request = json!({"id": id, "method": method, "params": "none"});
        stream
            .write_all(format!("{request}\n").as_bytes())
            .expect("written");
        let answer = gg_api::read_line(&mut answers).expect("an answer");
        let answer: gg_api::Answer = serde_json::from_str(&answer.expect("a line")).expect("JSON");
        let code = answer.error.map(|failure| failure.code);
        assert_ne!(code.as_deref(), Some("api.unknown_method"), "{method}");
    }
    // The parser's arms (`"step" =>`, `"undo" | "redo" =>`) and the connection's own
    // (`request.method == "ping"`).
    let parser = include_str!("parse.rs").split("fn parse(").nth(1);
    let arms = parser.expect("the parser").lines().filter_map(|line| {
        let (patterns, _) = line.trim_start().split_once(" =>")?;
        let names = patterns.split(" | ");
        let names = names.map(|name| name.strip_prefix('"')?.strip_suffix('"'));
        names.collect::<Option<Vec<_>>>()
    });
    let own = include_str!("connection.rs")
        .split("request.method == \"")
        .skip(1);
    let own = own.map(|rest| rest.split('"').next().expect("a name"));
    let named: Vec<&str> = arms.flatten().chain(own).collect();
    assert!(
        named.len() >= METHODS.len(),
        "the sources no longer name methods as this test reads them: {named:?}"
    );
    let unlisted: Vec<&str> = named
        .into_iter()
        .filter(|name| !METHODS.contains(name))
        .collect();
    assert!(unlisted.is_empty(), "`METHODS` doesn't list {unlisted:?}");
}
