//! Paths over a navmesh: A* over its polygons, from portal to portal, then the funnel algorithm,
//! which pulls the path through the portals taut to the corners where it turns.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use gg_core::math::{Vec2, Vec3};

use super::bake::across;
use super::mesh::NavMesh;

/// No polygon or portal, as a search's links hold it.
const NONE: u32 = u32::MAX;

/// What a path search works in, kept from one to the next, as an agent's are, so that a search
/// allocates nothing once it's sized to its navmesh: each polygon's best cost so far, where the
/// search entered it, the polygon and portal it came through, and the searches it was seen and
/// closed in; the open polygons by their estimated costs; and the path's portals and gates.
#[derive(Default)]
pub(super) struct Search {
    cost: Vec<f32>,
    at: Vec<Vec3>,
    parent: Vec<u32>,
    via: Vec<u32>,
    seen: Vec<u32>,
    closed: Vec<u32>,
    /// The search under way, which `seen` and `closed` are stamped with, so that a search starts
    /// without clearing them.
    stamp: u32,
    open: BinaryHeap<Open>,
    /// The path's portals, start to end, each with the polygon it leads out of.
    chain: Vec<(u32, u32)>,
    gates: Vec<Gate>,
}

/// An open polygon: its estimated cost to the end through it.
#[derive(Clone, Copy, Debug)]
struct Open {
    estimate: f32,
    polygon: u32,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Open {}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Open {
    /// Reversed, so that the heap gives the least estimate first, and of those the least polygon,
    /// whatever order they came in.
    fn cmp(&self, other: &Self) -> Ordering {
        let by_estimate = other.estimate.total_cmp(&self.estimate);
        by_estimate.then(other.polygon.cmp(&self.polygon))
    }
}

/// A gate the funnel passes through, its ends as seen from behind it, left and right, and the
/// polygon past it.
#[derive(Clone, Copy, Debug)]
struct Gate {
    left: Vec2,
    right: Vec2,
    polygon: u32,
}

impl Search {
    /// Sizes the search to `mesh`, for a search to start in it without allocating: once, as it's
    /// first searched, and again after a bake.
    pub(super) fn fit(&mut self, mesh: &NavMesh) {
        let count = mesh.polygons.len();
        if self.cost.len() != count {
            self.cost = vec![0.0; count];
            self.at = vec![Vec3::ZERO; count];
            self.parent = vec![NONE; count];
            self.via = vec![NONE; count];
            self.seen = vec![0; count];
            self.closed = vec![0; count];
            self.stamp = 0;
        }
        let pushes = mesh.portals.len() + 1;
        self.open.reserve(pushes.saturating_sub(self.open.len()));
        self.chain.reserve(count.saturating_sub(self.chain.len()));
        self.gates
            .reserve((count + 2).saturating_sub(self.gates.len()));
    }

    /// Starts a search: a new stamp, the stamps cleared as they come round again.
    fn start(&mut self, mesh: &NavMesh) {
        self.fit(mesh);
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.closed.fill(0);
            self.stamp = 1;
        }
        self.open.clear();
    }
}

/// Twice the signed area of the triangle `a`, `b`, `c`, as the funnel algorithm's `triarea2`:
/// positive where `c` turns clockwise from `b` about `a`, with X across and Z down.
fn triarea2(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    let (ab, ac) = (b - a, c - a);
    ac.x * ab.y - ab.x * ac.y
}

/// Whether `a` and `b` are the same point, within a thousandth.
fn same(a: Vec2, b: Vec2) -> bool {
    a.distance_squared(b) < 1e-6
}

impl NavMesh {
    /// The point of portal `portal` that makes the way from `from` through it to `to` shortest,
    /// across, at the floor of the polygon it leads to there: where the line from `from` to `to`
    /// crosses the portal's, or to `to` mirrored across it where both are on one side, held to the
    /// portal's ends; so that a search's costs follow the taut path, as a portal's middle, Detour's
    /// choice, doesn't across a long rectangle.
    fn entry(&self, portal: u32, from: Vec2, to: Vec2) -> Vec3 {
        let portal = self.portals[portal as usize];
        let (a, b) = (self.corner(portal.a), self.corner(portal.b));
        let along = b - a;
        let side = |point: Vec2| along.perp_dot(point - a);
        let (from_side, to_side) = (side(from), side(to));
        let length = along.length_squared();
        let to = if from_side * to_side > 0.0 && length > 0.0 {
            to - along.perp() * (2.0 * to_side / length)
        } else {
            to
        };
        let (from_side, to_side) = (side(from), side(to));
        let t = if from_side != to_side && length > 0.0 {
            let crossing = from + (to - from) * (from_side / (from_side - to_side));
            (crossing - a).dot(along) / length
        } else {
            0.5
        };
        let at = a + along * t.clamp(0.0, 1.0);
        Vec3::new(at.x, self.floor_in(portal.to, at), at.y)
    }

    /// Writes into `corners`, in place of what it held, the corners of the shortest path over the
    /// mesh from `from` to `to`, as their polygons and portals run, each a point on the mesh, past
    /// `from`, the last `to`: each end on the mesh where it is, or else snapped to the mesh's
    /// nearest point within the agent's height, as [`nearest`](Self::nearest) finds it. Returns
    /// whether a path joins them, `corners` left empty where none does.
    pub(super) fn find_path(
        &self,
        search: &mut Search,
        from: Vec3,
        to: Vec3,
        corners: &mut Vec<Vec3>,
    ) -> bool {
        corners.clear();
        let (Some((start, from)), Some((goal, to))) = (
            self.nearest(from, self.height),
            self.nearest(to, self.height),
        ) else {
            return false;
        };
        if !self.search(search, start, from, goal, to) {
            return false;
        }
        self.funnel(search, start, from, goal, to, corners);
        true
    }

    /// Searches from polygon `start`, at `from`, for polygon `goal`, at `to`, by A*, as Detour's
    /// `findPath` does: a polygon is entered at the point of its portal [`entry`](Self::entry)
    /// finds from where its parent was entered, the cost of the way there from that point, and the
    /// estimate of what's left the way from there to `to`. Leaves the path's portals in the
    /// search's chain, and returns whether one joins them.
    fn search(&self, search: &mut Search, start: u32, from: Vec3, goal: u32, to: Vec3) -> bool {
        search.start(self);
        let stamp = search.stamp;
        let first = start as usize;
        search.seen[first] = stamp;
        search.cost[first] = 0.0;
        search.at[first] = from;
        search.parent[first] = NONE;
        search.via[first] = NONE;
        search.open.push(Open {
            estimate: from.distance(to),
            polygon: start,
        });
        let mut found = false;
        while let Some(Open { polygon, .. }) = search.open.pop() {
            let p = polygon as usize;
            if search.closed[p] == stamp {
                continue;
            }
            search.closed[p] = stamp;
            if polygon == goal {
                found = true;
                break;
            }
            let [first, end] = self.polygons[p].portals;
            for portal in first..end {
                let next = self.portals[portal as usize].to;
                let q = next as usize;
                if search.closed[q] == stamp {
                    continue;
                }
                let entry = self.entry(portal, across(search.at[p]), across(to));
                let mut cost = search.cost[p] + search.at[p].distance(entry);
                let left = if next == goal {
                    cost += entry.distance(to);
                    0.0
                } else {
                    entry.distance(to)
                };
                if search.seen[q] != stamp || cost < search.cost[q] {
                    search.seen[q] = stamp;
                    search.cost[q] = cost;
                    search.at[q] = entry;
                    search.parent[q] = polygon;
                    search.via[q] = portal;
                    search.open.push(Open {
                        estimate: cost + left,
                        polygon: next,
                    });
                }
            }
        }
        search.chain.clear();
        if !found {
            return false;
        }
        let mut at = goal;
        while at != start {
            let (parent, via) = (search.parent[at as usize], search.via[at as usize]);
            search.chain.push((parent, via));
            at = parent;
        }
        search.chain.reverse();
        true
    }

    /// Pulls the path the search's chain of portals makes, from `from` in polygon `start` to `to`
    /// in polygon `goal`, taut, writing its corners after `from` into `corners`: the funnel
    /// algorithm, as Mikko Mononen's "Simple Stupid Funnel Algorithm" (2010), each portal's ends
    /// taken left and right as seen from the middle of the polygon it leads out of. Each corner is
    /// at the floor of the polygon past its portal, and the last is `to`.
    fn funnel(
        &self,
        search: &mut Search,
        start: u32,
        from: Vec3,
        goal: u32,
        to: Vec3,
        corners: &mut Vec<Vec3>,
    ) {
        let gates = &mut search.gates;
        gates.clear();
        let (from_across, to_across) = (across(from), across(to));
        gates.push(Gate {
            left: from_across,
            right: from_across,
            polygon: start,
        });
        for &(out_of, portal) in &search.chain {
            let portal = self.portals[portal as usize];
            let (a, b) = (self.corner(portal.a), self.corner(portal.b));
            let behind = self.middle(out_of);
            let (left, right) = if triarea2(behind, a, b) >= 0.0 {
                (a, b)
            } else {
                (b, a)
            };
            gates.push(Gate {
                left,
                right,
                polygon: portal.to,
            });
        }
        gates.push(Gate {
            left: to_across,
            right: to_across,
            polygon: goal,
        });
        let mut push = |corner: Vec2, polygon: u32| {
            let at = Vec3::new(corner.x, self.floor_in(polygon, corner), corner.y);
            if corners.last() != Some(&at) {
                corners.push(at);
            }
        };
        let mut apex = from_across;
        let (mut left, mut right) = (from_across, from_across);
        let (mut left_at, mut right_at) = (0, 0);
        let mut i = 1;
        while i < gates.len() {
            let gate = gates[i];
            if triarea2(apex, right, gate.right) <= 0.0 {
                if same(apex, right) || triarea2(apex, left, gate.right) > 0.0 {
                    right = gate.right;
                    right_at = i;
                } else {
                    // The right side crossed the left: the left is a corner, which the funnel
                    // starts again from.
                    push(left, gates[left_at].polygon);
                    apex = left;
                    right = left;
                    right_at = left_at;
                    i = left_at + 1;
                    continue;
                }
            }
            if triarea2(apex, left, gate.left) >= 0.0 {
                if same(apex, left) || triarea2(apex, right, gate.left) < 0.0 {
                    left = gate.left;
                    left_at = i;
                } else {
                    push(right, gates[right_at].polygon);
                    apex = right;
                    left = right;
                    left_at = right_at;
                    i = right_at + 1;
                    continue;
                }
            }
            i += 1;
        }
        if corners
            .last()
            .is_some_and(|&last| same(across(last), to_across))
        {
            corners.pop();
        }
        corners.push(to);
    }
}
