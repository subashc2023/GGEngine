//! The logical device with its queues.

mod candidate;

use std::ffi::{CStr, CString};
use std::fmt;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use ash::prelude::VkResult;
use ash::{ext, khr, vk};
use gg_core::{Error, Result};
use tracing::{debug, info, warn};

use crate::bindless::Bindless;
use crate::capture::{self, Capture, RenderDoc};
use crate::pipeline_cache::{self, Identity, Refusal};
use crate::vma::{Allocation, Allocator};
use crate::{Instance, Surface, Version, vk_error, vk_failed};
use candidate::{Candidate, pick};

/// A resource dropped while frames, or copies on the transfer queue, that may use it could still
/// be running on the GPU.
pub(crate) enum Retired {
    Buffer(vk::Buffer, Allocation),
    Image(vk::Image, vk::ImageView, Allocation),
    /// An image with a slot in the bindless sets.
    Texture(vk::Image, vk::ImageView, Allocation, u32),
    /// An image made in memory that others share, with its bindless slot, if it has one.
    Alias(vk::Image, vk::ImageView, Option<u32>),
    /// A view of a level of a storage image, with its storage slot in the bindless sets. It's
    /// retired before its image, so it goes first.
    StorageView(vk::ImageView, u32),
    /// Memory that images were made in, which goes once they're done with.
    Memory(Allocation),
    /// A texture's slot in the bindless sets, which goes apart from the images it showed: they go
    /// once nothing holds them.
    Slot(u32),
    Pipeline(vk::Pipeline),
    /// An acceleration structure. It's retired before the buffer it lives in, so it goes first.
    AccelerationStructure(vk::AccelerationStructureKHR),
}

/// Retired resources, each with the last frame and the last submission of copies that may use
/// it, and how far frames and copies have got.
#[derive(Default)]
struct Retirement {
    /// Whether a [`Frames`](crate::Frames) has claimed the device's frames: one does, or one that
    /// began a frame did.
    claimed: bool,
    /// The last frame that began recording.
    begun: u64,
    /// The last frame the GPU finished; those before it finished first.
    finished: u64,
    /// The submission of copies on the transfer queue being recorded, numbered from 1; 0 for
    /// none.
    transfer_recording: u64,
    /// The last submission of copies made, and the last the GPU finished; those before it
    /// finished first.
    transfers_submitted: u64,
    transfers_finished: u64,
    /// Each with the last frame and the last submission of copies that may use it.
    resources: Vec<(u64, u64, Retired)>,
}

impl Retirement {
    /// The last submission of copies that may use a resource dropped now: the one being recorded,
    /// else the last made.
    fn transfer_mark(&self) -> u64 {
        if self.transfer_recording > 0 {
            self.transfer_recording
        } else {
            self.transfers_submitted
        }
    }

    /// Whether the frames and copies up to `frame` and `transfer` have finished.
    fn done(&self, frame: u64, transfer: u64) -> bool {
        frame <= self.finished && transfer <= self.transfers_finished
    }

    /// Keeps `resource` until the last frame begun and the last submission of copies that may use
    /// it have finished, or gives it back to destroy at once if they have.
    fn retire(&mut self, resource: Retired) -> Option<Retired> {
        let (frame, transfer) = (self.begun, self.transfer_mark());
        if self.done(frame, transfer) {
            Some(resource)
        } else {
            self.resources.push((frame, transfer, resource));
            None
        }
    }

    /// Takes the resources whose frames and copies have all finished, to destroy.
    fn sweep(&mut self) -> impl Iterator<Item = Retired> + '_ {
        let (finished, transfers_finished) = (self.finished, self.transfers_finished);
        let done = move |&mut (frame, transfer, _): &mut (u64, u64, Retired)| {
            frame <= finished && transfer <= transfers_finished
        };
        (self.resources.extract_if(.., done)).map(|(_, _, resource)| resource)
    }
}

/// How meshes and textures reach the GPU's memory: the GPU's best way, or one a test or
/// measurement asks for. Frames copy them a part at a time, within a budget, and draw them once
/// every part is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadPath {
    /// Copied by a queue apart from graphics, beside the frames the GPU renders, which don't wait
    /// for the copies. The way of GPUs with a queue family for transfers, or for compute, apart
    /// from graphics.
    TransferQueue,
    /// Textures written by the CPU straight into their images (host image copy), buffers copied by
    /// the graphics queue. The way of GPUs without such a family whose drivers say that costs the
    /// GPU nothing in reading the images.
    HostImageCopy,
    /// Copied by the graphics queue, at the start of the frames that take their parts. The other
    /// GPUs'.
    GraphicsQueue,
}

/// How much of the GPU's own memory is in use, and how much the driver gives the process before
/// it starts paging, from [`Device::memory_budget`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBudget {
    pub usage: u64,
    pub budget: u64,
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
    /// The queue compute passes run on beside the graphics queue, for a device with a family for
    /// compute apart from graphics and from uploads. Otherwise they run on the graphics queue.
    compute: Option<Queue>,
    /// The families of the queues above, each once. Every buffer is shared by them all, so any
    /// queue reads what another wrote once it has waited for it, without a handover. Images are
    /// the graphics queue's alone, but for textures the transfer queue fills.
    families: Vec<u32>,
    /// The host image copy functions, for a device that uploads by host image copy.
    host_image_copy_fn: Option<ext::host_image_copy::Device>,
    uploads: UploadPath,
    /// The least a copy to an image moves on the queue that copies textures, in texels, or in
    /// blocks of a compressed image.
    copy_granularity: vk::Extent3D,
    /// How many bits of the graphics queue's timestamps count, and the compute queue's; 0 for a
    /// queue that writes none.
    timestamp_bits: u32,
    compute_timestamp_bits: u32,
    /// Reads the GPU's clock at once, for Tracy's timelines, with gg-core's `tracy` feature on a
    /// device whose driver can.
    calibrated: Option<khr::calibrated_timestamps::Device>,
    /// Which memory heaps are the GPU's own, by their indices' bits.
    device_local_heaps: u32,
    /// RenderDoc, for a device it can capture: one its layer is active on.
    renderdoc: Option<&'static RenderDoc>,
    mesh_shader_fn: Option<ext::mesh_shader::Device>,
    /// The acceleration structures' functions, for a device that traces rays.
    acceleration_structure_fn: Option<khr::acceleration_structure::Device>,
    /// `minAccelerationStructureScratchOffsetAlignment`, for a device that traces rays; else 0.
    scratch_alignment: u64,
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
    /// uploads by `uploads`, if given, else by the GPU's best way, draws through mesh shaders
    /// as `mesh_shaders` says, if given, else if the GPU has them, and traces rays from compute
    /// shaders as `ray_query` says, if given, else if the GPU can.
    ///
    /// Fails with `gpu.no_device` if no GPU can run the engine, `gpu.no_upload_path` if the GPU
    /// hasn't the upload path asked for, `gpu.no_mesh_shaders` if it hasn't the mesh shaders
    /// asked for, and `gpu.no_ray_query` if it hasn't the ray queries asked for.
    ///
    /// # Panics
    ///
    /// If `surface` was made by another instance.
    pub fn new(
        instance: &Arc<Instance>,
        gpu: Option<&str>,
        surface: Option<&Surface>,
        uploads: Option<UploadPath>,
        mesh_shaders: Option<bool>,
        ray_query: Option<bool>,
    ) -> Result<Arc<Self>> {
        let _zone = gg_core::zone!();
        if let Some(surface) = surface {
            surface.check_instance(instance);
        }
        // SAFETY: the instance is live.
        let physicals = unsafe { instance.raw().enumerate_physical_devices() }.map_err(
            vk_failed("vkEnumeratePhysicalDevices", "the instance's GPUs"),
        )?;
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
                compute = candidate.compute,
                host_image_copy = candidate.host_copy.name(),
                ray_query = candidate.ray_query,
                problem = candidate.problem.as_deref(),
                "GPU found"
            );
        }
        let chosen = pick(&candidates, gpu)?;
        let uploads = chosen.upload_path(uploads)?;
        let mesh_shaders = chosen.mesh_shaders(mesh_shaders)?;
        let ray_query = chosen.ray_query(ray_query)?;
        let physical = chosen.physical;
        let graphics = chosen
            .graphics
            .expect("a GPU without problems has a graphics queue");
        let transfer = chosen
            .transfer
            .filter(|_| uploads == UploadPath::TransferQueue);
        let compute = chosen.compute;
        // A transfer or compute family has no graphics, and the compute family isn't the transfer
        // family, so each is another family.
        let families: Vec<u32> = [Some(graphics), transfer, compute]
            .into_iter()
            .flatten()
            .collect();

        let priorities = [1.0];
        let queue_infos: Vec<_> = (families.iter())
            .map(|&family| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&priorities)
            })
            .collect();
        let calibrated = chosen.calibrated && gg_core::profile::ENABLED;
        let mut extensions = Vec::new();
        if calibrated {
            extensions.push(vk::KHR_CALIBRATED_TIMESTAMPS_NAME.as_ptr());
        }
        if surface.is_some() {
            extensions.push(vk::KHR_SWAPCHAIN_NAME.as_ptr());
        }
        if uploads == UploadPath::HostImageCopy {
            extensions.push(vk::EXT_HOST_IMAGE_COPY_NAME.as_ptr());
        }
        // For the memory budget, where the driver reports one.
        if chosen.memory_budget {
            extensions.push(vk::EXT_MEMORY_BUDGET_NAME.as_ptr());
        }
        if mesh_shaders {
            extensions.push(vk::EXT_MESH_SHADER_NAME.as_ptr());
        }
        // Acceleration structures require deferred host operations, though builds here don't
        // defer.
        if ray_query {
            extensions.extend([
                vk::KHR_ACCELERATION_STRUCTURE_NAME.as_ptr(),
                vk::KHR_RAY_QUERY_NAME.as_ptr(),
                vk::KHR_DEFERRED_HOST_OPERATIONS_NAME.as_ptr(),
            ]);
            // Where the GPU has it, without its features: Slang 2025.23.2, the Vulkan SDK
            // 1.4.335's, declares SPV_KHR_ray_tracing in every shader with ray queries, which
            // validation then refuses without it (VUID-VkShaderModuleCreateInfo-pCode-08742). Until
            // Slang 2026.11 is required, as docs/PLAN.md's row says.
            if chosen.ray_tracing_pipeline {
                extensions.push(vk::KHR_RAY_TRACING_PIPELINE_NAME.as_ptr());
            }
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
        let mut mesh_shader = vk::PhysicalDeviceMeshShaderFeaturesEXT::default().mesh_shader(true);
        let mut acceleration_structure =
            vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                .acceleration_structure(true);
        let mut ray_query_features =
            vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
        // Features, in order, for: models' textures, which recede from the camera; passes that
        // render IDs beside colors, whose targets blend apart as integers don't blend; games'
        // cooked textures, compressed as BC7; compute shaders that pick storage images from the
        // bindless set by a pushed index; shadow maps, whose casters between the light and the
        // near plane still cast; indirect draws whose instances start where their commands say;
        // with mesh shaders, the fragment shaders behind them that read primitives' IDs; and,
        // with ray queries, shaders that take a TLAS by its 64-bit address.
        let features = vk::PhysicalDeviceFeatures::default()
            .sampler_anisotropy(true)
            .independent_blend(true)
            .texture_compression_bc(true)
            .shader_storage_image_array_dynamic_indexing(true)
            .depth_clamp(true)
            .draw_indirect_first_instance(true)
            .geometry_shader(mesh_shaders)
            .shader_int64(ray_query);
        let mut create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extensions)
            .enabled_features(&features)
            .push_next(&mut vulkan12)
            .push_next(&mut vulkan13);
        if uploads == UploadPath::HostImageCopy {
            create_info = create_info.push_next(&mut host_image_copy);
        }
        if mesh_shaders {
            create_info = create_info.push_next(&mut mesh_shader);
        }
        if ray_query {
            create_info = create_info
                .push_next(&mut acceleration_structure)
                .push_next(&mut ray_query_features);
        }
        // SAFETY: the physical device came from this instance. Every Vulkan 1.4 device has these
        // features but the bindless ones, BC texture compression, depth clamp, host image copy,
        // mesh and geometry shaders, ray queries, acceleration structures and 64-bit integers in
        // shaders, which `Candidate::new` checked (Vulkan 1.4 requires sampler anisotropy,
        // independent blending, dynamic indexing of storage images and indirect draws' first
        // instance, and the extensions their features, in vk.xml's feature requirements). The
        // create info's pointers outlive the call.
        let raw = unsafe { instance.raw().create_device(physical, &create_info, None) }.map_err(
            vk_failed(
                "vkCreateDevice",
                format_args!("device {:?}, uploading by {uploads}", chosen.name),
            ),
        )?;
        let debug_utils = instance
            .has_debug_utils()
            .then(|| ext::debug_utils::Device::new(instance.raw(), &raw));
        // SAFETY: the device was just made from this physical device, with buffer device
        // addresses and, if the GPU has it, the memory budget extension, and `Device` keeps the
        // instance alive and drops the allocator first; the debug utils are the device's.
        let allocator = unsafe {
            Allocator::new(
                instance.entry(),
                instance.raw(),
                physical,
                &raw,
                debug_utils.as_ref(),
                chosen.memory_budget,
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
        let bindless = match unsafe { Bindless::new(&raw, chosen.anisotropy) } {
            Ok(bindless) => bindless,
            Err(error) => {
                drop(allocator);
                // SAFETY: nothing else has been made from the device.
                unsafe { raw.destroy_device(None) };
                return Err(error);
            }
        };

        let renderdoc = renderdoc_on(instance, physical);
        let device_local_heaps = (chosen.memory.memory_heaps_as_slice().iter())
            .zip(0..)
            .filter(|(heap, _)| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .fold(0, |heaps, (_, i)| heaps | 1 << i);
        let copy_granularity = if uploads == UploadPath::TransferQueue {
            chosen.transfer_granularity
        } else {
            chosen.graphics_granularity
        };
        let queue = |family| Queue {
            family,
            // SAFETY: the device was created with one queue in each of its families.
            raw: Mutex::new(unsafe { raw.get_device_queue(family, 0) }),
        };
        let device = Self {
            graphics: queue(graphics),
            transfer: transfer.map(queue),
            compute: compute.map(queue),
            families,
            host_image_copy_fn: (uploads == UploadPath::HostImageCopy)
                .then(|| ext::host_image_copy::Device::new(instance.raw(), &raw)),
            uploads,
            copy_granularity,
            timestamp_bits: chosen.timestamp_bits,
            compute_timestamp_bits: chosen.compute_timestamp_bits,
            calibrated: calibrated
                .then(|| khr::calibrated_timestamps::Device::new(instance.raw(), &raw)),
            device_local_heaps,
            renderdoc,
            mesh_shader_fn: mesh_shaders
                .then(|| ext::mesh_shader::Device::new(instance.raw(), &raw)),
            acceleration_structure_fn: ray_query
                .then(|| khr::acceleration_structure::Device::new(instance.raw(), &raw)),
            scratch_alignment: if ray_query {
                scratch_alignment(instance, physical)
            } else {
                0
            },
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
            if let Some(compute) = &device.compute {
                device.set_name(*compute.lock(), "compute queue");
            }
            device.bindless.set_names(&device);
        }
        instance.name_through(&device);
        info!(
            name = chosen.name,
            kind = chosen.kind(),
            vulkan = %Version(chosen.api),
            driver = chosen.driver,
            vram_mib = chosen.vram_mib,
            uploads = %uploads,
            compute_queue = compute.is_some(),
            mesh_shaders,
            ray_query,
            memory_budget = chosen.memory_budget,
            "GPU selected"
        );
        debug!(graphics, transfer, compute, "Queue families");
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

    /// Whether the device draws through mesh shaders, which pipelines whose geometry is a mesh
    /// shader need.
    pub fn mesh_shaders(&self) -> bool {
        self.mesh_shader_fn.is_some()
    }

    /// The mesh shaders' functions, for a device that draws through them.
    pub(crate) fn mesh_shader_fn(&self) -> Option<&ext::mesh_shader::Device> {
        self.mesh_shader_fn.as_ref()
    }

    /// Whether compute shaders trace rays through acceleration structures (`VK_KHR_ray_query`),
    /// which [`Blas`](crate::Blas) and [`Tlas`](crate::Tlas) need.
    pub fn ray_query(&self) -> bool {
        self.acceleration_structure_fn.is_some()
    }

    /// The acceleration structures' functions.
    ///
    /// # Panics
    ///
    /// If the device has no ray queries.
    pub(crate) fn acceleration_structure_fn(&self) -> &khr::acceleration_structure::Device {
        (self.acceleration_structure_fn.as_ref())
            .expect("only a device with ray queries has acceleration structures")
    }

    /// What builds' scratch addresses are multiples of; 0 without ray queries.
    pub(crate) fn scratch_alignment(&self) -> u64 {
        self.scratch_alignment
    }

    /// How much of the GPU's own memory (its device-local heaps) is in use, by this process and
    /// others, and how much the driver gives this process before it starts paging. It's what the
    /// driver reports (`VK_EXT_memory_budget`) as of the frame being recorded, or, for a driver
    /// that reports none, VMA's estimate: 80% of the heaps and what it allocated itself. An
    /// allocation that takes usage past the budget may still succeed, slowly.
    pub fn memory_budget(&self) -> MemoryBudget {
        let heaps = self.allocator.heap_budgets();
        let mut total = MemoryBudget::default();
        for (i, (usage, budget)) in heaps.into_iter().enumerate() {
            if self.device_local_heaps & 1 << i != 0 {
                total.usage += usage;
                total.budget += budget;
            }
        }
        total
    }

    /// The least a copy to an image moves on the queue that copies textures, as
    /// `minImageTransferGranularity` has it: a copy's offset and size are multiples of it, in
    /// texels or compressed blocks, on each axis but where it reaches the image's edge. (1, 1, 1)
    /// on most queues and on the host, which the host image copy path's textures take; AMD's
    /// transfer queues have 16 by 16 by 8.
    pub(crate) fn copy_granularity(&self) -> vk::Extent3D {
        self.copy_granularity
    }

    /// Whether the device can capture its work through RenderDoc: its layer is active on the GPU,
    /// as in an app run through `renderdoccmd capture`.
    pub fn can_capture(&self) -> bool {
        self.renderdoc.is_some()
    }

    /// Starts capturing the device's work through RenderDoc, until the capture ends (it writes a
    /// file) or drops (it discards). Fails with `gpu.no_renderdoc` unless the device
    /// [can capture](Device::can_capture), and with `gpu.capture_busy` while another capture is
    /// under way, such as one F12 started.
    pub fn start_capture(&self) -> Result<Capture<'_>> {
        let Some(renderdoc) = self.renderdoc else {
            let message = format!(
                "device {:?} can't capture: RenderDoc's layer isn't active on it",
                self.name
            );
            return Err(Error::new("gpu.no_renderdoc", message));
        };
        renderdoc.start(self)
    }

    /// Gives the device a pipeline cache, which every pipeline made from then on goes through,
    /// and returns whether it holds `data`: what [`Device::pipeline_cache`] gave on a device of
    /// the same GPU and driver, whole. Other data starts it empty, logged at debug if it's another
    /// GPU's, driver's or engine's, and with a `gpu.pipeline_cache` warning if it's damaged (no
    /// driver is given damaged data, since drivers needn't survive it) or the driver fails to load
    /// it, as when out of memory.
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
                make(&[]).map_err(vk_failed(
                    "vkCreatePipelineCache",
                    format_args!("device {:?}'s pipeline cache", self.name),
                ))?
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
        let payload = unsafe { self.raw.get_pipeline_cache_data(cache) }.map_err(vk_failed(
            "vkGetPipelineCacheData",
            format_args!("device {:?}'s pipeline cache", self.name),
        ))?;
        Ok(Some(pipeline_cache::seal(&self.identity, &payload)))
    }

    /// The pipeline cache pipelines are made through: null until one is loaded. Making pipelines
    /// through it needs no lock: the device doesn't enable pipeline cache control, without which
    /// caches are synchronized internally.
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

    /// Checks that the object `what` names is this device's: `made_by` is the handle of the
    /// device that made it. Vulkan lets an object be used only with the device that made it, and
    /// drivers check nothing.
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
        let _compute = self.compute.as_ref().map(Queue::lock);
        // SAFETY: the device is live, and its queues' locks are held.
        unsafe { self.raw.device_wait_idle() }.map_err(vk_failed(
            "vkDeviceWaitIdle",
            format_args!("device {:?}", self.name),
        ))
    }

    pub(crate) fn physical(&self) -> vk::PhysicalDevice {
        self.physical
    }

    pub(crate) fn allocator(&self) -> &Allocator {
        &self.allocator
    }

    /// Destroys `resource` once the GPU has finished every frame and every submission of copies
    /// that may use it: at once if none is running.
    pub(crate) fn retire(&self, resource: Retired) {
        let mut retirement = self.lock_retirement();
        if let Some(resource) = retirement.retire(resource) {
            // SAFETY: every frame that began and every submission of copies has finished, and a
            // resource dropped now can't be used in those after it.
            unsafe { self.destroy(resource) };
        }
    }

    /// Notes that a [`Frames`](crate::Frames) numbers the device's frames, from 1.
    ///
    /// # Panics
    ///
    /// If one does, or one did that began a frame: frame numbers would repeat, and resources
    /// retired by them would be destroyed while the GPU may still use them.
    pub(crate) fn claim_frames(&self) {
        let mut retirement = self.lock_retirement();
        assert!(
            !retirement.claimed,
            "a device has one Frames for its whole life"
        );
        retirement.claimed = true;
    }

    /// Notes that the [`Frames`](crate::Frames) that claimed the device's frames is gone. One that
    /// began none (as one whose making failed) numbered none, so another may claim them.
    pub(crate) fn release_frames(&self) {
        let mut retirement = self.lock_retirement();
        retirement.claimed &= retirement.begun > 0;
    }

    /// Notes that `frame` began recording, so resources retired from now on may be used in it.
    /// Frames are numbered from 1, by the one [`Frames`](crate::Frames) that claimed them.
    pub(crate) fn frame_begun(&self, frame: u64) {
        self.lock_retirement().begun = frame;
        self.allocator.set_frame(frame as u32);
    }

    /// Notes that the GPU has finished `frame` and those before it, and destroys the resources
    /// they were the last to use, whose copies have finished too.
    pub(crate) fn frames_finished(&self, frame: u64) {
        let mut retirement = self.lock_retirement();
        retirement.finished = frame;
        self.sweep(&mut retirement);
    }

    /// Notes that submission `transfer` of copies on the transfer queue began recording, so
    /// resources retired from now on may be used in it. Submissions are numbered from 1, by the
    /// one [`Frames`](crate::Frames) that makes them.
    pub(crate) fn transfer_begun(&self, transfer: u64) {
        self.lock_retirement().transfer_recording = transfer;
    }

    /// Notes that submission `transfer` of copies was made, so resources dropped from now on
    /// can't be used in it, unless another begins.
    pub(crate) fn transfer_submitted(&self, transfer: u64) {
        let mut retirement = self.lock_retirement();
        retirement.transfers_submitted = transfer;
        retirement.transfer_recording = 0;
    }

    /// Notes that the GPU has finished submission `transfer` of copies and those before it, and
    /// destroys the resources they were the last to use, whose frames have finished too.
    pub(crate) fn transfers_finished(&self, transfer: u64) {
        let mut retirement = self.lock_retirement();
        retirement.transfers_finished = transfer;
        self.sweep(&mut retirement);
    }

    /// Destroys the retired resources whose frames and copies have all finished.
    fn sweep(&self, retirement: &mut Retirement) {
        for resource in retirement.sweep() {
            // SAFETY: the frames and copies that may use it have finished.
            unsafe { self.destroy(resource) };
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
                Retired::Alias(image, view, slot) => {
                    self.raw.destroy_image_view(view, None);
                    self.raw.destroy_image(image, None);
                    if let Some(slot) = slot {
                        self.bindless.remove(slot);
                    }
                }
                Retired::StorageView(view, slot) => {
                    self.raw.destroy_image_view(view, None);
                    self.bindless.remove_storage(slot);
                }
                Retired::Memory(allocation) => self.allocator.free_memory(allocation),
                Retired::Slot(slot) => self.bindless.remove(slot),
                Retired::Pipeline(pipeline) => self.raw.destroy_pipeline(pipeline, None),
                Retired::AccelerationStructure(structure) => self
                    .acceleration_structure_fn()
                    .destroy_acceleration_structure(structure, None),
            }
        }
    }

    pub(crate) fn bindless(&self) -> &Bindless {
        &self.bindless
    }

    pub(crate) fn graphics_family(&self) -> u32 {
        self.graphics.family
    }

    /// How many bits of the graphics queue's timestamps count; 0 for a queue that writes none.
    pub(crate) fn timestamp_bits(&self) -> u32 {
        self.timestamp_bits
    }

    /// How many bits of the compute queue's timestamps count; 0 for a queue that writes none, and
    /// without a compute queue.
    pub(crate) fn compute_timestamp_bits(&self) -> u32 {
        self.compute_timestamp_bits
    }

    /// The transfer queue's family, for a device that uploads through one.
    pub(crate) fn transfer_family(&self) -> Option<u32> {
        self.transfer.as_ref().map(|queue| queue.family)
    }

    /// The compute queue's family, for a device with a compute queue apart from graphics.
    pub(crate) fn compute_family(&self) -> Option<u32> {
        self.compute.as_ref().map(|queue| queue.family)
    }

    /// The families of the device's queues, each once, which every buffer is shared by.
    pub(crate) fn families(&self) -> &[u32] {
        &self.families
    }

    /// What the GPU's clock reads now, in timestamp ticks, read at once through
    /// `VK_KHR_calibrated_timestamps`. `None` for a device whose driver can't, or without gg-core's
    /// `tracy` feature, which alone reads it so. Allocates nothing.
    pub(crate) fn read_clock(&self) -> Option<u64> {
        let calibrated = self.calibrated.as_ref()?;
        let info = vk::CalibratedTimestampInfoKHR::default().time_domain(vk::TimeDomainKHR::DEVICE);
        let (mut timestamp, mut deviation) = (0, 0);
        // SAFETY: the device is live with the extension enabled, and the device's own time domain
        // is one its driver calibrates, as choosing it checked. The pointers are to one info and
        // one timestamp, as the count says. ash's wrapper would allocate.
        let read = unsafe {
            (calibrated.fp().get_calibrated_timestamps_khr)(
                self.raw.handle(),
                1,
                &info,
                &mut timestamp,
                &mut deviation,
            )
        };
        (read == vk::Result::SUCCESS).then_some(timestamp)
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
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }.map_err(vk_failed(
            "vkQueueSubmit2",
            format_args!("{work} on device {:?}'s graphics queue", self.name),
        ))
    }

    /// Submits `work`, as its errors call it, to the compute queue.
    ///
    /// # Safety
    ///
    /// As for [`Device::submit`].
    ///
    /// # Panics
    ///
    /// If the device has no compute queue.
    pub(crate) unsafe fn submit_compute(
        &self,
        submits: &[vk::SubmitInfo2<'_>],
        work: impl fmt::Display,
    ) -> Result<()> {
        let compute = self.compute.as_ref();
        let queue = compute
            .expect("only a device with a compute queue submits to one")
            .lock();
        // SAFETY: the caller guarantees the submissions; the lock is held.
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }.map_err(vk_failed(
            "vkQueueSubmit2",
            format_args!("{work} on device {:?}'s compute queue", self.name),
        ))
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
        unsafe { self.raw.queue_submit2(*queue, submits, vk::Fence::null()) }.map_err(vk_failed(
            "vkQueueSubmit2",
            format_args!("{work} on device {:?}'s transfer queue", self.name),
        ))
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
    /// finish, for rare work outside frames, such as reading an image back. The commands and their
    /// errors are named `name`.
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
        let pool = unsafe { self.raw.create_command_pool(&pool_info, None) }.map_err(vk_failed(
            "vkCreateCommandPool",
            format_args!("command pool {name:?}"),
        ))?;
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
                    .map_err(vk_failed("vkAllocateCommandBuffers", commands_named))?[0];
                let begin_info = vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
                self.raw
                    .begin_command_buffer(commands, &begin_info)
                    .map_err(vk_failed("vkBeginCommandBuffer", commands_named))?;
                // Once begun, as RenderDoc forgets a command buffer's name as it's begun.
                self.set_name(commands, name);
                record(commands);
                self.raw
                    .end_command_buffer(commands)
                    .map_err(vk_failed("vkEndCommandBuffer", commands_named))?;
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
                    .map_err(vk_failed("vkQueueSubmit2", submitted))?;
                self.raw
                    .queue_wait_idle(*queue)
                    .map_err(vk_failed("vkQueueWaitIdle", submitted))
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
        if self.debug_utils.is_some()
            && let Ok(name) = CString::new(name)
        {
            // SAFETY: the caller guarantees the handle.
            unsafe { self.set_c_name(handle, &name) };
        }
    }

    /// [`Device::set_name`] with a name that's a C string already, which allocates nothing.
    ///
    /// # Safety
    ///
    /// As for [`Device::set_name`].
    pub(crate) unsafe fn set_c_name(&self, handle: impl vk::Handle, name: &CStr) {
        let Some(debug_utils) = &self.debug_utils else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(handle)
            .object_name(name);
        // SAFETY: the caller guarantees the handle. Names are a debugging aid, so a failure to
        // set one is ignored.
        let _ = unsafe { debug_utils.set_debug_utils_object_name(&info) };
    }

    /// Names `commands`, just begun, again while the device can capture, as RenderDoc forgets a
    /// command buffer's name as it's begun.
    ///
    /// # Safety
    ///
    /// `commands` must be a live command buffer of this device.
    pub(crate) unsafe fn name_begun(&self, commands: vk::CommandBuffer, name: &CStr) {
        if self.can_capture() {
            // SAFETY: the caller guarantees it.
            unsafe { self.set_c_name(commands, name) };
        }
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
        // allocator, its pipeline cache, its bindless set and retired resources are left; waiting
        // first lets the GPU finish, and the set and the allocator go before the device they were
        // made for. Pipelines don't need the cache they were made through.
        unsafe {
            let _ = self.raw.device_wait_idle();
            let retired = std::mem::take(&mut self.lock_retirement().resources);
            for (_, _, resource) in retired {
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

/// The SPIR-V capabilities a shader may declare on the devices the engine makes.
///
/// They are those Vulkan 1.4 gives without a feature, and those of the features [`Device::new`]
/// enables on every device, as the Vulkan SDK 1.4.335's vk.xml lists them under
/// `spirvcapabilities`. They change with those features. A shader that declares another needs a
/// feature some device hasn't enabled, which validation reports in debug builds and a driver may
/// crash on in others, as with the engine's own mesh shaders and the fragment shaders behind them.
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
/// least 500,000 update-after-bind sampled images, and as many storage images.
fn bindless_features() -> vk::PhysicalDeviceVulkan12Features<'static> {
    vk::PhysicalDeviceVulkan12Features::default()
        .runtime_descriptor_array(true)
        .descriptor_binding_partially_bound(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_storage_image_update_after_bind(true)
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
        features.descriptor_binding_storage_image_update_after_bind,
        features.descriptor_binding_update_unused_while_pending,
        features.shader_sampled_image_array_non_uniform_indexing,
    ]
    .iter()
    .all(|&feature| feature == vk::TRUE)
}

/// What a GPU with acceleration structures aligns builds' scratch to.
fn scratch_alignment(instance: &Instance, physical: vk::PhysicalDevice) -> u64 {
    let mut structures = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut structures);
    // SAFETY: the physical device came from this instance and has the extension whose properties
    // these are; they outlive the call.
    unsafe {
        (instance.raw()).get_physical_device_properties2(physical, &mut properties);
    }
    u64::from(structures.min_acceleration_structure_scratch_offset_alignment)
}

/// RenderDoc, if its layer is active on the GPU, as the tools active on it report (logged at
/// debug): the loader lists RenderDoc's layer whether or not it's active, and its library can be
/// in the process with the layer off.
fn renderdoc_on(instance: &Instance, physical: vk::PhysicalDevice) -> Option<&'static RenderDoc> {
    let raw = instance.raw();
    // SAFETY: the physical device came from this instance and supports Vulkan 1.4, which has the
    // query; the list has room for as many tools as it counted.
    let tools = unsafe {
        raw.get_physical_device_tool_properties_len(physical)
            .and_then(|len| {
                let mut tools = vec![vk::PhysicalDeviceToolProperties::default(); len];
                raw.get_physical_device_tool_properties(physical, &mut tools)
                    .map(|()| tools)
            })
    }
    .unwrap_or_default();
    let mut active = false;
    for tool in &tools {
        let [name, version, layer] = [
            tool.name_as_c_str(),
            tool.version_as_c_str(),
            tool.layer_as_c_str(),
        ]
        .map(|text| text.map(CStr::to_string_lossy).unwrap_or_default());
        debug!(
            name = %name,
            version = %version,
            layer = %layer,
            purposes = ?tool.purposes,
            "Tool active"
        );
        // RenderDoc 1.45 names itself, but not its layer.
        active |= tool.name_as_c_str() == Ok(c"RenderDoc");
    }
    active.then(capture::renderdoc).flatten()
}

#[cfg(test)]
mod tests {
    use ash::vk::Handle;

    use super::*;

    /// The numbers of `resources`, pipelines made of them.
    fn numbers(resources: impl Iterator<Item = Retired>) -> Vec<u64> {
        let mut numbers: Vec<u64> = resources
            .map(|resource| match resource {
                Retired::Pipeline(pipeline) => pipeline.as_raw(),
                _ => unreachable!("pipelines alone"),
            })
            .collect();
        numbers.sort_unstable();
        numbers
    }

    /// A resource dropped while nothing runs goes at once. One dropped while a frame records and
    /// copies are recording, or were made since the GPU last finished some, waits for both the
    /// frame and the copies. One dropped with no copies made since the last finished waits for
    /// the frame alone.
    #[test]
    fn a_resource_retires_until_both_its_frames_and_its_copies_have_finished() {
        let pipeline = |n: u64| Retired::Pipeline(vk::Pipeline::from_raw(n));
        let mut retirement = Retirement::default();
        assert!(retirement.retire(pipeline(1)).is_some(), "nothing runs");
        // Frame 5 is recording and frame 4 finished; copies submission 3 is recording, 2 was made
        // and 1 finished.
        retirement.begun = 5;
        retirement.finished = 4;
        retirement.transfer_recording = 3;
        retirement.transfers_submitted = 2;
        retirement.transfers_finished = 1;
        assert!(retirement.retire(pipeline(2)).is_none());
        // Submission 3 made: a resource dropped now may still be used in it.
        retirement.transfers_submitted = 3;
        retirement.transfer_recording = 0;
        assert!(retirement.retire(pipeline(3)).is_none());
        // The frame finished, but not the copies: both stay.
        retirement.finished = 5;
        assert!(numbers(retirement.sweep()).is_empty(), "the copies run");
        retirement.transfers_finished = 2;
        assert!(numbers(retirement.sweep()).is_empty(), "submission 3 runs");
        retirement.transfers_finished = 3;
        assert_eq!(numbers(retirement.sweep()), [2, 3]);
        // With submission 3 finished and none made since, a resource waits for its frame alone;
        // one dropped while copies are recording again waits for those too, though its frame has
        // finished.
        retirement.begun = 6;
        assert!(retirement.retire(pipeline(4)).is_none());
        retirement.transfer_recording = 4;
        assert!(retirement.retire(pipeline(5)).is_none());
        retirement.finished = 6;
        assert_eq!(numbers(retirement.sweep()), [4]);
        retirement.transfers_submitted = 4;
        retirement.transfer_recording = 0;
        retirement.transfers_finished = 4;
        assert_eq!(numbers(retirement.sweep()), [5]);
        assert!(retirement.resources.is_empty());
    }
}
