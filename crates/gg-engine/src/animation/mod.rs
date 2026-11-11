//! Animation at the world's ticks: sprites played through a sheet's cells, components' fields eased
//! from one value to another, and models' clips played on their parts, which move skinned meshes'
//! vertices as their joints, by state machines among them. Their state is the world's, in their
//! components, stepped by
//! [`Tick::DT`](crate::Tick::DT) at each tick a layer runs [`step_animation`] in, so that
//! snapshots, replays and peers see them alike.

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
    /// It stops there: a flipbook on its last cell, a tween with its field at its end, which
    /// leaves its entity's [`Tweens`], and a clip on its last pose.
    #[default]
    Once,
    /// It starts again from its start.
    Loop,
    /// It goes back to its start, and then forward again, and so on.
    PingPong,
}

/// Steps `world`'s animation a tick, [`Tick::DT`](crate::Tick::DT), as a system a layer's tick
/// runs, after what starts its tweens, plays its clips and sets its state machines' parameters,
/// such as scripts, and before physics, which takes in bodies where animation put them: its
/// [`Flipbook`]s, then its [`Tweens`], then its [`AnimationStates`], which set their animators'
/// plays, then its [`Animator`]s.
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

    /// The world's hash after flipbooks of each playback, and tweens of each curve eased at each
    /// end and at both, of positions, rotations and a sprite's alpha, have stepped four seconds: a
    /// reference CI checks on every platform, as curves reach libm and rotations glam's slerp. A
    /// difference on some platforms only is a bug in determinism, not a value to change here.
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
        assert_eq!(content, 0x67ff_00cd_b39a_493b, "{content:#018x}");
    }
}
