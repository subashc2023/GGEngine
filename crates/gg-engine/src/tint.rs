//! Tints: the component that colors what an entity and what it carries draw.

use gg_core::math::Vec4;

use crate::Component;

/// A color multiplying what an entity draws, linear RGBA, alpha included.
///
/// It multiplies the colors of the entity's [`Sprite`], [`Circle`] and [`Text`](crate::Text),
/// and the base color of its model draws ([`MeshDraw::with_material`]'s). It tints everything the
/// entity carries too: its children, a model root's nodes. Tints down the hierarchy multiply, as
/// transforms do. No tint is white. Particles and other shaders' meshes ignore it. A red cube, a
/// hit's flash or a team's color needs no asset of its own.
///
/// [`Sprite`]: crate::Sprite
/// [`Circle`]: crate::Circle
///
/// [`MeshDraw::with_material`]: crate::MeshDraw::with_material
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct Tint(#[reflect(color, default = Vec4::ONE)] pub Vec4);

impl Default for Tint {
    /// White, changing nothing.
    fn default() -> Self {
        Self(Vec4::ONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use crate::reflect::{Reflect, Shape, Show};

    /// Added by name it's white; its one field shows as a color and sets as any component's does.
    #[test]
    fn a_tint_is_white_shown_as_a_color_and_set_by_name() {
        let mut world = World::default();
        world.register::<Tint>().expect("described");
        let index = world.types().find("Tint").expect("described");
        let Shape::Struct { fields, tuple } = &world.types().info(index).shape else {
            panic!("a struct");
        };
        assert!(*tuple);
        assert_eq!(fields[0].meta.show, Show::Color);
        let entity = world.spawn(());
        world.add_default(entity, "Tint").expect("a default");
        assert_eq!(world.get::<Tint>(entity), Some(&Tint::default()));
        let red = Vec4::new(1.0, 0.0, 0.0, 0.5);
        let set = world.set_field(entity, "Tint", "0", &red.to_value());
        set.expect("a color");
        assert_eq!(world.get::<Tint>(entity), Some(&Tint(red)));
    }
}
