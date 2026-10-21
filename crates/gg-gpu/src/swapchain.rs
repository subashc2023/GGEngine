//! The swapchain: the images a window shows, rebuilt when the window changes size.

use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};
use tracing::{info, trace};

use crate::{COLOR_RANGE, Device, Surface, vk_error};

/// The images a window presents, in turn.
pub struct Swapchain {
    device: Arc<Device>,
    surface: Surface,
    raw: vk::SwapchainKHR,
    format: vk::SurfaceFormatKHR,
    extent: vk::Extent2D,
    /// The window's size, for surfaces that take their size from the swapchain.
    window_extent: vk::Extent2D,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    /// For each image, signaled when the frame that renders into it finishes, and waited on by
    /// its present. They're per image rather than per frame in flight because a present may
    /// still wait on its semaphore when the next frame starts; acquiring the image again shows
    /// that the wait is over
    /// (<https://docs.vulkan.org/guide/latest/swapchain_semaphore_reuse.html>).
    rendered: Vec<vk::Semaphore>,
    /// Set when the swapchain no longer matches the window and must be rebuilt.
    stale: bool,
}

impl Swapchain {
    /// A swapchain for `surface`, the surface `device` was opened for, sized to the window's
    /// `width` and `height` where the surface doesn't set its own size.
    pub fn new(device: &Arc<Device>, surface: Surface, width: u32, height: u32) -> Result<Self> {
        if device.swapchain_fn().is_none() {
            return Err(Error::new(
                "gpu.no_swapchain",
                "the device was opened without a surface, so it can't present",
            ));
        }
        // SAFETY: the physical device and the surface are live.
        let formats = unsafe {
            surface
                .surface_fn()
                .get_physical_device_surface_formats(device.physical(), surface.raw())
        }
        .map_err(|e| vk_error("vkGetPhysicalDeviceSurfaceFormatsKHR", e))?;
        let format = choose_format(&formats).ok_or_else(|| {
            Error::new(
                "gpu.no_surface_format",
                format!("{} offers no formats for the window", device.name()),
            )
        })?;
        let mut swapchain = Self {
            device: device.clone(),
            surface,
            raw: vk::SwapchainKHR::null(),
            format,
            extent: vk::Extent2D::default(),
            window_extent: vk::Extent2D { width, height },
            images: Vec::new(),
            views: Vec::new(),
            rendered: Vec::new(),
            stale: true,
        };
        swapchain.rebuild()?;
        Ok(swapchain)
    }

    /// Tells the swapchain that the window is now `width` by `height`, so that it's rebuilt
    /// before the next frame if its size no longer matches.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.window_extent = vk::Extent2D { width, height };
        if self.window_extent != self.extent {
            self.stale = true;
        }
    }

    /// Acquires the next image, signaling `acquired` once it's ready to render into. Returns
    /// `None` when there's nothing to present to this time: the window has no area, or the
    /// swapchain went out of date and is rebuilt for the next frame.
    ///
    /// # Safety
    ///
    /// `acquired` must be unsignaled, with no wait on it pending.
    pub(crate) unsafe fn acquire(&mut self, acquired: vk::Semaphore) -> Result<Option<u32>> {
        if self.stale && !self.rebuild()? {
            return Ok(None);
        }
        let swapchain_fn = self.swapchain_fn();
        // SAFETY: the swapchain is live, and the caller guarantees the semaphore.
        let acquired = unsafe {
            swapchain_fn.acquire_next_image(self.raw, u64::MAX, acquired, vk::Fence::null())
        };
        match acquired {
            // A suboptimal image is still presented; the swapchain is rebuilt after it.
            Ok((image, suboptimal)) => {
                self.stale |= suboptimal;
                Ok(Some(image))
            }
            // Nothing was acquired, so the semaphore stays unsignaled.
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.stale = true;
                Ok(None)
            }
            Err(e) => Err(vk_error("vkAcquireNextImageKHR", e)),
        }
    }

    /// Presents `image` once the frame rendering into it has signaled its semaphore.
    ///
    /// # Safety
    ///
    /// `image` must have been acquired, and its frame submitted with its `rendered` semaphore
    /// signaled.
    pub(crate) unsafe fn present(&mut self, image: u32) -> Result<()> {
        let waits = [self.rendered[image as usize]];
        let swapchains = [self.raw];
        let images = [image];
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&images);
        // SAFETY: the caller guarantees the image and its semaphore.
        match unsafe { self.device.present(&info) } {
            Ok(suboptimal) => {
                self.stale |= suboptimal;
                Ok(())
            }
            // The present's semaphore wait still happens, so the frame's resources are freed as
            // usual.
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.stale = true;
                Ok(())
            }
            Err(e) => Err(vk_error("vkQueuePresentKHR", e)),
        }
    }

    /// The `image`th image, its view and the swapchain's size.
    pub(crate) fn image(&self, image: u32) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        let image = image as usize;
        (self.images[image], self.views[image], self.extent)
    }

    /// The semaphore the `image`th image's frame signals for its present.
    pub(crate) fn rendered(&self, image: u32) -> vk::Semaphore {
        self.rendered[image as usize]
    }

    fn swapchain_fn(&self) -> &ash::khr::swapchain::Device {
        self.device
            .swapchain_fn()
            .expect("a swapchain's device was opened for a surface")
    }

    /// Rebuilds the swapchain at the surface's current size. Returns `false`, leaving it stale,
    /// if the window has no area to present to, as when it's minimized.
    fn rebuild(&mut self) -> Result<bool> {
        let device = self.device.clone();
        // SAFETY: the physical device and the surface are live.
        let capabilities = unsafe {
            self.surface
                .surface_fn()
                .get_physical_device_surface_capabilities(device.physical(), self.surface.raw())
        }
        .map_err(|e| vk_error("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", e))?;
        // A current width of u32::MAX means the surface takes its size from the swapchain.
        let extent = if capabilities.current_extent.width == u32::MAX {
            let (min, max) = (capabilities.min_image_extent, capabilities.max_image_extent);
            vk::Extent2D {
                width: self.window_extent.width.clamp(min.width, max.width),
                height: self.window_extent.height.clamp(min.height, max.height),
            }
        } else {
            capabilities.current_extent
        };
        if extent.width == 0 || extent.height == 0 {
            return Ok(false);
        }
        // Frames in flight may still use the old swapchain's images and semaphores. Waiting for
        // the device doesn't cover presents, whose semaphore waits could in theory still be
        // pending; VK_KHR_swapchain_maintenance1's present fences would close that gap.
        device.wait_idle()?;

        let max_images = match capabilities.max_image_count {
            0 => u32::MAX,
            max => max,
        };
        let composite_alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::INHERIT,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        ]
        .into_iter()
        .find(|&alpha| capabilities.supported_composite_alpha.contains(alpha))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface.raw())
            .min_image_count((capabilities.min_image_count + 1).min(max_images))
            .image_format(self.format.format)
            .image_color_space(self.format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(composite_alpha)
            // FIFO waits for vertical sync, and every device supports it.
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(self.raw);
        let swapchain_fn = device
            .swapchain_fn()
            .expect("a swapchain's device was opened for a surface");
        let first = self.raw == vk::SwapchainKHR::null();
        // SAFETY: the surface is live, and the device is idle, so nothing uses the old swapchain,
        // its views or its semaphores. Creating the new swapchain retires the old one.
        unsafe {
            let raw = swapchain_fn
                .create_swapchain(&info, None)
                .map_err(|e| vk_error("vkCreateSwapchainKHR", e))?;
            self.destroy_views_and_semaphores();
            swapchain_fn.destroy_swapchain(self.raw, None);
            self.raw = raw;
            self.extent = extent;
            self.images = swapchain_fn
                .get_swapchain_images(raw)
                .map_err(|e| vk_error("vkGetSwapchainImagesKHR", e))?;
            for (i, &image) in self.images.iter().enumerate() {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(self.format.format)
                    .subresource_range(COLOR_RANGE);
                let view = device
                    .raw()
                    .create_image_view(&view_info, None)
                    .map_err(|e| vk_error("vkCreateImageView", e))?;
                self.views.push(view);
                let rendered = device
                    .raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| vk_error("vkCreateSemaphore", e))?;
                self.rendered.push(rendered);
                device.set_name(image, &format!("swapchain image {i}"));
                device.set_name(view, &format!("swapchain image {i}"));
                device.set_name(rendered, &format!("swapchain image {i} rendered"));
            }
        }
        self.stale = false;
        // Dragging a window's edge rebuilds the swapchain every frame, so rebuilds log at trace.
        if first {
            info!(
                width = extent.width,
                height = extent.height,
                images = self.images.len(),
                format = ?self.format.format,
                color_space = ?self.format.color_space,
                present_mode = "fifo",
                "Swapchain created"
            );
        } else {
            trace!(
                width = extent.width,
                height = extent.height,
                "Swapchain rebuilt"
            );
        }
        Ok(true)
    }

    /// # Safety
    ///
    /// The device must be idle.
    unsafe fn destroy_views_and_semaphores(&mut self) {
        let raw = self.device.raw();
        // SAFETY: the caller guarantees the GPU is done with them.
        unsafe {
            for view in self.views.drain(..) {
                raw.destroy_image_view(view, None);
            }
            for semaphore in self.rendered.drain(..) {
                raw.destroy_semaphore(semaphore, None);
            }
        }
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // Frames in flight may still use the images and semaphores.
        let _ = self.device.wait_idle();
        // SAFETY: the device is idle, so nothing uses the swapchain or its objects. The surface,
        // a field, is destroyed after it.
        unsafe {
            self.destroy_views_and_semaphores();
            self.swapchain_fn().destroy_swapchain(self.raw, None);
        }
    }
}

/// Prefers 8-bit BGRA or RGBA in sRGB, which desktop drivers offer for windows.
fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> Option<vk::SurfaceFormatKHR> {
    formats
        .iter()
        .find(|format| {
            format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                && matches!(
                    format.format,
                    vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB
                )
        })
        .or(formats.first())
        .copied()
}
