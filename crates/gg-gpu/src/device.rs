//! Choosing a GPU, and the logical device with its queues.

use std::ffi::{CStr, CString};
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ash::prelude::VkResult;
use ash::{ext, khr, vk};
use gg_core::{Error, Result};
use tracing::{debug, info};

use crate::bindless::Bindless;
use crate::vma::{Allocation, Allocator};
use crate::{Instance, REQUIRED_VERSION, Surface, Version, vk_error};

/// A resource that was dropped while frames that may use it could still be running on the GPU.
pub(crate) enum Retired {
    Buffer(vk::Buffer, Allocation),
    Image(vk::Image, vk::ImageView, Allocation),
    /// An image with a slot in the bindless set.
    Texture(vk::Image, vk::ImageView, Allocation, u32),
    Pipeline(vk::Pipeline),
}

/// Retired resources, each with the last frame that may use it, and how far frames have got.
#[derive(Default)]
struct Retirement {
    /// Whether a [`Frames`](crate::Frames) has numbered the device's frames.
    claimed: bool,
    /// The last frame that began recording.
    begun: u64,
    /// The last frame the GPU finished, which the frames before it finished first.
    finished: u64,
    resources: Vec<(u64, Retired)>,
}

/// A GPU opened for rendering, with its graphics queue and its memory allocator.
pub struct Device {
    /// The instance, which must outlive the device.
    _instance: Arc<Instance>,
    physical: vk::PhysicalDevice,
    raw: ash::Device,
    name: String,
    /// Dropped by hand, before the device is destroyed.
    allocator: ManuallyDrop<Allocator>,
    /// Destroyed by hand, before the device is.
    bindless: Bindless,
    retirement: Mutex<Retirement>,
    debug_utils: Option<ext::debug_utils::Device>,
    swapchain_fn: Option<khr::swapchain::Device>,
    graphics: Queue,
}

/// A device queue. Vulkan lets one thread at a time use a queue, hence the lock.
struct Queue {
    family: u32,
    raw: Mutex<vk::Queue>,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, vk::Queue> {
        // A panic while the lock was held leaves the queue handle itself intact.
        self.raw.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Device {
    /// Opens the best GPU that supports Vulkan 1.4 and, given a surface, can present to it.
    /// Given `gpu`, only GPUs whose names contain it, ignoring case, are considered. Discrete
    /// GPUs come first, then integrated ones; among equals, the one with the most memory.
    pub fn new(
        instance: &Arc<Instance>,
        gpu: Option<&str>,
        surface: Option<&Surface>,
    ) -> Result<Arc<Self>> {
        let _zone = gg_core::zone!();
        // SAFETY: the instance is live.
        let physicals = unsafe { instance.raw().enumerate_physical_devices() }
            .map_err(|e| vk_error("vkEnumeratePhysicalDevices", e))?;
        let candidates: Vec<Candidate> = physicals
            .iter()
            .enumerate()
            .map(|(index, &physical)| Candidate::new(instance, index, physical, surface))
            .collect();
        for candidate in &candidates {
            debug!(
                index = candidate.index,
                name = candidate.name,
                kind = candidate.kind(),
                vulkan = %Version(candidate.api),
                driver = candidate.driver,
                vram_mib = candidate.vram_mib,
                problem = candidate.problem.as_deref(),
                "GPU found"
            );
        }
        let chosen = pick(&candidates, gpu)?;
        let physical = chosen.physical;
        let graphics = chosen
            .graphics
            .expect("a GPU without problems has a graphics queue");

        let priorities = [1.0];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(graphics)
            .queue_priorities(&priorities)];
        let extensions = if surface.is_some() {
            vec![vk::KHR_SWAPCHAIN_NAME.as_ptr()]
        } else {
            Vec::new()
        };
        // Scalar block layout lets shaders read vertices laid out as `#[repr(C)]` lays them out.
        let mut vulkan12 = bindless_features()
            .timeline_semaphore(true)
            .buffer_device_address(true)
            .scalar_block_layout(true);
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extensions)
            .push_next(&mut vulkan12)
            .push_next(&mut vulkan13);
        // SAFETY: the physical device came from this instance, every Vulkan 1.4 device has these
        // features but the bindless ones, which `Candidate::new` checked, and the create info's
        // pointers outlive the call.
        let raw = unsafe { instance.raw().create_device(physical, &create_info, None) }
            .map_err(|e| vk_error("vkCreateDevice", e))?;
        // SAFETY: the device was just made from this physical device, with buffer device
        // addresses, and `Device` keeps the instance alive and drops the allocator first.
        let allocator =
            match unsafe { Allocator::new(instance.entry(), instance.raw(), physical, &raw) } {
                Ok(allocator) => allocator,
                Err(error) => {
                    // SAFETY: nothing has been made from the device yet.
                    unsafe { raw.destroy_device(None) };
                    return Err(error);
                }
            };
        // SAFETY: the device has the bindless features, and `Device` destroys the set first.
        let bindless = match unsafe { Bindless::new(&raw) } {
            Ok(bindless) => bindless,
            Err(error) => {
                drop(allocator);
                // SAFETY: nothing else has been made from the device.
                unsafe { raw.destroy_device(None) };
                return Err(error);
            }
        };

        let device = Self {
            graphics: Queue {
                family: graphics,
                // SAFETY: the device was created with one queue in this family.
                raw: Mutex::new(unsafe { raw.get_device_queue(graphics, 0) }),
            },
            debug_utils: instance
                .has_debug_utils()
                .then(|| ext::debug_utils::Device::new(instance.raw(), &raw)),
            swapchain_fn: surface.map(|_| khr::swapchain::Device::new(instance.raw(), &raw)),
            _instance: instance.clone(),
            physical,
            name: chosen.name.clone(),
            allocator: ManuallyDrop::new(allocator),
            bindless,
            retirement: Mutex::default(),
            raw,
        };
        // SAFETY: the device, its queue and the bindless set are live, and the set is the
        // device's.
        unsafe {
            device.set_name(device.raw.handle(), &device.name);
            device.set_name(*device.graphics.lock(), "graphics queue");
            device.bindless.set_names(&device);
        }
        info!(
            name = chosen.name,
            kind = chosen.kind(),
            vulkan = %Version(chosen.api),
            driver = chosen.driver,
            vram_mib = chosen.vram_mib,
            "GPU selected"
        );
        debug!(graphics, "Queue family");
        Ok(Arc::new(device))
    }

    /// The GPU's name, such as `NVIDIA GeForce RTX 4090`. The device has it as its debug name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ash device, for code that breaks the safe API's rules on purpose, such as tests that
    /// check validation catches mistakes. Every call through it is unsafe.
    pub fn raw(&self) -> &ash::Device {
        &self.raw
    }

    /// Waits until the GPU has finished all the work submitted to it.
    pub(crate) fn wait_idle(&self) -> Result<()> {
        // Vulkan requires that nothing uses a queue while the device waits.
        let _queue = self.graphics.lock();
        // SAFETY: the device is live, and its one queue's lock is held.
        unsafe { self.raw.device_wait_idle() }.map_err(|e| vk_error("vkDeviceWaitIdle", e))
    }

    pub(crate) fn physical(&self) -> vk::PhysicalDevice {
        self.physical
    }

    pub(crate) fn allocator(&self) -> &Allocator {
        &self.allocator
    }

    /// Destroys `resource` once the GPU has finished every frame that may use it: at once if
    /// none is running.
    pub(crate) fn retire(&self, resource: Retired) {
        let mut retirement = self.lock_retirement();
        if retirement.begun <= retirement.finished {
            // SAFETY: every frame that began has finished, and a resource dropped now can't be
            // used in frames after it.
            unsafe { self.destroy(resource) };
        } else {
            let last_use = retirement.begun;
            retirement.resources.push((last_use, resource));
        }
    }

    /// Notes that a [`Frames`](crate::Frames) numbers the device's frames, from 1.
    ///
    /// # Panics
    ///
    /// If one has before: frame numbers would repeat, and resources retired by them would be
    /// destroyed while the GPU may still use them.
    pub(crate) fn claim_frames(&self) {
        let mut retirement = self.lock_retirement();
        assert!(
            !retirement.claimed,
            "a device has one Frames for its whole life"
        );
        retirement.claimed = true;
    }

    /// Notes that `frame` began recording, so resources retired from now on may be used in it.
    /// Frames are numbered from 1, by the one [`Frames`](crate::Frames) that claimed them.
    pub(crate) fn frame_begun(&self, frame: u64) {
        self.lock_retirement().begun = frame;
    }

    /// Notes that the GPU has finished `frame` and those before it, and destroys the resources
    /// they were the last to use.
    pub(crate) fn frames_finished(&self, frame: u64) {
        let mut retirement = self.lock_retirement();
        retirement.finished = frame;
        let mut i = 0;
        while i < retirement.resources.len() {
            if retirement.resources[i].0 <= frame {
                let (_, resource) = retirement.resources.swap_remove(i);
                // SAFETY: the frames that may use it have finished.
                unsafe { self.destroy(resource) };
            } else {
                i += 1;
            }
        }
    }

    fn lock_retirement(&self) -> MutexGuard<'_, Retirement> {
        // Destroying a resource doesn't panic midway, so the list is intact after a panic.
        self.retirement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// # Safety
    ///
    /// The GPU must be done with the resource.
    unsafe fn destroy(&self, resource: Retired) {
        // SAFETY: the caller guarantees it; each resource's parts were made together.
        unsafe {
            match resource {
                Retired::Buffer(buffer, allocation) => {
                    self.allocator.destroy_buffer(buffer, allocation);
                }
                Retired::Image(image, view, allocation) => {
                    self.raw.destroy_image_view(view, None);
                    self.allocator.destroy_image(image, allocation);
                }
                Retired::Texture(image, view, allocation, slot) => {
                    self.raw.destroy_image_view(view, None);
                    self.allocator.destroy_image(image, allocation);
                    self.bindless.remove(slot);
                }
                Retired::Pipeline(pipeline) => self.raw.destroy_pipeline(pipeline, None),
            }
        }
    }

    pub(crate) fn bindless(&self) -> &Bindless {
        &self.bindless
    }

    pub(crate) fn graphics_family(&self) -> u32 {
        self.graphics.family
    }

    /// The swapchain functions, which a device opened for a surface has.
    pub(crate) fn swapchain_fn(&self) -> Option<&khr::swapchain::Device> {
        self.swapchain_fn.as_ref()
    }

    /// Submits to the graphics queue.
    ///
    /// # Safety
    ///
    /// The submissions must be valid, and what they use must live until they finish.
    pub(crate) unsafe fn submit(&self, submits: &[vk::SubmitInfo2<'_>]) -> Result<()> {
        let queue = self.graphics.lock();
        // SAFETY: the caller guarantees the submissions; the lock is held.
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }
            .map_err(|e| vk_error("vkQueueSubmit2", e))
    }

    /// Presents through the graphics queue, which each swapchain checked it can present to.
    /// Returns whether a swapchain is suboptimal.
    ///
    /// # Safety
    ///
    /// The present info must be valid.
    pub(crate) unsafe fn present(&self, info: &vk::PresentInfoKHR<'_>) -> VkResult<bool> {
        let swapchain_fn = self
            .swapchain_fn()
            .expect("only a device opened for a surface presents");
        let queue = self.graphics.lock();
        // SAFETY: the caller guarantees the info; the lock is held.
        unsafe { swapchain_fn.queue_present(*queue, info) }
    }

    /// Records commands with `record`, runs them on the graphics queue and waits for them to
    /// finish: for rare work outside of frames, such as reading an image back.
    ///
    /// # Safety
    ///
    /// `record` must record valid commands into the command buffer it's given.
    pub(crate) unsafe fn run_once(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::TRANSIENT)
            .queue_family_index(self.graphics.family);
        // SAFETY: the device is live.
        let pool = unsafe { self.raw.create_command_pool(&pool_info, None) }
            .map_err(|e| vk_error("vkCreateCommandPool", e))?;
        // SAFETY: the pool is live.
        unsafe { self.set_name(pool, "one-off commands") };
        let ran = (|| {
            let allocate_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            // SAFETY: the pool is live, and the command buffer is recorded, submitted and waited
            // on in turn; the caller guarantees what's recorded.
            unsafe {
                let commands = self
                    .raw
                    .allocate_command_buffers(&allocate_info)
                    .map_err(|e| vk_error("vkAllocateCommandBuffers", e))?[0];
                self.set_name(commands, "one-off commands");
                let begin_info = vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
                self.raw
                    .begin_command_buffer(commands, &begin_info)
                    .map_err(|e| vk_error("vkBeginCommandBuffer", e))?;
                record(commands);
                self.raw
                    .end_command_buffer(commands)
                    .map_err(|e| vk_error("vkEndCommandBuffer", e))?;
                let command_infos =
                    [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
                let submit = vk::SubmitInfo2::default().command_buffer_infos(&command_infos);
                let queue = self.graphics.lock();
                self.raw
                    .queue_submit2(*queue, &[submit], vk::Fence::null())
                    .map_err(|e| vk_error("vkQueueSubmit2", e))?;
                self.raw
                    .queue_wait_idle(*queue)
                    .map_err(|e| vk_error("vkQueueWaitIdle", e))
            }
        })();
        // SAFETY: the queue is idle, or the work never started, so the pool is unused.
        unsafe { self.raw.destroy_command_pool(pool, None) };
        ran
    }

    /// Names a Vulkan object, for validation messages and for capture tools such as RenderDoc.
    ///
    /// # Safety
    ///
    /// `handle` must be a live object of this device or its instance.
    pub(crate) unsafe fn set_name(&self, handle: impl vk::Handle, name: &str) {
        let (Some(debug_utils), Ok(name)) = (&self.debug_utils, CString::new(name)) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(handle)
            .object_name(&name);
        // SAFETY: the caller guarantees the handle. Names are a debugging aid, so a failure to
        // set one is ignored.
        let _ = unsafe { debug_utils.set_debug_utils_object_name(&info) };
    }

    /// Opens a debug label, which captures and validation messages show around the commands
    /// recorded inside it.
    ///
    /// # Safety
    ///
    /// `commands` must be recording.
    pub(crate) unsafe fn begin_label(&self, commands: vk::CommandBuffer, name: &CStr) {
        if let Some(debug_utils) = &self.debug_utils {
            let label = vk::DebugUtilsLabelEXT::default().label_name(name);
            // SAFETY: the caller guarantees the command buffer is recording.
            unsafe { debug_utils.cmd_begin_debug_utils_label(commands, &label) };
        }
    }

    /// Closes the debug label [`Device::begin_label`] opened.
    ///
    /// # Safety
    ///
    /// `commands` must be recording, with a label open.
    pub(crate) unsafe fn end_label(&self, commands: vk::CommandBuffer) {
        if let Some(debug_utils) = &self.debug_utils {
            // SAFETY: the caller guarantees it.
            unsafe { debug_utils.cmd_end_debug_utils_label(commands) };
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: everything made from the device holds an Arc to it, so only the device, its
        // allocator and retired resources are left; waiting first lets the GPU finish, and the
        // allocator goes before the device it allocates from.
        unsafe {
            let _ = self.raw.device_wait_idle();
            let retired = std::mem::take(&mut self.lock_retirement().resources);
            for (_, resource) in retired {
                self.destroy(resource);
            }
            self.bindless.destroy(&self.raw);
            ManuallyDrop::drop(&mut self.allocator);
            self.raw.destroy_device(None);
        }
    }
}

/// A GPU the loader found, and what keeps the engine from using it, if anything.
struct Candidate {
    index: usize,
    physical: vk::PhysicalDevice,
    name: String,
    kind: vk::PhysicalDeviceType,
    api: u32,
    driver: String,
    vram_mib: u64,
    /// A queue family with graphics that can also present to the surface, if there is one.
    graphics: Option<u32>,
    problem: Option<String>,
}

impl Candidate {
    fn new(
        instance: &Instance,
        index: usize,
        physical: vk::PhysicalDevice,
        surface: Option<&Surface>,
    ) -> Self {
        let raw = instance.raw();
        // SAFETY: the physical device came from this instance.
        let (properties, memory, families) = unsafe {
            (
                raw.get_physical_device_properties(physical),
                raw.get_physical_device_memory_properties(physical),
                raw.get_physical_device_queue_family_properties(physical),
            )
        };
        let bindless = properties.api_version >= vk::API_VERSION_1_2 && {
            let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut vulkan12);
            // SAFETY: as above; the 1.2 features are core in Vulkan 1.2.
            unsafe { raw.get_physical_device_features2(physical, &mut features) };
            has_bindless(&vulkan12)
        };
        let name = properties.device_name_as_c_str().map_or_else(
            |_| format!("GPU {index}"),
            |name| name.to_string_lossy().into_owned(),
        );
        let driver = if properties.api_version >= vk::API_VERSION_1_2 {
            let mut driver = vk::PhysicalDeviceDriverProperties::default();
            let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut driver);
            // SAFETY: as above; the driver properties are core in Vulkan 1.2.
            unsafe { raw.get_physical_device_properties2(physical, &mut properties2) };
            let name = driver.driver_name_as_c_str().map(CStr::to_string_lossy);
            let info = driver.driver_info_as_c_str().map(CStr::to_string_lossy);
            format!("{} {}", name.unwrap_or_default(), info.unwrap_or_default())
        } else {
            format!("{:#x}", properties.driver_version)
        };
        let vram_mib = memory
            .memory_heaps_as_slice()
            .iter()
            .filter(|heap| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|heap| heap.size)
            .sum::<u64>()
            / (1024 * 1024);
        let presents = |family: u32| {
            surface.is_none_or(|surface| {
                // SAFETY: the physical device and the surface came from this instance.
                unsafe {
                    surface.surface_fn().get_physical_device_surface_support(
                        physical,
                        family,
                        surface.raw(),
                    )
                }
                .unwrap_or(false)
            })
        };
        let graphics = families
            .iter()
            .zip(0u32..)
            .find(|&(family, index)| {
                family.queue_flags.contains(vk::QueueFlags::GRAPHICS) && presents(index)
            })
            .map(|(_, index)| index);
        let problem = if properties.api_version < REQUIRED_VERSION {
            Some(format!(
                "supports Vulkan {}, not 1.4; a newer driver may add it",
                Version(properties.api_version)
            ))
        } else if graphics.is_none() && surface.is_some() {
            Some("has no graphics queue that can present to the window".to_owned())
        } else if graphics.is_none() {
            Some("has no graphics queue".to_owned())
        } else if surface.is_some() && !has_swapchain(instance, physical) {
            Some("doesn't support VK_KHR_swapchain".to_owned())
        } else if !bindless {
            Some("doesn't support the descriptor indexing that bindless textures need".to_owned())
        } else {
            None
        };
        Self {
            index,
            physical,
            name,
            kind: properties.device_type,
            api: properties.api_version,
            driver,
            vram_mib,
            graphics,
            problem,
        }
    }

    fn kind(&self) -> &'static str {
        match self.kind {
            vk::PhysicalDeviceType::DISCRETE_GPU => "discrete",
            vk::PhysicalDeviceType::INTEGRATED_GPU => "integrated",
            vk::PhysicalDeviceType::VIRTUAL_GPU => "virtual",
            vk::PhysicalDeviceType::CPU => "cpu",
            _ => "other",
        }
    }

    /// How strongly the GPU's kind is preferred: discrete, integrated, virtual, then the rest.
    fn rank(&self) -> u8 {
        match self.kind {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
            _ => 0,
        }
    }
}

/// The descriptor indexing features that the bindless set needs, which Vulkan 1.4 doesn't
/// require but every desktop driver has. A device that has `descriptorIndexing` can hold at
/// least 500,000 update-after-bind sampled images.
fn bindless_features() -> vk::PhysicalDeviceVulkan12Features<'static> {
    vk::PhysicalDeviceVulkan12Features::default()
        .runtime_descriptor_array(true)
        .descriptor_binding_partially_bound(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_update_unused_while_pending(true)
        .shader_sampled_image_array_non_uniform_indexing(true)
}

/// Whether a device has `descriptorIndexing` and each feature [`bindless_features`] enables.
fn has_bindless(features: &vk::PhysicalDeviceVulkan12Features<'_>) -> bool {
    [
        features.descriptor_indexing,
        features.runtime_descriptor_array,
        features.descriptor_binding_partially_bound,
        features.descriptor_binding_sampled_image_update_after_bind,
        features.descriptor_binding_update_unused_while_pending,
        features.shader_sampled_image_array_non_uniform_indexing,
    ]
    .iter()
    .all(|&feature| feature == vk::TRUE)
}

fn has_swapchain(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    // SAFETY: the physical device came from this instance.
    unsafe {
        instance
            .raw()
            .enumerate_device_extension_properties(physical)
    }
    .is_ok_and(|extensions| {
        extensions
            .iter()
            .any(|extension| extension.extension_name_as_c_str() == Ok(vk::KHR_SWAPCHAIN_NAME))
    })
}

/// Picks the GPU to use: of those without a problem whose names contain `gpu`, if given, the
/// best-ranked kind, then the most memory, then the first the loader listed.
fn pick<'a>(candidates: &'a [Candidate], gpu: Option<&str>) -> Result<&'a Candidate> {
    let wanted = gpu.map(str::to_lowercase);
    let named: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            wanted
                .as_deref()
                .is_none_or(|wanted| candidate.name.to_lowercase().contains(wanted))
        })
        .collect();
    let best = named
        .iter()
        .filter(|candidate| candidate.problem.is_none())
        .max_by(|a, b| (a.rank(), a.vram_mib, b.index).cmp(&(b.rank(), b.vram_mib, a.index)));
    if let Some(best) = best {
        return Ok(best);
    }
    let found = || {
        candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let message = if candidates.is_empty() {
        "the Vulkan loader found no GPU; install a GPU driver that supports Vulkan 1.4".to_owned()
    } else if let (Some(gpu), true) = (gpu, named.is_empty()) {
        format!("no GPU's name contains `{gpu}`; the GPUs are {}", found())
    } else {
        let problems: Vec<String> = named
            .iter()
            .map(|candidate| {
                let problem = candidate.problem.as_deref().unwrap_or_default();
                format!("{} {problem}", candidate.name)
            })
            .collect();
        format!("no GPU can run the engine: {}", problems.join("; "))
    };
    Err(Error::new("gpu.no_device", message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        index: usize,
        name: &str,
        kind: vk::PhysicalDeviceType,
        vram_mib: u64,
    ) -> Candidate {
        Candidate {
            index,
            physical: vk::PhysicalDevice::null(),
            name: name.to_owned(),
            kind,
            api: REQUIRED_VERSION,
            driver: String::new(),
            vram_mib,
            graphics: Some(0),
            problem: None,
        }
    }

    #[test]
    fn prefers_discrete_then_memory_and_honors_the_name_filter() {
        use vk::PhysicalDeviceType as Kind;
        let mut old = candidate(0, "Old Discrete", Kind::DISCRETE_GPU, 32768);
        old.problem = Some("supports Vulkan 1.3.0, not 1.4".to_owned());
        // The smaller discrete GPU comes first, so that only memory puts the larger ahead of it.
        let candidates = [
            old,
            candidate(1, "NVIDIA GeForce RTX 3050", Kind::DISCRETE_GPU, 8192),
            candidate(2, "AMD Radeon(TM) Graphics", Kind::INTEGRATED_GPU, 512),
            candidate(3, "NVIDIA GeForce RTX 4090", Kind::DISCRETE_GPU, 24000),
            candidate(4, "llvmpipe", Kind::CPU, 0),
        ];
        assert_eq!(pick(&candidates, None).unwrap().index, 3);
        assert_eq!(pick(&candidates, Some("amd")).unwrap().index, 2);
        assert_eq!(pick(&candidates, Some("LLVMPIPE")).unwrap().index, 4);
    }

    #[test]
    fn says_why_no_gpu_fits() {
        let mut old = candidate(0, "Old GPU", vk::PhysicalDeviceType::DISCRETE_GPU, 4096);
        old.problem = Some("supports Vulkan 1.3.0, not 1.4".to_owned());
        let candidates = [old];
        let error = pick(&candidates, None).err().expect("no GPU fits");
        assert_eq!(error.code(), "gpu.no_device");
        assert!(
            error.to_string().contains("Old GPU supports Vulkan 1.3.0"),
            "{error}"
        );
        let error = pick(&candidates, Some("nvidia"))
            .err()
            .expect("no GPU fits");
        assert!(
            error
                .to_string()
                .contains("no GPU's name contains `nvidia`"),
            "{error}"
        );
    }
}
