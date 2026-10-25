//! The engine's own system, drawing the world's entities; and its components that `gg-renderer`
//! defines, described to reflection here, since they can't derive it there.

use std::ops::Range;

use gg_core::Error;
use gg_core::math::{Mat4, Vec2};
use gg_renderer::{Draws, MeshDraw, Sampler, Sprite};

use crate::interpolation::{self, Past};
use crate::jobs;
use crate::reflect::{self, Digest, Field, Meta, Reflect, Result, Shape, Show, Type, Types, Value};
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

    fn digest(&self, to: &mut Digest<'_>) {
        let Sprite {
            color,
            texture: _,
            rect,
            sampler,
        } = self;
        color.digest(to);
        rect.digest(to);
        sampler.digest(to);
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

    /// By its index, which is its variant's in its description.
    fn digest(&self, to: &mut Digest<'_>) {
        self.index().digest(to);
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

    fn digest(&self, _: &mut Digest<'_>) {}
}

/// What drawing the world sorts and computes, kept from frame to frame so that it allocates nothing
/// once it's grown, and so that a frame that changes no entity's row, no z and no batch draws in
/// the order the last one sorted its rows by ID, without sorting them by ID again.
#[derive(Default)]
pub(crate) struct Scratch {
    /// This frame's runs of rows of one batch, then those at one z, back to front.
    segments: Vec<Segment>,
    /// The segments `items` were sorted from, which each frame's are compared with.
    last: Vec<Segment>,
    /// The world matrices of the rows of tables of children, which draw by them.
    matrices: Vec<Mat4>,
    /// The z's in the world of one table of children's rows, as the latest tick left them, which
    /// order them.
    depths: Vec<f32>,
    /// The matrices of the rows of tables of roots that draw between ticks, which draw by them:
    /// kept at its longest, so that the job system's threads write into it rather than its being
    /// filled anew each frame.
    blended: Vec<Mat4>,
    /// Where each table's rows start in `blended`, by the table's index, if they're there.
    blends: Vec<Option<usize>>,
    /// Each row drawn, in the order they draw.
    items: Vec<Item>,
    /// The rows of each batch, then of each z and kind, back to front.
    groups: Vec<Group>,
    /// The world and its version when `items` were sorted, if they are.
    sorted: Option<(u64, u64)>,
}

/// A run of one table's rows at one z, of one kind, or of one batch, for opaque meshes, whose rows
/// `sort` puts among the others of their z and kind, or of their batch.
#[derive(Clone)]
struct Segment {
    /// Its rows' z, or 0 for opaque meshes, which draw in any order.
    z: f32,
    /// Its rows' [`MeshDraw::batch`], for opaque meshes, or 0.
    batch: u64,
    /// Which segment it was found as: tables in the order they were made, then a table's sprites
    /// before its meshes, then its rows in order.
    order: u32,
    kind: Kind,
    table: u32,
    rows: Range<u32>,
    /// Where its table's world matrices start among the scratch's, for a table of children;
    /// [`ROOTS`] for a table of roots, whose rows draw by their transforms, or by `blended` from
    /// where `blends` says, while they draw between ticks.
    matrices: u32,
}

/// The same segment, its z bit for bit, so that a z that only changes its sign shows.
impl PartialEq for Segment {
    fn eq(&self, other: &Self) -> bool {
        self.z.to_bits() == other.z.to_bits()
            && self.batch == other.batch
            && self.order == other.order
            && self.kind == other.kind
            && self.table == other.table
            && self.rows == other.rows
            && self.matrices == other.matrices
    }
}

/// A [`Segment`] of roots.
const ROOTS: u32 = u32::MAX;

/// What a row draws: opaque meshes first, whatever their z, then, at one z, sprites under the meshes
/// that blend.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Opaque,
    Sprite,
    Mesh,
}

/// A row drawn: its entity's ID, which orders it among the rows of its batch, or of its z and kind,
/// and where it is.
struct Item {
    id: EntityId,
    /// Its segment, among the scratch's.
    segment: u32,
    /// Its row in its segment's table.
    row: u32,
}

/// The rows of one batch, or of one z and one kind, a range of the scratch's items.
struct Group {
    kind: Kind,
    items: Range<u32>,
}

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`] or a
/// [`MeshDraw`]: first the meshes whose shaders are opaque, whose depth hides what's behind them
/// whatever order they draw in, by their [`MeshDraw::batch`], so that each mesh with each shader is
/// one instanced draw, split only where a culling mesh's draws change between mirrored and not;
/// then the rest, which blend, back to front by z in the world, as the latest tick left them, at
/// one z sprites under meshes; each batch and each z in the order of their entities' IDs, so that
/// entities that overlap at one depth keep how they stack as others come and go. Each draws `alpha`
/// of the way from what `past` has it draw from to where the tick left it, while it draws between
/// ticks; at 1, where the tick left it.
/// A child draws where its ancestors carry it, by its world matrix, its transform's and each of
/// theirs drawn so, which it computes into `scratch` for each child once a frame; roots, in tables
/// without a [`Parent`], draw by their transforms alone, which those that draw between ticks blend
/// first, in their tables' order, in runs of rows that the job system's threads take.
///
/// It sorts runs of consecutive rows of one batch or at one z every frame, so it's quick while each
/// table's rows at one z stay together, as opaque meshes do whatever their z's. Spawning entities
/// of a table at mixed z's, or despawning, which moves a table's last row into the gap, splits those
/// runs; at worst, as when sprites each have their own z, as sorting by y gives them, it sorts
/// every sprite every frame. The rows of each batch and each z are sorted by ID, and kept sorted in
/// `scratch` while no entity's row, no z and no batch changes. A frame that changes them sorts every
/// row of a shared batch or z again, O(n log n), as the swarm's 100,000 at one z are; and every
/// frame draws them in that order, reading their tables' rows out of order rather than in a sweep.
/// A radix sort, or tables kept in ID order, waits for a game whose frames need the time.
pub(crate) fn draw_world(
    world: &World,
    past: &Past,
    alpha: f32,
    draws: &mut Draws,
    scratch: &mut Scratch,
) {
    let _zone = gg_core::zone!();
    let Scratch {
        segments,
        last,
        matrices,
        depths,
        blended,
        blends,
        items,
        groups,
        sorted,
    } = scratch;
    segments.clear();
    matrices.clear();
    blends.clear();
    blends.resize(world.tables().len(), None);
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    let sprite = world.component::<Sprite>();
    let mesh = world.component::<MeshDraw>();
    let parent = world.component::<Parent>();
    // What table `table`'s rows draw from, from its first row on, while they draw between ticks.
    let drawn_from = |table: usize| (alpha < 1.0).then(|| past.from(world, table)).flatten();
    // A child's matrices from its own space to the world's, as the latest tick left it and as it
    // draws, from its transform, `drawn` as it draws: its transform's, then each ancestor's in
    // turn up to its root, an ancestor without a transform carrying its children as though it
    // were at the origin, unturned, at its own size. The second is `None` while it's the first,
    // when nothing on the way draws between ticks. It walks up from each child, every frame, so
    // children of one parent each take the steps they share, and a deep hierarchy costs its depth
    // for each of them, until world transforms are stored and propagated, as physics or a profile
    // will need.
    let carry = |local: &Transform, drawn: Option<Transform>, parent: &Parent| {
        let start = (local.matrix(), drawn.map(|drawn| drawn.matrix()));
        world
            .lineage(*parent)
            .fold(start, |(at, drawn), (index, table, row)| {
                let Some(transforms) = table.column::<Transform>(transform) else {
                    return (at, drawn);
                };
                let ancestor = transforms[row].matrix();
                let drawn = match drawn_from(index) {
                    Some(from) => {
                        let pose = interpolation::between(&from[row], &transforms[row], alpha);
                        Some(pose.matrix() * drawn.unwrap_or(at))
                    }
                    None => drawn.map(|drawn| ancestor * drawn),
                };
                (ancestor * at, drawn)
            })
    };
    // How many of `blended`'s matrices the frame's roots take.
    let mut blending = 0;
    for (index, rows) in world.tables().iter().enumerate() {
        let Some(transforms) = rows.column::<Transform>(transform) else {
            continue;
        };
        let drawn = [(Kind::Sprite, sprite), (Kind::Mesh, mesh)]
            .map(|(kind, id)| (kind, id.is_some_and(|id| rows.has(id))));
        if drawn.iter().all(|&(_, drawn)| !drawn) {
            continue;
        }
        let table = index as u32;
        let len = transforms.len();
        // A row's batch, if it's a mesh whose shader is opaque, which draws by it rather than by
        // its z.
        let meshes = mesh.and_then(|id| rows.column::<MeshDraw>(id));
        let batch = |kind: Kind, row: usize| match (kind, meshes) {
            (Kind::Mesh, Some(meshes)) => meshes[row].batch(),
            _ => None,
        };
        match parent.and_then(|id| rows.column::<Parent>(id)) {
            None => {
                let z = |row: usize| transforms[row].translation.z;
                for (kind, _) in drawn.into_iter().filter(|&(_, drawn)| drawn) {
                    let batch = |row| batch(kind, row);
                    cut(segments, len, z, batch, kind, table, ROOTS);
                }
                if let Some(from) = drawn_from(index) {
                    let end = blending + len;
                    if blended.len() < end {
                        blended.resize(end, Mat4::IDENTITY);
                    }
                    blend(&mut blended[blending..end], &from[..len], transforms, alpha);
                    blends[index] = Some(blending);
                    blending = end;
                }
            }
            Some(parents) => {
                let start = matrices.len();
                let from = drawn_from(index);
                depths.clear();
                for (row, (local, parent)) in transforms.iter().zip(parents).enumerate() {
                    let drawn = from.map(|from| interpolation::between(&from[row], local, alpha));
                    let (at, drawn) = carry(local, drawn, parent);
                    depths.push(at.w_axis.z);
                    matrices.push(drawn.unwrap_or(at));
                }
                let z = |row: usize| depths[row];
                let start = u32::try_from(start).expect("fewer than 2^32 children drawn");
                for (kind, _) in drawn.into_iter().filter(|&(_, drawn)| drawn) {
                    let batch = |row| batch(kind, row);
                    cut(segments, len, z, batch, kind, table, start);
                }
            }
        }
    }
    // Opaque meshes first, by batch; then the rest back to front. An opaque segment's z is 0, and
    // another's batch.
    segments.sort_unstable_by(|a, b| {
        let opaque = (a.kind != Kind::Opaque).cmp(&(b.kind != Kind::Opaque));
        let z = opaque.then(a.batch.cmp(&b.batch)).then(a.z.total_cmp(&b.z));
        z.then(a.kind.cmp(&b.kind)).then(a.order.cmp(&b.order))
    });
    if *sorted != Some(world.version()) || segments != last {
        sort(world, segments, items, groups);
        *sorted = Some(world.version());
        last.clone_from(segments);
    }
    let tables = world.tables();
    // A segment's table, its transforms, and the matrices its rows draw by, if they're children or
    // roots that draw between ticks.
    let placed = |index: u32| {
        let segment = &segments[index as usize];
        let table = &tables[segment.table as usize];
        let transforms = table.column::<Transform>(transform);
        let transforms = transforms.expect("it has transforms");
        let drawn = match segment.matrices {
            ROOTS => blends[segment.table as usize].map(|start| &blended[start..]),
            start => Some(&matrices[start as usize..]),
        };
        (table, transforms, drawn)
    };
    let matrix = |transforms: &[Transform], drawn: Option<&[Mat4]>, row: usize| {
        drawn.map_or_else(|| transforms[row].matrix(), |drawn| drawn[row])
    };
    // Kept across groups, whose segments' indices differ.
    let (mut sprites_at, mut meshes_at) = ([None; AT_HAND], [None; AT_HAND]);
    for group in groups.iter() {
        let items = &items[group.items.start as usize..group.items.end as usize];
        match group.kind {
            Kind::Sprite => {
                let sprite = sprite.expect("a table has sprites");
                let columns = |index: u32| {
                    let (table, transforms, drawn) = placed(index);
                    let sprites = table.column::<Sprite>(sprite).expect("it has sprites");
                    (transforms, sprites, drawn)
                };
                draws.extend_sprites(items.iter().map(|item| {
                    let found = found(&mut sprites_at, item.segment, columns);
                    let (transforms, sprites, drawn) = found;
                    let row = item.row as usize;
                    (matrix(transforms, drawn, row), sprites[row])
                }));
            }
            Kind::Opaque | Kind::Mesh => {
                let mesh = mesh.expect("a table has meshes");
                let columns = |index: u32| {
                    let (table, transforms, drawn) = placed(index);
                    let meshes = table.column::<MeshDraw>(mesh).expect("it has meshes");
                    (transforms, meshes, drawn)
                };
                for item in items {
                    let found = found(&mut meshes_at, item.segment, columns);
                    let (transforms, meshes, drawn) = found;
                    let row = item.row as usize;
                    draws.push_draw(matrix(transforms, drawn, row), &meshes[row]);
                }
            }
        }
    }
}

/// How many rows a job blends.
const CHUNK: usize = 1024;

/// Writes into `out` the matrix of each row drawn `alpha` of the way from `from` to `to`, splitting
/// the rows in two at a multiple of [`CHUNK`], for two jobs, until each run is a chunk or less.
fn blend(out: &mut [Mat4], from: &[Transform], to: &[Transform], alpha: f32) {
    let len = out.len();
    if len <= CHUNK {
        // A shorter run can take less than the 10 µs or so a zone is for.
        let _zone = (len == CHUNK).then(|| gg_core::zone!("blend"));
        for ((out, from), to) in out.iter_mut().zip(from).zip(to) {
            *out = interpolation::between(from, to, alpha).matrix();
        }
        return;
    }
    let mid = len.div_ceil(CHUNK) / 2 * CHUNK;
    let (a, b) = out.split_at_mut(mid);
    let ((from_a, from_b), (to_a, to_b)) = (from.split_at(mid), to.split_at(mid));
    jobs::join(
        || blend(a, from_a, to_a, alpha),
        || blend(b, from_b, to_b, alpha),
    );
}

/// Adds to `segments` the runs of rows at one z, which `z` gives, among table `table`'s `len` rows,
/// of `kind`, but for meshes whose `batch` is one, which are opaque, the runs of rows of one batch;
/// their world matrices start at `matrices` among the scratch's.
fn cut(
    segments: &mut Vec<Segment>,
    len: usize,
    z: impl Fn(usize) -> f32,
    batch: impl Fn(usize) -> Option<u64>,
    kind: Kind,
    table: u32,
    matrices: u32,
) {
    // A row's kind, batch and z: an opaque mesh's z is 0, and another's batch.
    let key = |row: usize| match batch(row) {
        Some(batch) => (Kind::Opaque, batch, 0.0),
        None => (kind, 0, z(row)),
    };
    let same = |(a, a_batch, a_z): (Kind, u64, f32), (b, b_batch, b_z): (Kind, u64, f32)| {
        (a, a_batch) == (b, b_batch) && a_z.total_cmp(&b_z).is_eq()
    };
    let mut start = 0;
    for row in 1..=len {
        if row == len || !same(key(start), key(row)) {
            let (kind, batch, z) = key(start);
            segments.push(Segment {
                z,
                batch,
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

/// Puts each row of `segments`, sorted, into `items`, a group of them for each batch, and for each z
/// and kind, each group's in the order of its entities' IDs.
fn sort(world: &World, segments: &[Segment], items: &mut Vec<Item>, groups: &mut Vec<Group>) {
    items.clear();
    groups.clear();
    let tables = world.tables();
    let mut first = 0;
    let same = |a: &Segment, b: &Segment| {
        (a.kind, a.batch) == (b.kind, b.batch) && a.z.total_cmp(&b.z).is_eq()
    };
    for group in segments.chunk_by(same) {
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
    use std::f32::consts::FRAC_PI_2;
    use std::path::Path;

    use gg_core::math::{Quat, Vec3, Vec4};
    use gg_renderer::{Blend, Mesh, Meshes, Shader, Shaders};

    use super::*;
    use crate::Entity;

    #[derive(Clone, Copy, Component)]
    struct Marked;

    /// Where a scratch's buffers are, and how big, which drawing again without taking more memory
    /// leaves as they were.
    fn held(scratch: &Scratch) -> [(usize, usize); 8] {
        let Scratch {
            segments,
            last,
            matrices,
            depths,
            blended,
            blends,
            items,
            groups,
            sorted: _,
        } = scratch;
        [
            (segments.as_ptr().addr(), segments.capacity()),
            (last.as_ptr().addr(), last.capacity()),
            (matrices.as_ptr().addr(), matrices.capacity()),
            (depths.as_ptr().addr(), depths.capacity()),
            (blended.as_ptr().addr(), blended.capacity()),
            (blends.as_ptr().addr(), blends.capacity()),
            (items.as_ptr().addr(), items.capacity()),
            (groups.as_ptr().addr(), groups.capacity()),
        ]
    }

    /// A shader for triangles whose parameter is their color, which blends as `blend` says, added
    /// to `shaders`.
    fn flat_color(shaders: &mut Shaders, blend: Blend) -> Shader<[f32; 3], Vec4> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let flat = shaders.add::<[f32; 3], Vec4>("flat color", &fixture, blend);
        flat.expect("the shader compiles")
    }

    /// A triangle, added to `meshes`.
    fn add_triangle(meshes: &mut Meshes) -> Mesh<[f32; 3]> {
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = meshes.add("triangle", &corners, &[0, 1, 2]);
        triangle.expect("a mesh")
    }

    /// A triangle, and a shader for it that blends, whose parameter is its color: a mesh that
    /// draws back to front by its z, as sprites do.
    fn flat_triangle() -> (Mesh<[f32; 3]>, Shader<[f32; 3], Vec4>) {
        let flat = flat_color(&mut Shaders::default(), Blend::Alpha);
        (add_triangle(&mut Meshes::default()), flat)
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
        let (triangle, flat) = flat_triangle();
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
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Scratch::default(),
        );
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
        let (triangle, flat) = flat_triangle();
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
        draw_world(&world, &Past::default(), 1.0, &mut draws, &mut scratch);
        assert_eq!(draws, want);
        let before = held(&scratch);
        let mut again = Draws::default();
        draw_world(&world, &Past::default(), 1.0, &mut again, &mut scratch);
        assert_eq!(again, want);
        assert_eq!(held(&scratch), before, "drawing again takes no more memory");
    }

    /// Sprites, some with meshes, at one z across four tables, drawn after each change by a scratch
    /// kept from frame to frame and by a fresh one, which draw the same each time: after nothing,
    /// which takes no more memory; after the world is replaced by another spawned the same way,
    /// which has the same tables, rows and z's and as many changes, but other IDs; after entities
    /// despawned and others spawned in their tables, which keep their lengths, so that only which
    /// entity is in a row shows; after two entities trade tables; after a z is set, which moves no
    /// row; after two meshes are made opaque, one is set to another mesh, its z is set, and it's set
    /// to blend again; and after a spawn and a despawn alone.
    #[test]
    fn a_kept_scratch_draws_as_a_fresh_one_does() {
        let (triangle, flat) = flat_triangle();
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
            draw_world(world, &Past::default(), 1.0, &mut draws, kept);
            let mut fresh = Draws::default();
            draw_world(
                world,
                &Past::default(),
                1.0,
                &mut fresh,
                &mut Scratch::default(),
            );
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
        let other = of(true, false);
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
        // Two meshes made opaque, which draw first, by their meshes; one set to another mesh, which
        // swaps them without moving a row or changing a z; its z set, which changes nothing; and
        // set to blend again.
        let opaque = flat_color(&mut Shaders::default(), Blend::Opaque);
        let mut meshes = Meshes::default();
        let others: [_; 3] = std::array::from_fn(|_| add_triangle(&mut meshes));
        let set = |world: &mut World, entity, draw| {
            *world.get_mut::<MeshDraw>(entity).expect("meshed") = draw;
        };
        set(
            &mut world,
            lowered,
            MeshDraw::new(others[0], opaque, Vec4::ONE),
        );
        set(
            &mut world,
            other,
            MeshDraw::new(others[1], opaque, Vec4::ONE),
        );
        step(&world, &mut kept, "two meshes made opaque");
        set(
            &mut world,
            lowered,
            MeshDraw::new(others[2], opaque, Vec4::ONE),
        );
        step(&world, &mut kept, "an opaque mesh set to another mesh");
        z(&mut world, 0.25);
        step(&world, &mut kept, "an opaque mesh's z set");
        set(
            &mut world,
            lowered,
            MeshDraw::new(triangle, flat, Vec4::ONE),
        );
        step(&world, &mut kept, "an opaque mesh set to blend");
        entities.push(spawn(&mut world, true, false));
        step(&world, &mut kept, "a spawn");
        world.despawn(entities.swap_remove(1)).expect("alive");
        step(&world, &mut kept, "a despawn");
    }

    /// Two opaque meshes of one batch, each in a table of its own, the later table's with the
    /// smaller ID, which draw as one group of the batch, by ID; then that one set to another mesh,
    /// which splits them into two batches without moving their segments: a scratch kept from frame
    /// to frame draws them as a fresh one does each time.
    #[test]
    fn a_kept_scratch_sees_a_batch_split() {
        let opaque = flat_color(&mut Shaders::default(), Blend::Opaque);
        let mut meshes = Meshes::default();
        let [first, second] = [(); 2].map(|()| add_triangle(&mut meshes));
        let draw = |mesh| MeshDraw::new(mesh, opaque, Vec4::ONE);
        let mut world = World::default();
        let pair = [0.0, 1.0].map(|x| world.spawn((at_x(x, 0.5), draw(first))));
        let smaller = pair.iter().min_by_key(|&&entity| world.id(entity));
        let smaller = *smaller.expect("two");
        world.insert(smaller, Marked).expect("alive");
        let drawn = |world: &World, scratch: &mut Scratch| {
            let mut draws = Draws::default();
            draw_world(world, &Past::default(), 1.0, &mut draws, scratch);
            draws
        };
        let mut kept = Scratch::default();
        for what in ["one batch", "the batch split"] {
            if what == "the batch split" {
                *world.get_mut::<MeshDraw>(smaller).expect("meshed") = draw(second);
            }
            let fresh = drawn(&world, &mut Scratch::default());
            assert_eq!(drawn(&world, &mut kept), fresh, "after {what}");
        }
    }

    /// Draws the world's opaque meshes first, whatever their z's, by shader, then mesh, then ID,
    /// across tables and under a parent, so that each mesh with each shader is a run of its own;
    /// then the meshes that blend, and the sprites, back to front.
    #[test]
    fn opaque_meshes_draw_first_by_shader_and_mesh_then_id() {
        let (mut shaders, mut meshes) = (Shaders::default(), Meshes::default());
        let opaque = [Blend::Opaque; 2].map(|blend| flat_color(&mut shaders, blend));
        let blended = flat_color(&mut shaders, Blend::Alpha);
        let triangles = [(); 2].map(|()| add_triangle(&mut meshes));
        let mut world = World::default();
        // Each told apart by its x and its color: its shader's and its mesh's place among the
        // fixture's, its entity, where it draws and what.
        let mut want = Vec::new();
        for i in 0..16 {
            let (shader, mesh) = (i % 2, i / 2 % 2);
            let at = at_x(i as f32, [0.3, 0.1, 0.5, 0.0][i / 4]);
            let draw = MeshDraw::new(triangles[mesh], opaque[shader], Vec4::splat(i as f32));
            let entity = if i % 3 == 0 {
                world.spawn((at, draw, Marked))
            } else {
                world.spawn((at, draw))
            };
            want.push((shader, mesh, entity, at.matrix(), draw));
        }
        let pivot_at = Transform {
            rotation: Quat::from_rotation_z(0.5),
            ..at_x(-1.0, 0.2)
        };
        let pivot = world.spawn(pivot_at);
        let child_at = at_x(0.5, 0.05);
        let draw = MeshDraw::new(triangles[1], opaque[1], Vec4::splat(16.0));
        let child = world.spawn((child_at, draw));
        world.set_parent(child, Some(pivot)).expect("no cycle");
        want.push((1, 1, child, pivot_at.matrix() * child_at.matrix(), draw));
        let sprite = |n: f32| Sprite {
            color: Vec4::splat(n),
            ..Sprite::default()
        };
        let (near, far) = (at_x(17.0, 0.1), at_x(18.0, 0.0));
        world.spawn((near, sprite(17.0)));
        world.spawn((far, sprite(18.0)));
        let blending_at = at_x(19.0, 0.15);
        let blending = MeshDraw::new(triangles[0], blended, Vec4::splat(19.0));
        world.spawn((blending_at, blending));

        want.sort_by_key(|&(shader, mesh, entity, ..)| (shader, mesh, world.id(entity)));
        let mut expected = Draws::default();
        for (.., at, draw) in &want {
            expected.push_draw(*at, draw);
        }
        expected.push_sprite(far.matrix(), sprite(18.0));
        expected.push_sprite(near.matrix(), sprite(17.0));
        expected.push_draw(blending_at.matrix(), &blending);
        let mut draws = Draws::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Scratch::default(),
        );
        assert_eq!(draws, expected);
    }

    /// A transform at `x` and `z`.
    fn at_x(x: f32, z: f32) -> Transform {
        Transform {
            translation: Vec3::new(x, 0.0, z),
            ..Transform::IDENTITY
        }
    }

    /// What `world` draws with `scratch`, `alpha` of the way on from `past`, found for it.
    fn between_ticks(world: &World, past: &mut Past, alpha: f32, scratch: &mut Scratch) -> Draws {
        past.resolve(world);
        let mut draws = Draws::default();
        draw_world(world, past, alpha, &mut draws, scratch);
        draws
    }

    /// Roots blended in runs of rows on the job system's threads draw as each row's own blend has
    /// them, bit for bit, on a pool of four threads and outside one: 5,000 movers, which isn't a
    /// whole number of runs.
    #[test]
    fn blending_on_threads_draws_each_row_its_own_way() {
        let mut world = World::default();
        let mut movers: Vec<(Entity, Transform)> = (0..5000)
            .map(|i| {
                let at = at_x(i as f32, 0.0);
                (world.spawn((at, Sprite::default())), at)
            })
            .collect();
        let mut past = Past::default();
        past.capture(&world);
        for (i, &(entity, _)) in movers.iter().enumerate() {
            let to = world.get_mut::<Transform>(entity).expect("alive");
            to.translation.x += 1.0 + i as f32 / 1000.0;
        }
        movers.sort_by_key(|&(entity, _)| world.id(entity));
        let mut want = Draws::default();
        for &(entity, from) in &movers {
            let to = world.get::<Transform>(entity).expect("alive");
            let drawn = interpolation::between(&from, to, 0.3).matrix();
            want.push_sprite(drawn, Sprite::default());
        }
        let bits = |draws: &Draws| format!("{draws:?}");
        let one = between_ticks(&world, &mut past, 0.3, &mut Scratch::default());
        assert_eq!(bits(&one), bits(&want), "outside a pool");
        let four = jobs::on_threads(4, || {
            between_ticks(&world, &mut past, 0.3, &mut Scratch::default())
        });
        assert_eq!(bits(&four), bits(&want), "on four threads");
    }

    /// A child of a pivot that turns a quarter turn a tick and draws nothing draws between ticks
    /// on its orbit, by the pivot's transform drawn between them, then its own: not on the chord
    /// between where the two ticks put it, as blending its world matrices would.
    #[test]
    fn children_blend_their_locals_then_compose() {
        let mut world = World::default();
        let pivot = world.spawn(Transform::IDENTITY);
        let local = at_x(1.0, 0.0);
        let child = world.spawn((local, Sprite::default()));
        world.set_parent(child, Some(pivot)).expect("alive");
        let mut past = Past::default();
        past.capture(&world);
        let turned = Quat::from_rotation_z(FRAC_PI_2);
        world.get_mut::<Transform>(pivot).expect("alive").rotation = turned;
        let draws = between_ticks(&world, &mut past, 0.5, &mut Scratch::default());
        let halfway = Transform {
            rotation: Quat::IDENTITY.lerp(turned, 0.5),
            ..Transform::IDENTITY
        };
        let mut want = Draws::default();
        want.push_sprite(halfway.matrix() * local.matrix(), Sprite::default());
        assert_eq!(draws, want);
    }

    /// Drawing between ticks keeps the order the latest tick sorted by: a sprite whose z passes
    /// another's draws over it from the tick that passes it, though it's drawn below it on its way,
    /// and so does a child, whose own transform blends under its parent's. What didn't move draws
    /// bit for bit as the tick left it, at a z of -0.0 too, where a lerp of equal ends would round,
    /// or lose the zero's sign; a mesh blends as a sprite does; and a scratch kept between frames
    /// draws as a fresh one does.
    #[test]
    fn blending_keeps_the_order_the_ticks_sorted() {
        let (triangle, flat) = flat_triangle();
        let sprite = |n: f32| Sprite {
            color: Vec4::splat(n),
            ..Sprite::default()
        };
        // Where glam's lerp of equal ends rounds, at the fractions drawn, and a turn that its
        // normalizing changes.
        let still = Transform {
            translation: Vec3::new(3.7, -1.9, 0.5),
            rotation: Quat::from_xyzw(0.0, 0.0, 0.006_849_946_5, 0.999_976_5),
            scale: Vec3::splat(0.123),
        };
        let signed = Transform {
            translation: Vec3::new(-1.9, 3.7, -0.0),
            rotation: Quat::from_rotation_z(-1.3),
            scale: Vec3::new(0.123, 0.123, 1.0),
        };
        let mut world = World::default();
        world.spawn((still, sprite(1.0)));
        world.spawn((signed, sprite(2.0)));
        let passer = world.spawn((at_x(0.0, 0.375), sprite(3.0)));
        let mesh = MeshDraw::new(triangle, flat, Vec4::ONE);
        let meshed = world.spawn((at_x(0.0, 0.9), mesh));
        // Children of a parent that stays where it is, one passing the other's z.
        let parent = world.spawn(at_x(10.0, 0.0));
        let [_, passing] = [(0.75, 30.0), (0.6875, 31.0)].map(|(z, n)| {
            let child = world.spawn((at_x(0.0, z), sprite(n)));
            world.set_parent(child, Some(parent)).expect("alive");
            child
        });
        let mut swarm: Vec<(Entity, f32)> = (0..20)
            .map(|i| {
                let i = i as f32;
                (world.spawn((at_x(i, 0.1), sprite(4.0 + i))), i)
            })
            .collect();
        let mut past = Past::default();
        past.capture(&world);
        let mut moved = |entity: Entity, to: Transform| {
            *world.get_mut::<Transform>(entity).expect("alive") = to;
        };
        moved(passer, at_x(0.0, 0.625));
        moved(meshed, at_x(4.0, 0.9));
        moved(passing, at_x(0.0, 0.8125));
        for &(entity, i) in &swarm {
            moved(entity, at_x(i + 4.0, 0.1));
        }
        swarm.sort_by_key(|&(entity, _)| world.id(entity));
        // glam's lerp, as drawing computes it for what moves.
        let lerp = |from: f32, to: f32, alpha: f32| from * (1.0 - alpha) + to * alpha;
        // Bit for bit, which `Debug` shows of a zero's sign, and `==` doesn't.
        let bits = |draws: &Draws| format!("{draws:?}");
        let mut kept = Scratch::default();
        for alpha in [0.3, 0.7] {
            // Back to front by the tick's z: the one at -0.0, the swarm at 0.1 in ID order, the
            // still one at 0.5, the passer at 0.625, the children at 0.75 and 0.8125, then the
            // mesh at 0.9.
            let mut want = Draws::default();
            want.push_sprite(signed.matrix(), sprite(2.0));
            for &(_, i) in &swarm {
                let drifted = at_x(lerp(i, i + 4.0, alpha), 0.1);
                want.push_sprite(drifted.matrix(), sprite(4.0 + i));
            }
            want.push_sprite(still.matrix(), sprite(1.0));
            let passer = at_x(0.0, lerp(0.375, 0.625, alpha));
            want.push_sprite(passer.matrix(), sprite(3.0));
            let parent = at_x(10.0, 0.0).matrix();
            want.push_sprite(parent * at_x(0.0, 0.75).matrix(), sprite(30.0));
            let passing = at_x(0.0, lerp(0.6875, 0.8125, alpha));
            want.push_sprite(parent * passing.matrix(), sprite(31.0));
            want.push_draw(at_x(lerp(0.0, 4.0, alpha), 0.9).matrix(), &mesh);
            let draws = between_ticks(&world, &mut past, alpha, &mut kept);
            assert_eq!(bits(&draws), bits(&want), "at {alpha}, a scratch kept");
            let draws = between_ticks(&world, &mut past, alpha, &mut Scratch::default());
            assert_eq!(bits(&draws), bits(&want), "at {alpha}");
        }
    }
}
