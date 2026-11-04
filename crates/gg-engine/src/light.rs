//! Lights: the component that lights a world's models, and the lights a frame's lit shading takes
//! from the world as it draws.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use gg_core::math::Vec3;

use crate::draw::Poses;
use crate::interpolation::Past;
use crate::{Component, Reflect, Transform, World};

/// How a [`Light`] shines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum LightKind {
    /// From far away, all one way, as the sun's: along its entity's -Z, its intensity in lux, what
    /// reaches a surface that faces it, wherever it is.
    #[default]
    Directional,
    /// From its entity, every way alike: its intensity in candela, falling off with the square of
    /// the distance.
    Point,
    /// From its entity, in a cone round its -Z: wholly within its inner cone's angle of it, fading
    /// to nothing at its outer's, its intensity in candela, as a point light's.
    Spot,
}

/// A light, as KHR_lights_punctual describes them, which lights the models of its entity's world
/// from where the entity draws, ancestors carrying it, as a camera sees from where its entity
/// draws, and the way it faces: models' materials shade as glTF's metallic-roughness model says,
/// by every light, each frame, the environment's exposure scaling their light. A world without
/// lights draws its models unlit, their base colors alone. An entity with a light and no
/// [`Transform`] lights nothing. Scenes save it, and the Inspector and tools edit it, as any
/// component; a model's nodes have the lights its file gives them.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Light {
    /// How it shines: directional, from a point, or as a spot.
    pub kind: LightKind,
    /// Its color, in linear RGB.
    #[reflect(color, default = Vec3::ONE)]
    pub color: Vec3,
    /// How bright it is: in lux for a directional light, and in candela for the rest. 100,000 by
    /// default, a directional light's for a sunny day, which an [`Environment`]'s default exposure
    /// suits.
    ///
    /// [`Environment`]: crate::Environment
    #[reflect(min = 0.0, default = 100_000.0)]
    pub intensity: f32,
    /// How far a point or spot light reaches, past which it lights nothing, its falloff easing to
    /// nothing there; `None` for as far as its falloff takes it.
    pub range: Option<f32>,
    /// The angle from its -Z within which a spot lights wholly, from 0 to its outer cone's: one
    /// past it is taken as its outer cone's.
    #[reflect(degrees, min = 0.0, default = 0.0)]
    pub inner_cone: f32,
    /// The angle from its -Z at which a spot's light fades to nothing, up to a quarter turn: one
    /// past it is taken as a quarter turn.
    #[reflect(degrees, min = 0.0, default = FRAC_PI_4)]
    pub outer_cone: f32,
}

impl Default for Light {
    /// A directional light of white, as bright as a sunny day's sun.
    fn default() -> Self {
        Self {
            kind: LightKind::Directional,
            color: Vec3::ONE,
            intensity: 100_000.0,
            range: None,
            inner_cone: 0.0,
            outer_cone: FRAC_PI_4,
        }
    }
}

impl Light {
    /// It as the renderer shades with it, from `position`, shining along `direction`.
    fn lit(&self, position: Vec3, direction: Vec3) -> gg_renderer::Light {
        // KHR_lights_punctual keeps a spot's angles to 0 <= inner < outer <= a quarter turn; the
        // renderer keeps them apart, past an inner that's caught up with its outer.
        let outer = self.outer_cone.clamp(0.0, FRAC_PI_2);
        let kind = match self.kind {
            LightKind::Directional => gg_renderer::LightKind::Directional,
            LightKind::Point => gg_renderer::LightKind::Point,
            LightKind::Spot => gg_renderer::LightKind::Spot {
                inner: self.inner_cone.clamp(0.0, outer),
                outer,
            },
        };
        gg_renderer::Light {
            kind,
            position,
            direction,
            color: self.color.max(Vec3::ZERO) * self.intensity.max(0.0),
            range: self.range,
        }
    }
}

/// Puts in `lit`, in place of what it held, the lights of `world` as it draws `alpha` of the way on
/// from `past`: each from where its entity draws, shining along the entity's -Z as it faces then.
pub(crate) fn lights(world: &World, past: &Past, alpha: f32, lit: &mut Vec<gg_renderer::Light>) {
    lit.clear();
    let (Some(light), Some(transform)) =
        (world.component::<Light>(), world.component::<Transform>())
    else {
        return;
    };
    let Some(poses) = Poses::new(world, past, alpha) else {
        return;
    };
    for (index, table) in world.tables().iter().enumerate() {
        if !table.has(transform) {
            continue;
        }
        let lights = table.column::<Light>(light).unwrap_or_default();
        for (row, light) in lights.iter().enumerate() {
            let (position, rotation) = poses.pose(index, row);
            lit.push(light.lit(position, rotation * Vec3::NEG_Z));
        }
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Quat;

    use super::*;

    /// A light shines along its entity's -Z from where the entity draws, its parent carrying it,
    /// its color times its intensity; one without a transform lights nothing; and a spot's angles
    /// stay within a quarter turn, its inner within its outer.
    #[test]
    fn lights_shine_from_where_their_entities_draw() {
        let mut world = World::default();
        world.register::<Transform>().expect("registered");
        world.register::<Light>().expect("described");
        let turned = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(FRAC_PI_2),
            ..Transform::default()
        };
        let parent = world.spawn(turned);
        let spot = Light {
            kind: LightKind::Spot,
            color: Vec3::new(1.0, 0.5, 0.25),
            intensity: 8.0,
            inner_cone: 2.0,
            outer_cone: 3.0,
            ..Light::default()
        };
        let ahead = Transform {
            translation: Vec3::new(0.0, 0.0, -1.0),
            ..Transform::default()
        };
        let child = world.spawn((ahead, spot));
        world.set_parent(child, Some(parent)).expect("parented");
        world.spawn(Light::default());
        let mut lit = vec![];
        lights(&world, &Past::default(), 1.0, &mut lit);
        let [light] = lit[..] else {
            panic!("one light with a transform: {lit:?}");
        };
        // A quarter turn about Y takes -Z to -X.
        assert!(
            light.position.abs_diff_eq(Vec3::new(0.0, 2.0, 3.0), 1e-6),
            "{light:?}"
        );
        assert!(light.direction.abs_diff_eq(Vec3::NEG_X, 1e-6), "{light:?}");
        assert_eq!(light.color, Vec3::new(8.0, 4.0, 2.0));
        let gg_renderer::LightKind::Spot { inner, outer } = light.kind else {
            panic!("a spot: {light:?}");
        };
        assert_eq!((inner, outer), (FRAC_PI_2, FRAC_PI_2));
    }
}
