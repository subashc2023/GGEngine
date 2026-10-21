//! Turns the render data the engine extracts each frame into images: render graph, passes,
//! materials, lighting and post-processing, built on `gg-gpu`.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

mod ui;

use std::iter;
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

/// Renders frames into a window or, headless, into an offscreen image, and UI into the app's
/// other windows.
pub struct Renderer {
    frames: Frames,
    output: Output,
    /// The keys of the other windows, in the order of their swapchains in `windows`.
    window_keys: Vec<u64>,
    windows: Vec<Swapchain>,
    ui: UiPass,
    instance: Arc<Instance>,
    device: Arc<Device>,
    vsync: bool,
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
        Self::new(
            &instance,
            &device,
            Output::Window(swapchain),
            settings.vsync,
        )
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
        Self::new(&instance, &device, Output::Offscreen(image), settings.vsync)
    }

    fn new(
        instance: &Arc<Instance>,
        device: &Arc<Device>,
        output: Output,
        vsync: bool,
    ) -> Result<Self> {
        let format = match &output {
            Output::Window(swapchain) => swapchain.format(),
            Output::Offscreen(image) => image.format(),
        };
        Ok(Self {
            frames: Frames::new(device)?,
            output,
            window_keys: Vec::new(),
            windows: Vec::new(),
            ui: UiPass::new(device, format)?,
            instance: instance.clone(),
            device: device.clone(),
            vsync,
            validation: instance.validation(),
        })
    }

    /// Tells the renderer that the window's drawable area changed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Output::Window(swapchain) = &mut self.output {
            swapchain.resize(width, height);
        }
    }

    /// Starts presenting to another of the app's windows, which `key` names in
    /// [`Renderer::render`], and whose drawable area is `width` by `height` pixels. A headless
    /// renderer can't, since its GPU was opened without a window.
    ///
    /// # Panics
    ///
    /// If a window named `key` is already open.
    pub fn open_window(
        &mut self,
        key: u64,
        window: Arc<dyn Window>,
        width: u32,
        height: u32,
    ) -> Result<()> {
        assert!(
            !self.window_keys.contains(&key),
            "window {key} is already open"
        );
        let surface = Surface::new(&self.instance, window)?;
        let swapchain = Swapchain::new(&self.device, surface, width, height, self.vsync)?;
        self.ui.add_format(swapchain.format())?;
        self.window_keys.push(key);
        self.windows.push(swapchain);
        Ok(())
    }

    /// Stops presenting to window `key`, once the GPU is done with it, if it's open.
    pub fn close_window(&mut self, key: u64) {
        if let Some(i) = self.window_keys.iter().position(|&open| open == key) {
            self.window_keys.swap_remove(i);
            self.windows.swap_remove(i);
        }
    }

    /// Tells the renderer that window `key`'s drawable area changed size.
    pub fn resize_window(&mut self, key: u64, width: u32, height: u32) {
        if let Some(i) = self.window_keys.iter().position(|&open| open == key) {
            self.windows[i].resize(width, height);
        }
    }

    /// Renders a frame cleared to `clear_color`, in linear RGBA, with `ui` drawn over it, and
    /// draws into each of the other windows its UI from `windows`, over black. It presents them
    /// all. `windows` lists the UIs in the order egui ran their passes, which ran before `ui`'s:
    /// the order of their texture changes. A window that isn't open, or whose swapchain has no
    /// image free yet, is left as it was.
    ///
    /// Returns `false` when there was nothing to render into, as when the window is minimized;
    /// the UIs' texture changes are applied regardless.
    pub fn render(
        &mut self,
        clear_color: [f32; 4],
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        // egui sends each texture change once, so changes wait for a frame that renders.
        for (_, window) in windows.iter_mut() {
            self.ui.queue(window.textures);
        }
        self.ui.queue(ui.textures);
        let target = match &mut self.output {
            Output::Window(swapchain) => Target::Swapchain(swapchain),
            Output::Offscreen(image) => Target::Image(image),
        };
        let has_ui = |key: &u64| windows.iter().any(|(drawn, _)| drawn == key);
        let others = (self.window_keys.iter().zip(&mut self.windows))
            .filter(|(key, _)| has_ui(key))
            .map(|(_, swapchain)| Target::Swapchain(swapchain));
        let Some(mut frame) = self.frames.begin(iter::once(target).chain(others))? else {
            return Ok(false);
        };
        self.ui.write_textures(&mut frame)?;
        let mut pass = frame
            .begin_pass(0, c"frame", clear_color)
            .expect("a frame's first target has an image");
        self.ui
            .draw(&mut pass, ui.primitives, ui.pixels_per_point)?;
        drop(pass);
        let drawn = self
            .window_keys
            .iter()
            .filter_map(|key| windows.iter().find(|(drawn, _)| drawn == key));
        for (target, (_, ui)) in (1..).zip(drawn) {
            if let Some(mut pass) = frame.begin_pass(target, c"window", [0.0, 0.0, 0.0, 1.0]) {
                self.ui
                    .draw(&mut pass, ui.primitives, ui.pixels_per_point)?;
            }
        }
        self.ui.free_textures();
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
