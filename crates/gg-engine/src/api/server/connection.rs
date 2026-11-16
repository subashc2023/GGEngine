//! A connection's threads: its hello and grant, the requests it answers itself, and its answers.

use std::io::{self, BufRead as _, BufReader, Write as _};
use std::mem;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::{Failure, Hello, Pulse, Request};
use serde::Serialize;
use serde_json::Value;
use tracing::{Level, debug, info, warn};

use super::parse::{Call, bad_params, parse, take};
use super::{Incoming, Shared, failure};
use crate::api::Waiting;
use crate::grant::{Capability, Grant};

/// The client a connection whose `hello` names none is, the author of its edits.
const UNNAMED: &str = "api";

/// How long a connection has to say hello, its whole `hello` come, before it's dropped.
pub(super) const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The most connections saying hello at once, past which more are refused as they come: any local
/// process may open connections that never say it, each holding a thread until it's dropped.
pub(super) const MOST_SAYING_HELLO: usize = 8;

/// The longest a `logs` waits for an event to come, or a `debug` for a script to stop, in
/// milliseconds.
const MOST_WAIT_MS: u64 = 10_000;

/// The levels of `tracing`, in the order [`gg_api::LEVELS`] names them.
const LEVELS: [Level; 5] = [
    Level::ERROR,
    Level::WARN,
    Level::INFO,
    Level::DEBUG,
    Level::TRACE,
];

/// Where a request's answer goes: its connection's writer, which drops it once the connection has
/// gone; and the request's place among those its connection has in flight.
pub(super) struct Reply {
    pub(super) id: u64,
    pub(super) to: Sender<Answer>,
    pub(super) flight: Flight,
}

/// An answer's line, for its connection's writer, with its request's place among those in flight,
/// which goes once the line is written.
pub(super) type Answer = (String, Flight);

impl Reply {
    pub(super) fn send(self, answer: Result<String, Failure>) {
        let line = gg_api::answer_line(self.id, answer.as_deref());
        let _ = self.to.send((line, self.flight));
    }

    pub(super) fn result(self, result: &impl Serialize) {
        self.send(Ok(
            serde_json::to_string(result).expect("a result serializes")
        ));
    }

    /// What's told once its answer is written to its connection, or can't be, the connection
    /// gone: so that a run that ends once it has answered ends after the client has its answer.
    pub(super) fn landing(&mut self) -> Receiver<()> {
        let (landed, landing) = mpsc::channel();
        self.flight.1 = Some(landed);
        landing
    }
}

/// Accepts connections on `listener`, serving each on threads of its own, as [`connection`] does,
/// until the process ends; but while [`MOST_SAYING_HELLO`] are saying hello, it closes each that
/// comes, warning `api.too_many_hellos` as it starts to, once until one is served again.
pub(super) fn accept(
    listener: &TcpListener,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Arc<Shared>,
) {
    let saying_hello = Arc::new(AtomicUsize::new(0));
    let mut refusing = false;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                debug!(code = "api.accept", "Can't accept a connection: {error}");
                // An error that comes again, such as running out of handles, mustn't spin.
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        // This thread alone adds to the count, so it can't pass the most between the two.
        if saying_hello.load(Ordering::Relaxed) >= MOST_SAYING_HELLO {
            if !mem::replace(&mut refusing, true) {
                warn!(
                    code = "api.too_many_hellos",
                    most = MOST_SAYING_HELLO,
                    "Refusing connections while {MOST_SAYING_HELLO} are saying hello, as one that \
                     never says it holds a thread until it's dropped"
                );
            }
            continue;
        }
        refusing = false;
        saying_hello.fetch_add(1, Ordering::Relaxed);
        let place = SayingHello(saying_hello.clone());
        let (token, welcome) = (token.to_owned(), welcome.to_owned());
        let (to_main, shared) = (to_main.clone(), shared.clone());
        let serving = thread::Builder::new()
            .name("gg-api-read".to_owned())
            .spawn(move || connection(stream, &token, &welcome, &to_main, &shared, place));
        if let Err(error) = serving {
            debug!(code = "api.accept", "Can't serve a connection: {error}");
        }
    }
}

/// A connection's place among those saying hello, which [`MOST_SAYING_HELLO`] caps, given up as it
/// drops.
struct SayingHello(Arc<AtomicUsize>);

impl Drop for SayingHello {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How many requests a connection has in flight, read and not yet answered on the wire, which
/// [`gg_api::MOST_IN_FLIGHT`] caps.
#[derive(Default)]
pub(super) struct InFlight {
    count: Mutex<usize>,
    /// Notified as a request lands, its place given up.
    landed: Condvar,
}

impl InFlight {
    /// A place among those in flight, once one is free.
    pub(super) fn take(self: &Arc<Self>) -> Flight {
        let mut count = self.count.lock().expect("no thread panics holding it");
        while *count >= gg_api::MOST_IN_FLIGHT {
            count = (self.landed.wait(count)).expect("no thread panics holding it");
        }
        *count += 1;
        Flight(self.clone(), None)
    }
}

/// A request's place among those its connection has in flight, given up as it drops: once its
/// answer is written, or as it goes unanswered, as when the connection has gone; and what's told
/// then, if anything waits for it, as [`Reply::landing`] has it.
pub(super) struct Flight(Arc<InFlight>, Option<Sender<()>>);

impl Drop for Flight {
    fn drop(&mut self) {
        *self.0.count.lock().expect("no thread panics holding it") -= 1;
        self.0.landed.notify_one();
        if let Some(landed) = self.1.take() {
            let _ = landed.send(());
        }
    }
}

/// Serves connection `stream`, which holds `place` among those saying hello until it has: its first
/// request must be `hello` with `token`, as [`hello`] reads it within [`HELLO_TIMEOUT`], which it
/// answers with `welcome`; then it answers `ping`, `logs`, `note` and `debug` itself, and passes the
/// rest on to the main loop through `to_main`, waking it, each answer going back through a thread
/// that writes them as they come, which starts once the `hello` has been granted. It reads a request
/// only while fewer than [`gg_api::MOST_IN_FLIGHT`] are in flight, so that a client that sends
/// faster than it reads, or than the game answers, is held back by the connection, rather than its
/// requests and answers growing without bound in the game. A line that isn't a request ends it.
fn connection(
    stream: TcpStream,
    token: &str,
    welcome: &str,
    to_main: &Sender<Incoming>,
    shared: &Shared,
    place: SayingHello,
) {
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream);
    let Some((id, client, grant)) = hello(&mut reader, token, Instant::now() + HELLO_TIMEOUT)
    else {
        return;
    };
    drop(place);
    let _ = reader.get_ref().set_read_timeout(None);
    let Ok(writing) = reader.get_ref().try_clone() else {
        return;
    };
    let (to, answers) = mpsc::channel::<Answer>();
    let writer = thread::Builder::new()
        .name("gg-api-write".to_owned())
        .spawn(move || write_answers(writing, &answers));
    if writer.is_err() {
        return;
    }
    let in_flight = Arc::new(InFlight::default());
    let reply = Reply {
        id,
        to: to.clone(),
        flight: in_flight.take(),
    };
    reply.send(Ok(welcome.to_owned()));
    let names = grant.reads("Name");
    loop {
        let flight = in_flight.take();
        let Some(request) = next_request(&mut reader) else {
            break;
        };
        let reply = Reply {
            id: request.id,
            to: to.clone(),
            flight,
        };
        let own = match request.method.as_str() {
            "logs" => Some(Capability::Read(None)),
            "note" => Some(Capability::Note),
            "debug" => Some(Capability::Debug),
            _ => None,
        };
        if let Some(Err(failure)) = own.map(|own| granted(&grant, &request.method, &[own])) {
            reply.send(Err(failure));
            continue;
        }
        if request.method == "ping" {
            // A script stopped first, which holds the main thread even while the window's
            // minimized, between whose frames it answers otherwise.
            let stopped = shared.debugger.stopped().then_some("breakpoint");
            let waiting = || Waiting::now().map(Waiting::name);
            let pulse = Pulse {
                frames: shared.frames.load(Ordering::Relaxed),
                waiting: stopped.or_else(waiting).map(str::to_owned),
            };
            reply.result(&pulse);
            continue;
        }
        if request.method == "logs" {
            reply.send(logs(&request.params, shared));
            continue;
        }
        if request.method == "note" {
            reply.send(note(&request.params, &client));
            continue;
        }
        if request.method == "debug" {
            reply.send(debug(&request.params, shared));
            continue;
        }
        let parsed = parse(&request).and_then(|call| {
            granted(&grant, &request.method, &needs(&call))?;
            Ok(call)
        });
        match parsed {
            Ok(call) => {
                let client = client.clone();
                let incoming = Incoming {
                    call,
                    client,
                    names,
                    reply,
                };
                if let Err(mpsc::SendError(incoming)) = to_main.send(incoming) {
                    let message = "the game is ending, and took nothing of the request";
                    incoming
                        .reply
                        .send(Err(Failure::new("api.closed", message)));
                    return;
                }
                shared.passed.fetch_add(1, Ordering::Release);
                if let Some(wake) = shared
                    .waker
                    .lock()
                    .expect("no thread panics holding it")
                    .as_ref()
                {
                    wake();
                }
            }
            Err(failure) => reply.send(Err(failure)),
        }
    }
}

/// What `call` needs of its connection's grant: of the world, `read` or `write` of the type it
/// names alone, or else of the whole world; or the tool API's own capability its method needs,
/// with `files` for a path to write at.
pub(super) fn needs(call: &Call) -> Vec<Capability> {
    use Capability::{Files, Input, Patch, Play, Quit, Read, Scene, Snapshot, View, Write};

    match call {
        Call::Status => Vec::new(),
        Call::Pause | Call::Resume | Call::Play | Call::Stop | Call::Step(_) => vec![Play],
        Call::Scene(_) => vec![Scene],
        Call::Capture { path, .. } => [Some(View), path.as_ref().map(|_| Files)]
            .into_iter()
            .flatten()
            .collect(),
        Call::Frames { .. } => vec![View, Files],
        Call::View(_) | Call::Look(_) | Call::Pick(..) | Call::Images => vec![View],
        Call::Input(_) => vec![Input],
        Call::Get { component, .. } => vec![Read(Some(component.clone()))],
        Call::Entities { .. }
        | Call::Entity(_)
        | Call::Types
        | Call::Hash
        | Call::Cells
        | Call::Assets { .. }
        | Call::Ui
        | Call::Edits(_) => {
            vec![Read(None)]
        }
        Call::Snapshot { path, quit } => [
            Some(Snapshot),
            path.as_ref().map(|_| Files),
            quit.then_some(Quit),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Call::Patch(_) => vec![Patch],
        Call::Apply(operations, _) => operations
            .iter()
            .map(|operation| match operation {
                gg_api::Operation::Add { component, .. }
                | gg_api::Operation::Remove { component, .. }
                | gg_api::Operation::Set { component, .. }
                | gg_api::Operation::Revert { component, .. } => Write(Some(component.clone())),
                _ => Write(None),
            })
            .collect(),
        Call::SavePrefab { .. } => vec![Write(None), Scene],
        Call::Undo { .. } => vec![Write(None)],
        Call::Quit => vec![Quit],
    }
}

/// Whether `grant`, a connection's, covers each of `needs`, what a request of `method` needs.
///
/// Fails with `api.ungranted`, naming the first it doesn't.
pub(super) fn granted(grant: &Grant, method: &str, needs: &[Capability]) -> Result<(), Failure> {
    match needs.iter().find(|need| !grant.has(need)) {
        None => Ok(()),
        Some(need) => Err(Failure::new(
            "api.ungranted",
            format!(
                "`{method}` needs `{need}`, which the connection isn't granted: its `hello` \
                 granted `{grant}`"
            ),
        )),
    }
}

/// Whether `given` is `token`, compared in a time that doesn't depend on where they first differ,
/// so that a client can't find the token a byte at a time by timing its refusals: only their
/// lengths tell early, and the token's, 32, is no secret. `black_box` keeps the compiler from
/// ending the loop at the first difference, as far as std's best effort goes.
pub(super) fn same_token(given: &str, token: &str) -> bool {
    let (given, token) = (given.as_bytes(), token.as_bytes());
    given.len() == token.len()
        && (given.iter().zip(token))
            .fold(0, |differ, (a, b)| std::hint::black_box(differ | (a ^ b)))
            == 0
}

/// What `logs`, given `params`, answers, from the events `shared` keeps, as gg-api's docs say: read
/// on the connection's own thread, which a wait holds.
///
/// Fails with `api.bad_params` for parameters it doesn't take, a `level` that isn't one, or a wait
/// of more than [`MOST_WAIT_MS`].
fn logs(params: &Value, shared: &Shared) -> Result<String, Failure> {
    let method = "logs";
    let gg_api::Logs {
        from,
        run,
        last,
        level,
        wait_ms,
    } = take(method, params)?;
    let min = match level {
        Some(level) => {
            let at = gg_api::LEVELS.iter().position(|&name| name == level);
            let at = at.ok_or_else(|| {
                let what = format!(
                    "takes a `level` of `error`, `warn`, `info`, `debug` or `trace`, not `{level}`"
                );
                bad_params(method, what)
            })?;
            LEVELS[at]
        }
        None => Level::TRACE,
    };
    if wait_ms > MOST_WAIT_MS {
        let what = format!("waits at most {MOST_WAIT_MS} ms, not {wait_ms}");
        return Err(bad_params(method, what));
    }
    // Another run's cursor numbers another run's events, so all of this run's are new to it.
    let from = match (from, run) {
        (Some(_), Some(run)) if run != shared.run => Some(0),
        (from, _) => from,
    };
    let wait = Duration::from_millis(wait_ms);
    let read = (shared.events).read(from, last, min, wait, gg_api::MAX_LINE / 2);
    // Each line is an object of JSON already, as the log wrote it.
    let mut answer = format!(
        r#"{{"run":{},"next":{},"end":{},"missed":{},"events":["#,
        shared.run, read.next, read.end, read.missed
    );
    for (i, line) in read.lines.iter().enumerate() {
        if i > 0 {
            answer.push(',');
        }
        answer.push_str(line);
    }
    answer.push_str("]}");
    Ok(answer)
}

/// What `note`, given `params` by client `client`, answers, having logged the event it gives, as
/// gg-api's docs say: at its level, with its code, its message, `client`, and its fields as one
/// field of JSON text, `fields`, since an event's fields are flat. It's answered on the
/// connection's own thread, so that a note comes through while frames pause.
///
/// Fails with `api.bad_params` for parameters it doesn't take, as a level that isn't `info` or
/// `warn`; a code that isn't a stable code's shape, as [`is_code`] says, or is longer than
/// [`gg_api::MOST_NOTE_CODE`] bytes; an empty message; or a message or fields longer than
/// [`gg_api::MOST_NOTE`] bytes.
pub(super) fn note(params: &Value, client: &str) -> Result<String, Failure> {
    let method = "note";
    let gg_api::Note {
        code,
        level,
        message,
        fields,
    } = take(method, params)?;
    let (most_code, most) = (gg_api::MOST_NOTE_CODE, gg_api::MOST_NOTE);
    if code.len() > most_code {
        let what = format!(
            "takes a code of at most {most_code} bytes, not {}",
            code.len()
        );
        return Err(bad_params(method, what));
    }
    if !is_code(&code) {
        let what = format!(
            "takes a stable code, dotted names of lowercase ASCII letters, digits and `_`, each \
             starting with a letter, such as `gg.patch_failed`, not `{code}`"
        );
        return Err(bad_params(method, what));
    }
    if message.is_empty() || message.len() > most {
        let what = format!(
            "takes a message of 1 to {most} bytes, not {}",
            message.len()
        );
        return Err(bad_params(method, what));
    }
    let fields = fields.map(|fields| Value::Object(fields).to_string());
    if let Some(given) = fields.as_ref().filter(|given| given.len() > most) {
        let what = format!(
            "takes fields of at most {most} bytes as JSON, not {}",
            given.len()
        );
        return Err(bad_params(method, what));
    }
    let (code, fields) = (code.as_str(), fields.as_deref());
    match level {
        gg_api::NoteLevel::Info => info!(code, client, fields, "{message}"),
        gg_api::NoteLevel::Warn => warn!(code, client, fields, "{message}"),
    }
    Ok("{}".to_owned())
}

/// What `debug`, given `params`, answers, as gg-api's docs say: from `shared`'s debugger, on the
/// connection's own thread, so that it answers while a script has stopped and the main thread waits
/// there, and which a wait holds. Breakpoints and a pause wake the main loop, which places them, or
/// steps to the pause, as a frame starts.
///
/// Fails with `api.bad_params` for parameters it doesn't take, a script's path that's empty, a line
/// of 0 or a wait of more than [`MOST_WAIT_MS`]; and with `script.not_stopped` for a continue or a
/// step while no script has stopped.
fn debug(params: &Value, shared: &Shared) -> Result<String, Failure> {
    let method = "debug";
    let gg_api::Debug {
        breakpoints,
        action,
        wait_ms,
    } = take(method, params)?;
    if wait_ms > MOST_WAIT_MS {
        let what = format!("waits at most {MOST_WAIT_MS} ms, not {wait_ms}");
        return Err(bad_params(method, what));
    }
    if let Some(given) = &breakpoints {
        if given.script.is_empty() {
            return Err(bad_params(
                method,
                "takes a script's path from the project's root",
            ));
        }
        if given.lines.contains(&0) {
            return Err(bad_params(method, "takes lines from 1"));
        }
    }
    let debugger = &shared.debugger;
    let wake = breakpoints.is_some() || action == Some(gg_api::DebugAction::Pause);
    if let Some(gg_api::Breakpoints { script, lines }) = breakpoints {
        debugger.set(script, lines);
    }
    if let Some(action) = action {
        debugger.act(action).map_err(|error| failure(&error))?;
    }
    if wake
        && let Some(wake) = shared
            .waker
            .lock()
            .expect("no thread panics holding it")
            .as_ref()
    {
        wake();
    }
    let answer = debugger.answer(Duration::from_millis(wait_ms));
    Ok(serde_json::to_string(&answer).expect("an answer serializes"))
}

/// Whether `code` has a stable code's shape: two or more names, joined by dots, each of lowercase
/// ASCII letters, digits and `_`, starting with a letter, as `gg.patch_failed`.
fn is_code(code: &str) -> bool {
    let name = |name: &str| {
        name.starts_with(|c: char| c.is_ascii_lowercase())
            && (name.chars()).all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    code.contains('.') && code.split('.').all(name)
}

/// The ID, the client and the grant of the `hello` that `reader`, a new connection's, reads by
/// `deadline`, as [`read_hello`] does, if it gives `token`. A first request that isn't such a
/// `hello`, or one whose grant names a capability there's none of, is answered with the failure,
/// and `None` returned.
fn hello(
    reader: &mut BufReader<TcpStream>,
    token: &str,
    deadline: Instant,
) -> Option<(u64, Arc<str>, Grant)> {
    let hello = read_hello(reader, deadline)?;
    let given = (hello.method == "hello")
        .then(|| serde_json::from_value::<Hello>(hello.params).ok())
        .flatten();
    let granted = match given.filter(|given| same_token(&given.token, token)) {
        None => {
            let message = "a connection's first request is `hello`, with the session file's token";
            Err(Failure::new("api.unauthorized", message))
        }
        Some(Hello { client, grant, .. }) => match grant.map(Grant::parse) {
            None => Ok((client, Grant::all())),
            Some(Ok(grant)) => Ok((client, grant)),
            Some(Err(error)) => Err(Failure::new(error.code(), format!("{error:#}"))),
        },
    };
    match granted {
        Ok((client, grant)) => Some((hello.id, client.as_deref().unwrap_or(UNNAMED).into(), grant)),
        Err(failure) => {
            // Here, as no thread writes the connection's answers until its `hello` is granted. It's
            // the first line, which the hello's bound keeps to tens of KiB even escaped, within
            // what the loopback's buffers take, so it's written without the client reading; and
            // within a timeout, should they not take it, so that a client that never reads can't
            // keep its place among those saying hello.
            let line = gg_api::answer_line(hello.id, Err(&failure));
            let _ = reader.get_ref().set_write_timeout(Some(HELLO_TIMEOUT));
            let _ = reader.get_ref().write_all(line.as_bytes());
            None
        }
    }
}

/// The `hello` that `reader`, a new connection's, reads: its first line, of at most
/// [`gg_api::MAX_HELLO`] bytes, its line break aside, whole by `deadline`, each read waiting only
/// for the time left, so that a client sending a byte at a time can't hold the connection longer.
/// `None` if it doesn't come so, the connection ends or breaks first, or it isn't a request; a line
/// too long is refused once it's seen to be, the rest unread.
pub(super) fn read_hello(reader: &mut BufReader<TcpStream>, deadline: Instant) -> Option<Request> {
    let mut line = Vec::new();
    loop {
        // A timeout of zero is refused, as it would wait for good.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        reader.get_ref().set_read_timeout(Some(left)).ok()?;
        let read = match reader.fill_buf() {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if read.is_empty() {
            return None;
        }
        let (taken, whole) = match read.iter().position(|&byte| byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (read.len(), false),
        };
        if line.len() + taken - usize::from(whole) > gg_api::MAX_HELLO {
            debug!(
                code = "api.bad_request",
                "A tool's hello is longer than {} bytes, so its connection closes",
                gg_api::MAX_HELLO
            );
            return None;
        }
        line.extend_from_slice(&read[..taken]);
        reader.consume(taken);
        if whole {
            return request(&line);
        }
    }
}

/// The next request `reader` reads: `None` once the connection ends or breaks, or at a line that
/// isn't a request.
fn next_request(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let line = gg_api::read_line(reader).ok()??;
    request(line.as_bytes())
}

/// The request `line` is, which may end in its line break; `None` if it isn't one.
fn request(line: &[u8]) -> Option<Request> {
    serde_json::from_slice(line)
        .inspect_err(|error| {
            debug!(
                code = "api.bad_request",
                "A tool sent what isn't a request, so its connection closes: {error}"
            );
        })
        .ok()
}

/// Writes the lines `answers` brings to `stream`, each request's place in flight going once its
/// line is written, until the connection breaks or nothing more can come.
fn write_answers(mut stream: TcpStream, answers: &Receiver<Answer>) {
    for (line, _flight) in answers {
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
    }
}
