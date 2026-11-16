//! Lights: the component that lights a world's models, and the lights a frame's shading takes from
//! the world.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use gg_core::math::{DVec3, Vec3};

use crate::draw::Poses;
use crate::interpolation::Past;
use crate::placed::Placed;
use crate::{Component, Parent, Reflect, Transform, World};

/// How a [`Light`] shines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum LightKind {
    /// Like the sun: all one way, along the entity's -Z. Lux, what reaches a surface facing it,
    /// wherever it is.
    #[default]
    Directional,
    /// From the entity, every way alike. Candela, falling with the square of distance.
    Point,
    /// From the entity in a cone round -Z: full inside the inner angle, fading to nothing at the
    /// outer. Candela, as for a point light.
    Spot,
}

/// A KHR_lights_punctual light. It lights the world's models from where its entity draws
/// (ancestors carry it), facing as the entity does. Materials shade by every light each frame, as
/// glTF's metallic-roughness model says, scaled by the environment's exposure. Without lights,
/// models draw unlit in base colors. An entity with a light but no [`Transform`] lights nothing.
/// A model's nodes have the lights its file gives them.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Light {
    /// How it shines: directional, from a point, or as a spot.
    pub kind: LightKind,
    /// Its color, in linear RGB.
    #[reflect(color, default = Vec3::ONE)]
    pub color: Vec3,
    /// Lux for a directional light, candela for the rest. 100,000 by default: a sunny day, which
    /// an [`Environment`]'s default exposure suits.
    ///
    /// [`Environment`]: crate::Environment
    #[reflect(min = 0.0, default = 100_000.0)]
    pub intensity: f32,
    /// Reach of a point or spot light in world units; its falloff eases to zero there. `None`
    /// is as far as the falloff goes.
    pub range: Option<f32>,
    /// Angle from -Z within which a spot is at full strength, up to the outer cone's (larger
    /// counts as that).
    #[reflect(degrees, min = 0.0, default = 0.0)]
    pub inner_cone: f32,
    /// Angle from -Z where a spot fades to nothing, at most a quarter turn (larger counts as
    /// that).
    #[reflect(degrees, min = 0.0, default = FRAC_PI_4)]
    pub outer_cone: f32,
    /// Whether it casts shadows. Only a world's brightest directional light does; point and spot
    /// lights cast none yet. Opaque things between it and a model shade the model, out to the
    /// shadow distance.
    #[reflect(default = true)]
    pub shadows: bool,
    /// Shadow reach ahead of the camera in world units, fading over the last tenth. Farther is
    /// less sharp: the shadow maps spread wider.
    #[reflect(min = 0.0, default = 100.0)]
    pub shadow_distance: f32,
}

impl Default for Light {
    /// A white directional light as bright as the sun on a sunny day, casting shadows 100 units
    /// ahead of the camera.
    fn default() -> Self {
        Self {
            kind: LightKind::Directional,
            color: Vec3::ONE,
            intensity: 100_000.0,
            range: None,
            inner_cone: 0.0,
            outer_cone: FRAC_PI_4,
            shadows: true,
            shadow_distance: 100.0,
        }
    }
}

impl Light {
    /// It as the renderer shades with it, at `position` from the frame's origin, along `direction`.
    fn lit(&self, position: Vec3, direction: Vec3) -> gg_renderer::Light {
        // KHR_lights_punctual keeps a spot's angles to 0 <= inner < outer <= a quarter turn; the
        // renderer also keeps them apart once inner reaches outer.
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
            shadow_distance: self.shadows.then_some(self.shadow_distance),
        }
    }
}

/// The world's lights as frames draw them, kept between frames and found again only when their
/// inputs change, so a frame with unchanged lights costs nothing.
#[derive(Default)]
pub(crate) struct Lit {
    lights: Vec<gg_renderer::Light>,
    /// The world they were found in by serial, its version and change tick then, and the frame's
    /// origin.
    seen: Option<(u64, u64, u64, DVec3)>,
    /// Whether a light was moving between ticks then; the next frame finds them again.
    moving: bool,
}

impl Lit {
    /// The lights as last found.
    pub(crate) fn lights(&self) -> &[gg_renderer::Light] {
        &self.lights
    }

    /// Finds the lights of `world` as [`lights`] does, for `alpha`, `past`, `placed` and `origin`;
    /// `started` is the latest tick's change tick. It finds them again if the world is another or
    /// has changed rows, a light, its transform or its parent changed, `placed` placed one again,
    /// one is moving between ticks or was last frame, or `origin` moved. Else they stay.
    pub(crate) fn find(
        &mut self,
        world: &World,
        past: &Past,
        placed: &Placed,
        (alpha, origin): (f32, DVec3),
        started: u64,
    ) {
        let _zone = gg_core::zone!();
        let (serial, version) = world.version();
        let seen = (self.seen).replace((serial, version, world.change_tick(), origin));
        let (Some(light), Some(transform)) =
            (world.component::<Light>(), world.component::<Transform>())
        else {
            self.lights.clear();
            return;
        };
        let parent = world.component::<Parent>();
        let lit = |entity| world.get::<Light>(entity).is_some();
        let tables = || {
            let tables = world.tables().iter().enumerate();
            tables.filter(|(_, table)| table.has(light) && table.has(transform))
        };
        let moved = |(index, table): (usize, &crate::ecs::Table)| {
            let marks = table.changed(transform).expect("it has transforms");
            let blends = alpha < 1.0 && past.from(world, index).is_some();
            !past.instead_of(index).is_empty() || blends && marks.since(started.saturating_sub(1))
        };
        let moving = tables().any(moved) || placed.moving().iter().any(|&entity| lit(entity));
        let same = seen.is_some_and(|(was, then, tick, from)| {
            let changed = |table: &crate::ecs::Table| {
                let ids = [Some(light), Some(transform), parent].into_iter().flatten();
                ids.filter_map(|id| table.changed(id))
                    .any(|marks| marks.since(tick))
            };
            (was, then, from) == (serial, version, origin)
                && !self.moving
                && !moving
                && !tables().any(|(_, table)| changed(table))
                && !placed.changed().iter().any(|&entity| lit(entity))
        });
        self.moving = moving;
        if !same {
            lights(world, past, placed, (alpha, origin), &mut self.lights);
        }
    }
}

/// Replaces `lit` with the lights of `world` drawn `alpha` of the way on from `past`, placed as
/// `placed` has it. Each is where its entity draws, from the frame's `origin`, shining along the
/// entity's -Z as it then faces.
pub(crate) fn lights(
    world: &World,
    past: &Past,
    placed: &Placed,
    (alpha, origin): (f32, DVec3),
    lit: &mut Vec<gg_renderer::Light>,
) {
    lit.clear();
    let (Some(light), Some(transform)) =
        (world.component::<Light>(), world.component::<Transform>())
    else {
        return;
    };
    let Some(poses) = Poses::new(world, past, placed, alpha, origin) else {
        return;
    };
    for (index, table) in world.tables().iter().enumerate() {
        if !table.has(transform) {
            continue;
        }
        let lights = table.column::<Light>(light).unwrap_or_default();
        for (row, light) in lights.iter().enumerate() {
            let (position, rotation) = poses.pose(index, row);
            lit.push(light.lit((position - origin).as_vec3(), rotation * Vec3::NEG_Z));
        }
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Quat;

    use super::*;
    use crate::Entity;

    /// A light shines along its entity's -Z from where the entity draws, its parent carrying it,
    /// at color times intensity. One without a transform lights nothing. A spot's angles stay
    /// within a quarter turn, inner within outer.
    #[test]
    fn lights_shine_from_where_their_entities_draw() {
        let mut world = World::default();
        world.register::<Transform>().expect("registered");
        world.register::<Light>().expect("described");
        let turned = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
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
            translation: DVec3::new(0.0, 0.0, -1.0),
            ..Transform::default()
        };
        let child = world.spawn((ahead, spot));
        world.set_parent(child, Some(parent)).expect("parented");
        world.spawn(Light::default());
        let mut lit = vec![];
        lights(
            &world,
            &Past::default(),
            &Placed::of(&world, &Past::default(), 1.0),
            (1.0, DVec3::ZERO),
            &mut lit,
        );
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

    /// A light that casts shadows hands the renderer its shadow distance; one that doesn't, none.
    #[test]
    fn a_light_casts_shadows_as_far_as_its_distance_unless_it_casts_none() {
        let sun = Light {
            shadow_distance: 30.0,
            ..Light::default()
        };
        let lit = |light: Light| light.lit(Vec3::ZERO, Vec3::NEG_Z).shadow_distance;
        assert_eq!(lit(sun), Some(30.0));
        let shadowless = Light {
            shadows: false,
            ..sun
        };
        assert_eq!(lit(shadowless), None);
    }

    /// Kept lights equal those found afresh, across frames that change nothing, move a carrier,
    /// change a light, move one between ticks and after, and remove one.
    #[test]
    fn kept_lights_are_found_again_as_they_change() {
        let mut world = World::default();
        let point = Light {
            kind: LightKind::Point,
            ..Light::default()
        };
        let lone = world.spawn((Transform::default(), point));
        let parent = world.spawn(Transform::default());
        let carried = world.spawn((Transform::default(), point));
        world.set_parent(carried, Some(parent)).expect("no cycle");
        world.spawn(Transform::default());
        let (mut past, mut placed, mut kept) = (Past::default(), Placed::default(), Lit::default());
        let mut started = 0;
        type Change = fn(&mut World, [Entity; 3]);
        let frames: [(&str, bool, f32, Change); 7] = [
            ("the first", false, 1.0, |_, _| {}),
            ("nothing", false, 1.0, |_, _| {}),
            ("a carrier moved", false, 1.0, |world, [_, parent, _]| {
                world
                    .get_mut::<Transform>(parent)
                    .expect("alive")
                    .translation
                    .x = 2.0;
            }),
            ("a light changed", false, 1.0, |world, [lone, ..]| {
                world.get_mut::<Light>(lone).expect("alive").intensity = 3.0;
            }),
            ("one moved in a tick", true, 0.5, |world, [lone, ..]| {
                world
                    .get_mut::<Transform>(lone)
                    .expect("alive")
                    .translation
                    .y = 4.0;
            }),
            (
                "the frame after the tick that moved it",
                true,
                0.5,
                |_, _| {},
            ),
            ("one taken away", false, 1.0, |world, [lone, ..]| {
                world.despawn(lone).expect("alive");
            }),
        ];
        for (what, ticks, alpha, change) in frames {
            if ticks {
                past.capture(&mut world);
                started = world.change_tick();
            }
            change(&mut world, [lone, parent, carried]);
            past.resolve(&world);
            placed.place(&world, None);
            placed.draw(&world, &past, alpha, started, None);
            kept.find(&world, &past, &placed, (alpha, DVec3::ZERO), started);
            let mut fresh = Vec::new();
            lights(&world, &past, &placed, (alpha, DVec3::ZERO), &mut fresh);
            assert_eq!(kept.lights(), fresh, "after {what}");
            world.advance();
        }
    }
}
