//! A model's skins and clips, as glTF gives them: the joints that move a skinned mesh's vertices,
//! the keys that move its parts over time, and the poses they make, which bound where a skinned
//! mesh's vertices reach.

use gg_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};

use crate::Transform;

/// The joints that move a skinned mesh's vertices: model parts, each with an inverse bind matrix
/// (from the mesh's space to the joint's, as the mesh was bound).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Skin {
    /// Each joint's part, by its index among the model's parts.
    pub(crate) joints: Vec<u32>,
    /// Each joint's inverse bind matrix, as many as the joints.
    pub(crate) inverse_binds: Vec<Mat4>,
}

/// What a channel moves of its part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Path {
    Translation,
    Rotation,
    Scale,
}

/// How a channel's values go from key to key, as glTF's samplers say: along a line (a rotation the
/// shorter way at a steady rate), holding each until the next, or along a cubic Hermite spline
/// through each value with its tangents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

/// A model's animation: channels that move its parts' transforms along their keys.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Clip {
    /// The glTF animation's name, or its index as text for one without.
    pub(crate) name: String,
    /// Seconds it plays: until its channels' last key.
    pub(crate) duration: f32,
    pub(crate) channels: Vec<Channel>,
}

/// Keys that move one part's translation, rotation or scale.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Channel {
    /// The part it moves, by its index among the model's parts.
    pub(crate) part: u32,
    pub(crate) path: Path,
    pub(crate) interpolation: Interpolation,
    /// Each key's time in seconds, from 0, ascending.
    pub(crate) times: Vec<f32>,
    /// Each key's value: three floats for a translation or scale, four (X, Y, Z, W; a unit
    /// quaternion) for a rotation. A cubic spline holds, per key, the in-tangent, value and
    /// out-tangent in turn.
    pub(crate) values: Vec<f32>,
}

impl Channel {
    /// Floats in a value.
    pub(crate) fn width(&self) -> usize {
        match self.path {
            Path::Rotation => 4,
            Path::Translation | Path::Scale => 3,
        }
    }

    /// Its value at `time`: the first key's before it, the last's after, and between by its
    /// interpolation. A translation or scale is X, Y, Z; a rotation is a unit quaternion. NaN takes
    /// the first key's.
    pub(crate) fn sample(&self, time: f32) -> Vec4 {
        let keys = self.times.len();
        let width = self.width();
        let cubic = self.interpolation == Interpolation::CubicSpline;
        // Key `k`'s value or in- or out-tangent, `width` floats each.
        let at = |k: usize, part: usize| {
            let start = if cubic {
                (3 * k + part) * width
            } else {
                k * width
            };
            let value = &self.values[start..start + width];
            match *value {
                [x, y, z] => Vec4::new(x, y, z, 0.0),
                [x, y, z, w] => Vec4::new(x, y, z, w),
                _ => unreachable!("a value is three floats or four"),
            }
        };
        let value = |k: usize| at(k, usize::from(cubic));
        // The first key past `time`.
        let next = self.times.partition_point(|&key| key <= time);
        if next == 0 || time.is_nan() {
            return value(0);
        }
        if next == keys {
            return value(keys - 1);
        }
        let (k, span) = (next - 1, self.times[next] - self.times[next - 1]);
        let s = (time - self.times[k]) / span;
        let rotation = self.path == Path::Rotation;
        match self.interpolation {
            Interpolation::Step => value(k),
            Interpolation::Linear if rotation => {
                let [a, b] = [value(k), value(next)].map(Quat::from_vec4);
                Vec4::from(a.slerp(b, s))
            }
            Interpolation::Linear => value(k).lerp(value(next), s),
            Interpolation::CubicSpline => {
                // Hermite's basis, tangents scaled by the keys' span.
                let (s2, s3) = (s * s, s * s * s);
                let point = (2.0 * s3 - 3.0 * s2 + 1.0) * value(k)
                    + (s3 - 2.0 * s2 + s) * span * at(k, 2)
                    + (-2.0 * s3 + 3.0 * s2) * value(next)
                    + (s3 - s2) * span * at(next, 0);
                if rotation {
                    point.normalize_or(Vec4::W)
                } else {
                    point
                }
            }
        }
    }

    /// Sets what it moves of `transform` to `value`, a [`sample`](Self::sample).
    pub(crate) fn apply(&self, transform: &mut Transform, value: Vec4) {
        match self.path {
            Path::Translation => transform.translation = value.truncate().as_dvec3(),
            Path::Rotation => transform.rotation = Quat::from_vec4(value),
            Path::Scale => transform.scale = value.truncate(),
        }
    }
}

impl Clip {
    /// Keys in all its channels.
    pub(crate) fn keys(&self) -> usize {
        self.channels
            .iter()
            .map(|channel| channel.times.len())
            .sum()
    }

    /// The most times [`pose_times`](Self::pose_times) gives.
    pub(crate) fn most_poses(&self) -> usize {
        self.keys().min(MAX_POSES)
    }

    /// Sets `times` to where poses are found to bound a skinned mesh's reach: each key's time
    /// once, or, past [`MAX_POSES`] of them, that many evenly from start to end. `times` briefly
    /// holds all [`keys`](Self::keys)' times, so one with room for them allocates nothing.
    pub(crate) fn pose_times(&self, times: &mut Vec<f32>) {
        times.clear();
        for channel in &self.channels {
            times.extend_from_slice(&channel.times);
        }
        // Unstable: no memory of its own, and equal times are the same.
        times.sort_unstable_by(f32::total_cmp);
        times.dedup();
        if times.len() > MAX_POSES {
            let last = (MAX_POSES - 1) as f32;
            times.clear();
            times.extend((0..MAX_POSES).map(|i| self.duration * i as f32 / last));
        }
    }
}

/// The most poses of a clip that bound where a skinned mesh reaches.
const MAX_POSES: usize = 64;

/// Sets `placed` for the parts `needed`, in order, to the matrix from the part's space to the
/// model's, from `locals` and `parents` (each parent comes before its child). Other parts' entries
/// stay. A part needed has its parent needed too.
pub(crate) fn place(
    locals: &[Transform],
    parents: &[Option<usize>],
    needed: &[usize],
    placed: &mut [Mat4],
) {
    for &part in needed {
        // A model's parts are near its root, where `f32` holds.
        let own = locals[part].matrix_from(DVec3::ZERO);
        placed[part] = parents[part].map_or(own, |parent| placed[parent] * own);
    }
}

/// Bounds a skinned mesh's vertices in any pose without revisiting each vertex.
///
/// The vertices each joint moves, in the joint's space as the mesh was bound, are parted into
/// leaves of at most [`LEAF`] near ones, each held in a box and in a sphere from the box's center.
/// Boxes also hold the vertices no joint moves, and all vertices as they are. The joints' spaces
/// have four dimensions, every row of an inverse bind matrix taken, as the skin pass takes them.
pub(crate) struct Influences {
    /// The leaves, each joint's together, in the skin's joint order.
    leaves: Vec<Leaf>,
    /// Box round the vertices no joint moves, which stay where bound.
    still: [Vec3; 2],
    /// Box round all vertices as they are.
    rest: [Vec3; 2],
}

/// Some of a joint's vertices, held by [`Influences`]: the joint (index in its skin), its box's
/// center and half extents, and its sphere's radius.
#[derive(Clone, Copy)]
struct Leaf {
    joint: u32,
    center: Vec4,
    half: Vec4,
    radius: f32,
}

/// The most vertices of a joint in a leaf: few enough that a box turned with its joint holds
/// little more than they reach (one of a whole limb's wouldn't), enough that a pose costs a few of
/// a mesh's vertices.
const LEAF: usize = 16;

/// A box that holds nothing: growing it by any box gives that box.
const EMPTY: [Vec3; 2] = [Vec3::INFINITY, Vec3::NEG_INFINITY];

/// The box round `a` and `b`.
fn union([low, high]: [Vec3; 2], [l, h]: [Vec3; 2]) -> [Vec3; 2] {
    [low.min(l), high.max(h)]
}

/// Whether a joint moves a vertex by `weight`, as the skin pass has it: a weight that isn't finite
/// or is under 0 counts as 0.
fn moves(weight: f32) -> bool {
    weight.is_finite() && weight > 0.0
}

/// The `joints` that move a vertex by `weights`, as the skin pass has it, in a skin of `count`
/// joints: none if the weights add up to 0 or more than a float holds (the vertex stays where
/// bound); a joint past the skin's counts as its last.
fn moved_by(joints: [u16; 4], weights: [f32; 4], count: usize) -> impl Iterator<Item = usize> {
    let total: f32 = weights.into_iter().filter(|&weight| moves(weight)).sum();
    let moved = total > 0.0 && total.is_finite();
    (joints.into_iter().zip(weights))
        .filter(move |&(_, weight)| moved && moves(weight))
        .map(move |(joint, _)| usize::from(joint).min(count - 1))
}

impl Influences {
    /// How many times a joint of a `count`-joint skin moves a vertex, over all vertices' `joints`
    /// and `weights`: what [`bytes`](Self::bytes) takes.
    pub(crate) fn moved(count: usize, joints: &[[u16; 4]], weights: &[[f32; 4]]) -> usize {
        (joints.iter().zip(weights))
            .map(|(&joints, &weights)| moved_by(joints, weights, count).count())
            .sum()
    }

    /// The most bytes [`new`](Self::new) holds besides its own, for a `count`-joint skin moving
    /// vertices `moved` times: each joint's place of each vertex, and the leaves.
    pub(crate) fn bytes(count: usize, moved: usize) -> usize {
        moved * size_of::<(u32, Vec4)>() + Self::most_leaves(count, moved) * size_of::<Leaf>()
    }

    /// The most leaves a `count`-joint skin moving vertices `moved` times parts them into: a
    /// joint's leaves are each at least half of [`LEAF`], except that a joint's only one may be
    /// smaller.
    pub(crate) fn most_leaves(count: usize, moved: usize) -> usize {
        moved / (LEAF / 2) + count
    }

    /// Bounds `positions` as `skin`'s joints carry them, each vertex moved by `joints` and
    /// `weights` as the skin pass has it: a weight that isn't finite or is under 0 counts as 0, a
    /// vertex whose weights add up to 0 or more than a float holds stays where bound, and a joint
    /// past the skin's counts as its last.
    pub(crate) fn new(
        skin: &Skin,
        positions: &[[f32; 3]],
        joints: &[[u16; 4]],
        weights: &[[f32; 4]],
    ) -> Self {
        let count = skin.joints.len();
        let moved = Self::moved(count, joints, weights);
        let mut bound: Vec<(u32, Vec4)> = Vec::with_capacity(moved);
        let mut still = EMPTY;
        let mut rest = EMPTY;
        for ((position, &joints), &weights) in positions.iter().zip(joints).zip(weights) {
            let position = Vec3::from_array(*position);
            rest = union(rest, [position; 2]);
            let before = bound.len();
            for joint in moved_by(joints, weights, count) {
                let at = skin.inverse_binds[joint] * position.extend(1.0);
                bound.push((joint as u32, at));
            }
            if bound.len() == before {
                still = union(still, [position; 2]);
            }
        }
        // Each joint's together. Unstable, as each sort here is: no memory of its own, and parts
        // the same way every time a model is read.
        bound.sort_unstable_by_key(|&(joint, _)| joint);
        let mut leaves = Vec::with_capacity(Self::most_leaves(count, moved));
        for each in bound.chunk_by_mut(|a, b| a.0 == b.0) {
            leaf(each, &mut leaves);
        }
        Self {
            leaves,
            still,
            rest,
        }
    }

    /// The box (least corner, most corner) round the vertices as `skin`'s joints carry them, with
    /// the model's parts placed by `placed` and the mesh drawn by part `mesh`. It encloses each
    /// leaf's box and sphere as its joint carries them, which hold each vertex wherever its
    /// weights put it between joints. If the mesh part's matrix doesn't invert (scaled to
    /// nothing), the box of the positions as they are.
    pub(crate) fn posed(&self, placed: &[Mat4], mesh: usize, skin: &Skin) -> [Vec3; 2] {
        let into_mesh = placed[mesh].inverse();
        let mut posed = self.still;
        // The last leaf's joint, its matrix, the matrix's absolute rows, and their lengths.
        let mut last: Option<(u32, Mat4, [Vec4; 3], Vec3)> = None;
        for leaf in &self.leaves {
            let (_, matrix, abs, lengths) = match last {
                Some(last) if last.0 == leaf.joint => last,
                _ => {
                    let matrix = into_mesh * placed[skin.joints[leaf.joint as usize] as usize];
                    if !matrix.is_finite() {
                        return self.rest;
                    }
                    // Each mesh-space axis is a matrix row: a box's corners reach as far as the
                    // row's absolute values take its half extents, a sphere as far as the row's
                    // length takes its radius.
                    let rows = [0, 1, 2].map(|row| matrix.row(row));
                    let lengths = Vec3::from_array(rows.map(Vec4::length));
                    *last.insert((leaf.joint, matrix, rows.map(Vec4::abs), lengths))
                }
            };
            let corners = Vec3::from_array(abs.map(|row| row.dot(leaf.half)));
            let reach = corners.min(lengths * leaf.radius);
            let center = (matrix * leaf.center).truncate();
            posed = union(posed, [center - reach, center + reach]);
        }
        posed
    }
}

/// Adds to `leaves` the leaves of `bound`, a joint's vertex places: one if at most [`LEAF`], else
/// those of each half, split along the box's longest axis.
fn leaf(bound: &mut [(u32, Vec4)], leaves: &mut Vec<Leaf>) {
    let (low, high) = (bound.iter()).fold((Vec4::INFINITY, Vec4::NEG_INFINITY), |(l, h), p| {
        (l.min(p.1), h.max(p.1))
    });
    if bound.len() > LEAF {
        let axis = (high - low).max_position();
        bound.sort_unstable_by(|a, b| a.1[axis].total_cmp(&b.1[axis]));
        let (first, second) = bound.split_at_mut(bound.len() / 2);
        leaf(first, leaves);
        leaf(second, leaves);
        return;
    }
    let center = (low + high) / 2.0;
    let radius = (bound.iter()).fold(0.0_f32, |radius, p| radius.max(p.1.distance(center)));
    leaves.push(Leaf {
        joint: bound[0].0,
        center,
        half: (high - low) / 2.0,
        radius,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the skin pass moves `position` by `joints` and `weights`, with `skin`'s joints placed
    /// by `placed` and the mesh by part 0: weights not finite or under 0 count as 0, the rest are
    /// scaled to add up to 1, and a vertex whose weights add up to 0 stays.
    fn skinned(
        skin: &Skin,
        placed: &[Mat4],
        position: [f32; 3],
        joints: [u16; 4],
        weights: [f32; 4],
    ) -> Vec3 {
        let weights = weights.map(|w| if moves(w) { w } else { 0.0 });
        let total: f32 = weights.iter().sum();
        let position = Vec3::from_array(position);
        if !(total > 0.0 && total.is_finite()) {
            return position;
        }
        let into_mesh = placed[0].inverse();
        (joints.iter().zip(weights))
            .map(|(&joint, weight)| {
                let joint = usize::from(joint);
                let matrix =
                    into_mesh * placed[skin.joints[joint] as usize] * skin.inverse_binds[joint];
                (matrix * position.extend(1.0)).truncate() * (weight / total)
            })
            .sum()
    }

    /// Each vertex, whatever its weights (negative, NaN, infinite, summing over 1 or to 0), stays
    /// within the box its leaves bound in each pose of joints turned and scaled unevenly, one bound
    /// by a non-affine matrix. A pose whose mesh matrix doesn't invert gives the box of the
    /// vertices as they are.
    #[test]
    fn a_skinned_meshs_leaves_hold_each_vertex_whatever_its_weights() {
        let mut projective = Mat4::from_rotation_y(0.3);
        projective.x_axis.w = 0.1;
        projective.z_axis.w = -0.05;
        let skin = Skin {
            joints: vec![1, 2],
            inverse_binds: vec![
                Mat4::from_scale(Vec3::splat(2.0)) * Mat4::from_translation(-Vec3::X),
                projective,
            ],
        };
        let kinds = [
            [1.0, 0.0, 0.0, 0.0],
            [-1.0, 2.0, 0.0, 0.0],
            [f32::NAN, 1.0, 0.0, 0.0],
            [3.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [-1.0, -1.0, 0.0, 0.0],
            [f32::INFINITY, 0.5, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
        ];
        // Enough vertices to part each joint's into several leaves.
        let positions: Vec<[f32; 3]> = (0..80)
            .map(|i: u32| {
                let scattered = |by: u32, of: u32| (i * by % of) as f32 / of as f32 * 2.0 - 1.0;
                [scattered(37, 11) * 2.0, i as f32 * 0.05, scattered(53, 7)]
            })
            .collect();
        let weights: Vec<[f32; 4]> = (0..positions.len()).map(|i| kinds[i % 8]).collect();
        let joints = vec![[0, 1, 0, 0]; positions.len()];
        let influences = Influences::new(&skin, &positions, &joints, &weights);
        assert!(
            influences.leaves.len() > 2,
            "{} leaves",
            influences.leaves.len()
        );
        for turn in 0..8 {
            let angle = turn as f32 * 0.8;
            let placed = [
                Mat4::from_rotation_x(angle * 0.5) * Mat4::from_translation(Vec3::Z),
                Mat4::from_scale_rotation_translation(
                    Vec3::new(0.5, 2.0, 1.0),
                    Quat::from_rotation_z(angle),
                    Vec3::new(1.0, 0.0, -1.0),
                ),
                Mat4::from_scale_rotation_translation(
                    Vec3::new(3.0, 1.0, 0.25),
                    Quat::from_rotation_x(angle) * Quat::from_rotation_y(0.4),
                    Vec3::Y,
                ),
            ];
            let [low, high] = influences.posed(&placed, 0, &skin);
            let within = 1e-4 * (high - low).max_element();
            for ((&position, &joints), &weights) in positions.iter().zip(&joints).zip(&weights) {
                let moved = skinned(&skin, &placed, position, joints, weights);
                assert!(
                    moved.cmpge(low - within).all() && moved.cmple(high + within).all(),
                    "pose {turn}: {moved} of weights {weights:?} in {low}..{high}"
                );
            }
        }
        let rest = (positions.iter()).fold(EMPTY, |rest, &p| union(rest, [Vec3::from_array(p); 2]));
        let flat = [
            Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)),
            Mat4::IDENTITY,
            Mat4::IDENTITY,
        ];
        assert_eq!(influences.posed(&flat, 0, &skin), rest);
    }
}
