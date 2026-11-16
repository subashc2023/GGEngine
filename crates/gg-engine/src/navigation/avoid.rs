//! Avoidance: an agent's velocity chosen among those that keep it clear of each neighbor for a
//! while, by optimal reciprocal collision avoidance (van den Berg, Guy, Lin and Manocha,
//! "Reciprocal n-Body Collision Avoidance", 2011). Each neighbor's velocity obstacle bounds a
//! half-plane of velocities, the agent taking half of what each pair must change. A small linear
//! program finds the velocity nearest the preferred one inside all of them, or, if none is, the
//! one least deep in them.
//!
//! Ported from RVO2's `Agent.cpp` (`computeNewVelocity`, `linearProgram1` to `3`), for agents
//! alone (the navmesh and physics keep them from walls), in plain `f32` arithmetic and square
//! roots, which give the same bits on every platform.

use gg_core::math::Vec2;

use crate::Tick;

/// How far ahead, in seconds, an agent keeps clear of its neighbors. Farther sees them sooner and
/// turns earlier and more widely.
pub(super) const TIME_HORIZON: f32 = 1.5;

/// The most neighbors an agent steers round, the nearest, as RVO2's `maxNeighbors`. It bounds a
/// tick's work per agent in a crowd, and avoidance's lines fit the room they first get.
pub(super) const MAX_NEIGHBORS: usize = 10;

/// How near parallel two lines count as parallel, RVO2's `RVO_EPSILON`.
const EPSILON: f32 = 0.000_01;

/// How far to its right, as a share of it (about a degree), an agent among neighbors nudges its
/// preferred velocity, so two heading straight at each other part instead of stopping face to
/// face (ORCA's half-planes are perfectly symmetric). RVO2's examples perturb at random, which
/// peers wouldn't agree on; one nudge for all keeps agents to the right of each other.
const NUDGE: f32 = 0.02;

/// An agent as avoidance sees it, across. As a neighbor, `side` is 1 or -1: which way along X the
/// agent it's a neighbor of leaves it if they're in one place, by entity ID order, so two agents
/// in one place part, each its own way.
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

/// What avoidance works in, kept between agents and ticks so it allocates nothing once it has
/// room: each neighbor's line, and the lines the third program projects.
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
    /// The velocity `agent` takes (RVO2's `computeNewVelocity`): at most `max_speed`, nearest
    /// `preferred` (nudged to its right among neighbors), among those keeping it clear of each of
    /// `neighbors` for [`TIME_HORIZON`], each taking half of what that needs; if none does, the
    /// least deep in them. Not a number is none.
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
                // Collision: projected on the cut-off circle of a tick, so they part within one.
                // Where nothing tells which way, as agents in one place at rest, ID order does.
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

/// RVO2's `linearProgram1`: the point on `lines[line]`, bounded by the lines before it and the
/// circle of `radius`, nearest `optimum` (farthest along it if `direction`), into `result`.
/// Returns whether there is one.
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
        // Compared, not `min`ed and `max`ed: std leaves their zeros' signs to the platform.
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

/// RVO2's `linearProgram2`: the point within the circle of `radius` on the good side of `lines`,
/// nearest `optimum` (farthest along it if `direction`), into `result`. Returns how many lines it
/// satisfied before one it couldn't; all of them if every one.
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
            // Not on the side that keeps clear: the best on the line instead.
            let before = *result;
            if !program1(lines, i, radius, optimum, direction, result) {
                *result = before;
                return i;
            }
        }
    }
    lines.len()
}

/// RVO2's `linearProgram3`, for when the 2D program found no velocity on every line's side: from
/// line `failed` on, the velocity within the circle of `radius` least deep past any line, into
/// `result`, projecting the lines before each into `projected`.
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
            // Only rounding fails it: the result is within its bounds by what came before.
            *result = before;
        }
        distance = det(line.direction, line.point - *result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two agents heading straight at each other each step aside by half, stay clear while they
    /// pass, and both get by; one alone keeps its preference.
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
}
