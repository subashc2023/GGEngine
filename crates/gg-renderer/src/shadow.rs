//! Shadows: a directional light's cascades of shadow maps, fitted to the view and steadied against
//! its moves, and their GPU form.
//!
//! The view's depth, from its near plane to the shadows' reach, splits into [`CASCADES`] slices,
//! the nearest the shortest, by the practical split scheme (Zhang, Sun, Xu and Lo, "Parallel-Split
//! Shadow Maps on Programmable GPUs", GPU Gems 3, ch. 10). Each cascade is the light-space box
//! round the smallest sphere holding its slice, as in Valient's stable cascades ("Stable Rendering
//! of Cascaded Shadow Maps", ShaderX6). The sphere's size depends on the projection alone, not the
//! view's heading, and its center moves in whole texels in the light's space. So texels stay put in
//! the world as the view moves, and edges don't shimmer. Casters between the light and a box's near
//! plane still cast, their depth clamped there.

use bytemuck::NoUninit;
use gg_core::math::camera::rh::view;
use gg_core::math::{Mat4, Vec3, Vec4, libm};

use crate::{Light, LightKind, Projection, Viewpoint};

/// Cascades per light, each its own shadow map.
pub(crate) const CASCADES: usize = 4;

/// Each map's texels across and down: 16 MiB of 32-bit depth.
pub(crate) const MAP_SIZE: u32 = 2048;

/// How far a map's depths sit behind their casters, in multiples of the caster's depth slope per
/// texel, as [`GraphicsDesc::depth_bias`] says. A texel of slope beyond the receivers' normal
/// offset covers surfaces that curve between interpolated normals.
///
/// [`GraphicsDesc::depth_bias`]: gg_gpu::GraphicsDesc::depth_bias
pub(crate) const DEPTH_BIAS: f32 = 1.0;

/// The practical split scheme's lambda: the logarithmic share of each split, the rest uniform. A
/// fully logarithmic split gives each slice the same texels per pixel, but its nearest slices are
/// so short that the far ones run out of texels.
const LOGARITHMIC: f32 = 0.75;

/// The texels inside its map's edges where each cascade keeps the points it shadows, which grow its
/// sphere: the filter's reach, 3, the normal offset, at most 3, and 2 to spare. Equals `MARGIN` in
/// `shaders/model.slang`.
const MARGIN: f32 = 8.0;

/// The share of their reach after which the shadows fade, so they end without an edge.
const FADE_FROM: f32 = 0.9;

/// The brightest shadow caster among `lights` (the first among equals): its index, its unit
/// direction and its shadow distance. A caster is directional, has some light, shines some way, and
/// has a finite, positive shadow distance.
pub(crate) fn caster(lights: &[Light]) -> Option<(usize, Vec3, f32)> {
    let luminance = |light: &Light| light.color.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    (lights.iter().enumerate())
        .filter_map(|(i, light)| {
            let distance = light.shadow_distance?;
            let direction = light.direction.try_normalize()?;
            let casts = light.kind == LightKind::Directional
                && light.color.cmpgt(Vec3::ZERO).any()
                && distance > 0.0
                && distance.is_finite();
            casts.then_some((i, direction, distance))
        })
        .reduce(|best, next| {
            if luminance(&lights[next.0]) > luminance(&lights[best.0]) {
                next
            } else {
                best
            }
        })
}

/// A light's shadow cascades, fitted to the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cascades {
    /// The light's index as shaders read the lights.
    light: u32,
    /// Turns with the light alone; its -Z is the way the light shines.
    light_from_world: Mat4,
    /// Each cascade's sphere, nearest first: its center in the light's space, X and Y in whole
    /// texels, and its radius.
    spheres: [Vec4; CASCADES],
    /// A point's distance ahead of the view: its dot product with XYZ, plus W.
    view_depth: Vec4,
    /// Distances ahead of the view where the shadows start to fade, and end.
    fade_from: f32,
    reach: f32,
}

impl Cascades {
    /// The cascades of [`caster`]'s light among `lights`, fitted to `viewpoint` at `aspect` (width
    /// over height). `None` if none casts, or the view sees nothing within the distance.
    pub(crate) fn fit(lights: &[Light], viewpoint: &Viewpoint, aspect: f32) -> Option<Self> {
        let (light, direction, distance) = caster(lights)?;
        // Any up but one along the light.
        let up = if direction.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let light_from_world = view::look_to_mat4(Vec3::ZERO, direction, up);
        // How far the view sees, and how far it reaches off its axis at a distance: in proportion
        // under perspective, the same everywhere without.
        let diagonal = (1.0 + aspect * aspect).sqrt();
        let (near, far, across, logarithmic) = match viewpoint.projection {
            Projection::Perspective { fov, near } => {
                let tan = libm::tanf(fov / 2.0) * diagonal;
                (near, distance, Across::Perspective(tan), LOGARITHMIC)
            }
            Projection::Orthographic { height, near, far } => {
                let (near, far) = (near.min(far), near.max(far));
                let half = height / 2.0 * diagonal;
                // Without perspective, every split gives each slice the same texels per pixel.
                (
                    near,
                    far.min(near + distance),
                    Across::Orthographic(half),
                    0.0,
                )
            }
        };
        if far <= near {
            return None;
        }
        let splits = splits(near, far, logarithmic);
        let forward = viewpoint.rotation * Vec3::NEG_Z;
        let spheres = std::array::from_fn(|i| {
            let (from, to) = (splits[i], splits[i + 1]);
            let (center, radius) = across.sphere(from, to);
            // Grown so the slice keeps MARGIN texels inside its map's edges.
            let radius = radius / (1.0 - 2.0 * MARGIN / MAP_SIZE as f32);
            let center =
                light_from_world.transform_point3(viewpoint.position.as_vec3() + forward * center);
            let texel = 2.0 * radius / MAP_SIZE as f32;
            let snapped = (center.truncate() / texel).round() * texel;
            snapped.extend(center.z).extend(radius)
        });
        Some(Self {
            light: crate::light::shader_index(lights, light),
            light_from_world,
            spheres,
            view_depth: forward.extend(-forward.dot(viewpoint.position.as_vec3())),
            fade_from: far - (far - near) * (1.0 - FADE_FROM),
            reach: far,
        })
    }

    /// The world to Vulkan clip space matrix cascade `i` is drawn with: its box, seen from the
    /// light, onto clip space, depth 1 at the light's side to 0 at the far, as the renderer's
    /// cameras project.
    pub(crate) fn clip_from_world(&self, i: usize) -> Mat4 {
        let sphere = self.spheres[i];
        let (center, scale) = (sphere.truncate(), 1.0 / sphere.w);
        // A light-space point `p` is at `n = (p - center) * scale` in the box, -1 to 1 each way.
        // Clip space's X is n's X, its Y (pointing down) n's -Y, and its depth n's Z / 2 + 1/2.
        let into_box = Mat4::from_cols(
            Vec4::new(scale, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -scale, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.5 * scale, 0.0),
            Vec4::new(
                -center.x * scale,
                center.y * scale,
                0.5 - 0.5 * center.z * scale,
                1.0,
            ),
        );
        into_box * self.light_from_world
    }

    /// The shadows' GPU form, the maps at slots `maps` of the bindless set.
    pub(crate) fn data(&self, maps: [u32; CASCADES]) -> ShadowData {
        ShadowData {
            light_from_world: self.light_from_world,
            cascades: self
                .spheres
                .map(|sphere| sphere.truncate().extend(1.0 / sphere.w)),
            maps,
            view_depth: self.view_depth,
            light: self.light,
            fade_from: self.fade_from,
            reach: self.reach,
            map_size: MAP_SIZE as f32,
        }
    }
}

/// The cascades' split distances from `near` to `far`: each a `logarithmic` blend of the same ratio
/// from the one before and the same distance from it.
fn splits(near: f32, far: f32, logarithmic: f32) -> [f32; CASCADES + 1] {
    std::array::from_fn(|i| {
        let t = i as f32 / CASCADES as f32;
        let uniform = near + (far - near) * t;
        if logarithmic == 0.0 {
            return uniform;
        }
        let log = near * libm::powf(far / near, t);
        logarithmic * log + (1.0 - logarithmic) * uniform
    })
}

/// How far the view reaches off its axis, to its corners, at a distance ahead.
#[derive(Clone, Copy)]
enum Across {
    /// This share of the distance.
    Perspective(f32),
    /// This much at any distance.
    Orthographic(f32),
}

impl Across {
    /// The smallest sphere holding the view's slice from `from` to `to` ahead: its center's
    /// distance ahead on the axis, and its radius.
    ///
    /// The slice's corners are `from` or `to` ahead, and `a` or `b` off the axis. A sphere centered
    /// `c` ahead holds the near ones within `(c - from)² + a²` and the far ones within `(to - c)² +
    /// b²`. These are equal at `c = (from + to) / 2 + (b² - a²) / (2 (to - from))`, where the
    /// sphere is smallest. If that's past the slice's end, the center is the end, where the far
    /// corners alone reach it.
    fn sphere(self, from: f32, to: f32) -> (f32, f32) {
        let (a, b) = match self {
            Self::Perspective(tan) => (from * tan, to * tan),
            Self::Orthographic(half) => (half, half),
        };
        let center = ((from + to) / 2.0 + (b * b - a * a) / (2.0 * (to - from))).clamp(from, to);
        let (ahead, behind) = (center - from, to - center);
        let near = ahead * ahead + a * a;
        let far = behind * behind + b * b;
        (center, near.max(far).sqrt())
    }
}

/// `Shadows` in `shaders/mesh.slang`, field for field. Fields shared with [`Cascades`] mean the
/// same.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct ShadowData {
    light_from_world: Mat4,
    /// Each cascade's light-space center and 1 over its radius, nearest first. A point `p` is at
    /// `(p - center) / radius` in its box, -1 to 1 each way.
    cascades: [Vec4; CASCADES],
    /// Each map's slot in the bindless set.
    maps: [u32; CASCADES],
    view_depth: Vec4,
    /// As [`Cascades::light`]; `u32::MAX` for none.
    light: u32,
    fade_from: f32,
    reach: f32,
    /// [`MAP_SIZE`], as a float.
    map_size: f32,
}

impl Default for ShadowData {
    /// No light's shadows.
    fn default() -> Self {
        Self {
            light_from_world: Mat4::IDENTITY,
            cascades: [Vec4::ZERO; CASCADES],
            maps: [0; CASCADES],
            view_depth: Vec4::ZERO,
            light: u32::MAX,
            fade_from: 0.0,
            reach: 0.0,
            map_size: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_3;

    use gg_core::math::Quat;

    use super::*;

    /// A directional light along `direction`, `brightness` times white, casting shadows to
    /// `distance`.
    fn sun(direction: Vec3, distance: Option<f32>, brightness: f32) -> Light {
        Light {
            kind: LightKind::Directional,
            position: Vec3::ZERO,
            direction,
            color: Vec3::splat(brightness),
            range: None,
            shadow_distance: distance,
        }
    }

    /// With perspective, seeing a third of a turn from bottom to top, from 0.1 ahead.
    fn looking(position: Vec3, rotation: Quat) -> Viewpoint {
        Viewpoint {
            position: position.as_dvec3(),
            rotation,
            projection: Projection::Perspective {
                fov: FRAC_PI_3,
                near: 0.1,
            },
        }
    }

    /// The first among equals. Not point lights, nor directional ones without a distance, with one
    /// of 0 or infinity, shining no way, or of no light.
    #[test]
    fn the_brightest_directional_light_with_a_distance_casts() {
        let view = looking(Vec3::ZERO, Quat::IDENTITY);
        let down = Vec3::NEG_Y;
        let lights = [
            Light {
                kind: LightKind::Point,
                ..sun(down, Some(50.0), 9.0)
            },
            sun(down, None, 8.0),
            sun(down, Some(0.0), 7.0),
            sun(down, Some(f32::INFINITY), 7.0),
            sun(Vec3::ZERO, Some(50.0), 6.0),
            sun(down, Some(50.0), 1.0),
            sun(down, Some(50.0), 2.0),
            sun(down, Some(50.0), 2.0),
        ];
        let fit = |lights| Cascades::fit(lights, &view, 1.5).map(|cascades| cascades.light);
        assert_eq!(
            fit(&lights),
            Some(6),
            "past the brighter one that shines no way"
        );
        assert_eq!(fit(&lights[..5]), None);
        assert_eq!(fit(&[sun(down, Some(50.0), 0.0)]), None, "one of no light");
    }

    /// Every corner inside the box by `MARGIN` texels, less the half texel the center snaps by; the
    /// size the same wherever the view is and however it turns; and the matrix taking the center to
    /// the middle of clip space.
    #[test]
    fn cascades_hold_their_slices_keep_their_size_and_stay_on_their_texels() {
        let lights = [sun(Vec3::new(0.3, -1.0, 0.2).normalize(), Some(80.0), 1.0)];
        let aspect = 16.0 / 9.0;
        let tan = libm::tanf(FRAC_PI_3 / 2.0);
        let splits = splits(0.1, 80.0, LOGARITHMIC);
        let inside = 1.0 - 2.0 * (MARGIN - 0.5) / MAP_SIZE as f32;
        let mut sizes = None;
        let turns = [
            Quat::IDENTITY,
            Quat::from_rotation_y(1.0) * Quat::from_rotation_x(-0.4),
            Quat::from_rotation_y(-2.5) * Quat::from_rotation_x(0.9) * Quat::from_rotation_z(0.3),
        ];
        for rotation in turns {
            for position in [
                Vec3::ZERO,
                Vec3::new(10.3, 2.0, -7.7),
                Vec3::new(-0.01, 0.0, 0.0),
            ] {
                let view = looking(position, rotation);
                let cascades = Cascades::fit(&lights, &view, aspect).expect("a light casts");
                let radii = cascades.spheres.map(|sphere| sphere.w);
                assert_eq!(
                    *sizes.get_or_insert(radii),
                    radii,
                    "{rotation} at {position}"
                );
                for (i, sphere) in cascades.spheres.iter().enumerate() {
                    let texel = 2.0 * sphere.w / MAP_SIZE as f32;
                    let texels = sphere.truncate().truncate() / texel;
                    let off = (texels - texels.round()).abs().max_element();
                    assert!(off < 1e-3, "cascade {i}'s center is {off} off its texels");
                    let clip = cascades.clip_from_world(i);
                    let center =
                        cascades.light_from_world.inverse() * sphere.truncate().extend(1.0);
                    let middle = clip.project_point3(center.truncate());
                    assert!(
                        middle.abs_diff_eq(Vec3::new(0.0, 0.0, 0.5), 1e-4),
                        "{middle}"
                    );
                    for ahead in [splits[i], splits[i + 1]] {
                        for [x, y] in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
                            let corner = Vec3::new(x * tan * aspect, y * tan, -1.0) * ahead;
                            let world = position + rotation * corner;
                            let at = clip.project_point3(world);
                            let reach = at.x.abs().max(at.y.abs()).max((2.0 * at.z - 1.0).abs());
                            assert!(reach <= inside, "cascade {i}'s corner at {at}");
                        }
                    }
                }
            }
        }
    }

    /// With perspective, in ever longer slices. Without, as far as the view sees, if nearer, in
    /// even slices. They fade out over the last tenth.
    #[test]
    fn shadows_reach_their_distance_or_the_end_of_the_view() {
        let near = splits(0.1, 80.0, LOGARITHMIC);
        let lengths: Vec<f32> = near.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(lengths.is_sorted(), "{near:?}");
        assert_eq!(near[0], 0.1);
        assert!((near[CASCADES] - 80.0).abs() < 1e-3, "{near:?}");
        let lights = [sun(Vec3::NEG_Y, Some(100.0), 1.0)];
        let flat = |near, far| Viewpoint {
            projection: Projection::Orthographic {
                height: 8.0,
                near,
                far,
            },
            ..looking(Vec3::ZERO, Quat::IDENTITY)
        };
        let reach = |view| {
            let cascades = Cascades::fit(&lights, &view, 1.0).expect("a light casts");
            (cascades.fade_from, cascades.reach)
        };
        for (view, expected) in [
            (flat(21.0, 1.0), (19.0, 21.0)),
            (flat(-10.0, 500.0), (80.0, 90.0)),
            (looking(Vec3::ZERO, Quat::IDENTITY), (90.01, 100.0)),
        ] {
            let (fade_from, reach) = reach(view);
            assert!(
                (fade_from - expected.0).abs() < 1e-4 && reach == expected.1,
                "{view:?}: from {fade_from} to {reach}"
            );
        }
        assert_eq!(splits(-10.0, 30.0, 0.0), [-10.0, 0.0, 10.0, 20.0, 30.0]);
    }
}
