//! Lights, handed to the renderer each frame as plain data, after KHR_lights_punctual, and their
//! GPU form.

use bytemuck::NoUninit;
use gg_core::math::{Vec3, libm};

/// How a light shines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// From far away, all one way, as the sun. Intensity in lux, as reaches a surface facing it.
    Directional,
    /// From a point, every way alike. Intensity in candela, falling off with the distance squared.
    Point,
    /// From a point, in a cone round its direction: fully lit within `inner`, fading to nothing at
    /// `outer`. Both are angles from the direction in radians, `outer` the larger. Intensity in
    /// candela.
    Spot { inner: f32, outer: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    /// In the world. Ignored for a directional light.
    pub position: Vec3,
    /// A unit vector in the world. Ignored for a point light.
    pub direction: Vec3,
    /// Linear RGB times intensity, in lux or candela as the kind says.
    pub color: Vec3,
    /// How far it reaches, its falloff easing to 0 there, as KHR_lights_punctual recommends. `None`
    /// reaches as far as the falloff takes it. Ignored for a directional light.
    pub range: Option<f32>,
    /// How far ahead of the view a directional light casts shadows, in world units. `None` casts
    /// none. Only the frame's brightest such light (the first, among equals) casts them, in
    /// cascades fitted to the view, fading out over the last tenth. Ignored for point and spot
    /// lights.
    pub shadow_distance: Option<f32>,
}

impl Light {
    /// Whether it lights everywhere, so no cluster bins it: a directional light, or one whose range
    /// isn't a finite number above 0.
    pub(crate) fn everywhere(&self) -> bool {
        self.kind == LightKind::Directional
            || !self
                .range
                .is_some_and(|range| range > 0.0 && range.is_finite())
    }
}

/// The index of `lights[i]` as shaders read them: those [`Light::everywhere`] first, then the rest,
/// each in order.
///
/// # Panics
///
/// - `i` is past the lights.
/// - There are 2^32 lights or more.
pub(crate) fn shader_index(lights: &[Light], i: usize) -> u32 {
    let everywhere = lights[i].everywhere();
    let alike = (lights[..i].iter())
        .filter(|light| light.everywhere() == everywhere)
        .count();
    let before = if everywhere {
        0
    } else {
        lights.iter().filter(|light| light.everywhere()).count()
    };
    u32::try_from(before + alike).expect("fewer than 2^32 lights")
}

/// `Light` in `shaders/mesh.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct GpuLight {
    position: Vec3,
    /// 0 for directional, 1 for point, 2 for spot.
    kind: u32,
    direction: Vec3,
    /// 1 over the range, or 0 for none.
    inverse_range: f32,
    color: Vec3,
    /// A spot's cosine from its direction is scaled, then offset, then clamped to 0..1 and squared,
    /// as KHR_lights_punctual's reference does. 0 and 1 for other kinds.
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
        // An infinite range gives 0, as none.
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

    /// Per KHR_lights_punctual's reference. A light without a range reaches as far as its falloff.
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

    /// Directional, or with a range of `None`, 0 or infinity; then the rest, each in order.
    #[test]
    fn lights_that_light_everywhere_come_first() {
        let point = |range| Light {
            kind: LightKind::Point,
            position: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            color: Vec3::ONE,
            range,
            shadow_distance: None,
        };
        let lights = [
            point(Some(2.0)),
            Light {
                kind: LightKind::Directional,
                ..point(Some(2.0))
            },
            point(Some(3.0)),
            point(None),
            point(Some(0.0)),
            point(Some(f32::INFINITY)),
            Light {
                kind: LightKind::Spot {
                    inner: 0.1,
                    outer: 0.2,
                },
                ..point(Some(1.0))
            },
        ];
        let order: Vec<u32> = (0..lights.len())
            .map(|i| shader_index(&lights, i))
            .collect();
        assert_eq!(order, [4, 0, 5, 1, 2, 3, 6]);
    }
}
