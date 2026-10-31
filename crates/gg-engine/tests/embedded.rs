//! A run embedded in a tool, as the editor embeds a game: once a client has its view published to
//! memory the client made, a frame comes as the client takes the last, at the size the client asks
//! for, with the viewpoint and the look it drew with, and the client's input reaches the layers,
//! until it asks the run to end. It renders, so it needs a GPU and the validation layer, as the GPU
//! tests do; and it runs in a process of its own, as a run starts the log, which a process starts
//! once.
#![cfg(feature = "dev")]

use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use gg_api::view::{Frame, Reader};
use gg_api::{Captured, Client, Looked, Session};
use gg_engine::{App, Context, Key, Layer};
use serde_json::{Value, json};

/// How long the test waits for what should come soon, past which it fails rather than hangs: a
/// debug build under validation takes a while to start.
const DEADLINE: Duration = Duration::from_secs(60);

/// Clears the view to green, or to red while W is held.
struct Lit;

impl Layer for Lit {
    fn update(&mut self, cx: &mut Context, _: Duration) {
        let held = cx.input().key_down(Key::KeyW);
        cx.set_clear_color(if held { RED } else { GREEN });
    }
}

const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// Takes frames until one comes that `wanted` says is the one, and returns it with its pixels.
fn take_until(reader: &mut Reader, wanted: impl Fn(&Frame, &[u8]) -> bool) -> (Frame, Vec<u8>) {
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < DEADLINE, "no such frame came");
        if let Some(frame) = reader.take() {
            let mut pixels = vec![0; frame.bytes()];
            reader.read(&mut pixels);
            if wanted(&frame, &pixels) {
                return (frame, pixels);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Whether every pixel of `pixels` is `color`.
fn all(pixels: &[u8], color: [u8; 4]) -> bool {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|&pixel| pixel == color)
}

#[test]
fn an_embedded_run_publishes_its_view_as_its_client_takes_it() {
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    // The run's on this thread, as its layers stay on the thread they're made on.
    let client = thread::spawn(|| {
        let mut client = Client::connect(&session(), "test", DEADLINE).expect("it connects");
        let checked = panic::catch_unwind(AssertUnwindSafe(|| check(&mut client)));
        // Whatever the checks found, so that the run ends.
        let quit = client.call("quit", Value::Null);
        if let Err(panic) = checked {
            panic::resume_unwind(panic);
        }
        quit.expect("the run asked to end");
    });
    let app = App::new("Embedded Test").layer(Lit);
    let status = gg_engine::run_with(app, ["--embedded"]);
    if let Err(panic) = client.join() {
        panic::resume_unwind(panic);
    }
    assert_eq!(
        status,
        ExitCode::SUCCESS,
        "the run ended cleanly, without validation messages"
    );
}

/// The session of the run, once its file is written.
fn session() -> Session {
    let exe = std::env::current_exe().expect("the test's executable");
    let dir = Session::dir(exe.parent().expect("its directory"));
    let file = dir.join(Session::file_name("Embedded Test", std::process::id()));
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < DEADLINE, "the run never served the API");
        let text = std::fs::read(&file).ok();
        if let Some(session) = text.and_then(|text| serde_json::from_slice(&text).ok()) {
            return session;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Has the view published to memory of its own, and checks the frames it takes from it: green, at
/// the size it asks, the next only once it has taken the last, and red once it holds W.
fn check(client: &mut Client) {
    let mut reader = Reader::create(64, 32).expect("memory for the view");
    let memory = json!({"memory": reader.name()});
    client.call("view", memory).expect("the view published");
    reader.unlink();

    let (frame, pixels) = take_until(&mut reader, |_, _| true);
    assert_eq!(frame.size, [64, 32], "the size the client asked for");
    assert!(all(&pixels, [0, 255, 0, 255]), "the view cleared to green");
    // Paced: the run waits for the client to take a frame before it renders another, for a while.
    thread::sleep(Duration::from_millis(50));
    let published = reader.published();
    assert!(published <= frame.number + 1, "{published} published");

    assert!(reader.ask([32, 16]));
    let (frame, _) = take_until(&mut reader, |frame, _| frame.size == [32, 16]);
    assert!(frame.started <= frame.published);

    // The frames drawn with a look carry its viewpoint and its revision; and a capture of the
    // game's own view, whose frame isn't published, comes all the same.
    let viewpoint = json!({"position": [0, 0, 5], "rotation": [0, 0, 0, 1],
        "projection": {"Perspective": {"fov": 1.0, "near": 0.1}}});
    let looked: Looked =
        (client.call_as("look", json!({ "viewpoint": viewpoint }))).expect("the look");
    let (frame, _) = take_until(&mut reader, |frame, _| frame.look == looked.revision);
    assert_eq!(Some(frame.viewpoint), looked.viewpoint);
    let path = std::env::temp_dir().join(format!("gg-embedded-{}.png", std::process::id()));
    let own = json!({"own": true, "path": path});
    let captured: Captured = client.call_as("capture", own).expect("a capture");
    assert_eq!([captured.width, captured.height], [32, 16]);
    let _ = std::fs::remove_file(path);

    let held = json!({"events": [{"key": {"key": "KeyW", "pressed": true}}]});
    client.call("input", held).expect("the input");
    take_until(&mut reader, |_, pixels| all(pixels, [255, 0, 0, 255]));
}
