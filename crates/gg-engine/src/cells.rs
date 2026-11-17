//! Worlds of cells: [`Cells`] cuts space into cubes, each with its entities in a scene file. A cell
//! comes in as a [`Viewer`] nears and goes out, kept as text, when every viewer leaves. The world
//! alone decides which cells are in, as each tick ends (`Context::stream_cells`), so replays,
//! rollback, SyncTest reruns and lockstep peers bring the same cells in at the same tick. What the
//! world keeps is a saved resource the hash reads, so snapshots, joiners and clients carry it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::mem;
use std::path::Path;
use std::sync::Arc;

use gg_core::math::{DVec3, IVec3};
use gg_core::{Error, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::ecs::{Parsed, text_digest};
use crate::package::{self, Files};
use crate::{Component, Entity, EntityId, Owner, Saved, Transform, World};

/// The most cells in a world at once. Past it the nearest come in and the run warns (`cells.full`),
/// so a too-wide radius can't load a world into memory.
pub(crate) const MOST_CELLS: usize = 4096;

/// The most cells in [`looked`]'s box per viewer per tick: eight times [`MOST_CELLS`], so the box
/// holds the nearest [`MOST_CELLS`] (a sphere of about 10 cells' radius, a disc of about 36 in a
/// one-cell-deep world) at bounded cost.
const MOST_LOOKED: u64 = 8 * MOST_CELLS as u64;

/// The box of cells (first, last on each axis) of side `size` within `reach` of `at`, clipped to
/// `within` and to as many cells either way of `at`'s as fit [`MOST_LOOKED`]: a flat world's search
/// reaches farther than a deep one's.
fn looked(at: [f64; 3], reach: f64, size: f64, within: (IVec3, IVec3)) -> (IVec3, IVec3) {
    let cell_of = |x: f64| (x / size).floor() as i32;
    let [low, high] = [-reach, reach].map(|by| IVec3::from_array(at.map(|x| cell_of(x + by))));
    let middle = IVec3::from_array(at.map(cell_of));
    let clip = |away: i32| {
        let away = IVec3::splat(away);
        let low = low.max(middle.saturating_sub(away)).max(within.0);
        (low, high.min(middle.saturating_add(away)).min(within.1))
    };
    let cells = |(low, high): (IVec3, IVec3)| {
        let side = |axis: usize| (i64::from(high[axis]) - i64::from(low[axis]) + 1).max(0);
        (0..3).fold(1u64, |cells, axis| cells.saturating_mul(side(axis) as u64))
    };
    // Binary search for the farthest reach that fits; one cell always does.
    let (mut fits, mut over) = (0, i32::MAX);
    if cells(clip(over)) <= MOST_LOOKED {
        return clip(over);
    }
    while over - fits > 1 {
        let away = fits + (over - fits) / 2;
        match cells(clip(away)) <= MOST_LOOKED {
            true => fits = away,
            false => over = away,
        }
    }
    clip(fits)
}

/// On an entity of a world's scene: cuts space into cubes `size` units on a side from the origin.
///
/// A cell's entities are in `folder`'s scene file named by its grid place: the cube (768, 0, -512)
/// to (1024, 256, -256) at size 256 is `cells/3_0_-2.ron`. A cell without a file is empty unless
/// the game makes it ([`Context::make_cells`](crate::Context::make_cells)). As each tick ends,
/// cells near a [`Viewer`] come in and cells far from every viewer go out, kept as text
/// ([`Streamed`]). One a world; of several the lowest ID wins, warning (`cells.tie`). A 2D game's
/// cells have z 0.
///
/// Roots come in as the file holds them, each given [`Streamed`]; one placed outside its cell's
/// cube belongs to the cell it's in. The folder is listed once a run, from the package if the run
/// reads one, else the disk; only cell file names count. An unlistable folder brings no cell in
/// from a file, warning (`cells.listing`); a missing one is fine for made cells.
#[derive(Clone, Debug, PartialEq, Component)]
pub struct Cells {
    /// The cells' scene files' folder, from the asset root, `/` between its parts.
    pub folder: String,
    /// A cell's side, in units: finite and positive, else no cell comes in or goes out, warning
    /// (`cells.bad_size`).
    #[reflect(default = 256.0)]
    pub size: f32,
}

/// On an entity the world streams cells around.
///
/// As each tick ends, cells whose nearest point is
/// within `radius` of its [`World::pose`] come in, and those farther than `radius` plus half a cell
/// from every viewer go out, so a viewer on a cell's edge doesn't flip it in and out. A viewer, or
/// what holds one, never goes out with a cell.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Viewer {
    /// How far from it, in units, cells come in: 512 by default, two default cells.
    #[reflect(min = 0.0, default = 512.0)]
    pub radius: f32,
}

/// On a root that streams.
///
/// It goes out, kept with what its cell keeps, when its cell (by its
/// [`Transform`], the origin without one) goes out, or at once if the tick left it in a cell that's
/// out. It comes back with the cell as it was. A cell's load gives it to the cell's roots; a game
/// gives it to what it spawns that should stream, like a dropped item. What's under a root goes
/// with it. References from outside find it gone while it's out, as if despawned, and don't find it
/// again on return.
///
/// A root referring to an entity going with neither it nor its cell, which no scene could save,
/// holds its cell in, warning (`cells.held`); so does one whose texture wasn't loaded from a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Component)]
pub struct Streamed;

/// A cell's place on the grid: the cube from `n * size` to `(n + 1) * size` on each axis. Ordered
/// by x, then y, then z.
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

/// Its file's name without `.ron`: x, y and z, `_` between, like `3_0_-2`.
impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let IVec3 { x, y, z } = self.0;
        write!(f, "{x}_{y}_{z}")
    }
}

impl Cell {
    /// The cell `point` is in: each axis's `floor(point / size)`, in `f64` like positions, clamped
    /// to `i32`, a NaN at 0.
    pub(crate) fn at(point: DVec3, size: f32) -> Self {
        let size = f64::from(size);
        let axis = |x: f64| (x / size).floor() as i32;
        Self(IVec3::new(axis(point.x), axis(point.y), axis(point.z)))
    }

    /// The cell whose file is `name`, like `3_0_-2.ron`: three integers, `_` between, written as
    /// Rust writes them, so each cell has one name.
    pub(crate) fn of_file(name: &str) -> Option<Self> {
        let stem = name.strip_suffix(".ron")?;
        let mut axes = stem.split('_').map(|axis| {
            let n: i32 = axis.parse().ok()?;
            (n.to_string() == axis).then_some(n)
        });
        let (x, y, z) = (axes.next()??, axes.next()??, axes.next()??);
        axes.next().is_none().then_some(Self(IVec3::new(x, y, z)))
    }

    /// The squared distance from `point` to the cell's nearest point, in `f64` for IEEE arithmetic
    /// on every platform, like the world's.
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

/// What a game makes of a folder's fileless cells
/// ([`Context::make_cells`](crate::Context::make_cells)): the first and last cell on each axis, and
/// a thread-safe function making each one's text.
#[derive(Clone)]
pub(crate) struct MadeCells {
    pub(crate) from: Cell,
    pub(crate) to: Cell,
    pub(crate) text: Arc<dyn Fn(Cell) -> String + Send + Sync>,
}

impl MadeCells {
    pub(crate) fn makes(&self, cell: Cell) -> bool {
        cell.0.cmpge(self.from.0).all() && cell.0.cmple(self.to.0).all()
    }
}

/// Where a cell's entities come from before it first comes in: its file, or what the game makes.
#[derive(Clone)]
pub(crate) enum CellSource {
    File,
    Made(MadeCells),
}

/// What a world holds of its cells beside its entities:
/// - the cells in, and those coming in as the next tick starts;
/// - the digest of each came-in cell's file (or the game's text), so what the cell keeps stands for
///   it and a peer or replay with another file is told;
/// - for a cell in that came from that text alone, the digest of what its roots save as they came
///   ([`came_as`]): one going out unchanged keeps nothing and returns from its source;
/// - what each cell out keeps;
/// - cells whose load was refused; they retry only after every viewer has gone.
///
/// Saved, so snapshots, joiners and server clients have it. A world without [`Cells`] holds
/// nothing, so its hash reads none.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CellState {
    pub(crate) cells_in: BTreeSet<Cell>,
    pub(crate) coming: BTreeSet<Cell>,
    pub(crate) files: BTreeMap<Cell, u64>,
    pub(crate) came_as: BTreeMap<Cell, u64>,
    pub(crate) kept: BTreeMap<Cell, Kept>,
    pub(crate) refused: BTreeSet<Cell>,
}

/// What a cell out keeps: its entities as scene texts that come back in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kept {
    pub(crate) texts: Vec<KeptText>,
}

/// A scene text a cell keeps, shared so copies copy a pointer, and its digest, which the world's
/// hash reads in its place.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeptText {
    pub(crate) text: Arc<str>,
    pub(crate) digest: u64,
}

impl KeptText {
    fn new(text: &str) -> Self {
        Self {
            digest: text_digest(text),
            text: text.into(),
        }
    }
}

impl CellState {
    /// Whether it holds nothing; the world's hash then leaves it out.
    pub(crate) fn is_empty(&self) -> bool {
        self.cells_in.is_empty()
            && self.coming.is_empty()
            && self.files.is_empty()
            && self.came_as.is_empty()
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
    // Empty in older snapshots: each cell in keeps what it holds as it goes.
    #[serde(default)]
    came_as: Vec<([i32; 3], u64)>,
    kept: Vec<SavedKept>,
    refused: Vec<[i32; 3]>,
}

#[derive(Serialize, Deserialize)]
struct SavedKept {
    cell: [i32; 3],
    texts: Vec<String>,
}

/// As JSON on one line: its strings keep a scene text's line breaks escaped.
impl Saved for CellState {
    const NAME: &'static str = "cells";

    fn save(&self, _: &World) -> Result<String> {
        let cells = |cells: &BTreeSet<Cell>| cells.iter().map(|cell| cell.0.to_array()).collect();
        let digests = |digests: &BTreeMap<Cell, u64>| {
            (digests.iter())
                .map(|(cell, &digest)| (cell.0.to_array(), digest))
                .collect()
        };
        let saved = SavedCells {
            cells_in: cells(&self.cells_in),
            coming: cells(&self.coming),
            files: digests(&self.files),
            came_as: digests(&self.came_as),
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
        let digests = |digests: Vec<([i32; 3], u64)>| {
            let digests = digests.into_iter();
            digests.map(|(at, digest)| (cell(at), digest)).collect()
        };
        Ok(Self {
            cells_in: saved.cells_in.into_iter().map(cell).collect(),
            coming: saved.coming.into_iter().map(cell).collect(),
            files: digests(saved.files),
            came_as: digests(saved.came_as),
            kept: kept.collect(),
            refused: saved.refused.into_iter().map(cell).collect(),
        })
    }
}

/// What a server's client sees ([`World::seen_by`]), held by its states ([`World::save_seen`],
/// [`World::hash_seen`]):
/// - by slot, whether the entity is hidden: a streamed root of an unseen cell, or under one, so no
///   shown entity has a hidden child and none lies past the end;
/// - the cells' state in place of the server's: the cells it sees in, nothing else.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Seen {
    pub(crate) hidden: Vec<bool>,
    pub(crate) cells: CellState,
}

/// The squared distance from `cell` to the nearest of `viewers` (position, radius) whose radius
/// plus `reach` reaches it.
fn nearest_viewer(viewers: &[([f64; 3], f64)], cell: Cell, reach: f64, size: f64) -> Option<f64> {
    let near = viewers.iter().filter_map(|&(at, radius)| {
        let distance = cell.distance_squared(at, size);
        let reach = radius + reach;
        (reach >= 0.0 && distance <= reach * reach).then_some(distance)
    });
    near.min_by(f64::total_cmp)
}

/// What a cell brings in, each text parsed by [`World::parse_scene`]: its file's with the digest,
/// if yet to come in; each text it keeps, in turn; and, if only its file's, what its roots save as
/// they come ([`came_as`]).
pub(crate) struct Entering {
    pub(crate) file: Option<(u64, Parsed)>,
    pub(crate) kept: Vec<Parsed>,
    pub(crate) came_as: Option<u64>,
}

/// The digest of what `file`'s roots (a cell's file text or the game's making) save as once given
/// [`Streamed`], loaded in a scratch world of `world`'s types and assets, on any thread: what they
/// save as going out if nothing changed them. `None` for a text that doesn't load or save there,
/// such as one referring to an outside entity; its cell then keeps what it holds as it goes out.
pub(crate) fn came_as(world: &World, file: &str) -> Option<u64> {
    let _zone = gg_core::zone!();
    let mut loaded = world.scratch();
    let roots = loaded.load_scene(file).ok()?;
    for &root in &roots {
        loaded.insert(root, Streamed).expect("loaded");
    }
    let saved = loaded.save_scene(&roots).ok()?;
    Some(text_digest(&saved))
}

/// What the cells' step does as a tick ends ([`World::plan_cells`]). Kept between ticks so a quiet
/// tick allocates nothing once it has grown. Cells are in order.
#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) leaving: Vec<Cell>,
    pub(crate) entering: Vec<Cell>,
    /// The streamed roots the tick left in cells out, which go out at once into their cells' keep,
    /// and those of the cells going out; each by cell, then ID.
    pub(crate) strays: Vec<(Cell, Entity)>,
    pub(crate) leavers: Vec<(Cell, Entity)>,
    /// The refused cells whose viewers have gone, which may retry.
    pub(crate) forgotten: Vec<Cell>,
    pub(crate) full: bool,
    /// The cells out within a viewer's radius plus 1.5 cells, each with how far past that radius it
    /// is, nearest first, at most [`MOST_CELLS`]: those to prepare ahead ([`Preparing`]), which
    /// changes nothing in the world.
    pub(crate) ahead: Vec<(f64, Cell)>,
    /// `ahead`'s cells, in order, to search.
    pub(crate) nearby: Vec<Cell>,
    /// Scratch for [`World::plan_cells`]: viewers (position, radius); streamed roots with cell and
    /// ID; made cells near a viewer; would-be cells in with squared distance to the nearest viewer;
    /// the walk under a root for a viewer.
    viewers: Vec<([f64; 3], f64)>,
    roots: Vec<(Cell, EntityId, Entity)>,
    made: Vec<Cell>,
    near: Vec<(f64, Cell)>,
    walk: Vec<Entity>,
}

impl Plan {
    pub(crate) fn is_empty(&self) -> bool {
        self.leaving.is_empty()
            && self.entering.is_empty()
            && self.strays.is_empty()
            && self.forgotten.is_empty()
            && !self.full
    }
}

impl World {
    /// The cells in, in order; none without [`Cells`].
    pub fn cells_in(&self) -> impl Iterator<Item = Cell> + '_ {
        let state = self.resource::<CellState>();
        state
            .into_iter()
            .flat_map(|state| state.cells_in.iter().copied())
    }

    /// The cell `point` is in, under the world's [`Cells`]: `None` without one or if its size isn't
    /// finite and positive.
    pub fn cell_at(&self, point: DVec3) -> Option<Cell> {
        let (_, cells, _) = self.world_cells()?;
        let size = cells.size;
        (size.is_finite() && size > 0.0).then(|| Cell::at(point, size))
    }

    /// Cells in and cells out that keep something, for the tool API's `status`; `None` if no cell
    /// ever came in.
    #[cfg(feature = "dev")]
    pub(crate) fn cell_counts(&self) -> Option<(usize, usize)> {
        let state = self.resource::<CellState>()?;
        Some((state.cells_in.len(), state.kept.len()))
    }

    /// The world's [`Cells`] (the lowest ID's), that ID, and how many entities have one.
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

    /// Empties what the world holds of its cells, like a scene change its entities. The resource
    /// stays: a client keeps a resource its server's state lacks, so its copy empties too.
    pub(crate) fn empty_cells(&mut self) {
        if let Some(state) = self.resource_mut::<CellState>() {
            *state = CellState::default();
        }
    }

    /// Fills `plan` with what the cells' step does as this tick ends, for cells of side `size`.
    /// `listed` are the cells with files, in order; `made` the first and last the game makes.
    /// - A cell farther than a viewer's radius plus half a cell from every viewer goes out, with
    ///   its streamed roots except those that are or hold a viewer.
    /// - A cell out within a viewer's radius comes in if it has a file, is made, keeps something or
    ///   holds a streamed root, and isn't refused; past [`MOST_CELLS`], the nearest first.
    /// - A streamed root in a cell out that doesn't come in goes out at once.
    /// - A refused cell beyond that reach of every viewer is forgotten.
    ///
    /// `extra` (position, radius) is a viewer too: a development build's look viewpoint while the
    /// run is stopped (`Context::stream_stopped`).
    pub(crate) fn plan_cells(
        &self,
        size: f32,
        listed: &[Cell],
        made: Option<(Cell, Cell)>,
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
            made: made_near,
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
            let at = self
                .pose(entity)
                .expect("alive")
                .matrix
                .translation
                .to_array();
            viewers.push((at, f64::from(viewer.radius)));
        }
        viewers.extend(extra);
        self.streamed_roots(size, roots);
        let size = f64::from(size);
        // The made cells within each viewer's reach, for preparing ahead, by [`looked`]'s box.
        made_near.clear();
        if let Some((from, to)) = made {
            for &(at, radius) in viewers.iter() {
                let reach = radius + 1.5 * size;
                let (low, high) = looked(at, reach, size, (from.0, to.0));
                for x in low.x..=high.x {
                    for y in low.y..=high.y {
                        for z in low.z..=high.z {
                            made_near.push(Cell(IVec3::new(x, y, z)));
                        }
                    }
                }
            }
        }
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
        // How far `cell` is past the nearest viewer's radius, if within one's radius plus 1.5
        // cells. A square root, so only for ordering what's prepared ahead, never the plan.
        let past = |cell: Cell| {
            let past = viewers.iter().filter_map(|&(at, radius)| {
                let reach = radius + 1.5 * size;
                let distance = cell.distance_squared(at, size);
                (distance <= reach * reach).then(|| distance.sqrt() - radius)
            });
            past.min_by(f64::total_cmp)
        };
        let made = made_near.iter().copied();
        for cell in (listed.iter().copied())
            .chain(made)
            .chain(kept.copied())
            .chain(rooted)
        {
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

    /// Fills `roots` with the streamed roots, each with its cell (by its [`Transform`], the origin
    /// without one) and ID, ordered by cell, then ID.
    pub(crate) fn streamed_roots(&self, size: f32, roots: &mut Vec<(Cell, EntityId, Entity)>) {
        roots.clear();
        for (entity, _) in self.query::<(Entity, &Streamed)>() {
            if self.parent(entity).expect("alive").is_some() {
                continue;
            }
            let at = self
                .get::<Transform>(entity)
                .map_or(DVec3::ZERO, |at| at.translation);
            let id = self.id(entity).expect("alive");
            roots.push((Cell::at(at, size), id, entity));
        }
        roots.sort_unstable_by_key(|&(cell, id, _)| (cell, id));
    }

    /// What player `player`'s client sees, as a server's states send it; `had` are the cells it
    /// saw. It sees:
    /// - of the cells in, each within the radius of a [`Viewer`] of its own (its or an ancestor's
    ///   [`Owner`] names the player), and each it saw within the radius plus half a cell;
    /// - every entity but the streamed roots of unseen cells, with what's under them, except one
    ///   that is or holds a viewer of its own.
    ///
    /// A viewer no player owns is no client's. Without [`Cells`], or with a size that isn't finite
    /// and positive, nothing is hidden and no cell seen.
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
                let at = self
                    .pose(entity)
                    .expect("alive")
                    .matrix
                    .translation
                    .to_array();
                viewers.push((at, f64::from(viewer.radius)));
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
                .map_or(DVec3::ZERO, |at| at.translation);
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

    /// Takes `roots`, streamed roots of `cell`, out of the world with everything under them, into
    /// the cell's keep as a scene text after its others: with the cell going out if `leaving`, else
    /// at once into a cell out. A cell going out whose roots save as they came ([`came_as`]) keeps
    /// nothing, and its file is no longer noted as come in, so it returns from its source. Returns
    /// how many roots went.
    ///
    /// Fails like [`save_scene`](Self::save_scene), taking none out: a root may refer to an entity
    /// that doesn't go with it.
    pub(crate) fn keep_cell(
        &mut self,
        cell: Cell,
        roots: &[Entity],
        leaving: bool,
    ) -> Result<usize> {
        let state = self.resource::<CellState>();
        let came_as = state.and_then(|state| state.came_as.get(&cell).copied());
        let came_as = came_as.filter(|_| leaving);
        // A cell that came with roots and goes with none saves as a scene of none.
        let text = match (roots, came_as) {
            ([], None) => None,
            (roots, _) => Some(self.save_scene(roots)?),
        };
        let unchanged = came_as.is_some() && came_as == text.as_deref().map(text_digest);
        for &root in roots {
            self.despawn(root).expect("a streamed root is alive");
        }
        let state = self.cell_state_mut();
        if leaving {
            state.cells_in.remove(&cell);
            state.came_as.remove(&cell);
        }
        if unchanged {
            state.files.remove(&cell);
        } else if let Some(text) = text.filter(|_| !roots.is_empty()) {
            state
                .kept
                .entry(cell)
                .or_default()
                .texts
                .push(KeptText::new(&text));
        }
        Ok(roots.len())
    }

    /// Takes `cell` out while the run is stopped
    /// ([`Context::stream_stopped`](crate::Context::stream_stopped)): `roots`, its streamed roots,
    /// go with everything under them, keeping nothing, and its file is no longer noted as come in,
    /// so the cell comes in from its file again.
    #[cfg(feature = "dev")]
    pub(crate) fn forget_cell(&mut self, cell: Cell, roots: &[Entity]) {
        for &root in roots {
            self.despawn(root).expect("a streamed root is alive");
        }
        let state = self.cell_state_mut();
        state.cells_in.remove(&cell);
        state.files.remove(&cell);
        state.came_as.remove(&cell);
    }

    /// Notes that `cell`'s file, of digest `digest`, now holds the cell as the world does, so the
    /// cell doesn't come in from it; forgets what its roots came as.
    #[cfg(feature = "dev")]
    pub(crate) fn note_filed(&mut self, cell: Cell, digest: u64) {
        self.cell_state_mut().files.insert(cell, digest);
        self.forget_came_as(cell);
    }

    /// Forgets what `cell`'s roots came as ([`came_as`]) when their source text changes, so the
    /// cell keeps what it holds as it goes out: returning from the old text would lose an edit.
    pub(crate) fn forget_came_as(&mut self, cell: Cell) {
        if let Some(state) = self.resource_mut::<CellState>() {
            state.came_as.remove(&cell);
        }
    }

    /// The cell whose file has path `author` from the asset root, under the world's [`Cells`].
    pub(crate) fn cell_of_file(&self, author: &str) -> Option<Cell> {
        let (_, cells, _) = self.world_cells()?;
        let name = author
            .strip_prefix(cells.folder.as_str())?
            .strip_prefix('/')?;
        Cell::of_file(name)
    }

    /// Like [`cell_of_file`](Self::cell_of_file), but only if the cell is in and its file has come
    /// in, so a save of the file applies.
    #[cfg(feature = "dev")]
    pub(crate) fn filed_cell_in(&self, author: &str) -> Option<Cell> {
        let cell = self.cell_of_file(author)?;
        let state = self.resource::<CellState>()?;
        (state.cells_in.contains(&cell) && state.files.contains_key(&cell)).then_some(cell)
    }

    /// What a cell's file should be written as, from `roots` (its streamed roots, with everything
    /// under them), if that differs from `file`, its text as last applied (`None` if it hasn't come
    /// in). Nothing for a rootless cell whose file hasn't come in. Texts are compared and written
    /// [canonical](Self::cell_canonical); a `file` that no longer loads differs.
    ///
    /// Fails like [`save_scene`](Self::save_scene) for `roots`.
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

    /// Scene `text` as a cell's file holds it: [canonical](Self::scene_canonical), roots without
    /// the [`Streamed`] a cell's load gives them.
    ///
    /// Fails like `scene_canonical`.
    #[cfg(feature = "dev")]
    pub(crate) fn cell_canonical(&self, text: &str) -> Result<String> {
        let mut loaded = self.scratch();
        let roots = loaded.load_scene(text)?;
        for &root in &roots {
            loaded.remove::<Streamed>(root).expect("loaded");
        }
        loaded.save_scene(&roots)
    }

    /// Scene `text` canonical with its roots given [`Streamed`], as a cell brings them in, to
    /// compare a cell file's save with the cell in.
    ///
    /// Fails like [`scene_canonical`](Self::scene_canonical).
    #[cfg(feature = "dev")]
    pub(crate) fn streamed_text(&self, text: &str) -> Result<String> {
        let mut loaded = self.scratch();
        let roots = loaded.load_scene(text)?;
        for &root in &roots {
            loaded.insert(root, Streamed).expect("loaded");
        }
        loaded.save_scene(&roots)
    }

    /// Notes that `cells` come in as the next tick starts ([`enter_cell`](Self::enter_cell)).
    pub(crate) fn note_coming(&mut self, cells: &[Cell]) {
        self.cell_state_mut().coming.extend(cells);
    }

    /// The cells coming in, in order, no longer noted.
    pub(crate) fn take_coming(&mut self) -> BTreeSet<Cell> {
        let state = self.resource_mut::<CellState>();
        state.map_or_else(BTreeSet::new, |state| mem::take(&mut state.coming))
    }

    /// Brings `cell` in with `entering`'s texts in turn, each root given [`Streamed`], noting the
    /// file's digest and what the roots came as, if given. Returns how many roots came. All or
    /// none: if a text is refused the cell stays as it was.
    ///
    /// Fails like [`commit_scene`](Self::commit_scene), such as for an ID a live entity has
    /// (`ecs.id_taken`).
    pub(crate) fn enter_cell(&mut self, cell: Cell, entering: Entering) -> Result<usize> {
        let Entering {
            file,
            kept,
            came_as,
        } = entering;
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
        if let Some(came_as) = came_as {
            state.came_as.insert(cell, came_as);
        }
        Ok(roots.len())
    }

    /// Notes that `cell`'s load was refused, so it doesn't retry while a viewer stays near.
    pub(crate) fn refuse_cell(&mut self, cell: Cell) {
        self.cell_state_mut().refused.insert(cell);
    }

    /// Forgets that `cells` were refused, so they retry as a viewer comes near.
    pub(crate) fn forget_refused(&mut self, cells: &[Cell]) {
        let state = self.cell_state_mut();
        for cell in cells {
            state.refused.remove(cell);
        }
    }

    /// `cell`'s file's path from the asset root, under the world's [`Cells`]; the folder is `?`
    /// without.
    pub(crate) fn cell_file(&self, cell: Cell) -> String {
        let folder = self
            .world_cells()
            .map(|(_, cells, _)| cells.folder.as_str());
        format!("{}/{cell}.ron", folder.unwrap_or("?"))
    }

    /// The first cell, in order, that both worlds brought in from files of different digests: its
    /// [path](Self::cell_file) and the digests of this world's file and `other`'s.
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

/// What a run keeps of its worlds' cells beside the world: each folder listed ([`Cells`]) with its
/// cells' files in order; what the game [makes](crate::Context::make_cells) of each folder's cells;
/// the warnings given, by code and subject, each once a run, so a cell refused every tick or a tick
/// rerun doesn't warn again; the [`Cells`] tie last warned of; and the cells' step's plan.
#[derive(Default)]
pub(crate) struct CellRun {
    pub(crate) listed: Vec<(String, Arc<[Cell]>)>,
    pub(crate) made: Vec<(String, MadeCells)>,
    warned: HashSet<(&'static str, String)>,
    pub(crate) tie: Option<(EntityId, usize)>,
    pub(crate) plan: Plan,
    pub(crate) preparing: Preparing,
    /// The cells whose files came in as the tick started, with their digests, for a recording.
    pub(crate) came: Vec<(Cell, u64)>,
    /// While the run is stopped, the cells found unlike their files as no viewer was near, which
    /// stay in, and the scene files' [change count](crate::scene_files::SceneFiles::changes) when
    /// found, after which they're looked at again.
    #[cfg(feature = "dev")]
    pub(crate) held: (Vec<Cell>, u64),
}

impl CellRun {
    /// Whether the warning of `code` about `about` is yet to be given; it now counts as given.
    pub(crate) fn first(&mut self, code: &'static str, about: impl fmt::Display) -> bool {
        self.warned.insert((code, about.to_string()))
    }

    pub(crate) fn made(&self, folder: &str) -> Option<&MadeCells> {
        let mut made = self.made.iter();
        made.find(|(made, _)| made == folder).map(|(_, made)| made)
    }

    /// Where `cell` of `folder` comes from before it first comes in: its file (`listed` are the
    /// cells with files), else what the game makes of it.
    pub(crate) fn source(&self, folder: &str, listed: &[Cell], cell: Cell) -> Option<CellSource> {
        if listed.binary_search(&cell).is_ok() {
            return Some(CellSource::File);
        }
        let made = self.made(folder).filter(|made| made.makes(cell));
        made.map(|made| CellSource::Made(made.clone()))
    }

    /// The cells [`list_cells`] lists in `folder`, once a run; none, warning `cells.listing`, for
    /// a folder it can't list.
    pub(crate) fn listed(
        &mut self,
        root: Option<&Path>,
        files: &Files,
        folder: &str,
    ) -> Arc<[Cell]> {
        if let Some((_, listed)) = self.listed.iter().find(|(listed, _)| listed == folder) {
            return Arc::clone(listed);
        }
        // The folder of cells the game makes needn't be there.
        let unfiled = self.made(folder).is_some()
            && files.packaged().is_none()
            && root.is_some_and(|root| !package::under(root, folder).exists());
        let listed = match unfiled {
            true => Ok(Arc::from([])),
            false => list_cells(root, files, folder),
        };
        let listed: Arc<[Cell]> = listed.unwrap_or_else(|error| {
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

/// The cells whose files `folder` holds, in order: its path from the asset root `root`, `/` between
/// its parts. The cells are those its file names ([`Cell::of_file`]) give, from the game's package
/// (`files`) in a run that reads one, else the disk.
///
/// Fails with `cells.listing` for a path that isn't inside the asset root, in a run without a
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
