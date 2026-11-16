//! Preparing cells ahead of the ticks that bring them in: reading each cell's file, finding what
//! its texts name that isn't loaded, reading the models among it, and parsing the texts. It runs on
//! `gg-cells` threads, so the main thread only adds what was read and spawns what was parsed; in a
//! test's or replay's context it runs on the calling thread as asked. It changes nothing in the
//! world: what comes in, and when, is the world's ([`Cells`](crate::Cells)); a tick whose cells
//! aren't prepared waits for them.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use gg_core::{Error, Result};
use tracing::warn;

use super::{Cell, Entering};
use crate::assets::Asset;
use crate::gltf::Imported;
use crate::package::Files;
use crate::{World, cooked, gltf};

/// How many threads prepare cells: two, so one reading a large model's files (tens of ms) doesn't
/// hold up the next cell's file.
const THREADS: usize = 2;

/// A model's files as read for [`Context::add_model`](crate::Context::add_model), by a cell's
/// preparing or by the add itself, and when the read began, which hot reload watches from.
pub(crate) struct ModelRead {
    #[cfg_attr(
        not(feature = "dev"),
        allow(dead_code, reason = "only hot reload watches")
    )]
    pub(crate) at: SystemTime,
    pub(crate) read: Result<(Imported, Vec<PathBuf>)>,
}

impl ModelRead {
    /// Reads model `name`'s files at `path` from `files`: as cooked from a package, listing no
    /// files since nothing watches a package; else as glTF, listing the files read.
    pub(crate) fn now(name: &str, path: &Path, files: &Files) -> Self {
        let at = SystemTime::now();
        let read = match files.packaged() {
            Some(_) => cooked::read(name, path, files).map(|imported| (imported, Vec::new())),
            None => gltf::read_listing(name, path, files),
        };
        Self { at, read }
    }
}

/// A cell's texts with digests: its file's at `path` if it is to come in (the game's making if
/// `made`), and those it keeps, in turn.
#[derive(Clone)]
pub(crate) struct Texts {
    pub(crate) path: String,
    pub(crate) file: Option<(Arc<str>, u64)>,
    pub(crate) made: bool,
    pub(crate) kept: Vec<(Arc<str>, u64)>,
}

impl Prep {
    pub(crate) fn number(&self) -> u64 {
        self.number
    }
}

impl Texts {
    pub(crate) fn each(&self) -> impl Iterator<Item = &str> {
        let file = self.file.iter().map(|(text, _)| &**text);
        file.chain(self.kept.iter().map(|(text, _)| &**text))
    }

    /// `error` with the cell's path first in its message.
    pub(crate) fn named(&self, error: Error) -> Error {
        let message = format!("{}:{error}", self.path);
        error.with_message(message)
    }
}

/// A cell prepared to come in: texts parsed, with digests the cell must still have as it comes in;
/// `made` if its file's text is the game's making.
pub(crate) struct Prepared {
    pub(crate) file: Option<u64>,
    pub(crate) made: bool,
    pub(crate) kept: Vec<u64>,
    pub(crate) entering: Entering,
}

/// A cell being prepared, numbered so a thread's result for a dropped preparing isn't taken for a
/// later one of the same cell; when it started, and the main thread's time on it.
pub(crate) struct Prep {
    number: u64,
    pub(crate) started: Instant,
    pub(crate) main: Duration,
    pub(crate) stage: Stage,
}

pub(crate) enum Stage {
    /// Texts reading on a thread, then parsing if nothing they name is missing.
    Reading,
    /// `models`' files reading on threads; the rest of what the texts name loads on the main
    /// thread once read.
    Loading { texts: Texts, models: Vec<PathBuf> },
    /// Texts parsing on a thread.
    Parsing,
    /// Prepared, or why not, which the cell's coming in repeats.
    Ready(Result<Prepared>),
}

/// What a thread finished.
pub(crate) enum Finished {
    /// Texts read, with what they name that isn't loaded, and, if nothing is, parsed.
    Read {
        cell: Cell,
        number: u64,
        texts: Result<Texts>,
        missing: Vec<Asset<String>>,
        parsed: Option<Result<Prepared>>,
    },
    /// A model's files read.
    Model { path: PathBuf, read: ModelRead },
    Parsed {
        cell: Cell,
        number: u64,
        parsed: Result<Prepared>,
    },
}

type Job = Box<dyn FnOnce() -> Finished + Send>;

/// What a run prepares of the cells near its viewers: the cells being prepared; model files read
/// for them, by path, until a model's add takes them, and those reading; and since when a tick has
/// waited for its cells.
#[derive(Default)]
pub(crate) struct Preparing {
    workers: Workers,
    pub(crate) cells: BTreeMap<Cell, Prep>,
    pub(crate) models: HashMap<PathBuf, ModelRead>,
    reading: HashSet<PathBuf>,
    next: u64,
    pub(crate) waiting: Option<Instant>,
}

/// Where jobs run: as sent, results queued (a test's or replay's context); or on threads, which
/// the first job starts.
enum Workers {
    Inline(VecDeque<Finished>),
    Threads(Option<Threads>),
}

impl Default for Workers {
    fn default() -> Self {
        Self::Inline(VecDeque::new())
    }
}

/// The threads, from the main thread's side. Dropped, it lets them go unjoined: each ends after its
/// current job, finding no one to send to.
struct Threads {
    jobs: Sender<Job>,
    done: Receiver<Finished>,
    threads: Vec<JoinHandle<()>>,
}

impl Preparing {
    /// Prepares on threads, started by the first job.
    pub(crate) fn threaded() -> Self {
        Self {
            workers: Workers::Threads(None),
            ..Self::default()
        }
    }

    /// Starts preparing `cell` at [`Stage::Reading`]; returns the number its jobs carry.
    pub(crate) fn begin(&mut self, cell: Cell) -> u64 {
        let number = self.next;
        self.next += 1;
        let prep = Prep {
            number,
            started: Instant::now(),
            main: Duration::ZERO,
            stage: Stage::Reading,
        };
        self.cells.insert(cell, prep);
        number
    }

    pub(crate) fn prep(&mut self, cell: Cell, number: u64) -> Option<&mut Prep> {
        (self.cells.get_mut(&cell)).filter(|prep| prep.number == number)
    }

    /// Runs `job`; on the calling thread, warning `engine.loader`, if the threads can't start.
    pub(crate) fn send(&mut self, job: impl FnOnce() -> Finished + Send + 'static) {
        if let Workers::Threads(None) = self.workers {
            match Threads::start() {
                Ok(threads) => self.workers = Workers::Threads(Some(threads)),
                Err(error) => {
                    warn!(
                        code = error.code(),
                        "{error:#}; cells are prepared on the main thread as they're asked for"
                    );
                    self.workers = Workers::default();
                }
            }
        }
        match &mut self.workers {
            Workers::Inline(done) => done.push_back(job()),
            Workers::Threads(Some(threads)) => {
                // Fails only once every thread has panicked, whose panics `take` resumes.
                let _ = threads.jobs.send(Box::new(job));
            }
            Workers::Threads(None) => unreachable!("started above, or inline"),
        }
    }

    /// On a thread: gets the file text through `read` (the file, or the game's making if `made`;
    /// `path` names it), adds `kept`, finds what they name that `world` (a scratch world of the
    /// world's types and assets) hasn't loaded, and parses them in it if nothing is missing.
    pub(crate) fn read(
        &mut self,
        (cell, number): (Cell, u64),
        (path, made): (String, bool),
        read: Option<impl FnOnce() -> Result<Arc<str>> + Send + 'static>,
        kept: Vec<(Arc<str>, u64)>,
        world: World,
    ) {
        self.send(move || {
            let _zone = gg_core::zone!("prepare a cell");
            let file = read.map(|read| read().map(|text| (crate::ecs::text_digest(&text), text)));
            let texts = file.transpose().map(|file| Texts {
                path,
                file: file.map(|(digest, text)| (text, digest)),
                made,
                kept,
            });
            let texts = match texts {
                Ok(texts) => texts,
                Err(error) => {
                    let (texts, missing, parsed) = (Err(error), Vec::new(), None);
                    return Finished::Read {
                        cell,
                        number,
                        texts,
                        missing,
                        parsed,
                    };
                }
            };
            let (missing, parsed) = match missing(&world, &texts) {
                Ok(missing) if missing.is_empty() => (missing, Some(parse(&world, &texts))),
                Ok(missing) => (missing, None),
                Err(error) => (Vec::new(), Some(Err(error))),
            };
            Finished::Read {
                cell,
                number,
                texts: Ok(texts),
                missing,
                parsed,
            }
        });
    }

    /// Reads model `name`'s files on a thread, unless already read or reading.
    pub(crate) fn read_model(&mut self, name: Arc<str>, path: PathBuf, files: Files) {
        if self.models.contains_key(&path) || !self.reading.insert(path.clone()) {
            return;
        }
        self.send(move || {
            let _zone = gg_core::zone!("read a cell's model");
            let read = ModelRead::now(&name, &path, &files);
            Finished::Model { path, read }
        });
    }

    /// Notes the model at `path` as `read`, for a model's add to take.
    pub(crate) fn model_read(&mut self, path: PathBuf, read: ModelRead) {
        self.reading.remove(&path);
        self.models.insert(path, read);
    }

    pub(crate) fn reading(&self, path: &Path) -> bool {
        self.reading.contains(path)
    }

    /// Parses `texts` on a thread in `world`, a scratch world of the world's types and assets.
    pub(crate) fn parse(&mut self, (cell, number): (Cell, u64), texts: Texts, world: World) {
        self.send(move || {
            let _zone = gg_core::zone!("parse a cell");
            let parsed = parse(&world, &texts);
            Finished::Parsed {
                cell,
                number,
                parsed,
            }
        });
    }

    /// The next thing a thread finished, if any.
    ///
    /// # Panics
    ///
    /// - A thread panicked: this resumes it.
    pub(crate) fn take(&mut self) -> Option<Finished> {
        match &mut self.workers {
            Workers::Inline(done) => done.pop_front(),
            Workers::Threads(None) => None,
            Workers::Threads(Some(threads)) => {
                if let Ok(done) = threads.done.try_recv() {
                    return Some(done);
                }
                // A thread ends while the run holds it only as it panics.
                if let Some(at) = threads.threads.iter().position(JoinHandle::is_finished) {
                    match threads.threads.swap_remove(at).join() {
                        Err(panic) => panic::resume_unwind(panic),
                        Ok(()) => unreachable!("the threads run until the run lets go"),
                    }
                }
                None
            }
        }
    }

    /// Drops read model files no loading cell waits for, left by a cell dropped meanwhile.
    pub(crate) fn forget_models(&mut self) {
        if self.models.is_empty() {
            return;
        }
        let waited = |path: &PathBuf| {
            (self.cells.values()).any(|prep| match &prep.stage {
                Stage::Loading { models, .. } => models.contains(path),
                _ => false,
            })
        };
        let forgotten: Vec<PathBuf> = self.models.keys().filter(|p| !waited(p)).cloned().collect();
        for path in forgotten {
            self.models.remove(&path);
        }
    }
}

impl Threads {
    /// Starts threads that run jobs and send back results until the run lets go. Fails with
    /// `engine.loader` if one can't start.
    fn start() -> Result<Self> {
        let (jobs, to_run) = mpsc::channel::<Job>();
        let to_run = Arc::new(Mutex::new(to_run));
        let (back, done) = mpsc::channel();
        let threads = (0..THREADS)
            .map(|_| {
                let (to_run, back) = (Arc::clone(&to_run), back.clone());
                thread::Builder::new()
                    .name("gg-cells".to_owned())
                    .spawn(move || {
                        loop {
                            // Held only while waiting for a job, so the others run theirs.
                            let queue = to_run.lock().unwrap_or_else(PoisonError::into_inner);
                            let Ok(job) = queue.recv() else {
                                return;
                            };
                            drop(queue);
                            if back.send(job()).is_err() {
                                return;
                            }
                        }
                    })
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| {
                let message = "can't start the threads that prepare cells";
                Error::new("engine.loader", message).with_source(error)
            })?;
        Ok(Self {
            jobs,
            done,
            threads,
        })
    }
}

/// What `texts` name that `world` hasn't loaded, each once, in first-named order.
///
/// Fails like [`World::scene_assets`], with the cell's path first in the message.
fn missing(world: &World, texts: &Texts) -> Result<Vec<Asset<String>>> {
    let mut missing = Vec::new();
    for text in texts.each() {
        for asset in world.scene_assets(text).map_err(|e| texts.named(e))? {
            if !missing.contains(&asset) {
                missing.push(asset);
            }
        }
    }
    Ok(missing)
}

/// `texts` parsed in `world` by [`World::parse_scene`], with what the roots come as
/// ([`came_as`](super::came_as)) if the cell brings only its file's text.
///
/// Fails like `parse_scene`, with the cell's path first in the message.
pub(crate) fn parse(world: &World, texts: &Texts) -> Result<Prepared> {
    let parse = |text: &str| world.parse_scene(text).map_err(|e| texts.named(e));
    let file = (texts.file.as_ref())
        .map(|(text, digest)| Ok((*digest, parse(text)?)))
        .transpose()?;
    let kept = texts.kept.iter().map(|(text, _)| parse(text));
    let came_as = match &texts.file {
        Some((text, _)) if texts.kept.is_empty() => super::came_as(world, text),
        _ => None,
    };
    Ok(Prepared {
        file: texts.file.as_ref().map(|&(_, digest)| digest),
        made: texts.made,
        kept: texts.kept.iter().map(|&(_, digest)| digest).collect(),
        entering: Entering {
            file,
            kept: kept.collect::<Result<_>>()?,
            came_as,
        },
    })
}
