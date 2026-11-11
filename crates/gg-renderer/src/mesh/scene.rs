//! The world's opaque meshes, kept on the GPU from frame to frame: [`Scene`], what's changed of
//! them, which the engine fills and [`Uploads`](crate::Uploads) carries to a renderer; and what a
//! renderer keeps of them, each instance's record in a table in the GPU's own memory, written as it
//! changes, and the order frames cull and draw them in, by batch, then ID, written as it changes.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use gg_core::Result;
use gg_core::math::{Mat3, Mat4};
use gg_gpu::{Device, Faces, Frame, GpuBuffer};

use crate::{MeshDraw, halves};

/// The world's opaque meshes, which a renderer keeps on the GPU from frame to frame, as what's
/// changed of them since [`Uploads::take_scene`](crate::Uploads::take_scene) last took it: each an
/// instance of a mesh with a shader that writes depth, its transform, its parameters and its ID,
/// in a slot its caller picks, set as it changes and taken away as it goes. Frames draw them where
/// their draws say [`Draws::draw_scene`](crate::Draws::draw_scene), culled, and at their levels of
/// detail, as the frames' other opaque runs are: by shader, then mesh, then the faces they draw,
/// then ID, as many instanced draws as their meshes have levels for each, whatever their
/// instances. A frame whose instances didn't change writes nothing of them. It keeps its memory,
/// so that a frame that changes no more than the last allocates nothing.
#[derive(Debug, Default)]
pub struct Scene {
    pub(crate) changes: Vec<Change>,
}

/// A slot's instance, as [`Scene::changes`] gives it: its transform, ID and draw.
type Set = (Mat4, u64, MeshDraw);

/// A change to a renderer's scene, which [`Uploads::take_scene`](crate::Uploads::take_scene)
/// carries to it.
#[derive(Debug)]
pub(crate) enum Change {
    Set {
        slot: u32,
        transform: Mat4,
        id: u64,
        draw: MeshDraw,
    },
    Remove(u32),
    Clear,
}

impl Scene {
    /// Puts in slot `slot` an instance of `draw`, which `transform` places in the world, with the
    /// ID `id`, in place of what the slot held: its mesh's fronts alone, if it culls, as
    /// [`Draws::push_draw`](crate::Draws::push_draw) draws them.
    ///
    /// # Panics
    ///
    /// If `draw`'s shader doesn't write depth.
    pub fn set(&mut self, slot: u32, transform: Mat4, draw: &MeshDraw, id: u64) {
        assert!(
            draw.batch().is_some(),
            "a scene's meshes draw with shaders that write depth"
        );
        self.changes.push(Change::Set {
            slot,
            transform,
            id,
            draw: *draw,
        });
    }

    /// Takes away the instance in slot `slot`, if it holds one.
    pub fn remove(&mut self, slot: u32) {
        self.changes.push(Change::Remove(slot));
    }

    /// Takes away every instance, forgetting what changed before.
    pub fn clear(&mut self) {
        self.changes.clear();
        self.changes.push(Change::Clear);
    }

    /// What's changed, in order: each slot set, with its transform, ID and draw, or emptied; and
    /// for a change that empties every slot, `None`. For tests, which read what changed.
    #[doc(hidden)]
    pub fn changes(&self) -> impl Iterator<Item = Option<(u32, Option<Set>)>> {
        self.changes.iter().map(|change| match *change {
            Change::Set {
                slot,
                transform,
                id,
                draw,
            } => Some((slot, Some((transform, id, draw)))),
            Change::Remove(slot) => Some((slot, None)),
            Change::Clear => None,
        })
    }
}

/// What an instance of the scene draws with, which orders its batch among the rest: its shader,
/// its mesh, and the faces it draws, as [`faces`] numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Batch {
    shader: u32,
    mesh: u32,
    faces: u8,
}

/// An instance, in the order frames draw them: its batch, its ID, and its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Member {
    batch: Batch,
    id: u64,
    slot: u32,
}

/// An instance's record, as `Record` in `shaders/gather.slang` has it: its transform,
/// column-major; the transform the frame it changed in found it at, its own for an instance new to
/// its slot; its ID's low and high halves; the number of the frame its transform last changed in,
/// its low and high words, which the gather takes the transform before from only in that frame,
/// and the transform after; and its parameters, up to [`MeshDraw::MAX_PARAMS`] bytes, the rest
/// zeros.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Record {
    transform: [f32; 16],
    previous: [f32; 16],
    id: [u32; 2],
    changed: [u32; 2],
    params: [u32; MeshDraw::MAX_PARAMS / 4],
}

/// The bytes of a [`Record`].
const RECORD: u64 = size_of::<Record>() as u64;

/// A run of the scene's instances of one batch, as frames cull and draw it: its mesh, its shader,
/// the faces it draws, and where its instances are in the order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SceneRun {
    pub(crate) mesh: u32,
    pub(crate) shader: u32,
    pub(crate) faces: Faces,
    pub(crate) first: u32,
    pub(crate) count: u32,
}

/// What a renderer keeps of its scene: each slot's instance, its record on the CPU and in a table
/// on the GPU, and the instances in order, by batch, then ID, on the CPU and on the GPU too. A
/// table on the GPU is made again, twice the size, as more slots are used than it holds, the old
/// retiring until the GPU is done with it, and written whole from the CPU's records.
#[derive(Default)]
pub(crate) struct Kept {
    /// By slot: its instance, if it holds one.
    slots: Vec<Option<Member>>,
    /// By slot: its record.
    records: Vec<Record>,
    /// By slot: whether its record changed since a frame last wrote it, and the slots that did.
    dirty: Vec<bool>,
    changed: Vec<u32>,
    /// The instances, in order.
    members: Vec<Member>,
    /// The instances set into a slot since `members` was last ordered, in place of another or of
    /// none, and whether any was taken away.
    added: Vec<Member>,
    removed: bool,
    /// What `members` merges into.
    merged: Vec<Member>,
    /// The runs of `members`, and their slots in order.
    runs: Vec<SceneRun>,
    order: Vec<u32>,
    /// Whether `order` changed since a frame last wrote it.
    reordered: bool,
    /// The table of records on the GPU, and how many it holds; and the order there likewise.
    table: Option<(GpuBuffer, usize)>,
    ordered: Option<(GpuBuffer, usize)>,
    /// The bytes and their offsets a frame writes into the table.
    bytes: Vec<u8>,
    offsets: Vec<u64>,
}

/// The fewest records or slots a table on the GPU holds.
const LEAST: usize = 1024;

/// What `faces` orders by, among a mesh's runs with a shader.
fn faces(faces: Faces) -> u8 {
    match faces {
        Faces::Both => 0,
        Faces::CounterClockwise => 1,
        Faces::Clockwise => 2,
    }
}

/// The faces `faces` numbers.
fn faces_of(faces: u8) -> Faces {
    match faces {
        0 => Faces::Both,
        1 => Faces::CounterClockwise,
        _ => Faces::Clockwise,
    }
}

impl Kept {
    /// Takes `changes`, in order, into what it keeps, as frame `frame`'s, the next to begin, and
    /// orders its instances again if any came, went or moved to another batch or ID: in a merge of
    /// those that came, sorted, into those that stayed, so that ordering them again costs the
    /// instances once and what came as it's sorted. An instance set again keeps the transform the
    /// first of the frame's changes found it at, which the frame before drew it at, a frame that
    /// doesn't begin taking its changes on to the next, of the same number.
    pub(crate) fn apply(&mut self, changes: &mut Vec<Change>, frame: u64) {
        if changes.is_empty() {
            return;
        }
        let _zone = gg_core::zone!();
        for change in changes.drain(..) {
            match change {
                Change::Set {
                    slot,
                    transform,
                    id,
                    draw,
                } => {
                    let at = slot as usize;
                    if self.slots.len() <= at {
                        self.slots.resize(at + 1, None);
                        self.records.resize(at + 1, Record::zeroed());
                        self.dirty.resize(at + 1, false);
                    }
                    // As `Draws::push_draw` draws it.
                    let faces = if !draw.culls {
                        Faces::Both
                    } else if Mat3::from_mat4(transform).determinant() < 0.0 {
                        Faces::Clockwise
                    } else {
                        Faces::CounterClockwise
                    };
                    let member = Member {
                        batch: Batch {
                            shader: draw.shader,
                            mesh: draw.mesh,
                            faces: self::faces(faces),
                        },
                        id,
                        slot,
                    };
                    // Where the frame before drew it: as the record held it before the frame's
                    // first change, if the slot held its entity, or else where it is now.
                    let held = self.records[at];
                    let changed = halves(frame);
                    let previous = match self.slots[at] {
                        Some(held_member) if held_member.id == id && held.changed == changed => {
                            held.previous
                        }
                        Some(held_member) if held_member.id == id => held.transform,
                        _ => transform.to_cols_array(),
                    };
                    if self.slots[at] != Some(member) {
                        self.removed |= self.slots[at].is_some();
                        self.slots[at] = Some(member);
                        self.added.push(member);
                    }
                    let mut params = [0; MeshDraw::MAX_PARAMS / 4];
                    bytemuck::cast_slice_mut::<u32, u8>(&mut params)[..draw.params().len()]
                        .copy_from_slice(draw.params());
                    self.records[at] = Record {
                        transform: transform.to_cols_array(),
                        previous,
                        id: halves(id),
                        changed,
                        params,
                    };
                    if !self.dirty[at] {
                        self.dirty[at] = true;
                        self.changed.push(slot);
                    }
                }
                Change::Remove(slot) => {
                    if let Some(held) = self.slots.get_mut(slot as usize) {
                        self.removed |= held.take().is_some();
                    }
                }
                Change::Clear => {
                    self.slots.fill(None);
                    self.removed = true;
                    self.added.clear();
                }
            }
        }
        if self.added.is_empty() && !self.removed {
            return;
        }
        // Those that stayed are still in order; those that came, in theirs, merge into them.
        let slots = &self.slots;
        let held = |member: &Member| slots[member.slot as usize] == Some(*member);
        self.members.retain(held);
        self.added.retain(held);
        self.added.sort_unstable();
        self.added.dedup();
        self.merged.clear();
        let (mut stayed, mut came) = (self.members.iter().peekable(), self.added.iter().peekable());
        loop {
            let next = match (stayed.peek(), came.peek()) {
                (Some(&&a), Some(&&b)) if b < a => came.next(),
                (Some(_), _) => stayed.next(),
                (None, Some(_)) => came.next(),
                (None, None) => break,
            };
            self.merged.extend(next.copied());
        }
        // One set again as it was into a slot these changes emptied, as a clear empties each, both
        // stayed and came: an instance is drawn once.
        self.merged.dedup();
        std::mem::swap(&mut self.members, &mut self.merged);
        self.added.clear();
        self.removed = false;
        self.order.clear();
        self.order
            .extend(self.members.iter().map(|member| member.slot));
        self.runs.clear();
        let mut first = 0;
        for batch in self.members.chunk_by(|a, b| a.batch == b.batch) {
            let Batch {
                shader,
                mesh,
                faces,
            } = batch[0].batch;
            let count = batch.len() as u32;
            self.runs.push(SceneRun {
                mesh,
                shader,
                faces: faces_of(faces),
                first,
                count,
            });
            first += count;
        }
        self.reordered = true;
    }

    /// Its runs, in the order frames draw them.
    pub(crate) fn runs(&self) -> &[SceneRun] {
        &self.runs
    }

    /// Writes into its tables on the GPU what changed since a frame last did: the records that
    /// changed, each where its slot is, and the order whole, if it changed; or every record, into
    /// a table made again as more slots are used than the last held, or a first. Before the frame's
    /// passes, which read them.
    pub(crate) fn write(&mut self, device: &Arc<Device>, frame: &mut Frame<'_>) -> Result<()> {
        let slots = self.slots.len();
        if self.table.as_ref().is_none_or(|&(_, room)| room < slots) && slots > 0 {
            let room = slots.next_power_of_two().max(LEAST);
            // The table that's there, if any, retires until the GPU is done with the frames that
            // read it.
            let table = GpuBuffer::new(device, room as u64 * RECORD, "scene")?;
            self.table = Some((table, room));
            // Every record, written whole below.
            for (slot, dirty) in self.dirty.iter_mut().enumerate() {
                if !*dirty {
                    *dirty = true;
                    self.changed.push(slot as u32);
                }
            }
        }
        if !self.changed.is_empty()
            && let Some((table, _)) = &mut self.table
        {
            let _zone = gg_core::zone!("write scene");
            self.bytes.clear();
            self.offsets.clear();
            for &slot in &self.changed {
                self.bytes
                    .extend_from_slice(bytemuck::bytes_of(&self.records[slot as usize]));
                self.offsets.push(u64::from(slot) * RECORD);
                self.dirty[slot as usize] = false;
            }
            self.changed.clear();
            frame.write_buffer_pieces(table, RECORD, &self.offsets, &self.bytes)?;
        }
        if self.reordered && !self.order.is_empty() {
            let len = self.order.len();
            if self.ordered.as_ref().is_none_or(|&(_, room)| room < len) {
                let room = len.next_power_of_two().max(LEAST);
                let ordered = GpuBuffer::new(device, room as u64 * 4, "scene order")?;
                self.ordered = Some((ordered, room));
            }
            let (ordered, _) = self.ordered.as_mut().expect("just made");
            frame.write_buffer(ordered, 0, bytemuck::cast_slice(&self.order))?;
            self.reordered = false;
        }
        Ok(())
    }

    /// The addresses of its table of records and of its order on the GPU, 0 for one not made.
    pub(crate) fn addresses(&self) -> [u64; 2] {
        let address = |table: &Option<(GpuBuffer, usize)>| {
            table.as_ref().map_or(0, |(table, _)| table.address())
        };
        [address(&self.table), address(&self.ordered)]
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec3;

    use super::*;

    /// A draw of mesh `n` with opaque shader `n`.
    fn draw(n: u32) -> MeshDraw {
        MeshDraw::from_parts(n, n, true, false, &7u32.to_ne_bytes()).expect("four bytes")
    }

    fn runs(kept: &Kept) -> Vec<(u32, u32, Faces, Vec<u32>)> {
        let order = |run: &SceneRun| {
            kept.order[run.first as usize..(run.first + run.count) as usize].to_vec()
        };
        (kept.runs.iter())
            .map(|run| (run.shader, run.mesh, run.faces, order(run)))
            .collect()
    }

    /// The scene's instances are ordered by shader, then mesh, then faces, then ID, whatever their
    /// slots, as they come, go, and move to other batches; a run for each batch; and a slot set
    /// again with what it held keeps its place, its record alone changed.
    #[test]
    fn instances_order_by_batch_then_id_as_they_come_and_go() {
        let [a, b] = [draw(0), draw(1)];
        let culls = MeshDraw { culls: true, ..a };
        let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let mut scene = Scene::default();
        let mut kept = Kept::default();
        scene.set(4, Mat4::IDENTITY, &b, 9);
        scene.set(0, Mat4::IDENTITY, &a, 30);
        scene.set(7, Mat4::IDENTITY, &a, 20);
        scene.set(2, mirror, &culls, 5);
        scene.set(3, Mat4::IDENTITY, &culls, 6);
        kept.apply(&mut scene.changes, 1);
        let (both, ccw, cw) = (Faces::Both, Faces::CounterClockwise, Faces::Clockwise);
        let want = vec![
            (0, 0, both, vec![7, 0]),
            (0, 0, ccw, vec![3]),
            (0, 0, cw, vec![2]),
            (1, 1, both, vec![4]),
        ];
        assert_eq!(runs(&kept), want);

        // Moved, its record alone changes.
        kept.reordered = false;
        scene.set(7, Mat4::from_translation(Vec3::X), &a, 20);
        kept.apply(&mut scene.changes, 2);
        assert!(!kept.reordered);
        assert_eq!(kept.records[7].transform[12], 1.0);

        scene.remove(0);
        scene.set(5, Mat4::IDENTITY, &a, 1);
        scene.set(4, Mat4::IDENTITY, &a, 40);
        kept.apply(&mut scene.changes, 3);
        let want = vec![
            (0, 0, both, vec![5, 7, 4]),
            (0, 0, ccw, vec![3]),
            (0, 0, cw, vec![2]),
        ];
        assert_eq!(runs(&kept), want);

        scene.clear();
        scene.set(1, Mat4::IDENTITY, &b, 3);
        kept.apply(&mut scene.changes, 4);
        assert_eq!(runs(&kept), vec![(1, 1, both, vec![1])]);
    }

    /// An instance set again into its slot once the slot's emptied, by a clear, as every one is
    /// when the engine plays or stops a copy of the world, or taken away in the same frame's
    /// changes, is ordered once, as it was.
    #[test]
    fn an_instance_set_again_once_its_slot_is_emptied_is_ordered_once() {
        let a = draw(0);
        let mut scene = Scene::default();
        let mut kept = Kept::default();
        scene.set(0, Mat4::IDENTITY, &a, 1);
        scene.set(1, Mat4::IDENTITY, &a, 2);
        kept.apply(&mut scene.changes, 1);
        let want = vec![(0, 0, Faces::Both, vec![0, 1])];
        assert_eq!(runs(&kept), want);

        scene.clear();
        scene.set(0, Mat4::IDENTITY, &a, 1);
        scene.set(1, Mat4::IDENTITY, &a, 2);
        kept.apply(&mut scene.changes, 2);
        assert_eq!(runs(&kept), want, "after a clear");

        scene.remove(1);
        scene.set(1, Mat4::IDENTITY, &a, 2);
        kept.apply(&mut scene.changes, 3);
        assert_eq!(runs(&kept), want, "after a removal");
    }

    /// A record keeps the transform the frame before drew its instance at, from the first of a
    /// frame's changes, and the frame it changed in; an instance new to its slot, another entity's
    /// before, starts where it is.
    #[test]
    fn records_keep_where_the_frame_before_drew_them() {
        let a = draw(0);
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let mut scene = Scene::default();
        let mut kept = Kept::default();
        let record = |kept: &Kept| {
            let record = kept.records[3];
            (record.transform[12], record.previous[12], record.changed)
        };
        scene.set(3, at(1.0), &a, 9);
        kept.apply(&mut scene.changes, 5);
        assert_eq!(record(&kept), (1.0, 1.0, [5, 0]));

        // Moved in a frame, then again in it, as a frame that didn't begin leaves its changes.
        scene.set(3, at(2.0), &a, 9);
        kept.apply(&mut scene.changes, 6);
        scene.set(3, at(3.0), &a, 9);
        kept.apply(&mut scene.changes, 6);
        assert_eq!(record(&kept), (3.0, 1.0, [6, 0]));

        // In the next frame, from where that one left it.
        scene.set(3, at(4.0), &a, 9);
        kept.apply(&mut scene.changes, 7);
        assert_eq!(record(&kept), (4.0, 3.0, [7, 0]));

        // Another entity in the slot starts where it is.
        scene.set(3, at(8.0), &a, 10);
        kept.apply(&mut scene.changes, 1 << 32 | 8);
        assert_eq!(record(&kept), (8.0, 8.0, [8, 1]));
    }
}
