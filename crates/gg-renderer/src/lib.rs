//! Turns the render data the engine hands it each frame into images, built on `gg-gpu`: meshes
//! and sprites drawn in the game's view, as its camera sees them, with games' shaders and textures
//! and the engine's own, models' unlit materials among them, and the UI over them; and the render
//! graph, lighting, PBR materials and post-processing to come.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

mod camera;
mod mesh;
mod model;
mod shader;
mod sprite;
mod texture;
mod ui;
mod view;

use std::iter;
use std::num::NonZeroU64;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_core::math::{Mat3, Mat4};
use gg_core::{Error, Result};
use gg_gpu::{
    ColorImage, DepthImage, Device, Faces, Frames, Instance, InstanceDesc, Surface, Swapchain,
    Target,
};
use tracing::{info, trace};

pub use camera::{Camera, Projection};
pub use gg_gpu::{Blend, MAX_TEXTURE_SIZE, Sampler, UploadPath, Validation, Window, Wrap};
pub use mesh::{Mesh, MeshDraw, Meshes};
pub use model::{AlphaMode, Material, ModelVertex};
pub use shader::{Compiled, Shader, ShaderSource, Shaders};
pub use sprite::{Sprite, SpriteSheet};
pub use texture::{Mipmaps, Texture, Textures};
pub use ui::Ui;

use mesh::MeshPass;
use ui::UiPass;
use view::ViewPass;

/// The `size` pixels of a frame whose top left is at `origin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

/// What a frame draws besides UI, in order, each over those before it, but where an opaque one
/// before it is nearer: meshes, each with a shader for its vertex type, the parameters that shader
/// takes, and where it is in the world. Each run of consecutive draws of one mesh with one shader
/// that draw the same [`Faces`] renders as one instanced draw, whatever their transforms and
/// parameters, a texture among them: every draw draws both faces of its mesh's triangles but a
/// material's that isn't double-sided. Clearing it keeps its memory, so that a frame that draws no
/// more than the last allocates nothing.
#[derive(Debug, Default, PartialEq)]
pub struct Draws {
    /// The runs, in order.
    runs: Vec<Run>,
    /// Every draw's transform, in order.
    transforms: Vec<Mat4>,
    /// Every draw's parameters, one after another.
    params: Vec<u8>,
}

/// Consecutive draws of one mesh with one shader that draw the same faces: the instances of one
/// instanced draw.
#[derive(Debug, PartialEq)]
struct Run {
    mesh: u32,
    shader: u32,
    faces: Faces,
    /// Where its draws' transforms are in `Draws::transforms`.
    instances: Range<usize>,
    /// Where their parameters are in `Draws::params`.
    params: Range<usize>,
}

impl Draws {
    /// Draws `mesh` with `shader`, which reads its vertex type and takes `params`, over the
    /// meshes drawn before it but where an opaque one is nearer. `transform` takes the mesh's own
    /// space, which its vertices' positions are in, to the world's.
    pub fn push<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.transforms.push(transform);
        self.params.extend_from_slice(bytemuck::bytes_of(&params));
        self.extend_run(mesh.index, shader.index, Faces::Both, instance, start);
    }

    /// Draws `draw` over the meshes drawn before it but where an opaque one is nearer, as
    /// [`push`](Self::push) draws its mesh with its shader and parameters. A material's draw that
    /// isn't double-sided draws its mesh's fronts: the triangles whose corners wind counterclockwise,
    /// as glTF's fronts do, or clockwise where `transform` mirrors them, as glTF has it.
    pub fn push_draw(&mut self, transform: Mat4, draw: &MeshDraw) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.transforms.push(transform);
        self.params.extend_from_slice(draw.params());
        let faces = if !draw.culls {
            Faces::Both
        } else if Mat3::from_mat4(transform).determinant() < 0.0 {
            Faces::Clockwise
        } else {
            Faces::CounterClockwise
        };
        self.extend_run(draw.mesh, draw.shader, faces, instance, start);
    }

    /// Draws `sprite` over the meshes drawn before it but where an opaque one is nearer, as
    /// [`Sprite`] describes. `transform` takes the sprite's own space, where it's a unit square
    /// around the origin, to the world's.
    pub fn push_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.push(sprite::QUAD, sprite::SHADER, transform, sprite.params());
    }

    /// Draws each of `sprites`, a transform and a sprite, in turn, as
    /// [`push_sprite`](Self::push_sprite) does, making room for them and joining a run once for
    /// them all.
    pub fn extend_sprites(&mut self, sprites: impl ExactSizeIterator<Item = (Mat4, Sprite)>) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.params
            .reserve(sprites.len() * size_of::<sprite::Params>());
        let params = &mut self.params;
        self.transforms.extend(sprites.map(|(transform, sprite)| {
            params.extend_from_slice(bytemuck::bytes_of(&sprite.params()));
            transform
        }));
        if self.transforms.len() > instance {
            let (quad, shader) = (sprite::QUAD.index, sprite::SHADER.index);
            self.extend_run(quad, shader, Faces::Both, instance, start);
        }
    }

    /// Adds the draws from `instance` on, whose parameters start at `start`, of `mesh` with
    /// `shader`, drawing `faces`, to the last run if it's of them too, or as a run of their own.
    fn extend_run(&mut self, mesh: u32, shader: u32, faces: Faces, instance: usize, start: usize) {
        match self.runs.last_mut() {
            Some(run) if (run.mesh, run.shader, run.faces) == (mesh, shader, faces) => {
                run.instances.end = self.transforms.len();
                run.params.end = self.params.len();
            }
            _ => self.runs.push(Run {
                mesh,
                shader,
                faces,
                instances: instance..self.transforms.len(),
                params: start..self.params.len(),
            }),
        }
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Forgets every draw.
    pub fn clear(&mut self) {
        self.runs.clear();
        self.transforms.clear();
        self.params.clear();
    }
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
    /// own, its size, which the frame then shows there.
    pub viewport: Viewport,
    /// What the view sees the world from.
    pub camera: Camera,
    /// The meshes, shaders and textures added since a frame last took them, which it takes first:
    /// the shaders get their pipelines, and the meshes and textures reach the GPU.
    pub uploads: &'a mut Uploads,
    /// What to draw.
    pub draws: &'a Draws,
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
    /// What the game's view renders into, and the depth image its draws test, the size it last
    /// had, which has an area.
    view_image: Option<(ColorImage, DepthImage)>,
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
        let surface = Surface::new(&instance, window)?;
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

    /// Renders a frame from `data`, with `ui` drawn over it, and draws into each of the other
    /// windows its UI from `windows`, over black. It presents them all. `windows` lists the UIs
    /// in the order egui ran their passes, which ran before `ui`'s: the order of their texture
    /// changes. A window that isn't open, or whose swapchain has no image free yet, is left as it
    /// was.
    ///
    /// Returns `false` when there was nothing to render into, as when the window is minimized;
    /// the meshes and textures stay in `data.uploads`, and the UIs' texture changes wait, for the
    /// next frame that renders.
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
    /// for another vertex type or parameters of another type; or if its camera's projection can't
    /// project, as [`Projection`] says.
    pub fn render(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        self.waited = Duration::ZERO;
        self.drawn = DrawCounts::default();
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
        // Every frame, whether it renders or draws anything, so that a camera that can't project
        // always panics. A view without an area draws nothing, so its aspect ratio doesn't matter.
        let [width, height] = data.viewport.size;
        let view_projection = data.camera.view_projection(width as f32 / height as f32);
        // The view renders into an image of its own size, made again in the frame that changes
        // its size, so that it never shows stretched; the old one retires until the frames that
        // use it have finished. A drag that resizes the view makes an image every frame, which
        // growing it in steps, and rendering into part of it, would not. A view without an area
        // renders nothing, and keeps the image it had.
        let has_view = width > 0 && height > 0;
        if has_view
            && (self.view_image.as_ref())
                .is_none_or(|(image, _)| image.size() != data.viewport.size)
        {
            let image = ColorImage::new(&self.device, width, height, "view")?;
            let depth = DepthImage::new(&self.device, width, height, "view depth")?;
            self.view_image = Some((image, depth));
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
        let (view, depth) = match &mut self.view_image {
            Some((image, depth)) if has_view => (Some(Target::Image(image)), Some(&*depth)),
            _ => (None, None),
        };
        let targets = iter::once(target).chain(others).chain(view);
        let Some(mut frame) = self.frames.begin(targets)? else {
            return Ok(false);
        };
        self.meshes.upload(&mut frame, data.uploads)?;
        self.ui.write_textures(&mut frame)?;
        let (counts, view) = if has_view {
            let mut pass = frame
                .begin_pass(view_target, c"view", data.clear_color, depth)
                .expect("an offscreen image is always there");
            let counts = self.meshes.draw(&mut pass, view_projection, data.draws)?;
            drop(pass);
            (counts, Some(frame.begin_sampling(view_target)))
        } else {
            (DrawCounts::default(), None)
        };
        let mut pass = frame
            .begin_pass(0, c"frame", data.clear_color, None)
            .expect("a frame's first target has an image");
        if let Some(image) = view {
            self.view.draw(&mut pass, data.viewport, image);
        }
        self.ui
            .draw(&mut pass, ui.primitives, ui.pixels_per_point)?;
        drop(pass);
        let drawn = self
            .window_keys
            .iter()
            .filter_map(|key| windows.iter().find(|(drawn, _)| drawn == key));
        for (target, (_, ui)) in (1..).zip(drawn) {
            let black = [0.0, 0.0, 0.0, 1.0];
            if let Some(mut pass) = frame.begin_pass(target, c"window", black, None) {
                self.ui
                    .draw(&mut pass, ui.primitives, ui.pixels_per_point)?;
            }
        }
        self.ui.free_textures();
        frame.submit()?;
        self.waited = self.frames.waited();
        self.drawn = counts;
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

    /// What validation reports. It outlives the renderer, so it also counts what's reported
    /// while the renderer shuts down.
    pub fn validation(&self) -> Validation {
        self.instance.validation()
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::{Vec3, Vec4};

    use super::*;

    /// Each run's mesh, shader and draws.
    fn runs(draws: &Draws) -> Vec<(u32, u32, Range<usize>)> {
        let run = |run: &Run| (run.mesh, run.shader, run.instances.clone());
        draws.runs.iter().map(run).collect()
    }

    #[test]
    fn consecutive_draws_of_one_mesh_with_one_shader_are_one_run() {
        let square = Mesh::<[f32; 3]>::new(1);
        let triangle = Mesh::<[f32; 3]>::new(2);
        let flat = Shader::<[f32; 3], Vec4>::new(1, true);
        let other = Shader::<[f32; 3], Vec4>::new(2, true);
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let (red, green, blue) = (Vec4::X, Vec4::Y, Vec4::Z);
        let mut draws = Draws::default();
        // Sprites of three textures, which share a run.
        for (x, index) in [0, 5, 6].into_iter().enumerate() {
            let texture = Texture { index };
            let sprite = Sprite {
                texture,
                ..Sprite::default()
            };
            draws.push_sprite(at(x as f32), sprite);
        }
        draws.push(square, flat, at(3.0), red);
        draws.push(square, flat, at(4.0), green);
        // Another mesh, then another shader.
        draws.push(triangle, flat, at(5.0), blue);
        draws.push(triangle, other, at(6.0), red);
        // Back to sprites, after the rest, then more of them at once, which join their run.
        draws.push_sprite(at(7.0), Sprite::default());
        let more = [at(8.0), at(9.0)].map(|at| (at, Sprite::default()));
        draws.extend_sprites(more.into_iter());
        // A draw without its types joins a run of its mesh and shader, as one with them does.
        draws.push(triangle, other, at(10.0), green);
        draws.push_draw(at(11.0), &MeshDraw::new(triangle, other, blue));
        draws.push(triangle, other, at(12.0), red);
        assert_eq!(
            runs(&draws),
            [
                (0, 0, 0..3),
                (1, 1, 3..5),
                (2, 1, 5..6),
                (2, 2, 6..7),
                (0, 0, 7..10),
                (2, 2, 10..13)
            ]
        );
        // A run's transforms and parameters are its own draws'.
        let squares = &draws.runs[1];
        assert_eq!(
            draws.transforms[squares.instances.clone()],
            [at(3.0), at(4.0)]
        );
        let colors = [red, green];
        let colors: &[u8] = bytemuck::cast_slice(&colors);
        assert_eq!(&draws.params[squares.params.clone()], colors);
        let sprites = draws.runs[0].params.clone();
        assert_eq!(sprites, 0..3 * size_of::<sprite::Params>());
        let triangles = &draws.runs[5];
        let colors = [green, blue, red];
        let colors: &[u8] = bytemuck::cast_slice(&colors);
        assert_eq!(&draws.params[triangles.params.clone()], colors);
        let sprites = draws.runs[4].params.clone();
        assert_eq!(sprites.len(), 3 * size_of::<sprite::Params>());

        draws.clear();
        assert!(draws.is_empty());
        // No sprites make no run.
        draws.extend_sprites(iter::empty());
        assert!(draws.is_empty());
        draws.push(square, flat, at(0.0), red);
        assert_eq!(runs(&draws), [(1, 1, 0..1)]);
        assert_eq!(draws.params.len(), size_of::<Vec4>());
    }

    #[test]
    fn draws_that_cull_run_apart_where_their_transforms_mirror_them() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let corner = ModelVertex {
            position: Vec3::ZERO,
            uv: gg_core::math::Vec2::ZERO,
            color: Vec4::ONE,
        };
        let mesh = meshes.add("a", &[corner; 3], &[0, 1, 2]).unwrap();
        let mut draw = |double_sided| {
            let material = Material {
                double_sided,
                ..Material::default()
            };
            MeshDraw::with_material(mesh, &material, &mut shaders)
        };
        let (single, double) = (draw(false), draw(true));
        let at = Mat4::from_translation(Vec3::X);
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 2.0, 1.0));
        let faces = |draws: &Draws| draws.runs.iter().map(|run| run.faces).collect::<Vec<_>>();
        let mut draws = Draws::default();
        for transform in [at, mirrored, at] {
            draws.push_draw(transform, &single);
        }
        let expected = [
            Faces::CounterClockwise,
            Faces::Clockwise,
            Faces::CounterClockwise,
        ];
        assert_eq!(faces(&draws), expected);
        // Mirroring twice turns it back.
        draws.clear();
        draws.push_draw(mirrored * mirrored, &single);
        assert_eq!(faces(&draws), [Faces::CounterClockwise]);
        // Draws that draw both faces, and sprites, never split for it.
        draws.clear();
        for transform in [at, mirrored, at] {
            draws.push_draw(transform, &double);
        }
        for transform in [at, mirrored, at] {
            draws.push_sprite(transform, Sprite::default());
        }
        assert_eq!(faces(&draws), [Faces::Both, Faces::Both]);
        assert_eq!(runs(&draws), [(1, 1, 0..3), (0, 0, 3..6)]);
    }
}
