//! Streaming the world's cells in and out, and preparing them off the main thread.

use std::collections::BTreeMap;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gg_core::{Error, Result};
use tracing::{debug, warn};

use super::Context;
use super::world::no_root;
use crate::Entity;
#[cfg(feature = "dev")]
use crate::World;
use crate::assets::Asset;
use crate::cells::{
    self, Cell, CellSource, CellState, Entering, Finished, Kept, MadeCells, Prep, Prepared, Stage,
    Texts,
};
#[cfg(doc)]
use crate::cells::{CellRun, Preparing};
use crate::ecs::text_digest;
use crate::package;
use crate::perf;
#[cfg(feature = "dev")]
use crate::scene_files;
use crate::scene_files::SceneFile;

impl Context {
    /// Makes the cells of a world whose [`Cells`](crate::Cells) are of folder `folder` that have no
    /// file, from `from` to `to` on each axis, as `text` gives each: a cell's scene text, as its
    /// file would hold it. So a world reaches as far as the game makes it, without a file for each
    /// cell; a cell's file stands for the game's making of it, as a development build's save writes
    /// one for a made cell that changed. A layer makes them in its
    /// [`load`](crate::Layer::load), which a restart and a replay run too; again, for the same
    /// folder, in place of what it made before.
    ///
    /// `text` runs on a thread that prepares cells, as a viewer comes near, and again whenever the
    /// cell comes in from it, so it reads nothing but its cell, and gives each cell the same text
    /// each time and on every platform, as the world's ticks must; and the IDs its entities have
    /// are theirs alone. A made cell comes in, and goes out keeping nothing as it came, as one of a
    /// file does, its text's digest noted in the world, but neither kept by the run nor written to
    /// a recording, which the world's hash checks in its place. The cells within each viewer's
    /// radius and a cell and a half are looked for, each tick, in a box of at most 32,768 cells
    /// round the viewer: so a world one cell deep reaches some 90 cells either way, and the 4,096
    /// nearest, a world's most at once, are always among them.
    pub fn make_cells(
        &mut self,
        folder: &str,
        from: Cell,
        to: Cell,
        text: impl Fn(Cell) -> String + Send + Sync + 'static,
    ) {
        let made = MadeCells {
            from,
            to,
            text: Arc::new(text),
        };
        let run = &mut self.cells.made;
        match run.iter_mut().find(|(was, _)| was == folder) {
            Some((_, was)) => *was = made,
            None => run.push((folder.to_owned(), made)),
        }
    }

    /// Takes the world's cells out and finds those coming in, as [`Cells`](crate::Cells),
    /// [`Viewer`](crate::Viewer) and [`Streamed`](crate::Streamed) say, as each tick ends, after
    /// its layers', wherever a tick runs, again in a rollback or a SyncTest too: from the world
    /// alone, as [`World::plan_cells`] finds it, so that every run of the tick does the same. Cells
    /// go out first, in order, then the roots left in cells out; and the cells coming in are noted
    /// in the world, which the next tick brings in as it starts, as
    /// [`enter_coming`](Self::enter_coming) says, so that a tick whose cells aren't ready can wait
    /// for them before it runs.
    ///
    /// Logs, at the debug level, `cells.left`, with the cell, how many roots went and how long it
    /// took. A cell whose roots can't be kept as text stays in, warning `cells.held`. A server's
    /// client takes none out: its cells come and go with its server's states. A world without
    /// `Cells` costs a query; one whose cells stay as they are, the plan, whose room [`CellRun`]
    /// keeps from tick to tick.
    pub(crate) fn stream_cells(&mut self) {
        if self.client {
            return;
        }
        let Some((id, cells, count)) = self.world.world_cells() else {
            return;
        };
        let tie = (count > 1).then_some((id, count));
        if tie.is_some() && tie != self.cells.tie {
            warn!(
                code = "cells.tie",
                seen = %id,
                count,
                "{count} entities have Cells: the world streams by the one of the lowest ID, {id}"
            );
        }
        self.cells.tie = tie;
        let size = cells.size;
        if !(size.is_finite() && size > 0.0) {
            if self.cells.first("cells.bad_size", id) {
                warn!(
                    code = "cells.bad_size",
                    entity = %id,
                    size,
                    "The Cells of entity {id} are {size} units on a side, which isn't finite and \
                     positive, so no cell comes in or goes out"
                );
            }
            return;
        }
        let run = &mut self.cells;
        let listed = run.listed(self.root.as_deref(), &self.files, &cells.folder);
        let made = run.made(&cells.folder).map(|made| (made.from, made.to));
        self.world
            .plan_cells(size, &listed, made, None, &mut run.plan);
        let plan = mem::take(&mut run.plan);
        // Those no viewer is near any more.
        (run.preparing.cells).retain(|cell, _| plan.nearby.binary_search(cell).is_ok());
        let size = f64::from(size);
        let unprepared = |prepared: &BTreeMap<Cell, Prep>, &(past, cell): &(f64, Cell)| {
            past <= size && !prepared.contains_key(&cell)
        };
        let prepared = &self.cells.preparing.cells;
        if plan.is_empty() && !plan.ahead.iter().any(|ahead| unprepared(prepared, ahead)) {
            self.cells.plan = plan;
            return;
        }
        let folder = cells.folder.clone();
        let of = roots_of;
        for &cell in &plan.leaving {
            let started = Instant::now();
            let roots = of(&plan.leavers, cell);
            match self.world.keep_cell(cell, &roots, true) {
                Ok(roots) => debug!(
                    code = "cells.left",
                    cell = %cell,
                    roots,
                    ms = perf::millis(started.elapsed()),
                    "A cell went out"
                ),
                Err(error) => self.held(cell, &error),
            }
        }
        let mut strays = plan.strays.iter().map(|&(cell, _)| cell).peekable();
        while let Some(cell) = strays.next() {
            while strays.next_if_eq(&cell).is_some() {}
            let roots = of(&plan.strays, cell);
            match self.world.keep_cell(cell, &roots, false) {
                // What was prepared stands for what it kept before.
                Ok(_) => drop(self.cells.preparing.cells.remove(&cell)),
                Err(error) => self.held(cell, &error),
            }
        }
        self.world.note_coming(&plan.entering);
        self.world.forget_refused(&plan.forgotten);
        if plan.full && self.cells.first("cells.full", &folder) {
            let most = cells::MOST_CELLS;
            warn!(
                code = "cells.full",
                folder = &*folder,
                most,
                "More cells are near the viewers than the {most} a world holds at once: the \
                 nearest come in"
            );
        }
        let prepared = &self.cells.preparing.cells;
        let prepare = plan
            .ahead
            .iter()
            .filter(|ahead| unprepared(prepared, ahead));
        let prepare: Vec<Cell> = prepare.map(|&(_, cell)| cell).collect();
        for cell in prepare {
            let source = self.cells.source(&folder, &listed, cell);
            self.start_preparing(&folder, cell, source);
        }
        self.cells.plan = plan;
    }

    /// The cells' step while the run is stopped, as a world is edited, each frame, since no tick
    /// runs: as [`stream_cells`](Self::stream_cells) does as a tick ends, the look's viewpoint a
    /// viewer too, its radius the largest of the world's viewers' and [`VIEWED_RADIUS`]; but the
    /// cells coming in come once all of them are prepared, as [`cells_ready`](Self::cells_ready)
    /// prepares them, no frame waiting; a cell that a streamed root is in comes in wherever it is,
    /// so that a root moved into a cell brings the cell's file in with it, rather than going out
    /// into what the cell keeps; and a cell no viewer is near goes out only as its file has it, its
    /// roots written as a world's save would write its file the same as the file as last applied,
    /// as [`World::cell_written`] says, keeping nothing, as [`World::forget_cell`] says, logging
    /// `cells.left` at the debug level. So what's edited stays in until it's saved, and nothing
    /// edited is kept as text, which a save would have to write. A server's client streams none.
    #[cfg(feature = "dev")]
    pub(crate) fn stream_stopped(&mut self) {
        if self.client {
            return;
        }
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let size = cells.size;
        // The tick's step warns of it, as the copy plays.
        if !(size.is_finite() && size > 0.0) {
            return;
        }
        let folder = cells.folder.clone();
        // Every frame, as every tick, so that what's prepared ahead goes on from stage to stage.
        self.prepare_cells();
        let state = self.world.resource::<CellState>();
        let mut changed = false;
        if state.is_some_and(|state| !state.coming.is_empty()) && self.cells_ready() {
            self.enter_coming();
            changed = true;
        }
        let viewers = self.world.query::<(Entity, &cells::Viewer)>();
        let radius = viewers.fold(VIEWED_RADIUS, |most, (_, viewer)| most.max(viewer.radius));
        let viewer = (self.dev.look.viewpoint).map(|seen| {
            let at = seen.position.to_array().map(f64::from);
            (at, f64::from(radius))
        });
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        let made = self.cells.made(&folder).map(|made| (made.from, made.to));
        let mut plan = mem::take(&mut self.cells.plan);
        self.world
            .plan_cells(size, &listed, made, viewer, &mut plan);
        // The cells that streamed roots are in, out and neither coming nor refused, come in with
        // those near a viewer.
        let mut entering = plan.entering.clone();
        let state = self.world.resource::<CellState>();
        let waiting = |cell: &Cell| {
            state.is_some_and(|state| state.coming.contains(cell) || state.refused.contains(cell))
        };
        let strays = plan.strays.iter().map(|&(cell, _)| cell);
        entering.extend(strays.filter(|cell| !waiting(cell)));
        entering.sort_unstable();
        entering.dedup();
        self.world.note_coming(&entering);
        self.world.forget_refused(&plan.forgotten);
        changed |= !entering.is_empty() || !plan.forgotten.is_empty();
        let changes = self.scenes.changes;
        if self.cells.held.1 != changes {
            self.cells.held = (Vec::new(), changes);
        }
        for &cell in &plan.leaving {
            if self.cells.held.0.contains(&cell) {
                continue;
            }
            let started = Instant::now();
            let roots = roots_of(&plan.leavers, cell);
            let state = self.world.resource::<CellState>();
            let came = state.is_some_and(|state| state.files.contains_key(&cell));
            let source = self.cells.source(&folder, &listed, cell);
            let file = came.then(|| self.source_text(&folder, cell, source));
            let same = match file.transpose() {
                Ok(file) => matches!(self.world.cell_written(file.as_deref(), &roots), Ok(None)),
                Err(_) => false,
            };
            if !same {
                self.cells.held.0.push(cell);
                continue;
            }
            self.world.forget_cell(cell, &roots);
            changed = true;
            debug!(
                code = "cells.left",
                cell = %cell,
                roots = roots.len(),
                ms = perf::millis(started.elapsed()),
                "A cell went out as its file has it"
            );
        }
        let coming = |cell: &Cell, world: &World| {
            let state = world.resource::<CellState>();
            state.is_some_and(|state| state.coming.contains(cell))
        };
        let (preparing, world) = (&mut self.cells.preparing.cells, &self.world);
        preparing.retain(|cell, _| plan.nearby.binary_search(cell).is_ok() || coming(cell, world));
        let size = f64::from(size);
        let prepare = (plan.ahead.iter())
            .filter(|&&(past, cell)| past <= size && !preparing.contains_key(&cell))
            .map(|&(_, cell)| cell);
        let prepare: Vec<Cell> = prepare.collect();
        for cell in prepare {
            let source = self.cells.source(&folder, &listed, cell);
            self.start_preparing(&folder, cell, source);
        }
        self.cells.plan = plan;
        if changed {
            self.scenes.changed();
        }
    }

    /// Nothing: a build without `dev` never stops.
    #[cfg(not(feature = "dev"))]
    pub(crate) fn stream_stopped(&mut self) {}

    /// The world's cells, as the tool API's `cells` reads them, the world as edited at `ticks`, as
    /// [`scene_files::unsaved_cells`] finds those a save would write; `stopped` whether the run is,
    /// as the look's viewpoint is a viewer then, as [`stream_stopped`](Self::stream_stopped) says.
    #[cfg(feature = "dev")]
    pub(crate) fn cells_read(&mut self, stopped: bool, ticks: u64) -> gg_api::CellList {
        let Some((id, cells, _)) = self.world.world_cells() else {
            return gg_api::CellList::default();
        };
        let (entity, folder, size) = (id.to_string(), cells.folder.clone(), cells.size);
        let mut viewers = Vec::new();
        let mut radius = VIEWED_RADIUS;
        for (entity, viewer) in self.world.query::<(Entity, &cells::Viewer)>() {
            radius = radius.max(viewer.radius);
            let at = self.world.pose(entity).expect("alive").matrix.translation;
            viewers.push(gg_api::ViewerAt {
                entity: self.world.id(entity).map(|id| id.to_string()),
                position: at.to_array(),
                radius: viewer.radius,
            });
        }
        if stopped
            && !self.client
            && let Some(seen) = self.dev.look.viewpoint
        {
            viewers.push(gg_api::ViewerAt {
                entity: None,
                position: seen.position.to_array(),
                radius,
            });
        }
        let unsaved = scene_files::unsaved_cells(self, ticks).to_vec();
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        let mut roots = Vec::new();
        if size.is_finite() && size > 0.0 {
            self.world.streamed_roots(size, &mut roots);
        }
        let empty = CellState::default();
        let state = self.world.resource::<CellState>().unwrap_or(&empty);
        let mut all: Vec<Cell> = (listed.iter())
            .chain(&state.cells_in)
            .chain(&state.coming)
            .chain(state.kept.keys())
            .chain(&state.refused)
            .copied()
            .chain(roots.iter().map(|&(cell, ..)| cell))
            .collect();
        all.sort_unstable();
        all.dedup();
        let made = self.cells.made(&folder);
        let cells = all.into_iter().map(|cell| {
            let from = roots.partition_point(|&(other, ..)| other < cell);
            let to = roots.partition_point(|&(other, ..)| other <= cell);
            let file = listed.binary_search(&cell).is_ok();
            gg_api::CellAt {
                cell: cell.0.to_array(),
                file,
                made: !file && made.is_some_and(|made| made.makes(cell)),
                is_in: state.cells_in.contains(&cell),
                coming: state.coming.contains(&cell),
                refused: state.refused.contains(&cell),
                kept: state.kept.get(&cell).map_or(0, |kept| kept.texts.len()),
                roots: to - from,
                unsaved: unsaved.binary_search(&cell).is_ok(),
            }
        });
        let world = gg_api::WorldCells {
            entity,
            folder,
            size,
            viewers,
            cells: cells.collect(),
        };
        gg_api::CellList { world: Some(world) }
    }

    /// Lists the world's cells' folder again, as a development build's look at its scene files
    /// does while the run can stop, so that a cell's file made since brings its cell in, and one
    /// taken away no longer does. A folder that can't be listed keeps the cells it had, its first
    /// listing having warned (`cells.listing`).
    #[cfg(feature = "dev")]
    pub(crate) fn list_cells_again(&mut self) {
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let folder = cells.folder.as_str();
        let Ok(listed) = cells::list_cells(self.root.as_deref(), &self.files, folder) else {
            return;
        };
        match (self.cells.listed.iter_mut()).find(|(listed, _)| listed == folder) {
            Some((_, was)) => *was = listed,
            None => self.cells.listed.push((folder.to_owned(), listed)),
        }
    }

    /// Notes that each of `cells`' files, of its digest, holds what the world as edited holds of
    /// its cell, as a world's save leaves them, as [`World::note_filed`] says: of the world held
    /// aside while a copy plays, and else of the context's own, between ticks.
    #[cfg(feature = "dev")]
    pub(crate) fn note_filed(&mut self, cells: impl IntoIterator<Item = (Cell, u64)>) {
        let world = match &mut self.dev.edited {
            Some(edited) => &mut edited.world,
            None => &mut self.world,
        };
        for (cell, digest) in cells {
            world.note_filed(cell, digest);
        }
    }

    /// Empties what the world holds of its cells and what the run prepared of them, as the tool
    /// API's `scene new` and `open` put another scene in a world's place, so that the next world's
    /// cells come in from their files.
    #[cfg(feature = "dev")]
    pub(crate) fn empty_cells(&mut self) {
        self.world.empty_cells();
        self.cells.preparing.cells.clear();
        self.cells.held = Default::default();
    }

    /// Brings in the cells the last tick's end found coming, as
    /// [`stream_cells`](Self::stream_cells) says, as a tick starts, before its layers', wherever a
    /// tick runs: each in order, its file read, unless what the cell keeps stands for it, as
    /// [`scene_text`](Self::scene_text) keeps it, once a run, and what it and the texts the cell
    /// keeps name loaded first, as [`load_assets_of`](Self::load_assets_of) does for a scene
    /// change, in a server's session none; each text parsed, and the cell's entities spawned. A
    /// world whose `Cells` went meanwhile drops them. A server's client brings none in.
    ///
    /// Logs, at the debug level, `cells.entered`, with the cell, how many roots came, whether it
    /// came as prepared ahead, as [`cells_ready`](Self::cells_ready) prepares it, and how long it
    /// took; and notes, for a recording, the digest of each cell's file that came in. A cell that
    /// doesn't come in, as one whose file gives an entity's ID that's alive (`ecs.id_taken`) or
    /// names a model that doesn't load, stays out, warning `cells.refused`, with why, until every
    /// viewer has gone.
    pub(crate) fn enter_coming(&mut self) {
        self.cells.came.clear();
        if self.client {
            return;
        }
        let coming = self.world.take_coming();
        if coming.is_empty() {
            return;
        }
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let folder = cells.folder.clone();
        let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
        for cell in coming {
            let started = Instant::now();
            let source = self.cells.source(&folder, &listed, cell);
            let filed = matches!(source, Some(CellSource::File));
            match self.enter_cell(&folder, cell, source) {
                Ok((roots, file, prepared)) => {
                    // A text the game made isn't a file's, which a recording would name.
                    let file = file.filter(|_| filed);
                    self.cells.came.extend(file.map(|digest| (cell, digest)));
                    debug!(
                        code = "cells.entered",
                        cell = %cell,
                        roots,
                        prepared,
                        ms = perf::millis(started.elapsed()),
                        "A cell came in"
                    );
                }
                Err(error) => {
                    if self.cells.first("cells.refused", cell) {
                        warn!(
                            code = "cells.refused",
                            cell = %cell,
                            refused = error.code(),
                            "{error:#}; the cell stays out until every viewer has gone"
                        );
                    }
                    self.world.refuse_cell(cell);
                }
            }
        }
    }

    /// Brings cell `cell`, of the cells of `folder`, in, as
    /// [`enter_coming`](Self::enter_coming) says: its file read, or the game's making of it, as
    /// `source` says, and what it names loaded, unless what the cell keeps stands for it, and what
    /// each text it keeps names; or as prepared ahead, if what was prepared stands for its texts
    /// still, as [`stands_for`](Self::stands_for) says. Returns how many roots came, the digest of
    /// its file's text, if it came, and whether it came as prepared.
    ///
    /// Fails, bringing nothing in, as [`scene_text`](Self::scene_text) does, as
    /// [`load_assets_of`](Self::load_assets_of) does, and as [`World::parse_scene`] and
    /// [`World::enter_cell`] do, the cell's path beginning the message.
    fn enter_cell(
        &mut self,
        folder: &str,
        cell: Cell,
        source: Option<CellSource>,
    ) -> Result<(usize, Option<u64>, bool)> {
        let path = format!("{folder}/{cell}.ron");
        let prepared = match self.cells.preparing.cells.remove(&cell) {
            Some(Prep {
                stage: Stage::Ready(Ok(prepared)),
                ..
            }) => Some(prepared),
            _ => None,
        };
        let state = self.world.resource::<CellState>();
        let came = state.is_some_and(|state| state.files.contains_key(&cell));
        let kept = state.and_then(|state| state.kept.get(&cell)).cloned();
        let kept = kept.unwrap_or_default();
        let named = |error: Error| {
            let message = format!("{path}:{error}");
            error.with_message(message)
        };
        let source = source.filter(|_| !came);
        if let Some(prepared) = prepared
            && self.stands_for(&prepared, &path, source.as_ref(), &kept)
        {
            let file = prepared.file;
            let roots = self
                .world
                .enter_cell(cell, prepared.entering)
                .map_err(named)?;
            return Ok((roots, file, true));
        }
        let file = match source {
            Some(source) => Some(self.source_text(folder, cell, Some(source))?),
            None => None,
        };
        let texts = file.iter().map(String::as_str);
        for text in texts.chain(kept.texts.iter().map(|kept| &*kept.text)) {
            self.load_assets_of(&path, text)?;
        }
        let parse = |text: &str| self.world.parse_scene(text).map_err(named);
        let came_as = match &file {
            Some(text) if kept.texts.is_empty() => cells::came_as(&self.world, text),
            _ => None,
        };
        let file = file
            .as_deref()
            .map(|text| Ok((text_digest(text), parse(text)?)));
        let entering = Entering {
            file: file.transpose()?,
            kept: kept
                .texts
                .iter()
                .map(|kept| parse(&kept.text))
                .collect::<Result<_>>()?,
            came_as,
        };
        let file = entering.file.as_ref().map(|&(digest, _)| digest);
        let roots = self.world.enter_cell(cell, entering).map_err(named)?;
        Ok((roots, file, false))
    }

    /// Whether `prepared`, cell `path`'s, was parsed from the texts the cell would bring in now:
    /// what its `source` gives, its file's, as [`scene_text`](Self::scene_text) gives it, or the
    /// game's making of it, for a cell whose file is yet to come in; and those it keeps, `kept`. A
    /// development build's save of the file since, a file made for a made cell, or a root left in
    /// the cell since, makes it stand for them no more.
    fn stands_for(
        &mut self,
        prepared: &Prepared,
        path: &str,
        source: Option<&CellSource>,
        kept: &Kept,
    ) -> bool {
        let kept = kept.texts.iter().map(|kept| kept.digest);
        if !prepared.kept.iter().copied().eq(kept)
            || prepared.file.is_some() != source.is_some()
            || prepared.made != matches!(source, Some(CellSource::Made(_)))
        {
            return false;
        }
        match (prepared.file, source) {
            (Some(digest), Some(CellSource::File)) => {
                (self.scene_text(path)).is_ok_and(|text| text_digest(&text) == digest)
            }
            // The game makes a cell the same each time.
            _ => true,
        }
    }

    /// The text cell `cell` of the cells of `folder` comes in from, as `source` says: its file's, as
    /// [`scene_text`](Self::scene_text) gives it, or what the game makes of it, unless the run
    /// keeps a file of it, as a development build's save of a made cell that changed writes one.
    ///
    /// Fails as `scene_text` does, and with `scene.not_found` for a cell of neither.
    pub(crate) fn source_text(
        &mut self,
        folder: &str,
        cell: Cell,
        source: Option<CellSource>,
    ) -> Result<String> {
        let path = format!("{folder}/{cell}.ron");
        let cached = self.scenes.cached.iter().find(|file| file.author == path);
        match source {
            Some(CellSource::File) => self.scene_text(&path),
            Some(CellSource::Made(_)) if let Some(file) = cached => Ok(file.text.clone()),
            Some(CellSource::Made(made)) => Ok((made.text)(cell)),
            None => {
                let message = format!("there's no scene {path}, nor does the game make the cell");
                Err(Error::new("scene.not_found", message))
            }
        }
    }

    /// Starts preparing cell `cell`, of the cells of `folder`, as [`Preparing`] says: on a thread,
    /// its file read, as the run keeps it or from the disk or the game's package, or the game's
    /// making of it, as `source` says, if it's yet to come in, and the texts it keeps taken with
    /// it.
    fn start_preparing(&mut self, folder: &str, cell: Cell, source: Option<CellSource>) {
        let path = format!("{folder}/{cell}.ron");
        let state = self.world.resource::<CellState>();
        let came = state.is_some_and(|state| state.files.contains_key(&cell));
        let kept = (state.and_then(|state| state.kept.get(&cell)))
            .map(|kept| {
                let texts = kept.texts.iter();
                texts
                    .map(|kept| (Arc::clone(&kept.text), kept.digest))
                    .collect()
            })
            .unwrap_or_default();
        let source = source.filter(|_| !came);
        let made = matches!(source, Some(CellSource::Made(_)));
        let read = source.map(|source| {
            let cached = self.scenes.cached.iter().find(|file| file.author == path);
            let cached: Option<Arc<str>> = cached.map(|file| file.text.as_str().into());
            let (root, files, scene) = (self.root.clone(), self.files.clone(), path.clone());
            move || {
                if let Some(text) = cached {
                    return Ok(text);
                }
                if let CellSource::Made(made) = source {
                    return Ok((made.text)(cell).into());
                }
                let root = root.ok_or_else(|| no_root(&scene))?;
                let at = package::under(&root, &scene);
                Self::read_scene_text(&files, &root, &scene, &at, true).map(Arc::from)
            }
        });
        let world = self.world.scratch();
        let number = self.cells.preparing.begin(cell);
        (self.cells.preparing).read((cell, number), (path, made), read, kept, world);
    }

    /// Takes what the threads preparing cells have finished, and starts the next stage of each:
    /// for a cell whose texts name what isn't loaded, its models' files read on threads, then the
    /// rest loaded here, as [`load_assets_of`](Self::load_assets_of) loads it for a scene change,
    /// none in a server's session, and its texts parsed on a thread. A cell prepared, or that
    /// couldn't be, logs `cells.prepared` at the debug level, with how long it took and how long
    /// the main thread spent on it. A file read is kept as [`scene_text`](Self::scene_text) keeps
    /// it, unless the run has it already.
    pub(crate) fn prepare_cells(&mut self) {
        while let Some(done) = self.cells.preparing.take() {
            let started = Instant::now();
            let ready = match done {
                Finished::Read {
                    cell,
                    number,
                    texts,
                    missing,
                    parsed,
                } => {
                    if self.cells.preparing.prep(cell, number).is_none() {
                        continue;
                    }
                    // A text the game made it makes again.
                    if let Ok(Texts {
                        path,
                        file: Some((text, _)),
                        made: false,
                        ..
                    }) = &texts
                        && let Some(root) = &self.root
                        && !self.scenes.cached.iter().any(|file| file.author == *path)
                    {
                        let at = package::under(root, path);
                        let file = SceneFile::new(at, path.clone(), text.to_string());
                        self.scenes.cache(file);
                    }
                    let stage = match (texts, parsed) {
                        (Err(error), _) => Stage::Ready(Err(error)),
                        (Ok(_), Some(parsed)) => Stage::Ready(parsed),
                        (Ok(texts), None) => self.load_for((cell, number), texts, &missing),
                    };
                    vec![(cell, number, stage)]
                }
                Finished::Model { path, read } => {
                    self.cells.preparing.model_read(path.clone(), read);
                    let mut loaded = Vec::new();
                    for (&cell, prep) in &mut self.cells.preparing.cells {
                        if let Stage::Loading { models, .. } = &mut prep.stage {
                            models.retain(|model| *model != path);
                            if models.is_empty() {
                                loaded.push(cell);
                            }
                        }
                    }
                    let mut stages = Vec::new();
                    for cell in loaded {
                        let prep = self.cells.preparing.cells.get_mut(&cell).expect("loading");
                        let Stage::Loading { texts, .. } =
                            mem::replace(&mut prep.stage, Stage::Parsing)
                        else {
                            unreachable!("loading, as found above");
                        };
                        let number = prep.number();
                        let stage = self.finish_loading((cell, number), texts);
                        stages.push((cell, number, stage));
                    }
                    stages
                }
                Finished::Parsed {
                    cell,
                    number,
                    parsed,
                } => vec![(cell, number, Stage::Ready(parsed))],
            };
            let took = started.elapsed();
            for (cell, number, stage) in ready {
                let Some(prep) = self.cells.preparing.prep(cell, number) else {
                    continue;
                };
                prep.main += took;
                prep.stage = stage;
                if let Stage::Ready(prepared) = &prep.stage {
                    debug!(
                        code = "cells.prepared",
                        cell = %cell,
                        refused = prepared.as_ref().err().map(Error::code),
                        ms = perf::millis(prep.started.elapsed()),
                        main_ms = perf::millis(prep.main),
                        "A cell was prepared to come in"
                    );
                }
            }
        }
        self.cells.preparing.forget_models();
    }

    /// The stage after cell `cell`'s texts, `texts`, were found to name `missing`, which isn't
    /// loaded, for its preparing numbered `number`: the files of the models among them read on
    /// threads, which it waits for, unless none is to read, as in a server's session, which loads
    /// none; then, as [`finish_loading`](Self::finish_loading) says.
    fn load_for(
        &mut self,
        (cell, number): (Cell, u64),
        texts: Texts,
        missing: &[Asset<String>],
    ) -> Stage {
        let mut models = Vec::new();
        for asset in missing {
            let Asset::Model(name) = asset else {
                continue;
            };
            // What fails here, `load_assets_of` says as it loads them.
            let Ok(path) = self.asset_path(name, "model", false) else {
                continue;
            };
            let file = match &self.root {
                Some(root) => root.join(&*path),
                None => PathBuf::from(&*path),
            };
            if self.served
                || self.assets.has(&asset.as_deref())
                || self.within_root(&path, &file).is_err()
            {
                continue;
            }
            (self.cells.preparing).read_model(path, file.clone(), self.files.clone());
            if self.cells.preparing.reading(&file) {
                models.push(file);
            }
        }
        match models.is_empty() {
            true => self.finish_loading((cell, number), texts),
            false => Stage::Loading { texts, models },
        }
    }

    /// The stage after what cell `cell`'s texts, `texts`, name has been read, for its preparing
    /// numbered `number`: what they name loaded, as [`load_assets_of`](Self::load_assets_of) loads
    /// it, the models' files read on threads taken; and then its texts parsed on a thread, in a
    /// scratch world that has what was loaded; or, ready, why it couldn't be loaded.
    fn finish_loading(&mut self, (cell, number): (Cell, u64), texts: Texts) -> Stage {
        for text in texts.each() {
            if let Err(error) = self.load_assets_of(&texts.path, text) {
                return Stage::Ready(Err(error));
            }
        }
        self.share_assets();
        let world = self.world.scratch();
        self.cells.preparing.parse((cell, number), texts, world);
        Stage::Parsing
    }

    /// Whether the cells coming in as the next tick starts are prepared, as
    /// [`enter_coming`](Self::enter_coming) brings them in: taking what the threads have finished,
    /// as [`prepare_cells`](Self::prepare_cells) does, whatever's coming, and starting to prepare
    /// those coming that aren't being prepared; true for a server's client, which
    /// brings none in, and in a world with none coming. A tick that isn't run, as it waits for
    /// them, starts a wait, which logs `cells.waited` at the debug level as it ends, with how long it
    /// took; in a context without threads, as a test's, each is prepared then and there.
    pub(crate) fn cells_ready(&mut self) -> bool {
        if self.client {
            return true;
        }
        // Every tick, so that what's prepared ahead goes on from stage to stage between entries.
        self.prepare_cells();
        let state = self.world.resource::<CellState>();
        let Some(coming) = state.map(|state| &state.coming).filter(|c| !c.is_empty()) else {
            return true;
        };
        let coming: Vec<Cell> = coming.iter().copied().collect();
        let unprepared = |cx: &Self| {
            let cells = &cx.cells.preparing.cells;
            coming
                .iter()
                .copied()
                .filter(|cell| !cells.contains_key(cell))
                .collect::<Vec<_>>()
        };
        let unprepared = unprepared(self);
        if !unprepared.is_empty() {
            let Some((_, cells, _)) = self.world.world_cells() else {
                return true;
            };
            let folder = cells.folder.clone();
            let listed = (self.cells).listed(self.root.as_deref(), &self.files, &folder);
            for cell in unprepared {
                let source = self.cells.source(&folder, &listed, cell);
                self.start_preparing(&folder, cell, source);
            }
            self.prepare_cells();
        }
        let preparing = &mut self.cells.preparing;
        let ready = (coming.iter()).all(|cell| {
            let prep = preparing.cells.get(cell);
            matches!(prep.map(|prep| &prep.stage), Some(Stage::Ready(_)))
        });
        match (ready, preparing.waiting) {
            (true, Some(since)) => {
                preparing.waiting = None;
                debug!(
                    code = "cells.waited",
                    cells = coming.len(),
                    ms = perf::millis(since.elapsed()),
                    "A tick waited for the cells it brings in to be prepared"
                );
            }
            (false, None) => preparing.waiting = Some(Instant::now()),
            _ => {}
        }
        ready
    }

    /// Warns, once a run, that cell `cell`'s roots couldn't be kept as text, with `error`.
    fn held(&mut self, cell: Cell, error: &Error) {
        if self.cells.first("cells.held", cell) {
            warn!(
                code = "cells.held",
                cell = %cell,
                held = error.code(),
                "{error:#}; the cell's streamed roots stay in the world"
            );
        }
    }
}

/// The radius of the look's viewpoint as a viewer while the run is stopped, at least, as
/// [`Context::stream_stopped`] says: a [`Viewer`](cells::Viewer)'s default.
#[cfg(feature = "dev")]
const VIEWED_RADIUS: f32 = 512.0;

/// Those of `roots`, sorted by cell, that are in `cell`.
fn roots_of(roots: &[(Cell, Entity)], cell: Cell) -> Vec<Entity> {
    let from = roots.partition_point(|&(other, _)| other < cell);
    let to = roots.partition_point(|&(other, _)| other <= cell);
    roots[from..to].iter().map(|&(_, root)| root).collect()
}
