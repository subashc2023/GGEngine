//! A server's states, as [the server](super::server) sends them: what each client sees of the
//! world after a tick, written by a thread of the server's own, `gg-net-states`, from a copy of the
//! world taken as the tick ends, so that the server's main thread spends a copy a tick on them,
//! whatever the clients' number or the world's size. Each state holds what changed since its base,
//! the state the client has said it has, and names the rest as kept, which [`rebuild`] puts the
//! base's text in place of. The client rebuilds each state's text whole, reads and checks it as a
//! whole state, and keeps it for the next to be against, as the server does.
//!
//! A state's text is the world's, as `World::save_seen` writes it, but for these lines:
//!
//! ```text
//! kept tables 3                in place of the next 3 tables, as the base has them
//! kept children                in place of the `children` lines, as the base has them
//! kept resource Physics3D      in place of the resource's line, as the base has it
//! ```
//!
//! A table is kept while it's of the world the base is of, which the client's view hides the same
//! entities of, it has as many rows as it had, and, as its marks say, no row joined it and no
//! column of its changed since the tick the base is after, as the server's world's change tick
//! moves on after each tick. A row taken out marks nothing, as the table's last row moves into its
//! place, but leaves one fewer. The `children` lines are kept while no table is new, none has
//! another number of rows, and no row joined one and no `Parent` changed since. A resource is kept
//! while its text is the base's. A kept row that names an entity taken out since names it by its
//! ID, where the server's text would say `Dead`, and reads as `Dead` reads, as no entity has the
//! ID; one that comes back under its handle, as an undone delete puts it back, marks every table.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use gg_core::{Error, Result};

use super::MAX_WORLD;
use crate::cells::{Cell, Seen};
use crate::{Parent, World, WorldHash};

/// The level zstd compresses states at: its default, quick, as states go out each tick.
const LEVEL: i32 = 3;

/// A state's whole text, which the client keeps for the next state to be against, as the server
/// does, with what the server keeps beside it to tell which of its tables a later state keeps.
pub(super) struct Base {
    /// The text of what the client saw after the tick, whole, as [`rebuild`] gives it.
    pub(super) text: String,
    /// The server's world it's of, and that world's change tick as the tick ended; and how many
    /// rows each of its tables had.
    of: Source,
    rows: Vec<usize>,
    /// The slots of the entities the client's view hid, as [`Seen`] has them.
    hidden: Vec<bool>,
}

/// The server's world a copy is of, as the copy was taken: its serial, which another world,
/// restored or replaced, hasn't; and its change tick, which its tables' marks, copied, count by.
/// The copy's own change tick never falls, as a copy's doesn't, so that it's past the world's once
/// a layer puts a world of an earlier change tick in its place.
#[derive(Clone, Copy)]
struct Source {
    serial: u64,
    change_tick: u64,
}

impl Source {
    fn of(world: &World) -> Self {
        Self {
            serial: world.serial(),
            change_tick: world.change_tick(),
        }
    }
}

/// A client whose state the thread writes, as the server knows it as a tick ends.
pub(super) struct Wanted {
    /// Which client, as the server numbers them, and its player.
    pub(super) client: u64,
    pub(super) player: u8,
    /// The cells it saw as its last state was written, as [`World::seen_by`] has them.
    pub(super) had: BTreeSet<Cell>,
    /// The latest state it has said it has, by its tick, if the server still keeps it.
    pub(super) base: Option<(u64, Arc<Base>)>,
    /// The first tick of its inputs not applied, and how many ticks before the server ran it its
    /// latest inputs came, which its state's line says.
    pub(super) next: u64,
    pub(super) lead: i64,
}

/// A client's state, as the thread wrote it.
pub(super) struct Written {
    /// The client, as [`Wanted`] names it, and the tick of the base its state is against.
    pub(super) client: u64,
    pub(super) base: Option<u64>,
    /// As [`Wanted`] had them.
    pub(super) next: u64,
    pub(super) lead: i64,
    /// The hash of what the client sees, and the cells it sees.
    pub(super) hash: WorldHash,
    pub(super) cells: BTreeSet<Cell>,
    /// Its state, which the clients that saw alike against the same base share.
    pub(super) state: Arc<State>,
}

/// A state: its text, as zstd compresses it, against its base as zstd's dictionary, and its bytes
/// uncompressed; its text whole, for the next state to be against; and how many tables it wrote,
/// and kept.
pub(super) struct State {
    pub(super) bytes: Vec<u8>,
    pub(super) size: usize,
    pub(super) whole: Arc<Base>,
    pub(super) tables: [u64; 2],
}

/// The thread that writes the clients' states, `gg-net-states`: what it's given, and what it gives
/// back, the copy of the world it wrote them from with them; and that copy, while the thread is
/// idle, which the next tick's world is copied into, in its memory.
pub(super) struct States {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    copy: Option<World>,
}

/// The clients whose states the thread writes after tick `tick`, from `copy`, a copy of the world
/// `source` says.
struct Job {
    copy: World,
    source: Source,
    tick: u64,
    wanted: Vec<Wanted>,
}

/// The states after tick `tick`, written from `copy`.
struct Done {
    copy: World,
    tick: u64,
    written: Result<Vec<Written>>,
}

impl States {
    /// Starts the thread, which ends as the `States` is dropped, once it has written what it was
    /// given.
    ///
    /// Fails with `net.start` if the thread can't start.
    pub(super) fn start() -> Result<Self> {
        let (jobs, taken) = mpsc::channel::<Job>();
        let (gave, done) = mpsc::channel();
        thread::Builder::new()
            .name("gg-net-states".to_owned())
            .spawn(move || {
                for Job {
                    copy,
                    source,
                    tick,
                    wanted,
                } in taken
                {
                    let written = write(&copy, source, &wanted);
                    let done = Done {
                        copy,
                        tick,
                        written,
                    };
                    if gave.send(done).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| {
                let message = "can't start the thread that writes the clients' states";
                Error::new("net.start", message).with_source(error)
            })?;
        Ok(Self {
            jobs,
            done,
            copy: Some(World::default()),
        })
    }

    /// Whether the thread is idle: it has given back the states it was last given, which
    /// [`take`](Self::take) has taken.
    pub(super) fn idle(&self) -> bool {
        self.copy.is_some()
    }

    /// Has the thread write the states of `wanted` after tick `tick`, of `world` as it is, which
    /// it copies, as [`write()`] says.
    ///
    /// # Panics
    ///
    /// If the thread isn't [`idle`](Self::idle), or has panicked.
    pub(super) fn give(&mut self, world: &World, tick: u64, wanted: Vec<Wanted>) {
        let mut copy = self.copy.take().expect("the states thread is idle");
        copy.clone_from(world);
        let job = Job {
            copy,
            source: Source::of(world),
            tick,
            wanted,
        };
        (self.jobs.send(job)).expect("the states thread runs while it's given states to write");
    }

    /// The states the thread was last given, with their tick, once it has written them: by
    /// `until`, waiting for them until then, or at once, without waiting, if it's `None`. `None`
    /// if they aren't written by then, or the thread is idle.
    ///
    /// # Panics
    ///
    /// If the thread has panicked, as writing a state would have panicked on the main thread.
    pub(super) fn take(&mut self, until: Option<Instant>) -> Option<(u64, Result<Vec<Written>>)> {
        if self.idle() {
            return None;
        }
        let panicked = "the thread that writes the clients' states panicked";
        let done = match until {
            Some(until) => match self.done.recv_timeout(until - Instant::now().min(until)) {
                Ok(done) => done,
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => panic!("{panicked}"),
            },
            None => match self.done.try_recv() {
                Ok(done) => done,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => panic!("{panicked}"),
            },
        };
        self.copy = Some(done.copy);
        Some((done.tick, done.written))
    }
}

/// Writes the state of each of `wanted` from `world`, a copy of the server's world `source` says,
/// as the module says: each view of the world, as [`World::seen_by`] finds it, hashed once, its
/// head, resources and the tables and children its clients' states need written once, and each
/// state once for the clients that see alike against the same base.
///
/// Fails as [`World::save_seen`] does, and with `net.link` for a state that zstd can't compress, or
/// whose text whole is past [`MAX_WORLD`] bytes.
fn write(world: &World, source: Source, wanted: &[Wanted]) -> Result<Vec<Written>> {
    let _zone = gg_core::zone!();
    let mut views: Vec<View> = Vec::new();
    let mut written = Vec::with_capacity(wanted.len());
    for want in wanted {
        let seen = world.seen_by(want.player, &want.had);
        let at = match views.iter().position(|view| view.seen == seen) {
            Some(at) => at,
            None => {
                views.push(View::of(world, seen)?);
                views.len() - 1
            }
        };
        let view = &mut views[at];
        let base = want.base.as_ref().map(|(_, base)| base);
        let same = |against: &Option<Arc<Base>>| match (against, base) {
            (Some(against), Some(base)) => Arc::ptr_eq(against, base),
            (against, base) => against.is_none() && base.is_none(),
        };
        let state = match view.states.iter().find(|(against, _)| same(against)) {
            Some((_, state)) => Arc::clone(state),
            None => {
                let state = Arc::new(view.state(world, source, base.map(|base| &**base))?);
                view.states.push((base.cloned(), Arc::clone(&state)));
                state
            }
        };
        written.push(Written {
            client: want.client,
            base: want.base.as_ref().map(|&(tick, _)| tick),
            next: want.next,
            lead: want.lead,
            hash: view.hash,
            cells: view.seen.cells.cells_in.clone(),
            state,
        });
    }
    Ok(written)
}

/// A view of the world, as clients that see alike have it: what it sees and its hash; its head,
/// and its resources' texts, written as it's found; each table's text and its `children` lines,
/// once a state needs them; and its states, each by the base it's against.
struct View {
    seen: Seen,
    hash: WorldHash,
    head: String,
    tables: Vec<Option<String>>,
    children: Option<String>,
    resources: Vec<(&'static str, String)>,
    states: Vec<(Option<Arc<Base>>, Arc<State>)>,
}

impl View {
    /// What `seen` sees of `world`.
    ///
    /// Fails as [`World::save_resources`] does.
    fn of(world: &World, seen: Seen) -> Result<Self> {
        let mut head = String::new();
        world.save_head(&mut head, &seen.hidden);
        Ok(Self {
            hash: world.hash_seen(Some(&seen)),
            head,
            tables: vec![None; world.tables().len()],
            children: None,
            resources: world.save_resources(Some(&seen))?,
            states: Vec::new(),
            seen,
        })
    }

    /// The text of table `at` of `world`, as the view sees it.
    ///
    /// Fails as [`World::save_table`] does.
    fn table(&mut self, world: &World, at: usize) -> Result<&str> {
        let text = &mut self.tables[at];
        if text.is_none() {
            let mut table = String::new();
            world.save_table(&mut table, at, &self.seen.hidden)?;
            *text = Some(table);
        }
        Ok(text.as_deref().expect("written"))
    }

    /// The view's state of `world`, a copy of the world `source` says, against `base`, the latest
    /// state its client has, if the server keeps it, as the module says; and whole, otherwise.
    ///
    /// Fails as [`table`](Self::table) does, and with `net.link` for a state that zstd can't
    /// compress, or whose text whole is past [`MAX_WORLD`] bytes.
    fn state(&mut self, world: &World, source: Source, base: Option<&Base>) -> Result<State> {
        let line = "a string takes any text";
        let mut text = self.head.clone();
        let tables = world.tables();
        // A base whose tables, rows and slots are this world's, seen alike.
        let like =
            base.filter(|base| base.of.serial == source.serial && base.hidden == self.seen.hidden);
        let mut counts = [0, 0];
        let mut kept = 0;
        for (at, table) in tables.iter().enumerate() {
            let rows = table.entities().len();
            let keeps = like.is_some_and(|base| {
                base.rows.get(at) == Some(&rows) && !table.changed_since(base.of.change_tick)
            });
            if keeps {
                kept += 1;
                continue;
            }
            if kept > 0 {
                writeln!(text, "kept tables {kept}").expect(line);
                counts[1] += kept;
                kept = 0;
            }
            text.push_str(self.table(world, at)?);
            counts[0] += 1;
        }
        if kept > 0 {
            writeln!(text, "kept tables {kept}").expect(line);
            counts[1] += kept;
        }
        let parent = world.component::<Parent>();
        let children_kept = like.is_some_and(|base| {
            base.rows.len() == tables.len()
                && tables.iter().zip(&base.rows).all(|(table, &rows)| {
                    let reparented = parent.and_then(|parent| table.changed(parent));
                    table.entities().len() == rows
                        && !table.joined().since(base.of.change_tick)
                        && !reparented.is_some_and(|changed| changed.since(base.of.change_tick))
                })
        });
        if children_kept {
            text.push_str("kept children\n");
        } else {
            if self.children.is_none() {
                let mut children = String::new();
                world.save_children(&mut children, &self.seen.hidden);
                self.children = Some(children);
            }
            text.push_str(self.children.as_deref().expect("written"));
        }
        for (name, now) in &self.resources {
            if base.and_then(|base| resource(&base.text, name)) == Some(now.as_str()) {
                writeln!(text, "kept resource {name}").expect(line);
            } else {
                writeln!(text, "resource {name} {now}").expect(line);
            }
        }
        let compressed = match base {
            Some(base) => zstd::bulk::Compressor::with_dictionary(LEVEL, base.text.as_bytes())
                .and_then(|mut compressor| compressor.compress(text.as_bytes())),
            None => zstd::bulk::compress(text.as_bytes(), LEVEL),
        };
        let bytes = compressed.map_err(|error| {
            Error::new("net.link", "can't compress a client's state").with_source(error)
        })?;
        // Its own base holds what each `kept` line names, so that only a text past `MAX_WORLD`
        // fails, which no client would take.
        let whole = rebuild(base.map_or("", |base| &base.text), &text).map_err(|why| {
            Error::new("net.link", format!("can't write a client's state: {why}"))
        })?;
        Ok(State {
            bytes,
            size: text.len(),
            whole: Arc::new(Base {
                text: whole,
                of: source,
                rows: tables.iter().map(|table| table.entities().len()).collect(),
                hidden: self.seen.hidden.clone(),
            }),
            tables: counts,
        })
    }
}

/// The text of the resource `name` in `text`, a world's whole, as its `resource` line has it.
fn resource<'t>(text: &'t str, name: &str) -> Option<&'t str> {
    // Its lines last, after the tables' and the children's.
    let mut lines = text.lines().rev();
    let mut resources = lines
        .by_ref()
        .map_while(|line| line.strip_prefix("resource "));
    resources.find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
}

/// The whole text of the state `state`, against `base`, its base's whole text, `""` for none: its
/// lines, but for each `kept` line, in place of which goes what it names of `base`, as the module
/// says.
///
/// Fails, saying why, for a `kept tables` that names more tables than `base` holds after those
/// before, a `kept resource` that names one `base` hasn't, and a text past [`MAX_WORLD`] bytes.
pub(super) fn rebuild(base: &str, state: &str) -> std::result::Result<String, String> {
    let sections = Sections::of(base);
    let mut out = String::with_capacity(base.len().max(state.len()));
    let mut table = 0usize;
    for line in state.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        if let Some(count) = bare.strip_prefix("kept tables ") {
            let count: usize = count
                .parse()
                .map_err(|_| format!("`{bare}` names no count"))?;
            let tables = (table.checked_add(count))
                .and_then(|end| sections.tables.get(table..end))
                .ok_or_else(|| {
                    format!(
                        "`{bare}` names more tables than its base's {}, after the {table} before",
                        sections.tables.len()
                    )
                })?;
            if let (Some(first), Some(last)) = (tables.first(), tables.last()) {
                out.push_str(&base[first.start..last.end]);
            }
            table += count;
        } else if bare == "kept children" {
            out.push_str(&base[sections.children.clone()]);
        } else if let Some(name) = bare.strip_prefix("kept resource ") {
            let mut resources = sections.resources.iter();
            let (_, line) = (resources.find(|&&(of, _)| of == name))
                .ok_or_else(|| format!("`{bare}` names a resource its base hasn't"))?;
            out.push_str(&base[line.clone()]);
        } else {
            if bare == "table" || bare.starts_with("table ") {
                table += 1;
            }
            out.push_str(line);
        }
        if out.len() > MAX_WORLD {
            return Err(format!("its text whole is past {MAX_WORLD} bytes"));
        }
    }
    Ok(out)
}

/// Where the parts of a world's whole text are in it, by byte: each table's, its `table` line, its
/// rows and their `opaque` lines; the `children` lines; and each resource's line, by its name.
struct Sections<'t> {
    tables: Vec<Range<usize>>,
    children: Range<usize>,
    resources: Vec<(&'t str, Range<usize>)>,
}

impl<'t> Sections<'t> {
    fn of(text: &'t str) -> Self {
        let mut sections = Self {
            tables: Vec::new(),
            children: 0..0,
            resources: Vec::new(),
        };
        let mut children = None::<Range<usize>>;
        let mut at = 0;
        for line in text.split_inclusive('\n') {
            let range = at..at + line.len();
            at = range.end;
            let mut words = line.trim_end_matches('\n').split(' ');
            match words.next() {
                Some("table") => sections.tables.push(range),
                Some("row" | "opaque") => {
                    if let Some(table) = sections.tables.last_mut() {
                        table.end = range.end;
                    }
                }
                Some("children") => match &mut children {
                    Some(children) => children.end = range.end,
                    None => children = Some(range),
                },
                Some("resource") => {
                    if let Some(name) = words.next() {
                        sections.resources.push((name, range));
                    }
                }
                _ => {}
            }
        }
        sections.children = children.unwrap_or(0..0);
        sections
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::DVec3;

    use super::*;
    use crate::cells::CellState;
    use crate::reflect::tests::everything;
    use crate::{Log, Operation, Owner, Saved, Transform};

    /// A resource that saves itself, a count.
    #[derive(Clone)]
    struct Count(u32);

    impl Saved for Count {
        const NAME: &'static str = "Count";

        fn save(&self, _: &World) -> Result<String> {
            Ok(self.0.to_string())
        }

        fn load(text: &str, _: &World) -> Result<Self> {
            text.parse()
                .map(Self)
                .map_err(|_| Error::new("count", "not a count"))
        }
    }

    /// A transform at `x`.
    fn at(x: f64) -> Transform {
        Transform {
            translation: DVec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        }
    }

    /// The text of what nothing hides of `world`, whole.
    fn whole(world: &World) -> String {
        let mut text = String::new();
        (world.save_seen(&mut text, Some(&Seen::default()))).expect("saved");
        text
    }

    /// The state of player 0's client, which sees all of `world`, a server's after a tick, against
    /// `base`, as the thread writes it, the world's change tick then moved on, as a server's is;
    /// checked as a client reads it: rebuilt, it's the world's text whole, and, loaded, hashes as
    /// the state says. Returns its text, and how many tables it wrote and kept; and has `base` be
    /// the state.
    fn next(world: &mut World, base: &mut Option<(u64, Arc<Base>)>) -> (String, [u64; 2]) {
        let wanted = [Wanted {
            client: 0,
            player: 0,
            had: BTreeSet::new(),
            base: base.clone(),
            next: 0,
            lead: 0,
        }];
        let mut written = write(world, Source::of(world), &wanted).expect("written");
        let written = written.pop().expect("a client's");
        let state = &written.state;
        let text = read(state, base.as_ref().map(|(_, base)| &**base));
        let against = base.as_ref().map_or("", |(_, base)| &base.text);
        let rebuilt = rebuild(against, &text).expect("rebuilt");
        assert_eq!(rebuilt, whole(world));
        assert_eq!(rebuilt, state.whole.text);
        let mut loaded = world.clone();
        (loaded.load_whole((1..).zip(rebuilt.lines()), true)).expect("loaded");
        assert_eq!(loaded.hash(), written.hash);
        let tick = base.as_ref().map_or(0, |&(tick, _)| tick + 1);
        *base = Some((tick, Arc::clone(&state.whole)));
        world.advance();
        (text, state.tables)
    }

    /// The text of `state`, against `base`, as the client decompresses it.
    fn read(state: &State, base: Option<&Base>) -> String {
        let text = match base {
            Some(base) => zstd::bulk::Decompressor::with_dictionary(base.text.as_bytes())
                .and_then(|mut decompressor| decompressor.decompress(&state.bytes, state.size)),
            None => zstd::bulk::decompress(&state.bytes, state.size),
        };
        String::from_utf8(text.expect("decompressed")).expect("text")
    }

    /// The `table` lines of `text`.
    fn tables(text: &str) -> usize {
        let tables = text
            .lines()
            .filter(|line| line.split(' ').next() == Some("table"));
        tables.count()
    }

    /// A state holds the tables that changed since its base, as their marks say, or whose rows are
    /// fewer, the last taken out; the children, as a row joins or leaves or a parent changes; and a
    /// resource whose text changed: and names the rest as kept. Rebuilt, it's the world's text whole.
    #[test]
    fn a_state_holds_what_changed_since_its_base() {
        let mut world = World::seeded(1);
        world.register_saved::<Count>().expect("registered");
        world.insert_resource(Count(1));
        let parent = world.spawn(at(0.0));
        let child = world.spawn(at(1.0));
        let other = world.spawn(at(2.0));
        world.set_parent(child, Some(parent)).expect("parented");
        let [mover, last] = [3.0, 4.0].map(|x| world.spawn((at(x), Owner { player: 0 })));
        let count = world.tables().len() as u64;
        world.advance();
        let mut base = None;
        let (text, written) = next(&mut world, &mut base);
        assert!(!text.contains("kept"), "{text}");
        assert_eq!(written, [count, 0]);

        // A transform moved, and the count: their table and resource alone.
        world
            .get_mut::<Transform>(mover)
            .expect("alive")
            .translation
            .x = 5.0;
        world.resource_mut::<Count>().expect("inserted").0 = 2;
        let (text, written) = next(&mut world, &mut base);
        assert_eq!((tables(&text), written), (1, [1, count - 1]), "{text}");
        assert!(text.contains("kept children\n") && text.contains("resource Count 2\n"));

        // The last row taken out, which marks nothing: its table, and the children.
        world.despawn(last).expect("alive");
        let (text, written) = next(&mut world, &mut base);
        assert_eq!((tables(&text), written), (1, [1, count - 1]), "{text}");
        assert!(!text.contains("kept children") && text.contains("kept resource Count\n"));

        // A child given another parent: its table, and the children.
        world.set_parent(child, Some(other)).expect("parented");
        let (text, _) = next(&mut world, &mut base);
        assert_eq!(tables(&text), 1, "{text}");
        assert!(!text.contains("kept children"), "{text}");

        // Nothing: all kept.
        let (text, written) = next(&mut world, &mut base);
        assert_eq!((tables(&text), written), (0, [0, count]), "{text}");
        assert!(text.contains("kept children\n") && text.contains("kept resource Count\n"));
    }

    /// An entity a delete took, which an undo puts back, is named again by the rows that named it,
    /// written `Dead` meanwhile, though nothing reached their columns: their tables are written,
    /// and the client's world, loaded from the state, names it as the server's does.
    #[test]
    fn an_entity_put_back_has_the_rows_naming_it_written() {
        use crate::reflect::tests::Everything;

        let mut world = World::seeded(1);
        let target = world.spawn(at(0.0));
        let mut naming = everything();
        naming.target = Some(target);
        let naming = world.spawn(naming);
        world.advance();
        let mut base = None;
        next(&mut world, &mut base);
        let mut log = Log::default();
        let id = world.id(target).expect("alive");
        (log.apply(&mut world, Operation::Delete { entity: id })).expect("deleted");
        // Written while it's out, as its count changes too.
        world.get_mut::<Everything>(naming).expect("alive").count += 1;
        let (text, _) = next(&mut world, &mut base);
        assert!(text.contains("Some(Dead)"), "{text}");
        assert!(log.undo(&mut world).expect("undone"));
        let (text, _) = next(&mut world, &mut base);
        assert!(text.contains(&format!("Some(Entity(\"{id}\"))")), "{text}");
    }

    /// A world a layer puts in the server's world's place, whose change tick is earlier than the
    /// world's it replaced, has what changes in it written, not kept, though the thread's copy of
    /// the world, whose change tick never falls, is past it.
    #[test]
    fn a_world_put_in_place_has_what_changes_written() {
        let mut states = States::start().expect("started");
        // As a server's tick ends: the state against `base`, rebuilt whole, and the world's change
        // tick moved on.
        let mut state = |world: &mut World, base: Option<(u64, Arc<Base>)>| {
            let wanted = vec![Wanted {
                client: 0,
                player: 0,
                had: BTreeSet::new(),
                base,
                next: 0,
                lead: 0,
            }];
            states.give(world, 0, wanted);
            let until = Instant::now() + std::time::Duration::from_secs(5);
            let (_, written) = states.take(Some(until)).expect("written in 5 s");
            let mut written = written.expect("written");
            world.advance();
            Arc::clone(&written.pop().expect("a client's").state.whole)
        };
        let mut world = World::seeded(1);
        world.spawn(at(0.0));
        for _ in 0..8 {
            world.advance();
        }
        state(&mut world, None);
        let mut world = World::seeded(1);
        let moved = world.spawn(at(0.0));
        let base = state(&mut world, None);
        world
            .get_mut::<Transform>(moved)
            .expect("alive")
            .translation
            .x = 1.0;
        let next = state(&mut world, Some((0, base)));
        assert_eq!(next.text, whole(&world));
    }

    /// A state against a base of another world, as its serial says, or whose view hid other
    /// entities, keeps no table, nor the children.
    #[test]
    fn a_state_against_another_world_or_view_keeps_nothing() {
        let mut world = World::seeded(1);
        for x in [0.0, 1.0] {
            world.spawn(at(x));
        }
        world.spawn((at(2.0), Owner { player: 0 }));
        world.advance();
        let mut all = View::of(&world, Seen::default()).expect("a view");
        let base = all
            .state(&world, Source::of(&world), None)
            .expect("a state");
        let base = base.whole;
        world.advance();
        let source = Source::of(&world);
        let kept = all.state(&world, source, Some(&base)).expect("a state");
        assert_eq!(kept.tables, [0, world.tables().len() as u64]);
        let hiding = Seen {
            hidden: vec![true],
            cells: CellState::default(),
        };
        let mut hiding = View::of(&world, hiding).expect("a view");
        let another = Source {
            serial: source.serial + 1,
            ..source
        };
        for (state, what) in [
            (hiding.state(&world, source, Some(&base)), "another view"),
            (all.state(&world, another, Some(&base)), "another world"),
        ] {
            let state = state.expect("a state");
            assert_eq!(state.tables, [world.tables().len() as u64, 0], "{what}");
            let text = read(&state, Some(&base));
            assert!(!text.contains("kept tables"), "{what}: {text}");
            assert!(!text.contains("kept children"), "{what}: {text}");
        }
    }

    /// A world's whole text, of a head, three tables, children and two resources.
    const BASE: &str = "\
stream 0000000000000001
slots 3
free
vacant
table A
row 0 1 [Int(1)]
table A B
row 1 2 [Int(2), Int(3)]
opaque 1 x
table
row 2 3 []
children 1 2
children 2 3
resource Physics2D bodies
resource Cells in
";

    /// A state rebuilds against its base: each `kept` line in place of what it names, the rest as
    /// it is; and `kept tables` past the base's tables, or a `kept resource` the base hasn't,
    /// fails.
    #[test]
    fn a_state_rebuilds_against_its_base() {
        let state = "\
stream 0000000000000002
slots 3
free
vacant
kept tables 1
table A B
row 1 2 [Int(5), Int(3)]
kept tables 1
kept children
resource Physics2D moved
kept resource Cells
";
        let whole = rebuild(BASE, state).expect("it rebuilds");
        let expected = BASE
            .replace("0001", "0002")
            .replace("Int(2), Int(3)]\nopaque 1 x", "Int(5), Int(3)]")
            .replace("bodies", "moved");
        assert_eq!(whole, expected);
        // Whole, it's itself.
        assert_eq!(rebuild("", BASE).as_deref(), Ok(BASE));
        assert_eq!(rebuild(BASE, BASE).as_deref(), Ok(BASE));
        for bad in [
            "kept tables 4\n",
            "table A\nkept tables 3\n",
            "kept tables 18446744073709551615\n",
            "kept tables x\n",
            "kept resource Audio\n",
        ] {
            assert!(rebuild(BASE, bad).is_err(), "{bad}");
        }
        assert!(rebuild("", "kept tables 1\n").is_err());
        assert_eq!(rebuild("", "kept children\n").as_deref(), Ok(""));
    }

    /// A resource's text is found by its name among a world's last lines.
    #[test]
    fn a_resource_is_found_by_its_name() {
        assert_eq!(resource(BASE, "Physics2D"), Some("bodies"));
        assert_eq!(resource(BASE, "Cells"), Some("in"));
        assert_eq!(resource(BASE, "Physics"), None);
        assert_eq!(resource(BASE, "Audio"), None);
    }
}
