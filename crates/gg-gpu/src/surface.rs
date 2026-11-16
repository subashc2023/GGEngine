//! Surfaces, through which Vulkan presents to a window.

use std::ptr;
use std::sync::Arc;

use ash::{khr, vk};
use gg_core::{Error, Result};
use raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};

use crate::{Instance, vk_failed};

/// A window the engine can present to: any type with raw window handles, such as winit's.
pub trait Window: HasWindowHandle + HasDisplayHandle + Send + Sync {}

impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync + ?Sized> Window for T {}

/// A window's Vulkan surface. It holds the window, which must outlive it.
pub struct Surface {
    instance: Arc<Instance>,
    raw: vk::SurfaceKHR,
    /// What the debug names and errors of the surface and its swapchain call the window.
    name: String,
    /// Held so it outlives the surface, and a lost surface can be remade for it.
    window: Arc<dyn Window>,
}

impl Surface {
    /// A surface for `window`, which must be on the display `instance` was created for. `name`
    /// tells the window apart in debug names and errors: the surface's, `{name} surface`, and its
    /// swapchain's, such as `{name} swapchain image 0`.
    pub fn new(instance: &Arc<Instance>, window: Arc<dyn Window>, name: &str) -> Result<Self> {
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
        .map_err(vk_failed(
            "ash_window::create_surface",
            format_args!("surface \"{name} surface\""),
        ))?;
        Ok(Self {
            instance: instance.clone(),
            raw,
            name: name.to_owned(),
            window,
        })
    }

    /// A surface for the same window and name, replacing this one when lost. Fails if the window
    /// is gone, as a closed window's handles are.
    pub(crate) fn remake(&self) -> Result<Self> {
        Self::new(&self.instance, self.window.clone(), &self.name)
    }

    pub(crate) fn raw(&self) -> vk::SurfaceKHR {
        self.raw
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Checks that the surface is `instance`'s: it works only with devices of the instance that
    /// made it.
    ///
    /// # Panics
    ///
    /// If another instance made it.
    pub(crate) fn check_instance(&self, instance: &Instance) {
        assert!(
            ptr::eq(Arc::as_ptr(&self.instance), instance),
            "the surface was made by another Vulkan instance than the device's; a surface is used \
             only with the devices of the instance that made it"
        );
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
