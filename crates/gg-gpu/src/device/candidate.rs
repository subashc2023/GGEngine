//! Choosing a GPU.

use std::ffi::CStr;

use ash::vk;
use gg_core::{Error, Result};

use super::{UploadPath, has_bindless};
use crate::capture;
use crate::frame::graph::ImageKind;
use crate::pipeline_cache::Identity;
use crate::{
    Instance, MAX_TEXTURE_SIZE, REQUIRED_VERSION, Surface, Texture, TextureFormat, Version,
};

/// A GPU the loader found, and what keeps the engine from using it, if anything.
pub(super) struct Candidate {
    pub(super) index: usize,
    pub(super) physical: vk::PhysicalDevice,
    pub(super) name: String,
    kind: vk::PhysicalDeviceType,
    pub(super) api: u32,
    pub(super) driver: String,
    pub(super) vram_mib: u64,
    pub(super) limits: vk::PhysicalDeviceLimits,
    pub(super) identity: Identity,
    /// A queue family with graphics that can also present to the surface.
    pub(super) graphics: Option<u32>,
    /// A queue family for uploads apart from graphics.
    pub(super) transfer: Option<u32>,
    /// A queue family for compute apart from graphics and `transfer`.
    pub(super) compute: Option<u32>,
    /// The least a copy to or from an image moves on the graphics and transfer families'
    /// queues (`minImageTransferGranularity`), in texels, or blocks of a compressed image.
    pub(super) graphics_granularity: vk::Extent3D,
    pub(super) transfer_granularity: vk::Extent3D,
    /// How many bits of the graphics and compute families' timestamps count
    /// (`timestampValidBits`); 0 for none.
    pub(super) timestamp_bits: u32,
    pub(super) compute_timestamp_bits: u32,
    /// Whether the driver reads the GPU's clock without a submission
    /// (`VK_KHR_calibrated_timestamps`, in the device's own time domain).
    pub(super) calibrated: bool,
    pub(super) host_copy: HostCopy,
    pub(super) memory: vk::PhysicalDeviceMemoryProperties,
    /// Whether the driver reports each heap's budget (`VK_EXT_memory_budget`).
    pub(super) memory_budget: bool,
    /// Whether it draws through mesh shaders (`VK_EXT_mesh_shader`), with `geometryShader`.
    pub(super) mesh_shaders: bool,
    /// Whether compute shaders trace rays: `VK_KHR_ray_query` and `VK_KHR_acceleration_structure`,
    /// with `shaderInt64` and packed vertices as builds' input.
    pub(super) ray_query: bool,
    /// Whether its samplers filter anisotropically: every GPU but lavapipe (llvmpipe), Mesa's
    /// CPU driver. Its anisotropic filter returns NaN where a texture coordinate's derivatives
    /// are 0, as across triangles whose corners share their coordinates (flat-colored and
    /// palette-textured models). Mesa 25.0's does, Debian 13's, which CI's GPU tests run on:
    /// the skinned-mesh test's strips, whose coordinates are all 0, drew black. Remove once a
    /// Mesa release filters them, Debian's among them.
    pub(super) anisotropy: bool,
    pub(super) problem: Option<String>,
}

/// Whether a GPU can write the engine's textures from the CPU (host image copy), and whether its
/// driver says that slows the GPU's reads, as by turning compression off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostCopy {
    Unsupported,
    Costly,
    Free,
}

impl HostCopy {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Costly => "costly",
            Self::Free => "free",
        }
    }
}

impl Candidate {
    pub(super) fn new(
        instance: &Instance,
        index: usize,
        physical: vk::PhysicalDevice,
        surface: Option<&Surface>,
    ) -> Self {
        let raw = instance.raw();
        // SAFETY: the physical device came from this instance.
        let (properties, memory, families, core) = unsafe {
            (
                raw.get_physical_device_properties(physical),
                raw.get_physical_device_memory_properties(physical),
                raw.get_physical_device_queue_family_properties(physical),
                raw.get_physical_device_features(physical),
            )
        };
        let vulkan12 = (properties.api_version >= vk::API_VERSION_1_2).then(|| {
            let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut vulkan12);
            // SAFETY: as above; the 1.2 features are core in Vulkan 1.2.
            unsafe { raw.get_physical_device_features2(physical, &mut features) };
            vulkan12
        });
        let bindless = vulkan12.as_ref().is_some_and(has_bindless);
        let name = properties.device_name_as_c_str().map_or_else(
            |_| format!("GPU {index}"),
            |name| name.to_string_lossy().into_owned(),
        );
        let (driver, driver_id) = if properties.api_version >= vk::API_VERSION_1_2 {
            let mut driver = vk::PhysicalDeviceDriverProperties::default();
            let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut driver);
            // SAFETY: as above; the driver properties are core in Vulkan 1.2.
            unsafe { raw.get_physical_device_properties2(physical, &mut properties2) };
            let name = driver.driver_name_as_c_str().map(CStr::to_string_lossy);
            let info = driver.driver_info_as_c_str().map(CStr::to_string_lossy);
            let text = format!("{} {}", name.unwrap_or_default(), info.unwrap_or_default());
            (text, driver.driver_id)
        } else {
            let text = format!("{:#x}", properties.driver_version);
            (text, vk::DriverId::default())
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
        let compute = compute_family(&families, transfer);
        let bits = |family: Option<u32>| {
            (family.and_then(|family| families.get(family as usize)))
                .map_or(0, |family| family.timestamp_valid_bits)
        };
        let granularity = |family: Option<u32>| {
            family
                .and_then(|family| families.get(family as usize))
                .map_or(vk::Extent3D::default(), |family| {
                    family.min_image_transfer_granularity
                })
        };
        let extensions = device_extensions(instance, physical);
        // Host image copy's queries use Vulkan 1.3's, and a device without 1.4 is refused anyway.
        let host_copy = if properties.api_version >= REQUIRED_VERSION {
            host_copy_support(instance, physical, &extensions)
        } else {
            HostCopy::Unsupported
        };
        let problem = if properties.api_version < REQUIRED_VERSION {
            let version = Version(properties.api_version);
            // RenderDoc reports Vulkan 1.2 for a GPU whose buffer addresses it can't capture,
            // which 1.3 requires.
            let hidden = vulkan12
                .is_some_and(|features| features.buffer_device_address_capture_replay != vk::TRUE)
                && capture::renderdoc().is_some();
            Some(if hidden {
                format!(
                    "reports Vulkan {version} under RenderDoc, which needs \
                     bufferDeviceAddressCaptureReplay to capture buffer addresses, which this GPU \
                     lacks"
                )
            } else {
                format!("supports Vulkan {version}, not 1.4; a newer driver may add it")
            })
        } else if graphics.is_none() && surface.is_some() {
            Some("has no graphics queue that can present to the window".to_owned())
        } else if graphics.is_none() {
            Some("has no graphics queue".to_owned())
        } else if surface.is_some() && !has_extension(&extensions, vk::KHR_SWAPCHAIN_NAME) {
            Some("doesn't support VK_KHR_swapchain".to_owned())
        } else if !bindless {
            Some("doesn't support the descriptor indexing that bindless textures need".to_owned())
        } else if !has_depth(instance, physical) {
            Some(
                "can't render depth into D32_SFLOAT images and sample them by comparison, as \
                 shadow maps are"
                    .to_owned(),
            )
        } else if core.depth_clamp != vk::TRUE {
            Some("doesn't support depth clamp (depthClamp), which shadow maps need".to_owned())
        } else if core.texture_compression_bc != vk::TRUE {
            // Cooked textures are BC7 and BC5. Unlike depth, no format query is needed: the feature
            // requires every BC format's images to support sampling, linear filtering and copies
            // into them (Features chapter's textureCompressionBC; Formats chapter's required
            // format support).
            Some(
                "doesn't support BC texture compression (textureCompressionBC), which games' \
                 cooked textures need"
                    .to_owned(),
            )
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
            compute,
            graphics_granularity: granularity(graphics),
            transfer_granularity: granularity(transfer),
            timestamp_bits: bits(graphics),
            compute_timestamp_bits: bits(compute),
            calibrated: has_extension(&extensions, vk::KHR_CALIBRATED_TIMESTAMPS_NAME)
                && reads_clock(instance, physical),
            host_copy,
            memory,
            memory_budget: has_extension(&extensions, vk::EXT_MEMORY_BUDGET_NAME),
            // The extension requires its `meshShader` feature (vk.xml's `<feature>` among its
            // requirements), so it isn't queried: under Vulkan SDK 1.4.335's validation layer the
            // query crashed test runs that open dozens of devices, well after it.
            // A fragment shader behind a mesh shader reads the primitive's ID through SPIR-V's
            // Geometry capability, which `geometryShader` gives.
            mesh_shaders: properties.api_version >= REQUIRED_VERSION
                && has_extension(&extensions, vk::EXT_MESH_SHADER_NAME)
                && core.geometry_shader == vk::TRUE,
            // As with mesh shaders, the extensions require their features (`accelerationStructure`
            // and `rayQuery`), so those aren't queried. Shaders that take a TLAS by address declare
            // SPIR-V's Int64, which `shaderInt64` gives.
            ray_query: properties.api_version >= REQUIRED_VERSION
                && [
                    vk::KHR_ACCELERATION_STRUCTURE_NAME,
                    vk::KHR_RAY_QUERY_NAME,
                    vk::KHR_DEFERRED_HOST_OPERATIONS_NAME,
                ]
                .iter()
                .all(|name| has_extension(&extensions, name))
                && core.shader_int64 == vk::TRUE
                && builds_from_packed_vertices(instance, physical),
            anisotropy: driver_id != vk::DriverId::MESA_LLVMPIPE,
            problem,
        }
    }

    pub(super) fn kind(&self) -> &'static str {
        match self.kind {
            vk::PhysicalDeviceType::DISCRETE_GPU => "discrete",
            vk::PhysicalDeviceType::INTEGRATED_GPU => "integrated",
            vk::PhysicalDeviceType::VIRTUAL_GPU => "virtual",
            vk::PhysicalDeviceType::CPU => "cpu",
            _ => "other",
        }
    }

    /// Whether the GPU draws through mesh shaders: as `wanted` says, else if it can.
    ///
    /// Fails with `gpu.no_mesh_shaders` if they're wanted and the GPU has none.
    pub(super) fn mesh_shaders(&self, wanted: Option<bool>) -> Result<bool> {
        match wanted {
            Some(true) if !self.mesh_shaders => {
                let message = format!(
                    "mesh shaders were asked for, but {} has none (VK_EXT_mesh_shader), or no \
                     geometryShader, by which their fragments read their primitives' IDs",
                    self.name
                );
                Err(Error::new("gpu.no_mesh_shaders", message))
            }
            Some(wanted) => Ok(wanted),
            None => Ok(self.mesh_shaders),
        }
    }

    /// Whether compute shaders trace rays: as `wanted` says, else if the GPU can.
    ///
    /// Fails with `gpu.no_ray_query` if they're wanted and the GPU can't.
    pub(super) fn ray_query(&self, wanted: Option<bool>) -> Result<bool> {
        match wanted {
            Some(true) if !self.ray_query => {
                let message = format!(
                    "ray queries were asked for, but {} hasn't them (VK_KHR_ray_query and \
                     VK_KHR_acceleration_structure), shaderInt64, or R16G16B16A16_UNORM as \
                     acceleration-structure vertices",
                    self.name
                );
                Err(Error::new("gpu.no_ray_query", message))
            }
            Some(wanted) => Ok(wanted),
            None => Ok(self.ray_query),
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

    /// How the GPU uploads: by `wanted`; else through a transfer queue if it has one, else by host
    /// image copy if that's free, else through the graphics queue.
    pub(super) fn upload_path(&self, wanted: Option<UploadPath>) -> Result<UploadPath> {
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
                "can't write every texture format from the CPU"
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

/// Whether a device renders depth into D32_SFLOAT, the depth images' format, and samples it by
/// comparison, as shadow maps do. Vulkan requires the first of D32_SFLOAT or
/// X8_D24_UNORM_PACK32, and the second of no format, but nearly every device has both: AMD's
/// have no 24-bit depth, and Apple's GPUs have 32-bit float depth everywhere. Called for Vulkan
/// 1.3 or later, whose format properties tell the comparison.
fn has_depth(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    let mut features3 = vk::FormatProperties3::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut features3);
    // SAFETY: the physical device came from this instance, whose Vulkan 1.3 has format properties
    // 3, and the properties outlive the call.
    unsafe {
        instance.raw().get_physical_device_format_properties2(
            physical,
            ImageKind::Depth.format(),
            &mut properties,
        );
    }
    let needed = vk::FormatFeatureFlags2::DEPTH_STENCIL_ATTACHMENT
        | vk::FormatFeatureFlags2::SAMPLED_IMAGE
        | vk::FormatFeatureFlags2::SAMPLED_IMAGE_DEPTH_COMPARISON;
    features3.optimal_tiling_features.contains(needed)
}

/// Whether acceleration structures build from R16G16B16A16_UNORM vertices, as models pack their
/// positions, which BLASes read in place. Called for Vulkan 1.3 or later, whose format properties
/// tell it.
fn builds_from_packed_vertices(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    let mut features3 = vk::FormatProperties3::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut features3);
    // SAFETY: the physical device came from this instance, whose Vulkan 1.3 has format properties
    // 3, and the properties outlive the call.
    unsafe {
        instance.raw().get_physical_device_format_properties2(
            physical,
            vk::Format::R16G16B16A16_UNORM,
            &mut properties,
        );
    }
    (features3.buffer_features)
        .contains(vk::FormatFeatureFlags2::ACCELERATION_STRUCTURE_VERTEX_BUFFER_KHR)
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

/// The queue family uploads go through apart from graphics: of those without graphics that can
/// copy (compute families can without saying so), the one doing the fewest other kinds of work,
/// then the first. A transfer-only family is the GPU's copy engine. Sparse binding and protected
/// memory are features, not work, and don't count. A family whose least image transfer
/// granularity is (0, 0, 0) copies images only whole, a level at a time, so it takes no uploads:
/// textures stream by rows within a frame's budget (VUID-vkCmdCopyBufferToImage-imageOffset-07738).
fn transfer_family(families: &[vk::QueueFamilyProperties]) -> Option<u32> {
    let copies = vk::QueueFlags::TRANSFER | vk::QueueFlags::COMPUTE;
    let features = vk::QueueFlags::SPARSE_BINDING | vk::QueueFlags::PROTECTED;
    families
        .iter()
        .zip(0u32..)
        .filter(|(family, _)| {
            let vk::Extent3D {
                width,
                height,
                depth,
            } = family.min_image_transfer_granularity;
            family.queue_count > 0
                && family.queue_flags.intersects(copies)
                && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                && width > 0
                && height > 0
                && depth > 0
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

/// The queue family compute passes run on beside the graphics queue: of those with compute and
/// without graphics, other than `transfer`'s, the one doing the fewest other kinds of work, then
/// the first.
fn compute_family(families: &[vk::QueueFamilyProperties], transfer: Option<u32>) -> Option<u32> {
    let features = vk::QueueFlags::SPARSE_BINDING | vk::QueueFlags::PROTECTED;
    families
        .iter()
        .zip(0u32..)
        .filter(|&(family, index)| {
            family.queue_count > 0
                && family.queue_flags.contains(vk::QueueFlags::COMPUTE)
                && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                && Some(index) != transfer
        })
        .min_by_key(|&(family, index)| {
            let work = family.queue_flags & !features;
            (work.as_raw().count_ones(), index)
        })
        .map(|(_, index)| index)
}

/// Whether the driver reads its clock in the device's own time domain through
/// `VK_KHR_calibrated_timestamps`, which the device lists.
fn reads_clock(instance: &Instance, physical: vk::PhysicalDevice) -> bool {
    let functions =
        ash::khr::calibrated_timestamps::Instance::new(instance.entry(), instance.raw());
    // SAFETY: the physical device came from this instance, and lists the extension, whose
    // physical-device functions it may then be asked for.
    let domains = unsafe { functions.get_physical_device_calibrateable_time_domains(physical) };
    domains.is_ok_and(|domains| domains.contains(&vk::TimeDomainKHR::DEVICE))
}

/// Whether the device can write the engine's textures from the CPU, and at what cost: host image
/// copy for every texture format (BC7 and BC5 among them) at every texture size, into the
/// shader-read-only layout the bindless set declares textures in. Nothing is queried past the
/// extension's name unless the device lists it.
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
    for format in TextureFormat::ALL.map(TextureFormat::raw) {
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

/// Picks the GPU: of those without a problem whose names contain `gpu`, the best-ranked kind,
/// then the most memory, then the first the loader listed.
pub(super) fn pick<'a>(candidates: &'a [Candidate], gpu: Option<&str>) -> Result<&'a Candidate> {
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
            compute: None,
            graphics_granularity: vk::Extent3D::default(),
            transfer_granularity: vk::Extent3D::default(),
            timestamp_bits: 64,
            compute_timestamp_bits: 0,
            calibrated: false,
            host_copy: HostCopy::Unsupported,
            memory: vk::PhysicalDeviceMemoryProperties::default(),
            memory_budget: false,
            mesh_shaders: false,
            ray_query: false,
            anisotropy: true,
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
    fn transfers_and_compute_take_the_families_apart_from_graphics_with_the_fewest_flags() {
        use vk::QueueFlags as Q;
        let families = |flags: &[Q]| {
            flags
                .iter()
                .map(|&queue_flags| vk::QueueFamilyProperties {
                    queue_flags,
                    queue_count: 1,
                    min_image_transfer_granularity: vk::Extent3D {
                        width: 1,
                        height: 1,
                        depth: 1,
                    },
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
        assert_eq!(compute_family(&nvidia, Some(1)), Some(2));
        // An AMD iGPU's: compute before transfers alone, and video that can't copy.
        let amd = families(&[
            all,
            Q::COMPUTE | Q::TRANSFER | Q::SPARSE_BINDING,
            Q::TRANSFER | Q::SPARSE_BINDING,
            Q::VIDEO_DECODE_KHR,
            Q::VIDEO_ENCODE_KHR,
        ]);
        assert_eq!(transfer_family(&amd), Some(2));
        assert_eq!(compute_family(&amd, Some(2)), Some(1));
        // lavapipe's one family has graphics, so compute passes run on the graphics queue there.
        assert_eq!(transfer_family(&families(&[all])), None);
        assert_eq!(compute_family(&families(&[all]), None), None);
        // A compute family without the transfer bit it implies, as RADV may: it copies, but a
        // transfer-only family after it does less else.
        let compute = families(&[Q::GRAPHICS | Q::COMPUTE | Q::TRANSFER, Q::COMPUTE]);
        assert_eq!(transfer_family(&compute), Some(1));
        // The one family apart from graphics takes the uploads, not compute passes too.
        assert_eq!(compute_family(&compute, Some(1)), None);
        let both = families(&[all, Q::COMPUTE, Q::TRANSFER]);
        assert_eq!(transfer_family(&both), Some(2));
        assert_eq!(compute_family(&both, Some(2)), Some(1));
        // Video before transfers alone.
        let video = families(&[all, Q::TRANSFER | Q::VIDEO_DECODE_KHR, Q::TRANSFER]);
        assert_eq!(transfer_family(&video), Some(2));
        // A whole-image-only family takes no uploads, which stream by rows: compute does, and no
        // compute passes run beside the graphics queue.
        let mut whole = families(&[all, Q::TRANSFER, Q::COMPUTE]);
        whole[1].min_image_transfer_granularity = vk::Extent3D::default();
        assert_eq!(transfer_family(&whole), Some(2));
        assert_eq!(compute_family(&whole, Some(2)), None);
        whole[2].min_image_transfer_granularity = vk::Extent3D::default();
        assert_eq!(transfer_family(&whole), None);
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

    #[test]
    fn mesh_shaders_are_used_where_the_gpu_has_them_unless_asked_otherwise() {
        let gpu = |mesh_shaders| Candidate {
            mesh_shaders,
            ..candidate(0, "Some GPU", vk::PhysicalDeviceType::DISCRETE_GPU, 8192)
        };
        let (with, without) = (gpu(true), gpu(false));
        let used = |gpu: &Candidate, wanted| gpu.mesh_shaders(wanted).expect("a choice");
        assert!(used(&with, None) && used(&with, Some(true)));
        assert!(!used(&with, Some(false)));
        assert!(!used(&without, None) && !used(&without, Some(false)));
        let error = without.mesh_shaders(Some(true)).expect_err("none");
        assert_eq!(error.code(), "gpu.no_mesh_shaders");
        assert!(error.to_string().contains("Some GPU has none"), "{error}");
    }

    #[test]
    fn ray_queries_are_used_where_the_gpu_has_them_unless_asked_otherwise() {
        let gpu = |ray_query| Candidate {
            ray_query,
            ..candidate(0, "Some GPU", vk::PhysicalDeviceType::DISCRETE_GPU, 8192)
        };
        let (with, without) = (gpu(true), gpu(false));
        let used = |gpu: &Candidate, wanted| gpu.ray_query(wanted).expect("a choice");
        assert!(used(&with, None) && used(&with, Some(true)));
        assert!(!used(&with, Some(false)));
        assert!(!used(&without, None) && !used(&without, Some(false)));
        let error = without.ray_query(Some(true)).expect_err("none");
        assert_eq!(error.code(), "gpu.no_ray_query");
        assert!(
            error.to_string().contains("Some GPU hasn't them"),
            "{error}"
        );
    }
}
