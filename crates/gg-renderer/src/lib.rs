//! Turns the render data the engine hands it each frame into images, built on `gg-gpu`: meshes,
//! sprites, circles and lines drawn in the game's view, as its camera sees them, with games' shaders
//! and textures and the engine's own, models' unlit materials among them, and the UI over them;
//! picks, which read back the ID of the draw the view shows at a pixel; an outline round what the
//! view shows of an ID; and the render graph, lighting, PBR materials and post-processing to come.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

mod circle;
mod draws;
mod line;
mod mesh;
mod model;
mod pick;
mod shader;
mod sprite;
mod texture;
mod ui;
mod view;
mod viewpoint;

use std::iter;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use gg_core::{Error, Result};
use gg_gpu::{
    ColorImage, DepthImage, Device, Frames, IdImage, Instance, InstanceDesc, Surface, Swapchain,
    Target,
};
use tracing::{info, trace};

pub use circle::Circle;
pub use draws::Draws;
pub use gg_gpu::{Blend, MAX_TEXTURE_SIZE, Sampler, UploadPath, Validation, Window, Wrap};
pub use line::Line;
pub use mesh::{Mesh, MeshDraw, Meshes};
#[doc(hidden)]
pub use model::Params as ModelParams;
pub use model::{AlphaMode, Material, ModelVertex};
pub use pick::{MAX_PICKS, Pick};
pub use shader::{Compiled, Shader, ShaderSource, Shaders};
pub use sprite::{Sprite, SpriteSheet};
pub use texture::{Mipmaps, Texture, Textures};
pub use ui::Ui;
pub use view::{Outline, ViewCallback};
pub use viewpoint::{Projection, Viewpoint};

use mesh::MeshPass;
use pick::Picks;
use ui::UiPass;
use view::{Shown, ViewPass};

/// The `size` pixels of a frame whose top left is at `origin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

impl Viewport {
    /// The pixels of `rect`, in points of `pixels_per_point` pixels each, rounded as egui rounds
    /// its clip rectangles.
    pub fn from_points(rect: epaint::Rect, pixels_per_point: f32) -> Self {
        let min = (rect.min.to_vec2() * pixels_per_point).round();
        let max = (rect.max.to_vec2() * pixels_per_point).round();
        // Casts saturate, so a rect without an area has none in pixels either.
        Self {
            origin: [min.x as u32, min.y as u32],
            size: [(max.x - min.x) as u32, (max.y - min.y) as u32],
        }
    }

    /// Its width over its height: the aspect ratio the renderer projects the game's view with.
    pub fn aspect(&self) -> f32 {
        let [width, height] = self.size;
        width as f32 / height as f32
    }
}

/// `id`'s low and high halves, as the view's IDs hold it: shaders have no 64-bit integers.
fn halves(id: u64) -> [u32; 2] {
    [id as u32, (id >> 32) as u32]
}

/// What a frame drew in the game's view, which [`Renderer::drawn`] tells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    /// Instanced draws: one for each run of consecutive draws of one mesh with one shader that draw
    /// the same faces.
    pub draws: usize,
    /// The meshes they drew, one for each draw pushed to [`Draws`].
    pub instances: usize,
    /// The triangles of those meshes, culled ones among them: the GPU's work in drawing them.
    pub triangles: usize,
}

/// What's been added to a [`Meshes`], [`Shaders`] and [`Textures`] since it was last taken from
/// them, in order: the shaders a frame makes pipelines for, and the meshes and textures it copies
/// to the GPU, before it draws. Taking moves them out of the registries, which go on handing out
/// handles where they are, so that a renderer on another thread renders what they handed out
/// before. It keeps its memory, so that a frame that adds nothing allocates nothing.
#[derive(Default)]
pub struct Uploads {
    pub(crate) meshes: Vec<mesh::Added>,
    pub(crate) shaders: Vec<shader::Added>,
    pub(crate) textures: Vec<texture::Added>,
}

impl Uploads {
    /// Takes what's been added to `meshes`, `shaders` and `textures` since they were last taken
    /// from, after what it holds already.
    pub fn take(&mut self, meshes: &mut Meshes, shaders: &mut Shaders, textures: &mut Textures) {
        self.meshes.append(&mut meshes.added);
        self.shaders.append(&mut shaders.added);
        self.textures.append(&mut textures.added);
    }
}

/// What a frame draws besides UI: plain data, which the engine extracts every frame.
pub struct RenderData<'a> {
    /// The color the game's view clears to before the meshes are drawn, and the frame around it,
    /// in linear RGBA.
    pub clear_color: [f32; 4],
    /// Where the game's view shows in the frame. The meshes are drawn into an image of the view's
    /// own, its size, which the frame then shows there, under the UI; or, if the main window's UI
    /// paints a [`ViewCallback`], where that is instead.
    pub viewport: Viewport,
    /// What the view sees the world from.
    pub viewpoint: Viewpoint,
    /// The meshes, shaders and textures added since a frame last took them, which it takes first:
    /// the shaders get their pipelines, and the meshes and textures reach the GPU.
    pub uploads: &'a mut Uploads,
    /// What to draw.
    pub draws: &'a Draws,
    /// What the frame asks of the view, at most [`MAX_PICKS`], which [`Renderer::picked`] answers,
    /// as the frame two after the one that reads them begins: the frame reads them as it renders
    /// the view, after those a frame with nothing to render into left, up to [`MAX_PICKS`] in all,
    /// and the frames after it the rest.
    pub picks: &'a [Pick],
    /// What the frame outlines in the view, after reading the picks back, if anything.
    pub outline: Option<Outline>,
}

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
    /// Upload meshes and textures this way rather than the GPU's best, for tests and
    /// measurements of each way. A way the GPU lacks keeps the renderer from starting
    /// (`gpu.no_upload_path`).
    pub uploads: Option<UploadPath>,
    /// Make every pipeline through a pipeline cache, which holds this if it's what
    /// [`Renderer::pipeline_cache`] gave on the same GPU and driver: the pipelines it holds are
    /// made in a fraction of the time. Other data starts it empty, as `Device::load_pipeline_cache`
    /// says. `None` makes them without one.
    pub pipeline_cache: Option<&'a [u8]>,
    /// Capture this frame through RenderDoc, counting the frames the renderer renders from 1,
    /// and log the capture's path. RenderDoc's layer must be active on the GPU, as in an app run
    /// through `renderdoccmd capture`, or the renderer doesn't start (`renderer.no_renderdoc`).
    pub capture: Option<NonZeroU64>,
}

/// Renders frames into a window or, headless, into an offscreen image, and UI into the app's
/// other windows.
pub struct Renderer {
    frames: Frames,
    output: Output,
    /// The keys of the other windows, in the order of their swapchains in `windows`.
    window_keys: Vec<u64>,
    windows: Vec<Swapchain>,
    meshes: MeshPass,
    view: ViewPass,
    /// What the game's view renders into, the IDs its draws write beside it, and the depth image
    /// they test, the size it last had, which has an area.
    view_image: Option<(ColorImage, IdImage, DepthImage)>,
    /// Whether the last frame rendered the view, which [`Renderer::read_back_view`] reads back.
    viewed: bool,
    /// The picks asked of the view, from asked to answered.
    picks: Picks,
    ui: UiPass,
    instance: Arc<Instance>,
    device: Arc<Device>,
    vsync: bool,
    /// The frame to capture, as [`Settings::capture`] says.
    capture: Option<NonZeroU64>,
    /// Set once rendering fails, which can leave the GPU's copies of meshes and textures
    /// unfinished.
    failed: bool,
    /// How long the last frame waited, which [`Renderer::waited`] describes.
    waited: Duration,
    /// What the last frame drew, which [`Renderer::drawn`] describes.
    drawn: DrawCounts,
}

enum Output {
    Window(Swapchain),
    Offscreen(ColorImage),
}

// The engine renders on a thread of its own, which the renderer and its uploads move to.
const _: () = {
    const fn send<T: Send>() {}
    send::<Renderer>();
    send::<Uploads>();
};

impl Renderer {
    /// A renderer that presents to `window`, whose drawable area is `width` by `height` pixels.
    pub fn windowed(
        settings: &Settings<'_>,
        window: Arc<dyn Window>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let _zone = gg_core::zone!();
        let instance = Instance::new(&InstanceDesc {
            app_name: settings.app_name,
            validation: settings.validation,
            window: Some(&*window),
        })?;
        let surface = Surface::new(&instance, window, "main window")?;
        let device = Device::new(&instance, settings.gpu, Some(&surface), settings.uploads)?;
        let swapchain = Swapchain::new(&device, surface, width, height, settings.vsync)?;
        Self::new(&instance, &device, Output::Window(swapchain), settings)
    }

    /// A renderer without a window, which renders into an offscreen image of `width` by
    /// `height` pixels.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn headless(settings: &Settings<'_>, width: u32, height: u32) -> Result<Self> {
        let _zone = gg_core::zone!();
        let instance = Instance::new(&InstanceDesc {
            app_name: settings.app_name,
            validation: settings.validation,
            window: None,
        })?;
        let device = Device::new(&instance, settings.gpu, None, settings.uploads)?;
        let image = ColorImage::new(&device, width, height, "offscreen color")?;
        Self::new(&instance, &device, Output::Offscreen(image), settings)
    }

    fn new(
        instance: &Arc<Instance>,
        device: &Arc<Device>,
        output: Output,
        settings: &Settings<'_>,
    ) -> Result<Self> {
        if let Some(frame) = settings.capture
            && !device.can_capture()
        {
            let message = format!(
                "frame {frame} is to be captured, but RenderDoc's layer isn't active on device \
                 {:?}; run the app through `renderdoccmd capture`",
                device.name()
            );
            return Err(Error::new("renderer.no_renderdoc", message));
        }
        // Before the passes make their pipelines.
        if let Some(data) = settings.pipeline_cache {
            device.load_pipeline_cache(data)?;
        }
        let format = match &output {
            Output::Window(swapchain) => swapchain.format(),
            Output::Offscreen(_) => ColorImage::FORMAT,
        };
        Ok(Self {
            frames: Frames::new(device)?,
            output,
            window_keys: Vec::new(),
            windows: Vec::new(),
            meshes: MeshPass::new(device),
            view: ViewPass::new(device, format)?,
            view_image: None,
            viewed: false,
            picks: Picks::default(),
            ui: UiPass::new(device, format)?,
            instance: instance.clone(),
            device: device.clone(),
            vsync: settings.vsync,
            capture: settings.capture,
            failed: false,
            waited: Duration::ZERO,
            drawn: DrawCounts::default(),
        })
    }

    /// Tells the renderer that the window's drawable area changed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Output::Window(swapchain) = &mut self.output {
            swapchain.resize(width, height);
        }
    }

    /// Starts presenting to another of the app's windows, which `key` names in
    /// [`Renderer::render`], and whose drawable area is `width` by `height` pixels; its Vulkan
    /// objects are named after its `title`. A headless renderer can't, since its GPU was opened
    /// without a window.
    ///
    /// # Panics
    ///
    /// If a window named `key` is already open.
    pub fn open_window(
        &mut self,
        key: u64,
        window: Arc<dyn Window>,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<()> {
        assert!(
            !self.window_keys.contains(&key),
            "window {key} is already open"
        );
        let surface = Surface::new(&self.instance, window, title)?;
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

    /// Renders a frame from `data`, with `ui` drawn over it, and draws into each of the other
    /// windows its UI from `windows`, over black. It presents them all. `windows` lists the UIs
    /// in the order egui ran their passes, which ran before `ui`'s: the order of their texture
    /// changes. A window that isn't open, or whose swapchain has no image free yet, is left as it
    /// was.
    ///
    /// It reads back the IDs at the pixels `data.picks` asks for, and answers, in
    /// [`Renderer::picked`], those that the frame two before it read; then outlines in the view
    /// what `data.outline` says, from the view's IDs this frame.
    ///
    /// Returns `false` when there was nothing to render into, as when the window is minimized;
    /// the meshes and textures stay in `data.uploads`, and the UIs' texture changes and the picks
    /// wait, for the next frame that renders.
    ///
    /// An error ends the renderer, which can then only be dropped: a frame that failed can leave
    /// the GPU's copies of meshes and textures unfinished, so every later call fails with
    /// `renderer.failed`. A shader replaced since, whose new pipeline can't be made, is no error:
    /// it warns, and the shader goes on drawing as it did.
    ///
    /// The frame [`Settings::capture`] names is captured through RenderDoc, which logs `Frame
    /// captured` with the capture's path; if it has nothing to render into, the next frame that
    /// renders is captured instead. A capture that fails to start, as while another is under way
    /// (`gpu.capture_busy`), or that RenderDoc doesn't make (`gpu.capture_failed`), is an error that
    /// leaves the renderer as it was.
    ///
    /// # Panics
    ///
    /// If `data.uploads` holds meshes, shaders or textures taken from another [`Meshes`],
    /// [`Shaders`] or [`Textures`] than those it held before, which could draw a mesh with a shader
    /// for another vertex type or parameters of another type; if `data.draws` draws a mesh, or
    /// with a shader, that no upload so far has brought; if its viewpoint's projection can't
    /// project, as [`Projection`] says; or if it asks more than [`MAX_PICKS`] picks.
    pub fn render(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        self.waited = Duration::ZERO;
        self.drawn = DrawCounts::default();
        self.viewed = false;
        self.picks.ask(data.picks);
        if self.failed {
            return Err(Error::new(
                "renderer.failed",
                "a frame failed before, which can leave meshes and textures unfinished on the \
                 GPU; the renderer can only be dropped",
            ));
        }
        // The frame about to begin; one with nothing to render into begins none.
        let frame = self.frames.submitted() + 1;
        // Through another handle on the device, so that the capture doesn't borrow the renderer.
        let device =
            (self.capture.map(NonZeroU64::get) == Some(frame)).then(|| self.device.clone());
        let capture = device.as_deref().map(Device::start_capture).transpose()?;
        let rendered = self.render_frame(data, ui, windows);
        self.failed = rendered.is_err();
        // A frame that fails or renders nothing discards the capture, as it drops.
        let rendered = rendered?;
        if rendered && let Some(capture) = capture {
            let path = capture.end()?;
            info!(frame, path = %path.display(), "Frame captured");
        }
        Ok(rendered)
    }

    fn render_frame(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        let _zone = gg_core::zone!();
        // egui sends each texture change once, so changes wait for a frame that renders.
        for (_, window) in windows.iter_mut() {
            self.ui.queue(window.textures);
        }
        self.ui.queue(ui.textures);
        // Before the frame begins, which a failure would then present unrendered.
        self.meshes.add_shaders(data.uploads)?;
        // Every frame, whether it renders or draws anything, so that a viewpoint that can't
        // project always panics. A view without an area draws nothing, so its aspect ratio doesn't
        // matter.
        let [width, height] = data.viewport.size;
        let view_projection = data.viewpoint.view_projection(data.viewport.aspect());
        // The view renders into an image of its own size, with IDs and depth beside it, made again
        // in the frame that changes its size, so that it never shows stretched; the old ones retire
        // until the frames that use them have finished. A drag that resizes the view makes them
        // every frame, which growing them in steps, and rendering into part of them, would not. A
        // view without an area renders nothing, and keeps the images it had.
        let has_view = width > 0 && height > 0;
        if has_view
            && (self.view_image.as_ref())
                .is_none_or(|(image, ..)| image.size() != data.viewport.size)
        {
            let image = ColorImage::new(&self.device, width, height, "view")?;
            let ids = IdImage::new(&self.device, width, height, "view ids")?;
            let depth = DepthImage::new(&self.device, width, height, "view depth")?;
            self.view_image = Some((image, ids, depth));
            trace!(width, height, "View image made");
        }
        let target = match &mut self.output {
            Output::Window(swapchain) => Target::Swapchain(swapchain),
            Output::Offscreen(image) => Target::Image(image),
        };
        let has_ui = |key: &u64| windows.iter().any(|(drawn, _)| drawn == key);
        let others = (self.window_keys.iter().zip(&mut self.windows))
            .filter(|(key, _)| has_ui(key))
            .map(|(_, swapchain)| Target::Swapchain(swapchain));
        // After the output and the other windows, which keep their places.
        let view_target = 1 + self.window_keys.iter().filter(|key| has_ui(key)).count();
        let (view, ids, depth) = match &mut self.view_image {
            Some((image, ids, depth)) if has_view => {
                (Some(Target::Image(image)), Some(ids), Some(&*depth))
            }
            _ => (None, None, None),
        };
        let targets = iter::once(target).chain(others).chain(view);
        let Some(mut frame) = self.frames.begin(targets)? else {
            return Ok(false);
        };
        // The frame two before has finished, and what it read with it.
        self.picks.answer(frame.ids_read());
        self.meshes.upload(&mut frame, data.uploads)?;
        self.ui.write_textures(&mut frame)?;
        let (counts, view) = match ids {
            Some(ids) => {
                let clear = data.clear_color;
                let mut pass = frame
                    .begin_pass(view_target, c"view", clear, depth, Some(&mut *ids))
                    .expect("an offscreen image is always there");
                let size = data.viewport.size;
                let counts = self
                    .meshes
                    .draw(&mut pass, view_projection, size, data.draws)?;
                drop(pass);
                let pixels = self.picks.take(frame.number(), Some(ids.size()));
                frame.read_ids(ids, pixels)?;
                let outline = (data.outline.filter(Outline::shows))
                    .map(|outline| (frame.begin_sampling_ids(ids), outline));
                let image = frame.begin_sampling(view_target);
                (counts, Some(Shown { image, outline }))
            }
            None => {
                self.picks.take(frame.number(), None);
                (DrawCounts::default(), None)
            }
        };
        let mut pass = frame
            .begin_pass(0, c"frame", data.clear_color, None, None)
            .expect("a frame's first target has an image");
        // Under the whole UI, unless it paints the view among its own shapes.
        if let Some(shown) = view
            && !ui::draws_view(ui.primitives)
        {
            self.view.draw(&mut pass, data.viewport, shown, None);
        }
        let view = view.map(|shown| ui::View {
            pass: &self.view,
            shown,
            size: data.viewport.size,
        });
        self.ui
            .draw(&mut pass, ui.primitives, ui.pixels_per_point, view)?;
        drop(pass);
        let drawn = self
            .window_keys
            .iter()
            .filter_map(|key| windows.iter().find(|(drawn, _)| drawn == key));
        for (target, (_, ui)) in (1..).zip(drawn) {
            let black = [0.0, 0.0, 0.0, 1.0];
            // The view shows in the main window alone.
            if let Some(mut pass) = frame.begin_pass(target, c"window", black, None, None) {
                self.ui
                    .draw(&mut pass, ui.primitives, ui.pixels_per_point, None)?;
            }
        }
        self.ui.free_textures();
        frame.submit()?;
        self.waited = self.frames.waited();
        self.drawn = counts;
        self.viewed = has_view;
        Ok(true)
    }

    /// How long the last [`Renderer::render`] waited, if it rendered a frame: for the GPU to
    /// finish an earlier frame, and for the display to let go of an image. Zero if it didn't.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// What the last [`Renderer::render`] drew in the game's view, if it rendered a frame. Zero if
    /// it didn't, or if the view had no area. The UI's draws aren't counted.
    pub fn drawn(&self) -> DrawCounts {
        self.drawn
    }

    /// The answers to picks that came in during the last [`Renderer::render`]: each pick's tag,
    /// with the ID of the draw on top at its pixel, as [`Draws`] gives it, in the frame that read
    /// it, or 0 where none drew. That's the frame that asked it, or for a pick past the 64 a frame
    /// reads, or asked while frames had nothing to render into, the next that could read it. A
    /// pick of a pixel outside the view, as the frame that would read it renders it, is answered 0
    /// there; the rest, as the frame two after the one that read them begins, once the GPU has
    /// rendered that one.
    pub fn picked(&self) -> &[(u64, u64)] {
        self.picks.answered()
    }

    /// The GPU's name, such as `NVIDIA GeForce RTX 4090`.
    pub fn gpu_name(&self) -> &str {
        self.device.name()
    }

    /// How the renderer uploads meshes and textures.
    pub fn upload_path(&self) -> UploadPath {
        self.device.upload_path()
    }

    /// What the GPU's pipeline cache holds, for a later run's [`Settings::pipeline_cache`]: `None`
    /// for a renderer made without one, and once it has failed, since a device that's lost or out
    /// of memory may hand back what no driver should be given.
    pub fn pipeline_cache(&self) -> Result<Option<Vec<u8>>> {
        if self.failed {
            return Ok(None);
        }
        self.device.pipeline_cache()
    }

    /// Copies the last frame rendered offscreen to the CPU: rows of RGBA8 pixels, sRGB-encoded
    /// and tightly packed, top row first. `None` for a renderer with a window.
    pub fn read_back(&mut self) -> Result<Option<Vec<u8>>> {
        match &mut self.output {
            Output::Window(_) => Ok(None),
            Output::Offscreen(image) => image.read_back().map(Some),
        }
    }

    /// Copies the game's view as the last [`Renderer::render`] rendered it to the CPU, waiting for
    /// the GPU to finish that frame, which holds back the next: its width and height, and rows of
    /// RGBA8 pixels, sRGB-encoded and tightly packed, top row first, without the UI drawn over it.
    /// `None` if that call rendered no view, as when the view had no area or there was nothing to
    /// render into.
    pub fn read_back_view(&mut self) -> Result<Option<([u32; 2], Vec<u8>)>> {
        match &mut self.view_image {
            Some((image, ..)) if self.viewed => {
                let size = image.size();
                image.read_back().map(|pixels| Some((size, pixels)))
            }
            _ => Ok(None),
        }
    }

    /// What validation reports. It outlives the renderer, so it also counts what's reported
    /// while the renderer shuts down.
    pub fn validation(&self) -> Validation {
        self.instance.validation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_games_view_is_the_central_area_in_pixels() {
        let central =
            epaint::Rect::from_min_max(epaint::pos2(10.2, 31.0), epaint::pos2(400.6, 300.0));
        assert_eq!(
            Viewport::from_points(central, 1.25),
            Viewport {
                origin: [13, 39],
                size: [488, 336],
            }
        );
    }
}
