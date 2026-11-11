//! The swapchain: the images a window shows, rebuilt when the window changes size.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::{Error, Result};
use tracing::{debug, info, trace, warn};

use crate::{COLOR_RANGE, Device, Surface, vk_error};

/// The images a window presents, in turn.
pub struct Swapchain {
    device: Arc<Device>,
    surface: Surface,
    raw: vk::SwapchainKHR,
    format: vk::SurfaceFormatKHR,
    present_mode: vk::PresentModeKHR,
    /// Signaled by each paced acquire once the display has let go of the image, and reset after
    /// the wait on it.
    available: vk::Fence,
    /// Whether a paced acquire's wait on `available`, or its reset, failed: the next waits for it
    /// and resets it before acquiring, so that no acquire signals it while it may be signaled.
    available_owed: bool,
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
    /// Set when the window's surface was lost, so that the next acquire makes another for the
    /// window, and the swapchain again.
    surface_lost: bool,
    /// Set once acquiring, rebuilding or presenting failed for good, as [`Swapchain::lost`] says:
    /// the swapchain presents nothing again, and frames leave it out, but for one it's the first
    /// target of, which [`Swapchain::acquire`] fails.
    lost: bool,
}

impl Swapchain {
    /// A swapchain for `surface`, sized to the window's `width` and `height` where the surface
    /// doesn't set its own size. The device's graphics queue must be able to present to the
    /// surface, as it can to the one the device was opened for. With `vsync`, it presents at
    /// the display's refresh rate; otherwise as fast as frames are rendered, where the device
    /// can do that without tearing. Its objects are named after the surface's window, as
    /// [`Surface::new`] says.
    ///
    /// # Panics
    ///
    /// If the surface was made by another instance than the device's.
    pub fn new(
        device: &Arc<Device>,
        surface: Surface,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<Self> {
        let _zone = gg_core::zone!();
        surface.check_instance(device.instance());
        let gpu = device.name();
        if device.swapchain_fn().is_none() {
            return Err(Error::new(
                "gpu.no_swapchain",
                format!("device {gpu:?} was opened without a surface, so it can't present"),
            ));
        }
        let name = surface.name();
        let (formats, present_modes) = support(device, &surface)?;
        let format = choose_format(&formats).ok_or_else(|| {
            Error::new(
                "gpu.no_surface_format",
                format!("{} offers no formats for the window", device.name()),
            )
        })?;
        debug!(modes = ?present_modes, "Present modes");
        let present_mode = choose_present_mode(&present_modes, vsync);
        // SAFETY: the device is live.
        let available = unsafe {
            device
                .raw()
                .create_fence(&vk::FenceCreateInfo::default(), None)
        }
        .map_err(|e| {
            let fence = format_args!("fence \"{name} swapchain image available\"");
            vk_error("vkCreateFence", fence, e)
        })?;
        // SAFETY: the fence is live, and the surface is of the device's instance.
        unsafe {
            device.set_name(available, &format!("{name} swapchain image available"));
            device.set_name(surface.raw(), &format!("{name} surface"));
        }
        // From here on, dropping `swapchain` destroys the fence.
        let mut swapchain = Self {
            device: device.clone(),
            surface,
            raw: vk::SwapchainKHR::null(),
            format,
            present_mode,
            available,
            available_owed: false,
            extent: vk::Extent2D::default(),
            window_extent: vk::Extent2D { width, height },
            images: Vec::new(),
            views: Vec::new(),
            rendered: Vec::new(),
            stale: true,
            surface_lost: false,
            lost: false,
        };
        swapchain.rebuild()?;
        Ok(swapchain)
    }

    /// The format of the images it presents, which pipelines that draw into them are made for.
    pub fn format(&self) -> crate::Format {
        crate::Format::new(self.format.format)
    }

    /// Whether the swapchain is lost for good, so it presents nothing again: acquiring, rebuilding
    /// or presenting failed other than by running out of memory, after which it's rebuilt, or by
    /// the device's loss, which fails every frame; or its window's surface was lost, and another
    /// couldn't be made for the window, as when the window is gone. Frames go on without it, into
    /// their other targets; a frame it's the first target of fails to begin, with
    /// `gpu.swapchain_lost`. Only another swapchain, with another surface, presents to the window.
    pub fn lost(&self) -> bool {
        self.lost
    }

    /// Tells the swapchain that the window is now `width` by `height`, so that it's rebuilt
    /// before the next frame if its size no longer matches.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.window_extent = vk::Extent2D { width, height };
        if self.window_extent != self.extent {
            self.stale = true;
        }
    }

    /// Acquires the next image, and signals `acquired` for the GPU to wait on. With `pace`, it
    /// waits until the display has let go of the image; without, it takes an image only if the
    /// swapchain has one to give at once. Returns `None` when there's nothing to present to
    /// this time: the window has no area, the swapchain went out of date and is rebuilt for the
    /// next frame, or, without `pace`, no image is free yet. With `pace`, it adds how long it
    /// waited for the display to `waited`, which a rebuild first isn't part of.
    ///
    /// A failure leaves `acquired` unsignaled. A surface lost is made again for the window, and
    /// the swapchain with it, then acquired from once more; running out of memory leaves the
    /// swapchain to rebuild at the next acquire; the device's loss fails every frame; any other
    /// failure, or a surface that can't be made again, leaves the swapchain lost
    /// ([`Swapchain::lost`]), and every acquire after fails with `gpu.swapchain_lost`. An image
    /// acquired is given whatever follows, so that it's presented and its semaphore waited on: a
    /// failed wait for the display only goes unpaced, and the next paced acquire waits first.
    ///
    /// # Safety
    ///
    /// `acquired` must be unsignaled, with no wait on it pending.
    pub(crate) unsafe fn acquire(
        &mut self,
        acquired: vk::Semaphore,
        pace: bool,
        waited: &mut Duration,
    ) -> Result<Option<u32>> {
        if self.lost {
            let name = self.surface.name();
            return Err(Error::new(
                "gpu.swapchain_lost",
                format!(
                    "swapchain \"{name} swapchain\" was lost, as a failure before said, and \
                     presents nothing again; drop it and make another for the window"
                ),
            ));
        }
        let mut remade = false;
        loop {
            if self.surface_lost {
                remade = true;
                if let Err(error) = self.remake_surface() {
                    // Lost for good, but for running out of memory, after which the next acquire
                    // tries again: a window gone has no surface to make.
                    let again = matches!(failure(&error), Failure::Device | Failure::Memory);
                    self.lost |= !again;
                    return Err(error);
                }
            }
            // SAFETY: the caller guarantees the semaphore, which a failure leaves unsignaled.
            let tried = unsafe { self.try_acquire(acquired, pace, waited) };
            if let Err(error) = &tried {
                self.note(error);
                // A surface lost again, just after it was made, is lost for good.
                if self.surface_lost && !remade {
                    continue;
                }
                self.lost |= self.surface_lost;
            }
            return tried;
        }
    }

    /// Notes what `error`, of the swapchain's, leaves it to do, as [`failure`] sorts it.
    fn note(&mut self, error: &Error) {
        match failure(error) {
            Failure::Device => {}
            Failure::Surface => self.surface_lost = true,
            Failure::Memory => self.stale = true,
            Failure::Lost => self.lost = true,
        }
    }

    /// Makes another surface for the window, in place of one lost, once the device is idle and the
    /// swapchain and its objects, the lost surface's, are destroyed: the next acquire rebuilds the
    /// swapchain on it, in the format and present mode chosen before. Fails if the window is gone,
    /// or the new surface can't take that format or present from the graphics queue.
    fn remake_surface(&mut self) -> Result<()> {
        let _zone = gg_core::zone!();
        self.device.wait_idle()?;
        // SAFETY: the device is idle, so nothing uses the swapchain or its objects; the surface
        // goes after it, as it's replaced below.
        unsafe {
            self.destroy_views_and_semaphores();
            self.swapchain_fn().destroy_swapchain(self.raw, None);
        }
        self.raw = vk::SwapchainKHR::null();
        self.images.clear();
        self.stale = true;
        let surface = self.surface.remake()?;
        // SAFETY: the surface is live, and of the device's instance.
        unsafe { (self.device).set_name(surface.raw(), &format!("{} surface", surface.name())) };
        let (formats, modes) = support(&self.device, &surface)?;
        if !formats.contains(&self.format) {
            let name = surface.name();
            return Err(Error::new(
                "gpu.no_surface_format",
                format!(
                    "surface \"{name} surface\", made again, offers no {:?} for the swapchain",
                    self.format.format
                ),
            ));
        }
        if !modes.contains(&self.present_mode) {
            self.present_mode = vk::PresentModeKHR::FIFO;
        }
        // The lost surface goes here, after the swapchain made on it.
        self.surface = surface;
        self.surface_lost = false;
        info!(window = self.surface.name(), "Surface made again");
        Ok(())
    }

    /// Waits for the fence a paced acquire signaled, and resets it, noting a failure for the next
    /// paced acquire to wait again.
    fn wait_available(&mut self) -> Result<()> {
        let fences = [self.available];
        let raw = self.device.raw();
        // SAFETY: an acquire signaled the fence, or will, and nothing else uses it.
        let waited = unsafe {
            raw.wait_for_fences(&fences, true, u64::MAX)
                .and_then(|()| raw.reset_fences(&fences))
        };
        self.available_owed = waited.is_err();
        waited.map_err(|e| {
            let name = self.surface.name();
            let fence = format_args!("fence \"{name} swapchain image available\"");
            vk_error("vkWaitForFences", fence, e)
        })
    }

    /// As [`Swapchain::acquire`], but for what a failure leaves the swapchain to do.
    ///
    /// # Safety
    ///
    /// As for [`Swapchain::acquire`].
    unsafe fn try_acquire(
        &mut self,
        acquired: vk::Semaphore,
        pace: bool,
        waited: &mut Duration,
    ) -> Result<Option<u32>> {
        if self.stale && !self.rebuild()? {
            return Ok(None);
        }
        if pace && self.available_owed {
            // Nothing is acquired yet, so a failure here leaves nothing behind.
            self.wait_available()?;
        }
        let swapchain_fn = self.swapchain_fn();
        let (timeout, fence) = if pace {
            (u64::MAX, self.available)
        } else {
            (0, vk::Fence::null())
        };
        let start = Instant::now();
        let _zone = pace.then(|| gg_core::zone!("wait for the display"));
        // SAFETY: the swapchain is live, the caller guarantees the semaphore, and the fence is
        // unsignaled, since it's reset after each wait on it.
        let acquired =
            unsafe { swapchain_fn.acquire_next_image(self.raw, timeout, acquired, fence) };
        let image = match acquired {
            // A suboptimal image is still presented; the swapchain is rebuilt after it.
            Ok((image, suboptimal)) => {
                self.stale |= suboptimal;
                // Waits until the display has let go of the image, which under FIFO is at a
                // vertical sync, so that frames start once per refresh. Waiting only for the GPU
                // to finish the frame before last let them start in pairs, 1 ms apart and then
                // 7 ms at 240 Hz, though the display still showed one per refresh. The image is
                // acquired, and its semaphore signaled, whether or not the wait goes: a failure
                // leaves it to the next paced acquire, and the image is still given, to present.
                if pace && let Err(error) = self.wait_available() {
                    warn!(
                        code = error.code(),
                        swapchain = self.surface.name(),
                        "{error:#}; the frame goes unpaced"
                    );
                }
                Some(image)
            }
            // Nothing was acquired, so the semaphore stays unsignaled.
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.stale = true;
                None
            }
            Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) if !pace => None,
            Err(e) => {
                let (name, vk::Extent2D { width, height }) = (self.surface.name(), self.extent);
                let subject =
                    format_args!("swapchain \"{name} swapchain\" of {width}x{height} pixels");
                return Err(vk_error("vkAcquireNextImageKHR", subject, e));
            }
        };
        if pace {
            *waited += start.elapsed();
        }
        Ok(image)
    }

    /// Notes how presenting one of its images went: a suboptimal or out-of-date swapchain, or
    /// one whose present ran out of memory, which left its image acquired and its semaphore
    /// signaled, is rebuilt before the next frame, and its semaphores with it; a surface lost is
    /// made again at the next acquire; any other failure, but the device's loss, leaves the
    /// swapchain lost ([`Swapchain::lost`]).
    pub(crate) fn presented(&mut self, result: vk::Result) -> Result<()> {
        let presented = self.present_result(result);
        if let Err(error) = &presented {
            self.note(error);
        }
        presented
    }

    fn present_result(&mut self, result: vk::Result) -> Result<()> {
        match result {
            vk::Result::SUCCESS => Ok(()),
            // An out-of-date present's semaphore wait still happens, so the frame's resources
            // are freed as usual.
            vk::Result::SUBOPTIMAL_KHR | vk::Result::ERROR_OUT_OF_DATE_KHR => {
                self.stale = true;
                Ok(())
            }
            e => {
                let (name, vk::Extent2D { width, height }) = (self.surface.name(), self.extent);
                let subject =
                    format_args!("swapchain \"{name} swapchain\" of {width}x{height} pixels");
                Err(vk_error("vkQueuePresentKHR", subject, e))
            }
        }
    }

    pub(crate) fn raw(&self) -> vk::SwapchainKHR {
        self.raw
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
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
        let _zone = gg_core::zone!();
        let device = self.device.clone();
        let name = self.surface.name().to_owned();
        // SAFETY: the physical device and the surface are live.
        let capabilities = unsafe {
            self.surface
                .surface_fn()
                .get_physical_device_surface_capabilities(device.physical(), self.surface.raw())
        }
        .map_err(|e| {
            let gpu = device.name();
            let subject = format_args!("surface \"{name} surface\" on device {gpu:?}");
            vk_error("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", subject, e)
        })?;
        // The window can resize between this query and the swapchain's creation below, which then
        // makes it at a size the surface just left: a race no query closes, which validation lets
        // pass, as it checks the size against this query, and which the next acquire or present
        // reports as out of date, so that the swapchain is rebuilt again.
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
            .present_mode(self.present_mode)
            .clipped(true)
            .old_swapchain(self.raw);
        let swapchain_fn = device
            .swapchain_fn()
            .expect("a swapchain's device was opened for a surface");
        let first = self.raw == vk::SwapchainKHR::null();
        // SAFETY: the surface is live, and the device is idle, so nothing uses the old swapchain,
        // its views or its semaphores. Creating the new swapchain retires the old one; the new
        // one's objects are live as they're named.
        unsafe {
            let created = swapchain_fn.create_swapchain(&info, None);
            if created.is_err() {
                // The old swapchain is retired whether or not the new one is made, and a create
                // may name only one that isn't (VUID-VkSwapchainCreateInfoKHR-oldSwapchain-01933),
                // so it goes, and the next rebuild starts afresh.
                self.destroy_views_and_semaphores();
                swapchain_fn.destroy_swapchain(self.raw, None);
                self.raw = vk::SwapchainKHR::null();
                self.images.clear();
            }
            let raw = created.map_err(|e| {
                let subject = format_args!(
                    "swapchain \"{name} swapchain\" of {}x{} {:?} pixels, presenting by {:?}",
                    extent.width, extent.height, self.format.format, self.present_mode
                );
                vk_error("vkCreateSwapchainKHR", subject, e)
            })?;
            self.destroy_views_and_semaphores();
            swapchain_fn.destroy_swapchain(self.raw, None);
            self.raw = raw;
            device.set_name(raw, &format!("{name} swapchain"));
            self.extent = extent;
            self.images = swapchain_fn.get_swapchain_images(raw).map_err(|e| {
                let subject = format_args!("swapchain \"{name} swapchain\"'s images");
                vk_error("vkGetSwapchainImagesKHR", subject, e)
            })?;
            for (i, &image) in self.images.iter().enumerate() {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(self.format.format)
                    .subresource_range(COLOR_RANGE);
                let view = device
                    .raw()
                    .create_image_view(&view_info, None)
                    .map_err(|e| {
                        let subject = format_args!("image \"{name} swapchain image {i}\"'s view");
                        vk_error("vkCreateImageView", subject, e)
                    })?;
                self.views.push(view);
                let rendered = device
                    .raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| {
                        let subject =
                            format_args!("semaphore \"{name} swapchain image {i} rendered\"");
                        vk_error("vkCreateSemaphore", subject, e)
                    })?;
                self.rendered.push(rendered);
                let image_name = format!("{name} swapchain image {i}");
                device.set_name(image, &image_name);
                device.set_name(view, &image_name);
                device.set_name(rendered, &format!("{image_name} rendered"));
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
                present_mode = ?self.present_mode,
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
        // An acquire whose wait on the fence failed may still signal it: waited for again, for a
        // second at most, and otherwise left undestroyed, a fence's leak rather than a signal
        // into a destroyed fence.
        let fence_done = !self.available_owed || {
            let fences = [self.available];
            // SAFETY: the fence is live, and nothing else uses it.
            unsafe { (self.device.raw()).wait_for_fences(&fences, true, 1_000_000_000) }.is_ok()
        };
        if !fence_done {
            warn!(
                code = "gpu.fence_leaked",
                swapchain = self.surface.name(),
                "The swapchain's acquire fence never signaled, so it's left undestroyed"
            );
        }
        // SAFETY: the device is idle, so nothing uses the swapchain or its objects, and the fence
        // is destroyed only once every acquire that signaled it was waited on. The surface, a
        // field, is destroyed after it.
        unsafe {
            self.destroy_views_and_semaphores();
            self.swapchain_fn().destroy_swapchain(self.raw, None);
            if fence_done {
                self.device.raw().destroy_fence(self.available, None);
            }
        }
    }
}

/// What a swapchain's failure leaves it to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The device was lost, which fails every frame.
    Device,
    /// Its window's surface was lost: the next acquire makes another, and the swapchain on it.
    Surface,
    /// Memory ran out: the next acquire rebuilds the swapchain.
    Memory,
    /// Lost for good ([`Swapchain::lost`]).
    Lost,
}

/// What `error`, a swapchain's, leaves it to do, by its code.
pub(crate) fn failure(error: &Error) -> Failure {
    match error.code() {
        "gpu.device_lost" => Failure::Device,
        "gpu.surface_lost" => Failure::Surface,
        "gpu.out_of_memory" => Failure::Memory,
        _ => Failure::Lost,
    }
}

/// What `surface` offers `device`: its formats and present modes, checking first that the device's
/// graphics queue presents to it.
fn support(
    device: &Device,
    surface: &Surface,
) -> Result<(Vec<vk::SurfaceFormatKHR>, Vec<vk::PresentModeKHR>)> {
    let (name, gpu) = (surface.name(), device.name());
    // The surface, by its debug name.
    let surface_named = format_args!("surface \"{name} surface\" on device {gpu:?}");
    let (surface_fn, physical) = (surface.surface_fn(), device.physical());
    // SAFETY: the physical device and the surface are live, and the family is the device's.
    let presents = unsafe {
        surface_fn.get_physical_device_surface_support(
            physical,
            device.graphics_family(),
            surface.raw(),
        )
    }
    .map_err(|e| vk_error("vkGetPhysicalDeviceSurfaceSupportKHR", surface_named, e))?;
    if !presents {
        return Err(Error::new(
            "gpu.no_present",
            format!("{gpu}'s graphics queue can't present to the window"),
        ));
    }
    // SAFETY: the physical device and the surface are live.
    let formats =
        unsafe { surface_fn.get_physical_device_surface_formats(physical, surface.raw()) }
            .map_err(|e| vk_error("vkGetPhysicalDeviceSurfaceFormatsKHR", surface_named, e))?;
    // SAFETY: the physical device and the surface are live.
    let modes =
        unsafe { surface_fn.get_physical_device_surface_present_modes(physical, surface.raw()) }
            .map_err(|e| {
                let call = "vkGetPhysicalDeviceSurfacePresentModesKHR";
                vk_error(call, surface_named, e)
            })?;
    Ok((formats, modes))
}

/// FIFO with `vsync`, and otherwise mailbox where the device has it, which renders as fast as
/// the GPU can and shows the newest finished frame at each vertical sync, without tearing. FIFO,
/// which every device supports, waits for vertical sync, so the frame rate is the display's.
fn choose_present_mode(modes: &[vk::PresentModeKHR], vsync: bool) -> vk::PresentModeKHR {
    if !vsync && modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        vk::PresentModeKHR::FIFO
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A surface lost is made again, memory run out rebuilds, the device's loss fails every frame,
    /// and the rest lose the swapchain for good.
    #[test]
    fn a_swapchains_failures_remake_its_surface_rebuild_it_or_lose_it() {
        let failed = |result| failure(&vk_error("vkAcquireNextImageKHR", "a swapchain", result));
        assert_eq!(failed(vk::Result::ERROR_SURFACE_LOST_KHR), Failure::Surface);
        assert_eq!(
            failed(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            Failure::Memory
        );
        assert_eq!(
            failed(vk::Result::ERROR_OUT_OF_HOST_MEMORY),
            Failure::Memory
        );
        assert_eq!(failed(vk::Result::ERROR_DEVICE_LOST), Failure::Device);
        let in_use = failed(vk::Result::ERROR_NATIVE_WINDOW_IN_USE_KHR);
        assert_eq!(in_use, Failure::Lost);
    }
}
