//! The navmesh a bake makes: polygons (rectangles of a heightfield's walkable cells), the portals
//! where they meet, and each column's cells, by which points are found on it.

use gg_core::math::{Vec2, Vec3};

/// A navmesh: the walkable cells of a heightfield, each a square `cell_size` across with its floor
/// at a multiple of `cell_height` above the heightfield's lowest corner, joined into rectangles:
/// polygons. They're convex, so an agent crosses one in a straight line, and their edges shared
/// with neighbors' are portals. A column holds a cell for each floor an agent stands on there, as
/// under a platform and on it. A bake makes it whole and it never changes, so copies of the world
/// share it.
#[derive(Debug, PartialEq)]
pub(super) struct NavMesh {
    /// The heightfield's lowest corner, in the world's space.
    pub(super) min: Vec3,
    pub(super) cell_size: f32,
    pub(super) cell_height: f32,
    /// Columns along X and along Z.
    pub(super) width: u32,
    pub(super) depth: u32,
    /// The agent's headroom and climb, which a point stands on a floor within
    /// ([`nearest`](Self::nearest)).
    pub(super) height: f32,
    pub(super) climb: f32,
    pub(super) polygons: Vec<Polygon>,
    /// Each polygon's portals, in its range of them.
    pub(super) portals: Vec<Portal>,
    /// Where each column's cells start in `cells`, columns by Z then X, plus where the last ends.
    pub(super) columns: Vec<u32>,
    /// Each column's cells, the lowest first.
    pub(super) cells: Vec<Cell>,
}

/// A polygon of a [`NavMesh`]: a rectangle of columns, `width` along X from `x` and `depth` along
/// Z from `z`, each holding one of its cells, and the range of its portals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Polygon {
    pub(super) x: u32,
    pub(super) z: u32,
    pub(super) width: u32,
    pub(super) depth: u32,
    pub(super) portals: [u32; 2],
}

/// Where a polygon meets another, `to`: a run of its edge between grid corners `a` and `b`, each
/// in cells from the lowest, along X and Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Portal {
    pub(super) to: u32,
    pub(super) a: [u32; 2],
    pub(super) b: [u32; 2],
}

/// A walkable cell: its floor, in cell heights, and its polygon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cell {
    pub(super) floor: u16,
    pub(super) polygon: u32,
}

impl NavMesh {
    /// Grid corner `corner`'s X and Z in the world.
    pub(super) fn corner(&self, [x, z]: [u32; 2]) -> Vec2 {
        let size = self.cell_size;
        Vec2::new(self.min.x + x as f32 * size, self.min.z + z as f32 * size)
    }

    /// The world height of floor `floor`.
    pub(super) fn height_of(&self, floor: u16) -> f32 {
        self.min.y + f32::from(floor) * self.cell_height
    }

    pub(super) fn column(&self, x: u32, z: u32) -> &[Cell] {
        let at = (z * self.width + x) as usize;
        let range = self.columns[at] as usize..self.columns[at + 1] as usize;
        &self.cells[range]
    }

    /// The column `across` falls in, if any.
    fn column_at(&self, across: Vec2) -> Option<[u32; 2]> {
        let x = ((across.x - self.min.x) / self.cell_size).floor();
        let z = ((across.y - self.min.z) / self.cell_size).floor();
        let inside = |at: f32, of: u32| at >= 0.0 && at < of as f32;
        (inside(x, self.width) && inside(z, self.depth)).then_some([x as u32, z as u32])
    }

    /// The middle of polygon `polygon`, across.
    pub(super) fn middle(&self, polygon: u32) -> Vec2 {
        let Polygon {
            x, z, width, depth, ..
        } = self.polygons[polygon as usize];
        let size = self.cell_size;
        let half = |from: u32, cells: u32| from as f32 * size + cells as f32 * size * 0.5;
        Vec2::new(self.min.x + half(x, width), self.min.z + half(z, depth))
    }

    /// How high polygon `polygon`'s floor is at `across`, by its nearest cell (the cell there, for
    /// a point inside it).
    pub(super) fn floor_in(&self, polygon: u32, across: Vec2) -> f32 {
        let rect = self.polygons[polygon as usize];
        let cell = |at: f32, min: f32, from: u32, cells: u32| {
            let at = ((at - min) / self.cell_size).floor();
            // Compared: a point outside it, or not a number, takes its nearest edge.
            let first = from as f32;
            let last = (from + cells - 1) as f32;
            let at = if at > first { at } else { first };
            (if at < last { at } else { last }) as u32
        };
        let x = cell(across.x, self.min.x, rect.x, rect.width);
        let z = cell(across.y, self.min.z, rect.z, rect.depth);
        let cells = self.column(x, z).iter();
        let floor = cells
            .filter(|cell| cell.polygon == polygon)
            .map(|cell| cell.floor);
        self.height_of(
            floor
                .min()
                .expect("a polygon holds a cell in each of its columns"),
        )
    }

    /// How far `point` is from standing on a floor at `floor`: 0 from the floor up to the agent's
    /// height above it, else the way to that span.
    fn off_floor(&self, point: Vec3, floor: f32) -> f32 {
        if point.y < floor {
            floor - point.y
        } else if point.y > floor + self.height {
            point.y - floor - self.height
        } else {
            0.0
        }
    }

    /// The point of the mesh nearest `point`, within `reach`, and its polygon: on the nearest cell,
    /// at its floor, nearest across its square. A point counts as on a floor from the floor up to
    /// the agent's height above it, as an agent's middle is, so one over a cell within that height
    /// is on the mesh where it is. Nothing for a point or reach that isn't a number, or with no
    /// cell within reach. Ties go to the first cell, by Z, then X, then floor.
    pub(super) fn nearest(&self, point: Vec3, reach: f32) -> Option<(u32, Vec3)> {
        if !point.is_finite() || !(reach >= 0.0 && reach.is_finite()) {
            return None;
        }
        let size = self.cell_size;
        let range = |at: f32, min: f32, cells: u32| {
            let first = ((at - reach - min) / size).floor();
            let last = ((at + reach - min) / size).floor();
            let first = if first > 0.0 { first } else { 0.0 };
            let end = cells as f32 - 1.0;
            let last = if last < end { last } else { end };
            (first <= last).then_some((first as u32, last as u32))
        };
        let (x0, x1) = range(point.x, self.min.x, self.width)?;
        let (z0, z1) = range(point.z, self.min.z, self.depth)?;
        let mut best: Option<(f32, u32, Vec3)> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                let low = self.corner([x, z]);
                let near_x = point.x.clamp(low.x, low.x + size);
                let near_z = point.z.clamp(low.y, low.y + size);
                let (dx, dz) = (point.x - near_x, point.z - near_z);
                for cell in self.column(x, z) {
                    let floor = self.height_of(cell.floor);
                    let dy = self.off_floor(point, floor);
                    let distance = dx * dx + dy * dy + dz * dz;
                    if distance <= reach * reach && best.is_none_or(|(least, ..)| distance < least)
                    {
                        let at = Vec3::new(near_x, floor, near_z);
                        best = Some((distance, cell.polygon, at));
                    }
                }
            }
        }
        best.map(|(_, polygon, at)| (polygon, at))
    }

    /// The columns `across` counts as over, in turn: its own, then each whose edge it's within a
    /// thousandth of a cell of. A path along the navmesh's edge runs along an edge between cells,
    /// and one walking it strays over by rounding.
    fn columns_near(&self, across: Vec2) -> impl Iterator<Item = [u32; 2]> + '_ {
        let slack = self.cell_size * 1e-3;
        let nudges = [
            Vec2::ZERO,
            Vec2::new(slack, 0.0),
            Vec2::new(-slack, 0.0),
            Vec2::new(0.0, slack),
            Vec2::new(0.0, -slack),
        ];
        nudges
            .into_iter()
            .filter_map(move |nudge| self.column_at(across + nudge))
    }

    /// The floor `point` stands on ([`nearest`](Self::nearest)'s sense of standing), in the first
    /// of its [`columns_near`](Self::columns_near) that holds one: the nearest.
    pub(super) fn floor_under(&self, point: Vec3) -> Option<f32> {
        self.columns_near(Vec2::new(point.x, point.z))
            .find_map(|[x, z]| {
                let floors = self
                    .column(x, z)
                    .iter()
                    .map(|cell| self.height_of(cell.floor));
                let standing = floors.filter(|&floor| self.off_floor(point, floor) <= self.climb);
                standing.reduce(|a, b| {
                    if self.off_floor(point, b) < self.off_floor(point, a) {
                        b
                    } else {
                        a
                    }
                })
            })
    }

    /// The floor at `across` that one at height `from` steps onto, in the first of its
    /// [`columns_near`](Self::columns_near) that has one.
    pub(super) fn floor_near(&self, across: Vec2, from: f32) -> Option<f32> {
        self.columns_near(across)
            .find_map(|[x, z]| self.step_onto(x, z, from))
    }

    /// The nearest floor of column (`x`, `z`) within the agent's climb of height `from`; none
    /// outside the heightfield.
    fn step_onto(&self, x: u32, z: u32, from: f32) -> Option<f32> {
        if x >= self.width || z >= self.depth {
            return None;
        }
        let floors = self
            .column(x, z)
            .iter()
            .map(|cell| self.height_of(cell.floor));
        floors
            .filter(|floor| (floor - from).abs() <= self.climb)
            .reduce(|a, b| {
                if (b - from).abs() < (a - from).abs() {
                    b
                } else {
                    a
                }
            })
    }

    /// Whether `point` stands on a floor at height `floor` ([`nearest`](Self::nearest)'s sense of
    /// standing, within the agent's climb): an agent does on its corner's floor, not on another
    /// floor over or under it.
    pub(super) fn stands(&self, point: Vec3, floor: f32) -> bool {
        self.off_floor(point, floor) <= self.climb
    }

    /// Whether the way across from `from`, standing on the floor at height `floor`, straight to
    /// `to` stays on the mesh and comes to `to`'s floor. Each column it crosses, in Amanatides and
    /// Woo's grid traversal order ("A Fast Voxel Traversal Algorithm for Ray Tracing", 1987), must
    /// hold a floor within the agent's climb of the last. Both columns beside a corner it passes
    /// through count, so it cuts no corner of what isn't walkable. It must end on a floor within
    /// the climb of `to`'s height, so a corner on a floor over or under isn't in sight. It stops a
    /// thousandth short of `to`, so a path's corner on a cell's edge counts as in the cell it's
    /// come to from. Erosion keeps such a way the agent's radius from walls and edges.
    pub(super) fn in_sight(&self, from: Vec2, floor: f32, to: Vec3) -> bool {
        let goal = to.y;
        let to = Vec2::new(to.x, to.z);
        let to = to - (to - from) * 1e-3;
        let (Some([mut x, mut z]), Some(end)) = (self.column_at(from), self.column_at(to)) else {
            return false;
        };
        let way = to - from;
        let size = self.cell_size;
        // Per axis: the step to the next column, the share of the way to its first edge, and
        // the share between edges.
        let axis = |at: f32, min: f32, cell: u32, way: f32| {
            if way > 0.0 {
                let edge = min + (cell + 1) as f32 * size;
                (1i64, (edge - at) / way, size / way)
            } else if way < 0.0 {
                let edge = min + cell as f32 * size;
                (-1, (edge - at) / way, -size / way)
            } else {
                (0, f32::INFINITY, f32::INFINITY)
            }
        };
        let (step_x, mut next_x, delta_x) = axis(from.x, self.min.x, x, way.x);
        let (step_z, mut next_z, delta_z) = axis(from.y, self.min.z, z, way.y);
        let moved = |at: u32, step: i64| (i64::from(at) + step) as u32;
        let mut floor = floor;
        // Each step crosses an edge; the way crosses no more than the columns between its ends.
        let steps = i64::from(end[0].abs_diff(x)) + i64::from(end[1].abs_diff(z));
        for _ in 0..=steps * 2 {
            if [x, z] == end {
                return (floor - goal).abs() <= self.climb;
            }
            if next_x == next_z {
                // Through a corner: the columns on both sides of it, then the one across.
                let sides = [(moved(x, step_x), z), (x, moved(z, step_z))];
                if sides
                    .iter()
                    .any(|&(x, z)| self.step_onto(x, z, floor).is_none())
                {
                    return false;
                }
                (x, z) = (moved(x, step_x), moved(z, step_z));
                next_x += delta_x;
                next_z += delta_z;
            } else if next_x < next_z {
                x = moved(x, step_x);
                next_x += delta_x;
            } else {
                z = moved(z, step_z);
                next_z += delta_z;
            }
            match self.step_onto(x, z, floor) {
                Some(onto) => floor = onto,
                None => return false,
            }
        }
        false
    }
}
