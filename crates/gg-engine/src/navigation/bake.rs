//! Baking: the walkable surfaces of the world's static 3D colliders, as a navmesh. Their triangles
//! are voxelized into a heightfield, columns of solid spans; the spans an agent stands on are kept,
//! by the slope of the surface on top, the headroom above and the steps between neighbors; the
//! walkable area is eroded by the agent's radius; and the cells left are joined into rectangles,
//! each a convex polygon whose edges shared with its neighbors' are portals.
//!
//! Recast's pipeline is the reference as far as the erosion (Mikko Mononen's recastnavigation:
//! `Recast/Source/RecastRasterization.cpp`, `RecastFilter.cpp`, `RecastContour.cpp`'s neighbors and
//! `RecastArea.cpp`'s `rcErodeWalkableArea`), but that a collider's convex shape is voxelized as a
//! solid, a span from its lowest point in a column to its highest, so that a wall standing in a floor
//! leaves no floor inside it, and that spans merge with the flag of the higher top, or of either
//! where their tops are within a step. Recast's regions, contours and polygon mesh give way to
//! rectangles of cells, grown greedily, each as far along X as its cells join and then along Z, row
//! by row: they stay correct across slopes, steps and floors over floors, since each cell keeps its
//! own floor and a rectangle holds only cells joined to their neighbors, at the cost of more polygons
//! than Recast makes where walls cross the grid's axes, whose staircase of cells splits into
//! slivers, and of paths that bend at a rectangle's corner where a larger polygon would let them run
//! straight.

use gg_core::math::{Vec2, Vec3, libm};
use gg_core::{Error, Result};

use super::mesh::{Cell, NavMesh, Polygon, Portal};

/// No span, open span or polygon, as a link holds it.
const NONE: u32 = u32::MAX;

/// The most columns a heightfield has: 2^22, about 512 units square at cells of 0.25, so that a
/// bake's memory, some 4 bytes a column and 12 a span, stays within tens of megabytes.
const MAX_COLUMNS: u64 = 1 << 22;

/// The most levels a heightfield counts its spans' heights in: a span's top reaches one past
/// them, and the level past that, `u16::MAX`, marks an open span the erosion took away.
const MAX_LEVELS: u64 = u16::MAX as u64 - 2;

/// The ways to a column's neighbors, along X and Z, as Recast numbers them: -X, +Z, +X, -Z.
const WAYS: [(i32, i32); 4] = [(-1, 0), (0, 1), (1, 0), (0, -1)];

/// The fraction of a cell's area under which a convex shape's triangle in it counts for nothing, as
/// a vertical side does, which covers none: so that a box whose side lies on a column's edge doesn't
/// fill the column beside it.
const SLIVER: f32 = 1e-3;

/// What a bake takes: the heightfield's bounds and cells, and the agent's size, in the world's
/// units, the slope it climbs in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub(super) min: Vec3,
    pub(super) max: Vec3,
    pub(super) cell_size: f32,
    pub(super) cell_height: f32,
    pub(super) radius: f32,
    pub(super) height: f32,
    pub(super) climb: f32,
    pub(super) max_slope: f32,
}

/// A collider's triangles in the world's space, as a bake takes them: bounding a convex solid,
/// which is voxelized whole, or a surface, as a mesh collider's are, voxelized one triangle at a
/// time, each its own span, as Recast's are. A solid's triangles wind counterclockwise as seen from
/// outside it, and a surface's as seen from where it's walked on.
pub(super) struct Shape {
    pub(super) triangles: Vec<[Vec3; 3]>,
    pub(super) solid: bool,
}

/// The heightfield's grid: its lowest corner, its cells' sizes across and up and their inverses,
/// its columns along X and Z, and the levels its spans' heights are counted in up to its top.
struct Grid {
    min: Vec3,
    size: f32,
    inverse: f32,
    height: f32,
    inverse_height: f32,
    width: u32,
    depth: u32,
    levels: u16,
    /// How tall it is, from its lowest corner to its top.
    top: f32,
}

impl Grid {
    /// The grid of `settings`, failing with `navigation.bad_volume` for bounds, cells or an agent
    /// that aren't numbers or have no size, and with `navigation.too_large` for more than
    /// [`MAX_COLUMNS`] columns or [`MAX_LEVELS`] levels, counted without overflowing however many.
    fn new(settings: &Settings) -> Result<Self> {
        let Settings {
            min,
            max,
            cell_size,
            cell_height,
            radius,
            height,
            climb,
            max_slope,
        } = *settings;
        let finite = [cell_size, cell_height, radius, height, climb, max_slope];
        let sized = (max - min).cmpgt(Vec3::ZERO).all() && cell_size > 0.0 && cell_height > 0.0;
        let positive = [radius, height, climb, max_slope].iter().all(|&x| x >= 0.0);
        if !(min.is_finite() && max.is_finite() && finite.iter().all(|x| x.is_finite())) || !sized {
            let message = "its box and cells need sizes, and its agent's sizes numbers";
            return Err(Error::new("navigation.bad_volume", message));
        }
        if !positive {
            let message = "its agent's radius, height, climb and slope are none of them negative";
            return Err(Error::new("navigation.bad_volume", message));
        }
        // Saturating, as a cast does past the integer's range, and as the product does.
        let cells = |extent: f32, size: f32| libm::ceilf(extent / size) as u64;
        let width = cells(max.x - min.x, cell_size).max(1);
        let depth = cells(max.z - min.z, cell_size).max(1);
        let levels = cells(max.y - min.y, cell_height).max(1);
        if width.saturating_mul(depth) > MAX_COLUMNS || levels > MAX_LEVELS {
            let message = format!(
                "it holds {width} by {depth} columns of {levels} levels: more than {MAX_COLUMNS} \
                 columns or {MAX_LEVELS} levels, which larger cells would make fewer"
            );
            return Err(Error::new("navigation.too_large", message));
        }
        Ok(Self {
            min,
            size: cell_size,
            inverse: 1.0 / cell_size,
            height: cell_height,
            inverse_height: 1.0 / cell_height,
            width: width as u32,
            depth: depth as u32,
            levels: levels as u16,
            top: max.y - min.y,
        })
    }

    /// The levels of a span from `low` to `high` above the grid's lowest corner, as Recast snaps
    /// them, down and up, at least one apart; nothing for one wholly below or above the grid.
    fn levels_of(&self, low: f32, high: f32) -> Option<(u16, u16)> {
        if high < 0.0 || low > self.top {
            return None;
        }
        let low = if low < 0.0 { 0.0 } else { low };
        let high = if high > self.top { self.top } else { high };
        let levels = i32::from(self.levels);
        let min = ((low * self.inverse_height).floor() as i32).clamp(0, levels);
        let max = ((high * self.inverse_height).ceil() as i32).clamp(min + 1, levels + 1);
        Some((min as u16, max as u16))
    }

    /// The level a height `high` above the lowest corner snaps up to, as a span's top does.
    fn level_of(&self, high: f32) -> i32 {
        (high * self.inverse_height).ceil() as i32
    }

    /// The range of columns, along `axis`'s, that `low` to `high` along it cross, clamped to the
    /// grid's; nothing for a range wholly outside it.
    fn columns(&self, low: f32, high: f32, min: f32, cells: u32) -> Option<(u32, u32)> {
        let first = ((low - min) * self.inverse).floor();
        let last = ((high - min) * self.inverse).floor();
        if last < 0.0 || first >= cells as f32 {
            return None;
        }
        let first = if first > 0.0 { first as u32 } else { 0 };
        let last = if last < (cells - 1) as f32 {
            last as u32
        } else {
            cells - 1
        };
        Some((first, last))
    }
}

/// A span of solid in a column of the heightfield: from level `min` to `max`, whether its top is
/// walkable, and the next span up, in its pool.
#[derive(Clone, Copy)]
struct Span {
    min: u16,
    max: u16,
    walkable: bool,
    next: u32,
}

/// The heightfield: each column's spans, from the lowest up, linked in a pool, as Recast keeps
/// them, the spans merged away free for those after.
struct Heightfield {
    heads: Vec<u32>,
    spans: Vec<Span>,
    free: u32,
}

impl Heightfield {
    fn new(grid: &Grid) -> Self {
        Self {
            heads: vec![NONE; (grid.width * grid.depth) as usize],
            spans: Vec::new(),
            free: NONE,
        }
    }

    /// Adds a span from `min` to `max` to column `column`, merged with each it overlaps or touches:
    /// its top walkable as the higher top's, or as either's where the tops are within `climb`.
    fn add(&mut self, column: usize, mut min: u16, mut max: u16, mut walkable: bool, climb: u16) {
        let mut before = NONE;
        let mut at = self.heads[column];
        while at != NONE {
            let span = self.spans[at as usize];
            if span.min > max {
                break;
            }
            if span.max < min {
                before = at;
                at = span.next;
                continue;
            }
            walkable = if span.max.abs_diff(max) <= climb {
                walkable || span.walkable
            } else if span.max > max {
                span.walkable
            } else {
                walkable
            };
            min = min.min(span.min);
            max = max.max(span.max);
            let next = span.next;
            self.spans[at as usize].next = self.free;
            self.free = at;
            match before {
                NONE => self.heads[column] = next,
                before => self.spans[before as usize].next = next,
            }
            at = next;
        }
        let span = Span {
            min,
            max,
            walkable,
            next: at,
        };
        let index = match self.free {
            NONE => {
                self.spans.push(span);
                (self.spans.len() - 1) as u32
            }
            free => {
                self.free = self.spans[free as usize].next;
                self.spans[free as usize] = span;
                free
            }
        };
        match before {
            NONE => self.heads[column] = index,
            before => self.spans[before as usize].next = index,
        }
    }

    /// Column `column`'s spans, from the lowest up, by their indices.
    fn column(&self, column: usize) -> impl Iterator<Item = u32> + '_ {
        let mut at = self.heads[column];
        std::iter::from_fn(move || {
            let this = at;
            (this != NONE).then(|| {
                at = self.spans[this as usize].next;
                this
            })
        })
    }
}

/// The most vertices a triangle clipped to a column has: 3, and one more for each of the four cuts,
/// with room to spare for rounding's.
const CLIPPED: usize = 12;

/// A polygon clipped from a triangle, its vertices and how many.
#[derive(Clone, Copy)]
struct Clipped {
    vertices: [Vec3; CLIPPED],
    count: usize,
}

impl Clipped {
    const EMPTY: Self = Self {
        vertices: [Vec3::ZERO; CLIPPED],
        count: 0,
    };

    fn push(&mut self, vertex: Vec3) {
        if self.count < CLIPPED {
            self.vertices[self.count] = vertex;
            self.count += 1;
        }
    }

    fn vertices(&self) -> &[Vec3] {
        &self.vertices[..self.count]
    }
}

/// Divides convex polygon `polygon` where its coordinate `axis` is `at`: into what's at or before
/// it and what's at or after it, a vertex on the line going to both, as Recast's `dividePoly`.
fn divide(polygon: &Clipped, at: f32, axis: usize) -> (Clipped, Clipped) {
    let (mut before, mut after) = (Clipped::EMPTY, Clipped::EMPTY);
    let vertices = polygon.vertices();
    let off = |vertex: Vec3| at - vertex[axis];
    for (a, &vertex) in vertices.iter().enumerate() {
        let previous = vertices[(a + vertices.len() - 1) % vertices.len()];
        let (da, db) = (off(vertex), off(previous));
        if (da >= 0.0) != (db >= 0.0) {
            let s = db / (db - da);
            let cut = previous + (vertex - previous) * s;
            before.push(cut);
            after.push(cut);
            if da > 0.0 {
                before.push(vertex);
            } else if da < 0.0 {
                after.push(vertex);
            }
        } else {
            if da >= 0.0 {
                before.push(vertex);
                if da != 0.0 {
                    continue;
                }
            }
            after.push(vertex);
        }
    }
    (before, after)
}

/// The area polygon `polygon` covers across, in the XZ plane.
fn area(polygon: &Clipped) -> f32 {
    let vertices = polygon.vertices();
    let mut twice = 0.0;
    for (i, a) in vertices.iter().enumerate() {
        let b = vertices[(i + 1) % vertices.len()];
        twice += a.x * b.z - b.x * a.z;
    }
    twice.abs() * 0.5
}

/// Clips `triangle` to each column of `grid` it crosses, as Recast's `rasterizeTri` does, row by row
/// along Z and then column by column along X, and gives `each` each column's X and Z, the least and
/// greatest heights of the triangle in it above the grid's lowest corner, whether below or above
/// the grid, and the area it covers there. A triangle with a vertex that isn't a number gives
/// nothing.
fn rasterize(grid: &Grid, triangle: &[Vec3; 3], each: &mut impl FnMut(u32, u32, f32, f32, f32)) {
    if !triangle.iter().all(|vertex| vertex.is_finite()) {
        return;
    }
    let [a, b, c] = *triangle;
    let (low, high) = bounds(a, b);
    let (low, high) = (bounds(low, c).0, bounds(high, c).1);
    // The first and last rows, or columns, the triangle's bounds cross, from the one before the
    // grid's first, so that the first cut leaves out what's before the grid; nothing for bounds
    // wholly outside it.
    let range = |low: f32, high: f32, min: f32, cells: u32| {
        let first = ((low - min) * grid.inverse).floor();
        let last = ((high - min) * grid.inverse).floor();
        let cells = cells as i32;
        (last >= 0.0 && first < cells as f32).then(|| {
            (
                (first as i32).clamp(-1, cells - 1),
                (last as i32).min(cells - 1),
            )
        })
    };
    let Some((z0, z1)) = range(low.z, high.z, grid.min.z, grid.depth) else {
        return;
    };
    if range(low.x, high.x, grid.min.x, grid.width).is_none() {
        return;
    }
    let mut rest = Clipped::EMPTY;
    for vertex in [a, b, c] {
        rest.push(vertex);
    }
    let extent = |values: &mut dyn Iterator<Item = f32>| {
        values.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), at| {
            (if at < lo { at } else { lo }, if at > hi { at } else { hi })
        })
    };
    for z in z0..=z1 {
        let cut = grid.min.z + z as f32 * grid.size + grid.size;
        let (row, after) = divide(&rest, cut, 2);
        rest = after;
        if row.count < 3 || z < 0 {
            continue;
        }
        let (min_x, max_x) = extent(&mut row.vertices().iter().map(|vertex| vertex.x));
        let Some((x0, x1)) = range(min_x, max_x, grid.min.x, grid.width) else {
            continue;
        };
        let mut rest_of_row = row;
        for x in x0..=x1 {
            let cut = grid.min.x + x as f32 * grid.size + grid.size;
            let (column, after) = divide(&rest_of_row, cut, 0);
            rest_of_row = after;
            if column.count < 3 || x < 0 {
                continue;
            }
            let (min_y, max_y) = extent(&mut column.vertices().iter().map(|vertex| vertex.y));
            let floor = grid.min.y;
            each(
                x as u32,
                z as u32,
                min_y - floor,
                max_y - floor,
                area(&column),
            );
        }
    }
}

/// Whether a triangle's surface is one an agent walks on: facing up no steeper than the slope
/// whose cosine is `walkable`, as it winds.
fn walkable(triangle: &[Vec3; 3], walkable: f32) -> bool {
    let [a, b, c] = *triangle;
    let normal = (b - a).cross(c - a);
    let length = normal.length();
    length > 0.0 && normal.y / length >= walkable
}

/// What a convex solid holds in a column as its triangles are clipped to it: its least and
/// greatest heights there, and the greatest of its walkable surfaces'.
#[derive(Clone, Copy)]
struct Extent {
    low: f32,
    high: f32,
    walked: f32,
}

impl Extent {
    const NONE: Self = Self {
        low: f32::INFINITY,
        high: f32::NEG_INFINITY,
        walked: f32::NEG_INFINITY,
    };
}

/// Voxelizes `shape` into `field` on `grid`: a solid's triangles as one span a column, from its
/// lowest point there to its highest, cut to the grid's height, its top walkable if a walkable
/// surface of it reaches within `climb` of it inside the grid; a surface's as a span for each
/// triangle in each column, as Recast's are.
fn voxelize(
    grid: &Grid,
    field: &mut Heightfield,
    shape: &Shape,
    slope: f32,
    climb: u16,
    extents: &mut Vec<Extent>,
) {
    let width = grid.width as usize;
    if !shape.solid {
        for triangle in &shape.triangles {
            let ys = triangle.map(|vertex| vertex.y - grid.min.y);
            if ys.iter().all(|&y| y > grid.top) || ys.iter().all(|&y| y < 0.0) {
                continue;
            }
            let walks = walkable(triangle, slope);
            rasterize(grid, triangle, &mut |x, z, low, high, _| {
                if let Some((min, max)) = grid.levels_of(low, high) {
                    let column = z as usize * width + x as usize;
                    field.add(column, min, max, walks, climb);
                }
            });
        }
        return;
    }
    // The columns the solid's bounds cross, which its extents are kept for.
    let corners = shape.triangles.iter().flatten();
    let (low, high) = corners.fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(low, high), &at| (bounds(low, at).0, bounds(high, at).1),
    );
    let (Some((x0, x1)), Some((z0, z1))) = (
        grid.columns(low.x, high.x, grid.min.x, grid.width),
        grid.columns(low.z, high.z, grid.min.z, grid.depth),
    ) else {
        return;
    };
    let across = (x1 - x0 + 1) as usize;
    extents.clear();
    extents.resize(across * (z1 - z0 + 1) as usize, Extent::NONE);
    let least = SLIVER * grid.size * grid.size;
    for triangle in &shape.triangles {
        let walks = walkable(triangle, slope);
        rasterize(grid, triangle, &mut |x, z, low, high, covered| {
            if covered <= least || x < x0 || x > x1 || z < z0 || z > z1 {
                return;
            }
            let extent = &mut extents[(z - z0) as usize * across + (x - x0) as usize];
            if low < extent.low {
                extent.low = low;
            }
            if high > extent.high {
                extent.high = high;
            }
            if walks && high > extent.walked {
                extent.walked = high;
            }
        });
    }
    for (i, extent) in extents.iter().enumerate() {
        let Some((min, max)) = grid.levels_of(extent.low, extent.high) else {
            continue;
        };
        // A top above the grid is out of the volume, and walked by none of its agents.
        let walked = grid.level_of(extent.walked);
        let walks = extent.walked > f32::NEG_INFINITY
            && extent.high <= grid.top
            && i32::from(max) - walked <= i32::from(climb);
        let (x, z) = (x0 as usize + i % across, z0 as usize + i / across);
        field.add(z * width + x, min, max, walks, climb);
    }
}

/// An open span, a walkable floor and the space above it: its floor's level, the level of the
/// solid above it, and the open span it joins on each way, as [`WAYS`] numbers them, if any.
#[derive(Clone, Copy)]
struct Open {
    floor: u16,
    ceiling: u32,
    links: [u32; 4],
}

/// The heightfield's open spans, each column's from the lowest up, where they start in `opens` by
/// column, and the grid's size.
struct Compact {
    width: u32,
    depth: u32,
    starts: Vec<u32>,
    opens: Vec<Open>,
}

impl Compact {
    /// The open spans of column `x`, `z`, by their indices.
    fn column(&self, x: u32, z: u32) -> std::ops::Range<u32> {
        let at = (z * self.width + x) as usize;
        self.starts[at]..self.starts[at + 1]
    }

    /// What open span `open` joins on way `way`, if any.
    fn link(&self, open: u32, way: usize) -> Option<u32> {
        let link = self.opens[open as usize].links[way];
        (link != NONE).then_some(link)
    }
}

/// The open spans over `field`'s walkable tops, each joined to the one beside it on each way that
/// an agent of `height` levels steps onto, within `climb` levels, with its headroom between them.
fn compact(grid: &Grid, field: &Heightfield, height: u16, climb: u16) -> Compact {
    let (width, depth) = (grid.width, grid.depth);
    let mut starts = Vec::with_capacity((width * depth) as usize + 1);
    let mut opens = Vec::new();
    for column in 0..(width * depth) as usize {
        starts.push(opens.len() as u32);
        let mut spans = field.column(column).peekable();
        while let Some(at) = spans.next() {
            let span = field.spans[at as usize];
            let ceiling = spans
                .peek()
                .map_or(u32::MAX, |&next| u32::from(field.spans[next as usize].min));
            if span.walkable {
                opens.push(Open {
                    floor: span.max,
                    ceiling,
                    links: [NONE; 4],
                });
            }
        }
    }
    starts.push(opens.len() as u32);
    let mut compact = Compact {
        width,
        depth,
        starts,
        opens,
    };
    for z in 0..depth {
        for x in 0..width {
            for open in compact.column(x, z) {
                let here = compact.opens[open as usize];
                for (way, (dx, dz)) in WAYS.into_iter().enumerate() {
                    let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                    if nx < 0 || nz < 0 || nx >= width as i32 || nz >= depth as i32 {
                        continue;
                    }
                    let beside = compact.column(nx as u32, nz as u32);
                    let joined = beside.into_iter().find(|&other| {
                        let there = compact.opens[other as usize];
                        let bottom = u32::from(here.floor.max(there.floor));
                        let top = here.ceiling.min(there.ceiling);
                        let room = top.saturating_sub(bottom) >= u32::from(height);
                        room && here.floor.abs_diff(there.floor) <= climb
                    });
                    if let Some(other) = joined {
                        compact.opens[open as usize].links[way] = other;
                    }
                }
            }
        }
    }
    compact
}

/// Erodes `compact`'s walkable area by `radius` cells, as Recast's `rcErodeWalkableArea` does: each
/// open span's distance from the area's edge, where a span lacks a neighbor, by two passes of a
/// chamfer that counts 2 a step along an axis and 3 across a diagonal, and those nearer the edge
/// than the radius taken away, their links with them.
fn erode(compact: &mut Compact, radius: u16) {
    let count = compact.opens.len();
    let mut distance = vec![u16::MAX; count];
    for (open, at) in compact.opens.iter().zip(&mut distance) {
        if open.links.contains(&NONE) {
            *at = 0;
        }
    }
    let relax = |distance: &mut Vec<u16>, open: u32, from: u32, step: u16| {
        let through = distance[from as usize].saturating_add(step);
        if through < distance[open as usize] {
            distance[open as usize] = through;
        }
    };
    let (width, depth) = (compact.width, compact.depth);
    for z in 0..depth {
        for x in 0..width {
            for open in compact.column(x, z) {
                // From -X, and -X then -Z; from -Z, and -Z then +X.
                for (way, then) in [(0, 3), (3, 2)] {
                    if let Some(beside) = compact.link(open, way) {
                        relax(&mut distance, open, beside, 2);
                        if let Some(corner) = compact.link(beside, then) {
                            relax(&mut distance, open, corner, 3);
                        }
                    }
                }
            }
        }
    }
    for z in (0..depth).rev() {
        for x in (0..width).rev() {
            for open in compact.column(x, z) {
                // From +X, and +X then +Z; from +Z, and +Z then -X.
                for (way, then) in [(2, 1), (1, 0)] {
                    if let Some(beside) = compact.link(open, way) {
                        relax(&mut distance, open, beside, 2);
                        if let Some(corner) = compact.link(beside, then) {
                            relax(&mut distance, open, corner, 3);
                        }
                    }
                }
            }
        }
    }
    let least = radius.saturating_mul(2);
    let kept: Vec<bool> = distance.iter().map(|&at| at >= least).collect();
    for (open, &keep) in compact.opens.iter_mut().zip(&kept) {
        for link in &mut open.links {
            if !keep || (*link != NONE && !kept[*link as usize]) {
                *link = NONE;
            }
        }
    }
    // An open span taken away is marked by a floor no level reaches.
    for (open, keep) in compact.opens.iter_mut().zip(kept) {
        if !keep {
            open.floor = u16::MAX;
        }
    }
}

/// Bakes the navmesh of `shapes`, in the order given, as `settings` says: failing as
/// [`Grid::new`] does for settings it can't.
pub(super) fn bake(settings: &Settings, shapes: &[Shape]) -> Result<NavMesh> {
    let grid = Grid::new(settings)?;
    let levels = |extent: f32, round: fn(f32) -> f32| {
        let levels = round(extent * grid.inverse_height);
        if levels < f32::from(u16::MAX) {
            levels as u16
        } else {
            u16::MAX
        }
    };
    let climb = levels(settings.climb, f32::floor);
    let height = levels(settings.height, f32::ceil);
    let radius = libm::ceilf(settings.radius * grid.inverse);
    let radius = if radius < f32::from(u16::MAX) {
        radius as u16
    } else {
        u16::MAX
    };
    let slope = if settings.max_slope < std::f32::consts::FRAC_PI_2 {
        settings.max_slope
    } else {
        std::f32::consts::FRAC_PI_2
    };
    let slope = libm::cosf(slope);

    let mut field = Heightfield::new(&grid);
    let mut extents = Vec::new();
    for shape in shapes {
        voxelize(&grid, &mut field, shape, slope, climb, &mut extents);
    }
    filter(&mut field, climb, height);
    let mut compact = compact(&grid, &field, height, climb);
    erode(&mut compact, radius);
    Ok(partition(&grid, &compact, settings))
}

/// Recast's filters of what an agent stands on, as `rcFilterLowHangingWalkableObstacles` and
/// `rcFilterWalkableLowHeightSpans`: a span that isn't walkable whose top is within `climb` of a
/// walkable top just below it becomes walkable, as a curb does; and a walkable span with less than
/// `height` of room above it isn't.
fn filter(field: &mut Heightfield, climb: u16, height: u16) {
    for column in 0..field.heads.len() {
        let mut below: Option<(bool, u16)> = None;
        let mut at = field.heads[column];
        while at != NONE {
            let span = &mut field.spans[at as usize];
            let was = span.walkable;
            if let Some((true, top)) = below
                && !span.walkable
                && span.max.saturating_sub(top) <= climb
            {
                span.walkable = true;
            }
            below = Some((was, span.max));
            at = span.next;
        }
        let mut at = field.heads[column];
        while at != NONE {
            let span = field.spans[at as usize];
            let ceiling = match span.next {
                NONE => u32::MAX,
                next => u32::from(field.spans[next as usize].min),
            };
            if ceiling - u32::from(span.max) < u32::from(height) {
                field.spans[at as usize].walkable = false;
            }
            at = span.next;
        }
    }
}

/// Joins `compact`'s open spans left into rectangles, the navmesh's polygons, in order by Z, then
/// X, then floor: each the best anchored at the first span left, by its area times its shorter
/// side, its rows down along Z each as wide along X as its spans join each other and the row
/// before's, from the anchor's column, the first row's width narrowing as the rows go; with each
/// run of an edge that its spans join another polygon's on, a portal.
fn partition(grid: &Grid, compact: &Compact, settings: &Settings) -> NavMesh {
    let kept = |open: u32| compact.opens[open as usize].floor != u16::MAX;
    let mut owner = vec![NONE; compact.opens.len()];
    let mut polygons = Vec::new();
    // Each polygon's open spans, row by row, where each polygon's start.
    let mut held = Vec::new();
    let mut starts = Vec::new();
    // The rows tried from an anchor, each as wide as the first, and how wide each joins.
    let mut tried = Vec::new();
    let mut widths = Vec::new();
    for z in 0..grid.depth {
        for x in 0..grid.width {
            for open in compact.column(x, z) {
                if !kept(open) || owner[open as usize] != NONE {
                    continue;
                }
                let free = |open: u32| owner[open as usize] == NONE;
                tried.clear();
                tried.push(open);
                let mut last = open;
                while let Some(right) = compact.link(last, 2).filter(|&right| free(right)) {
                    tried.push(right);
                    last = right;
                }
                let first = tried.len();
                widths.clear();
                widths.push(first);
                // The best so far, its area times its shorter side, so that a squarer rectangle
                // goes before a strip of the same area, and its rows and width.
                let score = |rows: usize, width: usize| rows * width * rows.min(width);
                let mut best = (score(1, first), 1, first);
                loop {
                    let rows = widths.len();
                    let above = (rows - 1) * first;
                    tried.resize(above + 2 * first, NONE);
                    let mut width = 0;
                    while width < widths[rows - 1] {
                        let down = compact
                            .link(tried[above + width], 1)
                            .filter(|&down| free(down));
                        let beside = |down| {
                            width == 0
                                || compact.link(tried[above + first + width - 1], 2) == Some(down)
                        };
                        match down.filter(|&down| beside(down)) {
                            Some(down) => tried[above + first + width] = down,
                            None => break,
                        }
                        width += 1;
                    }
                    if width == 0 {
                        break;
                    }
                    widths.push(width);
                    if score(rows + 1, width) > best.0 {
                        best = (score(rows + 1, width), rows + 1, width);
                    }
                }
                let (_, depth, width) = best;
                let index = polygons.len() as u32;
                starts.push(held.len() as u32);
                for row in tried.chunks(first).take(depth) {
                    held.extend_from_slice(&row[..width]);
                }
                for &open in &held[starts[index as usize] as usize..] {
                    owner[open as usize] = index;
                }
                polygons.push(Polygon {
                    x,
                    z,
                    width: width as u32,
                    depth: depth as u32,
                    portals: [0, 0],
                });
            }
        }
    }
    starts.push(held.len() as u32);

    let mut portals = Vec::new();
    for (index, polygon) in polygons.iter_mut().enumerate() {
        let first = portals.len() as u32;
        let cells = &held[starts[index] as usize..starts[index + 1] as usize];
        let (width, depth) = (polygon.width, polygon.depth);
        let at = |i: u32, j: u32| cells[(j * width + i) as usize];
        // Each edge, as the way out of it, and its cells along it with their grid corners: -X,
        // +Z, +X, -Z, each from its lowest corner.
        let (x, z) = (polygon.x, polygon.z);
        let edges: [(usize, u32); 4] = [(0, depth), (1, width), (2, depth), (3, width)];
        for (way, length) in edges {
            let mut run: Option<(u32, u32)> = None;
            for k in 0..=length {
                let to = (k < length)
                    .then(|| {
                        let open = match way {
                            0 => at(0, k),
                            1 => at(k, depth - 1),
                            2 => at(width - 1, k),
                            _ => at(k, 0),
                        };
                        compact.link(open, way).map(|beside| owner[beside as usize])
                    })
                    .flatten();
                if let Some((other, start)) = run
                    && to != Some(other)
                {
                    let corner = |along: u32| match way {
                        0 => [x, z + along],
                        1 => [x + along, z + depth],
                        2 => [x + width, z + along],
                        _ => [x + along, z],
                    };
                    portals.push(Portal {
                        to: other,
                        a: corner(start),
                        b: corner(k),
                    });
                    run = None;
                }
                if let Some(other) = to
                    && run.is_none()
                {
                    run = Some((other, k));
                }
            }
        }
        polygon.portals = [first, portals.len() as u32];
    }

    let mut columns = Vec::with_capacity((grid.width * grid.depth) as usize + 1);
    let mut cells = Vec::new();
    for z in 0..grid.depth {
        for x in 0..grid.width {
            columns.push(cells.len() as u32);
            for open in compact.column(x, z).filter(|&open| kept(open)) {
                cells.push(Cell {
                    floor: compact.opens[open as usize].floor,
                    polygon: owner[open as usize],
                });
            }
        }
    }
    columns.push(cells.len() as u32);
    NavMesh {
        min: grid.min,
        cell_size: grid.size,
        cell_height: grid.height,
        width: grid.width,
        depth: grid.depth,
        height: settings.height,
        climb: settings.climb,
        polygons,
        portals,
        columns,
        cells,
    }
}

/// The triangles of a box, counterclockwise as seen from outside it: as wide, tall and deep as
/// `half` twice, around `middle` in its own space, which `place` takes to the world's.
pub(super) fn cuboid(half: Vec3, middle: Vec3, place: impl Fn(Vec3) -> Vec3) -> Vec<[Vec3; 3]> {
    let mut triangles = Vec::with_capacity(12);
    for axis in 0..3 {
        // The face's other two axes, in turn after it, so that `u` cross `v` points along it.
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for side in [-1.0, 1.0] {
            let corner = |a: f32, b: f32| {
                let mut at = Vec3::ZERO;
                at[axis] = side * half[axis];
                at[u] = a * half[u];
                at[v] = b * half[v];
                place(middle + at)
            };
            let quad = [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ];
            let [a, b, c, d] = if side > 0.0 {
                quad
            } else {
                [quad[0], quad[3], quad[2], quad[1]]
            };
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
    }
    triangles
}

/// How many segments a sphere's or a capsule's rings have, and how many bands its latitudes make,
/// pole to pole: fine enough that a sphere's top is within some 2% of its radius of a cell's.
const SEGMENTS: usize = 16;
const BANDS: usize = 8;

/// The triangles of a capsule, counterclockwise as seen from outside it: what's within `radius` of
/// the segment along Y from `-half` to `half` around `middle` in its own space, which `place` takes
/// to the world's; a sphere at a `half` of 0.
pub(super) fn capsule(
    radius: f32,
    half: f32,
    middle: Vec3,
    place: impl Fn(Vec3) -> Vec3,
) -> Vec<[Vec3; 3]> {
    // Each ring, from the bottom up, the equator twice, below and above the segment's middle.
    let mut rings: Vec<[Vec3; SEGMENTS]> = Vec::with_capacity(BANDS + 2);
    for band in 0..=BANDS + 1 {
        let (latitude, lift) = if band <= BANDS / 2 {
            (band, -half)
        } else {
            (band - 1, half)
        };
        let angle = std::f32::consts::PI * (latitude as f32 / BANDS as f32 - 0.5);
        let (sin, cos) = match latitude {
            0 => (-1.0, 0.0),
            l if l == BANDS => (1.0, 0.0),
            _ => libm::sincosf(angle),
        };
        rings.push(std::array::from_fn(|segment| {
            let around = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let (s, c) = libm::sincosf(around);
            let at = Vec3::new(cos * c * radius, sin * radius + lift, cos * s * radius);
            place(middle + at)
        }));
    }
    let mut triangles = Vec::with_capacity(rings.len() * SEGMENTS * 2);
    for pair in rings.windows(2) {
        for segment in 0..SEGMENTS {
            let next = (segment + 1) % SEGMENTS;
            let (a, b) = (pair[0][segment], pair[0][next]);
            let (c, d) = (pair[1][segment], pair[1][next]);
            triangles.push([a, c, d]);
            triangles.push([a, d, b]);
        }
    }
    triangles
}

/// The point of the XZ plane `at` is over.
pub(super) fn across(at: Vec3) -> Vec2 {
    Vec2::new(at.x, at.z)
}

/// The least of `a` and `b`, and the greatest, along each axis: compared, as `min` and `max` leave a
/// zero's sign to the platform.
pub(super) fn bounds(a: Vec3, b: Vec3) -> (Vec3, Vec3) {
    let least = |a: f32, b: f32| if b < a { b } else { a };
    let most = |a: f32, b: f32| if b > a { b } else { a };
    (
        Vec3::new(least(a.x, b.x), least(a.y, b.y), least(a.z, b.z)),
        Vec3::new(most(a.x, b.x), most(a.y, b.y), most(a.z, b.z)),
    )
}
