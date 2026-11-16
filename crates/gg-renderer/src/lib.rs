//! Turns the render data the engine hands it each frame into images, built on `gg-gpu`: meshes,
//! sprites, circles, lines, grids, text and emitters' particles, born on the GPU into a pool of
//! birth records that their age alone carries on from, drawn in the game's view, as its camera
//! sees them, with
//! games' shaders and textures and the engine's own, models' among them, shaded by glTF's
//! metallic-roughness materials, or KHR_materials_pbrSpecularGlossiness's, under the frame's
//! lights, a directional light's shadows, in cascades fitted to the view, and its sky, an image or
//! an atmosphere made from its sun, which shows behind them, prefiltered on the GPU, its light
//! darkened by ambient occlusion from the view's depth, and its specular light given way to the
//! reflections of what the view shows, traced through that depth, the atmosphere hazing what's
//! far, opaque and masked ones drawn by their meshlets shaded from
//! a visibility buffer, all in the frame's height fog, if it has one, a game's UI over them in the
//! view's own space, and the UI over the view;
//! the view drawn in colors of high dynamic range, its exposure adapting to the light it meters,
//! antialiased against the frames before it or by SMAA,
//! blurred by its depth of field and its motion, bloomed, graded, seen through a lens's vignette,
//! chromatic aberration and grain, and mapped by a tonemapper into the range it shows; picks,
//! which read back the ID of the draw the view shows at a pixel; an outline round what the view
//! shows of a set of IDs;
//! and each frame's passes in a render graph, which places the barriers between them and makes the
//! view's images and the shadows' maps.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

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
pub use texture::{add_levels, add_map_mipmaps, add_mipmaps};
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

/// The debug name of a headless renderer's image.
const OFFSCREEN: &str = "offscreen color";

/// `id`'s low and high halves, as the view's IDs hold it: shaders have no 64-bit integers.
fn halves(id: u64) -> [u32; 2] {
    [id as u32, (id >> 32) as u32]
}

/// Renders frames into a window or, headless, into an offscreen image, and UI into the app's
/// other windows.
pub struct Renderer {
    frames: Frames,
    output: Output,
    /// The last frame's [`RenderData::origin`], which what's kept from frame to frame is in the
    /// space of.
    origin: DVec3,
    /// The keys of the other windows, in the order of their swapchains in `windows`.
    window_keys: Vec<u64>,
    windows: Vec<Swapchain>,
    meshes: MeshPass,
    view: ViewPass,
    bloom: BloomPass,
    tonemap: TonemapPass,
    outline: OutlinePass,
    /// What fogs what's opaque in the view, if it's in fog.
    fog: FogPass,
    /// What resolves the view, antialiased, against the frames before, and what it resolves into
    /// and against, apart from its pipeline, as frames take them as targets while they record it.
    taa: TaaPass,
    history: taa::History,
    /// Made by the first frame that antialiases the view by SMAA.
    smaa: Option<SmaaPass>,
    /// What shows an image the frame makes in the view, made by the first frame that shows one;
    /// and the name of the last it couldn't find, which it warned of.
    show: Option<ShowPass>,
    missing: Option<String>,
    /// The images the last frame asked to list them made that a show finds, as
    /// [`Renderer::images`] gives them.
    images: Vec<FrameImage>,
    /// Made by the first frame with a sky to show.
    sky: Option<SkyPass>,
    /// The sky's light, prefiltered, apart from its pipelines, as frames take its images as
    /// targets while they record its passes.
    sky_light: SkyLight,
    /// Made by the first frame with an atmosphere to show.
    atmosphere: Option<AtmospherePass>,
    /// The frame's lights, an atmosphere's sun dimmed by the air, kept from frame to frame.
    sunlit: Vec<Light>,
    /// The passes of the frame being rendered, declared afresh each frame in the memory of the
    /// last's.
    graph: Graph,
    /// The passes into the other windows that the frame draws UI in, in their order.
    window_passes: Vec<PassId>,
    /// What the view's pixels go into while it shows pixels ([`RenderData::pixels`]), their size,
    /// and whether a frame has written them.
    pixels: Option<(GpuTexture, bool)>,
    /// The number of the last frame, if it read the view back, which [`Renderer::view_read`]
    /// gives; 0 otherwise.
    view_read: u64,
    /// The picks asked of the view, from asked to answered.
    picks: Picks,
    ui: UiPass,
    instance: Arc<Instance>,
    device: Arc<Device>,
    vsync: bool,
    /// As [`Settings::visibility_buffer`] says.
    visibility_buffer: bool,
    /// The frame to capture, as [`Settings::capture`] says.
    capture: Option<NonZeroU64>,
    /// Set once rendering fails, which can leave the GPU's copies of meshes and textures
    /// unfinished.
    failed: bool,
    /// How long the last frame waited, which [`Renderer::waited`] describes.
    waited: Duration,
    /// What the frame two before the last drew, which [`Renderer::drawn`] describes.
    drawn: DrawCounts,
    /// What each frame in flight drew of the draws it didn't cull, and whether it culled any, by
    /// its number, for the frame two later to add what the culling counted, as it comes back.
    counted: [(u64, DrawCounts, bool); 2],
    /// The frame's lights as shaders read them, and what bins them into the view's clusters.
    lighting: Lighting,
    /// What moves skinned meshes' vertices before the frame draws them.
    skinning: Skinning,
    /// What writes emitters' particles' births before the frame draws them, and draws them.
    particles: ParticlePass,
    /// What computes the view's ambient occlusion, and the history it averages it over the frames
    /// by, apart from it, as frames take its images as targets while they record its passes.
    ao: AoPass,
    ao_history: ao::History,
    /// What traces the view's reflections and keeps its colors for them, and the images it keeps
    /// them in, apart from it, as frames take them as targets while they record its passes.
    reflections: ReflectionPass,
    reflected: reflections::Images,
    /// What meters the view, for its exposure to adapt; what adapts the game's own view's exposure
    /// and a tool's look's, apart, as [`RenderData::looked`] says; and what lit shading multiplied
    /// light by in the last frame rendered, 0 before the first.
    exposure: ExposurePass,
    adapting: [Adapting; 2],
    exposed: f32,
    /// What blurs the view by its depth of field, and by its motion.
    depth_of_field: DepthOfFieldPass,
    motion_blur: MotionBlurPass,
    /// What lays a lens's effects over the view.
    lens: LensPass,
}

enum Output {
    Window(Swapchain),
    /// The image, and the size it's to have, which the next frame makes it again at, if it hasn't.
    Offscreen(ColorImage, [u32; 2]),
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
        let device = Device::new(
            &instance,
            settings.gpu,
            Some(&surface),
            settings.uploads,
            settings.mesh_shaders,
        )?;
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
        let device = Device::new(
            &instance,
            settings.gpu,
            None,
            settings.uploads,
            settings.mesh_shaders,
        )?;
        let image = ColorImage::new(&device, width, height, OFFSCREEN)?;
        let output = Output::Offscreen(image, [width, height]);
        Self::new(&instance, &device, output, settings)
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

    /// Tells the renderer that the window's drawable area changed size; or, headless, has the next
    /// frame render into an offscreen image of that size, made then.
    ///
    /// # Panics
    ///
    /// Headless, if a side is 0.
    pub fn resize(&mut self, width: u32, height: u32) {
        match &mut self.output {
            Output::Window(swapchain) => swapchain.resize(width, height),
            Output::Offscreen(_, size) => {
                assert!(width > 0 && height > 0, "an offscreen image has an area");
                *size = [width, height];
            }
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
    /// It reads back the IDs at the pixels `data.picks` asks for, unless `data.read_picks` is
    /// false, and answers, in [`Renderer::picked`], those that the frame two before it read; then
    /// outlines in the view what `data.outline` says, from the view's IDs this frame.
    ///
    /// Returns `false` when there was nothing to render into, as when the window is minimized;
    /// the meshes and textures stay in `data.uploads`, and the UIs' texture changes and the picks
    /// wait, for the next frame that renders.
    ///
    /// The frame streams the meshes and textures queued, those `data.uploads` brings after those
    /// before, to the GPU within [`Settings::upload_budget`], in order, and draws each from the
    /// frame it's all there on: a mesh draws nothing until then, and a texture samples as a pixel
    /// of white, or of zeros if its texels are data, which text draws nothing with; one replaced
    /// draws as it did until its replacement is there. Through a transfer queue, no frame waits for
    /// the copies: what the frame streams becomes drawable in a later frame. [`Renderer::streaming`]
    /// tells what's still on the way. An asset that would take the GPU's memory past its budget
    /// waits in the queue, warning `renderer.over_budget` once, while those after it go on, unless
    /// nothing of the queue is being copied, when it goes ahead alone, so that a game comes up on
    /// a GPU past its budget already; one whose buffers or image the GPU's memory has no room for
    /// warns `renderer.out_of_memory` once and waits, tried again as room comes. A mesh, texture
    /// or shader removed draws nothing, or samples as its stand-in, from the frame that takes the
    /// removal on; one replaced while it streams drops what was streaming for its replacement.
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
    /// with a shader, whose handle no upload so far has brought; if its viewpoint's projection
    /// can't project, as [`Projection`] says; or if it asks more than [`MAX_PICKS`] picks.
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

    /// How long the last [`Renderer::render`] waited, if it rendered a frame: for the GPU to
    /// finish an earlier frame, and for the display to let go of an image. Zero if it didn't.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// What the GPU drew in the game's view in the frame whose counts the last [`Renderer::render`]
    /// that rendered a frame read, two frames before it, once it had finished, as the culling of
    /// its opaque runs counts them on the GPU. Zero if the last call rendered no frame, before then,
    /// and for a frame that didn't render the view or whose view had no area. The UI's draws
    /// aren't counted, nor the draws of meshes not yet drawable or removed, or with shaders
    /// removed.
    pub fn drawn(&self) -> DrawCounts {
        self.drawn
    }

    /// The time on the GPU of the frame whose timestamps the last [`Renderer::render`] that
    /// rendered a frame read, two frames before it: from its first pass's beginning to its last's
    /// end. `None` before then, and for a GPU whose graphics queue writes no timestamps. With
    /// gg-core's `tracy` feature, Tracy's GPU timeline shows each pass.
    pub fn gpu_time(&self) -> Option<Duration> {
        self.frames.gpu_time()
    }

    /// What lit shading multiplied light by in the last frame rendered: [`RenderData::exposure`],
    /// or what [`RenderData::adaptation`] adapted it to. 0 before the first.
    pub fn exposure(&self) -> f32 {
        self.exposed
    }

    /// The bytes of GPU memory the frames' own images are in, the view's among them, which those
    /// whose uses in a frame don't overlap share.
    pub fn transient_bytes(&self) -> u64 {
        self.frames.transient_bytes()
    }

    /// What's being streamed to the GPU after the last [`Renderer::render`]: the meshes and
    /// textures queued, being copied, or copied and not yet drawable, and the bytes of them still
    /// to copy. Empty once every mesh and texture added is drawable, or removed: a test renders
    /// until then.
    pub fn streaming(&self) -> Streaming {
        self.meshes.streaming()
    }

    /// The answers to picks that came in during the last [`Renderer::render`]: each pick's tag,
    /// with the ID of the draw on top at its pixel, as [`Draws`] gives it, in the frame that read
    /// it, or 0 where none drew. That's the frame that asked it, or for a pick past the 64 a frame
    /// reads, or asked while frames had nothing to render into or read no picks, the next that
    /// could read it. A pick of a pixel outside the view, as the frame that would read it renders
    /// it, is answered 0 there; the rest, as the frame two after the one that read them begins,
    /// once the GPU has rendered that one.
    pub fn picked(&self) -> &[(u64, u64)] {
        self.picks.answered()
    }

    /// The images the last frame that [`RenderData::list_images`] asked to list them made that
    /// [`Show::Image`] finds: of the graph's own, before the show's pass, but the antialiasing's,
    /// which a frame showing an image, drawn without it, doesn't make; in the order the frame made
    /// them, each name once, as the last of that name, which a show finds; none for a frame that
    /// had no view to draw. Empty before such a frame.
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

    /// How much of the GPU's own memory is in use, and the budget the driver gives the process, as
    /// [`gg_gpu::Device::memory_budget`] says.
    pub fn memory_budget(&self) -> MemoryBudget {
        self.device.memory_budget()
    }

    /// What the frames' arenas hold, of the geometry, instances and UI frames copy to the GPU, and
    /// the most a frame has taken of its own, as [`Arenas`] says.
    pub fn arenas(&self) -> Arenas {
        self.frames.arenas()
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
            Output::Offscreen(image, _) => image.read_back().map(Some),
        }
    }

    /// The game's view as the last [`Renderer::render`] rendered it and read it back, as
    /// [`RenderData::read_view`] asked, with its outline but without the UI drawn over it, once the
    /// GPU has finished that frame, which this waits for, holding back the next. `None` if that
    /// call read no view back, as when the view had no area, showed pixels, or there was nothing to
    /// render into.
    pub fn view_read(&self) -> Result<Option<ImageRead<'_>>> {
        if self.view_read == 0 {
            return Ok(None);
        }
        self.frames.image_read(self.view_read)
    }

    /// What validation reports. It outlives the renderer, so it also counts what's reported
    /// while the renderer shuts down.
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
