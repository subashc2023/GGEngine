//! Choosing a GPU, and the logical device with its queues.

use std::ffi::{CStr, CString};
use std::fmt;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use ash::prelude::VkResult;
use ash::{ext, khr, vk};
use gg_core::{Error, Result};
use tracing::{debug, info, warn};

use crate::bindless::Bindless;
use crate::pipeline_cache::{self, Identity, Refusal};
use crate::vma::{Allocation, Allocator};
use crate::{
    DepthImage, Instance, MAX_TEXTURE_SIZE, REQUIRED_VERSION, Surface, Texture, Version, vk_error,
};

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

/// How meshes and textures reach the GPU's memory: the GPU's best way, or one a test or a
/// measurement asks for. Whichever it is, the frame that takes them waits for them and draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadPath {
    /// Copied by a queue apart from graphics, beside the frames the GPU renders: the way of GPUs
    /// with a queue family for transfers, or for compute, apart from graphics.
    TransferQueue,
    /// Textures written by the CPU straight into their images (host image copy), and buffers
    /// copied by the graphics queue: the way of GPUs without such a family whose drivers say that
    /// costs the GPU nothing in reading the images.
    HostImageCopy,
    /// Copied by the graphics queue, at the start of the frame that takes them: the other GPUs'.
    GraphicsQueue,
}

impl UploadPath {
    /// Every way, for tests that try each.
    pub const ALL: [Self; 3] = [
        Self::TransferQueue,
        Self::HostImageCopy,
        Self::GraphicsQueue,
    ];
}

impl fmt::Display for UploadPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TransferQueue => "transfer queue",
            Self::HostImageCopy => "host image copy",
            Self::GraphicsQueue => "graphics queue",
        })
    }
}

/// A GPU opened for rendering, with its queues and its memory allocator.
pub struct Device {
    /// The instance, which must outlive the device.
    instance: Arc<Instance>,
    physical: vk::PhysicalDevice,
    raw: ash::Device,
    name: String,
    limits: vk::PhysicalDeviceLimits,
    /// The GPU and driver its pipeline cache's data is for.
    identity: Identity,
    /// What pipelines are made through, once [`Device::load_pipeline_cache`] has made it.
    /// Destroyed by hand, before the device is.
    pipeline_cache: OnceLock<vk::PipelineCache>,
    /// Dropped by hand, before the device is destroyed.
    allocator: ManuallyDrop<Allocator>,
    /// Destroyed by hand, before the device is.
    bindless: Bindless,
    retirement: Mutex<Retirement>,
    debug_utils: Option<ext::debug_utils::Device>,
    swapchain_fn: Option<khr::swapchain::Device>,
    graphics: Queue,
    /// The queue uploads are copied on, for a device that uploads through a transfer queue.
    transfer: Option<Queue>,
    /// The host image copy functions, for a device that uploads by host image copy.
    host_image_copy_fn: Option<ext::host_image_copy::Device>,
    uploads: UploadPath,
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
    /// GPUs come first, then integrated ones; among equals, the one with the most memory. It
    /// uploads by `uploads`, if given, and otherwise by the best way the GPU has.
    ///
    /// # Panics
    ///
    /// If `surface` was made by another instance.
    pub fn new(
        instance: &Arc<Instance>,
        gpu: Option<&str>,
        surface: Option<&Surface>,
        uploads: Option<UploadPath>,
    ) -> Result<Arc<Self>> {
        let _zone = gg_core::zone!();
        if let Some(surface) = surface {
            surface.check_instance(instance);
        }
        // SAFETY: the instance is live.
        let physicals = unsafe { instance.raw().enumerate_physical_devices() }
            .map_err(|e| vk_error("vkEnumeratePhysicalDevices", "the instance's GPUs", e))?;
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
                transfer = candidate.transfer,
                host_image_copy = candidate.host_copy.name(),
                problem = candidate.problem.as_deref(),
                "GPU found"
            );
        }
        let chosen = pick(&candidates, gpu)?;
        let uploads = chosen.upload_path(uploads)?;
        let physical = chosen.physical;
        let graphics = chosen
            .graphics
            .expect("a GPU without problems has a graphics queue");
        let transfer = chosen
            .transfer
            .filter(|_| uploads == UploadPath::TransferQueue);

        let priorities = [1.0];
        let mut queue_infos = vec![
            vk::DeviceQueueCreateInfo::default()
                .queue_family_index(graphics)
                .queue_priorities(&priorities),
        ];
        // A family for transfers has no graphics, so it's another family.
        if let Some(transfer) = transfer {
            queue_infos.push(
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(transfer)
                    .queue_priorities(&priorities),
            );
        }
        let mut extensions = Vec::new();
        if surface.is_some() {
            extensions.push(vk::KHR_SWAPCHAIN_NAME.as_ptr());
        }
        if uploads == UploadPath::HostImageCopy {
            extensions.push(vk::EXT_HOST_IMAGE_COPY_NAME.as_ptr());
        }
        // Scalar block layout lets shaders read vertices laid out as `#[repr(C)]` lays them out.
        let mut vulkan12 = bindless_features()
            .timeline_semaphore(true)
            .buffer_device_address(true)
            .scalar_block_layout(true);
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let mut host_image_copy =
            vk::PhysicalDeviceHostImageCopyFeaturesEXT::default().host_image_copy(true);
        // For models' textures, which recede from the camera.
        let features = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(true);
        let mut create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extensions)
            .enabled_features(&features)
            .push_next(&mut vulkan12)
            .push_next(&mut vulkan13);
        if uploads == UploadPath::HostImageCopy {
            create_info = create_info.push_next(&mut host_image_copy);
        }
        // SAFETY: the physical device came from this instance, every Vulkan 1.4 device has these
        // features but the bindless ones and host image copy, which `Candidate::new` checked
        // (Vulkan 1.4 requires sampler anisotropy, in vk.xml's feature requirements), and the
        // create info's pointers outlive the call.
        let raw =
            unsafe { instance.raw().create_device(physical, &create_info, None) }.map_err(|e| {
                let device = format_args!("device {:?}, uploading by {uploads}", chosen.name);
                vk_error("vkCreateDevice", device, e)
            })?;
        let debug_utils = instance
            .has_debug_utils()
            .then(|| ext::debug_utils::Device::new(instance.raw(), &raw));
        // SAFETY: the device was just made from this physical device, with buffer device
        // addresses, and `Device` keeps the instance alive and drops the allocator first; the debug
        // utils are the device's.
        let allocator = unsafe {
            Allocator::new(
                instance.entry(),
                instance.raw(),
                physical,
                &raw,
                debug_utils.as_ref(),
                &chosen.name,
            )
        };
        let allocator = match allocator {
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
            transfer: transfer.map(|family| Queue {
                family,
                // SAFETY: the device was created with one queue in this family.
                raw: Mutex::new(unsafe { raw.get_device_queue(family, 0) }),
            }),
            host_image_copy_fn: (uploads == UploadPath::HostImageCopy)
                .then(|| ext::host_image_copy::Device::new(instance.raw(), &raw)),
            uploads,
            debug_utils,
            swapchain_fn: surface.map(|_| khr::swapchain::Device::new(instance.raw(), &raw)),
            instance: instance.clone(),
            physical,
            name: chosen.name.clone(),
            limits: chosen.limits,
            identity: chosen.identity,
            pipeline_cache: OnceLock::new(),
            allocator: ManuallyDrop::new(allocator),
            bindless,
            retirement: Mutex::default(),
            raw,
        };
        // SAFETY: the device, its queues and the bindless set are live, and the set is the
        // device's.
        unsafe {
            device.set_name(device.raw.handle(), &device.name);
            device.set_name(*device.graphics.lock(), "graphics queue");
            if let Some(transfer) = &device.transfer {
                device.set_name(*transfer.lock(), "transfer queue");
            }
            device.bindless.set_names(&device);
        }
        info!(
            name = chosen.name,
            kind = chosen.kind(),
            vulkan = %Version(chosen.api),
            driver = chosen.driver,
            vram_mib = chosen.vram_mib,
            uploads = %uploads,
            "GPU selected"
        );
        debug!(graphics, transfer, "Queue families");
        Ok(Arc::new(device))
    }

    /// The GPU's name, such as `NVIDIA GeForce RTX 4090`. The device has it as its debug name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How the device uploads meshes and textures.
    pub fn upload_path(&self) -> UploadPath {
        self.uploads
    }

    /// Gives the device a pipeline cache, which every pipeline made from then on goes through,
    /// and returns whether it holds `data`: what [`Device::pipeline_cache`] gave on a device of
    /// the same GPU and driver, whole. Other data starts it empty: logged at debug if it's another
    /// GPU's, driver's or engine's, and with a `gpu.pipeline_cache` warning if it's damaged, which
    /// no driver is given, since drivers needn't survive it, or if the driver fails to load it, as
    /// when it runs out of memory.
    ///
    /// # Panics
    ///
    /// If the device has a pipeline cache already.
    pub fn load_pipeline_cache(&self, data: &[u8]) -> Result<bool> {
        let _zone = gg_core::zone!();
        assert!(
            self.pipeline_cache.get().is_none(),
            "a device loads its pipeline cache once"
        );
        let mut payload = match pipeline_cache::open(&self.identity, data) {
            Ok(payload) => Some(payload),
            Err(Refusal::Empty) => None,
            Err(Refusal::Foreign(why)) => {
                debug!(gpu = self.name, "Pipeline cache not loaded: {why}");
                None
            }
            Err(Refusal::Damaged(why)) => {
                warn!(
                    code = "gpu.pipeline_cache",
                    gpu = self.name,
                    "The saved pipeline cache is damaged: {why}; pipelines are made anew"
                );
                None
            }
        };
        let make = |data: &[u8]| {
            let info = vk::PipelineCacheCreateInfo::default().initial_data(data);
            // SAFETY: the device is live, and the data, if any, is what vkGetPipelineCacheData
            // gave for this GPU and driver, whole, as its header shows.
            unsafe { self.raw.create_pipeline_cache(&info, None) }
        };
        let cache = match payload.map(make) {
            Some(Ok(cache)) => cache,
            made => {
                if let Some(Err(e)) = made {
                    let cache = format_args!("device {:?}'s pipeline cache", self.name);
                    let error = vk_error("vkCreatePipelineCache", cache, e);
                    warn!(
                        code = "gpu.pipeline_cache",
                        gpu = self.name,
                        "The saved pipeline cache can't be loaded: {error:#}; pipelines are made \
                         anew"
                    );
                    payload = None;
                }
                make(&[]).map_err(|e| {
                    let cache = format_args!("device {:?}'s pipeline cache", self.name);
                    vk_error("vkCreatePipelineCache", cache, e)
                })?
            }
        };
        // SAFETY: the cache is live.
        unsafe { self.set_name(cache, "pipeline cache") };
        if self.pipeline_cache.set(cache).is_err() {
            // SAFETY: the cache was just made, and nothing has used it.
            unsafe { self.raw.destroy_pipeline_cache(cache, None) };
            panic!("a device loads its pipeline cache once");
        }
        let loaded_bytes = payload.map_or(0, <[u8]>::len);
        debug!(gpu = self.name, loaded_bytes, "Pipeline cache made");
        Ok(payload.is_some())
    }

    /// What the device's pipeline cache holds, for [`Device::load_pipeline_cache`] in a later run;
    /// `None` if it has none.
    pub fn pipeline_cache(&self) -> Result<Option<Vec<u8>>> {
        let _zone = gg_core::zone!();
        let Some(&cache) = self.pipeline_cache.get() else {
            return Ok(None);
        };
        // SAFETY: the cache is live and the device's; reading its data needs no exclusive access.
        let payload = unsafe { self.raw.get_pipeline_cache_data(cache) }.map_err(|e| {
            let cache = format_args!("device {:?}'s pipeline cache", self.name);
            vk_error("vkGetPipelineCacheData", cache, e)
        })?;
        Ok(Some(pipeline_cache::seal(&self.identity, &payload)))
    }

    /// The pipeline cache pipelines are made through: null until one is loaded. Making pipelines
    /// through it needs no lock, since the device doesn't enable pipeline cache control, without
    /// which caches are synchronized internally.
    pub(crate) fn raw_pipeline_cache(&self) -> vk::PipelineCache {
        self.pipeline_cache.get().copied().unwrap_or_default()
    }

    /// The ash device, for code that breaks the safe API's rules on purpose, such as tests that
    /// check validation catches mistakes. Every call through it is unsafe.
    pub fn raw(&self) -> &ash::Device {
        &self.raw
    }

    /// The device's handle, by which objects tell the device that made them.
    pub(crate) fn handle(&self) -> vk::Device {
        self.raw.handle()
    }

    /// Checks that an object, which `what` names, is this device's: `made_by` is the handle of
    /// the device that made it. Vulkan lets an object be used only with the device that made it,
    /// and drivers check nothing.
    ///
    /// # Panics
    ///
    /// If another device made it.
    pub(crate) fn check_made(&self, what: &str, made_by: vk::Device) {
        assert!(
            made_by == self.raw.handle(),
            "{what} was made by another device ({made_by:?}) than {} ({:?}), which it's used \
             with; an object is used only with the device that made it",
            self.name,
            self.raw.handle(),
        );
    }

    pub(crate) fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The GPU's limits, which what's recorded keeps within.
    pub(crate) fn limits(&self) -> &vk::PhysicalDeviceLimits {
        &self.limits
    }

    /// Waits until the GPU has finished all the work submitted to it.
    pub(crate) fn wait_idle(&self) -> Result<()> {
        // Vulkan requires that nothing uses a queue while the device waits. The locks are always
        // taken in this order.
        let _graphics = self.graphics.lock();
        let _transfer = self.transfer.as_ref().map(Queue::lock);
        // SAFETY: the device is live, and its queues' locks are held.
        unsafe { self.raw.device_wait_idle() }.map_err(|e| {
            vk_error(
                "vkDeviceWaitIdle",
                format_args!("device {:?}", self.name),
                e,
            )
        })
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

    /// The transfer queue's family, for a device that uploads through one.
    pub(crate) fn transfer_family(&self) -> Option<u32> {
        self.transfer.as_ref().map(|queue| queue.family)
    }

    /// The host image copy functions, which a device that uploads by host image copy has.
    pub(crate) fn host_image_copy_fn(&self) -> Option<&ext::host_image_copy::Device> {
        self.host_image_copy_fn.as_ref()
    }

    /// The swapchain functions, which a device opened for a surface has.
    pub(crate) fn swapchain_fn(&self) -> Option<&khr::swapchain::Device> {
        self.swapchain_fn.as_ref()
    }

    /// Submits `work`, as its errors call it, to the graphics queue.
    ///
    /// # Safety
    ///
    /// The submissions must be valid, and what they use must live until they finish.
    pub(crate) unsafe fn submit(
        &self,
        submits: &[vk::SubmitInfo2<'_>],
        work: impl fmt::Display,
    ) -> Result<()> {
        let queue = self.graphics.lock();
        // SAFETY: the caller guarantees the submissions; the lock is held.
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }.map_err(|e| {
            let submitted = format_args!("{work} on device {:?}'s graphics queue", self.name);
            vk_error("vkQueueSubmit2", submitted, e)
        })
    }

    /// Submits `work`, as its errors call it, to the transfer queue.
    ///
    /// # Safety
    ///
    /// As for [`Device::submit`].
    ///
    /// # Panics
    ///
    /// If the device doesn't upload through a transfer queue.
    pub(crate) unsafe fn submit_transfers(
        &self,
        submits: &[vk::SubmitInfo2<'_>],
        work: impl fmt::Display,
    ) -> Result<()> {
        let transfer = self.transfer.as_ref();
        let queue = transfer
            .expect("only a device that uploads through a transfer queue submits to one")
            .lock();
        // SAFETY: the caller guarantees the submissions; the lock is held.
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }.map_err(|e| {
            let submitted = format_args!("{work} on device {:?}'s transfer queue", self.name);
            vk_error("vkQueueSubmit2", submitted, e)
        })
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
    /// finish: for rare work outside of frames, such as reading an image back. The commands, and
    /// their errors, are named `name`.
    ///
    /// # Safety
    ///
    /// `record` must record valid commands into the command buffer it's given.
    pub(crate) unsafe fn run_once(
        &self,
        name: &str,
        record: impl FnOnce(vk::CommandBuffer),
    ) -> Result<()> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::TRANSIENT)
            .queue_family_index(self.graphics.family);
        // SAFETY: the device is live.
        let pool = unsafe { self.raw.create_command_pool(&pool_info, None) }.map_err(|e| {
            vk_error(
                "vkCreateCommandPool",
                format_args!("command pool {name:?}"),
                e,
            )
        })?;
        // SAFETY: the pool is live.
        unsafe { self.set_name(pool, name) };
        let ran = (|| {
            let allocate_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let commands_named = format_args!("command buffer {name:?}");
            // SAFETY: the pool is live, and the command buffer is recorded, submitted and waited
            // on in turn; the caller guarantees what's recorded.
            unsafe {
                let commands = self
                    .raw
                    .allocate_command_buffers(&allocate_info)
                    .map_err(|e| vk_error("vkAllocateCommandBuffers", commands_named, e))?[0];
                self.set_name(commands, name);
                let begin_info = vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
                self.raw
                    .begin_command_buffer(commands, &begin_info)
                    .map_err(|e| vk_error("vkBeginCommandBuffer", commands_named, e))?;
                record(commands);
                self.raw
                    .end_command_buffer(commands)
                    .map_err(|e| vk_error("vkEndCommandBuffer", commands_named, e))?;
                let command_infos =
                    [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
                let submit = vk::SubmitInfo2::default().command_buffer_infos(&command_infos);
                let queue = self.graphics.lock();
                let submitted = format_args!(
                    "{commands_named} on device {:?}'s graphics queue",
                    self.name
                );
                self.raw
                    .queue_submit2(*queue, &[submit], vk::Fence::null())
                    .map_err(|e| vk_error("vkQueueSubmit2", submitted, e))?;
                self.raw
                    .queue_wait_idle(*queue)
                    .map_err(|e| vk_error("vkQueueWaitIdle", submitted, e))
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
        // allocator, its pipeline cache and retired resources are left; waiting first lets the
        // GPU finish, and the allocator goes before the device it allocates from. Pipelines
        // don't need the cache they were made through.
        unsafe {
            let _ = self.raw.device_wait_idle();
            let retired = std::mem::take(&mut self.lock_retirement().resources);
            for (_, resource) in retired {
                self.destroy(resource);
            }
            if let Some(&cache) = self.pipeline_cache.get() {
                self.raw.destroy_pipeline_cache(cache, None);
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
    limits: vk::PhysicalDeviceLimits,
    identity: Identity,
    /// A queue family with graphics that can also present to the surface, if there is one.
    graphics: Option<u32>,
    /// A queue family for uploads apart from graphics, if there is one.
    transfer: Option<u32>,
    host_copy: HostCopy,
    problem: Option<String>,
}

/// Whether a GPU can write the engine's textures from the CPU (host image copy), and whether its
/// driver says that costs the GPU anything in reading them, such as compression it turns off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostCopy {
    Unsupported,
    Costly,
    Free,
}

impl HostCopy {
    fn name(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Costly => "costly",
            Self::Free => "free",
        }
    }
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
        let transfer = transfer_family(&families);
        let extensions = device_extensions(instance, physical);
        // Host image copy's queries use Vulkan 1.3's, and a device without 1.4 is refused anyway.
        let host_copy = if properties.api_version >= REQUIRED_VERSION {
            host_copy_support(instance, physical, &extensions)
        } else {
            HostCopy::Unsupported
        };
        let problem = if properties.api_version < REQUIRED_VERSION {
            Some(format!(
                "supports Vulkan {}, not 1.4; a newer driver may add it",
                Version(properties.api_version)
            ))
        } else if graphics.is_none() && surface.is_some() {
            Some("has no graphics queue that can present to the window".to_owned())
        } else if graphics.is_none() {
            Some("has no graphics queue".to_owned())
        } else if surface.is_some() && !has_extension(&extensions, vk::KHR_SWAPCHAIN_NAME) {
            Some("doesn't support VK_KHR_swapchain".to_owned())
        } else if !bindless {
            Some("doesn't support the descriptor indexing that bindless textures need".to_owned())
        } else if !has_depth(instance, physical) {
            Some("can't render depth into D32_SFLOAT images".to_owned())
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
            limits: properties.limits,
            identity: Identity::new(&properties),
            graphics,
            transfer,
            host_copy,
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

    /// How the GPU uploads: by `wanted`, if given; otherwise through a transfer queue if it has
    /// one, by host image copy if its driver says that's free, and through the graphics queue if
    /// neither.
    fn upload_path(&self, wanted: Option<UploadPath>) -> Result<UploadPath> {
        let Some(wanted) = wanted else {
            return Ok(if self.transfer.is_some() {
                UploadPath::TransferQueue
            } else if self.host_copy == HostCopy::Free {
                UploadPath::HostImageCopy
            } else {
                UploadPath::GraphicsQueue
            });
        };
        let missing = match wanted {
            UploadPath::TransferQueue if self.transfer.is_none() => {
                "has no queue family for transfers apart from graphics"
            }
            UploadPath::HostImageCopy if self.host_copy == HostCopy::Unsupported => {
                "can't write RGBA8 textures from the CPU"
            }
            _ => return Ok(wanted),
        };
        let message = format!(
            "uploads by {wanted} were asked for, but {} {missing}",
            self.name
        );
        Err(Error::new("gpu.no_upload_path", message))
    }
}

/// The SPIR-V capabilities a shader may declare on the devices the engine makes: those Vulkan 1.4
/// gives without a feature, and those of the features [`Device::new`] enables, as the Vulkan SDK
/// 1.4.335's vk.xml lists them under `spirvcapabilities`; they change with those features. A
/// shader that declares another needs a feature no device has enabled, which validation reports
/// in debug builds and a driver may crash on in others.
pub const SHADER_CAPABILITIES: [u32; 17] = [
    0,    // Matrix
    1,    // Shader
    40,   // InputAttachment
    43,   // Sampled1D
    44,   // Image1D
    46,   // SampledBuffer
    47,   // ImageBuffer
    49,   // StorageImageExtendedFormats
    50,   // ImageQuery
    51,   // DerivativeControl
    55,   // StorageImageReadWithoutFormat
    56,   // StorageImageWriteWithoutFormat
    4437, // DeviceGroup
    5301, // ShaderNonUniform
    5302, // RuntimeDescriptorArray, by runtimeDescriptorArray
    5307, // SampledImageArrayNonUniformIndexing, by shaderSampledImageArrayNonUniformIndexing
    5347, // PhysicalStorageBufferAddresses, by bufferDeviceAddress
];

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

/// Whether a device renders depth into depth images' format, D32_SFLOAT. Vulkan requires that of
/// it or of X8_D24_UNORM_PACK32, but nearly every device has it, AMD's among them, which have no
/// 24-bit depth, and Apple's GPUs, whose Metal has 32-bit float depth everywhere.
fn has_depth(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    // SAFETY: the physical device came from this instance.
    let properties = unsafe {
        instance
            .raw()
            .get_physical_device_format_properties(physical, DepthImage::FORMAT)
    };
    let features = properties.optimal_tiling_features;
    features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
}

/// The device's extensions; none, if they can't be listed.
fn device_extensions(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> Vec<vk::ExtensionProperties> {
    // SAFETY: the physical device came from this instance.
    unsafe {
        instance
            .raw()
            .enumerate_device_extension_properties(physical)
    }
    .unwrap_or_default()
}

fn has_extension(extensions: &[vk::ExtensionProperties], name: &CStr) -> bool {
    extensions
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(name))
}

/// The queue family that uploads go through apart from graphics, if there is one: of those
/// without graphics that can copy, as compute families can without saying so, the one that does
/// the fewest other kinds of work, then the first. A family for transfers alone is the GPU's copy
/// engine. Sparse binding and protected memory are features rather than kinds of work, and don't
/// count.
fn transfer_family(families: &[vk::QueueFamilyProperties]) -> Option<u32> {
    let copies = vk::QueueFlags::TRANSFER | vk::QueueFlags::COMPUTE;
    let features = vk::QueueFlags::SPARSE_BINDING | vk::QueueFlags::PROTECTED;
    families
        .iter()
        .zip(0u32..)
        .filter(|(family, _)| {
            family.queue_count > 0
                && family.queue_flags.intersects(copies)
                && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
        })
        .min_by_key(|&(family, index)| {
            let mut work = family.queue_flags & !features;
            if work.contains(vk::QueueFlags::COMPUTE) {
                work |= vk::QueueFlags::TRANSFER;
            }
            (work.as_raw().count_ones(), index)
        })
        .map(|(_, index)| index)
}

/// Whether the device can write the engine's textures from the CPU, and at what cost: host image
/// copy for both texture formats, at every size a texture has, into the shader-read-only layout
/// the bindless set declares textures in. Nothing past the extension's name is queried unless
/// the device lists the extension.
fn host_copy_support(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    extensions: &[vk::ExtensionProperties],
) -> HostCopy {
    if !has_extension(extensions, vk::EXT_HOST_IMAGE_COPY_NAME) {
        return HostCopy::Unsupported;
    }
    let raw = instance.raw();
    let mut feature = vk::PhysicalDeviceHostImageCopyFeaturesEXT::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut feature);
    // SAFETY: the physical device came from this instance, and lists the extension whose
    // features these are.
    unsafe { raw.get_physical_device_features2(physical, &mut features) };
    if feature.host_image_copy != vk::TRUE {
        return HostCopy::Unsupported;
    }
    let mut counted = vk::PhysicalDeviceHostImageCopyPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut counted);
    // SAFETY: as above; given no arrays, the call counts the layouts.
    unsafe { raw.get_physical_device_properties2(physical, &mut properties) };
    let mut layouts = vec![vk::ImageLayout::UNDEFINED; counted.copy_dst_layout_count as usize];
    let mut listed =
        vk::PhysicalDeviceHostImageCopyPropertiesEXT::default().copy_dst_layouts(&mut layouts);
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut listed);
    // SAFETY: as above; the array has room for as many layouts as the device counted.
    unsafe { raw.get_physical_device_properties2(physical, &mut properties) };
    if !layouts.contains(&vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) {
        return HostCopy::Unsupported;
    }
    let mut free = true;
    // Both texture formats.
    for format in [vk::Format::R8G8B8A8_UNORM, vk::Format::R8G8B8A8_SRGB] {
        let mut format_features = vk::FormatProperties3::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut format_features);
        // SAFETY: as above; the third format properties are core in Vulkan 1.3.
        unsafe { raw.get_physical_device_format_properties2(physical, format, &mut properties) };
        let host_transfer = vk::FormatFeatureFlags2::HOST_IMAGE_TRANSFER_EXT;
        if !format_features
            .optimal_tiling_features
            .contains(host_transfer)
        {
            return HostCopy::Unsupported;
        }
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(Texture::USAGE | vk::ImageUsageFlags::HOST_TRANSFER_EXT);
        let mut performance = vk::HostImageCopyDevicePerformanceQueryEXT::default();
        let mut properties = vk::ImageFormatProperties2::default().push_next(&mut performance);
        // SAFETY: as above.
        let supported = unsafe {
            raw.get_physical_device_image_format_properties2(physical, &info, &mut properties)
        };
        let largest = properties.image_format_properties.max_extent;
        if supported.is_err()
            || largest.width < MAX_TEXTURE_SIZE
            || largest.height < MAX_TEXTURE_SIZE
        {
            return HostCopy::Unsupported;
        }
        free &= performance.optimal_device_access == vk::TRUE;
    }
    if free {
        HostCopy::Free
    } else {
        HostCopy::Costly
    }
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
            limits: vk::PhysicalDeviceLimits::default(),
            identity: Identity::new(&vk::PhysicalDeviceProperties::default()),
            graphics: Some(0),
            transfer: None,
            host_copy: HostCopy::Unsupported,
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

    #[test]
    fn a_transfer_queue_is_the_family_apart_from_graphics_with_the_fewest_flags() {
        use vk::QueueFlags as Q;
        let families = |flags: &[Q]| {
            flags
                .iter()
                .map(|&queue_flags| vk::QueueFamilyProperties {
                    queue_flags,
                    queue_count: 1,
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        };
        let all = Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING;
        // An RTX 4090's: transfers alone, then compute, then video and optical flow.
        let nvidia = families(&[
            all,
            Q::TRANSFER | Q::SPARSE_BINDING,
            Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING,
            Q::TRANSFER | Q::VIDEO_DECODE_KHR,
            Q::TRANSFER | Q::VIDEO_ENCODE_KHR,
            Q::TRANSFER | Q::OPTICAL_FLOW_NV,
        ]);
        assert_eq!(transfer_family(&nvidia), Some(1));
        // An AMD iGPU's: compute before transfers alone, and video that can't copy.
        let amd = families(&[
            all,
            Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING,
            Q::TRANSFER | Q::SPARSE_BINDING,
            Q::VIDEO_DECODE_KHR,
            Q::VIDEO_ENCODE_KHR,
        ]);
        assert_eq!(transfer_family(&amd), Some(2));
        // lavapipe's one family has graphics.
        assert_eq!(transfer_family(&families(&[all])), None);
        // A compute family that leaves out the transfer bit it implies, as RADV may: it copies, but
        // a family for transfers alone after it does less else.
        let compute = families(&[Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, Q::COMPUTE]);
        assert_eq!(transfer_family(&compute), Some(1));
        let both = families(&[all, Q::COMPUTE, Q::TRANSFER]);
        assert_eq!(transfer_family(&both), Some(2));
        // Video before transfers alone.
        let video = families(&[all, Q::TRANSFER | Q::VIDEO_DECODE_KHR, Q::TRANSFER]);
        assert_eq!(transfer_family(&video), Some(2));
    }

    #[test]
    fn uploads_take_the_best_path_or_the_one_asked_for() {
        use UploadPath::{GraphicsQueue, HostImageCopy, TransferQueue};
        let gpu = |transfer, host_copy| Candidate {
            transfer,
            host_copy,
            ..candidate(0, "Some GPU", vk::PhysicalDeviceType::DISCRETE_GPU, 8192)
        };
        let path = |gpu: &Candidate, wanted| gpu.upload_path(wanted).expect("a path");
        let everything = gpu(Some(1), HostCopy::Free);
        assert_eq!(path(&everything, None), TransferQueue);
        assert_eq!(path(&everything, Some(TransferQueue)), TransferQueue);
        assert_eq!(path(&everything, Some(HostImageCopy)), HostImageCopy);
        assert_eq!(path(&everything, Some(GraphicsQueue)), GraphicsQueue);
        let free = gpu(None, HostCopy::Free);
        assert_eq!(path(&free, None), HostImageCopy);
        // A forced path doesn't need host copies to be free, only to work.
        let costly = gpu(None, HostCopy::Costly);
        assert_eq!(path(&costly, None), GraphicsQueue);
        assert_eq!(path(&costly, Some(HostImageCopy)), HostImageCopy);
        assert_eq!(path(&gpu(None, HostCopy::Unsupported), None), GraphicsQueue);

        let error = free
            .upload_path(Some(TransferQueue))
            .expect_err("no transfer family");
        assert_eq!(error.code(), "gpu.no_upload_path");
        assert!(
            error
                .to_string()
                .contains("Some GPU has no queue family for transfers"),
            "{error}"
        );
        let unsupported = gpu(Some(1), HostCopy::Unsupported);
        let error = unsupported
            .upload_path(Some(HostImageCopy))
            .expect_err("no host image copy");
        assert_eq!(error.code(), "gpu.no_upload_path");
    }
}
