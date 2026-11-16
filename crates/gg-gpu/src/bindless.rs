//! The bindless descriptor sets: every texture shaders sample, a fixed set of samplers, and the
//! levels of the storage images compute shaders write. There is one set per frame in flight, all
//! of one layout that every pipeline shares. Shaders pick a texture, sampler and storage image by
//! index and read buffers by device address, so nothing else is ever bound. A frame binds the set
//! of its slot. A slot's view changes at once in the set of the frame being recorded, and in each
//! other set as its next frame begins, once the frame that last used it has finished.

use std::collections::HashSet;
use std::iter;
use std::sync::{Mutex, PoisonError};

use ash::vk;
use gg_core::{Error, Result};

use crate::frame::FRAMES_IN_FLIGHT;
use crate::{Device, vk_failed};

/// How many sets there are: one for each frame in flight.
const SETS: usize = FRAMES_IN_FLIGHT as usize;

/// How many textures the set holds. A device with descriptor indexing holds at least 500,000.
pub(crate) const TEXTURES: u32 = 65_536;

/// How many of the set's slots, from the first, are placed: a texture names its slot, as
/// [`Texture::placed`](crate::Texture::placed) does, so its index is known before it's made.
/// The other 4,096 are given out wherever one is free.
pub const PLACED_TEXTURES: u32 = TEXTURES - 4_096;

/// How many storage images the set holds, a mip level each, which compute shaders write and read
/// by index, in the general layout. A device with descriptor indexing holds at least 500,000.
pub(crate) const STORAGE_IMAGES: u32 = 4_096;

/// The bytes of push constants every pipeline gets: 128, the most every device allows. Shaders
/// see them from offset 0, in every stage.
pub(crate) const PUSH_CONSTANTS: u32 = 128;

/// A texture's filter and what lies past its edges. Shaders take the sampler from the set by
/// [`Sampler::index`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sampler {
    LinearClamp,
    NearestClamp,
    LinearRepeat,
    NearestRepeat,
    LinearMirror,
    NearestMirror,
}

impl Sampler {
    /// Every sampler, in the order of their indices.
    pub const ALL: [Self; 6] = [
        Self::LinearClamp,
        Self::NearestClamp,
        Self::LinearRepeat,
        Self::NearestRepeat,
        Self::LinearMirror,
        Self::NearestMirror,
    ];

    /// Where shaders find this sampler in the set's sampler array.
    pub fn index(self) -> u32 {
        self as u32
    }

    fn info(self) -> vk::SamplerCreateInfo<'static> {
        use vk::{Filter, SamplerMipmapMode as Mipmap};
        let (filter, mipmap) = match self {
            Self::LinearClamp | Self::LinearRepeat | Self::LinearMirror => {
                (Filter::LINEAR, Mipmap::LINEAR)
            }
            Self::NearestClamp | Self::NearestRepeat | Self::NearestMirror => {
                (Filter::NEAREST, Mipmap::NEAREST)
            }
        };
        let wrap = match self {
            Self::LinearClamp | Self::NearestClamp => Wrap::Clamp,
            Self::LinearRepeat | Self::NearestRepeat => Wrap::Repeat,
            Self::LinearMirror | Self::NearestMirror => Wrap::Mirror,
        };
        let address = wrap.address();
        vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(mipmap)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .max_lod(vk::LOD_CLAMP_NONE)
    }
}

/// What a sampler gives past a texture's edges: the edge texels, the texture repeated, or the
/// texture mirrored at each edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

impl Wrap {
    const ALL: [Self; 3] = [Self::Clamp, Self::Repeat, Self::Mirror];

    /// Where shaders find, after every [`Sampler`], the anisotropic sampler that wraps this way.
    /// It filters linearly within and between mip levels, with up to 16 samples along the way a
    /// surface recedes, for models' textures; on lavapipe without those samples, as
    /// `Candidate::anisotropy` in src/device/candidate.rs says. Not for sprites: its samples
    /// spread around the requested point and read past their texels.
    pub fn anisotropic(self) -> u32 {
        Sampler::ALL.len() as u32 + self as u32
    }

    fn address(self) -> vk::SamplerAddressMode {
        match self {
            Self::Clamp => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            Self::Repeat => vk::SamplerAddressMode::REPEAT,
            Self::Mirror => vk::SamplerAddressMode::MIRRORED_REPEAT,
        }
    }

    /// Its anisotropic sampler: 16 samples at most, the least `maxSamplerAnisotropy` a device with
    /// anisotropy may have, which every Vulkan 1.4 device has; without `anisotropy`, linear
    /// filtering alone.
    fn anisotropic_info(self, anisotropy: bool) -> vk::SamplerCreateInfo<'static> {
        let address = self.address();
        vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .anisotropy_enable(anisotropy)
            .max_anisotropy(16.0)
            .max_lod(vk::LOD_CLAMP_NONE)
    }
}

/// Where shaders find, after every [`Sampler`] and [`Wrap`]'s anisotropic ones, the sampler that
/// compares depth: a `SamplerComparisonState` for `SampleCmp`, to sample a shadow map, an image
/// of depth ([`Graph::depth`](crate::Graph::depth)). Each of the four texels nearest the point
/// is 1 where the depth compared is as near as the texel's or nearer (depth runs from 1 at the
/// near plane), else 0, and the four filter linearly, as percentage-closer filtering takes them.
/// Past the edges, the edge texels. Shaders sample nothing else through it, and nothing but by
/// comparison.
pub const COMPARISON_SAMPLER: u32 = (Sampler::ALL.len() + Wrap::ALL.len()) as u32;

/// How many samplers the set has: every [`Sampler`], [`Wrap`]'s anisotropic ones, then the
/// comparison one.
const SAMPLERS: usize = COMPARISON_SAMPLER as usize + 1;

/// [`COMPARISON_SAMPLER`]'s create info.
fn comparison_info() -> vk::SamplerCreateInfo<'static> {
    let clamp = vk::SamplerAddressMode::CLAMP_TO_EDGE;
    vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(clamp)
        .address_mode_v(clamp)
        .address_mode_w(clamp)
        .compare_enable(true)
        .compare_op(vk::CompareOp::GREATER_OR_EQUAL)
}

/// The samplers' debug names, in the order of their indices.
fn sampler_names() -> impl Iterator<Item = String> {
    (Sampler::ALL
        .map(|kind| format!("{kind:?} sampler"))
        .into_iter())
    .chain(Wrap::ALL.map(|wrap| format!("Anisotropic{wrap:?} sampler")))
    .chain(iter::once("Comparison sampler".to_owned()))
}

/// The sets, their layout and pipeline layout, which texture slots are free, and the writes each
/// set takes as its next frame begins.
pub(crate) struct Bindless {
    samplers: Vec<vk::Sampler>,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// One per frame in flight, in the order of their slots.
    sets: Vec<vk::DescriptorSet>,
    pipeline_layout: vk::PipelineLayout,
    /// Also held while writing a set, which Vulkan requires one thread at a time to do.
    state: Mutex<State>,
}

struct State {
    slots: Slots,
    storage: StorageSlots,
    rewrites: Rewrites,
}

/// Which storage image slots are in use: by a level of a storage image, or retired until the GPU
/// is done with the frames that may write it. Each takes its view in every set at once and keeps
/// it until freed.
struct StorageSlots {
    /// Slots whose views were destroyed, to reuse first.
    free: Vec<u32>,
    /// The first slot never used.
    next: u32,
}

impl StorageSlots {
    /// Takes a free slot for a level of the image `name`.
    fn take(&mut self, name: &str) -> Result<u32> {
        match self.free.pop() {
            Some(index) => Ok(index),
            None if self.next < STORAGE_IMAGES => {
                self.next += 1;
                Ok(self.next - 1)
            }
            None => Err(Error::new(
                "gpu.too_many_storage_images",
                format!(
                    "a level of storage image {name:?} has no slot: the bindless set holds \
                     {STORAGE_IMAGES} storage images, and all are in use"
                ),
            )),
        }
    }
}

/// The views queued for texture slots in the sets of frames that may still be running, each
/// written into its set as that set's next frame begins.
struct Rewrites {
    /// Per set, the slots to write and their views. A slot is in a list at most once, with the
    /// last view queued for it.
    queued: Vec<Vec<(u32, vk::ImageView)>>,
}

impl Rewrites {
    fn new() -> Self {
        Self {
            queued: (0..SETS).map(|_| Vec::new()).collect(),
        }
    }

    /// Notes that texture slot `index` took `view` in set `written`, written at once, and queues
    /// `view` for it in every other set, replacing any view queued before: the last wins. Nothing
    /// is queued for `written`, the set of the frame being recorded: its list was emptied as that
    /// frame began, and only the frame being recorded queues.
    fn queue(&mut self, written: usize, index: u32, view: vk::ImageView) {
        debug_assert!(
            !self.queued[written].iter().any(|&(slot, _)| slot == index),
            "set {written} is the frame being recorded's, whose list its frame emptied as it began"
        );
        for (set, queued) in self.queued.iter_mut().enumerate() {
            if set == written {
                continue;
            }
            match queued.iter().position(|&(slot, _)| slot == index) {
                Some(at) => queued[at].1 = view,
                None => queued.push((index, view)),
            }
        }
    }

    /// Forgets what's queued for slot `index` in every set: it was written into them all.
    fn clear(&mut self, index: u32) {
        for queued in &mut self.queued {
            queued.retain(|&(slot, _)| slot != index);
        }
    }

    /// Takes what's queued for set `set`, leaving its list empty with its memory.
    fn take(&mut self, set: usize) -> std::vec::Drain<'_, (u32, vk::ImageView)> {
        self.queued[set].drain(..)
    }
}

/// Which texture slots are in use: by a texture, or retired until the GPU is done with the frames
/// that may sample it.
struct Slots {
    /// The placed slots in use.
    placed: HashSet<u32>,
    /// Unplaced slots whose textures were destroyed, to reuse first.
    free: Vec<u32>,
    /// The first unplaced slot never used.
    next: u32,
}

impl Slots {
    fn new() -> Self {
        Self {
            placed: HashSet::new(),
            free: Vec::new(),
            next: PLACED_TEXTURES,
        }
    }

    /// Takes `slot`, a placed one, or with `None` a free unplaced one, for the image `name`.
    ///
    /// # Panics
    ///
    /// If `slot` isn't placed, or is in use.
    fn take(&mut self, slot: Option<u32>, name: &str) -> Result<u32> {
        if let Some(slot) = slot {
            assert!(
                slot < PLACED_TEXTURES,
                "texture slot {slot} isn't one of the {PLACED_TEXTURES} placed ones"
            );
            assert!(
                self.placed.insert(slot),
                "texture slot {slot} is in use, by a texture or by one the GPU may still sample"
            );
            return Ok(slot);
        }
        match self.free.pop() {
            Some(index) => Ok(index),
            None if self.next < TEXTURES => {
                self.next += 1;
                Ok(self.next - 1)
            }
            None => Err(Error::new(
                "gpu.too_many_textures",
                format!(
                    "image {name:?} has no slot: the bindless set gives out {} textures past the \
                     placed ones, and all are in use",
                    TEXTURES - PLACED_TEXTURES
                ),
            )),
        }
    }

    /// Frees `slot` for another texture.
    fn release(&mut self, slot: u32) {
        if slot < PLACED_TEXTURES {
            self.placed.remove(&slot);
        } else {
            self.free.push(slot);
        }
    }
}

impl Bindless {
    /// The set; its [`Wrap::anisotropic`] samplers filter anisotropically if `anisotropy`.
    ///
    /// # Safety
    ///
    /// The device must be live, with the bindless features enabled, and outlive the set, which
    /// it must [`destroy`](Bindless::destroy).
    pub(crate) unsafe fn new(device: &ash::Device, anisotropy: bool) -> Result<Self> {
        let mut bindless = Self {
            samplers: Vec::new(),
            layout: vk::DescriptorSetLayout::null(),
            pool: vk::DescriptorPool::null(),
            sets: Vec::new(),
            pipeline_layout: vk::PipelineLayout::null(),
            state: Mutex::new(State {
                slots: Slots::new(),
                storage: StorageSlots {
                    free: Vec::new(),
                    next: 0,
                },
                rewrites: Rewrites::new(),
            }),
        };
        // SAFETY: the caller guarantees the device; on failure, what was made so far is
        // destroyed, null handles included, which Vulkan allows.
        let made = unsafe { bindless.make(device, anisotropy) };
        if made.is_err() {
            // SAFETY: as above.
            unsafe { bindless.destroy(device) };
        }
        made.map(|()| bindless)
    }

    /// # Safety
    ///
    /// As for [`Bindless::new`].
    unsafe fn make(&mut self, device: &ash::Device, anisotropy: bool) -> Result<()> {
        // SAFETY: the caller guarantees the device; every handle is made from live ones, and
        // the infos outlive the calls.
        unsafe {
            let infos = (Sampler::ALL.map(Sampler::info).into_iter())
                .chain(Wrap::ALL.map(|wrap| wrap.anisotropic_info(anisotropy)))
                .chain(iter::once(comparison_info()));
            for (info, name) in infos.zip(sampler_names()) {
                let sampler = device
                    .create_sampler(&info, None)
                    .map_err(vk_failed("vkCreateSampler", format_args!("{name:?}")))?;
                self.samplers.push(sampler);
            }
            // Textures change while frames using other textures are in flight, and a slot of the
            // recording frame's set changes while the frame before samples it through the other
            // set; storage images come and go while frames using others are.
            let texture_flags = vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING
                | vk::DescriptorBindingFlags::PARTIALLY_BOUND;
            let binding_flags = [
                texture_flags,
                vk::DescriptorBindingFlags::empty(),
                texture_flags,
            ];
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(TEXTURES)
                    .stage_flags(vk::ShaderStageFlags::ALL),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(SAMPLERS as u32)
                    .stage_flags(vk::ShaderStageFlags::ALL),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(STORAGE_IMAGES)
                    .stage_flags(vk::ShaderStageFlags::ALL),
            ];
            let mut flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
                .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                .bindings(&bindings)
                .push_next(&mut flags_info);
            let contents = format_args!(
                "{TEXTURES} textures, {SAMPLERS} samplers and {STORAGE_IMAGES} storage images"
            );
            self.layout = device
                .create_descriptor_set_layout(&layout_info, None)
                .map_err(vk_failed(
                    "vkCreateDescriptorSetLayout",
                    format_args!("\"bindless set layout\" of {contents}"),
                ))?;

            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: TEXTURES * SETS as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: (SAMPLERS * SETS) as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: STORAGE_IMAGES * SETS as u32,
                },
            ];
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                .max_sets(SETS as u32)
                .pool_sizes(&sizes);
            self.pool = device
                .create_descriptor_pool(&pool_info, None)
                .map_err(vk_failed(
                    "vkCreateDescriptorPool",
                    format_args!("\"bindless pool\" of {SETS} sets of {contents}"),
                ))?;
            let layouts = [self.layout; SETS];
            let allocate_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(self.pool)
                .set_layouts(&layouts);
            self.sets = device
                .allocate_descriptor_sets(&allocate_info)
                .map_err(vk_failed("vkAllocateDescriptorSets", "the bindless sets"))?;
            // Written into each set once, before any pipeline uses it, not made immutable in the
            // layout: AMD's driver (25.10.30) reads immutable samplers that shaders index
            // non-uniformly as a sampler of zeros, which filters by the nearest texel and never
            // leaves a texture's first level.
            let samplers: Vec<_> = (self.samplers.iter())
                .map(|&sampler| vk::DescriptorImageInfo::default().sampler(sampler))
                .collect();
            let writes: Vec<_> = (self.sets.iter())
                .map(|&set| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&samplers)
                })
                .collect();
            device.update_descriptor_sets(&writes, &[]);

            let push_constants = [vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::ALL,
                offset: 0,
                size: PUSH_CONSTANTS,
            }];
            let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default()
                .set_layouts(&layouts[..1])
                .push_constant_ranges(&push_constants);
            self.pipeline_layout = device
                .create_pipeline_layout(&pipeline_layout_info, None)
                .map_err(vk_failed(
                    "vkCreatePipelineLayout",
                    format_args!(
                        "\"bindless pipeline layout\" with {PUSH_CONSTANTS} bytes of push constants"
                    ),
                ))?;
        }
        Ok(())
    }

    /// The set frames of slot `slot` bind.
    pub(crate) fn set(&self, slot: usize) -> vk::DescriptorSet {
        self.sets[slot]
    }

    /// Names the samplers, sets, layouts and pool.
    ///
    /// # Safety
    ///
    /// `device` must be the sets'.
    pub(crate) unsafe fn set_names(&self, device: &Device) {
        // SAFETY: the caller guarantees the objects are the device's, and they live as long as
        // the sets.
        unsafe {
            for (&sampler, name) in self.samplers.iter().zip(sampler_names()) {
                device.set_name(sampler, &name);
            }
            device.set_name(self.layout, "bindless set layout");
            device.set_name(self.pool, "bindless pool");
            for (slot, &set) in self.sets.iter().enumerate() {
                device.set_name(set, &format!("bindless set {slot}"));
            }
            device.set_name(self.pipeline_layout, "bindless pipeline layout");
        }
    }

    /// The layout every pipeline uses.
    pub(crate) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    /// Puts `view`, of the image `name`, in texture slot `slot`, a placed one, or with `None` a
    /// free unplaced one, and returns its index: [`Bindless::reserve`], then [`Bindless::write`].
    ///
    /// # Safety
    ///
    /// As for [`Bindless::write`], but for the slot, which is free as it's reserved.
    ///
    /// # Panics
    ///
    /// As [`Bindless::reserve`] does.
    pub(crate) unsafe fn add(
        &self,
        device: &ash::Device,
        view: vk::ImageView,
        slot: Option<u32>,
        name: &str,
    ) -> Result<u32> {
        let index = self.reserve(slot, name)?;
        // SAFETY: the caller guarantees the device and view, and no frame uses the slot, which was
        // free when it was reserved.
        unsafe { self.write(device, view, index) };
        Ok(index)
    }

    /// Takes texture slot `slot`, a placed one, or with `None` a free unplaced one, for the view
    /// [`Bindless::write`] puts in it, of the image `name`, and returns its index. A texture
    /// reserves its slot before making its image, so a refused slot leaks nothing.
    ///
    /// # Panics
    ///
    /// If `slot` isn't placed, or is in use.
    pub(crate) fn reserve(&self, slot: Option<u32>, name: &str) -> Result<u32> {
        self.lock().slots.take(slot, name)
    }

    /// Puts `view` in texture slot `index`, which [`Bindless::reserve`] gave, in every set at
    /// once: for a slot just reserved, which no frame uses.
    ///
    /// # Safety
    ///
    /// `device` must be the sets', and `view` a live view of a sampled image of it, in the
    /// shader-read-only layout whenever a shader samples it. No submitted frame the GPU may still
    /// run may use the slot, as the binding's flags allow updating only while unused; a frame
    /// still recording takes the new view.
    pub(crate) unsafe fn write(&self, device: &ash::Device, view: vk::ImageView, index: u32) {
        let mut state = self.lock();
        for set in 0..SETS {
            // SAFETY: the caller guarantees the device and view, and that no submitted frame uses
            // the slot; the lock is held.
            unsafe { write_slot(device, self.sets[set], index, view) };
        }
        // Nothing queued for the slot before, for a texture that had it, may write over it.
        state.rewrites.clear(index);
    }

    /// Puts `view` in texture slot `index`, which frames the GPU may still run may sample through
    /// their sets: at once in the set of slot `recording`, the frame being recorded, and in each
    /// other set as its next frame begins ([`Bindless::apply`]), once the frame that last used it
    /// has finished. The view the slot held stays in use until then, so it retires by frames as
    /// any resource does.
    ///
    /// # Safety
    ///
    /// `device` must be the sets', `view` as for [`Bindless::write`], and `recording` the slot of
    /// the frame being recorded, whose set no submitted frame the GPU may still run uses: the
    /// frame that last used it has finished. `view` must also live until the frame being
    /// recorded has finished, as a view that retires by frames does: each other set takes it as
    /// its next frame begins, having waited only for frames before this one.
    pub(crate) unsafe fn rewrite(
        &self,
        device: &ash::Device,
        view: vk::ImageView,
        index: u32,
        recording: usize,
    ) {
        let mut state = self.lock();
        // SAFETY: the caller guarantees the device, the view and the set; the lock is held.
        unsafe { write_slot(device, self.sets[recording], index, view) };
        state.rewrites.queue(recording, index, view);
    }

    /// Writes into the set of slot `slot` the views [`Bindless::rewrite`] queued for it, as the
    /// slot's next frame begins.
    ///
    /// # Safety
    ///
    /// `device` must be the sets', and no submitted frame the GPU may still run may use the set:
    /// the frame that last used the slot has finished.
    pub(crate) unsafe fn apply(&self, device: &ash::Device, slot: usize) {
        let mut state = self.lock();
        for (index, view) in state.rewrites.take(slot) {
            // SAFETY: the caller guarantees the device and the set; the lock is held. The view was
            // queued by a frame begun since the set's last, on another set, and lives until that
            // frame has finished, as `rewrite` requires. The frame beginning has waited only for
            // the set's last, and destroyed only what no later frame may use, so the view is live.
            unsafe { write_slot(device, self.sets[slot], index, view) };
        }
    }

    /// Frees a texture slot: once the GPU is done with the frames that may use it, or at once for
    /// one reserved and never written.
    pub(crate) fn remove(&self, index: u32) {
        self.lock().slots.release(index);
    }

    /// Puts `view`, of a level of the storage image `name`, in a free storage slot in every set at
    /// once, and returns its index.
    ///
    /// # Safety
    ///
    /// `device` must be the sets', and `view` a live view of one level of a storage image of it,
    /// in the general layout whenever a shader reads or writes it; no frame uses the slot, which
    /// is free as it's taken.
    pub(crate) unsafe fn add_storage(
        &self,
        device: &ash::Device,
        view: vk::ImageView,
        name: &str,
    ) -> Result<u32> {
        let mut state = self.lock();
        let index = state.storage.take(name)?;
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        for &set in &self.sets {
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(2)
                .dst_array_element(index)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&image_info);
            // SAFETY: the caller guarantees the device and view, and that no frame uses the slot;
            // the lock is held.
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
        Ok(index)
    }

    /// Frees a storage slot, once the GPU is done with the frames that may use it.
    pub(crate) fn remove_storage(&self, index: u32) {
        self.lock().storage.free.push(index);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // Neither the slot list nor the queues are ever left half-changed.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// # Safety
    ///
    /// `device` must be the sets', and the GPU must be done with them.
    pub(crate) unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: the caller guarantees it; null handles are allowed, and destroying the pool
        // frees the sets.
        unsafe {
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
            for &sampler in &self.samplers {
                device.destroy_sampler(sampler, None);
            }
        }
    }
}

/// Puts `view` in texture slot `index` of `set`.
///
/// # Safety
///
/// `device` must be `set`'s, `view` as for [`Bindless::write`], no submitted frame the GPU may
/// still run may use the set, and the lock must be held.
unsafe fn write_slot(
    device: &ash::Device,
    set: vk::DescriptorSet,
    index: u32,
    view: vk::ImageView,
) {
    let image_info = [vk::DescriptorImageInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
    let write = vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .dst_array_element(index)
        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
        .image_info(&image_info);
    // SAFETY: the caller guarantees it.
    unsafe { device.update_descriptor_sets(&[write], &[]) };
}

#[cfg(test)]
mod tests {
    use ash::vk::Handle;

    use super::*;

    /// A view queued for a slot reaches every set but the one written at once, as each set's turn
    /// comes. A later view queued by the same frame replaces it in the sets not yet reached. One
    /// queued by the next frame, whose set took the first as it began, is owed to the first
    /// frame's set.
    #[test]
    fn a_rewrite_reaches_each_other_set_in_turn_and_the_last_queued_wins() {
        let view = |n: u64| vk::ImageView::from_raw(n);
        let mut rewrites = Rewrites::new();
        rewrites.queue(0, 5, view(1));
        rewrites.queue(0, 9, view(2));
        rewrites.queue(0, 5, view(3));
        assert!(
            rewrites.take(0).next().is_none(),
            "set 0 was written at once"
        );
        // Set 1's frame begins, taking what it's owed, then changes slot 5 again.
        assert_eq!(
            rewrites.take(1).collect::<Vec<_>>(),
            [(5, view(3)), (9, view(2))]
        );
        rewrites.queue(1, 5, view(4));
        assert_eq!(rewrites.take(0).collect::<Vec<_>>(), [(5, view(4))]);
        assert!(rewrites.take(0).next().is_none(), "taken once");
        assert!(rewrites.take(1).next().is_none());
        // A slot written into every set at once owes no set anything, whatever was queued.
        rewrites.queue(0, 7, view(4));
        rewrites.queue(1, 8, view(5));
        rewrites.clear(7);
        assert!(rewrites.take(1).next().is_none());
        assert_eq!(rewrites.take(0).collect::<Vec<_>>(), [(8, view(5))]);
    }

    #[test]
    fn placed_slots_come_back_once_released_and_the_rest_are_given_out_past_them() {
        let mut slots = Slots::new();
        assert_eq!(slots.take(Some(7), "a").ok(), Some(7));
        assert_eq!(slots.take(None, "b").ok(), Some(PLACED_TEXTURES));
        assert_eq!(slots.take(None, "c").ok(), Some(PLACED_TEXTURES + 1));
        slots.release(7);
        slots.release(PLACED_TEXTURES);
        assert_eq!(slots.take(Some(7), "d").ok(), Some(7));
        assert_eq!(slots.take(None, "e").ok(), Some(PLACED_TEXTURES));
    }

    #[test]
    #[should_panic(expected = "texture slot 7 is in use")]
    fn a_placed_slot_in_use_is_refused() {
        let mut slots = Slots::new();
        let _ = slots.take(Some(7), "a");
        let _ = slots.take(Some(7), "b");
    }

    #[test]
    #[should_panic(expected = "isn't one of the")]
    fn only_the_placed_slots_can_be_placed() {
        let _ = Slots::new().take(Some(PLACED_TEXTURES), "a");
    }

    #[test]
    fn the_slots_given_out_run_out_at_the_end_of_the_set() {
        let mut slots = Slots::new();
        for _ in PLACED_TEXTURES..TEXTURES {
            assert!(slots.take(None, "given out").is_ok());
        }
        let error = slots.take(None, "one too many").expect_err("no slot left");
        assert_eq!(error.code(), "gpu.too_many_textures");
        assert!(error.to_string().contains("\"one too many\""), "{error}");
    }

    #[test]
    fn storage_slots_run_out_at_the_end_and_come_back_once_freed() {
        let mut slots = StorageSlots {
            free: Vec::new(),
            next: 0,
        };
        for expected in 0..STORAGE_IMAGES {
            assert_eq!(slots.take("given out").ok(), Some(expected));
        }
        let error = slots.take("one too many").expect_err("no slot left");
        assert_eq!(error.code(), "gpu.too_many_storage_images");
        slots.free.push(7);
        assert_eq!(slots.take("again").ok(), Some(7));
    }
}
