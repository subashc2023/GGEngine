//! Environments: the component that says how the game's view shows its world's light, whatever
//! viewpoint it sees from, and which of a world's environments the view renders with.

use gg_core::math::libm::exp2f;
use gg_renderer::Tonemapper;
use tracing::warn;

use crate::{Component, Entity, EntityId, World};

/// How the game's view shows its world's light, whatever viewpoint it sees from, the editor's and
/// the tool API's looks' too, so that they show what the game does: its exposure, and the
/// tonemapper that maps the view's colors, drawn in high dynamic range, into the range it shows. A
/// world has one, on an entity of its own; of several, the view renders with the one of the lowest
/// ID, warning (`engine.environment_tie`). A world without one isn't tonemapped, so that what's
/// in the range a view shows, as a 2D game's sprites, shows as it's drawn, and lit shading exposes
/// its light as the default environment's exposure does.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Environment {
    /// How much light the view takes in, as a camera's exposure value at ISO 100, EV100: lit
    /// shading multiplies light, in lux and candela, by 1 / (1.2 × 2^EV100), so that each step up
    /// halves what shows. 15, the default, suits a sunny day; an overcast one about 12, and a lit
    /// room about 7. What isn't lit, as sprites, text and unlit materials, shows its colors at any.
    #[reflect(default = 15.0)]
    pub exposure: f32,
    /// How the view's colors, drawn in high dynamic range, map into the range it shows: Khronos PBR
    /// Neutral by default.
    pub tonemapper: Tonemapper,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            exposure: 15.0,
            tonemapper: Tonemapper::default(),
        }
    }
}

impl Environment {
    /// What lit shading multiplies light by at its exposure: 1 over the largest luminance that
    /// doesn't saturate a camera of that exposure value, 1.2 × 2^EV100, as Frostbite's and
    /// Filament's do (Lagarde and de Rousiers, "Moving Frostbite to Physically Based Rendering",
    /// 2014).
    pub fn light_scale(&self) -> f32 {
        1.0 / (1.2 * exp2f(self.exposure))
    }
}

/// Environments that several entities have at once: the one the view renders with, of the lowest
/// ID, and how many there are.
pub(crate) type Tie = (EntityId, usize);

/// The environment the view renders `world` with: that of the lowest ID among its entities with an
/// [`Environment`], if any. Several warn (`engine.environment_tie`) as they come to be and as they
/// change, which `warned` keeps track of, rather than every frame.
pub(crate) fn seen(world: &World, warned: &mut Option<Tie>) -> Option<Environment> {
    let mut lowest: Option<(EntityId, Environment)> = None;
    let mut count = 0;
    for (entity, &environment) in world.query::<(Entity, &Environment)>() {
        let id = world.id(entity).expect("a query's entities are alive");
        count += 1;
        if lowest.is_none_or(|(low, _)| id < low) {
            lowest = Some((id, environment));
        }
    }
    let tie = lowest.filter(|_| count > 1).map(|(seen, _)| (seen, count));
    if let Some((seen, count)) = tie
        && tie != *warned
    {
        warn!(
            code = "engine.environment_tie",
            seen = %seen,
            count,
            "{count} entities have an Environment: the view renders with the one of the lowest \
             ID, {seen}"
        );
    }
    *warned = tie;
    lowest.map(|(_, environment)| environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_of_exposure_halves_the_light_that_shows() {
        let at = |exposure| Environment {
            exposure,
            ..Environment::default()
        };
        assert_eq!(at(0.0).light_scale(), 1.0 / 1.2);
        assert_eq!(at(15.0).light_scale() * 2.0, at(14.0).light_scale());
        // The sun's 100,000 lux on a white surface, which reflects 1/π of it a steradian, comes
        // to about 0.8 on a sunny day's exposure.
        let white = 100_000.0 / std::f32::consts::PI * at(15.0).light_scale();
        assert!((0.8..0.82).contains(&white), "{white}");
    }

    /// Of several environments the view renders with the one of the lowest ID, which a tie keeps
    /// warned of; with none, there's none.
    #[test]
    fn the_lowest_ids_environment_is_seen() {
        let mut world = World::default();
        world.register::<Environment>().expect("described");
        let mut warned = None;
        assert_eq!(seen(&world, &mut warned), None);
        let dim = Environment {
            exposure: 7.0,
            tonemapper: Tonemapper::Agx,
        };
        let first = world.spawn(dim);
        assert_eq!(seen(&world, &mut warned), Some(dim));
        assert_eq!(warned, None);
        let second = world.spawn(Environment::default());
        let [first, second] = [first, second].map(|entity| world.id(entity).expect("alive"));
        let lowest = first.min(second);
        let expected = if lowest == first {
            dim
        } else {
            Environment::default()
        };
        assert_eq!(seen(&world, &mut warned), Some(expected));
        assert_eq!(warned, Some((lowest, 2)));
    }
}
