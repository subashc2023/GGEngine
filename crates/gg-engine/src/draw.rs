//! The engine's own system, drawing the world's entities; and its components that `gg-renderer`
//! defines, described to reflection here, since they can't derive it there.

use std::ops::Range;

use gg_core::Error;
use gg_core::math::Vec2;
use gg_renderer::{Draws, MeshDraw, Sampler, Sprite};

use crate::reflect::{self, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value};
use crate::{Component, Transform, World};

impl Component for Sprite {}

/// Taken apart whole, so that a field `gg-renderer` adds fails to compile here until it's
/// described. Its texture is opaque, so that no value makes the index of a texture, which shaders
/// sample unchecked.
impl Reflect for Sprite {
    fn describe(types: &mut Types) -> Result<Type> {
        let field = |name: &'static str, ty, show, doc: &'static str| Field {
            name: name.into(),
            ty,
            meta: Meta {
                show,
                doc: doc.into(),
                ..Meta::default()
            },
        };
        let doc =
            "What a sprite shows: a rectangle of a texture, tinted, on a square a unit across.";
        types.add::<Self>("Sprite".into(), doc.into(), |types| {
            let fields = vec![
                field(
                    "color",
                    Type::Vec4,
                    Show::Color,
                    "In linear RGBA, alpha not multiplied in, which multiplies the texture's \
                     colors and alpha.",
                ),
                field(
                    "texture",
                    Type::Opaque,
                    Show::Plain,
                    "The texture it shows, which code sets.",
                ),
                field(
                    "rect",
                    <[Vec2; 2]>::describe(types)?,
                    Show::Plain,
                    "The part of the texture it shows, its top left then its bottom right, from \
                     (0, 0) at the texture's top left to (1, 1) at its bottom right.",
                ),
                field(
                    "sampler",
                    Sampler::describe(types)?,
                    Show::Plain,
                    "How the texture is sampled.",
                ),
            ];
            Ok(Shape::Struct {
                fields,
                tuple: false,
            })
        })
    }

    fn to_value(&self) -> Value {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        Value::Struct(vec![
            ("color".into(), color.to_value()),
            ("texture".into(), Value::Opaque),
            ("rect".into(), rect.to_value()),
            ("sampler".into(), sampler.to_value()),
        ])
    }

    fn from_value(value: &Value) -> Result<Self> {
        let [_, texture, ..] = reflect::fields(value, SPRITE_FIELDS)?;
        reflect::make_opaque(texture).map_err(|error| reflect::within(error, "texture"))
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        let Sprite {
            color,
            texture,
            rect,
            sampler,
        } = self;
        let [new_color, new_texture, new_rect, new_sampler] =
            reflect::fields(value, SPRITE_FIELDS)?;
        let at = |name| move |error: Error| reflect::within(error, name);
        Ok(Sprite {
            color: color.with_value(new_color).map_err(at("color"))?,
            texture: reflect::keep(texture, new_texture).map_err(at("texture"))?,
            rect: rect.with_value(new_rect).map_err(at("rect"))?,
            sampler: sampler.with_value(new_sampler).map_err(at("sampler"))?,
        })
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, index: u32) -> Option<&dyn Reflect> {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect> {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        match index {
            0 => Some(color),
            2 => Some(rect),
            3 => Some(sampler),
            _ => None,
        }
    }
}

/// A sprite's fields, in the order its description gives them.
const SPRITE_FIELDS: [&str; 4] = ["color", "texture", "rect", "sampler"];

/// Every sampler, in the order of their indices. A sampler `gg-renderer` adds fails to compile in
/// [`sampler_name`], and goes here too.
pub(crate) const SAMPLERS: [Sampler; 6] = [
    Sampler::LinearClamp,
    Sampler::NearestClamp,
    Sampler::LinearRepeat,
    Sampler::NearestRepeat,
    Sampler::LinearMirror,
    Sampler::NearestMirror,
];

fn sampler_name(sampler: Sampler) -> &'static str {
    match sampler {
        Sampler::LinearClamp => "LinearClamp",
        Sampler::NearestClamp => "NearestClamp",
        Sampler::LinearRepeat => "LinearRepeat",
        Sampler::NearestRepeat => "NearestRepeat",
        Sampler::LinearMirror => "LinearMirror",
        Sampler::NearestMirror => "NearestMirror",
    }
}

/// An enum of unit variants, one for each sampler.
impl Reflect for Sampler {
    fn describe(types: &mut Types) -> Result<Type> {
        let variants = SAMPLERS.map(|sampler| reflect::Variant {
            name: sampler_name(sampler).into(),
            fields: Vec::new(),
            tuple: false,
        });
        let doc = "How a texture is sampled: its filter, and what lies past its edges.";
        types.add::<Self>("Sampler".into(), doc.into(), |_| {
            Ok(Shape::Enum(variants.into()))
        })
    }

    fn to_value(&self) -> Value {
        Value::Variant(sampler_name(*self).into(), Vec::new())
    }

    fn from_value(value: &Value) -> Result<Self> {
        let (name, given) = reflect::variant(value)?;
        let Some(sampler) = SAMPLERS.into_iter().find(|&s| sampler_name(s) == name) else {
            return Err(reflect::no_variant(name, &SAMPLERS.map(sampler_name)));
        };
        reflect::pick(given, [])?;
        Ok(sampler)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        Self::from_value(value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }
}

impl Component for MeshDraw {}

/// Opaque: its mesh and its shader are indices private to `gg-renderer`, which no value may make.
impl Reflect for MeshDraw {
    fn describe(types: &mut Types) -> Result<Type> {
        let doc = "A mesh drawn with a shader and its parameters, which code sets.";
        types.add::<Self>("MeshDraw".into(), doc.into(), |_| Ok(Shape::Opaque))
    }

    fn to_value(&self) -> Value {
        Value::Opaque
    }

    fn from_value(value: &Value) -> Result<Self> {
        reflect::make_opaque(value)
    }

    fn with_value(&self, value: &Value) -> Result<Self> {
        reflect::keep(self, value)
    }

    fn set_value(&mut self, value: &Value) -> Result<()> {
        *self = self.with_value(value)?;
        Ok(())
    }

    fn part(&self, _: u32) -> Option<&dyn Reflect> {
        None
    }

    fn part_mut(&mut self, _: u32) -> Option<&mut dyn Reflect> {
        None
    }
}

/// Rows of one table at one z, drawn together.
pub(crate) struct Segment {
    z: f32,
    /// Which segment it was found as: tables in the order they were made, then a table's sprites
    /// before its meshes, then its rows in order.
    order: u32,
    kind: Kind,
    table: u32,
    rows: Range<u32>,
}

#[derive(Clone, Copy)]
enum Kind {
    Sprite,
    Mesh,
}

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`] or a
/// [`MeshDraw`], back to front by its translation's z; at one z, in the order queries find them,
/// and an entity with both draws its sprite under its mesh. `segments` is the scratch it sorts,
/// kept from frame to frame so that it allocates nothing once it's grown.
///
/// It sorts runs of consecutive rows at one z, so it's quick while each table's rows at one z stay
/// together. Spawning entities of a table at mixed z's, or despawning, which moves a table's last
/// row into the gap, splits those runs; at worst, as when sprites each have their own z, as sorting
/// by y gives them, it sorts every sprite every frame, until the renderer sorts by depth, with
/// Phase 4.
pub(crate) fn draw_world(world: &World, draws: &mut Draws, segments: &mut Vec<Segment>) {
    let _zone = gg_core::zone!();
    segments.clear();
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    let sprite = world.component::<Sprite>();
    let mesh = world.component::<MeshDraw>();
    for (table, rows) in world.tables().iter().enumerate() {
        let Some(transforms) = rows.column::<Transform>(transform) else {
            continue;
        };
        for (kind, id) in [(Kind::Sprite, sprite), (Kind::Mesh, mesh)] {
            if id.is_some_and(|id| rows.has(id)) {
                cut(segments, transforms, kind, table as u32);
            }
        }
    }
    segments.sort_unstable_by(|a, b| a.z.total_cmp(&b.z).then(a.order.cmp(&b.order)));
    let tables = world.tables();
    for segment in segments.iter() {
        let table = &tables[segment.table as usize];
        let rows = segment.rows.start as usize..segment.rows.end as usize;
        let transforms = &table
            .column::<Transform>(transform)
            .expect("it has transforms")[rows.clone()];
        match segment.kind {
            Kind::Sprite => {
                let sprites = sprite.and_then(|id| table.column::<Sprite>(id));
                let sprites = &sprites.expect("it has sprites")[rows];
                let both = transforms.iter().zip(sprites);
                draws.extend_sprites(both.map(|(transform, sprite)| (transform.matrix(), *sprite)));
            }
            Kind::Mesh => {
                let meshes = mesh.and_then(|id| table.column::<MeshDraw>(id));
                let meshes = &meshes.expect("it has meshes")[rows];
                for (transform, mesh) in transforms.iter().zip(meshes) {
                    draws.push_draw(transform.matrix(), mesh);
                }
            }
        }
    }
}

/// Adds to `segments` the runs of rows at one z among `transforms`, table `table`'s, of `kind`.
fn cut(segments: &mut Vec<Segment>, transforms: &[Transform], kind: Kind, table: u32) {
    let z = |row: usize| transforms[row].translation.z;
    let mut start = 0;
    for row in 1..=transforms.len() {
        if row == transforms.len() || z(row).total_cmp(&z(start)).is_ne() {
            segments.push(Segment {
                z: z(start),
                order: segments.len() as u32,
                kind,
                table,
                rows: start as u32..row as u32,
            });
            start = row;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gg_core::math::{Vec3, Vec4};
    use gg_renderer::{Blend, Meshes, Shaders};

    use super::*;

    #[derive(Clone, Copy, Component)]
    struct Marked;

    enum Expected {
        Sprite(Transform, Sprite),
        Mesh(Transform, MeshDraw),
    }

    /// Spawns sprites and meshes at several z's across tables, and checks that they're drawn back
    /// to front, and at one z in the order queries find them: tables in the order they were made,
    /// a table's sprites before its meshes, then its rows in order. Two tables' rows alternate
    /// between two z's, so that dozens of runs share each, enough for a sort that loses that
    /// order to show it.
    #[test]
    fn the_world_draws_back_to_front_by_z() {
        let mut shaders = Shaders::default();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let flat = shaders
            .add::<[f32; 3], Vec4>("flat color", &fixture, Blend::Opaque)
            .expect("the shader compiles");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = Meshes::default().add("triangle", &corners, &[0, 1, 2]);
        let triangle = triangle.expect("a mesh");
        // Each draw is told apart by its x and its color.
        let mut next = 0.0;
        let mut new = |z: f32| {
            next += 1.0;
            let at = Transform {
                translation: Vec3::new(next, 0.0, z),
                ..Transform::IDENTITY
            };
            let sprite = Sprite {
                color: Vec4::splat(next),
                ..Sprite::default()
            };
            (at, sprite, MeshDraw::new(triangle, flat, Vec4::splat(next)))
        };
        let mut world = World::default();
        // In the order of their tables, each table's sprites before its meshes, then its rows.
        let mut expected = Vec::new();
        let alternating = |row: usize| if row.is_multiple_of(2) { 0.5 } else { 0.6 };
        for z in [0.3, 0.1, 0.1].into_iter().chain((0..20).map(alternating)) {
            let (at, sprite, _) = new(z);
            world.spawn((at, sprite));
            expected.push(Expected::Sprite(at, sprite));
        }
        for z in (0..20).map(alternating).chain([0.1]) {
            let (at, sprite, _) = new(z);
            world.spawn((Marked, sprite, at));
            expected.push(Expected::Sprite(at, sprite));
        }
        for z in [0.2, 0.1] {
            let (at, _, mesh) = new(z);
            world.spawn((at, mesh));
            expected.push(Expected::Mesh(at, mesh));
        }
        let mut meshes = Vec::new();
        for z in [0.2, 0.0] {
            let (at, sprite, mesh) = new(z);
            world.spawn((at, sprite, mesh));
            expected.push(Expected::Sprite(at, sprite));
            meshes.push(Expected::Mesh(at, mesh));
        }
        expected.append(&mut meshes);
        // Nothing to draw, or nowhere to draw it.
        let (at, sprite, _) = new(0.0);
        world.spawn(at);
        world.spawn(sprite);

        // A stable sort, which keeps that order among each z's.
        expected.sort_by(|a, b| {
            let z = |expected: &Expected| match expected {
                Expected::Sprite(at, _) | Expected::Mesh(at, _) => at.translation.z,
            };
            z(a).total_cmp(&z(b))
        });
        let mut want = Draws::default();
        for expected in &expected {
            match expected {
                Expected::Sprite(at, sprite) => want.push_sprite(at.matrix(), *sprite),
                Expected::Mesh(at, mesh) => want.push_draw(at.matrix(), mesh),
            }
        }
        let mut draws = Draws::default();
        draw_world(&world, &mut draws, &mut Vec::new());
        assert_eq!(draws, want);
    }
}
