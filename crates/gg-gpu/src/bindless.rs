//! The bindless descriptor set: every texture shaders can sample, and a fixed set of samplers,
//! in one descriptor set that every pipeline shares. Shaders pick a texture and a sampler by
//! index, and read buffers through their device addresses, so nothing else is ever bound.

use std::sync::{Mutex, PoisonError};

use ash::vk;
use gg_core::{Error, Result};

use crate::{Device, vk_error};

/// How many textures the set holds. A device with descriptor indexing holds at least 500,000.
pub(crate) const TEXTURES: u32 = 65_536;

/// The bytes of push constants every pipeline gets: 128, the most every device allows. Shaders
/// see them from offset 0, in every stage.
pub(crate) const PUSH_CONSTANTS: u32 = 128;

/// How shaders sample a texture: its filter, and what lies past its edges. Shaders take the
/// sampler from the set by [`Sampler::index`].
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
    const ALL: [Self; 6] = [
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
        use vk::{Filter, SamplerAddressMode as Address, SamplerMipmapMode as Mipmap};
        let (filter, mipmap) = match self {
            Self::LinearClamp | Self::LinearRepeat | Self::LinearMirror => {
                (Filter::LINEAR, Mipmap::LINEAR)
            }
            Self::NearestClamp | Self::NearestRepeat | Self::NearestMirror => {
                (Filter::NEAREST, Mipmap::NEAREST)
            }
        };
        let address = match self {
            Self::LinearClamp | Self::NearestClamp => Address::CLAMP_TO_EDGE,
            Self::LinearRepeat | Self::NearestRepeat => Address::REPEAT,
            Self::LinearMirror | Self::NearestMirror => Address::MIRRORED_REPEAT,
        };
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

/// The set, its layout and the pipeline layout built on it, and which texture slots are free.
pub(crate) struct Bindless {
    samplers: Vec<vk::Sampler>,
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    /// Also held while writing the set, which Vulkan requires one thread at a time to do.
    slots: Mutex<Slots>,
}

#[derive(Default)]
struct Slots {
    /// Slots whose textures were destroyed, to reuse first.
    free: Vec<u32>,
    /// The first slot never used.
    next: u32,
}

impl Bindless {
    /// # Safety
    ///
    /// The device must be live, with the bindless features enabled, and outlive the set, which
    /// it must [`destroy`](Bindless::destroy).
    pub(crate) unsafe fn new(device: &ash::Device) -> Result<Self> {
        let mut bindless = Self {
            samplers: Vec::new(),
            layout: vk::DescriptorSetLayout::null(),
            pool: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            slots: Mutex::default(),
        };
        // SAFETY: the caller guarantees the device; on failure, what was made so far is
        // destroyed, null handles included, which Vulkan allows.
        let made = unsafe { bindless.make(device) };
        if made.is_err() {
            // SAFETY: as above.
            unsafe { bindless.destroy(device) };
        }
        made.map(|()| bindless)
    }

    /// # Safety
    ///
    /// As for [`Bindless::new`].
    unsafe fn make(&mut self, device: &ash::Device) -> Result<()> {
        // SAFETY: the caller guarantees the device; every handle is made from live ones, and
        // the infos outlive the calls.
        unsafe {
            for sampler in Sampler::ALL {
                let sampler = device
                    .create_sampler(&sampler.info(), None)
                    .map_err(|e| vk_error("vkCreateSampler", e))?;
                self.samplers.push(sampler);
            }
            // Textures change while frames that use other textures are in flight.
            let texture_flags = vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING
                | vk::DescriptorBindingFlags::PARTIALLY_BOUND;
            let binding_flags = [texture_flags, vk::DescriptorBindingFlags::empty()];
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(TEXTURES)
                    .stage_flags(vk::ShaderStageFlags::ALL),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .stage_flags(vk::ShaderStageFlags::ALL)
                    .immutable_samplers(&self.samplers),
            ];
            let mut flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
                .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                .bindings(&bindings)
                .push_next(&mut flags_info);
            self.layout = device
                .create_descriptor_set_layout(&layout_info, None)
                .map_err(|e| vk_error("vkCreateDescriptorSetLayout", e))?;

            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: TEXTURES,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: Sampler::ALL.len() as u32,
                },
            ];
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                .max_sets(1)
                .pool_sizes(&sizes);
            self.pool = device
                .create_descriptor_pool(&pool_info, None)
                .map_err(|e| vk_error("vkCreateDescriptorPool", e))?;
            let layouts = [self.layout];
            let allocate_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(self.pool)
                .set_layouts(&layouts);
            self.set = device
                .allocate_descriptor_sets(&allocate_info)
                .map_err(|e| vk_error("vkAllocateDescriptorSets", e))?[0];

            let push_constants = [vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::ALL,
                offset: 0,
                size: PUSH_CONSTANTS,
            }];
            let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default()
                .set_layouts(&layouts)
                .push_constant_ranges(&push_constants);
            self.pipeline_layout = device
                .create_pipeline_layout(&pipeline_layout_info, None)
                .map_err(|e| vk_error("vkCreatePipelineLayout", e))?;
        }
        Ok(())
    }

    pub(crate) fn set(&self) -> vk::DescriptorSet {
        self.set
    }

    /// Gives the samplers, the set, and the layouts and pool it's made with their debug names.
    ///
    /// # Safety
    ///
    /// `device` must be the set's.
    pub(crate) unsafe fn set_names(&self, device: &Device) {
        // SAFETY: the caller guarantees the objects are the device's, and they live as long as
        // the set.
        unsafe {
            for (&sampler, kind) in self.samplers.iter().zip(Sampler::ALL) {
                device.set_name(sampler, &format!("{kind:?} sampler"));
            }
            device.set_name(self.layout, "bindless set layout");
            device.set_name(self.pool, "bindless pool");
            device.set_name(self.set, "bindless set");
            device.set_name(self.pipeline_layout, "bindless pipeline layout");
        }
    }

    /// The layout every pipeline is made with.
    pub(crate) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    /// Puts `view` in a free texture slot, and returns the slot's index.
    ///
    /// # Safety
    ///
    /// `device` must be the set's, and `view` a live view of a sampled image of it, in the
    /// shader-read-only layout whenever a shader samples it.
    pub(crate) unsafe fn add(&self, device: &ash::Device, view: vk::ImageView) -> Result<u32> {
        let mut slots = self.lock();
        let index = match slots.free.pop() {
            Some(index) => index,
            None if slots.next < TEXTURES => {
                slots.next += 1;
                slots.next - 1
            }
            None => {
                return Err(Error::new(
                    "gpu.too_many_textures",
                    format!("the bindless set holds {TEXTURES} textures, and all are in use"),
                ));
            }
        };
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(0)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image_info);
        // SAFETY: the caller guarantees the device and view; the lock is held, and no frame in
        // flight uses the slot, which was free.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
        Ok(index)
    }

    /// Frees a texture slot, once the GPU is done with the frames that may use it.
    pub(crate) fn remove(&self, index: u32) {
        self.lock().free.push(index);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slots> {
        // The slot list is never left half-changed.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// # Safety
    ///
    /// `device` must be the set's, and the GPU must be done with it.
    pub(crate) unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: the caller guarantees it; null handles are allowed, and destroying the pool
        // frees the set.
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
