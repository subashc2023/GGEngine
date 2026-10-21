//! Surfaces, through which Vulkan presents to a window.

use std::sync::Arc;

use ash::{khr, vk};
use gg_core::{Error, Result};
use raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};

use crate::{Instance, vk_error};

/// A window the engine can present to: any window type with raw window handles, such as
/// winit's.
pub trait Window: HasWindowHandle + HasDisplayHandle + Send + Sync {}

impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync + ?Sized> Window for T {}

/// A window's Vulkan surface. It holds on to the window, which must outlive it.
pub struct Surface {
    instance: Arc<Instance>,
    raw: vk::SurfaceKHR,
    _window: Arc<dyn Window>,
}

impl Surface {
    /// A surface for `window`, which must be the window `instance` was created for.
    pub fn new(instance: &Arc<Instance>, window: Arc<dyn Window>) -> Result<Self> {
        if instance.surface_fn().is_none() {
            return Err(Error::new(
                "gpu.no_surface",
                "the Vulkan instance was created without a window, so it can't present to one",
            ));
        }
        let handle_error = |error: HandleError| {
            Error::new("gpu.window", "can't get the window's handles").with_source(error)
        };
        let display = window.display_handle().map_err(handle_error)?.as_raw();
        let handle = window.window_handle().map_err(handle_error)?.as_raw();
        // SAFETY: the surface holds the window and the instance, so both outlive it.
        let raw = unsafe {
            ash_window::create_surface(instance.entry(), instance.raw(), display, handle, None)
        }
        .map_err(|e| vk_error("creating the window's surface", e))?;
        Ok(Self {
            instance: instance.clone(),
            raw,
            _window: window,
        })
    }

    pub(crate) fn raw(&self) -> vk::SurfaceKHR {
        self.raw
    }

    pub(crate) fn surface_fn(&self) -> &khr::surface::Instance {
        self.instance
            .surface_fn()
            .expect("a surface's instance was made for a window")
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the swapchain that owns a surface is destroyed before it.
        unsafe { self.surface_fn().destroy_surface(self.raw, None) };
    }
}
