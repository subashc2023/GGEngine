//! The engine's own system, drawing the world's entities; and its components that `gg-renderer`
//! defines, described to reflection here, since they can't derive it there.

use std::ops::Range;

use gg_core::Error;
use gg_core::math::{Mat4, Vec2};
use gg_renderer::{Draws, MeshDraw, Sampler, Sprite};

use crate::reflect::{self, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value};
use crate::{Component, EntityId, Parent, Transform, World, Writable};

/// Built on the default sprite, so that a scene's sprite, which leaves its texture out, shows the
/// white texture until code sets another.
impl Component for Sprite {
    fn base() -> Option<Self> {
        Some(Self::default())
    }
}

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
/// once it's grown, and so that a frame that changes no entity's row and no z draws in the order
/// the last one sorted its rows by ID, without sorting them by ID again.
#[derive(Default)]
pub(crate) struct Scratch {
    /// This frame's runs of rows at one z, back to front.
    segments: Vec<Segment>,
    /// The segments `items` were sorted from, which each frame's are compared with.
    last: Vec<Segment>,
    /// The world matrices of the rows of tables of children, which draw by them.
    matrices: Vec<Mat4>,
    /// Each row drawn, in the order they draw.
    items: Vec<Item>,
    /// The rows of each z and kind, back to front.
    groups: Vec<Group>,
    /// The world and its version when `items` were sorted, if they are.
    sorted: Option<(u64, u64)>,
}

/// A run of one table's rows at one z, of one kind, whose rows `sort` puts among the others of
/// their z and kind.
#[derive(Clone)]
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

/// The same segment, its z bit for bit, so that a z that only changes its sign shows.
impl PartialEq for Segment {
    fn eq(&self, other: &Self) -> bool {
        self.z.to_bits() == other.z.to_bits()
            && self.order == other.order
            && self.kind == other.kind
            && self.table == other.table
            && self.rows == other.rows
            && self.matrices == other.matrices
    }
}

/// A [`Segment`] of roots.
const ROOTS: u32 = u32::MAX;

/// What a row draws: at one z, sprites draw under meshes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Sprite,
    Mesh,
}

/// A row drawn: its entity's ID, which orders it among the rows of its z and kind, and where it is.
struct Item {
    id: EntityId,
    /// Its segment, among the scratch's.
    segment: u32,
    /// Its row in its segment's table.
    row: u32,
}

/// The rows of one z and one kind, a range of the scratch's items.
struct Group {
    kind: Kind,
    items: Range<u32>,
}

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`] or a
/// [`MeshDraw`], back to front by its z in the world; at one z, sprites under meshes, each in the
/// order of their entities' IDs, so that entities that overlap at one z keep how they stack as
/// others come and go. A child draws where its ancestors carry it, by its world matrix, which it
/// computes into `scratch` for each child once a frame; roots, in tables without a [`Parent`], draw
/// by their transforms alone.
///
/// It sorts runs of consecutive rows at one z every frame, so it's quick while each table's rows at
/// one z stay together. Spawning entities of a table at mixed z's, or despawning, which moves a
/// table's last row into the gap, splits those runs; at worst, as when sprites each have their own
/// z, as sorting by y gives them, it sorts every sprite every frame, until the renderer sorts by
/// depth, with Phase 4. The rows of each z are sorted by ID, and kept sorted in `scratch` while no
/// entity's row and no z changes. A frame that changes them sorts every row at a shared z again,
/// O(n log n), as the swarm's 100,000 at one z are; and every frame draws them in that order,
/// reading their tables' rows out of order rather than in a sweep. A radix sort, or tables kept in
/// ID order, waits for a game whose frames need the time.
pub(crate) fn draw_world(world: &World, draws: &mut Draws, scratch: &mut Scratch) {
    let _zone = gg_core::zone!();
    let Scratch {
        segments,
        last,
        matrices,
        items,
        groups,
        sorted,
    } = scratch;
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
    segments.sort_unstable_by(|a, b| {
        let z = a.z.total_cmp(&b.z);
        z.then(a.kind.cmp(&b.kind)).then(a.order.cmp(&b.order))
    });
    if *sorted != Some(world.version()) || segments != last {
        sort(world, segments, items, groups);
        *sorted = Some(world.version());
        last.clone_from(segments);
    }
    let tables = world.tables();
    // A segment's table, its transforms, and its rows' world matrices if they're children.
    let placed = |index: u32| {
        let segment = &segments[index as usize];
        let table = &tables[segment.table as usize];
        let transforms = table.column::<Transform>(transform);
        let transforms = transforms.expect("it has transforms");
        let carried = (segment.matrices != ROOTS).then(|| &matrices[segment.matrices as usize..]);
        (table, transforms, carried)
    };
    let matrix = |transforms: &[Transform], carried: Option<&[Mat4]>, row: usize| {
        carried.map_or_else(|| transforms[row].matrix(), |carried| carried[row])
    };
    // Kept across groups, whose segments' indices differ.
    let (mut sprites_at, mut meshes_at) = ([None; AT_HAND], [None; AT_HAND]);
    for group in groups.iter() {
        let items = &items[group.items.start as usize..group.items.end as usize];
        match group.kind {
            Kind::Sprite => {
                let sprite = sprite.expect("a table has sprites");
                let columns = |index: u32| {
                    let (table, transforms, carried) = placed(index);
                    let sprites = table.column::<Sprite>(sprite).expect("it has sprites");
                    (transforms, sprites, carried)
                };
                draws.extend_sprites(items.iter().map(|item| {
                    let found = found(&mut sprites_at, item.segment, columns);
                    let (transforms, sprites, carried) = found;
                    let row = item.row as usize;
                    (matrix(transforms, carried, row), sprites[row])
                }));
            }
            Kind::Mesh => {
                let mesh = mesh.expect("a table has meshes");
                let columns = |index: u32| {
                    let (table, transforms, carried) = placed(index);
                    let meshes = table.column::<MeshDraw>(mesh).expect("it has meshes");
                    (transforms, meshes, carried)
                };
                for item in items {
                    let found = found(&mut meshes_at, item.segment, columns);
                    let (transforms, meshes, carried) = found;
                    let row = item.row as usize;
                    draws.push_draw(matrix(transforms, carried, row), &meshes[row]);
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

/// Puts each row of `segments`, sorted back to front, into `items`, a group of them for each z and
/// kind, each group's in the order of its entities' IDs.
fn sort(world: &World, segments: &[Segment], items: &mut Vec<Item>, groups: &mut Vec<Group>) {
    items.clear();
    groups.clear();
    let tables = world.tables();
    let mut first = 0;
    for group in segments.chunk_by(|a, b| a.z.total_cmp(&b.z).is_eq() && a.kind == b.kind) {
        let start = items.len();
        for (index, segment) in (first..).zip(group) {
            let entities = tables[segment.table as usize].entities();
            for row in segment.rows.clone() {
                let id = world.id(entities[row as usize]);
                let id = id.expect("a table's rows are alive");
                items.push(Item {
                    id,
                    segment: index,
                    row,
                });
            }
        }
        // Each entity's ID is its own, so that the order is the same however it's sorted.
        items[start..].sort_unstable_by_key(|item| item.id);
        let end = u32::try_from(items.len()).expect("fewer than 2^32 rows drawn");
        groups.push(Group {
            kind: group[0].kind,
            items: start as u32..end,
        });
        first += group.len() as u32;
    }
}

/// How many segments' columns drawing keeps at hand, each in the slot its index picks. A group's
/// segments have consecutive indices, so a group of this many or fewer, whose rows interleave in ID
/// order, finds each one's columns once.
const AT_HAND: usize = 16;

/// What `find` gives for segment `index`: what `at` keeps for it, or else found, and kept in its
/// slot.
// Called for each item drawn, so asked to be inlined: left to LLVM, whether it is turns on how
// the crate splits into codegen units, and a frame of the swarm's 100,000 sprites shows the calls.
#[inline]
fn found<T: Copy>(
    at: &mut [Option<(u32, T)>; AT_HAND],
    index: u32,
    find: impl FnOnce(u32) -> T,
) -> T {
    let at = &mut at[index as usize % AT_HAND];
    match *at {
        Some((segment, columns)) if segment == index => columns,
        _ => at.insert((index, find(index))).1,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gg_core::math::{Quat, Vec3, Vec4};
    use gg_renderer::{Blend, Meshes, Shaders};

    use super::*;
    use crate::Entity;

    #[derive(Clone, Copy, Component)]
    struct Marked;

    /// Where a scratch's buffers are, and how big, which drawing again without taking more memory
    /// leaves as they were.
    fn held(scratch: &Scratch) -> [(usize, usize); 5] {
        let Scratch {
            segments,
            last,
            matrices,
            items,
            groups,
            sorted: _,
        } = scratch;
        [
            (segments.as_ptr().addr(), segments.capacity()),
            (last.as_ptr().addr(), last.capacity()),
            (matrices.as_ptr().addr(), matrices.capacity()),
            (items.as_ptr().addr(), items.capacity()),
            (groups.as_ptr().addr(), groups.capacity()),
        ]
    }

    enum Expected {
        Sprite(Entity, Transform, Sprite),
        Mesh(Entity, Transform, MeshDraw),
    }

    /// Spawns sprites and meshes at several z's across tables, and checks that they're drawn back
    /// to front, and at one z sprites under meshes, each in the order of their entities' IDs,
    /// whatever tables they're in. Two tables' rows alternate between two z's, so that dozens of
    /// runs share each, enough for an order by table or by row to show.
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
        let mut expected = Vec::new();
        let alternating = |row: usize| if row.is_multiple_of(2) { 0.5 } else { 0.6 };
        for z in [0.3, 0.1, 0.1].into_iter().chain((0..20).map(alternating)) {
            let (at, sprite, _) = new(z);
            let entity = world.spawn((at, sprite));
            expected.push(Expected::Sprite(entity, at, sprite));
        }
        for z in (0..20).map(alternating).chain([0.1]) {
            let (at, sprite, _) = new(z);
            let entity = world.spawn((Marked, sprite, at));
            expected.push(Expected::Sprite(entity, at, sprite));
        }
        for z in [0.2, 0.1] {
            let (at, _, mesh) = new(z);
            let entity = world.spawn((at, mesh));
            expected.push(Expected::Mesh(entity, at, mesh));
        }
        for z in [0.2, 0.0, 0.1] {
            let (at, sprite, mesh) = new(z);
            let entity = world.spawn((at, sprite, mesh));
            expected.push(Expected::Sprite(entity, at, sprite));
            expected.push(Expected::Mesh(entity, at, mesh));
        }
        // Nothing to draw, or nowhere to draw it.
        let (at, sprite, _) = new(0.0);
        world.spawn(at);
        world.spawn(sprite);

        // Back to front; at one z, sprites under meshes, each by ID.
        let key = |expected: &Expected| match expected {
            Expected::Sprite(entity, at, _) => (at.translation.z, 0, world.id(*entity)),
            Expected::Mesh(entity, at, _) => (at.translation.z, 1, world.id(*entity)),
        };
        expected.sort_by(|a, b| {
            let (a, b) = (key(a), key(b));
            a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
        });
        let mut want = Draws::default();
        for expected in &expected {
            match expected {
                Expected::Sprite(_, at, sprite) => want.push_sprite(at.matrix(), *sprite),
                Expected::Mesh(_, at, mesh) => want.push_draw(at.matrix(), mesh),
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
        // matrix its own, then each ancestor's in turn; at one z, sprites under meshes, each in the
        // order of their IDs.
        let mut twins = [0, 1].map(|i| (world.id(twins[i]), twins_at[i]));
        twins.sort_by_key(|&(id, _)| id);
        let [pivot, middle] = [pivot_at.matrix(), middle_at.matrix()];
        let twins = twins.map(|(_, at)| pivot * (middle * at.matrix()));
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
        let before = held(&scratch);
        let mut again = Draws::default();
        draw_world(&world, &mut again, &mut scratch);
        assert_eq!(again, want);
        assert_eq!(held(&scratch), before, "drawing again takes no more memory");
    }

    /// Sprites, some with meshes, at one z across four tables, drawn after each change by a scratch
    /// kept from frame to frame and by a fresh one, which draw the same each time: after nothing,
    /// which takes no more memory; after the world is replaced by another spawned the same way,
    /// which has the same tables, rows and z's and as many changes, but other IDs; after entities
    /// despawned and others spawned in their tables,
    /// which keep their lengths, so that only which entity is in a row shows; after two entities
    /// trade tables; after a z is set, which moves no row; and after a spawn and a despawn alone.
    #[test]
    fn a_kept_scratch_draws_as_a_fresh_one_does() {
        let mut shaders = Shaders::default();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let flat = shaders
            .add::<[f32; 3], Vec4>("flat color", &fixture, Blend::Opaque)
            .expect("the shader compiles");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = Meshes::default().add("triangle", &corners, &[0, 1, 2]);
        let triangle = triangle.expect("a mesh");
        // Each told apart by its x and its color.
        let mut n = 0.0;
        let mut spawn = |world: &mut World, meshed: bool, marked: bool| {
            n += 1.0;
            let at = Transform {
                translation: Vec3::new(n, 0.0, 0.5),
                ..Transform::IDENTITY
            };
            let sprite = Sprite {
                color: Vec4::splat(n),
                ..Sprite::default()
            };
            let mesh = MeshDraw::new(triangle, flat, Vec4::splat(n));
            match (meshed, marked) {
                (false, false) => world.spawn((at, sprite)),
                (true, false) => world.spawn((at, sprite, mesh)),
                (false, true) => world.spawn((at, sprite, Marked)),
                (true, true) => world.spawn((at, sprite, mesh, Marked)),
            }
        };
        // The same tables, rows and z's in any world.
        let mut populate = |world: &mut World| -> Vec<Entity> {
            (0..12)
                .map(|i| spawn(world, i % 2 == 0, i % 3 == 0))
                .collect()
        };
        let mut world = World::default();
        populate(&mut world);
        let step = |world: &World, kept: &mut Scratch, what: &str| {
            let mut draws = Draws::default();
            draw_world(world, &mut draws, kept);
            let mut fresh = Draws::default();
            draw_world(world, &mut fresh, &mut Scratch::default());
            assert_eq!(draws, fresh, "after {what}");
        };
        let mut kept = Scratch::default();
        step(&world, &mut kept, "the first frame");
        let before = held(&kept);
        step(&world, &mut kept, "nothing");
        assert_eq!(held(&kept), before, "drawing again takes no more memory");
        world = World::default();
        let mut entities = populate(&mut world);
        step(&world, &mut kept, "the world replaced");
        // Each round despawns one entity and spawns another into its table, which keeps its length,
        // so that the order a stale scratch kept would draw other entities' rows.
        for round in 0..20 {
            let gone = entities.swap_remove(round * 7 % entities.len());
            let meshed = world.get::<MeshDraw>(gone).is_some();
            let marked = world.get::<Marked>(gone).is_some();
            world.despawn(gone).expect("alive");
            entities.push(spawn(&mut world, meshed, marked));
            step(
                &world,
                &mut kept,
                "one despawned and another spawned in its table",
            );
        }
        let of = |meshed: bool, marked: bool| {
            let found = entities.iter().copied().find(|&entity| {
                world.get::<MeshDraw>(entity).is_some() == meshed
                    && world.get::<Marked>(entity).is_some() == marked
            });
            found.expect("one of each")
        };
        let (plain, marked, lowered) = (of(false, false), of(false, true), of(true, true));
        world.insert(plain, Marked).expect("alive");
        world.remove::<Marked>(marked).expect("alive");
        step(&world, &mut kept, "two entities trading tables");
        let z = |world: &mut World, z| {
            let at = world.get_mut::<Transform>(lowered).expect("placed");
            at.translation.z = z;
        };
        z(&mut world, 0.25);
        step(&world, &mut kept, "a z set");
        z(&mut world, 0.5);
        step(&world, &mut kept, "the z set back");
        entities.push(spawn(&mut world, true, false));
        step(&world, &mut kept, "a spawn");
        world.despawn(entities.swap_remove(1)).expect("alive");
        step(&world, &mut kept, "a despawn");
    }
}
