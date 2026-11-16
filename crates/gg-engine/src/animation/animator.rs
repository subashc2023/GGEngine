//! Animators: a model's clips played on its parts, sampled, blended and cross-faded at the ticks.

use std::mem;
use std::sync::Arc;

use gg_core::math::{Quat, Vec4};
use gg_core::{Error, Result};

use super::Playback;
use super::skeleton::{Channel, Clip, Path};
use crate::ecs::node_id;
use crate::{Component, Entity, ModelRoot, Reflect, Tick, Transform, World};

/// What a model's root plays on its model's parts, at the ticks
/// [`step_animation`](super::step_animation) runs in: clips of its model, each at a weight, which
/// blend where they move the same parts, as a play's [`weight`](Play::weight) says, so that a walk
/// and a run blend into a jog, and cross-fade as their weights ease, as [`World::play`] has them
/// do. A part no play moves keeps its transform, as the model spawned it or code set it; a part
/// whose node has a skin's joints moves the skinned mesh's vertices with it, as the frame draws
/// them. An entity without a [`ModelRoot`] plays nothing.
#[derive(Clone, Debug, Default, PartialEq, Component)]
pub struct Animator(pub Vec<Play>);

/// A clip an [`Animator`] plays.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct Play {
    /// The clip, by its name among its model's: its glTF animation's name, or, for one without, its
    /// index among them, as text, such as `"0"`.
    pub clip: String,
    /// The seconds it has played, which each tick moves on by its speed: brought back within a
    /// round as it loops, and kept at its end once it has played once. Negative, while it plays
    /// forward, it waits that long at its start.
    pub time: f32,
    /// How fast it plays: 1 as the clip goes, 2 twice as fast, and backward below 0.
    #[reflect(default = 1.0)]
    pub speed: f32,
    /// Whether it plays once and holds its last pose, loops, or goes back and forth.
    #[reflect(default = Playback::Loop)]
    pub playback: Playback,
    /// How much it gives of the pose of each part it moves among the plays that move it, as
    /// weights blend: of two at 1 and 3, a quarter and three quarters.
    #[reflect(min = 0.0, default = 1.0)]
    pub weight: f32,
    /// The weight it eases to, at a steady rate, as `fade` says; one that has eased to 0 leaves the
    /// animator.
    #[reflect(min = 0.0, default = 1.0)]
    pub target: f32,
    /// The seconds its weight takes to ease from 0 to 1, or back: at 0, it's at its target at once.
    #[reflect(min = 0.0)]
    pub fade: f32,
}

impl Play {
    /// A play of `clip`, from its start, as it goes, looping, at a weight of 1.
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

    /// Moves it on by `dt` seconds through a clip of `duration` seconds, and its weight toward its
    /// target, as its fields say.
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

    /// Where it is in a clip of `duration` seconds: its start while it waits, and back from the
    /// end on its way back, going back and forth.
    fn clip_time(&self, duration: f32) -> f32 {
        let time = self.time.max(0.0);
        match self.playback {
            Playback::PingPong if time > duration => 2.0 * duration - time,
            _ => time.min(duration),
        }
    }
}

/// The clip of `clips` named `name`, the first of that name.
pub(super) fn named<'c>(clips: &'c [Clip], name: &str) -> Option<&'c Clip> {
    clips.iter().find(|clip| clip.name == name)
}

/// The channel of `clip` that moves `part`'s `path`, the first.
fn moving(clip: &Clip, part: u32, path: Path) -> Option<&Channel> {
    (clip.channels.iter()).find(|channel| channel.part == part && channel.path == path)
}

/// Steps each animator of `world` a tick, [`Tick::DT`]: each play moves on and eases its weight,
/// one of a clip its model hasn't leaving with a warning (`animation.no_clip`), and one eased to
/// 0 leaving; then each part a play moves takes the plays' poses blended by their weights, a
/// translation or scale as their mean, a rotation as their normalized sum, each the same way
/// round as the first, written only where it changed. Each part's node is found by the ID
/// [`node_id`] mixes from the root's, without a map, so a tick allocates nothing.
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
                // Blended where the first play that moves it is.
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

/// The pose of `part`'s `path` that `plays`, which `clips` are of, give blended by their weights, of
/// those whose clips move it, as [`step`] says: the first's alone, as it samples, if it's the only
/// one.
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
    /// Has `entity`, a model's root, play its model's clip `clip`, cross-fading from what its
    /// [`Animator`] plays over `fade` seconds: each other play eases to a weight of 0, and leaves
    /// once it's there; and the play of `clip` eases to 1, from where it is, if the animator plays
    /// it, or from its start, at 0, looping, if it doesn't. A fade of 0 cuts to it at once. Gives
    /// the entity an animator if it has none.
    ///
    /// Fails with `animation.no_clip`, naming the model's clips, if the entity has no
    /// [`ModelRoot`], or its model has no clip of that name; and with `reflect.out_of_range` for a
    /// fade that's negative or isn't a number.
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

    /// A model of a root part and its child, and clips of them: "Slide", the root along X from 0
    /// to 2 over a second, along a line, and the child about Z a quarter turn; "Lift", the root
    /// along Y to 4 over two seconds by steps of a second; and "Swing", the child about Z along a
    /// cubic spline, whose tangents are 0, so that it eases in and out.
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

    /// A context with the model of [`clips`], and a root of it, which it returns.
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

    /// The clips and weights of `root`'s animator's plays.
    pub(crate) fn weights(world: &World, root: Entity) -> Vec<(String, f32)> {
        let Animator(plays) = world.get::<Animator>(root).expect("an animator");
        plays
            .iter()
            .map(|play| (play.clip.clone(), play.weight))
            .collect()
    }

    /// A clip's channels sample along their keys: a line between them, a rotation the shorter
    /// way round, steps held until the next, and a cubic spline through its values with its
    /// tangents; the first key's before them and the last's after.
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
        // Eased in and out: a quarter of the way in time, less than a quarter of the way round,
        // and halfway at half.
        let angle = |time| {
            let (axis, angle) = Quat::from_vec4(swing.channels[0].sample(time)).to_axis_angle();
            angle * axis.z.signum()
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(angle(0.25) < quarter / 4.0, "{}", angle(0.25));
        assert!((angle(0.5) - quarter / 2.0).abs() < 1e-5, "{}", angle(0.5));
        assert!((angle(1.0) - quarter).abs() < 1e-5, "{}", angle(1.0));
    }

    /// A play moves its model's parts as its clip samples at its time, which each tick moves on by
    /// its speed: looping, it starts again; once, it holds its last pose; and back and forth, it
    /// comes back the way it went. A part it doesn't move keeps its transform.
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
        // Lift moves the root alone, so its child keeps the transform it spawned with.
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

    /// Plays that move the same part blend by their weights; one eased to 0 leaves; and
    /// `World::play` cross-fades, the play of the clip asked for easing to 1 as each other eases
    /// to 0, over the fade, then alone; a clip it doesn't play joins at 0, from its start.
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
        // Slide's (2, 0, 0) a quarter and Lift's (0, 4, 0) three quarters.
        let root_node = node(world, root, 0);
        assert_eq!(at(world, root_node).translation, DVec3::new(0.5, 3.0, 0.0));
        world.play(root, "Slide", 0.25).expect("its clip");
        // Slide, at 1, has nowhere to ease to; Lift eases from 3 toward 0 at 4 a second.
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

    /// A play of a clip its model hasn't is warned of and leaves the animator, the others going
    /// on; and `World::play` refuses one, a root without a model, and a fade that isn't 0 or more.
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

    /// The world's hash after roots have played each clip, looping, once and back and forth, at
    /// their own speeds, blended, and cross-faded part of the way through, is the same on every
    /// platform, as a model's clips, sampled and blended, must be for lockstep peers and replays:
    /// a reference CI checks on every platform it runs.
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
