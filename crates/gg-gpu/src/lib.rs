//! The Vulkan layer, built on `ash`: instance and device setup, memory, buffers and images,
//! bindless descriptors, pipelines, swapchain, synchronization and queue submission.
//!
//! `gg-renderer` is its only client; the engine reaches the GPU through the renderer.

// Vulkan through ash is an unsafe API; wrapping it in safe types is this crate's job.
#![allow(unsafe_code)]

mod bindless;
mod buffer;
mod device;
mod frame;
mod image;
mod instance;
mod pipeline;
mod surface;
mod swapchain;
mod texture;
mod vma;

use std::fmt;

use ash::vk;
use gg_core::Error;

pub use bindless::Sampler;
pub use buffer::GpuBuffer;
pub use device::Device;
pub use frame::{Frame, Frames, Indices, Pass, Target, Upload};
pub use image::ColorImage;
pub use instance::{Instance, InstanceDesc, Validation};
pub use pipeline::{Blend, Format, GraphicsDesc, Pipeline};
pub use surface::{Surface, Window};
pub use swapchain::Swapchain;
pub use texture::{Texture, TextureFormat};

/// The Vulkan version a device must support.
const REQUIRED_VERSION: u32 = vk::make_api_version(0, 1, 4, 0);

/// The whole of a color image with one mip level and one layer.
const COLOR_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

/// The engine error for a failed Vulkan call, with codes of their own for the failures callers
/// react to.
fn vk_error(call: &str, result: vk::Result) -> Error {
    let code = match result {
        vk::Result::ERROR_DEVICE_LOST => "gpu.device_lost",
        vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
            "gpu.out_of_memory"
        }
        vk::Result::ERROR_SURFACE_LOST_KHR => "gpu.surface_lost",
        _ => "gpu.vulkan",
    };
    Error::new(code, format!("{call} failed with {result:?}")).with_source(result)
}

/// Shows a packed Vulkan version as `1.4.341`.
struct Version(u32);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = self.0;
        write!(
            f,
            "{}.{}.{}",
            vk::api_version_major(version),
            vk::api_version_minor(version),
            vk::api_version_patch(version)
        )
    }
}
