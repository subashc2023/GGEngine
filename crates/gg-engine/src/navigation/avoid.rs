//! Avoidance: an agent's velocity chosen among those that keep it clear of each neighbor for a
//! while, by optimal reciprocal collision avoidance (van den Berg, Guy, Lin and Manocha,
//! "Reciprocal n-Body Collision Avoidance", 2011): each neighbor's velocity obstacle bounds a
//! half-plane of the velocities, the agent taking half of what each pair must change, and a small
//! linear program finds the velocity nearest the one it prefers in all of them, or, where none is,
//! the one least deep in them. Ported from RVO2's `Agent.cpp` (`computeNewVelocity` and
//! `linearProgram1` to `3`), agents alone, as the navmesh and physics keep them from walls, in the
//! plain arithmetic and square roots of `f32`, which give the same bits on every platform.

use gg_core::math::Vec2;

use crate::Tick;

/// How far ahead, in seconds, an agent keeps clear of its neighbors: farther sees them sooner, and
/// turns earlier and more widely.
pub(super) const TIME_HORIZON: f32 = 1.5;

/// The most neighbors an agent steers round, the nearest, as RVO2's `maxNeighbors`: so that what a
/// tick does for each agent in a crowd stays bounded, and avoidance's lines fit the room they're
/// given as they're first drawn.
pub(super) const MAX_NEIGHBORS: usize = 10;

/// How near parallel two lines are taken as parallel, RVO2's `RVO_EPSILON`.
const EPSILON: f32 = 0.000_01;

/// How far to its right an agent among neighbors nudges the velocity it prefers, as a share of it,
/// about a degree: so that two heading straight at each other part rather than stopping face to
/// face, where ORCA's half-planes, perfectly symmetric, leave them no way round. RVO2's examples
/// perturb the preferred velocities at random, which peers wouldn't agree on; one nudge for every
/// agent keeps them to the right of each other, as traffic keeps.
const NUDGE: f32 = 0.02;

/// An agent as avoidance sees it, across: where it is, how fast it goes, how far round it it keeps
/// others, and, as a neighbor, which way along X the agent it's a neighbor of leaves it where
/// they're in one place, 1 or -1, as the order of their entities' IDs gives, so that two agents in
/// one place part, each its own way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Disc {
    pub(super) position: Vec2,
    pub(super) velocity: Vec2,
    pub(super) radius: f32,
    pub(super) side: f32,
}

/// A directed line of the velocities' plane: the velocities on its left keep clear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Line {
    point: Vec2,
    direction: Vec2,
}

/// What avoidance works in, kept from one agent and tick to the next, so that it allocates nothing
/// once it has room: each neighbor's line, and the lines the third program projects.
#[derive(Default)]
pub(super) struct Lines {
    lines: Vec<Line>,
    projected: Vec<Line>,
}

/// `a` crossed with `b`, RVO2's `det`.
fn det(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

impl Lines {
    /// The velocity `agent` takes, no faster than `max_speed`, nearest `preferred`, nudged to its
    /// right among neighbors, of those that keep it clear of each of `neighbors` for
    /// [`TIME_HORIZON`], each taking half of what keeping clear needs, as RVO2's
    /// `computeNewVelocity`; or, where none does, the one that comes least deep among them. One that
    /// isn't a number is none.
    pub(super) fn velocity(
        &mut self,
        agent: Disc,
        neighbors: impl Iterator<Item = Disc>,
        preferred: Vec2,
        max_speed: f32,
    ) -> Vec2 {
        let inverse_horizon = 1.0 / TIME_HORIZON;
        let inverse_step = 1.0 / Tick::DT.as_secs_f32();
        self.lines.clear();
        self.lines.reserve(MAX_NEIGHBORS);
        self.projected
            .reserve(MAX_NEIGHBORS.saturating_sub(self.projected.len()));
        for other in neighbors {
            let relative_position = other.position - agent.position;
            let relative_velocity = agent.velocity - other.velocity;
            let distance_squared = relative_position.length_squared();
            let combined_radius = agent.radius + other.radius;
            let combined_squared = combined_radius * combined_radius;
            let (direction, u);
            if distance_squared > combined_squared {
                // No collision: the velocity obstacle's cut-off circle or one of its legs.
                let w = relative_velocity - relative_position * inverse_horizon;
                let w_squared = w.length_squared();
                let dot = w.dot(relative_position);
                if dot < 0.0 && dot * dot > combined_squared * w_squared {
                    // Projected on the cut-off circle.
                    let w_length = w_squared.sqrt();
                    let unit = w / w_length;
                    direction = Vec2::new(unit.y, -unit.x);
                    u = unit * (combined_radius * inverse_horizon - w_length);
                } else {
                    // Projected on a leg.
                    let leg = (distance_squared - combined_squared).sqrt();
                    let p = relative_position;
                    direction = if det(p, w) > 0.0 {
                        Vec2::new(
                            p.x * leg - p.y * combined_radius,
                            p.x * combined_radius + p.y * leg,
                        ) / distance_squared
                    } else {
                        -Vec2::new(
                            p.x * leg + p.y * combined_radius,
                            -p.x * combined_radius + p.y * leg,
                        ) / distance_squared
                    };
                    u = direction * relative_velocity.dot(direction) - relative_velocity;
                }
            } else {
                // Collision: projected on the cut-off circle of a tick, so that they part within
                // one. Where nothing tells which way, as agents in one place at rest, the way
                // their order gives.
                let w = relative_velocity - relative_position * inverse_step;
                let w_length = w.length();
                let unit = if w_length > 0.0 {
                    w / w_length
                } else {
                    Vec2::new(other.side, 0.0)
                };
                direction = Vec2::new(unit.y, -unit.x);
                u = unit * (combined_radius * inverse_step - w_length);
            }
            self.lines.push(Line {
                point: agent.velocity + u * 0.5,
                direction,
            });
        }
        // Its right, across: its way crossed with up.
        let optimum = if self.lines.is_empty() {
            preferred
        } else {
            preferred + Vec2::new(-preferred.y, preferred.x) * NUDGE
        };
        let mut result = Vec2::ZERO;
        let failed = program2(&self.lines, max_speed, optimum, false, &mut result);
        if failed < self.lines.len() {
            program3(
                &self.lines,
                &mut self.projected,
                failed,
                max_speed,
                &mut result,
            );
        }
        if result.is_finite() {
            result
        } else {
            Vec2::ZERO
        }
    }
}

/// Solves the one-dimensional linear program on line `line` of `lines`, bounded by the lines before
/// it and the circle of `radius`, as RVO2's `linearProgram1`: the point of it nearest `optimum`, or
/// farthest along `optimum` where `direction` says, into `result`. Returns whether it has one.
fn program1(
    lines: &[Line],
    line: usize,
    radius: f32,
    optimum: Vec2,
    direction: bool,
    result: &mut Vec2,
) -> bool {
    let this = lines[line];
    let dot = this.point.dot(this.direction);
    let discriminant = dot * dot + radius * radius - this.point.length_squared();
    if discriminant < 0.0 {
        // The circle leaves out all of the line.
        return false;
    }
    let root = discriminant.sqrt();
    let (mut t_left, mut t_right) = (-dot - root, -dot + root);
    for other in &lines[..line] {
        let denominator = det(this.direction, other.direction);
        let numerator = det(other.direction, this.point - other.point);
        if denominator.abs() <= EPSILON {
            // Near parallel.
            if numerator < 0.0 {
                return false;
            }
            continue;
        }
        let t = numerator / denominator;
        // Compared rather than `min`ed and `max`ed, whose zeros' signs std leaves to the platform.
        if denominator >= 0.0 {
            if t < t_right {
                t_right = t;
            }
        } else if t > t_left {
            t_left = t;
        }
        if t_left > t_right {
            return false;
        }
    }
    let at = |t: f32| this.point + this.direction * t;
    *result = if direction {
        if optimum.dot(this.direction) > 0.0 {
            at(t_right)
        } else {
            at(t_left)
        }
    } else {
        let t = this.direction.dot(optimum - this.point);
        if t < t_left {
            at(t_left)
        } else if t > t_right {
            at(t_right)
        } else {
            at(t)
        }
    };
    true
}

/// Solves the two-dimensional linear program of `lines` within the circle of `radius`, as RVO2's
/// `linearProgram2`: the point nearest `optimum`, or farthest along it where `direction` says, into
/// `result`. Returns the number of lines it satisfied before one it couldn't, all of them where it
/// satisfied every one.
fn program2(
    lines: &[Line],
    radius: f32,
    optimum: Vec2,
    direction: bool,
    result: &mut Vec2,
) -> usize {
    *result = if direction {
        optimum * radius
    } else if optimum.length_squared() > radius * radius {
        optimum.normalize() * radius
    } else {
        optimum
    };
    for (i, line) in lines.iter().enumerate() {
        if det(line.direction, line.point - *result) > 0.0 {
            // It isn't on the line's side that keeps clear: the best on the line instead.
            let before = *result;
            if !program1(lines, i, radius, optimum, direction, result) {
                *result = before;
                return i;
            }
        }
    }
    lines.len()
}

/// Solves the three-dimensional linear program of `lines` from line `failed` on, as RVO2's
/// `linearProgram3`, where the two-dimensional one found no velocity on every line's side: the
/// velocity least deep past any of them, within the circle of `radius`, into `result`, projecting
/// the lines before each into `projected`.
fn program3(
    lines: &[Line],
    projected: &mut Vec<Line>,
    failed: usize,
    radius: f32,
    result: &mut Vec2,
) {
    let mut distance = 0.0;
    for i in failed..lines.len() {
        let line = lines[i];
        if det(line.direction, line.point - *result) <= distance {
            continue;
        }
        projected.clear();
        for other in &lines[..i] {
            let determinant = det(line.direction, other.direction);
            let point = if determinant.abs() <= EPSILON {
                if line.direction.dot(other.direction) > 0.0 {
                    // Parallel, the same way.
                    continue;
                }
                // Parallel, opposite ways.
                (line.point + other.point) * 0.5
            } else {
                let along = det(other.direction, line.point - other.point) / determinant;
                line.point + line.direction * along
            };
            projected.push(Line {
                point,
                direction: (other.direction - line.direction).normalize(),
            });
        }
        let before = *result;
        let toward = Vec2::new(-line.direction.y, line.direction.x);
        if program2(projected, radius, toward, true, result) < projected.len() {
            // Only rounding fails it, as the result is within its bounds by what came before.
            *result = before;
        }
        distance = det(line.direction, line.point - *result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two agents heading straight at each other, each preferring its way, step aside each by half,
    /// keeping clear for the whole time they pass, and both get by; one alone keeps its preference.
    #[test]
    fn two_agents_head_on_pass_each_other_clear() {
        let dt = Tick::DT.as_secs_f32();
        let mut lines = Lines::default();
        let (speed, radius) = (2.0, 0.5);
        let mut agents = [
            (Vec2::new(-4.0, 0.0), Vec2::ZERO, Vec2::X * speed, 1.0),
            (Vec2::new(4.0, 0.0), Vec2::ZERO, Vec2::NEG_X * speed, -1.0),
        ];
        let disc = |(position, velocity, _, side): (Vec2, Vec2, Vec2, f32)| Disc {
            position,
            velocity,
            radius,
            side,
        };
        let alone = lines.velocity(disc(agents[0]), std::iter::empty(), agents[0].2, speed);
        assert_eq!(alone, agents[0].2);
        let mut nearest = f32::INFINITY;
        for _ in 0..512 {
            let chosen: Vec<Vec2> = (0..2)
                .map(|i| {
                    let other = disc(agents[1 - i]);
                    let preferred = agents[i].2;
                    lines.velocity(disc(agents[i]), std::iter::once(other), preferred, speed)
                })
                .collect();
            for (agent, velocity) in agents.iter_mut().zip(chosen) {
                agent.1 = velocity;
                agent.0 += velocity * dt;
                assert!(velocity.length() <= speed + 1e-4, "{velocity}");
            }
            nearest = nearest.min(agents[0].0.distance(agents[1].0));
        }
        assert!(nearest >= 2.0 * radius * 0.98, "came within {nearest}");
        assert!(agents[0].0.x > 4.0 && agents[1].0.x < -4.0, "{agents:?}");
    }

    /// Agents in one place at rest part, each the way its neighbor's side says, rather than
    /// staying there: the agent's own side, as the agents' step leaves it, counts for nothing.
    #[test]
    fn agents_in_one_place_part() {
        let mut lines = Lines::default();
        let disc = |side| Disc {
            position: Vec2::ZERO,
            velocity: Vec2::ZERO,
            radius: 0.5,
            side,
        };
        let a = lines.velocity(disc(0.0), std::iter::once(disc(1.0)), Vec2::ZERO, 3.0);
        let b = lines.velocity(disc(0.0), std::iter::once(disc(-1.0)), Vec2::ZERO, 3.0);
        assert!(a.x > 0.0 && b.x < 0.0, "apart: {a} {b}");
    }
}
