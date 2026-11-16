//! Animators: a model's clips played on its parts, sampled, blended and cross-faded at the ticks.

use std::mem;
use std::sync::Arc;

use gg_core::math::{Quat, Vec4};
use gg_core::{Error, Result};

use super::Playback;
use super::skeleton::{Channel, Clip, Path};
use crate::ecs::node_id;
use crate::{Component, Entity, ModelRoot, Reflect, Tick, Transform, World};

/// What a model's root plays on its parts: clips of its model, each at a weight.
///
/// Plays that move the same part blend by [`weight`](Play::weight), so a walk and a run make a
/// jog, and cross-fade as their weights ease ([`World::play`]). A part no play moves keeps its
/// transform, as spawned or set by code. A part whose node has a skin's joints moves the skinned
/// mesh's vertices with it. An entity without a [`ModelRoot`] plays nothing.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub struct Animator(pub Vec<Play>);

/// A clip an [`Animator`] plays.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct Play {
    /// The clip's name among its model's: its glTF animation's name, or for one without, its
    /// index as text, such as `"0"`.
    pub clip: String,
    /// Seconds played. Each tick advances it by `speed`; looping wraps it within a round, and
    /// playing once stops it at the end. Negative, playing forward, it waits that long at the
    /// start.
    pub time: f32,
    /// Playback rate: 1 as the clip goes, 2 twice as fast, backward below 0.
    #[reflect(default = 1.0)]
    pub speed: f32,
    /// Plays once and holds the last pose, loops, or goes back and forth.
    #[reflect(default = Playback::Loop)]
    pub playback: Playback,
    /// Its share of the pose of each part it moves, among the plays that move it: two at 1 and 3
    /// give a quarter and three quarters.
    #[reflect(min = 0.0, default = 1.0)]
    pub weight: f32,
    /// The weight it eases to, at a steady rate set by `fade`. A play that has eased to 0 leaves
    /// the animator.
    #[reflect(min = 0.0, default = 1.0)]
    pub target: f32,
    /// Seconds its weight takes to ease from 0 to 1, or back. At 0 it's at `target` at once.
    #[reflect(min = 0.0)]
    pub fade: f32,
}

impl Play {
    /// A play of `clip` from its start, at its speed, looping, at weight 1.
    pub fn new(clip: &str) -> Self {
        Self {
            clip: clip.to_owned(),
            time: 0.0,
            speed: 1.0,
            playback: Playback::Loop,
            weight: 1.0,
            target: 1.0,
            fade: 0.0,
        }
    }

    /// Advances its time by `dt` seconds through a clip of `duration`, and its weight toward its
    /// target.
    fn advance(&mut self, duration: f32, dt: f32) {
        if self.speed.is_finite() {
            self.time += self.speed * dt;
        }
        if duration.is_nan() || duration <= 0.0 {
            self.time = self.time.min(0.0);
        } else {
            match self.playback {
                Playback::Once => self.time = self.time.min(duration),
                Playback::Loop if self.time >= duration || self.speed < 0.0 => {
                    self.time = self.time.rem_euclid(duration);
                }
                Playback::PingPong if self.time >= 2.0 * duration || self.speed < 0.0 => {
                    self.time = self.time.rem_euclid(2.0 * duration);
                }
                Playback::Loop | Playback::PingPong => {}
            }
        }
        if !self.time.is_finite() {
            self.time = 0.0;
        }
        let target = if self.target.is_finite() {
            self.target.max(0.0)
        } else {
            0.0
        };
        self.weight = if self.fade > 0.0 && self.weight.is_finite() {
            let step = dt / self.fade;
            if self.weight < target {
                (self.weight + step).min(target)
            } else {
                (self.weight - step).max(target)
            }
        } else {
            target
        };
    }

    /// Its place in a clip of `duration` seconds: the start while waiting, counting back from the
    /// end on a ping-pong's way back.
    fn clip_time(&self, duration: f32) -> f32 {
        let time = self.time.max(0.0);
        match self.playback {
            Playback::PingPong if time > duration => 2.0 * duration - time,
            _ => time.min(duration),
        }
    }
}

/// The first clip named `name`.
pub(super) fn named<'c>(clips: &'c [Clip], name: &str) -> Option<&'c Clip> {
    clips.iter().find(|clip| clip.name == name)
}

/// The first channel of `clip` that moves `part`'s `path`.
fn moving(clip: &Clip, part: u32, path: Path) -> Option<&Channel> {
    (clip.channels.iter()).find(|channel| channel.part == part && channel.path == path)
}

/// Steps each animator one [`Tick::DT`].
///
/// Each play advances and eases its weight. A play of a clip its model lacks is warned of
/// (`animation.no_clip`) and leaves, as does one eased to 0. Then each part a play moves takes the
/// plays' poses blended by weight: translation or scale as their weighted mean, rotation as their
/// normalized sum, each the same way round as the first. A part is written only where it changed.
/// Its node is found by the ID [`node_id`] mixes from the root's, without a map, so a tick
/// allocates nothing.
pub(super) fn step(world: &mut World) {
    let dt = Tick::DT.as_secs_f32();
    let mut roots = world.take_list();
    roots.extend((world.query::<(Entity, &Animator, &ModelRoot)>()).map(|(root, _, _)| root));
    for &root in &roots {
        let (Some(&ModelRoot(model)), Some(id)) = (world.get::<ModelRoot>(root), world.id(root))
        else {
            continue;
        };
        let Some(clips) = world
            .assets()
            .nodes(model)
            .map(|nodes| Arc::clone(&nodes.clips))
        else {
            continue;
        };
        let mut plays = match world.get_mut::<Animator>(root) {
            Some(animator) => mem::take(&mut animator.0),
            None => continue,
        };
        plays.retain_mut(|play| match named(&clips, &play.clip) {
            Some(clip) => {
                play.advance(clip.duration, dt);
                play.weight > 0.0 || play.target > 0.0
            }
            None => {
                tracing::warn!(
                    code = "animation.no_clip",
                    entity = %id,
                    clip = play.clip,
                    "Entity {id}'s animator plays clip {:?}, which its model hasn't: it's left out",
                    play.clip
                );
                false
            }
        });
        let shown = |play: &Play| play.weight > 0.0;
        for (i, play) in plays.iter().enumerate().filter(|(_, play)| shown(play)) {
            let clip = named(&clips, &play.clip).expect("kept for its clip");
            for channel in &clip.channels {
                let (part, path) = (channel.part, channel.path);
                // Blended at the first play that moves it.
                let earlier = plays[..i].iter().filter(|play| shown(play));
                if earlier
                    .filter_map(|play| named(&clips, &play.clip))
                    .any(|clip| moving(clip, part, path).is_some())
                {
                    continue;
                }
                let value = blend(&clips, &plays[i..], part, path);
                let Some(node) = node_id(id, part as usize).and_then(|node| world.entity(node))
                else {
                    continue;
                };
                let Some(&transform) = world.get::<Transform>(node) else {
                    continue;
                };
                let mut posed = transform;
                channel.apply(&mut posed, value);
                if posed != transform
                    && let Some(placed) = world.get_mut::<Transform>(node)
                {
                    *placed = posed;
                }
            }
        }
        if let Some(animator) = world.get_mut::<Animator>(root) {
            animator.0 = plays;
        }
    }
    world.keep_list(roots);
}

/// The pose of `part`'s `path` blended by weight from `plays` whose clips move it, as [`step`]
/// says. A lone play's sample is used as it is.
fn blend(clips: &[Clip], plays: &[Play], part: u32, path: Path) -> Vec4 {
    let mut first: Option<Vec4> = None;
    let (mut sum, mut total, mut count) = (Vec4::ZERO, 0.0, 0);
    for play in plays.iter().filter(|play| play.weight > 0.0) {
        let Some(clip) = named(clips, &play.clip) else {
            continue;
        };
        let Some(channel) = moving(clip, part, path) else {
            continue;
        };
        let mut value = channel.sample(play.clip_time(clip.duration));
        let first = *first.get_or_insert(value);
        if path == Path::Rotation && first.dot(value) < 0.0 {
            value = -value;
        }
        sum += value * play.weight;
        total += play.weight;
        count += 1;
    }
    let first = first.expect("the first play moves it");
    match (count, path) {
        (1, _) => first,
        (_, Path::Rotation) => Vec4::from(Quat::from_vec4(sum).normalize()),
        _ => sum / total,
    }
}

impl World {
    /// Has `entity`, a model's root, play its model's `clip`, cross-fading over `fade` seconds
    /// from what its [`Animator`] plays: every other play eases to weight 0 and leaves there, and
    /// `clip` eases to 1 from where it is, or from its start at weight 0, looping, if not playing.
    /// A fade of 0 cuts at once. Gives the entity an animator if it has none.
    ///
    /// # Errors
    ///
    /// - `animation.no_clip`, naming the model's clips: no [`ModelRoot`], or no such clip.
    /// - `reflect.out_of_range`: `fade` is negative or NaN.
    pub fn play(&mut self, entity: Entity, clip: &str, fade: f32) -> Result<()> {
        if fade.is_nan() || fade < 0.0 {
            return Err(Error::new(
                "reflect.out_of_range",
                format!("a fade of {fade} seconds, where a fade is 0 or more"),
            ));
        }
        let model = self.get::<ModelRoot>(entity).map(|&ModelRoot(model)| model);
        let clips = model.and_then(|model| self.assets().nodes(model));
        let clips = clips.map(|nodes| Arc::clone(&nodes.clips));
        if !clips
            .as_ref()
            .is_some_and(|clips| named(clips, clip).is_some())
        {
            let names: Vec<&str> = (clips.iter().flat_map(|clips| clips.iter()))
                .map(|clip| clip.name.as_str())
                .collect();
            let id = self
                .id(entity)
                .map_or_else(|| "that's gone".to_owned(), |id| id.to_string());
            return Err(Error::new(
                "animation.no_clip",
                format!(
                    "entity {id} has no clip {clip:?} to play: its model's clips are {names:?}"
                ),
            ));
        }
        if self.get::<Animator>(entity).is_none() {
            self.insert(entity, Animator::default())?;
        }
        let animator = self.get_mut::<Animator>(entity).expect("given one");
        for play in animator.0.iter_mut().filter(|play| play.clip != clip) {
            play.target = 0.0;
            play.fade = fade;
        }
        match animator.0.iter_mut().find(|play| play.clip == clip) {
            Some(play) => {
                play.target = 1.0;
                play.fade = fade;
            }
            None => animator.0.push(Play {
                weight: if fade > 0.0 { 0.0 } else { 1.0 },
                fade,
                ..Play::new(clip)
            }),
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use gg_core::math::DVec3;

    use super::*;
    use crate::animation::{Interpolation, step_animation};
    use crate::gltf::{Imported, Part};
    use crate::model::tests::added;
    use crate::{Context, Model};

    /// A model of a root part and its child, with clips: "Slide", the root along X from 0 to 2 in
    /// a second, linearly, and the child a quarter turn about Z; "Lift", the root along Y to 4 in
    /// two seconds, in steps of a second; "Swing", the child about Z on a cubic spline with 0
    /// tangents, so it eases in and out.
    pub(crate) fn clips() -> Imported {
        let part = |parent| Part {
            name: None,
            transform: Transform::IDENTITY,
            parent,
            primitive: None,
            light: None,
            skin: None,
        };
        let channel = |part, path, interpolation, times: &[f32], values: &[f32]| Channel {
            part,
            path,
            interpolation,
            times: times.to_vec(),
            values: values.to_vec(),
        };
        let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
        let still = Quat::IDENTITY.to_array();
        let linear = Interpolation::Linear;
        let slide = [0.0, 0.0, 0.0, 2.0, 0.0, 0.0];
        let turn = [still, quarter].concat();
        let swing = [[0.0; 4], still, [0.0; 4], [0.0; 4], quarter, [0.0; 4]].concat();
        let lift = [0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 4.0, 0.0];
        Imported {
            parts: vec![part(None), part(Some(0))],
            clips: vec![
                Clip {
                    name: "Slide".to_owned(),
                    duration: 1.0,
                    channels: vec![
                        channel(0, Path::Translation, linear, &[0.0, 1.0], &slide),
                        channel(1, Path::Rotation, linear, &[0.0, 1.0], &turn),
                    ],
                },
                Clip {
                    name: "Lift".to_owned(),
                    duration: 2.0,
                    channels: vec![channel(
                        0,
                        Path::Translation,
                        Interpolation::Step,
                        &[0.0, 1.0, 2.0],
                        &lift,
                    )],
                },
                Clip {
                    name: "Swing".to_owned(),
                    duration: 1.0,
                    channels: vec![channel(
                        1,
                        Path::Rotation,
                        Interpolation::CubicSpline,
                        &[0.0, 1.0],
                        &swing,
                    )],
                },
            ],
            ..Imported::default()
        }
    }

    /// The node of part `part` of `root`'s model.
    pub(crate) fn node(world: &World, root: Entity, part: usize) -> Entity {
        let id = world.id(root).expect("alive");
        world
            .entity(node_id(id, part).expect("an ID"))
            .expect("spawned")
    }

    /// A context with the model of [`clips`], and the root it spawns.
    pub(crate) fn spawned() -> (Context, Entity) {
        let mut cx = Context::seeded(1);
        let (model, _) = added(&mut cx, "m", clips());
        let root = Model::spawn(model, cx.world_mut(), Transform::IDENTITY);
        (cx, root)
    }

    /// `entity`'s transform in `world`.
    pub(crate) fn at(world: &World, entity: Entity) -> Transform {
        *world.get::<Transform>(entity).expect("placed")
    }

    /// The clips and weights of `root`'s plays.
    pub(crate) fn weights(world: &World, root: Entity) -> Vec<(String, f32)> {
        let Animator(plays) = world.get::<Animator>(root).expect("an animator");
        plays
            .iter()
            .map(|play| (play.clip.clone(), play.weight))
            .collect()
    }

    /// Channels sample along their keys: a line between them, rotation the shorter way, steps held
    /// until the next, a cubic spline through the values with its tangents. Before the first key
    /// the first holds, after the last the last.
    #[test]
    fn channels_sample_along_their_keys() {
        let clips = clips().clips;
        let [slide, lift, swing] = &clips[..] else {
            panic!("three clips");
        };
        let x = |time| slide.channels[0].sample(time).x;
        assert_eq!(
            [x(-1.0), x(0.25), x(0.5), x(1.0), x(5.0)],
            [0.0, 0.5, 1.0, 2.0, 2.0]
        );
        let eighth = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let turned = Quat::from_vec4(slide.channels[1].sample(0.5));
        assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
        let y = |time| lift.channels[0].sample(time).y;
        assert_eq!(
            [y(0.0), y(0.99), y(1.0), y(1.5), y(2.0)],
            [0.0, 0.0, 2.0, 2.0, 4.0]
        );
        // Eased in and out: under a quarter round at a quarter of the time, halfway at half.
        let angle = |time| {
            let (axis, angle) = Quat::from_vec4(swing.channels[0].sample(time)).to_axis_angle();
            angle * axis.z.signum()
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(angle(0.25) < quarter / 4.0, "{}", angle(0.25));
        assert!((angle(0.5) - quarter / 2.0).abs() < 1e-5, "{}", angle(0.5));
        assert!((angle(1.0) - quarter).abs() < 1e-5, "{}", angle(1.0));
    }

    /// A play moves its model's parts as its clip samples at its time: looping restarts, once holds
    /// the last pose, a ping-pong comes back. A part it doesn't move keeps its transform.
    #[test]
    fn a_play_moves_its_parts_through_its_clip_as_its_playback_says() {
        for (playback, ticks, x) in [
            (Playback::Loop, 16, 0.5),
            (Playback::Loop, 80, 0.5),
            (Playback::Once, 80, 2.0),
            (Playback::PingPong, 80, 1.5),
            (Playback::PingPong, 144, 0.5),
        ] {
            let (mut cx, root) = spawned();
            let world = cx.world_mut();
            let slide = Play {
                playback,
                ..Play::new("Slide")
            };
            world.insert(root, Animator(vec![slide])).expect("alive");
            for _ in 0..ticks {
                step_animation(world);
            }
            let moved = at(world, node(world, root, 0)).translation;
            assert_eq!(
                moved,
                DVec3::new(x, 0.0, 0.0),
                "{playback:?} after {ticks} ticks"
            );
        }
        // Lift moves the root alone: the child keeps its spawned transform.
        let (mut cx, root) = spawned();
        let world = cx.world_mut();
        world
            .insert(root, Animator(vec![Play::new("Lift")]))
            .expect("alive");
        let child = node(world, root, 1);
        let rest = at(world, child);
        for _ in 0..70 {
            step_animation(world);
        }
        assert_eq!(
            at(world, node(world, root, 0)).translation,
            DVec3::new(0.0, 2.0, 0.0)
        );
        assert_eq!(at(world, child), rest);
    }

    /// Plays on the same part blend by weight; one eased to 0 leaves; `World::play` cross-fades,
    /// its clip easing to 1 as the others ease to 0, then alone; a clip not playing joins at 0,
    /// from its start.
    #[test]
    fn plays_blend_by_their_weights_and_cross_fade() {
        let (mut cx, root) = spawned();
        let world = cx.world_mut();
        let slide = Play {
            playback: Playback::Once,
            ..Play::new("Slide")
        };
        let lift = Play {
            weight: 3.0,
            target: 3.0,
            playback: Playback::Once,
            ..Play::new("Lift")
        };
        world
            .insert(root, Animator(vec![slide, lift]))
            .expect("alive");
        for _ in 0..128 {
            step_animation(world);
        }
        // A quarter of Slide's (2, 0, 0) and three quarters of Lift's (0, 4, 0).
        let root_node = node(world, root, 0);
        assert_eq!(at(world, root_node).translation, DVec3::new(0.5, 3.0, 0.0));
        world.play(root, "Slide", 0.25).expect("its clip");
        // Slide is at 1 already; Lift eases from 3 toward 0 at 4 a second.
        step_animation(world);
        let eased = [
            ("Slide".to_owned(), 1.0),
            ("Lift".to_owned(), 3.0 - 4.0 / 64.0),
        ];
        assert_eq!(weights(world, root), eased);
        for _ in 0..47 {
            step_animation(world);
        }
        assert_eq!(weights(world, root), [("Slide".to_owned(), 1.0)]);
        assert_eq!(at(world, root_node).translation, DVec3::new(2.0, 0.0, 0.0));
        world.play(root, "Swing", 0.5).expect("its clip");
        step_animation(world);
        let joined = weights(world, root);
        assert_eq!(joined[1], ("Swing".to_owned(), 2.0 / 64.0), "{joined:?}");
    }

    /// A play of a clip its model lacks is warned of and leaves, the others going on;
    /// `World::play` refuses one, a root without a model, and a fade under 0 or NaN.
    #[test]
    fn a_clip_its_model_hasnt_is_refused_or_left_out() {
        let logged = crate::testing::logged();
        let (mut cx, root) = spawned();
        let world = cx.world_mut();
        let plays = vec![Play::new("Dance"), Play::new("Slide")];
        world.insert(root, Animator(plays)).expect("alive");
        step_animation(world);
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert_eq!(codes, ["animation.no_clip"]);
        assert_eq!(weights(world, root), [("Slide".to_owned(), 1.0)]);
        let refused = world.play(root, "Dance", 0.0).expect_err("no such clip");
        assert_eq!(refused.code(), "animation.no_clip");
        let named = refused.to_string();
        assert!(
            named.contains("[\"Slide\", \"Lift\", \"Swing\"]"),
            "{named}"
        );
        let bare = world.spawn(Transform::IDENTITY);
        let unmodeled = world.play(bare, "Slide", 0.0).expect_err("no model");
        assert_eq!(unmodeled.code(), "animation.no_clip");
        let fade = world
            .play(root, "Slide", f32::NAN)
            .expect_err("a fade of no number");
        assert_eq!(fade.code(), "reflect.out_of_range");
    }

    /// The world's hash after roots play each clip (looping, once, ping-pong, at their own speeds),
    /// blended and cross-faded midway: a reference CI checks on every platform, as sampled and
    /// blended clips must match for lockstep peers and replays.
    #[test]
    fn skeletal_animation_hashes_as_its_reference_says() {
        let logged = crate::testing::logged();
        let mut cx = Context::seeded(7);
        let (model, _) = added(&mut cx, "m", clips());
        let world = cx.world_mut();
        let playbacks = [Playback::Once, Playback::Loop, Playback::PingPong];
        let mut roots = Vec::new();
        for (i, clip) in ["Slide", "Lift", "Swing"].into_iter().enumerate() {
            for (j, playback) in playbacks.into_iter().enumerate() {
                let root = Model::spawn(model, world, Transform::IDENTITY);
                let play = Play {
                    playback,
                    speed: 0.6 + 0.35 * j as f32,
                    time: -0.05 * i as f32,
                    ..Play::new(clip)
                };
                let swing = Play {
                    weight: 0.3 + 0.2 * i as f32,
                    target: 0.3 + 0.2 * i as f32,
                    ..Play::new("Swing")
                };
                world
                    .insert(root, Animator(vec![play, swing]))
                    .expect("alive");
                roots.push(root);
            }
        }
        for tick in 0..256 {
            if tick == 100 {
                for (i, &root) in roots.iter().enumerate() {
                    world
                        .play(root, ["Lift", "Swing", "Slide"][i % 3], 0.4)
                        .expect("a clip");
                }
            }
            step_animation(world);
        }
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.is_empty(), "every clip played: {codes:?}");
        let content = world.hash().content;
        assert_eq!(content, 0x416a_9d4b_facf_9131, "{content:#018x}");
    }
}
