//! Shadows: the cascades a directional light's shadows are drawn into, each a shadow map, fitted to
//! the view and steadied against its moves, and what shaders read of them through the frame's data.
//!
//! The view's depth, from its near plane to how far the shadows reach, splits into [`CASCADES`]
//! slices, the nearest the shortest, as the practical split scheme has it (Zhang, Sun, Xu and Lo,
//! "Parallel-Split Shadow Maps on Programmable GPUs", GPU Gems 3, ch. 10). Each slice's cascade is
//! the box round the smallest sphere that holds the slice, seen from the light, as Valient's stable
//! cascades are ("Stable Rendering of Cascaded Shadow Maps", ShaderX6): the sphere's size depends
//! on the view's projection alone, not on which way it looks, and its center moves in whole texels
//! of its map, in the light's space, so that as the view turns and moves, each map's texels stay
//! where they are in the world, and shadows' edges don't shimmer. Casters between the light and a
//! box's near plane cast from it, their depth clamped there.

use bytemuck::NoUninit;
use gg_core::math::camera::rh::view;
use gg_core::math::{Mat4, Vec3, Vec4, libm};

use crate::{Light, LightKind, Projection, Viewpoint};

/// How many cascades a light's shadows are in, each a shadow map of its own.
pub(crate) const CASCADES: usize = 4;

/// The texels across and down each cascade's shadow map: 16 MiB of 32-bit depth each.
pub(crate) const MAP_SIZE: u32 = 2048;

/// How far a shadow map's depths go back from the casters they're of, in multiples of the most a
/// caster's depth changes across a texel, as [`GraphicsDesc::depth_bias`] says: a texel's worth of
/// the caster's own slope, past what the receivers' normal offset covers, for surfaces that curve
/// between the normals shading interpolates.
///
/// [`GraphicsDesc::depth_bias`]: gg_gpu::GraphicsDesc::depth_bias
pub(crate) const DEPTH_BIAS: f32 = 1.0;

/// How much of each split is logarithmic, where the rest is uniform: the practical split scheme's
/// lambda. A logarithmic split gives each slice the same texels per pixel, but its nearest slices
/// are so short that the farther ones run out of texels; this keeps most of its share.
const LOGARITHMIC: f32 = 0.75;

/// The texels inside its map's edges that each cascade keeps the points it shadows to, which its
/// sphere grows by: the filter's reach round a point, 3, and how far the normal offset moves it,
/// at most 3, with 2 to spare. `MARGIN` in `shaders/model.slang` is the same.
const MARGIN: f32 = 8.0;

/// How far through their reach the shadows start to fade, so that they end without an edge.
const FADE_FROM: f32 = 0.9;

/// The cascades of a light's shadows, fitted to the view: where each is in the light's space, and
/// how far ahead of the view they reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cascades {
    /// The light's index among the frame's, as shaders read them.
    light: u32,
    /// From the world to the light's space, which turns with the light alone: its -Z the way the
    /// light shines.
    light_from_world: Mat4,
    /// Each cascade's sphere: its center in the light's space, its X and Y in whole texels of its
    /// map, and its radius, from the nearest the view out.
    spheres: [Vec4; CASCADES],
    /// How far a point is ahead of the view: its dot product with XYZ, plus W.
    view_depth: Vec4,
    /// How far ahead of the view the shadows start to fade, and how far they reach.
    fade_from: f32,
    reach: f32,
}

impl Cascades {
    /// The cascades of the brightest of `lights` that casts shadows, a directional light of some
    /// light that shines some way, with a shadow distance, the first of those as bright, fitted to
    /// what `viewpoint` sees of a view `aspect` times as wide as it's tall; `None` if none casts
    /// them, or the view sees nothing within its shadow distance.
    pub(crate) fn fit(lights: &[Light], viewpoint: &Viewpoint, aspect: f32) -> Option<Self> {
        let luminance = |light: &Light| light.color.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let (light, direction, distance) = (lights.iter().enumerate())
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
            })?;
        // Any up but one along the light.
        let up = if direction.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let light_from_world = view::look_to_mat4(Vec3::ZERO, direction, up);
        // How far the view sees, ahead of it, and how far its view reaches off its axis at a
        // distance ahead: in proportion under perspective, the same everywhere without.
        let diagonal = (1.0 + aspect * aspect).sqrt();
        let (near, far, across, logarithmic) = match viewpoint.projection {
            Projection::Perspective { fov, near } => {
                let tan = libm::tanf(fov / 2.0) * diagonal;
                (near, distance, Across::Perspective(tan), LOGARITHMIC)
            }
            Projection::Orthographic { height, near, far } => {
                let (near, far) = (near.min(far), near.max(far));
                let half = height / 2.0 * diagonal;
                // Without perspective, every slice gets as many texels a pixel at any split.
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
            // Grown so that the slice keeps MARGIN texels inside its map's edges.
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

    /// The matrix from the world to Vulkan's clip space that cascade `i` is drawn with: its box,
    /// as seen from the light, mapped onto clip space, and its depth onto 1 at the near side, the
    /// light's, to 0 at the far, as the renderer's cameras project it.
    pub(crate) fn clip_from_world(&self, i: usize) -> Mat4 {
        let sphere = self.spheres[i];
        let (center, scale) = (sphere.truncate(), 1.0 / sphere.w);
        // A point `p` in the light's space is at `n = (p - center) * scale` in the box, -1 to 1
        // each way: clip space's X is n's X, its Y, which points down, n's -Y, and its depth
        // half n's Z, plus a half.
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

    /// What shaders read of the shadows, their maps' slots in the bindless set `maps`.
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

/// Where the view's depth from `near` to `far` ahead of it splits into the cascades' slices, from
/// `near` to `far`: as much of each split as `logarithmic` says at the same ratio from the one
/// before, and the rest the same distance on from it.
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

/// How far the view reaches off its axis at a distance ahead of it, to its corners.
#[derive(Clone, Copy)]
enum Across {
    /// This much of the distance.
    Perspective(f32),
    /// This much, however far.
    Orthographic(f32),
}

impl Across {
    /// The smallest sphere that holds the slice of the view from `from` to `to` ahead of it: how
    /// far ahead its center is, on the view's axis, and its radius.
    ///
    /// The slice's corners are each `from` or `to` ahead, and `a` or `b` off the axis, as far as
    /// the view reaches there. A sphere centered `c` ahead holds the near ones within
    /// `(c - from)² + a²` and the far ones within `(to - c)² + b²`, which are the same where
    /// `c = (from + to) / 2 + (b² - a²) / (2 (to - from))`: the sphere is smallest there, or, past
    /// the slice's end, at its end, where the corners there alone reach it.
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

/// What shaders read of a frame's shadows, through its data: `Shadows` in `shaders/mesh.slang`,
/// field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct ShadowData {
    /// From the world to the light's space, its -Z the way the light shines.
    light_from_world: Mat4,
    /// Each cascade's center in the light's space, and 1 over its radius: a point `p` there is at
    /// `(p - center) / radius` in its box, -1 to 1 each way, from the nearest the view out.
    cascades: [Vec4; CASCADES],
    /// Each cascade's shadow map's slot in the bindless set.
    maps: [u32; CASCADES],
    /// How far a point is ahead of the view: its dot product with XYZ, plus W.
    view_depth: Vec4,
    /// The light that casts them, by its index among the frame's, as shaders read them; `u32::MAX`
    /// for none.
    light: u32,
    /// How far ahead of the view they start to fade, and how far they reach.
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

    /// A directional light along `direction`, `brightness` times white, casting shadows as far as
    /// `distance`, if given.
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

    /// The brightest directional light with a shadow distance casts, the first of those as bright;
    /// point lights don't, nor directional ones without a distance, or with one of none or past
    /// any, or shining no way, or of no light.
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

    /// Each cascade holds its slice of the view's depth, every corner of it inside its box, as
    /// many texels in from its map's edges as `MARGIN` says, but for the half a texel that its
    /// center moves to a texel's corner; each is the same size wherever the view is and however it
    /// turns, its center in whole texels of its map; and the matrix it's drawn with takes its
    /// center to the middle of clip space.
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

    /// With perspective, the shadows reach their distance, the slices growing ever longer; without,
    /// as far as the view sees, if that's nearer, in slices as long as each other; and they fade out
    /// over the last tenth of their reach.
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
