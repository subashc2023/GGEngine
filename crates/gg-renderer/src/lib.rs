//! Turns the render data the engine hands it each frame into images, built on `gg-gpu`: meshes,
//! sprites, circles, lines, grids and text drawn in the game's view, as its camera sees them, with
//! games' shaders and textures and the engine's own, models' among them, shaded by glTF's
//! metallic-roughness materials, or KHR_materials_pbrSpecularGlossiness's, under the frame's
//! lights, a directional light's shadows, in
//! cascades fitted to the view, and its sky, which shows behind them, prefiltered on the GPU, a
//! game's UI over them in the view's own space, and the UI over the view; the view drawn in colors
//! of high dynamic range, which a tonemapper maps into the range it shows; picks, which read back
//! the ID of the draw the view shows at a pixel; an outline round what the view shows of an ID;
//! and each frame's passes in a render graph, which places the barriers between them and makes the
//! view's images and the shadows' maps.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.

mod circle;
mod draws;
mod grid;
mod light;
mod line;
mod mesh;
mod model;
mod pick;
mod shader;
mod shadow;
mod sky;
mod sprite;
mod text;
mod texture;
mod tonemap;
mod ui;
mod view;
mod viewpoint;

use std::iter;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use gg_core::math::{Mat4, Vec4};
use gg_core::{Error, Result, label};
use gg_gpu::{
    ColorImage, Device, Frames, Graph, ImageId, Instance, InstanceDesc, PassId, Surface, Swapchain,
    Target, Texture as GpuTexture, TextureFormat,
};
use tracing::info;

pub use circle::Circle;
pub use draws::Draws;
pub use gg_gpu::{
    Arenas, Blend, ImageRead, MAX_TEXTURE_SIZE, MemoryBudget, Sampler, UploadPath, Validation,
    Window, Wrap, mip_sizes,
};
pub use grid::{Grid, GridLines};
pub use light::{Light, LightKind};
pub use line::Line;
pub use mesh::{Lod, Lods, Mesh, MeshDraw, Meshes};
#[doc(hidden)]
pub use model::Params as ModelParams;
pub use model::{AlphaMode, Material, MaterialId, Materials, ModelVertex, Sampled, Workflow};
pub use pick::{MAX_PICKS, Pick};
pub use shader::{Compiled, Shader, ShaderSource, Shaders, compile_shader};
pub use sky::Sky;
pub use sprite::{Sprite, SpriteSheet};
pub use text::Glyph;
pub use texture::{Map, Mipmaps, Texels, Texture, Textures};
#[doc(hidden)]
pub use texture::{add_levels, add_map_mipmaps, add_mipmaps};
pub use tonemap::Tonemapper;
pub use ui::Ui;
pub use view::{Outline, ViewCallback};
pub use viewpoint::{Projection, Viewpoint};

use light::GpuLight;
use mesh::{FrameData, MeshPass};
use pick::Picks;
use shadow::{CASCADES, Cascades, MAP_SIZE, ShadowData};
use sky::{SkyData, SkyLight, SkyPass};
use tonemap::TonemapPass;
use ui::UiPass;
use view::{OutlinePass, ViewPass};

/// The debug name of a headless renderer's image.
const OFFSCREEN: &str = "offscreen color";

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

/// The matrix from the canvas's space, as [`RenderData::canvas`] has it, of a view `size` pixels
/// across and down, to Vulkan's clip space, whose Y points down: every point at the near plane,
/// where reversed depth is 1, which every draw's depth test passes.
fn canvas_projection(size: [u32; 2]) -> Mat4 {
    let [width, height] = size.map(|side| side as f32);
    Mat4::from_cols(
        Vec4::new(2.0 / width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0 / height, 0.0, 0.0),
        Vec4::ZERO,
        Vec4::new(-1.0, 1.0, 1.0, 1.0),
    )
}

/// What a frame drew in the game's view, which [`Renderer::drawn`] tells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    /// Instanced draws: one for each run of consecutive draws of one mesh with one shader that draw
    /// the same faces, or, of a mesh with levels of detail, one for each level its run's instances
    /// drew at, or each stretch of them in a row that did, if its shader blends.
    pub draws: usize,
    /// The meshes they drew, one for each draw pushed to [`Draws`].
    pub instances: usize,
    /// The triangles of those meshes, at the levels they drew, culled ones among them: the GPU's
    /// work in drawing them.
    pub triangles: usize,
}

impl std::ops::AddAssign for DrawCounts {
    fn add_assign(&mut self, other: Self) {
        self.draws += other.draws;
        self.instances += other.instances;
        self.triangles += other.triangles;
    }
}

/// What's been added to a [`Meshes`], [`Shaders`], [`Textures`] and [`Materials`], replaced in
/// them or removed from them, since it was last taken from them, in order: the shaders a frame
/// makes pipelines for, the meshes and textures it queues to stream to the GPU, the materials it
/// writes into their table, and the removals it applies, before it draws. Taking moves them out of
/// the registries, which go on handing out handles where they are, so that a renderer on another
/// thread renders what they handed out before. It keeps its memory, so that a frame that adds
/// nothing allocates nothing.
#[derive(Default)]
pub struct Uploads {
    pub(crate) meshes: Vec<mesh::Change>,
    pub(crate) shaders: Vec<shader::Change>,
    pub(crate) textures: Vec<texture::Change>,
    pub(crate) materials: Vec<(u32, model::Entry)>,
}

impl Uploads {
    /// Takes what's changed in `meshes`, `shaders`, `textures` and `materials` since they were last
    /// taken from, after what it holds already.
    pub fn take(
        &mut self,
        meshes: &mut Meshes,
        shaders: &mut Shaders,
        textures: &mut Textures,
        materials: &mut Materials,
    ) {
        self.meshes.append(&mut meshes.changes);
        self.shaders.append(&mut shaders.changes);
        self.textures.append(&mut textures.changes);
        self.materials.append(&mut materials.changes);
    }
}

/// The most bytes a frame copies to the GPU of the meshes and textures being streamed, unless
/// [`Settings::upload_budget`] says otherwise: 16 MiB, which a PCIe link moves in about a
/// millisecond, and the renderer's staging holds in one allocation.
pub const DEFAULT_UPLOAD_BUDGET: u64 = 16 << 20;

/// What's being streamed to the GPU, which [`Renderer::streaming`] tells: the meshes and textures
/// queued, being copied, or copied and not yet drawable, and the bytes of them still to copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Streaming {
    pub assets: usize,
    pub bytes: u64,
}

impl Streaming {
    /// Whether nothing is being streamed: every mesh and texture added is drawable, or removed.
    pub fn is_empty(&self) -> bool {
        self.assets == 0
    }
}

/// What a frame draws besides UI: plain data, which the engine extracts every frame.
pub struct RenderData<'a> {
    /// The color the game's view clears to before the meshes are drawn, which its tonemapper maps
    /// with them, and the frame around it, as it is, in linear RGBA.
    pub clear_color: [f32; 4],
    /// How the view's colors, drawn in high dynamic range, map into the range it shows, 0 to 1:
    /// [`Tonemapper::None`] clamps them, so that what's in that range shows as it was drawn.
    pub tonemapper: Tonemapper,
    /// What lit shading multiplies light by, which shaders read from the frame's data, as
    /// [`Shaders::add`] describes: light in physical units, as lux and candela, times this, is what
    /// the tonemapper maps. What isn't lit, as sprites, text and unlit materials, draws its colors
    /// as they are.
    pub exposure: f32,
    /// The time the view shows, in seconds, which shaders read from the frame's data, as
    /// [`Shaders::add`] describes.
    pub time: f32,
    /// What lights the view's lit shading, which shaders read through the frame's data, as
    /// [`Shaders::add`] describes: without any, or a sky, models' materials draw unlit, their base
    /// colors alone. Of the directional lights with a shadow distance, the brightest casts
    /// shadows, as [`Light::shadow_distance`] says.
    pub lights: &'a [Light],
    /// What the view shows behind the world, in place of the clear color, and lights it by from
    /// all around, which shaders read through the frame's data too: from the first frame that
    /// renders the view once its texture's slot shows an image of its own, which that frame
    /// prefilters, and again each time it shows another. While it shows a stand-in, the view shows
    /// no sky and is lit by none. In a frame with a directional light that shines, which stands for
    /// the sky's sun, its light leaves the sun out, as [`Sky`] says.
    pub sky: Option<Sky>,
    /// Where the game's view shows in the frame. The meshes are drawn into an image of the view's
    /// own, its size, which the frame then shows there, under the UI; or, if the main window's UI
    /// paints a [`ViewCallback`], where that is instead.
    pub viewport: Viewport,
    /// What the view sees the world from.
    pub viewpoint: Viewpoint,
    /// The meshes, shaders and textures added, replaced or removed since a frame last took them,
    /// which it takes first: the shaders get their pipelines, the removals apply, and the meshes
    /// and textures queue up to stream to the GPU, within the frame's budget.
    pub uploads: &'a mut Uploads,
    /// What to draw.
    pub draws: &'a Draws,
    /// What to draw after `draws`, in the view's own space, as a game's UI is: each transform takes
    /// a draw's space to the view's pixels, from its bottom left, +Y up, at its near plane, so that
    /// each draw shows over everything before it, the nearest of `draws` too.
    pub canvas: &'a Draws,
    /// What the frame asks of the view, at most [`MAX_PICKS`], which [`Renderer::picked`] answers,
    /// as the frame two after the one that reads them begins: the frame reads them as it renders
    /// the view, after those a frame with nothing to render into, or that read none, left, up to
    /// [`MAX_PICKS`] in all, and the frames after it the rest.
    pub picks: &'a [Pick],
    /// Whether the frame reads picks at all. A frame that doesn't, as one that renders a view other
    /// than the one they were asked of, leaves those `picks` asks and those waiting already for
    /// the next frame that does, and answers those the frame two before it read all the same.
    pub read_picks: bool,
    /// What the frame outlines in the view, after reading the picks back, if anything: drawn into
    /// the view's image, so that the view read back shows it too.
    pub outline: Option<Outline>,
    /// Whether the frame reads the view back as it renders it, outline and all, which
    /// [`Renderer::view_read`] then gives.
    pub read_view: bool,
    /// Pixels the view shows in place of what `draws` would draw, from its top left, each onto one
    /// of the frame's, as far as they reach of it, the rest of it the clear color: the view of
    /// another process's game, say. The frame draws nothing in the view then, and answers its
    /// picks 0. Pixels the frame is to show again, which no frame wrote at their size, as a frame
    /// with nothing to render into doesn't, show nothing.
    pub pixels: Option<Pixels<'a>>,
}

/// Pixels for the view to show, as [`RenderData::pixels`] has it: their width and height, and rows
/// of RGBA8, sRGB-encoded and tightly packed, top row first; or `None` for the bytes, to show those
/// the last frame that rendered wrote, if it wrote some of this size.
#[derive(Clone, Copy, Debug)]
pub struct Pixels<'a> {
    pub size: [u32; 2],
    pub bytes: Option<&'a [u8]>,
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
    /// The most bytes a frame copies to the GPU of the meshes and textures being streamed:
    /// [`DEFAULT_UPLOAD_BUDGET`], 16 MiB, unless a test or a measurement wants another. A frame
    /// copies parts of what's queued, in the order it was added, while its bytes are under the
    /// budget, cutting the part that would cross it to fit, but never below the smallest whole
    /// part, a row of a texture's level or 16 bytes of a buffer, so that each frame makes
    /// progress. Through a transfer queue, a frame that begins with the copies of two or more
    /// frames before it unfinished, as a copy engine that can't keep up leaves, copies nothing, so
    /// that what's in flight stays within three frames' budgets however the frames outpace it.
    pub upload_budget: u64,
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
    tonemap: TonemapPass,
    outline: OutlinePass,
    /// Made by the first frame with a sky to show.
    sky: Option<SkyPass>,
    /// The sky's light, prefiltered, apart from its pipelines, as frames take its images as
    /// targets while they record its passes.
    sky_light: SkyLight,
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
    /// The frame to capture, as [`Settings::capture`] says.
    capture: Option<NonZeroU64>,
    /// Set once rendering fails, which can leave the GPU's copies of meshes and textures
    /// unfinished.
    failed: bool,
    /// How long the last frame waited, which [`Renderer::waited`] describes.
    waited: Duration,
    /// What the last frame drew, which [`Renderer::drawn`] describes.
    drawn: DrawCounts,
    /// The frame's lights as shaders read them, kept between frames, so that one with no more than
    /// the last allocates nothing.
    lights: Vec<GpuLight>,
}

enum Output {
    Window(Swapchain),
    /// The image, and the size it's to have, which the next frame makes it again at, if it hasn't.
    Offscreen(ColorImage, [u32; 2]),
}

/// The game's view's images and passes in a frame's graph.
struct ViewPasses {
    /// What draws the shadows the view shows, if a light casts them.
    shadows: Option<ShadowPasses>,
    /// What the view draws into, of high dynamic range.
    hdr: ImageId,
    /// What it shows, tonemapped.
    color: ImageId,
    ids: ImageId,
    /// What draws the view.
    pass: PassId,
    /// What tonemaps it.
    tonemap: PassId,
    /// What reads the view's IDs at the picks' pixels, if the frame reads picks.
    read_ids: Option<PassId>,
    /// What outlines what the view shows of an ID, if anything.
    outline: Option<(PassId, Outline)>,
    /// What reads the view back, if the frame does.
    read_view: Option<PassId>,
}

/// The shadows' cascades, fitted to the view, and their shadow maps and the passes that draw them,
/// in a frame's graph, before the view's pass, which samples them.
struct ShadowPasses {
    cascades: Cascades,
    maps: [ImageId; CASCADES],
    passes: [PassId; CASCADES],
}

/// What the frame shows as the game's view: an image of its graph's, or a texture, by its index.
enum Shown {
    Image(ImageId),
    Texture(u32),
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
            window_keys: Vec::new(),
            windows: Vec::new(),
            meshes: MeshPass::new(device, settings.upload_budget)?,
            view: ViewPass::new(device, format)?,
            tonemap: TonemapPass::new(device)?,
            outline: OutlinePass::new(device)?,
            sky: None,
            sky_light: SkyLight::default(),
            graph: Graph::default(),
            window_passes: Vec::new(),
            pixels: None,
            view_read: 0,
            picks: Picks::default(),
            ui: UiPass::new(device, format)?,
            instance: instance.clone(),
            device: device.clone(),
            vsync: settings.vsync,
            capture: settings.capture,
            failed: false,
            waited: Duration::ZERO,
            drawn: DrawCounts::default(),
            lights: Vec::new(),
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
        // A view without an area renders nothing; nor does one that shows pixels, which go into a
        // texture of their own size, made again as that changes.
        let has_view = width > 0 && height > 0;
        let pixels = data.pixels.filter(|_| has_view);
        if let Some(Pixels {
            size,
            bytes: Some(_),
        }) = pixels
            && (self.pixels.as_ref()).is_none_or(|(texture, _)| texture.size() != size)
        {
            let [width, height] = size;
            let format = TextureFormat::Rgba8Srgb;
            let name = "view pixels";
            let texture = GpuTexture::new(&self.device, width, height, format, false, name)?;
            self.pixels = Some((texture, false));
        }
        // Pixels to show again that no frame wrote at their size show nothing.
        let pixels_shown = pixels.filter(|Pixels { size, bytes }| {
            (self.pixels.as_ref()).is_some_and(|(texture, written)| {
                texture.size() == *size && (bytes.is_some() || *written)
            })
        });
        let renders_view = has_view && pixels.is_none();
        if let Output::Offscreen(image, size) = &mut self.output
            && image.size() != *size
        {
            *image = ColorImage::new(&self.device, size[0], size[1], OFFSCREEN)?;
        }
        // The sky the view shows, while its texture's slot shows an image of its own, with that
        // image's size, how many times the slot has come to show another, and what its light
        // keeps each texel to, leaving out a sun that a directional light stands for.
        let sky = data.sky.filter(|_| renders_view).and_then(|sky| {
            let (size, shown) = self.meshes.texture_shown(sky.texture)?;
            Some((sky, size, shown, sky::cutoff(sky, data.lights)))
        });
        if sky.is_some() && self.sky.is_none() {
            self.sky = Some(SkyPass::new(&self.device)?);
        }
        // The frame's passes: the view, into images of its own, its size, so that it never shows
        // stretched, made again in the frame that changes its size, which a drag that resizes the
        // view does every frame, as growing them in steps and rendering into part of them wouldn't;
        // what the view's IDs are read back by; its tonemapping, into the image it shows; its
        // outline, and what reads it back; then the frame, which shows the view under the UI, and
        // the other windows' UI, over black.
        let has_ui = |key: &u64| windows.iter().any(|(drawn, _)| drawn == key);
        let windows_drawn = self.window_keys.iter().filter(|key| has_ui(key)).count();
        let graph = &mut self.graph;
        graph.clear();
        let output = graph.target(0);
        let size = data.viewport.size;
        // First, the sky's prefiltering, if this frame prefilters it: the first to render the view
        // since its texture came to show the image it shows, or since a directional light came to
        // stand for its sun or ceased to. Its images are the targets after the windows'. A frame
        // that doesn't takes the light from those before.
        let plan = match sky {
            Some((sky, sky_size, shown, cutoff)) if !self.sky_light.holds(sky, shown, cutoff) => {
                let first = 1 + windows_drawn;
                let sky_light = &mut self.sky_light;
                Some(sky_light.plan(&self.device, graph, first, sky, sky_size, cutoff)?)
            }
            _ => None,
        };
        let held = match (sky, &plan) {
            (Some((sky, _, shown, cutoff)), None) => self.sky_light.data(sky, shown, cutoff),
            _ => None,
        };
        let view = renders_view.then(|| {
            // The cascades of the shadows of the light that casts them, if one does, each drawn
            // before the view samples it.
            let aspect = data.viewport.aspect();
            let shadows = Cascades::fit(data.lights, &data.viewpoint, aspect).map(|cascades| {
                let names = [
                    "shadow cascade 0",
                    "shadow cascade 1",
                    "shadow cascade 2",
                    "shadow cascade 3",
                ];
                let labels = [
                    label!("shadow cascade 0"),
                    label!("shadow cascade 1"),
                    label!("shadow cascade 2"),
                    label!("shadow cascade 3"),
                ];
                let maps = names.map(|name| graph.shadow(name, [MAP_SIZE; 2]));
                let passes = std::array::from_fn(|i| graph.render(labels[i]).depth(maps[i]).add());
                ShadowPasses {
                    cascades,
                    maps,
                    passes,
                }
            });
            let hdr = graph.hdr("view colors", size);
            let ids = graph.ids("view ids", size);
            let depth = graph.depth("view depth", size);
            let mut pass = (graph.render(label!("view")))
                .clear(hdr, data.clear_color)
                .ids(ids)
                .depth(depth);
            // What the draws light by, as the frame that prefilters it leaves it, and the shadows
            // they're in.
            for light in plan.iter().flat_map(sky::Plan::sampled) {
                pass = pass.sample(light);
            }
            for &map in shadows.iter().flat_map(|shadows| &shadows.maps) {
                pass = pass.sample(map);
            }
            let pass = pass.add();
            let read_ids = (data.read_picks).then(|| graph.read_ids(label!("read ids"), ids));
            // Cleared as a graph's own image is first written, though the pass writes every pixel.
            let color = graph.color("view", size);
            let tonemap = (graph.render(label!("tonemap")))
                .clear(color, data.clear_color)
                .sample(hdr)
                .add();
            // Into the view's image, from the IDs the picks read, so that wherever the view shows,
            // and as it's read back, it shows outlined.
            let outline = data.outline.filter(Outline::shows).map(|outline| {
                let pass = graph
                    .render(label!("outline"))
                    .keep(color)
                    .sample(ids)
                    .add();
                (pass, outline)
            });
            let read_view = (data.read_view).then(|| graph.read_image(label!("read view"), color));
            ViewPasses {
                shadows,
                hdr,
                color,
                ids,
                pass,
                tonemap,
                read_ids,
                outline,
                read_view,
            }
        });
        let mut frame_pass = (graph.render(label!("frame"))).clear(output, data.clear_color);
        if let Some(view) = &view {
            frame_pass = frame_pass.sample(view.color);
        }
        let frame_pass = frame_pass.add();
        self.window_passes.clear();
        let black = [0.0, 0.0, 0.0, 1.0];
        // After the output, the other windows that draw UI, in their order.
        for target in 1..=windows_drawn {
            let target = graph.target(target);
            let pass = graph.render(label!("window")).clear(target, black).add();
            self.window_passes.push(pass);
        }
        let target = match &mut self.output {
            Output::Window(swapchain) => Target::Swapchain(swapchain),
            Output::Offscreen(image, _) => Target::Image(image),
        };
        let others = (self.window_keys.iter().zip(&mut self.windows))
            .filter(|(key, _)| has_ui(key))
            .map(|(_, swapchain)| Target::Swapchain(swapchain));
        let lit = (plan.is_some()).then(|| self.sky_light.targets());
        let targets = iter::once(target)
            .chain(others)
            .chain(lit.into_iter().flatten());
        let Some(mut frame) = self.frames.begin(targets, &self.graph)? else {
            return Ok(false);
        };
        // The frame two before has finished, and what it read with it.
        self.picks.answer(frame.ids_read());
        self.meshes.upload(&mut frame, data.uploads)?;
        self.ui.write_textures(&mut frame)?;
        if let Some(plan) = &plan {
            let pass = self.sky.as_ref().expect("made for a sky to show");
            pass.prefilter(&mut frame, plan);
        }
        // What the view shows, and how much of its area that covers.
        let (counts, view) = match (view, pixels_shown) {
            (Some(view), _) => {
                self.lights.clear();
                self.lights.extend(data.lights.iter().map(GpuLight::new));
                let lights = if self.lights.is_empty() {
                    0
                } else {
                    frame.upload(bytemuck::cast_slice(&self.lights))?.address()
                };
                let shared = FrameData {
                    camera: data.viewpoint.position,
                    time: data.time,
                    exposure: data.exposure,
                    light_count: u32::try_from(self.lights.len()).expect("fewer than 2^32 lights"),
                    lights,
                    materials: self.meshes.materials_address(),
                    eye: data.viewpoint.eye(),
                    sky: SkyData::default(),
                    shadows: ShadowData::default(),
                };
                // The casters, each cascade's from the light, which read the frame's data but for
                // what the view alone samples.
                if let Some(shadows) = &view.shadows {
                    let address = frame.upload(bytemuck::bytes_of(&shared))?.address();
                    for (i, &cascade) in shadows.passes.iter().enumerate() {
                        let mut pass = (frame.render(cascade))
                            .expect("the graph's own images are always there");
                        let clip = shadows.cascades.clip_from_world(i);
                        let map = [MAP_SIZE; 2];
                        (self.meshes).draw(&mut pass, clip, address, map, data.draws, true)?;
                    }
                }
                let mut pass =
                    (frame.render(view.pass)).expect("the graph's own images are always there");
                let lit = match (&plan, sky) {
                    (Some(plan), Some((sky, ..))) => Some(plan.data(&pass, sky)),
                    _ => held,
                };
                let shadows = (view.shadows.as_ref()).map(|shadows| {
                    let maps = shadows.maps.map(|map| pass.sampled(map));
                    shadows.cascades.data(maps)
                });
                let shared = FrameData {
                    sky: lit.unwrap_or_default(),
                    shadows: shadows.unwrap_or_default(),
                    ..shared
                };
                let shared = pass.upload(bytemuck::bytes_of(&shared))?.address();
                // Behind everything, in place of the clear color.
                if let Some(SkyData { sky, luminance, .. }) = lit {
                    let drawn = self.sky.as_ref().expect("made for a sky to show");
                    let aspect = data.viewport.aspect();
                    let scale = luminance * data.exposure;
                    drawn.draw(&mut pass, &data.viewpoint, aspect, sky, scale);
                }
                let mut counts = (self.meshes).draw(
                    &mut pass,
                    view_projection,
                    shared,
                    size,
                    data.draws,
                    false,
                )?;
                pass.begin_label(c"canvas");
                let canvas = canvas_projection(size);
                counts +=
                    (self.meshes).draw(&mut pass, canvas, shared, size, data.canvas, false)?;
                pass.end_label();
                drop(pass);
                if let Some(read) = view.read_ids {
                    let picked = self.picks.take(frame.number(), Some(size));
                    frame.read_ids(read, picked)?;
                }
                let mut pass =
                    (frame.render(view.tonemap)).expect("the graph's own images are always there");
                let hdr = pass.sampled(view.hdr);
                self.tonemap.draw(&mut pass, hdr, data.tonemapper);
                drop(pass);
                if let Some((outlined, outline)) = view.outline {
                    let mut pass =
                        (frame.render(outlined)).expect("the graph's own images are always there");
                    let ids = pass.sampled(view.ids);
                    self.outline.draw(&mut pass, ids, outline);
                }
                if let Some(read) = view.read_view {
                    self.view_read = frame.number();
                    frame.read_image(read)?;
                }
                (counts, Some((Shown::Image(view.color), size)))
            }
            (None, Some(Pixels { size, bytes })) => {
                if data.read_picks {
                    self.picks.take(frame.number(), None);
                }
                let (texture, written) =
                    (self.pixels.as_mut()).expect("the pixels' texture was made");
                if let Some(bytes) = bytes {
                    frame.write_texture(texture, [0, 0], size, bytes)?;
                    *written = true;
                }
                let shown = [size[0].min(width), size[1].min(height)];
                (
                    DrawCounts::default(),
                    Some((Shown::Texture(texture.index()), shown)),
                )
            }
            (None, None) => {
                if data.read_picks {
                    self.picks.take(frame.number(), None);
                }
                (DrawCounts::default(), None)
            }
        };
        let mut pass = (frame.render(frame_pass)).expect("a frame's first target has an image");
        let view = view.map(|(shown, size)| {
            let image = match shown {
                Shown::Image(image) => pass.sampled(image),
                Shown::Texture(index) => index,
            };
            (image, size)
        });
        // Under the whole UI, unless it paints the view among its own shapes.
        if let Some((image, size)) = view
            && !ui::draws_view(ui.primitives)
        {
            let viewport = Viewport {
                size,
                ..data.viewport
            };
            pass.begin_label(c"view");
            self.view.draw(&mut pass, viewport, image, None);
            pass.end_label();
        }
        let view = view.map(|(image, size)| ui::View {
            pass: &self.view,
            image,
            size,
        });
        pass.begin_label(c"ui");
        self.ui
            .draw(&mut pass, ui.primitives, ui.pixels_per_point, view)?;
        pass.end_label();
        drop(pass);
        let drawn = self
            .window_keys
            .iter()
            .filter_map(|key| windows.iter().find(|(drawn, _)| drawn == key));
        for (&window, (_, ui)) in self.window_passes.iter().zip(drawn) {
            // The view shows in the main window alone.
            if let Some(mut pass) = frame.render(window) {
                self.ui
                    .draw(&mut pass, ui.primitives, ui.pixels_per_point, None)?;
            }
        }
        self.ui.free_textures();
        frame.submit()?;
        if let (Some(plan), Some((sky, _, shown, _))) = (&plan, sky) {
            self.sky_light.prefiltered(sky, shown, plan);
        }
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
    /// it didn't, or if the view had no area. The UI's draws aren't counted, nor the draws of
    /// meshes not yet drawable or removed, or with shaders removed.
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
    use super::*;

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
