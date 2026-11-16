//! Streaming meshes, textures and materials to the GPU, within a frame's budget.

use std::sync::Arc;
use std::time::Instant;

use gg_core::{Error, Result};
use gg_gpu::{Device, Frame, GpuBuffer, MemoryBudget, Texture as GpuTexture, TextureFormat};
use tracing::{debug, warn};

use super::registry::MeshletParts;
use super::{Change, GpuMesh, Held, Lods, MeshPass};
use crate::packed::MeshletBounds;
use crate::texture::{self, Map, Texels};
use crate::{Streaming, Uploads, model};

/// A texture's slot: the texture in it, what its texels stand for, which says which stand-in the
/// slot shows without an image of its own, whether it was removed, which leaves it showing the
/// stand-in for good, as stale handles may still sample it, and how many times it has come to show
/// another image, its own or the stand-in, which tells what it shows apart from what it showed.
pub(super) struct TextureSlot {
    texture: GpuTexture,
    kind: texture::Kind,
    removed: bool,
    shown: u64,
}

/// The textures the renderer's slots show until their own images are there: a pixel of white for
/// colors, so that a sprite shows its tint, and for a surface's map, so that a material shows its
/// factors; a pixel of zeros for data, so that text draws nothing rather than boxes; and a normal
/// pointing straight out for a normal map, so that a surface shows as its vertices shape it.
/// Written in the first frame that begins, before any slot shows them.
pub(super) struct StandIns {
    white: GpuTexture,
    zeros: GpuTexture,
    flat: GpuTexture,
    written: bool,
}

impl StandIns {
    pub(super) fn new(device: &Arc<Device>) -> Result<Self> {
        let white = TextureFormat::Rgba8Srgb;
        let data = TextureFormat::Rgba8Unorm;
        Ok(Self {
            white: GpuTexture::new(device, 1, 1, white, false, "white stand-in")?,
            zeros: GpuTexture::new(device, 1, 1, data, false, "zero stand-in")?,
            flat: GpuTexture::new(device, 1, 1, data, false, "flat normal stand-in")?,
            written: false,
        })
    }

    /// The stand-in for a texture whose texels stand for `kind`.
    fn of(&self, kind: texture::Kind) -> &GpuTexture {
        match kind {
            texture::Kind::Color | texture::Kind::Map(Map::Surface) => &self.white,
            texture::Kind::Data => &self.zeros,
            texture::Kind::Map(Map::Normal) => &self.flat,
        }
    }

    /// Writes them, in the first frame.
    fn write(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        if self.written {
            return Ok(());
        }
        frame.write_texture(&mut self.white, [0, 0], [1, 1], &[255; 4])?;
        frame.write_texture(&mut self.zeros, [0, 0], [1, 1], &[0; 4])?;
        // X and Y of 0, which a byte holds as nearly as 128 does.
        frame.write_texture(&mut self.flat, [0, 0], [1, 1], &[128, 128, 255, 255])?;
        self.written = true;
        Ok(())
    }
}

/// The table of materials, by their handles, in the GPU's own memory: written as materials are added
/// and replaced, in the frame's own commands, before its passes, so that every draw from that
/// frame on reads what was written; and made again, twice the size, as more are added than it
/// holds, the old one retiring until the GPU is done with it. What it holds is kept on the CPU
/// too, for a table made again to start from.
pub(super) struct MaterialTable {
    /// The table, and how many materials it holds.
    pub(super) table: Option<(GpuBuffer, usize)>,
    pub(super) entries: Vec<model::Entry>,
}

impl MaterialTable {
    /// The fewest materials a table holds.
    const LEAST: usize = 64;

    /// Writes `changes`, taking them, each a material's index and its entry, into the table, which
    /// it makes, or makes again larger, as they need: one copy of every entry from the first that
    /// changed to the last, or of them all into a table made again.
    ///
    /// # Panics
    ///
    /// If a material's index is past those before it, as they are where they came from another
    /// `Materials` than those before them.
    fn write(
        &mut self,
        device: &Arc<Device>,
        frame: &mut Frame<'_>,
        changes: &mut Vec<(u32, model::Entry)>,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        // The first entry that changed, and the one past the last.
        let (mut first, mut end) = (usize::MAX, 0);
        for (index, entry) in changes.drain(..) {
            let index = index as usize;
            match index.cmp(&self.entries.len()) {
                std::cmp::Ordering::Less => self.entries[index] = entry,
                std::cmp::Ordering::Equal => self.entries.push(entry),
                std::cmp::Ordering::Greater => {
                    panic!("a renderer draws with the materials of one `Materials`")
                }
            }
            (first, end) = (first.min(index), end.max(index + 1));
        }
        let size = size_of::<model::Entry>();
        let len = self.entries.len();
        if self.table.as_ref().is_none_or(|&(_, room)| room < len) {
            let room = len.next_power_of_two().max(Self::LEAST);
            // The table that's there, if any, retires until the GPU is done with the frames that
            // read it.
            let table = GpuBuffer::new(device, (room * size) as u64, "materials")?;
            self.table = Some((table, room));
            (first, end) = (0, len);
        }
        let (table, _) = self.table.as_mut().expect("just made");
        let bytes = bytemuck::cast_slice(&self.entries[first..end]);
        frame.write_buffer(table, (first * size) as u64, bytes)
    }

    /// The address of the table, which shaders read the materials through: 0 before any is added.
    fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// What culls the meshlets of the meshes drawn by them, in one table in the GPU's own memory,
/// which the culling reaches through one address, as `shaders/meshlets.slang` takes every address
/// from its push constants: each mesh's meshlets a run of entries, added as the mesh comes to be
/// drawable, and written in the frame's own commands, before its passes; the table made again,
/// twice the size, as it fills, the old one retiring until the GPU is done with it, from what's kept
/// of it on the CPU. A mesh removed or replaced leaves its run unused, which no mesh takes again:
/// the table grows by every mesh with meshlets a run adds, as a texture's slot isn't given out
/// again, until reusing runs is worth it, as for a game that streams levels in and out.
pub(super) struct MeshletTable {
    /// The table, and how many entries it has room for.
    pub(super) table: Option<(GpuBuffer, usize)>,
    pub(super) entries: Vec<MeshletBounds>,
    /// How many of `entries` the table holds.
    pub(super) written: usize,
}

impl MeshletTable {
    /// The fewest entries a table holds.
    const LEAST: usize = 1024;

    /// Adds `bounds`, a mesh's meshlets', and returns where they start.
    fn add(&mut self, bounds: &[MeshletBounds]) -> u32 {
        let first = self.entries.len();
        self.entries.extend_from_slice(bounds);
        u32::try_from(first).expect("fewer than 2^32 meshlets")
    }

    /// Writes the entries added since it last wrote into the table, which it makes, or makes again
    /// larger, as they need: one copy of those added, or of them all into a table made again.
    fn write(&mut self, device: &Arc<Device>, frame: &mut Frame<'_>) -> Result<()> {
        let len = self.entries.len();
        if self.written == len {
            return Ok(());
        }
        let size = size_of::<MeshletBounds>();
        if self.table.as_ref().is_none_or(|&(_, room)| room < len) {
            let room = len.next_power_of_two().max(Self::LEAST);
            let table = GpuBuffer::new(device, (room * size) as u64, "meshlet bounds")?;
            self.table = Some((table, room));
            self.written = 0;
        }
        let (table, _) = self.table.as_mut().expect("just made");
        let bytes = bytemuck::cast_slice(&self.entries[self.written..]);
        frame.write_buffer(table, (self.written * size) as u64, bytes)?;
        self.written = len;
        Ok(())
    }

    /// The address of the table: 0 before any meshlet is added.
    pub(super) fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// Where a mesh's meshlets are in the [`MeshletTable`], and each level's.
pub(crate) struct PlacedMeshlets {
    /// The mesh's first entry.
    pub(crate) first: u32,
    /// Each level's meshlets among the mesh's: the first, and how many.
    pub(crate) levels: Vec<[u32; 2]>,
    /// The most meshlets a level has.
    pub(crate) most: u32,
}

/// An asset being streamed to the GPU.
pub(super) struct Item {
    index: u32,
    name: String,
    /// The bytes it takes on the GPU, which every part copies some of.
    total: u64,
    /// Whether it has warned that it waits for room in the GPU's memory.
    warned: bool,
    /// When it was queued.
    queued: Instant,
    asset: Asset,
}

enum Asset {
    Mesh {
        /// Its bytes, kept until the last part is copied.
        vertices: Vec<u8>,
        indices: Vec<u32>,
        /// How many bytes its vertices are, the first of `total`, and its indices the rest.
        vertex_bytes: u64,
        vertex_size: usize,
        lods: Lods,
        meshlets: Option<MeshletParts>,
        skinned: Option<u32>,
        /// Its buffers, once there's room for them: boxed, so that the queue's items stay small.
        buffers: Option<Box<(GpuBuffer, GpuBuffer)>>,
    },
    Texture {
        size: [u32; 2],
        texels: Texels,
        /// Its pixels, kept until the last part is copied, with the mip levels made for them once
        /// its image is.
        pixels: Vec<u8>,
        /// Whether its image is made, once there's room for it.
        made: bool,
    },
}

impl Item {
    /// Whether it's `kind`'s asset of handle `index`.
    fn is(&self, kind: Kind, index: u32) -> bool {
        let is_mesh = matches!(self.asset, Asset::Mesh { .. });
        self.index == index && is_mesh == (kind == Kind::Mesh)
    }

    /// Whether its buffers or image are made, so that it's being copied, or copied and waiting to
    /// be drawable.
    fn made(&self) -> bool {
        match &self.asset {
            Asset::Mesh { buffers, .. } => buffers.is_some(),
            Asset::Texture { made, .. } => *made,
        }
    }

    /// Logs that it's drawable, `renderer.mesh_streamed` or `renderer.texture_streamed`, at the
    /// debug level, as there may be many: its bytes on the GPU, and how long since it was queued,
    /// as it waited for its turn and for room and was copied, a texture's levels made among it if
    /// its loader didn't make them.
    fn streamed(&self) {
        let ms = gg_core::log::millis(self.queued.elapsed());
        match &self.asset {
            Asset::Mesh { .. } => debug!(
                code = "renderer.mesh_streamed",
                mesh = &*self.name,
                bytes = self.total,
                ms,
                "Mesh streamed"
            ),
            Asset::Texture { size, .. } => debug!(
                code = "renderer.texture_streamed",
                texture = &*self.name,
                ?size,
                bytes = self.total,
                ms,
                "Texture streamed"
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mesh,
    Texture,
}

/// How many submissions of copies on the transfer queue may be unfinished as a frame begins
/// before the frame copies nothing ([`Frame::uploads_behind`]): with two frames in flight, a copy
/// engine that keeps up leaves at most the last frame's unfinished, so two means it has fallen a
/// frame or more behind, as over a slow link or under contention. Holding off then bounds what's
/// in flight to two frames' parts, their staging and their command buffers, with one more frame's
/// being recorded, whatever the frame rate.
const MAX_UPLOADS_BEHIND: u64 = 2;

/// The bytes a frame may copy of `budget` with `behind` submissions of copies unfinished as it
/// began: none once the transfer queue is [`MAX_UPLOADS_BEHIND`] behind, so that the frame neither
/// copies nor makes buffers or images, but takes what's drawable.
pub(super) fn budget_behind(behind: u64, budget: u64) -> u64 {
    if behind >= MAX_UPLOADS_BEHIND {
        0
    } else {
        budget
    }
}

/// Whether an asset of `bytes` would take the GPU's memory past its budget, given `budget` and
/// how much is in `usage`.
pub(super) fn exceeds(usage: u64, budget: u64, bytes: u64) -> bool {
    usage.saturating_add(bytes) > budget
}

/// Whether an asset of `bytes` waits for room in the GPU's memory, given `budget`, how much is in
/// `usage`, and whether anything of the queue is `copying`, made and not yet drawable: it waits
/// while it would take usage past the budget, as removals make room, unless nothing is copying,
/// when it goes ahead alone, so that a game comes up on a GPU whose memory is past the budget
/// already, as other processes' can take it, an asset at a time; the next asset past the budget
/// waits until this one is drawable.
pub(super) fn waits(usage: u64, budget: u64, bytes: u64, copying: bool) -> bool {
    exceeds(usage, budget, bytes) && copying
}

/// Whether the asset `name`, of `bytes`, waits in the queue for room in the GPU's memory, as
/// [`waits`] decides from what `device` reports: warning the first time it's past the budget,
/// which `warned` keeps, whether it waits or goes ahead alone.
fn waits_for_room(
    device: &Device,
    name: &str,
    bytes: u64,
    copying: bool,
    warned: &mut bool,
) -> bool {
    let MemoryBudget { usage, budget } = device.memory_budget();
    if !exceeds(usage, budget, bytes) {
        return false;
    }
    let waits = waits(usage, budget, bytes, copying);
    if !*warned {
        let goes = if waits {
            "waits for room, until removals make some or nothing else is being copied"
        } else {
            "goes ahead alone, as nothing else is being copied"
        };
        warn!(
            code = "renderer.over_budget",
            asset = name,
            bytes,
            usage,
            budget,
            "{name:?}'s {bytes} bytes would take the {usage} in use in the GPU's memory past the \
             budget of {budget}: it {goes}"
        );
        *warned = true;
    }
    waits
}

/// Whether making a buffer or an image failed for want of memory, which [`room_for`] has the
/// asset wait out rather than end the renderer: the budget it's gated by is an estimate.
fn out_of_memory(error: &Error) -> bool {
    error.code() == "gpu.out_of_memory"
}

/// What `made`, the buffers or image of the asset `name`, of `bytes`, gave: `None` if the GPU's
/// memory had no room, which warns the first time, `warned` keeping it, and leaves the asset to
/// wait, to be tried again as room comes; any other error as it is.
pub(super) fn room_for<T>(
    made: Result<T>,
    name: &str,
    bytes: u64,
    warned: &mut bool,
) -> Result<Option<T>> {
    match made {
        Ok(made) => Ok(Some(made)),
        Err(error) if out_of_memory(&error) => {
            if !*warned {
                warn!(
                    code = "renderer.out_of_memory",
                    asset = name,
                    bytes,
                    "{name:?}'s {bytes} bytes don't fit the GPU's memory: {error:#}; it waits, \
                     and is tried again as room comes"
                );
                *warned = true;
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The buffers of the mesh `name`, `vertex_bytes` of vertices and `index_bytes` of indices.
fn mesh_buffers(
    device: &Arc<Device>,
    name: &str,
    vertex_bytes: u64,
    index_bytes: u64,
) -> Result<(GpuBuffer, GpuBuffer)> {
    let vertices = GpuBuffer::new(device, vertex_bytes, &format!("{name} vertices"))?;
    // Should the indices not fit, the vertices' buffer retires with the error.
    let indices = GpuBuffer::new(device, index_bytes, &format!("{name} indices"))?;
    Ok((vertices, indices))
}

impl MeshPass {
    /// Takes the changes to the meshes, textures and materials in `uploads`, writes the materials
    /// into their table and the scene's changes into its, and streams what's queued to the GPU,
    /// within the frame's budget, which must happen before the frame's passes. A texture
    /// added gets its slot, which shows a stand-in; its pixels, and a mesh's vertices and indices,
    /// queue up behind what was added before, and each frame copies parts of the queue in order,
    /// making an asset's buffers or image as its turn comes, if the GPU's memory has room for
    /// them, as [`waits`] and [`room_for`] decide, and otherwise leaving it to wait, warning once,
    /// while those after it go on; a frame the transfer queue has fallen behind copies nothing
    /// ([`budget_behind`]). A mesh draws once its buffers are there, and a texture's slot shows
    /// its image once that is; one replaced keeps drawing what it had until then, and whatever of
    /// it was still being streamed is dropped. A mesh or texture removed draws nothing, or shows
    /// its stand-in, from this frame on, and whatever of it was queued is dropped.
    ///
    /// # Panics
    ///
    /// If they were taken from another `Meshes` or `Textures` than the pass's meshes and textures
    /// before them.
    pub(crate) fn upload(&mut self, frame: &mut Frame<'_>, uploads: &mut Uploads) -> Result<()> {
        let _zone = gg_core::zone!();
        self.stand_ins.write(frame)?;
        for change in uploads.meshes.drain(..) {
            self.mesh_changed(change);
        }
        for change in uploads.textures.drain(..) {
            self.texture_changed(frame, change)?;
        }
        (self.materials).write(&self.device, frame, &mut uploads.materials)?;
        self.scene.write(&self.device, frame)?;
        self.stream(frame)?;
        // The meshlets of the meshes that came to be drawable.
        self.meshlet_table.write(&self.device, frame)
    }

    /// The address of the table of materials, which the frame's data carries: 0 before any is
    /// added.
    pub(crate) fn materials_address(&self) -> u64 {
        self.materials.address()
    }

    /// Takes a change to the meshes: queues what was added, or removes a mesh.
    fn mesh_changed(&mut self, change: Change) {
        let added = match change {
            Change::Added(added) => added,
            Change::Removed(index) => {
                let held = self.meshes.get_mut(index as usize);
                let held = held.expect("a renderer draws the meshes of one `Meshes`");
                // The buffers retire until the GPU is done with the frames that draw them.
                *held = Held::Removed;
                self.streaming.retain(|item| !item.is(Kind::Mesh, index));
                return;
            }
        };
        let index = added.index as usize;
        // A mesh replaced is one the pass has; any other is the next its `Meshes` added.
        let follows = if added.replacement {
            index < self.meshes.len()
        } else {
            index == self.meshes.len()
        };
        assert!(follows, "a renderer draws the meshes of one `Meshes`");
        if !added.replacement {
            self.meshes.push(Held::Waiting);
        }
        debug_assert!(
            !matches!(self.meshes[index], Held::Removed),
            "its `Meshes` drops a removed mesh's replacement"
        );
        // In place of any of the mesh still queued, whose buffers go with it.
        self.streaming
            .retain(|item| !item.is(Kind::Mesh, added.index));
        let vertex_bytes = added.vertices.len() as u64;
        let total = vertex_bytes + (added.indices.len() * size_of::<u32>()) as u64;
        self.streaming.push_back(Item {
            index: added.index,
            name: added.name,
            total,
            warned: false,
            queued: Instant::now(),
            asset: Asset::Mesh {
                vertices: added.vertices,
                indices: added.indices,
                vertex_bytes,
                vertex_size: added.vertex_size,
                lods: added.lods,
                meshlets: added.meshlets,
                skinned: added.skinned,
                buffers: None,
            },
        });
    }

    /// Takes a change to the textures: makes a slot's texture, queues an image, or removes a
    /// texture.
    fn texture_changed(&mut self, frame: &mut Frame<'_>, change: texture::Change) -> Result<()> {
        let Self {
            device,
            textures,
            stand_ins,
            streaming,
            ..
        } = self;
        match change {
            texture::Change::Reserved { index, name, kind } => {
                assert_eq!(
                    index as usize,
                    textures.len(),
                    "a renderer samples the textures of one `Textures`"
                );
                // In the slot of its handle's index, which shaders sample it by.
                let texture = GpuTexture::placed(device, index, stand_ins.of(kind), &name)?;
                textures.push(TextureSlot {
                    texture,
                    kind,
                    removed: false,
                    shown: 0,
                });
            }
            texture::Change::Image(added) => {
                let slot = textures.get_mut(added.index as usize);
                let slot = slot.expect("a renderer samples the textures of one `Textures`");
                debug_assert!(
                    !slot.removed,
                    "its `Textures` drops a removed texture's images"
                );
                // In place of any image of the texture still queued, whose image, if it was made,
                // retires with whatever was copied into it.
                streaming.retain(|item| !item.is(Kind::Texture, added.index));
                slot.texture.discard();
                let (format, mipmapped) = (added.texels.format(), added.texels.mipmapped());
                streaming.push_back(Item {
                    index: added.index,
                    name: added.name,
                    total: format.bytes(added.size, mipmapped),
                    warned: false,
                    queued: Instant::now(),
                    asset: Asset::Texture {
                        size: added.size,
                        texels: added.texels,
                        pixels: added.pixels,
                        made: false,
                    },
                });
            }
            texture::Change::Removed(index) => {
                let slot = textures.get_mut(index as usize);
                let slot = slot.expect("a renderer samples the textures of one `Textures`");
                debug_assert!(!slot.removed, "its `Textures` removes a texture once");
                streaming.retain(|item| !item.is(Kind::Texture, index));
                frame.hide_texture(&mut slot.texture, stand_ins.of(slot.kind));
                slot.removed = true;
                slot.shown += 1;
            }
        }
        Ok(())
    }

    /// Copies the next parts of what's queued, in order, within the frame's budget, or none if the
    /// transfer queue has fallen behind, and takes what has come to be drawable off the queue,
    /// into its place.
    fn stream(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        if self.streaming.is_empty() {
            return Ok(());
        }
        let _zone = gg_core::zone!();
        let budget = budget_behind(frame.uploads_behind(), self.budget);
        let mut copying = self.streaming.iter().any(Item::made);
        let mut spent = 0;
        let mut i = 0;
        while i < self.streaming.len() {
            if self.advance(frame, i, budget, &mut spent, &mut copying)? {
                if let Some(item) = self.streaming.remove(i) {
                    item.streamed();
                }
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Streams the `i`th queued asset on: makes its buffers or image if there's room, which sets
    /// `copying`, copies its next parts while the frame's bytes `spent` are under `budget`, and
    /// once every part is there, takes it for drawing. Returns whether it's drawable, and so done.
    fn advance(
        &mut self,
        frame: &mut Frame<'_>,
        i: usize,
        budget: u64,
        spent: &mut u64,
        copying: &mut bool,
    ) -> Result<bool> {
        let Self {
            device,
            meshes,
            textures,
            streaming,
            meshlet_table,
            ..
        } = self;
        let item = &mut streaming[i];
        match &mut item.asset {
            Asset::Mesh {
                vertices,
                indices,
                vertex_bytes,
                vertex_size,
                lods,
                meshlets,
                skinned,
                buffers,
            } => {
                if buffers.is_none() {
                    if *spent >= budget
                        || waits_for_room(
                            device,
                            &item.name,
                            item.total,
                            *copying,
                            &mut item.warned,
                        )
                    {
                        return Ok(false);
                    }
                    let index_bytes = item.total - *vertex_bytes;
                    let made = mesh_buffers(device, &item.name, *vertex_bytes, index_bytes);
                    let Some(made) = room_for(made, &item.name, item.total, &mut item.warned)?
                    else {
                        return Ok(false);
                    };
                    *buffers = Some(Box::new(made));
                    *copying = true;
                }
                let (vertex_buffer, index_buffer) = &mut **buffers.as_mut().expect("just made");
                let copied = |vertices: &GpuBuffer, indices: &GpuBuffer| {
                    vertices.streamed() + indices.streamed()
                };
                while *spent < budget && copied(vertex_buffer, index_buffer) < item.total {
                    let part = if vertex_buffer.streamed() < *vertex_bytes {
                        frame.stream_buffer(vertex_buffer, vertices, budget - *spent)?
                    } else {
                        let bytes: &[u8] = bytemuck::cast_slice(indices);
                        frame.stream_buffer(index_buffer, bytes, budget - *spent)?
                    };
                    *spent += part;
                }
                if copied(vertex_buffer, index_buffer) < item.total {
                    return Ok(false);
                }
                // The last part is copied: the bytes go, whatever else waits.
                *vertices = Vec::new();
                *indices = Vec::new();
                let vertices_ready = frame.finish_buffer(vertex_buffer)?;
                let indices_ready = frame.finish_buffer(index_buffer)?;
                if !(vertices_ready && indices_ready) {
                    return Ok(false);
                }
                let (vertices, indices) = *buffers.take().expect("just there");
                let mesh = GpuMesh {
                    vertices,
                    indices,
                    lods: std::mem::replace(lods, Lods::whole(0)),
                    vertex_size: *vertex_size,
                    meshlets: meshlets.take().map(|parts| PlacedMeshlets {
                        first: meshlet_table.add(&parts.bounds),
                        most: (parts.levels.iter())
                            .map(|&[_, count]| count)
                            .max()
                            .unwrap_or(0),
                        levels: parts.levels,
                    }),
                    skinned: *skinned,
                };
                let held = &mut meshes[item.index as usize];
                // Which the draws' sizes are checked against: its own type, since only the
                // `Meshes` that added it replaces it.
                if let Some(old) = held.ready() {
                    assert_eq!(
                        old.vertex_size, mesh.vertex_size,
                        "mesh {:?} is replaced by vertices of another type; a renderer draws the \
                         meshes of one `Meshes`",
                        item.name
                    );
                }
                // The old buffers, if any, retire until the GPU is done with the frames that
                // draw them.
                *held = Held::Ready(mesh);
                Ok(true)
            }
            Asset::Texture {
                size,
                texels,
                pixels,
                made,
            } => {
                let slot = &mut textures[item.index as usize];
                let texture = &mut slot.texture;
                if !*made {
                    if *spent >= budget
                        || waits_for_room(
                            device,
                            &item.name,
                            item.total,
                            *copying,
                            &mut item.warned,
                        )
                    {
                        return Ok(false);
                    }
                    let [width, height] = *size;
                    // A replacement may be compressed where the image it replaces wasn't, or the
                    // other way round, as its `Textures` keeps only its kind.
                    let refilled =
                        texture.refill(width, height, texels.format(), texels.mipmapped());
                    if room_for(refilled, &item.name, item.total, &mut item.warned)?.is_none() {
                        return Ok(false);
                    }
                    // Once the image is there, so that a refill tried again doesn't make them again.
                    texture::add_levels(pixels, *size, *texels);
                    *made = true;
                    *copying = true;
                }
                while *spent < budget && texture.streamed() < item.total {
                    let part = frame.stream_texture(texture, pixels, budget - *spent)?;
                    *spent += part;
                }
                if texture.streamed() < item.total {
                    return Ok(false);
                }
                // The last part is copied: the pixels go, whatever else waits.
                *pixels = Vec::new();
                let finished = frame.finish_texture(texture)?;
                slot.shown += u64::from(finished);
                Ok(finished)
            }
        }
    }

    /// What `texture`'s slot shows, if it's an image of its own rather than a stand-in: its size,
    /// and how many times the slot has come to show another image, which tells this one apart
    /// from those it showed before. `None` for a texture that isn't the pass's.
    pub(crate) fn texture_shown(&self, texture: crate::Texture) -> Option<([u32; 2], u64)> {
        let slot = self.textures.get(texture.index as usize)?;
        let size = slot.texture.size();
        (size != [0, 0]).then_some((size, slot.shown))
    }

    /// How many bytes of the `item`th queued asset are copied.
    fn copied(&self, item: &Item) -> u64 {
        match &item.asset {
            Asset::Mesh { buffers, .. } => buffers
                .as_ref()
                .map_or(0, |buffers| buffers.0.streamed() + buffers.1.streamed()),
            Asset::Texture { .. } => self.textures[item.index as usize].texture.streamed(),
        }
    }

    /// What's being streamed.
    pub(crate) fn streaming(&self) -> Streaming {
        Streaming {
            assets: self.streaming.len(),
            bytes: (self.streaming.iter())
                .map(|item| item.total - self.copied(item))
                .sum(),
        }
    }
}
