//! The Vulkan layer, built on `ash`: instance and device setup, memory, buffers and images,
//! bindless descriptors, pipelines, swapchain, synchronization and queue submission, the render
//! graph, which places a frame's barriers and makes its own images, and frame captures through
//! RenderDoc.
//!
//! `gg-renderer` is its only client; the engine reaches the GPU through the renderer.

// Vulkan through ash is an unsafe API; wrapping it in safe types is this crate's job.
#![allow(unsafe_code)]

mod bindless;
mod buffer;
mod capture;
mod device;
mod frame;
mod image;
mod instance;
mod pipeline;
mod pipeline_cache;
mod surface;
mod swapchain;
mod texture;
mod vma;

use std::fmt;

use ash::vk;
use gg_core::Error;

pub use bindless::{PLACED_TEXTURES, Sampler, Wrap};
pub use buffer::GpuBuffer;
pub use capture::Capture;
pub use device::{Device, MemoryBudget, SHADER_CAPABILITIES, UploadPath};
pub use frame::{
    Arenas, Frame, Frames, Graph, ImageId, ImageRead, Indices, MAX_READS, Pass, PassId, RenderPass,
    Target, Upload,
};
pub use image::ColorImage;
pub use instance::{Instance, InstanceDesc, Validation};
pub use pipeline::{Blend, Faces, Format, GraphicsDesc, Pipeline};
pub use surface::{Surface, Window};
pub use swapchain::Swapchain;
pub use texture::{MAX_TEXTURE_SIZE, Texture, TextureFormat, mip_sizes};

/// The Vulkan version a device must support.
const REQUIRED_VERSION: u32 = vk::make_api_version(0, 1, 4, 0);

/// The whole of a color image: every mip level, and its one layer.
const COLOR_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: vk::REMAINING_MIP_LEVELS,
    base_array_layer: 0,
    layer_count: 1,
};

/// Mip level `level` of a color image, in its one layer: what a copy writes or reads.
const fn color_layers(level: u32) -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: level,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// The region of a copy of `size` pixels between `offset` in a buffer and mip level `level` of a
/// color image, at `origin` in it. The buffer's rows are tightly packed, a row length and image
/// height of 0, in whole blocks for a compressed image.
fn copy_region(offset: u64, origin: [u32; 2], size: [u32; 2], level: u32) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .buffer_offset(offset)
        .image_subresource(color_layers(level))
        .image_offset(vk::Offset3D {
            x: origin[0] as i32,
            y: origin[1] as i32,
            z: 0,
        })
        .image_extent(vk::Extent3D {
            width: size[0],
            height: size[1],
            depth: 1,
        })
}

/// The whole of a depth image, as [`COLOR_RANGE`] is of a color one.
const DEPTH_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::DEPTH,
    ..COLOR_RANGE
};

/// The engine error for a failed Vulkan call made for `subject`: the object it's about, by its
/// debug name, and what was asked of it. The failures callers react to have codes of their own.
fn vk_error(call: &str, subject: impl fmt::Display, result: vk::Result) -> Error {
    let code = match result {
        vk::Result::ERROR_DEVICE_LOST => "gpu.device_lost",
        vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
            "gpu.out_of_memory"
        }
        vk::Result::ERROR_SURFACE_LOST_KHR => "gpu.surface_lost",
        _ => "gpu.vulkan",
    };
    // The result by its name alone, not as the error's source too: ash 0.38 describes Vulkan 1.0's
    // results and ERROR_UNKNOWN alone, and names the rest, which would then show twice.
    let message = format!("{call} failed with {result:?} for {subject}");
    Error::new(code, message)
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
