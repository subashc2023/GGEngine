//! Choosing a GPU, and the logical device with its queues.

use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ash::prelude::VkResult;
use ash::{ext, khr, vk};
use gg_core::{Error, Result};
use tracing::{debug, info};

use crate::{Instance, REQUIRED_VERSION, Surface, Version, vk_error};

/// A GPU opened for rendering, with its queues.
pub struct Device {
    /// The instance, which must outlive the device.
    _instance: Arc<Instance>,
    physical: vk::PhysicalDevice,
    raw: ash::Device,
    name: String,
    memory: vk::PhysicalDeviceMemoryProperties,
    debug_utils: Option<ext::debug_utils::Device>,
    swapchain_fn: Option<khr::swapchain::Device>,
    graphics: Queue,
    compute: Option<Queue>,
    transfer: Option<Queue>,
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

        // SAFETY: the physical device came from this instance.
        let (families, memory) = unsafe {
            (
                instance
                    .raw()
                    .get_physical_device_queue_family_properties(physical),
                instance
                    .raw()
                    .get_physical_device_memory_properties(physical),
            )
        };
        let find = |wanted: vk::QueueFlags, unwanted: vk::QueueFlags| {
            families
                .iter()
                .zip(0u32..)
                .find(|(family, _)| {
                    family.queue_flags.contains(wanted) && !family.queue_flags.intersects(unwanted)
                })
                .map(|(_, index)| index)
        };
        // Async compute and transfer get queues of their own where the GPU has queue families
        // dedicated to them.
        let compute = find(vk::QueueFlags::COMPUTE, vk::QueueFlags::GRAPHICS);
        let transfer = find(
            vk::QueueFlags::TRANSFER,
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
        );
        let priorities = [1.0];
        let queue_infos: Vec<_> = [Some(graphics), compute, transfer]
            .into_iter()
            .flatten()
            .map(|family| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&priorities)
            })
            .collect();
        let extensions = if surface.is_some() {
            vec![vk::KHR_SWAPCHAIN_NAME.as_ptr()]
        } else {
            Vec::new()
        };
        let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extensions)
            .push_next(&mut vulkan12)
            .push_next(&mut vulkan13);
        // SAFETY: the physical device came from this instance, every Vulkan 1.4 device has these
        // features, and the create info's pointers outlive the call.
        let raw = unsafe { instance.raw().create_device(physical, &create_info, None) }
            .map_err(|e| vk_error("vkCreateDevice", e))?;

        let queue = |family| Queue {
            family,
            // SAFETY: the device was created with one queue in each of these families.
            raw: Mutex::new(unsafe { raw.get_device_queue(family, 0) }),
        };
        let device = Self {
            graphics: queue(graphics),
            compute: compute.map(queue),
            transfer: transfer.map(queue),
            debug_utils: instance
                .has_debug_utils()
                .then(|| ext::debug_utils::Device::new(instance.raw(), &raw)),
            swapchain_fn: surface.map(|_| khr::swapchain::Device::new(instance.raw(), &raw)),
            _instance: instance.clone(),
            physical,
            name: chosen.name.clone(),
            memory,
            raw,
        };
        // SAFETY: the device and its queues are live.
        unsafe {
            device.set_name(device.raw.handle(), &device.name);
            device.set_name(*device.graphics.lock(), "graphics queue");
            if let Some(queue) = &device.compute {
                device.set_name(*queue.lock(), "compute queue");
            }
            if let Some(queue) = &device.transfer {
                device.set_name(*queue.lock(), "transfer queue");
            }
        }
        info!(
            name = chosen.name,
            kind = chosen.kind(),
            vulkan = %Version(chosen.api),
            driver = chosen.driver,
            vram_mib = chosen.vram_mib,
            "GPU selected"
        );
        debug!(graphics, compute, transfer, "Queue families");
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
    pub fn wait_idle(&self) -> Result<()> {
        // Vulkan requires that nothing uses a queue while the device waits.
        let _queues = (
            self.graphics.lock(),
            self.compute.as_ref().map(Queue::lock),
            self.transfer.as_ref().map(Queue::lock),
        );
        // SAFETY: the device is live, and every queue's lock is held.
        unsafe { self.raw.device_wait_idle() }.map_err(|e| vk_error("vkDeviceWaitIdle", e))
    }

    pub(crate) fn physical(&self) -> vk::PhysicalDevice {
        self.physical
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

    /// Presents through the graphics queue, which can present to the surface the device was
    /// opened for. Returns whether the swapchain is suboptimal.
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

    /// A memory type allowed by `bits` with the `required` properties, preferring one that also
    /// has the `preferred` ones.
    pub(crate) fn memory_type(
        &self,
        bits: u32,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        let find = |flags: vk::MemoryPropertyFlags| {
            self.memory
                .memory_types_as_slice()
                .iter()
                .zip(0u32..)
                .find(|(memory_type, index)| {
                    bits & (1 << index) != 0 && memory_type.property_flags.contains(flags)
                })
                .map(|(_, index)| index)
        };
        find(required | preferred).or_else(|| find(required))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: everything made from the device holds an Arc to it, so only the device is
        // left; waiting first lets the GPU finish.
        unsafe {
            let _ = self.raw.device_wait_idle();
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
        let candidates = [
            old,
            candidate(1, "AMD Radeon(TM) Graphics", Kind::INTEGRATED_GPU, 512),
            candidate(2, "NVIDIA GeForce RTX 4090", Kind::DISCRETE_GPU, 24000),
            candidate(3, "llvmpipe", Kind::CPU, 0),
        ];
        assert_eq!(pick(&candidates, None).unwrap().index, 2);
        assert_eq!(pick(&candidates, Some("amd")).unwrap().index, 1);
        assert_eq!(pick(&candidates, Some("LLVMPIPE")).unwrap().index, 3);
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
