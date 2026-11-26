//! The Stats menu's overlays: numbers over the game's view at its top left, and the log's lines at
//! its bottom left. Clicks pass through them.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use egui::{Align2, Area, Color32, Frame, Id, Order, RichText};
use gg_core::log::{EVENTS, Events};
use tracing::Level;

use super::Overlay;
use crate::input::Input;
use crate::layer::{Sim, SimState};
use crate::memory::{self, Resident};
use crate::net::{NetRole, NetStats};
use crate::perf::{Perf, Sample};
use crate::{Context, Event, Pad, PadButton, Trigger};

/// The overlays at the view's top left, in the order they show.
const CORNER: [Overlay; 7] = [
    Overlay::Fps,
    Overlay::Frame,
    Overlay::Memory,
    Overlay::Rendering,
    Overlay::Simulation,
    Overlay::Input,
    Overlay::Network,
];

/// How often the resident memory is read: the OS is asked each time.
const MEMORY_EVERY: Duration = Duration::from_secs(1);

/// How many of the log's lines show, and how long each takes to fade out.
const LOG_LINES: usize = 5;
const LOG_FADE: Duration = Duration::from_secs(10);

/// The most bytes of log lines read at once, and how many reads a frame makes: the lines past
/// them wait for the next frame.
const LOG_BYTES: usize = 64 << 10;
const LOG_READS: usize = 8;

/// The longest a log line's message shows, in characters.
const LOG_WIDTH: usize = 160;

/// What the Stats overlays keep from frame to frame.
#[derive(Default)]
pub(super) struct Stats {
    memory: Memory,
    pads: Vec<PadState>,
    log: Log,
}

impl Stats {
    /// Notes what the overlays on show of `event`, which goes on to the layers below.
    pub(super) fn event(&mut self, event: &Event<'_>) {
        match *event {
            Event::PadButton {
                pad,
                button,
                pressed,
            } => self.pad(pad).press(button, pressed),
            Event::PadStick { pad, stick, value } => {
                self.pad(pad).sticks[stick as usize] = value.to_array();
            }
            Event::PadTrigger {
                pad,
                trigger,
                value,
            } => self.pad(pad).triggers[trigger as usize] = value,
            _ => {}
        }
    }

    /// Reads what the overlays on show that isn't read as they draw.
    pub(super) fn update(&mut self, cx: &mut Context, dt: Duration) {
        let state = &cx.dev.overlays;
        if state.on(Overlay::Memory) {
            self.memory.refresh(dt, memory::resident);
        } else {
            self.memory = Memory::default();
        }
        let now = Instant::now();
        if state.on(Overlay::Log) {
            self.log.read(&EVENTS, now);
        } else {
            self.log = Log::default();
        }
    }

    /// Shows the overlays on: those at the top left `top` points down from the window's top, and
    /// the log at the bottom left.
    pub(super) fn show(&mut self, cx: &Context, egui: &egui::Context, top: f32) {
        let state = &cx.dev.overlays;
        if CORNER.into_iter().any(|overlay| state.on(overlay)) {
            let area = Area::new(Id::unique("gg debug overlays stats"))
                .anchor(Align2::LEFT_TOP, [8.0, top + 8.0])
                .order(Order::Foreground)
                .interactable(false);
            area.show(egui, |ui| {
                backing().show(ui, |ui| {
                    numbers(ui);
                    for overlay in CORNER.into_iter().filter(|&o| state.on(o)) {
                        match overlay {
                            Overlay::Fps => fps(&cx.perf, ui),
                            Overlay::Frame => frame(&cx.perf, ui),
                            Overlay::Memory => lines(ui, &memory_lines(self.memory.resident)),
                            Overlay::Rendering => rendering(&cx.perf, ui),
                            Overlay::Simulation => {
                                let entities = cx.world().entities(None).map_or(0, |e| e.len());
                                lines(ui, &[simulation(cx.dev.sim, entities)]);
                            }
                            Overlay::Input => {
                                lines(ui, &input_lines(cx.input(), &self.pads));
                            }
                            Overlay::Network => lines(ui, &network_lines(cx.dev.net)),
                            _ => {}
                        }
                    }
                });
            });
        }
        if state.on(Overlay::Log) && !self.log.shown.is_empty() {
            let area = Area::new(Id::unique("gg debug overlays log"))
                .anchor(Align2::LEFT_BOTTOM, [8.0, -8.0])
                .order(Order::Foreground)
                .interactable(false);
            area.show(egui, |ui| {
                backing().show(ui, |ui| {
                    numbers(ui);
                    self.log.show(ui, Instant::now());
                });
            });
        }
    }

    /// The state of `pad`, which its first event adds.
    fn pad(&mut self, pad: Pad) -> &mut PadState {
        let at = self.pads.iter().position(|state| state.pad == pad);
        let at = at.unwrap_or_else(|| {
            self.pads.push(PadState::new(pad));
            self.pads.len() - 1
        });
        &mut self.pads[at]
    }
}

/// The process's resident memory, read at most once a second as frames' times add up.
#[derive(Default)]
struct Memory {
    resident: Resident,
    /// The time since the last read; `None` until the first.
    since: Option<Duration>,
}

impl Memory {
    /// Reads memory with `read` if none has been or [`MEMORY_EVERY`] has passed in the frames'
    /// `dt` since; whether it did.
    fn refresh(&mut self, dt: Duration, read: impl FnOnce() -> Resident) -> bool {
        let since = self.since.map(|since| since + dt);
        if since.is_some_and(|since| since < MEMORY_EVERY) {
            self.since = since;
            return false;
        }
        let _zone = gg_core::zone!();
        self.resident = read();
        self.since = Some(Duration::ZERO);
        true
    }
}

/// What a gamepad's events left held and moved.
struct PadState {
    pad: Pad,
    /// One bit for each [`PadButton`], by its place in the enum.
    held: u32,
    /// Left's and right's X and Y, by [`Stick`](crate::Stick).
    sticks: [[f32; 2]; 2],
    /// Left's and right's travel, by [`Trigger`].
    triggers: [f32; 2],
}

impl PadState {
    fn new(pad: Pad) -> Self {
        Self {
            pad,
            held: 0,
            sticks: [[0.0; 2]; 2],
            triggers: [0.0; 2],
        }
    }

    fn press(&mut self, button: PadButton, pressed: bool) {
        let bit = 1 << button as u32;
        match pressed {
            true => self.held |= bit,
            false => self.held &= !bit,
        }
    }

    fn held(&self) -> impl Iterator<Item = PadButton> {
        let held = self.held;
        PadButton::ALL
            .into_iter()
            .filter(move |&button| held & (1 << button as u32) != 0)
    }
}

/// The last warnings and errors, newest last, each until it has faded.
#[derive(Default)]
struct Log {
    /// Where the next read goes on; `None` until the first, which starts from the log's end.
    next: Option<u64>,
    shown: VecDeque<Entry>,
}

struct Entry {
    at: Instant,
    level: Level,
    code: Option<String>,
    message: String,
}

impl Log {
    /// Takes the warnings and errors `events` kept since the last read, `now`.
    fn read(&mut self, events: &Events, now: Instant) {
        let _zone = gg_core::zone!();
        self.expire(now);
        for _ in 0..LOG_READS {
            let (from, last) = match self.next {
                Some(from) => (Some(from), None),
                None => (None, Some(0)),
            };
            let read = events.read(from, last, Level::WARN, Duration::ZERO, LOG_BYTES);
            self.next = Some(read.next);
            for line in &read.lines {
                self.push(line, now);
            }
            if read.next >= read.end {
                break;
            }
        }
    }

    /// Keeps `line`, an event's JSON, unless it isn't a warning or error.
    fn push(&mut self, line: &str, now: Instant) {
        let Some(mut entry) = Entry::parse(line) else {
            return;
        };
        entry.at = now;
        if self.shown.len() == LOG_LINES {
            self.shown.pop_front();
        }
        self.shown.push_back(entry);
    }

    /// Drops the lines that have faded by `now`.
    fn expire(&mut self, now: Instant) {
        while (self.shown.front())
            .is_some_and(|entry| now.saturating_duration_since(entry.at) >= LOG_FADE)
        {
            self.shown.pop_front();
        }
    }

    fn show(&self, ui: &mut egui::Ui, now: Instant) {
        let width = (self.shown.iter()).map(|entry| entry.code.as_deref().map_or(0, str::len));
        let width = width.max().unwrap_or(0);
        for entry in &self.shown {
            let age = now.saturating_duration_since(entry.at);
            let fade = 1.0 - (age.as_secs_f32() / LOG_FADE.as_secs_f32()).min(1.0);
            let color = match entry.level {
                Level::ERROR => Color32::from_rgb(255, 110, 100),
                _ => Color32::from_rgb(255, 210, 90),
            };
            let code = entry.code.as_deref().unwrap_or("");
            let text = format!("{:<5}  {code:<width$}  {}", entry.level, entry.message);
            ui.label(RichText::new(text).color(color.gamma_multiply(fade)));
        }
    }
}

impl Entry {
    /// What a JSON line of the log says: its level, `code` and `message`. `None` for a line that
    /// isn't JSON or names no level.
    fn parse(line: &str) -> Option<Self> {
        let event: serde_json::Value = serde_json::from_str(line).ok()?;
        let level = event.get("level")?.as_str()?.parse().ok()?;
        let text = |field| event.get(field).and_then(|value| value.as_str());
        let message = text("message").unwrap_or_default();
        let message = message.lines().next().unwrap_or_default();
        Some(Self {
            at: Instant::now(),
            level,
            code: text("code").map(str::to_owned),
            message: message.chars().take(LOG_WIDTH).collect(),
        })
    }
}

/// The translucent black the numbers show on.
fn backing() -> Frame {
    Frame::NONE
        .fill(Color32::from_black_alpha(170))
        .corner_radius(4)
        .inner_margin(6)
}

/// Light monospaced text, so numbers keep their places as they change.
fn numbers(ui: &mut egui::Ui) {
    ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
    ui.visuals_mut().override_text_color = Some(Color32::from_gray(230));
    ui.spacing_mut().item_spacing.y = 2.0;
}

/// The frames of the last second, newest first, as the Performance panel's text sums them: how
/// many, and their sum. `None` before one is measured.
fn last_second(perf: &Perf) -> Option<(u32, Sample)> {
    let mut frames = 0;
    let mut sum = Sample::default();
    for sample in perf.recent() {
        if sum.frame >= 1_000_000_000 {
            break;
        }
        frames += 1;
        sum.frame += sample.frame;
        sum.ticks += sample.ticks;
        sum.update += sample.update;
        sum.ui += sample.ui;
        sum.handoff += sample.handoff;
        sum.limit += sample.limit;
        sum.render += sample.render;
        sum.wait += sample.wait;
        sum.gpu += sample.gpu;
    }
    (frames > 0).then_some((frames, sum))
}

/// Milliseconds of a mean of `frames` frames' nanoseconds `sum`.
fn mean_ms(sum: u64, frames: u32) -> f64 {
    sum as f64 / f64::from(frames) / 1e6
}

fn fps(perf: &Perf, ui: &mut egui::Ui) {
    let Some((frames, sum)) = last_second(perf) else {
        ui.label("fps     measuring");
        return;
    };
    let mean = mean_ms(sum.frame, frames);
    ui.label(format!("fps     {:>6.0}  {mean:>6.2} ms", 1000.0 / mean));
}

fn frame(perf: &Perf, ui: &mut egui::Ui) {
    let Some((frames, sum)) = last_second(perf) else {
        return;
    };
    let ms = |part: u64| mean_ms(part, frames);
    ui.label(format!(
        "cpu     ticks {:.2}  update {:.2}  ui {:.2}  handoff {:.2}  limit {:.2}",
        ms(sum.ticks),
        ms(sum.update),
        ms(sum.ui),
        ms(sum.handoff),
        ms(sum.limit)
    ));
    ui.label(format!(
        "render  {:.2}  waits {:.2}  gpu {:.2}",
        ms(sum.render),
        ms(sum.wait),
        ms(sum.gpu)
    ));
}

fn rendering(perf: &Perf, ui: &mut egui::Ui) {
    let Some(last) = perf.recent().next() else {
        return;
    };
    ui.label(format!(
        "draws   {}  instances {}  triangles {}",
        last.draws, last.instances, last.triangles
    ));
}

/// Each of `texts` as a label.
fn lines(ui: &mut egui::Ui, texts: &[String]) {
    for text in texts {
        ui.label(text);
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / f64::from(1 << 20))
}

fn memory_lines(resident: Resident) -> [String; 1] {
    let size = |bytes: Option<u64>| bytes.map_or("n/a".to_owned(), mib);
    [format!(
        "memory  {}  peak {}",
        size(resident.now),
        size(resident.peak)
    )]
}

fn simulation(sim: Sim, entities: usize) -> String {
    let state = match sim.state {
        SimState::Running => "running",
        SimState::Paused => "paused",
        SimState::Stopped => "stopped",
    };
    format!("sim     tick {}  {state}  entities {entities}", sim.tick)
}

/// Bytes as KiB or MiB.
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..0x10_0000 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => mib(bytes),
    }
}

fn network_lines(net: Option<NetStats>) -> Vec<String> {
    let Some(net) = net else {
        return vec!["net     not in a session".to_owned()];
    };
    let role = match net.role {
        NetRole::Host => "host".to_owned(),
        NetRole::Joiner => "joiner".to_owned(),
        NetRole::Server => format!("server, {} clients", net.links),
        NetRole::Client => "client".to_owned(),
    };
    let mut head = format!("net     {role}");
    match (net.trip, net.deviation) {
        (Some(trip), Some(deviation)) => {
            let [trip, deviation] = [trip, deviation].map(|seconds| seconds * 1000.0);
            let _ = write!(head, "  rtt {trip:.1} ms +-{deviation:.1}");
        }
        (Some(trip), None) => {
            let _ = write!(head, "  rtt {:.1} ms", trip * 1000.0);
        }
        _ => head.push_str("  rtt n/a"),
    }
    if let Some(delay) = net.delay {
        let _ = write!(head, "  delay {delay} ticks");
        if net.window > 0 {
            let _ = write!(head, "  window {}", net.window);
        }
    }
    let mut tail = format!(
        "        sent {}  taken {}",
        size(net.bytes[0]),
        size(net.bytes[1])
    );
    let [rollbacks, reran, most] = net.rolled;
    let _ = write!(tail, "  waits {}", net.waits);
    if net.window > 0 || matches!(net.role, NetRole::Client) {
        let _ = write!(tail, "  rollbacks {rollbacks} (reran {reran}, most {most})");
    }
    vec![head, tail]
}

fn input_lines(input: &Input, pads: &[PadState]) -> Vec<String> {
    let cursor = match input.cursor() {
        Some([x, y]) => format!("{x:.0}, {y:.0}"),
        None => "outside".to_owned(),
    };
    let mut names: Vec<String> = input.keys_down().map(|key| format!("{key:?}")).collect();
    names.sort();
    let mut buttons: Vec<String> = input.buttons_down().map(|b| format!("{b:?}")).collect();
    buttons.sort();
    let list = |names: Vec<String>| match names.is_empty() {
        true => "-".to_owned(),
        false => names.join(" "),
    };
    let mut out = vec![
        format!("cursor  {cursor}"),
        format!("keys    {}", list(names)),
        format!("mouse   {}", list(buttons)),
    ];
    for state in pads {
        let held = state.held().map(|button| format!("{button:?}")).collect();
        let [left, right] = state.sticks;
        out.push(format!(
            "pad {:<3} {}  L {:+.2},{:+.2}  R {:+.2},{:+.2}  LT {:.2}  RT {:.2}",
            state.pad.0,
            list(held),
            left[0],
            left[1],
            right[0],
            right[1],
            state.triggers[Trigger::Left as usize],
            state.triggers[Trigger::Right as usize],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, MouseButton, Stick};

    const WARNING: &str = r#"{"timestamp":"2026-11-25T10:00:00Z","level":"WARN","code":"net.link","message":"Can't send the peer a packet","target":"gg_engine::net"}"#;

    #[test]
    fn a_warning_line_gives_its_level_code_and_message() {
        let entry = Entry::parse(WARNING).expect("a warning");
        assert_eq!(entry.level, Level::WARN);
        assert_eq!(entry.code.as_deref(), Some("net.link"));
        assert_eq!(entry.message, "Can't send the peer a packet");

        let error = r#"{"level":"ERROR","message":"first\nsecond"}"#;
        let entry = Entry::parse(error).expect("an error");
        assert_eq!((entry.level, entry.code), (Level::ERROR, None));
        assert_eq!(entry.message, "first");
        assert!(Entry::parse("not json").is_none());
        assert!(Entry::parse(r#"{"message":"no level"}"#).is_none());
    }

    #[test]
    fn the_log_reads_each_event_once_keeps_five_and_lets_them_fade() {
        let events = Events::new();
        let line = |n: u32| format!(r#"{{"level":"WARN","code":"c.{n}","message":"m{n}"}}"#);
        events.push(Level::WARN, &line(0));
        let (mut log, start) = (Log::default(), Instant::now());
        // The first read starts from the log's end: older events don't show.
        log.read(&events, start);
        assert!(log.shown.is_empty());
        for n in 1..=7 {
            events.push(Level::WARN, &line(n));
            events.push(Level::INFO, &line(100 + n));
        }
        log.read(&events, start);
        log.read(&events, start);
        let codes: Vec<_> = log.shown.iter().filter_map(|e| e.code.as_deref()).collect();
        assert_eq!(codes, ["c.3", "c.4", "c.5", "c.6", "c.7"]);

        log.read(&events, start + LOG_FADE - Duration::from_millis(1));
        assert_eq!(log.shown.len(), 5);
        log.read(&events, start + LOG_FADE);
        assert!(log.shown.is_empty());
    }

    #[test]
    fn memory_is_read_at_most_once_a_second() {
        let mut memory = Memory::default();
        let mut reads = 0;
        let mut frame = |memory: &mut Memory, dt: Duration| {
            memory.refresh(dt, || {
                reads += 1;
                Resident {
                    now: Some(reads),
                    peak: Some(reads * 2),
                }
            })
        };
        let ms = Duration::from_millis;
        assert!(frame(&mut memory, ms(16)), "the first frame reads");
        for _ in 0..61 {
            assert!(!frame(&mut memory, ms(16)));
        }
        // 61 more frames of 16 ms make 976 ms; the next makes 992, and the one after 1008.
        assert!(!frame(&mut memory, ms(16)));
        assert_eq!(memory.resident.now, Some(1));
        assert!(frame(&mut memory, ms(16)));
        assert_eq!(memory.resident.now, Some(2));
        assert_eq!(
            memory_lines(memory.resident)[0],
            "memory  0.0 MiB  peak 0.0 MiB"
        );
        assert_eq!(
            memory_lines(Resident::default())[0],
            "memory  n/a  peak n/a"
        );
    }

    #[test]
    fn a_pads_held_buttons_sticks_and_triggers_follow_its_events() {
        let mut stats = Stats::default();
        let pad = Pad(2);
        let button = |button, pressed| Event::PadButton {
            pad,
            button,
            pressed,
        };
        stats.event(&button(PadButton::South, true));
        stats.event(&button(PadButton::DPadRight, true));
        stats.event(&button(PadButton::South, false));
        stats.event(&Event::PadStick {
            pad,
            stick: Stick::Right,
            value: gg_core::math::Vec2::new(0.5, -1.0),
        });
        stats.event(&Event::PadTrigger {
            pad,
            trigger: Trigger::Left,
            value: 0.25,
        });
        stats.event(&Event::PadButton {
            pad: Pad(0),
            button: PadButton::Start,
            pressed: true,
        });
        assert_eq!(stats.pads.len(), 2);
        let state = &stats.pads[0];
        assert_eq!(state.held().collect::<Vec<_>>(), [PadButton::DPadRight]);
        assert_eq!(state.sticks, [[0.0, 0.0], [0.5, -1.0]]);
        assert_eq!(state.triggers, [0.25, 0.0]);

        let lines = input_lines(&Input::default(), &stats.pads);
        assert_eq!(lines[0], "cursor  outside");
        assert_eq!(
            lines[3],
            "pad 2   DPadRight  L +0.00,+0.00  R +0.50,-1.00  LT 0.25  RT 0.00"
        );
        assert!(lines[4].starts_with("pad 0   Start "));
    }

    #[test]
    fn input_lines_name_the_cursor_keys_and_buttons_held() {
        let mut input = Input::default();
        input.record(&Event::CursorMoved { x: 10.4, y: 20.6 });
        for key in [Key::KeyW, Key::ShiftLeft] {
            input.record(&Event::Key {
                key,
                pressed: true,
                repeat: false,
            });
        }
        input.record(&Event::MouseButton {
            button: MouseButton::Right,
            pressed: true,
        });
        assert_eq!(
            input_lines(&input, &[]),
            ["cursor  10, 21", "keys    KeyW ShiftLeft", "mouse   Right"]
        );
        assert_eq!(input_lines(&Input::default(), &[])[1], "keys    -");
    }

    #[test]
    fn the_simulations_line_says_its_tick_and_state() {
        let sim = Sim {
            tick: 42,
            state: SimState::Paused,
        };
        assert_eq!(simulation(sim, 7), "sim     tick 42  paused  entities 7");
    }

    #[test]
    fn the_network_lines_say_without_a_session_and_show_a_peers_numbers() {
        assert_eq!(network_lines(None), ["net     not in a session"]);
        let net = NetStats {
            role: NetRole::Host,
            trip: Some(0.0123),
            deviation: Some(0.0011),
            delay: Some(3),
            window: 8,
            waits: 4,
            rolled: [5, 12, 3],
            bytes: [1536, 3 << 20],
            links: 1,
        };
        assert_eq!(
            network_lines(Some(net)),
            [
                "net     host  rtt 12.3 ms +-1.1  delay 3 ticks  window 8",
                "        sent 1.5 KiB  taken 3.0 MiB  waits 4  rollbacks 5 (reran 12, most 3)",
            ]
        );
    }
}
