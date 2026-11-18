//! Streaming meshes, textures and materials to the GPU, within a frame's budget.

use std::sync::Arc;
use std::time::Instant;

use gg_core::{Error, Result};
use gg_gpu::{Device, Frame, GpuBuffer, MemoryBudget, Texture as GpuTexture, TextureFormat};
use tracing::{debug, warn};

use super::registry::{MeshletParts, Positions};
use super::{Change, GpuMesh, Held, Lods, MeshPass};
use crate::packed::MeshletBounds;
use crate::texture::{self, Map, Texels};
use crate::{Streaming, Uploads, model};

/// A texture's slot: its texture; its kind, which picks its stand-in; whether removed (showing the
/// stand-in for good, as stale handles may sample it); and how many times it has switched images.
pub(super) struct TextureSlot {
    texture: GpuTexture,
    kind: texture::Kind,
    removed: bool,
    shown: u64,
}

/// Shown until images arrive: white for colors (sprites show their tint) and surface maps
/// (materials show their factors), zeros for data (text draws nothing), a flat normal for normal
/// maps. Written in the first frame.
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

    fn of(&self, kind: texture::Kind) -> &GpuTexture {
        match kind {
            texture::Kind::Color | texture::Kind::Map(Map::Surface) => &self.white,
            texture::Kind::Data => &self.zeros,
            texture::Kind::Map(Map::Normal) => &self.flat,
        }
    }

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

/// The materials table, by handle, in GPU memory, written in the frame's commands before its
/// passes. Grown at twice the size from the CPU's copy, the old one retiring until the GPU is done
/// with it.
pub(super) struct MaterialTable {
    /// The table, and how many materials it holds.
    pub(super) table: Option<(GpuBuffer, usize)>,
    pub(super) entries: Vec<model::Entry>,
}

impl MaterialTable {
    /// The fewest materials a table holds.
    const LEAST: usize = 64;

    /// Writes `changes` (each a material's index and entry) into the table, making or growing it as
    /// needed: one copy from the first changed entry to the last, or of all into a grown table.
    ///
    /// # Panics
    ///
    /// - If a material's index skips past those before it, as from another `Materials`.
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
            // The old table retires until the GPU is done with the frames reading it.
            let table = GpuBuffer::new(device, (room * size) as u64, "materials")?;
            self.table = Some((table, room));
            (first, end) = (0, len);
        }
        let (table, _) = self.table.as_mut().expect("just made");
        let bytes = bytemuck::cast_slice(&self.entries[first..end]);
        frame.write_buffer(table, (first * size) as u64, bytes)
    }

    /// Shaders read the materials through it; 0 before any is added.
    fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// Every mesh's meshlet bounds, as one GPU table at one address (`shaders/meshlets.slang` takes
/// addresses from push constants). Written in the frame's commands before its passes; grown at
/// twice the size from the CPU's copy, the old one retiring. A removed or replaced mesh's run stays
/// unused, as texture slots do, so it grows with every mesh; reuse runs once a game streams levels
/// in and out.
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

    /// Adds a mesh's meshlets' `bounds`; returns where they start.
    fn add(&mut self, bounds: &[MeshletBounds]) -> u32 {
        let first = self.entries.len();
        self.entries.extend_from_slice(bounds);
        u32::try_from(first).expect("fewer than 2^32 meshlets")
    }

    /// Writes the entries added since last time, making or growing the table as needed: one copy of
    /// the new ones, or of all into a grown table.
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

    /// 0 before any meshlet is added.
    pub(super) fn address(&self) -> u64 {
        self.table.as_ref().map_or(0, |(table, _)| table.address())
    }
}

/// Where a mesh's meshlets are in the [`MeshletTable`], and each level's.
pub(crate) struct PlacedMeshlets {
    pub(crate) first: u32,
    /// Each level's first meshlet among the mesh's, and count.
    pub(crate) levels: Vec<[u32; 2]>,
    /// The most meshlets a level has.
    pub(crate) most: u32,
}

/// An asset streaming to the GPU.
pub(super) struct Item {
    index: u32,
    name: String,
    /// Its bytes on the GPU, which the parts copy.
    total: u64,
    /// Whether it has warned that it waits for GPU memory.
    warned: bool,
    queued: Instant,
    asset: Asset,
}

enum Asset {
    Mesh {
        /// Kept until the last part is copied.
        vertices: Vec<u8>,
        indices: Vec<u32>,
        /// Its vertices' bytes, the first of `total`; its indices are the rest.
        vertex_bytes: u64,
        vertex_size: usize,
        lods: Lods,
        meshlets: Option<MeshletParts>,
        skinned: Option<u32>,
        positions: Option<Box<Positions>>,
        /// Once there's room. Boxed to keep the queue's items small.
        buffers: Option<Box<(GpuBuffer, GpuBuffer)>>,
    },
    Texture {
        size: [u32; 2],
        texels: Texels,
        /// Kept until the last part is copied, with the mip levels made once its image is.
        pixels: Vec<u8>,
        /// Whether its image is made, once there's room.
        made: bool,
    },
}

impl Item {
    fn is(&self, kind: Kind, index: u32) -> bool {
        let is_mesh = matches!(self.asset, Asset::Mesh { .. });
        self.index == index && is_mesh == (kind == Kind::Mesh)
    }

    /// Whether its buffers or image are made: it's copying, or copied and waiting to be drawable.
    fn made(&self) -> bool {
        match &self.asset {
            Asset::Mesh { buffers, .. } => buffers.is_some(),
            Asset::Texture { made, .. } => *made,
        }
    }

    /// Logs that it is drawable, `renderer.mesh_streamed` or `renderer.texture_streamed`, at debug,
    /// as there may be many: its bytes on the GPU and time since queued (waiting, copying, and
    /// making a texture's levels if its loader didn't).
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

/// Unfinished transfer submissions at a frame's start past which it copies nothing
/// ([`Frame::uploads_behind`]). With two frames in flight, a copy engine that keeps up leaves at
/// most one; two means it's behind (a slow link, contention). Holding off bounds what's in flight
/// to two frames' parts, staging and command buffers, plus the frame recording, at any frame rate.
const MAX_UPLOADS_BEHIND: u64 = 2;

/// The bytes a frame may copy of `budget` with `behind` copies unfinished: none at
/// [`MAX_UPLOADS_BEHIND`], when the frame neither copies nor makes buffers or images, but takes
/// what's drawable.
pub(super) fn budget_behind(behind: u64, budget: u64) -> u64 {
    if behind >= MAX_UPLOADS_BEHIND {
        0
    } else {
        budget
    }
}

/// Whether an asset of `bytes` would take GPU memory `usage` past `budget`.
pub(super) fn exceeds(usage: u64, budget: u64, bytes: u64) -> bool {
    usage.saturating_add(bytes) > budget
}

/// Whether an asset of `bytes` waits for GPU memory: while it would take `usage` past `budget`,
/// unless nothing is `copying`. Then it goes ahead alone, so a game starts, an asset at a time, on
/// a GPU already past budget from other processes.
pub(super) fn waits(usage: u64, budget: u64, bytes: u64, copying: bool) -> bool {
    exceeds(usage, budget, bytes) && copying
}

/// Whether asset `name`, of `bytes`, waits for GPU memory, as [`waits`] decides from `device`'s
/// report. Warns the first time it's past the budget (kept in `warned`), whether it waits or goes
/// ahead.
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

/// Whether making a buffer or image failed for want of memory, which [`room_for`] waits out rather
/// than ending the renderer: its budget gate is an estimate.
fn out_of_memory(error: &Error) -> bool {
    error.code() == "gpu.out_of_memory"
}

/// What `made`, asset `name`'s buffers or image of `bytes`, gave: `None` if GPU memory had no room,
/// warning once (kept in `warned`) and leaving the asset to retry as room comes; any other error as
/// is.
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

/// Mesh `name`'s buffers: `vertex_bytes` of vertices and `index_bytes` of indices.
fn mesh_buffers(
    device: &Arc<Device>,
    name: &str,
    vertex_bytes: u64,
    index_bytes: u64,
) -> Result<(GpuBuffer, GpuBuffer)> {
    let vertices = GpuBuffer::new(device, vertex_bytes, &format!("{name} vertices"))?;
    // If the indices don't fit, the vertices' buffer retires with the error.
    let indices = GpuBuffer::new(device, index_bytes, &format!("{name} indices"))?;
    Ok((vertices, indices))
}

impl MeshPass {
    /// Takes `uploads`' changes to meshes, textures and materials, writes materials and the scene's
    /// changes to their tables, and streams the queue within the frame's budget, before the frame's
    /// passes. Assets queue in order. Each frame copies parts, making an asset's buffers or image
    /// in its turn if memory has room ([`waits`], [`room_for`]); else it waits, warning once, while
    /// later ones go on. A frame whose transfer queue is behind copies nothing ([`budget_behind`]).
    /// Until an asset is there, its slot shows a stand-in, or a replaced one its old data. Removals
    /// take effect this frame, dropping what was queued.
    ///
    /// # Panics
    ///
    /// - If they come from another `Meshes` or `Textures` than earlier ones.
    pub(crate) fn upload(&mut self, frame: &mut Frame<'_>, uploads: &mut Uploads) -> Result<()> {
        let _zone = gg_core::zone!();
        let changes = !(uploads.meshes.is_empty()
            && uploads.textures.is_empty()
            && uploads.materials.is_empty());
        self.stand_ins.write(frame)?;
        for change in uploads.meshes.drain(..) {
            self.mesh_changed(change);
        }
        for change in uploads.textures.drain(..) {
            self.texture_changed(frame, change)?;
        }
        (self.materials).write(&self.device, frame, &mut uploads.materials)?;
        self.scene.write(&self.device, frame)?;
        let queued = self.streaming.len();
        self.stream(frame)?;
        // What's drawable leaves the queue.
        self.uploaded = changes || self.streaming.len() < queued;
        // The meshlets of meshes that became drawable.
        self.meshlet_table.write(&self.device, frame)
    }

    /// The materials table's address, which the frame's data carries: 0 before any is added.
    pub(crate) fn materials_address(&self) -> u64 {
        self.materials.address()
    }

    /// Queues what was added, or removes a mesh.
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
        // A replaced mesh is one the pass has; any other is its `Meshes`' next.
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
                positions: added.positions,
                buffers: None,
            },
        });
    }

    /// Makes a slot's texture, queues an image, or removes a texture.
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
                // In place of any of the texture's image still queued, which, if made, retires with
                // what was copied in.
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

    /// Copies the queue's next parts in order within the frame's budget (none if the transfer queue
    /// is behind), and moves what's drawable into place.
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

    /// Advances the `i`th queued asset: makes its buffers or image if there's room (setting
    /// `copying`), copies parts while `spent` is under `budget`, and once all are there takes it
    /// for drawing. Returns whether it's drawable, and so done.
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
                positions,
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
                    positions: positions.take().map(|positions| *positions),
                };
                let held = &mut meshes[item.index as usize];
                // Draws' sizes are checked against it: its own type, as only the `Meshes` that
                // added it replaces it.
                if let Some(old) = held.ready() {
                    assert_eq!(
                        old.vertex_size, mesh.vertex_size,
                        "mesh {:?} is replaced by vertices of another type; a renderer draws the \
                         meshes of one `Meshes`",
                        item.name
                    );
                }
                // Old buffers retire until the GPU is done with the frames drawing them.
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
                    // A replacement may be compressed where the old image wasn't, or the other way
                    // round, as its `Textures` keeps only its kind.
                    let refilled =
                        texture.refill(width, height, texels.format(), texels.mipmapped());
                    if room_for(refilled, &item.name, item.total, &mut item.warned)?.is_none() {
                        return Ok(false);
                    }
                    // Once the image is there, so a retried refill doesn't make them again.
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

    /// What `texture`'s slot shows, if its own image rather than a stand-in: its size, and how many
    /// times the slot has switched images, telling this one from earlier ones. `None` for a texture
    /// that isn't the pass's.
    pub(crate) fn texture_shown(&self, texture: crate::Texture) -> Option<([u32; 2], u64)> {
        let slot = self.textures.get(texture.index as usize)?;
        let size = slot.texture.size();
        (size != [0, 0]).then_some((size, slot.shown))
    }

    /// How many bytes of `item` are copied.
    fn copied(&self, item: &Item) -> u64 {
        match &item.asset {
            Asset::Mesh { buffers, .. } => buffers
                .as_ref()
                .map_or(0, |buffers| buffers.0.streamed() + buffers.1.streamed()),
            Asset::Texture { .. } => self.textures[item.index as usize].texture.streamed(),
        }
    }

    pub(crate) fn streaming(&self) -> Streaming {
        Streaming {
            assets: self.streaming.len(),
            bytes: (self.streaming.iter())
                .map(|item| item.total - self.copied(item))
                .sum(),
        }
    }
}
