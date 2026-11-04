//! The engine's own system, drawing the world's entities.

use std::ops::Range;

use gg_core::math::{Mat4, Quat, Vec3};
use gg_renderer::{Circle, Draws, MeshDraw, Shaders, Sprite};

use crate::ecs::ComponentId;
use crate::font::Fonts;
use crate::interpolation::{self, Past};
use crate::jobs;
use crate::{EntityId, Parent, Text, Transform, World};

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
    /// Its rows' z, as [`depth`] gives it, or 0 for opaque meshes, which draw in any order.
    z: f32,
    /// Its rows' [`MeshDraw::batch`], for opaque meshes, or 0.
    batch: u64,
    /// Which segment it was found as: tables in the order they were made, then a table's sprites,
    /// circles, texts and meshes, then its rows in order.
    order: u32,
    kind: Kind,
    table: u32,
    rows: Range<u32>,
    /// Where its table's world matrices start among the scratch's, for a table of children;
    /// [`ROOTS`] for a table of roots, whose rows draw by their transforms, or by `blended` from
    /// where `blends` says, while they draw between ticks.
    matrices: u32,
}

/// The same segment, its z bit for bit, as `total_cmp` tells z's apart, so that a NaN is the same
/// as itself.
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

/// What a row draws: opaque meshes first, whatever their z, then, at one z, sprites, then circles,
/// then text, under the meshes that blend.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Opaque,
    Sprite,
    Circle,
    Text,
    Mesh,
}

/// A row drawn: its entity's ID, which orders it among the rows of its batch, or of its z and kind,
/// and which it draws with, and where it is.
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

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`], a [`Circle`],
/// a [`Text`] or a [`MeshDraw`]: first the meshes whose shaders are opaque, whose depth hides
/// what's behind them whatever order they draw in, by their [`MeshDraw::batch`], so that each mesh
/// with each shader is one instanced draw, split only where a culling mesh's draws change between
/// mirrored and not; then the rest, which blend, back to front by z in the world, as the latest
/// tick left them, -0.0 and 0.0 one z, at one z sprites, then circles, then text, under meshes;
/// each batch and each z in the order of their entities' IDs, so that entities that overlap at one
/// depth keep how they stack as others come and go. Circles draw with the engine's circle shader
/// and text with its text shader, which `shaders` adds the first time any do; text is laid out in
/// its font of `fonts`, as [`Fonts::lay_out`] says. Each draws `alpha` of the way from what `past`
/// has it draw from to where the tick left it, while it draws between ticks; at 1, where the tick
/// left it. Each draw's ID is its entity's, which picks read back where it shows. A child draws
/// where its ancestors carry it, by its world matrix, its transform's and each of theirs drawn so,
/// which it computes into `scratch` for each child once a frame; roots, in tables without a
/// [`Parent`], draw by their transforms alone, which those that draw between ticks blend first, in
/// their tables' order, in runs of rows that the job system's threads take.
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
    shaders: &mut Shaders,
    fonts: &mut Fonts,
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
    let Some(poses) = Poses::new(world, past, alpha) else {
        return;
    };
    let transform = poses.transform;
    let sprite = world.component::<Sprite>();
    let circle = world.component::<Circle>();
    let text = world.component::<Text>();
    let mesh = world.component::<MeshDraw>();
    let parent = world.component::<Parent>();
    // How many of `blended`'s matrices the frame's roots take.
    let mut blending = 0;
    for (index, rows) in world.tables().iter().enumerate() {
        let Some(transforms) = rows.column::<Transform>(transform) else {
            continue;
        };
        let drawn = [
            (Kind::Sprite, sprite),
            (Kind::Circle, circle),
            (Kind::Text, text),
            (Kind::Mesh, mesh),
        ]
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
                let (from, instead) = (poses.from(index), poses.past.instead_of(index));
                if from.is_some() || !instead.is_empty() {
                    let end = blending + len;
                    if blended.len() < end {
                        blended.resize(end, Mat4::IDENTITY);
                    }
                    let out = &mut blended[blending..end];
                    match from {
                        Some(from) => blend(out, &from[..len], transforms, alpha),
                        None => {
                            for (out, local) in out.iter_mut().zip(transforms) {
                                *out = local.matrix();
                            }
                        }
                    }
                    for &(_, row, pose) in instead {
                        out[row as usize] = pose.matrix();
                    }
                    blends[index] = Some(blending);
                    blending = end;
                }
            }
            Some(parents) => {
                let start = matrices.len();
                depths.clear();
                for (row, (local, parent)) in transforms.iter().zip(parents).enumerate() {
                    let drawn = poses.drawn(index, row, local);
                    let (at, drawn) = poses.carry(local, drawn, parent);
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
    let mut sprites_at = [None; AT_HAND];
    let (mut circles_at, mut meshes_at) = ([None; AT_HAND], [None; AT_HAND]);
    let mut texts_at = [None; AT_HAND];
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
                    (matrix(transforms, drawn, row), sprites[row], item.id.get())
                }));
            }
            Kind::Circle => {
                let circle = circle.expect("a table has circles");
                let columns = |index: u32| {
                    let (table, transforms, drawn) = placed(index);
                    let circles = table.column::<Circle>(circle).expect("it has circles");
                    (transforms, circles, drawn)
                };
                let circles = items.iter().map(|item| {
                    let found = found(&mut circles_at, item.segment, columns);
                    let (transforms, circles, drawn) = found;
                    let row = item.row as usize;
                    (matrix(transforms, drawn, row), circles[row], item.id.get())
                });
                draws.extend_circles(shaders, circles);
            }
            Kind::Text => {
                let text = text.expect("a table has texts");
                let columns = |index: u32| {
                    let (table, transforms, drawn) = placed(index);
                    let texts = table.column::<Text>(text).expect("it has texts");
                    (transforms, texts, drawn)
                };
                for item in items {
                    let found = found(&mut texts_at, item.segment, columns);
                    let (transforms, texts, drawn) = found;
                    let row = item.row as usize;
                    let at = matrix(transforms, drawn, row);
                    let shown = &texts[row];
                    let style = shown.style();
                    let glyphs = fonts.lay_out(at, &shown.text, &style, item.id.get());
                    draws.extend_glyphs(shaders, glyphs);
                }
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
                    let at = matrix(transforms, drawn, row);
                    draws.push_draw(at, &meshes[row], item.id.get());
                }
            }
        }
    }
}

/// Where the world's entities draw this frame: each `alpha` of the way from what `past` has it draw
/// from to where the latest tick left it, while it draws between ticks, or as `past` has it draw
/// instead, as [`Past::instead`] says; and children where their ancestors carry them as they draw.
pub(crate) struct Poses<'a> {
    world: &'a World,
    past: &'a Past,
    alpha: f32,
    /// [`Transform`]'s id among the world's components.
    transform: ComponentId,
}

impl<'a> Poses<'a> {
    /// `None` for a world without transforms, of which nothing draws.
    pub(crate) fn new(world: &'a World, past: &'a Past, alpha: f32) -> Option<Self> {
        Some(Self {
            world,
            past,
            alpha,
            transform: world.component::<Transform>()?,
        })
    }

    /// What table `table`'s rows draw from, from its first row on, while they draw between ticks.
    fn from(&self, table: usize) -> Option<&'a [Transform]> {
        (self.alpha < 1.0)
            .then(|| self.past.from(self.world, table))
            .flatten()
    }

    /// The pose row `row` of table `table` draws as, its transform `local` as the latest tick left
    /// it: as the past has it draw instead, if it does, or else on its way from what it draws from,
    /// while it draws between ticks; `None` where it draws as the tick left it.
    fn drawn(&self, table: usize, row: usize, local: &Transform) -> Option<Transform> {
        let instead = self.past.instead_of(table);
        if !instead.is_empty()
            && let Ok(at) = instead.binary_search_by_key(&(row as u32), |&(_, of, _)| of)
        {
            return Some(instead[at].2);
        }
        let from = self.from(table)?;
        Some(interpolation::between(&from[row], local, self.alpha))
    }

    /// A child's matrices from its own space to the world's, as the latest tick left it and as it
    /// draws, from its transform, `drawn` as it draws: its transform's, then each ancestor's in
    /// turn up to its root, an ancestor without a transform carrying its children as though it
    /// were at the origin, unturned, at its own size. The second is `None` while it's the first,
    /// when nothing on the way draws between ticks. It walks up from each child, every frame, so
    /// children of one parent each take the steps they share, and a deep hierarchy costs its depth
    /// for each of them, until world transforms are stored and propagated, once bodies go under a
    /// parent or a profile shows these walks.
    fn carry(
        &self,
        local: &Transform,
        drawn: Option<Transform>,
        parent: &Parent,
    ) -> (Mat4, Option<Mat4>) {
        let start = (local.matrix(), drawn.map(|drawn| drawn.matrix()));
        self.world
            .lineage(*parent)
            .fold(start, |(at, drawn), (index, table, row)| {
                let Some(transforms) = table.column::<Transform>(self.transform) else {
                    return (at, drawn);
                };
                let ancestor = transforms[row].matrix();
                let drawn = match self.drawn(index, row, &transforms[row]) {
                    Some(pose) => Some(pose.matrix() * drawn.unwrap_or(at)),
                    None => drawn.map(|drawn| ancestor * drawn),
                };
                (ancestor * at, drawn)
            })
    }

    /// Row `row` of table `table`, which has transforms: its transform, as the latest tick left it
    /// and drawn between ticks while it is, and its parent, if it has one.
    fn row(
        &self,
        table: usize,
        row: usize,
    ) -> (&'a Transform, Option<Transform>, Option<&'a Parent>) {
        let rows = &self.world.tables()[table];
        let transforms = rows.column::<Transform>(self.transform);
        let local = &transforms.expect("it has transforms")[row];
        let drawn = self.drawn(table, row, local);
        let parent = self.world.component::<Parent>();
        let parents = parent.and_then(|id| rows.column::<Parent>(id));
        (local, drawn, parents.map(|parents| &parents[row]))
    }

    /// The matrix row `row` of table `table`, which has transforms, draws by this frame, as
    /// [`draw_world`] draws it, bit for bit: its transform's, drawn between ticks while it is, and
    /// carried by its ancestors as they draw.
    pub(crate) fn matrix(&self, table: usize, row: usize) -> Mat4 {
        let (local, drawn, parent) = self.row(table, row);
        match parent {
            None => drawn.unwrap_or(*local).matrix(),
            Some(parent) => {
                let (at, drawn) = self.carry(local, drawn, parent);
                drawn.unwrap_or(at)
            }
        }
    }

    /// Where row `row` of table `table`, which has transforms, draws this frame, for a viewpoint to
    /// see from: its position, where a sprite of its transform draws, bit for bit; and its
    /// rotation, its own turned by each ancestor's in turn as they draw, their scales aside, so
    /// that a mirrored ancestor mirrors where it is but not which way it faces.
    pub(crate) fn pose(&self, table: usize, row: usize) -> (Vec3, Quat) {
        let (local, drawn, parent) = self.row(table, row);
        let Some(parent) = parent else {
            let drawn = drawn.unwrap_or(*local);
            return (drawn.translation, drawn.rotation);
        };
        let (at, carried) = self.carry(local, drawn, parent);
        let own = drawn.unwrap_or(*local).rotation;
        let lineage = self.world.lineage(*parent);
        let rotation = lineage.fold(own, |rotation, (index, table, row)| {
            let Some(transforms) = table.column::<Transform>(self.transform) else {
                return rotation;
            };
            let ancestor = self.drawn(index, row, &transforms[row]);
            ancestor.unwrap_or(transforms[row]).rotation * rotation
        });
        // Normalized, since each product of unit quaternions rounds a little off a unit.
        let position = carried.unwrap_or(at).w_axis.truncate();
        (position, rotation.normalize())
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
        None => (kind, 0, depth(z(row))),
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

/// `z` as drawing orders by it: -0.0 as 0.0, since they're one depth, and every other z as it is,
/// bit for bit, which `total_cmp` orders the same way on every machine. Compared rather than added
/// to 0.0, which would leave a NaN's bits to the platform.
fn depth(z: f32) -> f32 {
    if z == 0.0 { 0.0 } else { z }
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
    use std::iter;
    use std::path::Path;

    use gg_core::math::{Quat, Vec3, Vec4};
    use gg_renderer::{Blend, Mesh, Meshes, Shader, Shaders, Textures};

    use super::*;
    use crate::{Component, Entity};

    #[derive(Clone, Copy, Component)]
    struct Marked;

    /// Draws `sprite` of entity `id` into `draws`, where `matrix` places it, as drawing the world
    /// does: with the entity's ID.
    fn sprite_of(draws: &mut Draws, matrix: Mat4, sprite: Sprite, id: EntityId) {
        draws.extend_sprites(iter::once((matrix, sprite, id.get())));
    }

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
        let id = |entity: &Entity| world.id(*entity).expect("alive");
        for expected in &expected {
            match expected {
                Expected::Sprite(entity, at, sprite) => {
                    sprite_of(&mut want, at.matrix(), *sprite, id(entity));
                }
                Expected::Mesh(entity, at, mesh) => {
                    want.push_draw(at.matrix(), mesh, id(entity).get());
                }
            }
        }
        let mut draws = Draws::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        assert_eq!(draws, want);
    }

    /// Sprites and meshes that blend, at 0.0 and -0.0 in turn, draw as one z, all the sprites under
    /// all the meshes, each in the order of their IDs, whichever zero each is at; and a zero that
    /// changes only its sign changes nothing, for a scratch kept from frame to frame too.
    #[test]
    fn both_zeros_are_one_z() {
        let (triangle, flat) = flat_triangle();
        let mut world = World::default();
        let (mut sprites, mut meshes) = (Vec::new(), Vec::new());
        // Each told apart by its x and its color.
        for (i, z) in [0.0, -0.0, -0.0, 0.0, -0.0, 0.0].into_iter().enumerate() {
            let n = i as f32;
            let at = at_x(n, z);
            let sprite = Sprite {
                color: Vec4::splat(n),
                ..Sprite::default()
            };
            sprites.push((world.spawn((at, sprite)), at, sprite));
            let mesh = MeshDraw::new(triangle, flat, Vec4::splat(n));
            meshes.push((world.spawn((at, mesh)), at, mesh));
        }
        sprites.sort_by_key(|&(entity, ..)| world.id(entity));
        meshes.sort_by_key(|&(entity, ..)| world.id(entity));
        let mut want = Draws::default();
        let id = |entity: Entity| world.id(entity).expect("alive");
        for &(entity, at, sprite) in &sprites {
            sprite_of(&mut want, at.matrix(), sprite, id(entity));
        }
        for &(entity, at, mesh) in &meshes {
            want.push_draw(at.matrix(), &mesh, id(entity).get());
        }
        let drawn = |world: &World, scratch: &mut Scratch| {
            let mut draws = Draws::default();
            let shaders = &mut Shaders::default();
            draw_world(
                world,
                &Past::default(),
                1.0,
                &mut draws,
                shaders,
                &mut Fonts::new(&mut Textures::default()),
                scratch,
            );
            draws
        };
        let mut kept = Scratch::default();
        assert_eq!(drawn(&world, &mut kept), want);
        // `==` takes -0.0 for 0.0, so the matrices drawn match whichever zero they're at.
        let (flipped, ..) = sprites[0];
        let at = world.get_mut::<Transform>(flipped).expect("alive");
        at.translation.z = -at.translation.z;
        assert_eq!(drawn(&world, &mut kept), want, "a scratch kept");
        assert_eq!(drawn(&world, &mut Scratch::default()), want);
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
        let lowest = world.spawn((low, sprite(1.0)));
        let high = placed(2.0, 0.5, 0.0, [1.0; 2]);
        let highest = world.spawn((high, sprite(2.0)));
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
        // order of their IDs, which they draw with.
        let id = |entity: Entity| world.id(entity).expect("alive");
        let [lowest, middle_id, grand, meshed, kid, highest] =
            [lowest, middle, grand, meshed, kid, highest].map(id);
        let mut twins = [0, 1].map(|i| (id(twins[i]), twins_at[i]));
        twins.sort_by_key(|&(id, _)| id);
        let [pivot, middle] = [pivot_at.matrix(), middle_at.matrix()];
        let twins = twins.map(|(id, at)| (id, pivot * (middle * at.matrix())));
        let mut want = Draws::default();
        sprite_of(&mut want, low.matrix(), sprite(1.0), lowest);
        sprite_of(&mut want, pivot * middle, sprite(4.0), middle_id);
        let grandchild = pivot * (middle * grand_at.matrix());
        sprite_of(&mut want, grandchild, sprite(3.0), grand);
        let meshed_at = pivot * (middle * mesh_at.matrix());
        sprite_of(&mut want, meshed_at, sprite(5.0), meshed);
        want.push_draw(meshed_at, &mesh, meshed.get());
        for (id, twin) in twins {
            sprite_of(&mut want, twin, sprite(7.0), id);
        }
        for (id, twin) in twins {
            want.push_draw(twin, &mesh, id.get());
        }
        sprite_of(&mut want, pivot * kid_at.matrix(), sprite(6.0), kid);
        sprite_of(&mut want, high.matrix(), sprite(2.0), highest);
        let mut scratch = Scratch::default();
        let mut draws = Draws::default();
        let shaders = &mut Shaders::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut scratch,
        );
        assert_eq!(draws, want);
        let before = held(&scratch);
        let mut again = Draws::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut again,
            shaders,
            &mut Fonts::new(&mut Textures::default()),
            &mut scratch,
        );
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
            let shaders = &mut Shaders::default();
            draw_world(
                world,
                &Past::default(),
                1.0,
                &mut draws,
                shaders,
                &mut Fonts::new(&mut Textures::default()),
                kept,
            );
            let mut fresh = Draws::default();
            draw_world(
                world,
                &Past::default(),
                1.0,
                &mut fresh,
                &mut Shaders::default(),
                &mut Fonts::new(&mut Textures::default()),
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
            let shaders = &mut Shaders::default();
            draw_world(
                world,
                &Past::default(),
                1.0,
                &mut draws,
                shaders,
                &mut Fonts::new(&mut Textures::default()),
                scratch,
            );
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
        let nearer = world.spawn((near, sprite(17.0)));
        let farther = world.spawn((far, sprite(18.0)));
        let blending_at = at_x(19.0, 0.15);
        let blending = MeshDraw::new(triangles[0], blended, Vec4::splat(19.0));
        let blender = world.spawn((blending_at, blending));

        let id = |entity: Entity| world.id(entity).expect("alive");
        want.sort_by_key(|&(shader, mesh, entity, ..)| (shader, mesh, id(entity)));
        let mut expected = Draws::default();
        for &(_, _, entity, at, draw) in &want {
            expected.push_draw(at, &draw, id(entity).get());
        }
        sprite_of(&mut expected, far.matrix(), sprite(18.0), id(farther));
        sprite_of(&mut expected, near.matrix(), sprite(17.0), id(nearer));
        expected.push_draw(blending_at.matrix(), &blending, id(blender).get());
        let mut draws = Draws::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        assert_eq!(draws, expected);
    }

    /// Circles draw among the rest back to front by z, at one z over the sprites and under the
    /// meshes that blend, each in the order of their IDs, with their entities' IDs, and children
    /// where their ancestors carry them.
    #[test]
    fn circles_draw_over_sprites_and_under_meshes() {
        let (triangle, flat) = flat_triangle();
        let circle = |n: f32| Circle {
            color: Vec4::splat(n),
            ..Circle::default()
        };
        let sprite = |n: f32| Sprite {
            color: Vec4::splat(n),
            ..Sprite::default()
        };
        let mesh = MeshDraw::new(triangle, flat, Vec4::ONE);
        let mut world = World::default();
        let far = world.spawn((at_x(1.0, 0.2), circle(1.0)));
        let all = world.spawn((at_x(2.0, 0.5), sprite(2.0), circle(2.0), mesh));
        let near = world.spawn((at_x(3.0, 0.5), circle(3.0)));
        let parent = world.spawn(at_x(10.0, 0.0));
        let child = world.spawn((at_x(4.0, 0.3), circle(4.0)));
        world.set_parent(child, Some(parent)).expect("no cycle");

        // At 0.2, 0.3 and 0.5 in the world; at 0.5, the sprite, both circles by ID, then the mesh.
        let id = |entity: Entity| world.id(entity).expect("alive");
        let mut pair = [(all, 2.0), (near, 3.0)].map(|(entity, x)| (id(entity), x));
        pair.sort_by_key(|&(id, _)| id);
        let mut shaders = Shaders::default();
        let mut want = Draws::default();
        let carried = at_x(10.0, 0.0).matrix() * at_x(4.0, 0.3).matrix();
        let circles = [
            (at_x(1.0, 0.2).matrix(), 1.0, id(far)),
            (carried, 4.0, id(child)),
        ];
        for (matrix, n, id) in circles {
            want.extend_circles(&mut shaders, iter::once((matrix, circle(n), id.get())));
        }
        sprite_of(&mut want, at_x(2.0, 0.5).matrix(), sprite(2.0), id(all));
        let pair = pair.map(|(id, x)| (at_x(x, 0.5).matrix(), circle(x), id.get()));
        want.extend_circles(&mut shaders, pair.into_iter());
        want.push_draw(at_x(2.0, 0.5).matrix(), &mesh, id(all).get());
        let mut draws = Draws::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            &mut Scratch::default(),
        );
        assert_eq!(draws, want);
    }

    /// Text draws among the rest back to front by z, at one z over circles and under the meshes
    /// that blend, its glyphs its entity's draws, where its transform places it; one that names no
    /// font draws in the engine's own, which bakes as it's first drawn, its field waiting to be put
    /// in the texture every context keeps for it.
    #[test]
    fn text_draws_over_circles_and_under_meshes() {
        let (triangle, flat) = flat_triangle();
        let mesh = MeshDraw::new(triangle, flat, Vec4::ONE);
        let text = Text {
            text: "Hi".to_owned(),
            ..Text::default()
        };
        let mut world = World::default();
        let all = world.spawn((at_x(2.0, 0.5), Circle::default(), text.clone(), mesh));
        let far = world.spawn((at_x(1.0, 0.2), text.clone()));
        let id = |entity: Entity| world.id(entity).expect("alive").get();
        let mut textures = Textures::default();
        let mut fonts = Fonts::new(&mut textures);
        let mut shaders = Shaders::default();
        let mut want = Draws::default();
        let glyphs = |fonts: &mut Fonts, at: Transform, entity| {
            let laid: Vec<_> = fonts
                .lay_out(at.matrix(), &text.text, &text.style(), id(entity))
                .collect();
            assert_eq!(laid.len(), 2, "a glyph a letter");
            assert!(
                laid.iter()
                    .all(|(_, glyph, _)| glyph.field == fonts.inter_field())
            );
            laid
        };
        want.extend_glyphs(
            &mut shaders,
            glyphs(&mut fonts, at_x(1.0, 0.2), far).into_iter(),
        );
        let circle = (at_x(2.0, 0.5).matrix(), Circle::default(), id(all));
        want.extend_circles(&mut shaders, iter::once(circle));
        want.extend_glyphs(
            &mut shaders,
            glyphs(&mut fonts, at_x(2.0, 0.5), all).into_iter(),
        );
        want.push_draw(at_x(2.0, 0.5).matrix(), &mesh, id(all));
        let mut draws = Draws::default();
        let scratch = &mut Scratch::default();
        draw_world(
            &world,
            &Past::default(),
            1.0,
            &mut draws,
            &mut Shaders::default(),
            &mut fonts,
            scratch,
        );
        assert_eq!(draws, want);
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
        draw_world(
            world,
            past,
            alpha,
            &mut draws,
            &mut Shaders::default(),
            &mut Fonts::new(&mut Textures::default()),
            scratch,
        );
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
            let id = world.id(entity).expect("alive");
            sprite_of(&mut want, drawn, Sprite::default(), id);
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
        let drawn = halfway.matrix() * local.matrix();
        let id = world.id(child).expect("alive");
        sprite_of(&mut want, drawn, Sprite::default(), id);
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
        let stays = world.spawn((still, sprite(1.0)));
        let zero = world.spawn((signed, sprite(2.0)));
        let passer = world.spawn((at_x(0.0, 0.375), sprite(3.0)));
        let mesh = MeshDraw::new(triangle, flat, Vec4::ONE);
        let meshed = world.spawn((at_x(0.0, 0.9), mesh));
        // Children of a parent that stays where it is, one passing the other's z.
        let parent = world.spawn(at_x(10.0, 0.0));
        let [passed, passing] = [(0.75, 30.0), (0.6875, 31.0)].map(|(z, n)| {
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
        let id = |entity: Entity| world.id(entity).expect("alive");
        for alpha in [0.3, 0.7] {
            // Back to front by the tick's z: the one at -0.0, the swarm at 0.1 in ID order, the
            // still one at 0.5, the passer at 0.625, the children at 0.75 and 0.8125, then the
            // mesh at 0.9.
            let mut want = Draws::default();
            sprite_of(&mut want, signed.matrix(), sprite(2.0), id(zero));
            for &(entity, i) in &swarm {
                let drifted = at_x(lerp(i, i + 4.0, alpha), 0.1);
                sprite_of(&mut want, drifted.matrix(), sprite(4.0 + i), id(entity));
            }
            sprite_of(&mut want, still.matrix(), sprite(1.0), id(stays));
            let passer_at = at_x(0.0, lerp(0.375, 0.625, alpha));
            sprite_of(&mut want, passer_at.matrix(), sprite(3.0), id(passer));
            let parent = at_x(10.0, 0.0).matrix();
            let passed_at = parent * at_x(0.0, 0.75).matrix();
            sprite_of(&mut want, passed_at, sprite(30.0), id(passed));
            let passing_at = parent * at_x(0.0, lerp(0.6875, 0.8125, alpha)).matrix();
            sprite_of(&mut want, passing_at, sprite(31.0), id(passing));
            let meshed_at = at_x(lerp(0.0, 4.0, alpha), 0.9).matrix();
            want.push_draw(meshed_at, &mesh, id(meshed).get());
            let draws = between_ticks(&world, &mut past, alpha, &mut kept);
            assert_eq!(bits(&draws), bits(&want), "at {alpha}, a scratch kept");
            let draws = between_ticks(&world, &mut past, alpha, &mut Scratch::default());
            assert_eq!(bits(&draws), bits(&want), "at {alpha}");
        }
    }
}
