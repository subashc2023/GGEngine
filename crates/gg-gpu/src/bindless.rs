//! The bindless descriptor set: every texture shaders can sample, and a fixed set of samplers,
//! in one descriptor set that every pipeline shares. Shaders pick a texture and a sampler by
//! index, and read buffers through their device addresses, so nothing else is ever bound.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use ash::vk;
use gg_core::{Error, Result};

use crate::{Device, vk_error};

/// How many textures the set holds. A device with descriptor indexing holds at least 500,000.
pub(crate) const TEXTURES: u32 = 65_536;

/// How many of the set's slots, from the first, are placed: the texture made for one names it, as
/// [`Texture::placed`](crate::Texture::placed) does, so that its index is known before it's made.
/// The other 4,096 are given out wherever one is free.
pub const PLACED_TEXTURES: u32 = TEXTURES - 4_096;

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

/// What a sampler gives past a texture's edges: the edges' texels, the texture repeated, or the
/// texture mirrored at each edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

impl Wrap {
    const ALL: [Self; 3] = [Self::Clamp, Self::Repeat, Self::Mirror];

    /// Where shaders find, after every [`Sampler`], the anisotropic sampler that wraps this way: it
    /// filters linearly within and between a texture's mip levels, with up to 16 samples along the
    /// way a surface recedes, as models' textures are sampled. Not for sprites, whose texels it
    /// would read past, since its samples spread around the point a shader asks for.
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

    /// What its anisotropic sampler is made from: 16 samples at most, the least
    /// `maxSamplerAnisotropy` a device with anisotropy may have, and every Vulkan 1.4 device has it.
    fn anisotropic_info(self) -> vk::SamplerCreateInfo<'static> {
        let address = self.address();
        vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .anisotropy_enable(true)
            .max_anisotropy(16.0)
            .max_lod(vk::LOD_CLAMP_NONE)
    }
}

/// How many samplers the set has: every [`Sampler`], then [`Wrap`]'s anisotropic ones.
const SAMPLERS: usize = Sampler::ALL.len() + Wrap::ALL.len();

/// The samplers' debug names, in the order of their indices.
fn sampler_names() -> impl Iterator<Item = String> {
    (Sampler::ALL
        .map(|kind| format!("{kind:?} sampler"))
        .into_iter())
    .chain(Wrap::ALL.map(|wrap| format!("Anisotropic{wrap:?} sampler")))
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

/// Which texture slots are in use: by a texture, or by one retired until the GPU is done with the
/// frames that may sample it.
struct Slots {
    /// The placed slots in use.
    placed: HashSet<u32>,
    /// Slots past the placed ones whose textures were destroyed, to reuse first.
    free: Vec<u32>,
    /// The first slot past the placed ones never used.
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

    /// Takes `slot`, a placed one, or with `None` a free slot past them, for the image named
    /// `name`.
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
            slots: Mutex::new(Slots::new()),
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
            let infos = (Sampler::ALL.map(Sampler::info).into_iter())
                .chain(Wrap::ALL.map(Wrap::anisotropic_info));
            for (info, name) in infos.zip(sampler_names()) {
                let sampler = device
                    .create_sampler(&info, None)
                    .map_err(|e| vk_error("vkCreateSampler", format_args!("{name:?}"), e))?;
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
                    .descriptor_count(SAMPLERS as u32)
                    .stage_flags(vk::ShaderStageFlags::ALL),
            ];
            let mut flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
                .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                .bindings(&bindings)
                .push_next(&mut flags_info);
            let contents = format_args!("{TEXTURES} textures and {SAMPLERS} samplers");
            self.layout = device
                .create_descriptor_set_layout(&layout_info, None)
                .map_err(|e| {
                    let layout = format_args!("\"bindless set layout\" of {contents}");
                    vk_error("vkCreateDescriptorSetLayout", layout, e)
                })?;

            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: TEXTURES,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: SAMPLERS as u32,
                },
            ];
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                .max_sets(1)
                .pool_sizes(&sizes);
            self.pool = device
                .create_descriptor_pool(&pool_info, None)
                .map_err(|e| {
                    let pool = format_args!("\"bindless pool\" of {contents}");
                    vk_error("vkCreateDescriptorPool", pool, e)
                })?;
            let layouts = [self.layout];
            let allocate_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(self.pool)
                .set_layouts(&layouts);
            self.set = device
                .allocate_descriptor_sets(&allocate_info)
                .map_err(|e| vk_error("vkAllocateDescriptorSets", "\"bindless set\"", e))?[0];
            // Written into the set once, before any pipeline uses it, rather than made immutable
            // in its layout: AMD's driver (25.10.30) reads immutable samplers that shaders index
            // non-uniformly as a sampler of zeros, which filters by the nearest texel and never
            // leaves a texture's first level.
            let samplers: Vec<_> = (self.samplers.iter())
                .map(|&sampler| vk::DescriptorImageInfo::default().sampler(sampler))
                .collect();
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&samplers);
            device.update_descriptor_sets(&[write], &[]);

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
                .map_err(|e| {
                    let layout = format_args!(
                        "\"bindless pipeline layout\" with {PUSH_CONSTANTS} bytes of push constants"
                    );
                    vk_error("vkCreatePipelineLayout", layout, e)
                })?;
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
            for (&sampler, name) in self.samplers.iter().zip(sampler_names()) {
                device.set_name(sampler, &name);
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

    /// Puts `view`, of the image named `name`, in texture slot `slot`, a placed one, or with `None`
    /// a free slot past them, and returns the slot's index: [`Bindless::reserve`], then
    /// [`Bindless::write`].
    ///
    /// # Safety
    ///
    /// As for [`Bindless::write`].
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
        // SAFETY: the caller guarantees the device and view.
        unsafe { self.write(device, view, index) };
        Ok(index)
    }

    /// Takes texture slot `slot`, a placed one, or with `None` a free slot past them, for the view
    /// [`Bindless::write`] puts in it, of the image named `name`, and returns the slot's index. A
    /// texture reserves its slot before it makes its image, so that a refused slot leaves nothing
    /// to leak.
    ///
    /// # Panics
    ///
    /// If `slot` isn't placed, or is in use.
    pub(crate) fn reserve(&self, slot: Option<u32>, name: &str) -> Result<u32> {
        self.lock().take(slot, name)
    }

    /// Puts `view` in texture slot `index`, which [`Bindless::reserve`] gave.
    ///
    /// # Safety
    ///
    /// `device` must be the set's, and `view` a live view of a sampled image of it, in the
    /// shader-read-only layout whenever a shader samples it.
    pub(crate) unsafe fn write(&self, device: &ash::Device, view: vk::ImageView, index: u32) {
        let _slots = self.lock();
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
        // flight uses the slot, which was free when it was reserved.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Frees a texture slot: once the GPU is done with the frames that may use it, or at once for
    /// one reserved and never written.
    pub(crate) fn remove(&self, index: u32) {
        self.lock().release(index);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
