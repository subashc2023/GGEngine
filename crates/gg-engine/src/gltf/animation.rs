//! A model's skins and animations, and the reach of what its skins move.

use std::io;
use std::path::Path;

use gg_core::Result;
use gg_core::math::{Mat4, Quat, Vec3};

use super::scene::unit;
use super::{Imported, Reader};
use crate::Transform;
use crate::animation::{Channel, Clip, Influences, Interpolation, Path as Moved, Skin, place};

/// The most steps finding a model's skinned meshes' reach may take, each a part placed, a leaf of
/// [`Influences`] posed or a channel set, for one of its clips' poses: some seconds in a debug
/// build, past what a model of fifty clips of 64 poses, two hundred parts and 50,000 skinned
/// vertices takes, so that a small file of many clips and many skinned parts can't take minutes to
/// read.
const MAX_POSE_WORK: u64 = 1 << 27;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Skin `index`: its joints' parts, each node of them in the scene, and their inverse bind
    /// matrices, identities without them.
    pub(super) fn skin(&mut self, index: u32) -> Result<Skin> {
        let (model, gltf) = (self.model, self.gltf);
        let skin = model.item(&gltf.skins, "skin", index)?;
        if skin.joints.is_empty() || skin.joints.len() > usize::from(u16::MAX) + 1 {
            return Err(model.bad(format!(
                "has skin {index} of {} joints, where glTF's skins have from 1 to 65,536",
                skin.joints.len()
            )));
        }
        self.memory
            .hold(skin.joints.len() * (size_of::<u32>() + size_of::<Mat4>()))?;
        let joints = (skin.joints.iter())
            .map(|&node| {
                let part = self.parts_of.get(node as usize).copied().flatten();
                let part = part.ok_or_else(|| {
                    model.bad(format!(
                        "has skin {index} of joint node {node}, which isn't in its scene"
                    ))
                })?;
                Ok(u32::try_from(part).expect("fewer than 2^32 parts"))
            })
            .collect::<Result<Vec<u32>>>()?;
        let inverse_binds = match skin.inverse_bind_matrices {
            Some(accessor) => {
                let what = format!("skin {index}");
                self.attribute(&what, "its inverse bind matrices", accessor, &[16], false)?;
                let matrices = self.floats[&accessor].0.as_chunks::<16>().0;
                if matrices.len() < joints.len() {
                    return Err(model.bad(format!(
                        "has skin {index} of {} joints and {} inverse bind matrices",
                        joints.len(),
                        matrices.len()
                    )));
                }
                (matrices[..joints.len()].iter())
                    .map(Mat4::from_cols_array)
                    .collect()
            }
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        Ok(Skin {
            joints,
            inverse_binds,
        })
    }

    /// Reads each animation into a clip, in the file's order, its channels that move nodes the
    /// scene reaches, each checked as glTF says: its times from 0 up, each after the last, and as
    /// many values as its interpolation takes, its rotations made unit. A channel of morph weights,
    /// or of a node outside the scene, is ignored, as `imported.ignored` says.
    pub(super) fn animations(&mut self) -> Result<()> {
        let (model, gltf) = (self.model, self.gltf);
        for (i, animation) in gltf.animations.iter().enumerate() {
            let name = animation.name.clone().unwrap_or_else(|| i.to_string());
            self.memory.hold(name.len())?;
            let mut channels = Vec::new();
            let mut duration: f32 = 0.0;
            for (c, channel) in animation.channels.iter().enumerate() {
                let what = format!("channel {c} of animation {i}");
                let Some(node) = channel.target.node else {
                    continue;
                };
                let path = match channel.target.path.as_str() {
                    "translation" => Moved::Translation,
                    "rotation" => Moved::Rotation,
                    "scale" => Moved::Scale,
                    "weights" => {
                        self.imported
                            .ignored
                            .push(format!("{what}, of morph weights"));
                        continue;
                    }
                    other => {
                        return Err(model.bad(format!(
                            "has {what} of path {other:?}, which glTF doesn't have"
                        )));
                    }
                };
                let part = self.parts_of.get(node as usize).copied().flatten();
                let Some(part) = part else {
                    (self.imported.ignored)
                        .push(format!("{what}, of node {node} outside the scene"));
                    continue;
                };
                let sampler = model.item(
                    &animation.samplers,
                    &format!("sampler of animation {i}"),
                    channel.sampler,
                )?;
                let interpolation = match sampler.interpolation.as_deref().unwrap_or("LINEAR") {
                    "LINEAR" => Interpolation::Linear,
                    "STEP" => Interpolation::Step,
                    "CUBICSPLINE" => Interpolation::CubicSpline,
                    other => {
                        return Err(model.bad(format!(
                            "has {what} interpolated {other:?}, which glTF doesn't have"
                        )));
                    }
                };
                self.attribute(&what, "its times", sampler.input, &[1], false)?;
                let times = self.memory.copy(&self.floats[&sampler.input].0)?;
                let ordered = times.windows(2).all(|pair| pair[0] < pair[1]);
                if !(ordered && times[0] >= 0.0) {
                    return Err(model.bad(format!(
                        "has {what} of times that don't go up from 0, each after the last"
                    )));
                }
                let rotation = path == Moved::Rotation;
                let width = if rotation { 4 } else { 3 };
                let (count, finite) = self.floats_of(
                    &what,
                    "its values",
                    sampler.output,
                    &[width],
                    rotation,
                    rotation,
                )?;
                debug_assert_eq!(count, width);
                if !finite {
                    let output = sampler.output;
                    return Err(model.bad(format!(
                        "accessor {output} holds a number that isn't finite"
                    )));
                }
                let mut values = self.memory.copy(&self.floats[&sampler.output].0)?;
                let keys = if interpolation == Interpolation::CubicSpline {
                    3
                } else {
                    1
                };
                if values.len() != times.len() * keys * width {
                    return Err(model.bad(format!(
                        "has {what} of {} times and {} values, where its interpolation takes {}",
                        times.len(),
                        values.len() / width,
                        times.len() * keys
                    )));
                }
                if rotation {
                    // Values of a cubic spline in turn its tangents and its values, of which the
                    // values alone are rotations.
                    for (v, value) in values.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        if keys == 3 && v % 3 != 1 {
                            continue;
                        }
                        let unit = unit(Quat::from_array(*value)).ok_or_else(|| {
                            model.bad(format!("has {what} of a rotation that won't make unit"))
                        })?;
                        *value = unit.to_array();
                    }
                }
                duration = duration.max(*times.last().expect("an accessor has elements"));
                channels.push(Channel {
                    part: u32::try_from(part).expect("fewer than 2^32 parts"),
                    path,
                    interpolation,
                    times,
                    values,
                });
            }
            self.memory.hold(channels.len() * size_of::<Channel>())?;
            self.imported.clips.push(Clip {
                name,
                duration,
                channels,
            });
        }
        Ok(())
    }

    /// Gives each primitive a skinned part draws its reach: the box its vertices span as the part's
    /// skin moves them, as the model rests and posed at each clip's keys' times, or at as many as
    /// [`Clip::pose_times`] gives, each clip's from the model at rest; grown by a tenth of its
    /// longest side each way, for the poses between those times, and between clips blended. Each
    /// pose bounds the vertices by the leaves [`Influences`] parts each joint's into, so that a
    /// pose costs some of the vertices; and the poses are found one at a time, of the parts that
    /// place the joints alone, in memory counted as the rest of reading is, so that a small file of
    /// many clips and parts can't ask for their product. A model whose poses would take more than
    /// [`MAX_POSE_WORK`] to bound is refused, as one of more memory is.
    pub(super) fn reach(&mut self) -> Result<()> {
        let Self {
            model,
            imported,
            memory,
            ..
        } = self;
        let Imported {
            parts,
            primitives,
            skins,
            clips,
            ..
        } = imported;
        let skinned = || {
            (parts.iter().enumerate())
                .filter_map(|(part, skinned)| Some((part, skinned.skin?, skinned.primitive?)))
        };
        if skinned().next().is_none() {
            return Ok(());
        }
        // The parts' rest, posed and placed transforms, their parents, and which of them place the
        // skins' joints, as a flag and in order; each skinned part, its leaves and its box; and
        // a clip's keys' times, as its poses' are found.
        let each_part = 2 * size_of::<Transform>()
            + size_of::<Option<usize>>()
            + size_of::<bool>()
            + size_of::<usize>()
            + size_of::<Mat4>();
        let each_skinned =
            size_of::<(usize, usize, usize)>() + size_of::<Influences>() + size_of::<[Vec3; 2]>();
        let (mut leaves, mut leaf_bytes) = (0, 0);
        for (_, skin, primitive) in skinned() {
            let (count, primitive) = (skins[skin].joints.len(), &primitives[primitive]);
            let moved = Influences::moved(count, &primitive.joints, &primitive.weights);
            leaves += Influences::most_leaves(count, moved);
            leaf_bytes += each_skinned + Influences::bytes(count, moved);
        }
        let keys = clips.iter().map(Clip::keys).max().unwrap_or(0);
        let held = parts.len() * each_part + leaf_bytes + keys * size_of::<f32>();
        memory.hold_for(held, "its skinned meshes' poses")?;
        let skinned: Vec<(usize, usize, usize)> = skinned().collect();
        let rest: Vec<Transform> = parts.iter().map(|part| part.transform).collect();
        let parents: Vec<Option<usize>> = parts.iter().map(|part| part.parent).collect();
        let mut needed = vec![false; parts.len()];
        for &(part, skin, _) in &skinned {
            needed[part] = true;
            for &joint in &skins[skin].joints {
                needed[joint as usize] = true;
            }
        }
        // Each part's parent after it, as parents come first.
        for part in (0..parts.len()).rev() {
            if let (true, Some(parent)) = (needed[part], parents[part]) {
                needed[parent] = true;
            }
        }
        let order: Vec<usize> = (0..parts.len()).filter(|&part| needed[part]).collect();
        // What each pose takes: its clip's channels set, its parts placed and its leaves posed.
        let each_pose = (order.len() + leaves) as u64;
        let work = (clips.iter()).fold(each_pose, |work, clip| {
            let pose = each_pose + clip.channels.len() as u64;
            work.saturating_add(pose.saturating_mul(clip.most_poses() as u64))
        });
        if work > MAX_POSE_WORK {
            return Err(model.unsupported(format!(
                "takes {work} steps to bound its skinned meshes as its clips pose them, past the \
                 {MAX_POSE_WORK} the engine takes: each pose's parts placed and leaves of vertices \
                 posed"
            )));
        }
        let influences: Vec<Influences> = (skinned.iter())
            .map(|&(_, skin, primitive)| {
                let primitive = &primitives[primitive];
                let (positions, joints) = (&primitive.positions, &primitive.joints);
                Influences::new(&skins[skin], positions, joints, &primitive.weights)
            })
            .collect();
        let mut boxes = vec![[Vec3::INFINITY, Vec3::NEG_INFINITY]; skinned.len()];
        let mut locals = rest.clone();
        let mut placed = vec![Mat4::IDENTITY; parts.len()];
        let mut bound = |placed: &[Mat4]| {
            for ((&(part, skin, _), influences), [low, high]) in
                (skinned.iter().zip(&influences)).zip(&mut boxes)
            {
                let [l, h] = influences.posed(placed, part, &skins[skin]);
                (*low, *high) = (low.min(l), high.max(h));
            }
        };
        place(&rest, &parents, &order, &mut placed);
        bound(&placed);
        let mut times = Vec::with_capacity(keys);
        for clip in clips.iter() {
            let channels = || (clip.channels.iter()).filter(|c| needed[c.part as usize]);
            clip.pose_times(&mut times);
            for &time in &times {
                for channel in channels() {
                    locals[channel.part as usize] = rest[channel.part as usize];
                }
                for channel in channels() {
                    let local = &mut locals[channel.part as usize];
                    channel.apply(local, channel.sample(time));
                }
                place(&locals, &parents, &order, &mut placed);
                bound(&placed);
            }
            for channel in channels() {
                locals[channel.part as usize] = rest[channel.part as usize];
            }
        }
        for (&(_, _, primitive), &posed) in skinned.iter().zip(&boxes) {
            let primitive = &mut primitives[primitive];
            let [low, high] =
                (primitive.reach.into_iter()).fold(posed, |[l, h], [a, b]| [l.min(a), h.max(b)]);
            let spare = Vec3::splat((high - low).max_element() / 10.0);
            primitive.reach = Some([low - spare, high + spare]);
        }
        memory.release(held);
        Ok(())
    }
}
