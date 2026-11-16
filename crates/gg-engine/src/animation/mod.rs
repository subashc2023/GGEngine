//! Animation at the world's ticks: sprite flipbooks, tweens of components' fields, models' clips
//! (which move skinned meshes' joints) and state machines over them. See
//! [Animation](../../../../ARCHITECTURE.md#animation).
//!
//! State lives in components and steps by [`Tick::DT`](crate::Tick::DT) at each tick a layer runs
//! [`step_animation`], so snapshots, replays and peers see it alike.

mod animator;
mod flipbook;
mod skeleton;
mod states;
mod tween;

pub use animator::{Animator, Play};
pub use flipbook::Flipbook;
pub(crate) use skeleton::{Channel, Clip, Influences, Interpolation, Path, Skin, place};
pub use states::{
    AnimationParam, AnimationState, AnimationStates, BlendPoint, ParamCondition, ParamTest,
    ParamValue, StateMotion, StateTransition,
};
pub use tween::{Curve, Ease, Tween, TweenValue, Tweens};

use crate::{Reflect, World};

/// How an animation goes on once it reaches its end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Playback {
    /// Stops there: a flipbook on its last cell, a tween with its field at its end (it leaves its
    /// entity's [`Tweens`]), a clip on its last pose.
    #[default]
    Once,
    /// Starts again from the start.
    Loop,
    /// Goes back to the start, then forward again, and so on.
    PingPong,
}

/// Steps `world`'s animation one [`Tick::DT`](crate::Tick::DT), as a system in a layer's tick.
///
/// Run it after what starts tweens, plays clips and sets state machines' parameters (scripts, say),
/// and before physics, which takes in bodies where animation put them. Order: [`Flipbook`]s,
/// [`Tweens`], [`AnimationStates`] (which set their animators' plays), then [`Animator`]s.
pub fn step_animation(world: &mut World) {
    let _zone = gg_core::zone!();
    flipbook::step(world);
    tween::step(world);
    states::step(world);
    animator::step(world);
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Quat, Vec3};

    use super::*;
    use crate::{Sprite, Transform};

    /// The world's hash after four seconds of flipbooks of each playback and tweens of each curve
    /// and ease (positions, rotations, a sprite's alpha): a reference CI checks on every platform,
    /// as curves reach libm and rotations glam's slerp. A difference on some platforms only is a
    /// determinism bug, not a value to change.
    #[test]
    fn animation_hashes_as_its_reference_says() {
        let logged = crate::testing::logged();
        let mut world = World::seeded(7);
        let curves = [
            Curve::Linear,
            Curve::Sine,
            Curve::Quad,
            Curve::Cubic,
            Curve::Back,
            Curve::Elastic,
            Curve::Bounce,
        ];
        let playbacks = [Playback::Once, Playback::Loop, Playback::PingPong];
        for (i, curve) in curves.into_iter().enumerate() {
            for (j, ease) in [Ease::In, Ease::Out, Ease::InOut].into_iter().enumerate() {
                let playback = playbacks[(i + j) % 3];
                let flipbook = Flipbook {
                    columns: 4,
                    rows: 4,
                    first: i as u32,
                    frames: 5 + j as u32,
                    rate: 7.0 + i as f32,
                    playback,
                    time: -0.1 * j as f32,
                };
                let entity = world.spawn((Transform::IDENTITY, Sprite::default(), flipbook));
                let to = Vec3::new(1.0, 2.0, 3.0);
                let seconds = 0.7 + 0.3 * i as f32;
                let turned = Quat::from_rotation_y(2.0);
                let tweens = [
                    Tween::new("Transform", "translation", Vec3::ZERO, to, seconds),
                    Tween::new("Transform", "rotation", Quat::IDENTITY, turned, 1.3),
                    Tween::new("Sprite", "color.w", 0.9, 0.2, 0.45),
                ];
                for tween in tweens {
                    let tween = Tween {
                        curve,
                        ease,
                        playback,
                        ..tween
                    };
                    world.tween(entity, tween).expect("fields of their kinds");
                }
            }
        }
        for _ in 0..256 {
            step_animation(&mut world);
        }
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.is_empty(), "every tween set its field: {codes:?}");
        let content = world.hash().content;
        assert_eq!(content, 0xf582_1fb6_3439_e843, "{content:#018x}");
    }
}
