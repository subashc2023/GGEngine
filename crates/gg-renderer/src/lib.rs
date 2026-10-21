//! Turns the render data the engine extracts each frame into images: render graph, passes,
//! materials, lighting and post-processing, built on `gg-gpu`.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

mod ui;

use std::sync::Arc;

use gg_core::Result;
use gg_gpu::{ColorImage, Device, Frames, Instance, InstanceDesc, Surface, Swapchain, Target};

pub use gg_gpu::{Validation, Window};
pub use ui::Ui;

use ui::UiPass;

/// How to set up a renderer.
pub struct Settings<'a> {
    /// The app's name, which GPU tools such as RenderDoc show.
    pub app_name: &'a str,
    /// Run under Vulkan validation.
    pub validation: bool,
    /// Use the GPU whose name contains this, ignoring case.
    pub gpu: Option<&'a str>,
    /// Present at the display's refresh rate, rather than as fast as frames are rendered.
    pub vsync: bool,
}

/// Renders frames into a window or, headless, into an offscreen image.
pub struct Renderer {
    frames: Frames,
    output: Output,
    ui: UiPass,
    validation: Validation,
}

enum Output {
    Window(Swapchain),
    Offscreen(ColorImage),
}

impl Renderer {
    /// A renderer that presents to `window`, whose drawable area is `width` by `height` pixels.
    pub fn windowed(
        settings: &Settings<'_>,
        window: Arc<dyn Window>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let instance = Instance::new(&InstanceDesc {
            app_name: settings.app_name,
            validation: settings.validation,
            window: Some(&*window),
        })?;
        let surface = Surface::new(&instance, window)?;
        let device = Device::new(&instance, settings.gpu, Some(&surface))?;
        let swapchain = Swapchain::new(&device, surface, width, height, settings.vsync)?;
        Self::new(&instance, &device, Output::Window(swapchain))
    }

    /// A renderer without a window, which renders into an offscreen image of `width` by
    /// `height` pixels.
    pub fn headless(settings: &Settings<'_>, width: u32, height: u32) -> Result<Self> {
        let instance = Instance::new(&InstanceDesc {
            app_name: settings.app_name,
            validation: settings.validation,
            window: None,
        })?;
        let device = Device::new(&instance, settings.gpu, None)?;
        let image = ColorImage::new(&device, width, height, "offscreen color")?;
        Self::new(&instance, &device, Output::Offscreen(image))
    }

    fn new(instance: &Instance, device: &Arc<Device>, output: Output) -> Result<Self> {
        let format = match &output {
            Output::Window(swapchain) => swapchain.format(),
            Output::Offscreen(image) => image.format(),
        };
        Ok(Self {
            frames: Frames::new(device)?,
            output,
            ui: UiPass::new(device, format)?,
            validation: instance.validation(),
        })
    }

    /// Tells the renderer that the window's drawable area changed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Output::Window(swapchain) = &mut self.output {
            swapchain.resize(width, height);
        }
    }

    /// Renders a frame cleared to `clear_color`, in linear RGBA, with `ui` drawn over it, and
    /// presents it. Returns `false` when there was nothing to render into, as when the window
    /// is minimized; the UI's texture changes are applied regardless.
    pub fn render(&mut self, clear_color: [f32; 4], ui: Ui<'_>) -> Result<bool> {
        let target = match &mut self.output {
            Output::Window(swapchain) => Target::Swapchain(swapchain),
            Output::Offscreen(image) => Target::Image(image),
        };
        // egui sends each texture change once, so changes wait for a frame that renders.
        self.ui.queue(ui.textures);
        let Some(mut frame) = self.frames.begin(target)? else {
            return Ok(false);
        };
        self.ui.write_textures(&mut frame)?;
        let mut pass = frame.begin_pass(c"frame", clear_color);
        self.ui
            .draw(&mut pass, ui.primitives, ui.pixels_per_point)?;
        drop(pass);
        frame.submit()?;
        Ok(true)
    }

    /// Copies the last frame rendered offscreen to the CPU: rows of RGBA8 pixels, sRGB-encoded
    /// and tightly packed, top row first. `None` for a renderer with a window.
    pub fn read_back(&mut self) -> Result<Option<Vec<u8>>> {
        match &mut self.output {
            Output::Window(_) => Ok(None),
            Output::Offscreen(image) => image.read_back().map(Some),
        }
    }

    /// What validation reports. It outlives the renderer, so it also counts what's reported
    /// while the renderer shuts down.
    pub fn validation(&self) -> Validation {
        self.validation.clone()
    }
}
