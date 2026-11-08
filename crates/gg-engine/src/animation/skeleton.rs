//! A model's skins and clips, as glTF's skins and animations give them: the joints that move a
//! skinned mesh's vertices, and the keys that move a model's parts over time; and the poses they
//! make, which bound where a skinned mesh's vertices reach.

use gg_core::math::{Mat4, Quat, Vec3, Vec4};

use crate::Transform;

/// The joints that move a skinned mesh's vertices: each one of the model's parts, and its inverse
/// bind matrix, from the mesh's space to the joint's as the mesh was bound to it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Skin {
    /// Each joint's part, by its index among the model's parts.
    pub(crate) joints: Vec<u32>,
    /// Each joint's inverse bind matrix, as many as the joints.
    pub(crate) inverse_binds: Vec<Mat4>,
}

/// What a channel moves of its part: its translation, its rotation or its scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Path {
    Translation,
    Rotation,
    Scale,
}

/// How a channel's values go from each key to the next, as glTF's samplers say: along a line, a
/// rotation the shorter way round at a steady rate; holding each until the next; or along a cubic
/// Hermite spline, through each key's value with its tangents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

/// A model's animation: channels that move its parts' transforms over time, each along its keys.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Clip {
    /// Its name, as glTF's animation has it, or its index, as text, for one without.
    pub(crate) name: String,
    /// How long it plays, in seconds: until its channels' last key.
    pub(crate) duration: f32,
    pub(crate) channels: Vec<Channel>,
}

/// Keys that move one of a model's parts' translation, rotation or scale.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Channel {
    /// The part it moves, by its index among the model's parts.
    pub(crate) part: u32,
    pub(crate) path: Path,
    pub(crate) interpolation: Interpolation,
    /// Each key's time, in seconds, from 0 up, each after the last.
    pub(crate) times: Vec<f32>,
    /// Each key's value, three floats for a translation or a scale, four for a rotation, X, Y, Z
    /// then W, a unit quaternion; along a cubic spline, each key's tangent coming in, its value and
    /// its tangent going out, in turn.
    pub(crate) values: Vec<f32>,
}

impl Channel {
    /// How many floats a value of it is.
    pub(crate) fn width(&self) -> usize {
        match self.path {
            Path::Rotation => 4,
            Path::Translation | Path::Scale => 3,
        }
    }

    /// Its value at `time`: its first key's before it, its last's after it, and between, as its
    /// interpolation goes; a translation or a scale in X, Y and Z, or a rotation, unit. A time that
    /// isn't a number takes its first key's.
    pub(crate) fn sample(&self, time: f32) -> Vec4 {
        let keys = self.times.len();
        let width = self.width();
        let cubic = self.interpolation == Interpolation::CubicSpline;
        // Key `k`'s value, its tangent coming in, or going out, each `width` floats.
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
        // The first key past `time`, as the times ascend.
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
                // Hermite's basis, the tangents scaled by the span between the keys.
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

    /// Sets what it moves of `transform` to `value`, as [`sample`](Self::sample) gives it.
    pub(crate) fn apply(&self, transform: &mut Transform, value: Vec4) {
        match self.path {
            Path::Translation => transform.translation = value.truncate(),
            Path::Rotation => transform.rotation = Quat::from_vec4(value),
            Path::Scale => transform.scale = value.truncate(),
        }
    }
}

impl Clip {
    /// The times its poses are found at to bound where a skinned mesh reaches: each of its keys'
    /// times, once, or, of a clip of more than [`MAX_POSES`] of them, that many from its start to
    /// its end, evenly.
    pub(crate) fn pose_times(&self) -> Vec<f32> {
        let mut times: Vec<f32> = self.channels.iter().flat_map(|c| c.times.clone()).collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        if times.len() > MAX_POSES {
            let last = (MAX_POSES - 1) as f32;
            times = (0..MAX_POSES)
                .map(|i| self.duration * i as f32 / last)
                .collect();
        }
        times
    }
}

/// The most poses of a clip that bound where a skinned mesh reaches.
const MAX_POSES: usize = 64;

/// The matrix from each of a model's parts' space to the model's, given each part's transform,
/// `locals`, and its parent, `parents`, which comes before it.
pub(crate) fn placed(locals: &[Transform], parents: &[Option<usize>]) -> Vec<Mat4> {
    let mut placed: Vec<Mat4> = Vec::with_capacity(locals.len());
    for (local, parent) in locals.iter().zip(parents) {
        let own = local.matrix();
        placed.push(parent.map_or(own, |parent| placed[parent] * own));
    }
    placed
}

/// The box, its least corner and its most, round `positions` as `skin`'s joints carry them, posed
/// as `placed` places the model's parts, the mesh drawn by part `mesh`, its vertices moved by
/// `joints` as much as `weights` say. A part whose matrix doesn't invert, as one scaled to
/// nothing, gives the box of the positions as they are.
pub(crate) fn skinned_box(
    placed: &[Mat4],
    mesh: usize,
    skin: &Skin,
    positions: &[[f32; 3]],
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
) -> [Vec3; 2] {
    let into_mesh = placed[mesh].inverse();
    let matrices: Vec<Mat4> = (skin.joints.iter().zip(&skin.inverse_binds))
        .map(|(&joint, &bind)| into_mesh * placed[joint as usize] * bind)
        .collect();
    let mut low = Vec3::INFINITY;
    let mut high = Vec3::NEG_INFINITY;
    let finite = matrices.iter().all(Mat4::is_finite);
    for ((position, joints), weights) in positions.iter().zip(joints).zip(weights) {
        let position = Vec3::from_array(*position);
        let total: f32 = weights.iter().sum();
        let moved = if finite && total > 0.0 {
            (joints.iter().zip(weights))
                .map(|(&joint, &weight)| {
                    let joint = (joint as usize).min(matrices.len() - 1);
                    matrices[joint].transform_point3(position) * (weight / total)
                })
                .sum()
        } else {
            position
        };
        low = low.min(moved);
        high = high.max(moved);
    }
    [low, high]
}
