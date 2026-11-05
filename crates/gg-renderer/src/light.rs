//! Lights: what lights the frame's lit shading, handed to the renderer each frame as plain data, as
//! KHR_lights_punctual describes them, and what shaders read of them through the frame's data.

use bytemuck::NoUninit;
use gg_core::math::{Vec3, libm};

/// How a light shines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// From far away, all one way, as the sun's: its intensity in lux, what reaches a surface that
    /// faces it.
    Directional,
    /// From a point, every way alike: its intensity in candela, falling off with the square of the
    /// distance.
    Point,
    /// From a point, in a cone round its direction: wholly within `inner` of it, fading to nothing
    /// at `outer`, each an angle from its direction in radians, `outer` the larger; its intensity
    /// in candela, as a point light's.
    Spot { inner: f32, outer: f32 },
}

/// A light the frame shades with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    /// Where it shines from, in the world; ignored for a directional light.
    pub position: Vec3,
    /// The way it shines, a unit vector in the world; ignored for a point light.
    pub direction: Vec3,
    /// Its color, in linear RGB, times its intensity, in lux or candela as its kind says.
    pub color: Vec3,
    /// How far it reaches, past which it lights nothing, its falloff easing to 0 there, as
    /// KHR_lights_punctual recommends; `None` for as far as its falloff takes it. Ignored for a
    /// directional light.
    pub range: Option<f32>,
    /// How far ahead of the view a directional light casts shadows, in world units, if it casts
    /// them: the brightest such light of the frame, the first of those as bright, casts them, in
    /// cascades fitted to the view, fading out over the last tenth of the distance. `None` for none.
    /// Ignored for point and spot lights, which cast none.
    pub shadow_distance: Option<f32>,
}

/// A light as shaders read it: `Light` in `shaders/mesh.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct GpuLight {
    position: Vec3,
    /// 0 for directional, 1 for point, 2 for spot.
    kind: u32,
    direction: Vec3,
    /// 1 over its range, or 0 for none.
    inverse_range: f32,
    color: Vec3,
    /// What a spot's cosine from its direction is scaled by, then offset by, before it's clamped
    /// to 0 to 1 and squared, as KHR_lights_punctual's reference does it: 0 and 1 for the rest,
    /// which light every way.
    angle_scale: f32,
    angle_offset: f32,
}

impl GpuLight {
    pub(crate) fn new(light: &Light) -> Self {
        let (kind, angle_scale, angle_offset) = match light.kind {
            LightKind::Directional => (0, 0.0, 1.0),
            LightKind::Point => (1, 0.0, 1.0),
            LightKind::Spot { inner, outer } => {
                let (inner, outer) = (libm::cosf(inner), libm::cosf(outer));
                // Kept off 0 where the cone's edges meet, as the reference does.
                let scale = 1.0 / (inner - outer).max(0.001);
                (2, scale, -outer * scale)
            }
        };
        let inverse_range = light
            .range
            .filter(|&range| range > 0.0)
            .map_or(0.0, |range| 1.0 / range);
        Self {
            position: light.position,
            kind,
            direction: light.direction,
            inverse_range,
            color: light.color,
            angle_scale,
            angle_offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spot's cone fades from its inner angle to its outer, as KHR_lights_punctual's reference
    /// has it: wholly lit at the inner, unlit at the outer; a light without a range reaches as far
    /// as its falloff takes it.
    #[test]
    fn a_spots_cone_fades_between_its_angles() {
        let spot = Light {
            kind: LightKind::Spot {
                inner: 0.2,
                outer: 0.5,
            },
            position: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            color: Vec3::ONE,
            range: Some(4.0),
            shadow_distance: None,
        };
        let gpu = GpuLight::new(&spot);
        let fade =
            |angle: f32| (libm::cosf(angle) * gpu.angle_scale + gpu.angle_offset).clamp(0.0, 1.0);
        assert!((fade(0.2) - 1.0).abs() < 1e-5, "{}", fade(0.2));
        assert!(fade(0.5).abs() < 1e-5, "{}", fade(0.5));
        assert!(fade(0.35) > 0.0 && fade(0.35) < 1.0);
        assert_eq!((gpu.kind, gpu.inverse_range), (2, 0.25));
        let point = GpuLight::new(&Light {
            kind: LightKind::Point,
            range: None,
            ..spot
        });
        assert_eq!(
            (
                point.kind,
                point.inverse_range,
                point.angle_scale,
                point.angle_offset
            ),
            (1, 0.0, 0.0, 1.0)
        );
    }
}
