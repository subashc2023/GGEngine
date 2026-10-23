//! The engine's own system, drawing the world's entities; and its components that `gg-renderer`
//! defines, described to reflection here, since they can't derive it there.

use std::ops::Range;

use gg_core::Error;
use gg_core::math::{Mat4, Vec2};
use gg_renderer::{Draws, MeshDraw, Sampler, Sprite};

use crate::reflect::{self, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value};
use crate::{Component, Parent, Transform, World, Writable};

impl Component for Sprite {}
impl Writable for Sprite {}

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
impl Writable for MeshDraw {}

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

/// What drawing the world sorts and computes, kept from frame to frame so that it allocates nothing
/// once it's grown.
#[derive(Default)]
pub(crate) struct Scratch {
    segments: Vec<Segment>,
    /// The world matrices of the rows of tables of children, which draw by them.
    matrices: Vec<Mat4>,
}

/// Rows of one table at one z, drawn together.
struct Segment {
    z: f32,
    /// Which segment it was found as: tables in the order they were made, then a table's sprites
    /// before its meshes, then its rows in order.
    order: u32,
    kind: Kind,
    table: u32,
    rows: Range<u32>,
    /// Where its table's world matrices start among the scratch's, for a table of children;
    /// [`ROOTS`] for a table of roots, whose rows draw by their transforms.
    matrices: u32,
}

/// A [`Segment`] of roots.
const ROOTS: u32 = u32::MAX;

#[derive(Clone, Copy)]
enum Kind {
    Sprite,
    Mesh,
}

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`] or a
/// [`MeshDraw`], back to front by its z in the world; at one z, in the order queries find them,
/// and an entity with both draws its sprite under its mesh. A child draws where its ancestors carry
/// it, by its world matrix, which it computes into `scratch` for each child once a frame; roots,
/// in tables without a [`Parent`], draw by their transforms alone.
///
/// It sorts runs of consecutive rows at one z, so it's quick while each table's rows at one z stay
/// together. Spawning entities of a table at mixed z's, or despawning, which moves a table's last
/// row into the gap, splits those runs; at worst, as when sprites each have their own z, as sorting
/// by y gives them, it sorts every sprite every frame, until the renderer sorts by depth, with
/// Phase 4.
pub(crate) fn draw_world(world: &World, draws: &mut Draws, scratch: &mut Scratch) {
    let _zone = gg_core::zone!();
    let Scratch { segments, matrices } = scratch;
    segments.clear();
    matrices.clear();
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    let sprite = world.component::<Sprite>();
    let mesh = world.component::<MeshDraw>();
    let parent = world.component::<Parent>();
    // A child's matrix from its own space to the world's: its transform's, then each ancestor's in
    // turn up to its root, an ancestor without a transform carrying its children as though it
    // were at the origin, unturned, at its own size. It walks up from each child, every frame, so
    // children of one parent each take the steps they share, and a deep hierarchy costs its depth
    // for each of them, until world transforms are stored and propagated, as physics or a profile
    // will need.
    let carry = |local: &Transform, parent: &Parent| {
        world
            .lineage(*parent)
            .fold(local.matrix(), |m, (table, row)| {
                match table.column::<Transform>(transform) {
                    Some(transforms) => transforms[row].matrix() * m,
                    None => m,
                }
            })
    };
    for (table, rows) in world.tables().iter().enumerate() {
        let Some(transforms) = rows.column::<Transform>(transform) else {
            continue;
        };
        let drawn = [(Kind::Sprite, sprite), (Kind::Mesh, mesh)]
            .map(|(kind, id)| (kind, id.is_some_and(|id| rows.has(id))));
        if drawn.iter().all(|&(_, drawn)| !drawn) {
            continue;
        }
        let table = table as u32;
        let len = transforms.len();
        match parent.and_then(|id| rows.column::<Parent>(id)) {
            None => {
                let z = |row: usize| transforms[row].translation.z;
                for (kind, _) in drawn.into_iter().filter(|&(_, drawn)| drawn) {
                    cut(segments, len, z, kind, table, ROOTS);
                }
            }
            Some(parents) => {
                let start = matrices.len();
                let carried = transforms.iter().zip(parents);
                matrices.extend(carried.map(|(local, parent)| carry(local, parent)));
                let z = |row: usize| matrices[start + row].w_axis.z;
                let start = u32::try_from(start).expect("fewer than 2^32 children drawn");
                for (kind, _) in drawn.into_iter().filter(|&(_, drawn)| drawn) {
                    cut(segments, len, z, kind, table, start);
                }
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
        // Its children's world matrices, unless they're roots.
        let carried = (segment.matrices != ROOTS).then(|| {
            let start = segment.matrices as usize;
            &matrices[start + rows.start..start + rows.end]
        });
        match segment.kind {
            Kind::Sprite => {
                let sprites = sprite.and_then(|id| table.column::<Sprite>(id));
                let sprites = &sprites.expect("it has sprites")[rows];
                match carried {
                    Some(carried) => {
                        draws.extend_sprites(carried.iter().copied().zip(sprites.iter().copied()));
                    }
                    None => {
                        let both = transforms.iter().zip(sprites);
                        let both = both.map(|(transform, sprite)| (transform.matrix(), *sprite));
                        draws.extend_sprites(both);
                    }
                }
            }
            Kind::Mesh => {
                let meshes = mesh.and_then(|id| table.column::<MeshDraw>(id));
                let meshes = &meshes.expect("it has meshes")[rows];
                for (row, mesh) in meshes.iter().enumerate() {
                    let matrix = carried.map_or_else(|| transforms[row].matrix(), |m| m[row]);
                    draws.push_draw(matrix, mesh);
                }
            }
        }
    }
}

/// Adds to `segments` the runs of rows at one z, which `z` gives, among table `table`'s `len` rows,
/// of `kind`, whose world matrices start at `matrices` among the scratch's.
fn cut(
    segments: &mut Vec<Segment>,
    len: usize,
    z: impl Fn(usize) -> f32,
    kind: Kind,
    table: u32,
    matrices: u32,
) {
    let mut start = 0;
    for row in 1..=len {
        if row == len || z(row).total_cmp(&z(start)).is_ne() {
            segments.push(Segment {
                z: z(start),
                order: segments.len() as u32,
                kind,
                table,
                rows: start as u32..row as u32,
                matrices,
            });
            start = row;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gg_core::math::{Quat, Vec3, Vec4};
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
        draw_world(&world, &mut draws, &mut Scratch::default());
        assert_eq!(draws, want);
    }

    /// Draws children where their ancestors carry them, back to front by their z's in the world: a
    /// child of a pivot that draws nothing, between two roots, where its own z would put it under
    /// both; a grandchild whose table was made before its parent's; children with a sprite and a
    /// mesh each, two of them at one z after the first in their table; and the child of an
    /// ancestor without a transform. Their ancestors turn and scale unevenly, which doesn't
    /// commute, so that matrices multiplied the wrong way round show. Drawing again in the same
    /// scratch takes no more memory.
    #[test]
    fn children_draw_where_their_ancestors_carry_them() {
        let mut shaders = Shaders::default();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let flat = shaders
            .add::<[f32; 3], Vec4>("flat color", &fixture, Blend::Opaque)
            .expect("the shader compiles");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = Meshes::default().add("triangle", &corners, &[0, 1, 2]);
        let triangle = triangle.expect("a mesh");
        let placed = |x: f32, z: f32, turn: f32, scale: [f32; 2]| Transform {
            translation: Vec3::new(x, 0.25 * x, z),
            rotation: Quat::from_rotation_z(turn),
            scale: Vec3::new(scale[0], scale[1], 1.0),
        };
        let sprite = |n: f32| Sprite {
            color: Vec4::splat(n),
            ..Sprite::default()
        };
        let mut world = World::default();
        let low = placed(1.0, 0.3, 0.0, [1.0; 2]);
        world.spawn((low, sprite(1.0)));
        let high = placed(2.0, 0.5, 0.0, [1.0; 2]);
        world.spawn((high, sprite(2.0)));
        let pivot_at = placed(0.5, 0.4, 0.7, [2.0, 0.5]);
        let pivot = world.spawn(pivot_at);
        let grand_at = placed(0.25, 0.02, -1.1, [0.5, 1.5]);
        let grand = world.spawn((grand_at, sprite(3.0), Marked));
        let middle_at = placed(1.0, 0.01, 0.3, [1.5, 0.75]);
        let middle = world.spawn((middle_at, sprite(4.0)));
        let mesh_at = placed(-0.5, 0.03, 0.2, [0.5, 2.0]);
        let mesh = MeshDraw::new(triangle, flat, Vec4::splat(5.0));
        let meshed = world.spawn((mesh_at, sprite(5.0), mesh));
        let twins_at = [-1.5, -2.5].map(|x| placed(x, 0.035, 0.9, [1.0, 0.5]));
        let twins = twins_at.map(|at| world.spawn((at, sprite(7.0), mesh)));
        let bare = world.spawn(Marked);
        let kid_at = placed(-1.0, 0.05, -0.4, [1.0, 3.0]);
        let kid = world.spawn((kid_at, sprite(6.0)));
        for (child, parent) in [
            (grand, middle),
            (middle, pivot),
            (meshed, middle),
            (twins[0], middle),
            (twins[1], middle),
            (bare, pivot),
            (kid, bare),
        ] {
            world.set_parent(child, Some(parent)).expect("no cycle");
        }

        // Back to front: 0.3, 0.41, 0.43, 0.44, 0.445, 0.45 and 0.5 in the world, each child's
        // matrix its own, then each ancestor's in turn; at one z, a table's sprites under its
        // meshes.
        let [pivot, middle] = [pivot_at.matrix(), middle_at.matrix()];
        let twins = twins_at.map(|at| pivot * (middle * at.matrix()));
        let mut want = Draws::default();
        want.push_sprite(low.matrix(), sprite(1.0));
        want.push_sprite(pivot * middle, sprite(4.0));
        want.push_sprite(pivot * (middle * grand_at.matrix()), sprite(3.0));
        let meshed = pivot * (middle * mesh_at.matrix());
        want.push_sprite(meshed, sprite(5.0));
        want.push_draw(meshed, &mesh);
        for twin in twins {
            want.push_sprite(twin, sprite(7.0));
        }
        for twin in twins {
            want.push_draw(twin, &mesh);
        }
        want.push_sprite(pivot * kid_at.matrix(), sprite(6.0));
        want.push_sprite(high.matrix(), sprite(2.0));
        let mut scratch = Scratch::default();
        let mut draws = Draws::default();
        draw_world(&world, &mut draws, &mut scratch);
        assert_eq!(draws, want);
        let held = |scratch: &Scratch| {
            let Scratch { segments, matrices } = scratch;
            (
                segments.as_ptr(),
                segments.capacity(),
                matrices.as_ptr(),
                matrices.capacity(),
            )
        };
        let before = held(&scratch);
        let mut again = Draws::default();
        draw_world(&world, &mut again, &mut scratch);
        assert_eq!(again, want);
        assert_eq!(held(&scratch), before, "drawing again takes no more memory");
    }
}
