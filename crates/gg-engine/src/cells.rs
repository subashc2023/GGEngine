//! Worlds of cells: a world's scene whose [`Cells`] cuts space into cubes, each cell's entities in a
//! scene file of its own, which come into the world as a [`Viewer`] comes near and go out, kept as
//! text, as every viewer goes away. Which cells are in is the simulation's own, decided from the
//! world alone as each tick ends, as `Context::stream_cells` says, so that a replay, a tick run
//! again by rollback or a SyncTest, and each lockstep peer bring the same cells in at the same
//! tick, and what the world keeps of them is a saved resource, which the hash reads, so that a
//! snapshot, a lockstep joiner and a server's client take it with the world.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::mem;
use std::path::Path;
use std::sync::Arc;

use gg_core::math::{IVec3, Vec3};
use gg_core::{Error, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::ecs::Parsed;
use crate::package::{self, Files};
use crate::{Component, Entity, EntityId, Owner, Saved, Transform, World};

/// The most cells in a world at once: past it the nearest come in, and the run warns
/// (`cells.full`), so that a radius set too wide can't load a world into memory.
pub(crate) const MOST_CELLS: usize = 4096;

/// On an entity of a world's scene: space is cut into cubes `size` units on a side from the origin,
/// each a cell whose entities are in the scene file of `folder` named by its place on the grid, as
/// `cells/3_0_-2.ron` for the cell from (768, 0, -512) to (1024, 256, -256) of a size of 256; a cell
/// without a file is empty. As each tick ends, the cells near a [`Viewer`] come into the world, and
/// those far from every viewer go out, as [`Streamed`] says, kept as text, until a viewer comes
/// near again. One a world; of several, the one of the lowest ID, warning (`cells.tie`), as
/// [`Environment`](crate::Environment) has it. A 2D game's cells are those whose z is 0.
///
/// A cell's roots come in as its file holds them, each given [`Streamed`], so that each belongs
/// where it stands: one a file places outside its cell's cube belongs to the cell it's in. The
/// folder is listed once a run, as a world first needs it, from the game's package in a run that
/// reads one and else from the disk, its files' names those of cells alone counting; one that
/// can't be listed brings no cell in from a file, warning (`cells.listing`).
#[derive(Clone, Debug, PartialEq, Component)]
pub struct Cells {
    /// The folder of the cells' scene files, from the asset root, `/` between its parts.
    pub folder: String,
    /// A cell's side, in units: finite and positive, else no cell comes in or goes out, warning
    /// (`cells.bad_size`).
    #[reflect(default = 256.0)]
    pub size: f32,
}

/// On an entity the world streams cells around: as each tick ends, each cell within `radius` of
/// where it is, as [`World::pose`] has it, to the cell's nearest point, comes in, and each farther
/// than `radius` and half a cell from every viewer goes out, so that a viewer on a cell's edge
/// doesn't bring it in and take it out tick after tick. A viewer never goes out with a cell, nor
/// what holds one.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Viewer {
    /// How far from it, in units, cells come in: 512 by default, two cells of the default size.
    #[reflect(min = 0.0, default = 512.0)]
    pub radius: f32,
}

/// On a root that streams: as each tick ends, it goes out with the cell where it is, as its
/// [`Transform`] has it, the origin without one, when that cell goes out, or at once if the tick
/// left it in a cell that's out, kept with what the cell keeps, and comes back with the cell, as
/// it was. A cell's load gives it to the cell's roots; a game gives it to what it spawns that
/// should stream, as a dropped item or an enemy. What's under a root goes with it. What refers to
/// a streamed entity from outside it finds it gone once it's out, as one despawned is, and doesn't
/// find it again as it comes back.
///
/// A root that refers to an entity going with neither it nor its cell, which no scene could save,
/// holds its cell in, warning (`cells.held`), as does one whose texture wasn't loaded from a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct Streamed;

/// A cell's place on the grid: the cube from `0 * size` to `(0 + 1) * size` on each axis, its
/// [`Cells`]'s `size`. Cells are ordered by x, then y, then z.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cell(pub IVec3);

impl Ord for Cell {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.to_array().cmp(&other.0.to_array())
    }
}

impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// As its file's name has it, without `.ron`: its x, y and z, `_` between, as `3_0_-2`.
impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let IVec3 { x, y, z } = self.0;
        write!(f, "{x}_{y}_{z}")
    }
}

impl Cell {
    /// The cell `point` is in, of cells `size` on a side: each axis's `floor(point / size)`, held
    /// to an `i32`'s range, a NaN at 0.
    pub(crate) fn at(point: Vec3, size: f32) -> Self {
        let axis = |x: f32| (x / size).floor() as i32;
        Self(IVec3::new(axis(point.x), axis(point.y), axis(point.z)))
    }

    /// The cell whose file is named `name`, as `3_0_-2.ron`, if it's a cell's: three integers, `_`
    /// between, written as Rust writes them, so that each cell has one name.
    pub(crate) fn of_file(name: &str) -> Option<Self> {
        let stem = name.strip_suffix(".ron")?;
        let mut axes = stem.split('_').map(|axis| {
            let n: i32 = axis.parse().ok()?;
            (n.to_string() == axis).then_some(n)
        });
        let (x, y, z) = (axes.next()??, axes.next()??, axes.next()??);
        axes.next().is_none().then_some(Self(IVec3::new(x, y, z)))
    }

    /// The square of the distance from `point` to the cell's nearest point, of cells `size` on a
    /// side, in `f64`, whose arithmetic is IEEE's on every platform, as the world's is.
    fn distance_squared(self, point: [f64; 3], size: f64) -> f64 {
        let low = self.0.to_array().map(|n| f64::from(n) * size);
        (0..3)
            .map(|axis| {
                let (low, high) = (low[axis], low[axis] + size);
                let away = (low - point[axis]).max(point[axis] - high).max(0.0);
                away * away
            })
            .sum()
    }
}

/// What a world of cells holds of them beside its entities, as the module says: the cells in; those
/// coming in as the next tick starts, as the last ended; the digest of the file of each cell whose
/// file has come in, so that what the cell keeps stands for it after, and a peer or a replay with
/// another file is told; what each cell out keeps of its entities; and the cells whose load was
/// refused while a viewer stays near, which don't try again until every viewer has gone. Saved, so
/// that a snapshot, a lockstep joiner and a server's client have it; and no world without [`Cells`]
/// has one with anything in it, so that the hash of such a world reads none.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CellState {
    pub(crate) cells_in: BTreeSet<Cell>,
    pub(crate) coming: BTreeSet<Cell>,
    pub(crate) files: BTreeMap<Cell, u64>,
    pub(crate) kept: BTreeMap<Cell, Kept>,
    pub(crate) refused: BTreeSet<Cell>,
}

/// What a cell out keeps: its entities as they went out, as scene texts, each in turn, which come
/// back in that order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kept {
    pub(crate) texts: Vec<KeptText>,
}

/// A scene text a cell keeps, shared, so that rollback's copies and a joiner's snapshot copy a
/// pointer, and its digest, which the world's hash reads in its place.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeptText {
    pub(crate) text: Arc<str>,
    pub(crate) digest: u64,
}

impl KeptText {
    fn new(text: &str) -> Self {
        Self {
            digest: crate::ecs::text_digest(text),
            text: text.into(),
        }
    }
}

impl CellState {
    /// Whether it holds nothing, as a world's whose cells never came in or that a scene change
    /// emptied, which the world's hash leaves out.
    pub(crate) fn is_empty(&self) -> bool {
        self.cells_in.is_empty()
            && self.coming.is_empty()
            && self.files.is_empty()
            && self.kept.is_empty()
            && self.refused.is_empty()
    }
}

/// [`CellState`] as a snapshot file holds it.
#[derive(Serialize, Deserialize)]
struct SavedCells {
    cells_in: Vec<[i32; 3]>,
    coming: Vec<[i32; 3]>,
    files: Vec<([i32; 3], u64)>,
    kept: Vec<SavedKept>,
    refused: Vec<[i32; 3]>,
}

#[derive(Serialize, Deserialize)]
struct SavedKept {
    cell: [i32; 3],
    texts: Vec<String>,
}

/// As JSON, whose strings keep a scene text's line breaks escaped, on one line.
impl Saved for CellState {
    const NAME: &'static str = "cells";

    fn save(&self, _: &World) -> Result<String> {
        let cells = |cells: &BTreeSet<Cell>| cells.iter().map(|cell| cell.0.to_array()).collect();
        let saved = SavedCells {
            cells_in: cells(&self.cells_in),
            coming: cells(&self.coming),
            files: (self.files.iter())
                .map(|(cell, &digest)| (cell.0.to_array(), digest))
                .collect(),
            kept: (self.kept.iter())
                .map(|(cell, kept)| SavedKept {
                    cell: cell.0.to_array(),
                    texts: kept
                        .texts
                        .iter()
                        .map(|kept| kept.text.to_string())
                        .collect(),
                })
                .collect(),
            refused: cells(&self.refused),
        };
        serde_json::to_string(&saved).map_err(|error| Error::new("snapshot.bad", error.to_string()))
    }

    fn load(text: &str, _: &World) -> Result<Self> {
        let saved: SavedCells = serde_json::from_str(text).map_err(|error| {
            let message = format!("the cells' state isn't one: {error}");
            Error::new("snapshot.bad", message)
        })?;
        let cell = |at: [i32; 3]| Cell(IVec3::from_array(at));
        let kept = saved.kept.into_iter().map(|kept| {
            let texts = kept.texts.iter().map(|text| KeptText::new(text)).collect();
            (cell(kept.cell), Kept { texts })
        });
        let files = saved.files.into_iter();
        Ok(Self {
            cells_in: saved.cells_in.into_iter().map(cell).collect(),
            coming: saved.coming.into_iter().map(cell).collect(),
            files: files.map(|(at, digest)| (cell(at), digest)).collect(),
            kept: kept.collect(),
            refused: saved.refused.into_iter().map(cell).collect(),
        })
    }
}

/// What a server's client sees of the world, as [`World::seen_by`] finds it, which its states hold,
/// as [`World::save_seen`] writes them and [`World::hash_seen`] hashes them: by slot, whether the
/// entity there is hidden, a streamed root of a cell it doesn't see or what's under one, so that no
/// entity shown has a child hidden, none past its end; and the cells' state its world holds in
/// place of the server's, the cells it sees in and nothing else, what the cells out keep staying
/// on the server.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Seen {
    pub(crate) hidden: Vec<bool>,
    pub(crate) cells: CellState,
}

/// The square of the distance from `cell`, of cells `size` on a side, to the nearest of `viewers`,
/// each where it is and its radius, whose radius and `reach` reach it, if one does.
fn nearest_viewer(viewers: &[([f64; 3], f64)], cell: Cell, reach: f64, size: f64) -> Option<f64> {
    let near = viewers.iter().filter_map(|&(at, radius)| {
        let distance = cell.distance_squared(at, size);
        let reach = radius + reach;
        (reach >= 0.0 && distance <= reach * reach).then_some(distance)
    });
    near.min_by(f64::total_cmp)
}

/// What a cell brings in as it comes in, each text parsed as [`World::parse_scene`] parses it: its
/// file's, with the file's digest, while its file is yet to come in, as what the cell keeps then
/// stands for it; and each text it keeps, in turn.
pub(crate) struct Entering {
    pub(crate) file: Option<(u64, Parsed)>,
    pub(crate) kept: Vec<Parsed>,
}

/// What the cells' step does as a tick ends, which [`World::plan_cells`] finds from the world: kept
/// from tick to tick, so that a tick that changes nothing allocates nothing once it has grown.
#[derive(Default)]
pub(crate) struct Plan {
    /// The cells in that go out, in order.
    pub(crate) leaving: Vec<Cell>,
    /// The cells out that come in, in order.
    pub(crate) entering: Vec<Cell>,
    /// The streamed roots the tick left in cells out, which go out at once into what their cells
    /// keep, by cell, then ID; and those of the cells going out, likewise.
    pub(crate) strays: Vec<(Cell, Entity)>,
    pub(crate) leavers: Vec<(Cell, Entity)>,
    /// The cells refused whose viewers have gone, which may try again.
    pub(crate) forgotten: Vec<Cell>,
    /// Whether more cells would be in than [`MOST_CELLS`].
    pub(crate) full: bool,
    /// The cells out, coming in or not, within a viewer's radius and one and a half cells, each
    /// with how far it is past the radius of the viewer it's nearest the edge of, the nearest first,
    /// at most [`MOST_CELLS`]: those to prepare ahead, as [`Preparing`] does, which changes nothing
    /// in the world.
    pub(crate) ahead: Vec<(f64, Cell)>,
    /// The cells of head, in order, to find one among.
    pub(crate) nearby: Vec<Cell>,
    /// The viewers, where each is and its radius; the streamed roots, each with its cell and ID;
    /// the cells that would come in, each with the square of its distance to the nearest viewer;
    /// and what's under a root, for the walk that finds a viewer under it.
    viewers: Vec<([f64; 3], f64)>,
    roots: Vec<(Cell, EntityId, Entity)>,
    near: Vec<(f64, Cell)>,
    walk: Vec<Entity>,
}

impl Plan {
    /// Whether it changes nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.leaving.is_empty()
            && self.entering.is_empty()
            && self.strays.is_empty()
            && self.forgotten.is_empty()
            && !self.full
    }
}

impl World {
    /// The cells in, ordered as [`Cell`] says; none in a world without [`Cells`].
    pub fn cells_in(&self) -> impl Iterator<Item = Cell> + '_ {
        let state = self.resource::<CellState>();
        state
            .into_iter()
            .flat_map(|state| state.cells_in.iter().copied())
    }

    /// The cell `point` is in, under the world's [`Cells`]: `None` in a world without one, or
    /// whose size isn't finite and positive.
    pub fn cell_at(&self, point: Vec3) -> Option<Cell> {
        let (_, cells, _) = self.world_cells()?;
        let size = cells.size;
        (size.is_finite() && size > 0.0).then(|| Cell::at(point, size))
    }

    /// How many cells are in, and how many out keep something, as the tool API's `status` says;
    /// `None` in a world whose cells never came in.
    #[cfg(feature = "dev")]
    pub(crate) fn cell_counts(&self) -> Option<(usize, usize)> {
        let state = self.resource::<CellState>()?;
        Some((state.cells_in.len(), state.kept.len()))
    }

    /// The world's [`Cells`], that of the lowest ID among its entities with one, with that ID and
    /// how many have one; `None` in a world without.
    pub(crate) fn world_cells(&self) -> Option<(EntityId, &Cells, usize)> {
        let mut lowest: Option<(EntityId, &Cells)> = None;
        let mut count = 0;
        for (entity, cells) in self.query::<(Entity, &Cells)>() {
            let id = self.id(entity).expect("a query's entities are alive");
            count += 1;
            if lowest.is_none_or(|(low, _)| id < low) {
                lowest = Some((id, cells));
            }
        }
        lowest.map(|(id, cells)| (id, cells, count))
    }

    /// Empties what the world holds of its cells, as a scene change does with its entities, keeping
    /// the resource, so that a client's copy, which keeps a resource its server's state holds none
    /// of, is emptied too.
    pub(crate) fn empty_cells(&mut self) {
        if let Some(state) = self.resource_mut::<CellState>() {
            *state = CellState::default();
        }
    }

    /// Fills `plan` with what the cells' step does as this tick ends, of cells `size` on a side,
    /// `listed` those with files, in order: each cell in farther than a viewer's radius and half a
    /// cell from every viewer goes out, with the streamed roots in it but those that are or hold a
    /// viewer; each cell out within a viewer's radius comes in, of those with a file, those that
    /// keep something and those a streamed root is in, but a cell refused, the nearest first past
    /// [`MOST_CELLS`]; each streamed root in a cell out that doesn't come in goes out at once; and
    /// each cell refused farther than a radius and half a cell from every viewer is forgotten.
    /// `extra`, where it is and its radius, is a viewer too, as a development build's look's
    /// viewpoint is while the run is stopped, as `Context::stream_stopped` says.
    pub(crate) fn plan_cells(
        &self,
        size: f32,
        listed: &[Cell],
        extra: Option<([f64; 3], f64)>,
        plan: &mut Plan,
    ) {
        let _zone = gg_core::zone!();
        let Plan {
            leaving,
            entering,
            strays,
            leavers,
            forgotten,
            full,
            ahead,
            nearby,
            viewers,
            roots,
            near,
            walk,
        } = plan;
        leaving.clear();
        entering.clear();
        forgotten.clear();
        strays.clear();
        leavers.clear();
        near.clear();
        ahead.clear();
        *full = false;
        viewers.clear();
        for (entity, viewer) in self.query::<(Entity, &Viewer)>() {
            let at = self.pose(entity).expect("alive").matrix.w_axis;
            let at = [at.x, at.y, at.z].map(f64::from);
            viewers.push((at, f64::from(viewer.radius)));
        }
        viewers.extend(extra);
        self.streamed_roots(size, roots);
        let size = f64::from(size);
        let nearest = |cell: Cell, reach: f64| nearest_viewer(viewers, cell, reach, size);
        let empty = CellState::default();
        let state = self.resource::<CellState>().unwrap_or(&empty);
        for &cell in &state.cells_in {
            if nearest(cell, size / 2.0).is_none() {
                leaving.push(cell);
            }
        }
        for &cell in &state.refused {
            if nearest(cell, size / 2.0).is_none() {
                forgotten.push(cell);
            }
        }
        let out = |cell: &Cell| {
            !state.cells_in.contains(cell)
                && !state.coming.contains(cell)
                && !state.refused.contains(cell)
        };
        let kept = state.kept.keys();
        let rooted = roots.iter().map(|&(cell, ..)| cell);
        // How far `cell` is past the radius of the viewer it's nearest the edge of, if it's within
        // one's radius and one and a half cells: a square root, which the plan's own choices
        // never take, as only what's prepared ahead goes by it.
        let past = |cell: Cell| {
            let past = viewers.iter().filter_map(|&(at, radius)| {
                let reach = radius + 1.5 * size;
                let distance = cell.distance_squared(at, size);
                (distance <= reach * reach).then(|| distance.sqrt() - radius)
            });
            past.min_by(f64::total_cmp)
        };
        for cell in listed.iter().copied().chain(kept.copied()).chain(rooted) {
            if !out(&cell) {
                continue;
            }
            if let Some(distance) = nearest(cell, 0.0) {
                near.push((distance, cell));
            }
            if let Some(past) = past(cell) {
                ahead.push((past, cell));
            }
        }
        let by_distance =
            |a: &(f64, Cell), b: &(f64, Cell)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
        near.sort_unstable_by(by_distance);
        near.dedup_by_key(|&mut (_, cell)| cell);
        ahead.sort_unstable_by(by_distance);
        ahead.dedup_by_key(|&mut (_, cell)| cell);
        ahead.truncate(MOST_CELLS);
        nearby.clear();
        nearby.extend(ahead.iter().map(|&(_, cell)| cell));
        nearby.sort_unstable();
        let room = MOST_CELLS.saturating_sub(state.cells_in.len() - leaving.len());
        if near.len() > room {
            *full = true;
            near.truncate(room);
        }
        entering.extend(near.iter().map(|&(_, cell)| cell));
        entering.sort_unstable();
        for &(cell, _, root) in roots.iter() {
            let going = leaving.binary_search(&cell).is_ok();
            let staying = !going && state.cells_in.contains(&cell);
            if staying || entering.binary_search(&cell).is_ok() || self.holds_viewer(root, walk) {
                continue;
            }
            match going {
                true => leavers.push((cell, root)),
                false => strays.push((cell, root)),
            }
        }
    }

    /// Fills `roots` with the streamed roots, each with the cell it's in, of cells `size` on a side,
    /// as its [`Transform`] has it, the origin without one, and its ID, by cell, then ID.
    pub(crate) fn streamed_roots(&self, size: f32, roots: &mut Vec<(Cell, EntityId, Entity)>) {
        roots.clear();
        for (entity, _) in self.query::<(Entity, &Streamed)>() {
            if self.parent(entity).expect("alive").is_some() {
                continue;
            }
            let at = self
                .get::<Transform>(entity)
                .map_or(Vec3::ZERO, |at| at.translation);
            let id = self.id(entity).expect("alive");
            roots.push((Cell::at(at, size), id, entity));
        }
        roots.sort_unstable_by_key(|&(cell, id, _)| (cell, id));
    }

    /// What player `player`'s client sees of the world, as a server's states send it, `had` the
    /// cells it saw: of the cells in, each within the radius of a [`Viewer`] of its own, one that
    /// it or an ancestor's [`Owner`] says is the player's, and each it saw within the radius and
    /// half a cell, as the cells' own step has them; and every entity but the streamed roots of
    /// the cells it doesn't see, but one that is or holds a viewer of its own, with what's under
    /// them. A viewer no player owns is no client's. Nothing hidden and no cell seen in a world
    /// without [`Cells`], or whose size isn't finite and positive.
    pub(crate) fn seen_by(&self, player: u8, had: &BTreeSet<Cell>) -> Seen {
        let _zone = gg_core::zone!();
        let mut seen = Seen::default();
        let size = self.world_cells().map(|(_, cells, _)| cells.size);
        let Some(size) = size.filter(|size| size.is_finite() && *size > 0.0) else {
            return seen;
        };
        let (mut viewers, mut own) = (Vec::new(), Vec::new());
        for (entity, viewer) in self.query::<(Entity, &Viewer)>() {
            let mut root = entity;
            let mut owner = None;
            loop {
                owner = owner.or(self.get::<Owner>(root).map(|owner| owner.player));
                match self.parent(root).expect("alive") {
                    Some(parent) => root = parent,
                    None => break,
                }
            }
            if owner == Some(player) {
                let at = self.pose(entity).expect("alive").matrix.w_axis;
                viewers.push(([at.x, at.y, at.z].map(f64::from), f64::from(viewer.radius)));
                own.push(root);
            }
        }
        let wide = f64::from(size);
        let state = self.resource::<CellState>();
        for &cell in state.iter().flat_map(|state| &state.cells_in) {
            let reach = if had.contains(&cell) { wide / 2.0 } else { 0.0 };
            if nearest_viewer(&viewers, cell, reach, wide).is_some() {
                seen.cells.cells_in.insert(cell);
            }
        }
        let mut walk = Vec::new();
        for (root, _) in self.query::<(Entity, &Streamed)>() {
            let at = self
                .get::<Transform>(root)
                .map_or(Vec3::ZERO, |at| at.translation);
            if self.parent(root).expect("alive").is_some()
                || own.contains(&root)
                || seen.cells.cells_in.contains(&Cell::at(at, size))
            {
                continue;
            }
            walk.push(root);
            while let Some(entity) = walk.pop() {
                let slot = entity.slot();
                if seen.hidden.len() <= slot {
                    seen.hidden.resize(slot + 1, false);
                }
                seen.hidden[slot] = true;
                walk.extend(self.children(entity).expect("alive"));
            }
        }
        seen
    }

    /// Whether `root` is a [`Viewer`], or holds one under it.
    fn holds_viewer(&self, root: Entity, walk: &mut Vec<Entity>) -> bool {
        walk.clear();
        walk.push(root);
        while let Some(entity) = walk.pop() {
            if self.get::<Viewer>(entity).is_some() {
                return true;
            }
            walk.extend(self.children(entity).expect("alive"));
        }
        false
    }

    /// Takes `roots`, streamed roots of cell `cell`, out of the world with everything under them,
    /// into what the cell keeps, as a scene text of their own, after what it keeps: as the cell goes
    /// out with them, if `leaving`, or at once, into a cell out. Returns how many roots went.
    ///
    /// Fails, taking none out, as [`save_scene`](Self::save_scene) does, as for a root that refers
    /// to an entity that doesn't go with it.
    pub(crate) fn keep_cell(
        &mut self,
        cell: Cell,
        roots: &[Entity],
        leaving: bool,
    ) -> Result<usize> {
        let text = match roots {
            [] => None,
            roots => Some(KeptText::new(&self.save_scene(roots)?)),
        };
        for &root in roots {
            self.despawn(root).expect("a streamed root is alive");
        }
        let state = self.cell_state_mut();
        if leaving {
            state.cells_in.remove(&cell);
        }
        if let Some(text) = text {
            state.kept.entry(cell).or_default().texts.push(text);
        }
        Ok(roots.len())
    }

    /// Takes cell `cell` out as its file has it, while the run is stopped, as
    /// [`Context::stream_stopped`](crate::Context::stream_stopped) says: `roots`, its streamed
    /// roots, go with everything under them, keeping nothing, and its file is no longer noted as
    /// having come in, so that the cell comes in from its file again.
    #[cfg(feature = "dev")]
    pub(crate) fn forget_cell(&mut self, cell: Cell, roots: &[Entity]) {
        for &root in roots {
            self.despawn(root).expect("a streamed root is alive");
        }
        let state = self.cell_state_mut();
        state.cells_in.remove(&cell);
        state.files.remove(&cell);
    }

    /// Notes that cell `cell`'s file, of digest `digest`, holds what the world holds of the cell,
    /// as a world's save leaves it, so that it doesn't come in from the file.
    #[cfg(feature = "dev")]
    pub(crate) fn note_filed(&mut self, cell: Cell, digest: u64) {
        self.cell_state_mut().files.insert(cell, digest);
    }

    /// The cell whose file's path from the asset root is `author`, under the world's [`Cells`], if
    /// it's in and its file has come in, so that a save of the file applies to it.
    #[cfg(feature = "dev")]
    pub(crate) fn filed_cell_in(&self, author: &str) -> Option<Cell> {
        let (_, cells, _) = self.world_cells()?;
        let name = author
            .strip_prefix(cells.folder.as_str())?
            .strip_prefix('/')?;
        let cell = Cell::of_file(name)?;
        let state = self.resource::<CellState>()?;
        (state.cells_in.contains(&cell) && state.files.contains_key(&cell)).then_some(cell)
    }

    /// What a cell's file is written as, as a world's save writes it, of `roots`, the streamed
    /// roots in it, with everything under them, if that differs from `file`, the text its file held
    /// as last applied, `None` for a cell whose file hasn't come in: nothing for a cell without
    /// roots whose file hasn't come in. A file is compared, and written, canonical, its roots
    /// without [`Streamed`], as [`cell_canonical`](Self::cell_canonical) gives it; one whose text
    /// no longer loads differs.
    ///
    /// Fails as [`save_scene`](Self::save_scene) does for `roots`.
    #[cfg(feature = "dev")]
    pub(crate) fn cell_written(
        &self,
        file: Option<&str>,
        roots: &[Entity],
    ) -> Result<Option<String>> {
        if roots.is_empty() && file.is_none() {
            return Ok(None);
        }
        let now = self.cell_canonical(&self.save_scene(roots)?)?;
        let was = file.map(|file| self.cell_canonical(file));
        Ok((!matches!(was, Some(Ok(was)) if was == now)).then_some(now))
    }

    /// Scene `text` as a cell's file holds it: canonical, as
    /// [`scene_canonical`](Self::scene_canonical) gives it, but with its roots without
    /// [`Streamed`], which a cell's load gives them.
    ///
    /// Fails as `scene_canonical` does.
    #[cfg(feature = "dev")]
    pub(crate) fn cell_canonical(&self, text: &str) -> Result<String> {
        let mut loaded = self.scratch();
        let roots = loaded.load_scene(text)?;
        for &root in &roots {
            loaded.remove::<Streamed>(root).expect("loaded");
        }
        loaded.save_scene(&roots)
    }

    /// Scene `text` with its roots given [`Streamed`], as a cell brings them in, canonical, so
    /// that a cell file's save is compared with the cell in as the world holds it.
    ///
    /// Fails as [`scene_canonical`](Self::scene_canonical) does.
    #[cfg(feature = "dev")]
    pub(crate) fn streamed_text(&self, text: &str) -> Result<String> {
        let mut loaded = self.scratch();
        let roots = loaded.load_scene(text)?;
        for &root in &roots {
            loaded.insert(root, Streamed).expect("loaded");
        }
        loaded.save_scene(&roots)
    }

    /// Notes that `cells` come in as the next tick starts, as [`enter_cell`](Self::enter_cell)
    /// brings each in.
    pub(crate) fn note_coming(&mut self, cells: &[Cell]) {
        self.cell_state_mut().coming.extend(cells);
    }

    /// The cells coming in, which it no longer notes, in order; none without.
    pub(crate) fn take_coming(&mut self) -> BTreeSet<Cell> {
        let state = self.resource_mut::<CellState>();
        state.map_or_else(BTreeSet::new, |state| mem::take(&mut state.coming))
    }

    /// Brings cell `cell` in, with what `entering` holds of it, in turn, each root given
    /// [`Streamed`], noting its file's digest if its file came; and returns how many roots came.
    /// All of it, or, if a text is refused, none, the cell staying as it was.
    ///
    /// Fails as [`commit_scene`](Self::commit_scene) does, for an ID an entity alive has
    /// (`ecs.id_taken`).
    pub(crate) fn enter_cell(&mut self, cell: Cell, entering: Entering) -> Result<usize> {
        let Entering { file, kept } = entering;
        let (digest, file) = file.unzip();
        let mut roots = Vec::new();
        for parsed in file.into_iter().chain(kept) {
            match self.commit_scene(parsed) {
                Ok(loaded) => roots.extend(loaded),
                Err(error) => {
                    for &root in &roots {
                        self.despawn(root).expect("loaded");
                    }
                    return Err(error);
                }
            }
        }
        for &root in &roots {
            self.insert(root, Streamed).expect("loaded");
        }
        let state = self.cell_state_mut();
        state.kept.remove(&cell);
        state.cells_in.insert(cell);
        if let Some(digest) = digest {
            state.files.insert(cell, digest);
        }
        Ok(roots.len())
    }

    /// Notes that cell `cell`'s load was refused, so that it doesn't try again while a viewer stays
    /// near.
    pub(crate) fn refuse_cell(&mut self, cell: Cell) {
        self.cell_state_mut().refused.insert(cell);
    }

    /// Forgets that `cells` were refused, so that they try again as a viewer comes near.
    pub(crate) fn forget_refused(&mut self, cells: &[Cell]) {
        let state = self.cell_state_mut();
        for cell in cells {
            state.refused.remove(cell);
        }
    }

    /// The path of cell `cell`'s file from the asset root, under the world's [`Cells`]; with the
    /// folder `?` in a world without.
    pub(crate) fn cell_file(&self, cell: Cell) -> String {
        let folder = self
            .world_cells()
            .map(|(_, cells, _)| cells.folder.as_str());
        format!("{}/{cell}.ron", folder.unwrap_or("?"))
    }

    /// The first cell, in order, whose file both this world and `other` brought in, from files of
    /// other digests: its file's path, as [`cell_file`](Self::cell_file) gives it, and the digests
    /// of this world's file and `other`'s.
    pub(crate) fn cell_file_parted(&self, other: &World) -> Option<(String, u64, u64)> {
        let (ours, theirs) = (
            self.resource::<CellState>()?,
            other.resource::<CellState>()?,
        );
        let (cell, ours, theirs) = ours.files.iter().find_map(|(&cell, &ours)| {
            let &theirs = theirs.files.get(&cell)?;
            (ours != theirs).then_some((cell, ours, theirs))
        })?;
        Some((self.cell_file(cell), ours, theirs))
    }

    fn cell_state_mut(&mut self) -> &mut CellState {
        if self.resource::<CellState>().is_none() {
            self.insert_resource(CellState::default());
        }
        self.resource_mut::<CellState>().expect("inserted")
    }
}

/// What a run keeps of its worlds' cells beside the world: each folder it listed, with its cells'
/// files, in order, once a run, as [`Cells`] says; the warnings it gave, by code and what each is
/// about, each once a run, so that neither a cell refused tick after tick nor a tick run again
/// warns again; the tie of [`Cells`] it last warned of; and the plan of the cells' step.
#[derive(Default)]
pub(crate) struct CellRun {
    pub(crate) listed: Vec<(String, Arc<[Cell]>)>,
    warned: HashSet<(&'static str, String)>,
    pub(crate) tie: Option<(EntityId, usize)>,
    pub(crate) plan: Plan,
    pub(crate) preparing: Preparing,
    /// The cells whose files came in as the tick started, with their digests, for a recording.
    pub(crate) came: Vec<(Cell, u64)>,
    /// While the run is stopped, the cells found unlike their files as no viewer was near, which
    /// stay in, and the count of the scene files' changes they were found at, as
    /// [`SceneFiles::changes`](crate::scene_files::SceneFiles::changes) counts them, after which
    /// they're looked at again.
    #[cfg(feature = "dev")]
    pub(crate) held: (Vec<Cell>, u64),
}

impl CellRun {
    /// Whether the warning of `code` about `about` is yet to be given, which it now counts as given.
    pub(crate) fn first(&mut self, code: &'static str, about: impl fmt::Display) -> bool {
        self.warned.insert((code, about.to_string()))
    }

    /// The cells whose files the folder `folder` holds, as [`list_cells`] lists them under `root`
    /// from `files`, once a run; none, warning `cells.listing`, for one it can't list.
    pub(crate) fn listed(
        &mut self,
        root: Option<&Path>,
        files: &Files,
        folder: &str,
    ) -> Arc<[Cell]> {
        if let Some((_, listed)) = self.listed.iter().find(|(listed, _)| listed == folder) {
            return Arc::clone(listed);
        }
        let listed: Arc<[Cell]> = list_cells(root, files, folder).unwrap_or_else(|error| {
            warn!(
                code = "cells.listing",
                folder, "{error:#}, so no cell comes in from a file"
            );
            Arc::new([])
        });
        self.listed.push((folder.to_owned(), Arc::clone(&listed)));
        listed
    }
}

/// The cells whose files the folder `folder` holds, its path from the asset root `root`, `/`
/// between its parts, in order: those its files' names, as [`Cell::of_file`] reads them, are, from
/// the game's package in a run that reads one, `files`, and else from the disk.
///
/// Fails with `cells.listing` for a path that isn't one inside the asset root, in a run without a
/// root, and for a folder the disk can't list.
pub(crate) fn list_cells(root: Option<&Path>, files: &Files, folder: &str) -> Result<Arc<[Cell]>> {
    if !package::inside(folder) || folder.chars().any(char::is_control) {
        let message = format!(
            "`{folder}` isn't a folder's path from the asset root, `/` between its parts, none of \
             them empty, `.` or `..`, nor holding `\\`, `:` or a control character"
        );
        return Err(Error::new("cells.listing", message));
    }
    let Some(root) = root else {
        let message = format!("there's no asset root to find the cells' folder {folder} under");
        return Err(Error::new("cells.listing", message));
    };
    let dir = package::under(root, folder);
    let mut cells: Vec<Cell> = match files.packaged() {
        Some(_) => (files.paths().iter())
            .filter(|path| path.parent() == Some(&*dir))
            .filter_map(|path| Cell::of_file(path.file_name()?.to_str()?))
            .collect(),
        None => {
            let entries = std::fs::read_dir(&dir).map_err(|error| {
                let message = format!("can't list the cells' folder {folder}");
                Error::new("cells.listing", message).with_source(error)
            })?;
            let files = entries.flatten().filter(|entry| {
                let kind = entry.file_type();
                kind.is_ok_and(|kind| kind.is_file())
            });
            let names = files.filter_map(|entry| entry.file_name().into_string().ok());
            names.filter_map(|name| Cell::of_file(&name)).collect()
        }
    };
    cells.sort_unstable();
    cells.dedup();
    Ok(cells.into())
}

mod prepare;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use prepare::{Finished, ModelRead, Prep, Prepared, Preparing, Stage, Texts};
