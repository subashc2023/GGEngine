//! Turns the render data the engine hands it each frame into images, on `gg-gpu`. It knows nothing
//! of the scene, ECS or gameplay: it takes plain data, so rendering can run on its own thread, a
//! frame behind the simulation.
//!
//! What it draws, in the game's view as its camera sees it:
//!
//! - Meshes, sprites, circles, lines, grids and text, with games' shaders and textures and the
//!   engine's.
//! - Emitters' particles, born on the GPU into a pool of birth records that age alone carries on
//!   from.
//! - Models, by glTF's metallic-roughness materials or KHR_materials_pbrSpecularGlossiness's, under
//!   the frame's lights.
//! - A directional light's shadows, in cascades fitted to the view.
//! - A sky behind it all, an image or an atmosphere made from its sun, prefiltered on the GPU. The
//!   atmosphere hazes what's far.
//! - Ambient occlusion from the view's depth, and reflections traced through it, in place of
//!   specular light.
//! - Opaque and masked meshes by their meshlets, shaded from a visibility buffer.
//! - Height fog, a game's UI in the view's space, and the UI over the view.
//!
//! The view is drawn in HDR, its exposure adapting to the light it meters. It's antialiased against
//! the frames before it or by SMAA, blurred by depth of field and motion, bloomed, graded, put
//! through a lens's vignette, chromatic aberration and grain, and tonemapped.
//!
//! Picks read back the ID of the draw at a pixel. An outline rings what the view shows of a set of
//! IDs. Each frame's passes are a render graph, which places their barriers and makes the view's
//! images and the shadow maps.

mod ao;
mod atmosphere;
mod bloom;
mod circle;
mod cluster;
mod data;
mod depth_of_field;
mod draws;
mod exposure;
mod fog;
mod frame;
mod grade;
mod grid;
mod lens;
mod light;
mod line;
mod mesh;
mod model;
mod motion_blur;
mod packed;
mod particles;
mod pick;
mod reflections;
mod shader;
mod shadow;
mod show;
mod skin;
mod sky;
mod smaa;
mod sprite;
mod taa;
mod text;
mod texture;
mod tonemap;
mod ui;
mod view;
mod viewpoint;

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use gg_core::math::DVec3;
use gg_core::{Error, Result};
use gg_gpu::{
    ColorImage, Device, Frames, Graph, Instance, InstanceDesc, PassId, Surface, Swapchain,
    Texture as GpuTexture,
};
use tracing::info;

pub use ao::AmbientOcclusion;
pub use atmosphere::Atmosphere;
pub use circle::Circle;
pub use data::{
    DEFAULT_UPLOAD_BUDGET, DrawCounts, Pixels, RenderData, Settings, Streaming, Uploads, Viewport,
};
pub use depth_of_field::DepthOfField;
pub use draws::Draws;
pub use exposure::{Adaptation, light_scale};
pub use fog::Fog;
pub use gg_gpu::{
    Arenas, Blend, ImageKind, ImageRead, MAX_TEXTURE_SIZE, MemoryBudget, Sampler, UploadPath,
    Validation, Window, Wrap, mip_sizes,
};
pub use grade::Grade;
pub use grid::{Grid, GridLines};
pub use lens::Lens;
pub use light::{Light, LightKind};
pub use line::Line;
pub use mesh::{Lod, Lods, MAX_LEVELS, Mesh, MeshDraw, Meshes, Scene};
#[doc(hidden)]
pub use model::Params as ModelParams;
pub use model::{AlphaMode, Material, MaterialId, Materials, ModelVertex, Sampled, Workflow};
pub use motion_blur::MotionBlur;
pub use packed::{
    MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Meshlet, Meshlets, ModelMesh, PackedVertex,
    Packing, SkinVertex,
};
pub use particles::{Births, Bursts, Emission, MAX_PARTICLES, ParticleShape, Particles};
pub use pick::{MAX_PICKS, Pick};
pub use reflections::Reflections;
pub use shader::{Compiled, Shader, ShaderSource, Shaders, compile_shader};
pub use show::{Channel, FrameImage, Show};
pub use sky::Sky;
pub use sprite::{Sprite, SpriteSheet};
pub use taa::Antialiasing;
pub use text::Glyph;
pub use texture::{Map, Mipmaps, Texels, Texture, Textures};
#[doc(hidden)]
pub use texture::{add_levels, add_map_mipmaps, add_mipmaps, widen_by_normals};
pub use tonemap::Tonemapper;
pub use ui::Ui;
pub use view::{Outline, ViewCallback};
pub use viewpoint::{Projection, Viewpoint};

use ao::AoPass;
use atmosphere::AtmospherePass;
use bloom::BloomPass;
use cluster::Lighting;
use depth_of_field::DepthOfFieldPass;
use exposure::{Adapting, ExposurePass};
use fog::FogPass;
use lens::LensPass;
use mesh::MeshPass;
use motion_blur::MotionBlurPass;
use particles::ParticlePass;
use pick::Picks;
use reflections::ReflectionPass;
use show::ShowPass;
use skin::Skinning;
use sky::{SkyLight, SkyPass};
use smaa::SmaaPass;
use taa::TaaPass;
use tonemap::TonemapPass;
use ui::UiPass;
use view::{OutlinePass, ViewPass};

/// A headless renderer's image's debug name.
const OFFSCREEN: &str = "offscreen color";

/// `id` as the view's IDs hold it, low half first: shaders have no 64-bit integers.
fn halves(id: u64) -> [u32; 2] {
    [id as u32, (id >> 32) as u32]
}

/// Renders frames into a window or an offscreen image, and UI into the app's other windows.
pub struct Renderer {
    frames: Frames,
    output: Output,
    /// The last frame's [`RenderData::origin`]. What's kept across frames is relative to it.
    origin: DVec3,
    /// The other windows' keys, in `windows`' order.
    window_keys: Vec<u64>,
    windows: Vec<Swapchain>,
    meshes: MeshPass,
    view: ViewPass,
    bloom: BloomPass,
    tonemap: TonemapPass,
    outline: OutlinePass,
    fog: FogPass,
    /// `history` is apart, as frames take its images as targets while recording.
    taa: TaaPass,
    history: taa::History,
    /// Made by the first frame that uses SMAA.
    smaa: Option<SmaaPass>,
    /// Made by the first frame that shows an image. `missing` is the last name it couldn't find,
    /// which it warned of.
    show: Option<ShowPass>,
    missing: Option<String>,
    /// What [`Renderer::images`] gives.
    images: Vec<FrameImage>,
    /// Made by the first frame with a sky.
    sky: Option<SkyPass>,
    /// Apart from the sky's pipelines, as frames take its images as targets while recording.
    sky_light: SkyLight,
    /// Made by the first frame with an atmosphere.
    atmosphere: Option<AtmospherePass>,
    /// The frame's lights, an atmosphere's sun dimmed by the air. Kept across frames.
    sunlit: Vec<Light>,
    /// Declared afresh each frame, in the last's memory.
    graph: Graph,
    /// The UI passes into the other windows, in order.
    window_passes: Vec<PassId>,
    /// The texture the view's pixels go into while it shows [`RenderData::pixels`], and whether a
    /// frame has written it.
    pixels: Option<(GpuTexture, bool)>,
    /// The last frame's number if it read the view back, else 0: [`Renderer::view_read`]'s frame.
    view_read: u64,
    /// From asked to answered.
    picks: Picks,
    ui: UiPass,
    instance: Arc<Instance>,
    device: Arc<Device>,
    vsync: bool,
    /// From [`Settings::visibility_buffer`].
    visibility_buffer: bool,
    /// From [`Settings::capture`].
    capture: Option<NonZeroU64>,
    /// Set once rendering fails, which can leave the GPU's copies of meshes and textures
    /// unfinished.
    failed: bool,
    /// What [`Renderer::waited`] gives.
    waited: Duration,
    /// What [`Renderer::drawn`] gives.
    drawn: DrawCounts,
    /// Per frame in flight, by number: what it drew of the draws it didn't cull, and whether it
    /// culled any. The frame two later adds the culling's counts as they come back.
    counted: [(u64, DrawCounts, bool); 2],
    /// The frame's lights as shaders read them, and their binning into the view's clusters.
    lighting: Lighting,
    skinning: Skinning,
    /// Writes emitters' births before the frame draws, and draws the particles.
    particles: ParticlePass,
    /// `ao_history`, averaging the AO over frames, is apart, as frames take its images as targets
    /// while recording.
    ao: AoPass,
    ao_history: ao::History,
    /// `reflected`, the images of the view's colors it traces into, is apart, as frames take them
    /// as targets while recording.
    reflections: ReflectionPass,
    reflected: reflections::Images,
    /// Meters the view. `adapting` holds the game's view's exposure and a tool's look's, adapted
    /// apart ([`RenderData::looked`]). `exposed` is what lit shading multiplied light by in the
    /// last frame, 0 before the first.
    exposure: ExposurePass,
    adapting: [Adapting; 2],
    exposed: f32,
    depth_of_field: DepthOfFieldPass,
    motion_blur: MotionBlurPass,
    lens: LensPass,
}

enum Output {
    Window(Swapchain),
    /// The image, and the size it's to have: the next frame remakes it if they differ.
    Offscreen(ColorImage, [u32; 2]),
}

// The renderer and its uploads move to the engine's render thread.
const _: () = {
    const fn send<T: Send>() {}
    send::<Renderer>();
    send::<Uploads>();
};

impl Renderer {
    /// A renderer presenting to `window`, of `width` by `height` drawable pixels.
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
        let device = Device::new(
            &instance,
            settings.gpu,
            Some(&surface),
            settings.uploads,
            settings.mesh_shaders,
            None,
        )?;
        let swapchain = Swapchain::new(&device, surface, width, height, settings.vsync)?;
        Self::new(&instance, &device, Output::Window(swapchain), settings)
    }

    /// A renderer into an offscreen image of `width` by `height` pixels.
    ///
    /// # Panics
    ///
    /// - If a side is 0.
    pub fn headless(settings: &Settings<'_>, width: u32, height: u32) -> Result<Self> {
        let _zone = gg_core::zone!();
        let instance = Instance::new(&InstanceDesc {
            app_name: settings.app_name,
            validation: settings.validation,
            window: None,
        })?;
        Self::headless_on(&instance, settings, width, height)
    }

    /// [`Renderer::headless`] on `instance`, in place of the one `settings.app_name` and
    /// `settings.validation` would make. For the GPU tests, which share one.
    ///
    /// # Panics
    ///
    /// - If a side is 0.
    #[doc(hidden)]
    pub fn headless_on(
        instance: &Arc<Instance>,
        settings: &Settings<'_>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let device = Device::new(
            instance,
            settings.gpu,
            None,
            settings.uploads,
            settings.mesh_shaders,
            None,
        )?;
        let image = ColorImage::new(&device, width, height, OFFSCREEN)?;
        let output = Output::Offscreen(image, [width, height]);
        Self::new(instance, &device, output, settings)
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
            Output::Offscreen(..) => ColorImage::FORMAT,
        };
        Ok(Self {
            frames: Frames::new(device)?,
            output,
            origin: DVec3::ZERO,
            window_keys: Vec::new(),
            windows: Vec::new(),
            meshes: MeshPass::new(device, settings.upload_budget)?,
            view: ViewPass::new(device, format)?,
            bloom: BloomPass::new(device)?,
            tonemap: TonemapPass::new(device)?,
            outline: OutlinePass::new(device)?,
            fog: FogPass::new(device)?,
            taa: TaaPass::new(device)?,
            history: taa::History::default(),
            smaa: None,
            show: None,
            missing: None,
            images: Vec::new(),
            sky: None,
            sky_light: SkyLight::default(),
            atmosphere: None,
            sunlit: Vec::new(),
            graph: Graph::default(),
            window_passes: Vec::new(),
            pixels: None,
            view_read: 0,
            picks: Picks::default(),
            ui: UiPass::new(device, format)?,
            instance: instance.clone(),
            device: device.clone(),
            vsync: settings.vsync,
            visibility_buffer: settings.visibility_buffer,
            capture: settings.capture,
            failed: false,
            waited: Duration::ZERO,
            drawn: DrawCounts::default(),
            counted: [(0, DrawCounts::default(), false); 2],
            lighting: Lighting::new(device)?,
            skinning: Skinning::new(device)?,
            particles: ParticlePass::new(device)?,
            ao: AoPass::new(device)?,
            ao_history: ao::History::default(),
            reflections: ReflectionPass::new(device)?,
            reflected: reflections::Images::default(),
            exposure: ExposurePass::new(device)?,
            adapting: [Adapting::default(), Adapting::default()],
            exposed: 0.0,
            depth_of_field: DepthOfFieldPass::new(device)?,
            motion_blur: MotionBlurPass::new(device)?,
            lens: LensPass::new(device)?,
        })
    }

    /// Tells the renderer the window's drawable area changed size. Headless, the next frame renders
    /// into an offscreen image of that size.
    ///
    /// # Panics
    ///
    /// - Headless, if a side is 0.
    pub fn resize(&mut self, width: u32, height: u32) {
        match &mut self.output {
            Output::Window(swapchain) => swapchain.resize(width, height),
            Output::Offscreen(_, size) => {
                assert!(width > 0 && height > 0, "an offscreen image has an area");
                *size = [width, height];
            }
        }
    }

    /// Starts presenting to another of the app's windows, named `key` in [`Renderer::render`], of
    /// `width` by `height` drawable pixels. Its Vulkan objects are named after `title`. A headless
    /// renderer can't: its GPU was opened without a window.
    ///
    /// # Panics
    ///
    /// - If window `key` is already open.
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

    /// Stops presenting to window `key`, if it's open, once the GPU is done with it.
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

    /// Whether windows present at the display's refresh rate ([`Renderer::set_vsync`]).
    pub fn vsync(&self) -> bool {
        self.vsync
    }

    /// Presents every window at the display's refresh rate from the next frame with `vsync`, or
    /// as fast as frames render without, as [`Settings::vsync`] says. Headless, it does nothing.
    pub fn set_vsync(&mut self, vsync: bool) {
        self.vsync = vsync;
        if let Output::Window(swapchain) = &mut self.output {
            swapchain.set_vsync(vsync);
        }
        for window in &mut self.windows {
            window.set_vsync(vsync);
        }
    }

    /// Renders a frame from `data` with `ui` over it, draws each other window's UI from `windows`
    /// over black, and presents them all. `windows` is in the order egui ran their passes, before
    /// `ui`'s: the order of their texture changes. A window not open, or with no swapchain image
    /// free yet, is left as it was.
    ///
    /// It reads `data.picks`, unless `data.read_picks` is false, and answers picks as [`Pick`]
    /// says; then it outlines what `data.outline` says from this frame's IDs.
    ///
    /// Returns `false` when there was nothing to render into, as when the window is minimized. The
    /// meshes and textures then stay in `data.uploads`, and the UIs' texture changes and the picks
    /// wait for the next frame that renders.
    ///
    /// Streaming:
    ///
    /// - The frame streams the queued meshes and textures, `data.uploads`' after those before, to
    ///   the GPU in order, within [`Settings::upload_budget`].
    /// - Each draws from the frame it's all there on. Until then a mesh draws nothing, and a
    ///   texture samples as a white pixel, or zeros if its texels are data (text then draws
    ///   nothing). One replaced draws as before until its replacement is there.
    /// - Through a transfer queue no frame waits for the copies: what a frame streams draws in a
    ///   later one. [`Renderer::streaming`] tells what's still on the way.
    /// - An asset that would take GPU memory past its budget waits, warning `renderer.over_budget`
    ///   once, while those after it go on. When nothing else is being copied it goes ahead alone,
    ///   so a game comes up on a GPU already past its budget.
    /// - One whose buffers or image don't fit in GPU memory warns `renderer.out_of_memory` once and
    ///   waits, retried as room comes.
    /// - A mesh, texture or shader removed draws nothing, or samples as its stand-in, from the
    ///   frame that takes the removal on. One replaced while it streams drops that stream for its
    ///   replacement's.
    ///
    /// [`Settings::capture`]'s frame is captured through RenderDoc, which logs `Frame captured`
    /// with the capture's path. If that frame has nothing to render into, the next that renders is
    /// captured.
    ///
    /// # Errors
    ///
    /// - Any error ends the renderer, which can then only be dropped. A failed frame can leave the
    ///   GPU's copies of meshes and textures unfinished, so every later call fails with
    ///   `renderer.failed`.
    /// - A capture that fails to start, as while another is under way (`gpu.capture_busy`), or that
    ///   RenderDoc doesn't make (`gpu.capture_failed`). It leaves the renderer as it was.
    ///
    /// A replaced shader whose new pipeline can't be made is no error: it warns, and the shader
    /// draws as before.
    ///
    /// # Panics
    ///
    /// - If `data.uploads` holds meshes, shaders or textures taken from another [`Meshes`],
    ///   [`Shaders`] or [`Textures`] than before, which could draw a mesh with a shader of another
    ///   vertex or parameter type.
    /// - If `data.draws` draws a mesh, or with a shader, whose handle no upload has brought yet.
    /// - If its viewpoint's projection can't project, as [`Projection`] says.
    /// - If it asks more than [`MAX_PICKS`] picks.
    pub fn render(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        self.waited = Duration::ZERO;
        self.drawn = DrawCounts::default();
        self.view_read = 0;
        self.picks.ask(data.picks);
        if self.failed {
            return Err(Error::new(
                "renderer.failed",
                "a frame failed before, which can leave meshes and textures unfinished on the \
                 GPU; the renderer can only be dropped",
            ));
        }
        // The frame about to begin. One with nothing to render into begins none.
        let frame = self.frames.submitted() + 1;
        // Another handle on the device, so the capture doesn't borrow the renderer.
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

    /// How long the last [`Renderer::render`] waited for the GPU to finish an earlier frame and for
    /// the display to free an image. Zero if it rendered no frame.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// What the GPU drew in the game's view two frames before the last [`Renderer::render`] that
    /// rendered one, as its opaque runs' culling counted on the GPU. Zero if the last call rendered
    /// no frame, before then, and for a frame without the view or whose view had no area. Not
    /// counted: the UI's draws, and draws of meshes not yet drawable or removed, or with shaders
    /// removed.
    pub fn drawn(&self) -> DrawCounts {
        self.drawn
    }

    /// The GPU time of the frame two before the last [`Renderer::render`] that rendered one, from
    /// its first pass's start to its last's end. `None` before then, and when the graphics queue
    /// writes no timestamps. With gg-core's `tracy` feature, Tracy's GPU timeline shows each pass.
    pub fn gpu_time(&self) -> Option<Duration> {
        self.frames.gpu_time()
    }

    /// What lit shading multiplied light by in the last frame: [`RenderData::exposure`], or what
    /// [`RenderData::adaptation`] adapted it to. 0 before the first.
    pub fn exposure(&self) -> f32 {
        self.exposed
    }

    /// The GPU memory, in bytes, of the frames' own images, the view's among them. Images whose
    /// uses in a frame don't overlap share it.
    pub fn transient_bytes(&self) -> u64 {
        self.frames.transient_bytes()
    }

    /// What's still streaming after the last [`Renderer::render`]. Empty once every mesh and
    /// texture added is drawable or removed; a test renders until then.
    pub fn streaming(&self) -> Streaming {
        self.meshes.streaming()
    }

    /// Answers that came in during the last [`Renderer::render`]: each pick's tag, and the ID read,
    /// as [`Pick`] says.
    pub fn picked(&self) -> &[(u64, u64)] {
        self.picks.answered()
    }

    /// The images [`Show::Image`] finds, made by the last frame that [`RenderData::list_images`]
    /// asked: the graph's own before the show's pass, but the antialiasing's, which a frame showing
    /// an image skips. In the order made, each name once, as its last image, which a show finds.
    /// None for a frame with no view to draw; empty before such a frame.
    pub fn images(&self) -> &[FrameImage] {
        &self.images
    }

    /// The GPU's name, such as `NVIDIA GeForce RTX 4090`.
    pub fn gpu_name(&self) -> &str {
        self.device.name()
    }

    /// How the renderer uploads meshes and textures.
    pub fn upload_path(&self) -> UploadPath {
        self.device.upload_path()
    }

    /// GPU memory in use, and the driver's budget for the process, as
    /// [`gg_gpu::Device::memory_budget`] says.
    pub fn memory_budget(&self) -> MemoryBudget {
        self.device.memory_budget()
    }

    /// What the frames' arenas hold of the geometry, instances and UI copied to the GPU, and the
    /// most a frame has taken, as [`Arenas`] says.
    pub fn arenas(&self) -> Arenas {
        self.frames.arenas()
    }

    /// The GPU's pipeline cache, for a later run's [`Settings::pipeline_cache`]. `None` for a
    /// renderer made without one, and once it has failed: a lost or out-of-memory device may hand
    /// back what no driver should be given.
    pub fn pipeline_cache(&self) -> Result<Option<Vec<u8>>> {
        if self.failed {
            return Ok(None);
        }
        self.device.pipeline_cache()
    }

    /// Copies the last frame rendered offscreen to the CPU: tightly packed rows of sRGB-encoded
    /// RGBA8 pixels, top row first. `None` for a renderer with a window.
    pub fn read_back(&mut self) -> Result<Option<Vec<u8>>> {
        match &mut self.output {
            Output::Window(_) => Ok(None),
            Output::Offscreen(image, _) => image.read_back().map(Some),
        }
    }

    /// The game's view as the last [`Renderer::render`] read it back ([`RenderData::read_view`]),
    /// with its outline but not the UI. Waits for the GPU to finish that frame, holding back the
    /// next. `None` if that call read no view back: the view had no area or showed pixels, or there
    /// was nothing to render into.
    pub fn view_read(&self) -> Result<Option<ImageRead<'_>>> {
        if self.view_read == 0 {
            return Ok(None);
        }
        self.frames.image_read(self.view_read)
    }

    /// What validation reports. It outlives the renderer, so it also counts reports during
    /// shutdown.
    pub fn validation(&self) -> Validation {
        self.instance.validation()
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec4;

    use super::*;
    use crate::frame::canvas_projection;

    /// The canvas's corners are the view's, its bottom left at clip space's bottom left, whose Y
    /// points down, and every point at the near plane.
    #[test]
    fn the_canvas_spans_the_views_pixels_at_its_near_plane() {
        let clip = canvas_projection([400, 300]);
        let at = |x: f32, y: f32, z: f32| clip * Vec4::new(x, y, z, 1.0);
        assert_eq!(at(0.0, 0.0, 0.0), Vec4::new(-1.0, 1.0, 1.0, 1.0));
        assert_eq!(at(400.0, 300.0, -7.0), Vec4::new(1.0, -1.0, 1.0, 1.0));
        assert_eq!(at(100.0, 225.0, 3.0), Vec4::new(-0.5, -0.5, 1.0, 1.0));
    }

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
