//! A game's UI from its file, drawn by a run embedded in a tool, as the editor embeds a game. Pins
//! that:
//! - its capture matches its reference image, within what GPUs differ by in rasterizing and
//!   filtering;
//! - the tool API's `ui` lists its nodes where the capture shows them;
//! - a click at a button's pixels through `input` presses it, which the layer shows in the UI's
//!   text.
//!
//! It needs a GPU and the validation layer, as the GPU tests do (lavapipe serves), and runs in a
//! process of its own, since a process starts the log once. `GG_UPDATE_REFERENCES=1` writes the
//! capture as the new reference instead of comparing.
#![cfg(feature = "dev")]

use std::fs::{self, File};
use std::io::BufWriter;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use gg_api::{Captured, Client, Session, UiList, UiNode, UiValue};
use gg_engine::{App, Context, Event, GameUi, Layer, UiEvent};
use serde_json::{Value, json};

/// How long to wait before failing rather than hanging: a debug build under validation starts
/// slowly.
const DEADLINE: Duration = Duration::from_secs(60);

/// The view's size the client asks for: a third of a pixel to each of the canvas's units.
const SIZE: [u32; 2] = [640, 360];

/// A menu (first button focused, last disabled), a row of texts along the view's top, a corner
/// panel with a textured image and a text saying what was pressed, and a panel of every widget
/// at the right, its list holding more than it shows (it clips), a bar as wide as its first
/// under its scroll bar.
const MENU: &str = r#"(
    focus: "play",
    nodes: [
        Panel(
            name: "menu",
            color: (0.02, 0.03, 0.06, 0.92),
            radius: 24.0,
            padding: (48.0, 40.0),
            children: [
                Column(
                    gap: 16.0,
                    children: [
                        Text(name: "title", text: "MENU", size: 72.0),
                        Button(
                            name: "play",
                            text: "PLAY",
                            width: Fixed(420.0),
                            height: Fixed(80.0),
                        ),
                        Button(
                            name: "options",
                            text: "OPTIONS",
                            width: Fixed(420.0),
                            height: Fixed(80.0),
                        ),
                        Button(
                            name: "credits",
                            text: "CREDITS",
                            width: Fixed(420.0),
                            height: Fixed(80.0),
                            disabled: true,
                        ),
                    ],
                ),
            ],
        ),
        Row(
            name: "hud",
            anchor: (Center, Start),
            width: Fill(1.0),
            margin: (32.0, 24.0),
            children: [
                Text(name: "score", text: "SCORE 120", size: 40.0),
                Stack(width: Fill(1.0)),
                Text(name: "clock", text: "00:00", size: 40.0),
            ],
        ),
        Panel(
            name: "corner",
            anchor: (Start, End),
            margin: (32.0, 32.0),
            padding: (16.0, 16.0),
            color: (1.0, 1.0, 1.0, 0.1),
            radius: 12.0,
            children: [
                Row(
                    gap: 16.0,
                    children: [
                        Image(name: "logo", texture: "checker.png", width: Fixed(64.0)),
                        Text(name: "status", text: "READY", size: 32.0),
                    ],
                ),
            ],
        ),
        Panel(
            name: "widgets",
            anchor: (End, Center),
            margin: (32.0, 0.0),
            padding: (32.0, 32.0),
            color: (0.02, 0.03, 0.06, 0.92),
            radius: 24.0,
            children: [
                Column(
                    gap: 12.0,
                    children: [
                        Slider(name: "volume", value: 0.7, width: Fixed(320.0)),
                        Toggle(name: "sound", text: "SOUND", on: true, width: Fixed(320.0)),
                        Dropdown(
                            name: "difficulty",
                            options: ["EASY", "NORMAL", "HARD"],
                            choice: 1,
                            width: Fixed(320.0),
                        ),
                        Field(name: "player", hint: "NAME", width: Fixed(320.0)),
                        List(
                            name: "lines",
                            width: Fixed(320.0),
                            height: Fixed(120.0),
                            color: (1.0, 1.0, 1.0, 0.06),
                            children: [
                                Panel(
                                    width: Fill(1.0),
                                    height: Fixed(36.0),
                                    color: (0.3, 0.32, 0.4, 1.0),
                                ),
                                Text(text: "FIRST LINE", size: 40.0),
                                Text(text: "SECOND LINE", size: 40.0),
                                Text(text: "THIRD LINE", size: 40.0),
                                Text(text: "FOURTH LINE", size: 40.0),
                            ],
                        ),
                    ],
                ),
            ],
        ),
    ],
)
"#;

/// Draws the menu each frame; its status says which button was pressed or widget changed.
struct Menu {
    dir: PathBuf,
    ui: Option<GameUi>,
}

impl Layer for Menu {
    fn load(&mut self, cx: &mut Context) {
        match cx.add_ui("menu", self.dir.join("menu.ui")) {
            Ok(ui) => self.ui = Some(ui),
            Err(error) => cx.fail(error),
        }
    }

    fn update(&mut self, cx: &mut Context, _: Duration) {
        let ui = self.ui.expect("loaded");
        ui.set_text(cx, "clock", "12:34").expect("a text");
        ui.draw(cx);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> std::ops::ControlFlow<()> {
        let ui = self.ui.expect("loaded");
        let status = match ui.event(cx, event) {
            Some(UiEvent::Pressed(name)) => format!("PRESSED {}", name.to_uppercase()),
            Some(UiEvent::Changed(name, _)) => format!("CHANGED {}", name.to_uppercase()),
            _ => return std::ops::ControlFlow::Continue(()),
        };
        ui.set_text(cx, "status", &status).expect("a text");
        std::ops::ControlFlow::Continue(())
    }
}

#[test]
fn a_ui_draws_as_its_reference_and_lists_where_it_draws() {
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("skipped: GG_SKIP_GPU_TESTS is set");
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui");
    fs::create_dir_all(&dir).expect("the test's directory is made");
    fs::write(dir.join("menu.ui"), MENU).expect("the UI's file is written");
    // 8 by 8 texels, in squares of 2, orange and white.
    write_png(&dir.join("checker.png"), [8, 8], |x, y| {
        match (x / 2 + y / 2) % 2 {
            0 => [240, 140, 40, 255],
            _ => [250, 250, 250, 255],
        }
    });
    // The run is on this thread: layers stay on the thread they're made on.
    let client = thread::spawn(|| {
        let mut client = Client::connect(&session(), "test", DEADLINE).expect("it connects");
        let checked = panic::catch_unwind(AssertUnwindSafe(|| check(&mut client)));
        // Whatever the checks found, end the run.
        let quit = client.call("quit", Value::Null);
        if let Err(panic) = checked {
            panic::resume_unwind(panic);
        }
        quit.expect("the run asked to end");
    });
    let menu = Menu {
        dir: dir.clone(),
        ui: None,
    };
    let app = App::new("UI Test").root(dir).layer(menu);
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

fn session() -> Session {
    let exe = std::env::current_exe().expect("the test's executable");
    let dir = Session::dir(exe.parent().expect("its directory"));
    let file = dir.join(Session::file_name("UI Test", std::process::id()));
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < DEADLINE, "the run never served the API");
        let text = fs::read(&file).ok();
        if let Some(session) = text.and_then(|text| serde_json::from_slice(&text).ok()) {
            return session;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn node<'l>(list: &'l UiList, name: &str) -> &'l UiNode {
    let found = (list.uis[0].nodes.iter()).find(|node| node.name.as_deref() == Some(name));
    found.unwrap_or_else(|| panic!("a node `{name}`: {list:?}"))
}

/// Publishes the view at [`SIZE`], compares its capture with the reference, checks the tool API's
/// `ui` against the capture, and clicks a button.
fn check(client: &mut Client) {
    let mut reader = gg_api::view::Reader::create(SIZE[0], SIZE[1]).expect("the view's memory");
    let memory = json!({"memory": reader.name()});
    client.call("view", memory).expect("the view published");
    reader.unlink();
    // Wait for a frame at the size asked for; the capture follows it.
    let start = Instant::now();
    while reader.take().is_none_or(|frame| frame.size != SIZE) {
        assert!(
            start.elapsed() < DEADLINE,
            "no frame of the size asked for came"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui.capture.png");
    let captured: Captured = client
        .call_as("capture", json!({"own": true, "path": path}))
        .expect("a capture");
    assert_eq!([captured.width, captured.height], SIZE);
    assert!(!captured.streaming, "everything streamed");
    let (size, pixels) = read_png(&path);
    assert_eq!(size, SIZE);
    compare("ui", SIZE, &pixels);

    // The nodes sit where the capture shows them, a third of a pixel to a unit.
    let list: UiList = client.call_as("ui", Value::Null).expect("the UIs");
    assert_eq!(list.view, SIZE);
    assert!(list.uis[0].drawn && list.uis[0].input);
    assert_eq!(list.uis[0].focus.as_deref(), Some("play"));
    let play = node(&list, "play");
    assert_eq!(play.rect[2..], [420.0, 80.0]);
    for (units, pixels) in play.rect.iter().zip(play.pixels) {
        assert!((units / 3.0 - pixels).abs() < 1e-3, "{play:?}");
    }
    assert!(play.focused && !node(&list, "credits").focused);
    assert!(node(&list, "credits").disabled);
    let corner = node(&list, "corner");
    assert_eq!(corner.rect[0], 32.0, "{corner:?}");
    assert!(
        (corner.rect[1] + corner.rect[3] - 1048.0).abs() < 1e-3,
        "{corner:?}"
    );
    let logo = node(&list, "logo");
    assert_eq!(logo.rect[2..], [64.0, 64.0], "its texture's shape");
    assert_eq!(node(&list, "clock").text.as_deref(), Some("12:34"));
    let hud = node(&list, "hud");
    assert_eq!(hud.rect, [32.0, 24.0, 1920.0 - 64.0, hud.rect[3]]);
    // The menu's middle at the view's.
    let menu = node(&list, "menu");
    let middle = [
        menu.pixels[0] + menu.pixels[2] / 2.0,
        menu.pixels[1] + menu.pixels[3] / 2.0,
    ];
    assert!((middle[0] - 320.0).abs() < 1e-3 && (middle[1] - 180.0).abs() < 1e-3);

    // A click at the options' middle focuses it and presses it on release.
    let options = node(&list, "options").pixels;
    let at = [options[0] + options[2] / 2.0, options[1] + options[3] / 2.0];
    let button = |pressed| json!({"button": {"button": "left", "pressed": pressed}});
    let events = json!({"events": [{"cursor": at}, button(true), button(false)]});
    client.call("input", events).expect("the input");
    let list: UiList = client.call_as("ui", Value::Null).expect("the UIs");
    assert_eq!(list.uis[0].focus.as_deref(), Some("options"));
    let status = node(&list, "status").text.as_deref();
    assert_eq!(status, Some("PRESSED OPTIONS"));

    // The widgets list what they hold; the list's last line lies past its bottom, clipped.
    assert_eq!(node(&list, "volume").value, Some(UiValue::Number(0.7)));
    assert_eq!(node(&list, "sound").value, Some(UiValue::On(true)));
    let difficulty = node(&list, "difficulty");
    assert_eq!(difficulty.value, Some(UiValue::Choice(1)));
    assert_eq!(difficulty.text.as_deref(), Some("NORMAL"));
    let lines = node(&list, "lines").rect;
    let fourth = list.uis[0].nodes.last().expect("the fourth line").rect;
    assert!(fourth[1] > lines[1] + lines[3], "{fourth:?} past {lines:?}");
    // A click flips the toggle; a click on the text field takes typed text.
    let middle = |name: &str| {
        let pixels = node(&list, name).pixels;
        [pixels[0] + pixels[2] / 2.0, pixels[1] + pixels[3] / 2.0]
    };
    let events = json!({"events": [
        {"cursor": middle("sound")}, button(true), button(false),
        {"cursor": middle("player")}, button(true), button(false), {"text": "ADA"},
    ]});
    client.call("input", events).expect("the input");
    let list: UiList = client.call_as("ui", Value::Null).expect("the UIs");
    assert_eq!(node(&list, "sound").value, Some(UiValue::On(false)));
    assert_eq!(node(&list, "player").text.as_deref(), Some("ADA"));
    assert_eq!(list.uis[0].focus.as_deref(), Some("player"));
    let status = node(&list, "status").text.as_deref();
    assert_eq!(status, Some("CHANGED PLAYER"));
}

fn write_png(path: &Path, size: [u32; 2], pixel: impl Fn(u32, u32) -> [u8; 4]) {
    let file = File::create(path).expect("the PNG is made");
    let mut encoder = png::Encoder::new(BufWriter::new(file), size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    let mut writer = encoder.write_header().expect("the PNG's header is written");
    writer
        .write_image_data(&pixels)
        .expect("the PNG's pixels are written");
}

fn read_png(path: &Path) -> ([u32; 2], Vec<u8>) {
    let file = File::open(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("its pixels");
    pixels.truncate(info.buffer_size());
    ([info.width, info.height], pixels)
}

/// Compares RGBA8 `pixels` with the reference image `name`, allowing for GPU differences in
/// rasterizing and filtering, as the renderer's GPU tests do: a pixel matches if no channel is off
/// by more than 16, and up to one pixel in a hundred may not match. A mismatch writes the pixels
/// beside the test's directory.
fn compare(name: &str, size: [u32; 2], pixels: &[u8]) {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/references")
        .join(format!("{name}.png"));
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        fs::create_dir_all(reference.parent().expect("a folder")).expect("made");
        write_png(&reference, size, |x, y| {
            let at = ((y * size[0] + x) * 4) as usize;
            pixels[at..at + 4].try_into().expect("a pixel")
        });
        println!("wrote {}", reference.display());
        return;
    }
    let (reference_size, expected) = read_png(&reference);
    assert_eq!(
        reference_size, size,
        "{name}'s reference is a different size"
    );
    let mismatched = (pixels.as_chunks::<4>().0.iter())
        .zip(expected.as_chunks::<4>().0)
        .filter(|(got, want)| got.iter().zip(*want).any(|(&a, &b)| a.abs_diff(b) > 16))
        .count();
    let allowed = (size[0] * size[1]) as usize / 100;
    if mismatched > allowed {
        let actual = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
        write_png(&actual, size, |x, y| {
            let at = ((y * size[0] + x) * 4) as usize;
            pixels[at..at + 4].try_into().expect("a pixel")
        });
        panic!(
            "{mismatched} pixels differ from {name}'s reference, more than the {allowed} allowed; \
             the output is at {}",
            actual.display()
        );
    }
}
