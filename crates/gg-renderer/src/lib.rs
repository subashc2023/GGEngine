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
mod depth_of_field;
mod draws;
mod exposure;
mod fog;
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

use std::iter;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use gg_core::math::{DVec3, Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result, label};
use gg_gpu::{
    ColorImage, ComputePass, Device, Frames, Graph, ImageId, Instance, InstanceDesc, PassId,
    RenderPass, Surface, Swapchain, Target, Texture as GpuTexture, TextureFormat,
};
use tracing::{info, warn};

pub use ao::AmbientOcclusion;
pub use atmosphere::Atmosphere;
pub use circle::Circle;
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
use atmosphere::{AerialData, AtmospherePass};
use bloom::BloomPass;
use cluster::{ClusterData, ClusterPlan, Lighting};
use depth_of_field::DepthOfFieldPass;
use exposure::{Adapting, ExposurePass};
use fog::{FogData, FogPass};
use lens::LensPass;
use mesh::{CullPlan, CullView, FIRST, FrameData, MeshPass, Normals, Runs, Shaded, Way};
use motion_blur::MotionBlurPass;
use particles::{ParticlePass, ParticlePlan};
use pick::Picks;
use reflections::{ReflectionData, ReflectionPass};
use shadow::{CASCADES, Cascades, MAP_SIZE, ShadowData};
use show::ShowPass;
use skin::{SkinPlan, Skinning};
use sky::{SkyData, SkyLight, SkyPass, Source};
use smaa::SmaaPass;
use taa::TaaPass;
use tonemap::TonemapPass;
use ui::UiPass;
use view::{OutlinePass, OutlinePlan, ViewPass};

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

/// Warns that the view shows image `name`, which the frame doesn't make, unless it's `found`: once
/// while frames go on asking for it, `missing` naming the last image warned of.
fn show_missing(missing: &mut Option<String>, name: &str, found: bool) {
    if found {
        *missing = None;
    } else if missing.as_deref() != Some(name) {
        warn!(
            code = "renderer.no_image",
            image = name,
            "the view shows image {name:?}, which the frame doesn't make, as black"
        );
        *missing = Some(name.to_owned());
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
    /// Instanced draws of an instance or more: one for each run of consecutive draws of one mesh
    /// with one shader that draw the same faces, or, of a mesh with levels of detail, one for each
    /// level its run's instances drew at, or, if its shader blends or it's on the canvas, each
    /// stretch of them in a row that did; and one for each emitter's particles.
    pub draws: usize,
    /// The meshes they drew, one for each draw pushed to [`Draws`] that culling kept, and one for
    /// each slot of an emitter's ring, of its particle or of none.
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
/// writes into their table, and the removals it applies, before it draws; and what's changed in a
/// [`Scene`], which it writes into the scene's table. Taking moves them out of
/// the registries, which go on handing out handles where they are, so that a renderer on another
/// thread renders what they handed out before. It keeps its memory, so that a frame that adds
/// nothing allocates nothing.
#[derive(Default)]
pub struct Uploads {
    pub(crate) meshes: Vec<mesh::Change>,
    pub(crate) shaders: Vec<shader::Change>,
    pub(crate) textures: Vec<texture::Change>,
    pub(crate) materials: Vec<(u32, model::Entry)>,
    pub(crate) scene: Vec<mesh::SceneChange>,
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

    /// Takes what's changed in `scene` since it was last taken from, after what it holds
    /// already, for a frame to write to the GPU before it draws the scene: so that one that
    /// doesn't render, as one with no image to render into, leaves them for the next.
    pub fn take_scene(&mut self, scene: &mut Scene) {
        self.scene.append(&mut scene.changes);
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
    /// [`Tonemapper::None`] clamps them, so that what's in that range shows as it was drawn; the
    /// others dither what they map against banding in the view's 8 bits.
    pub tonemapper: Tonemapper,
    /// How the view is antialiased, before it's tonemapped: [`Antialiasing::Taa`] resolves each
    /// frame against those before it, as it says, from the frame before when it resolved the view
    /// at its size, and from this frame's alone otherwise, as when it didn't render the view; what
    /// the canvas draws, after it, isn't. [`Antialiasing::Smaa`] antialiases each frame's view
    /// alone, as it says, before the canvas too. [`Antialiasing::None`] draws what each pixel's
    /// center shows.
    pub antialiasing: Antialiasing,
    /// What lit shading multiplies light by, which shaders read from the frame's data, as
    /// [`Shaders::add`] describes: light in physical units, as lux and candela, times this, is what
    /// the tonemapper maps, unless `adaptation` adapts it. What isn't lit, as sprites, text and
    /// unlit materials, draws its colors as they are.
    pub exposure: f32,
    /// How the exposure adapts to what the view sees, if it does, as [`Adaptation`] says: each
    /// frame that renders the view meters it, after it's antialiased and before it blooms and its
    /// canvas draws, and the frame two later reads that back as it begins, so that none waits for
    /// it, and shades at what the frame before shaded at moved toward what the meter last called
    /// for, by how far `time` moved on since the frame before. Frames shade at `exposure`, within
    /// its bounds, until a metering comes back, whose exposure they take at once. A change to the
    /// bounds, the compensation or the speeds takes effect in the frame it's made, the exposure
    /// adapting on; one to the percentiles, which the meter's passes take, leaves out what frames
    /// before it metered, until one metered by them comes back, which is taken at once. Every
    /// reader of the exposure takes what it moved to, shaders through the frame's data among them,
    /// and antialiasing's history and the colors reflections trace against are scaled by its
    /// change. `None` exposes every frame at `exposure`.
    pub adaptation: Option<Adaptation>,
    /// Whether the view is seen from where a tool looks, as the tool API's look sets a viewpoint,
    /// rather than from where the game sees it. An exposure that adapts adapts apart for each, so
    /// that a tool looking round a dark room exposes neither the game's own view nor a capture of
    /// it for the room: a frame of the game's own view while a tool looks, as a capture's, takes
    /// the game's own exposure as it last adapted, moved toward what it last metered.
    pub looked: bool,
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
    /// How much the view blooms, from 0, for none, to 1: its colors, as drawn and antialiased,
    /// before its canvas, so that a game's UI doesn't bloom, lerped by this toward their bloom, a
    /// weighted mean of them blurred further and further, which spreads their light round what's
    /// bright without adding any. A view under 32 pixels on a side doesn't bloom.
    pub bloom: f32,
    /// How the view's light is graded, as [`Grade`] says: its colors, as drawn, antialiased and
    /// bloomed, before its canvas, so that a game's UI isn't graded, and before the tonemapper
    /// maps them. The identity, which leaves them as they are, costs nothing.
    pub grade: Grade,
    /// How the view's ambient occlusion darkens its light from all around, the sky's, if at all, as
    /// [`AmbientOcclusion`] says: computed in a frame whose view a sky lights, seen through a
    /// perspective, where something opaque draws, and in no other, which runs none of it.
    pub ambient_occlusion: Option<AmbientOcclusion>,
    /// The height fog the view is in, if any, as [`Fog`] says: what's opaque in the view, the sky
    /// or the clear color among it, fogged once it's drawn, by a pass of its own over the view's
    /// colors, from its depth; and what blends after, fogging itself as it draws, the engine's
    /// models, particles, sprites, circles and text, but not lines or grids, which are for seeing
    /// by, nor the canvas. `None`, or fog of no density, fogs nothing.
    pub fog: Option<Fog>,
    /// How the view reflects what it shows, if at all, as [`Reflections`] says: in place of the
    /// sky's specular light on what's opaque, traced through the view's depth against the colors
    /// the frame before showed, antialiased, before its bloom, its grade and its canvas, which each
    /// frame that reflects keeps; in a frame whose view a sky lights, seen through a perspective,
    /// that the frame before kept those colors for, at its size, where something opaque draws, and
    /// in no other, which keeps them alone, or, without a sky or a perspective, runs none of it.
    pub reflections: Option<Reflections>,
    /// The atmosphere, which the view shows behind the world in place of `sky`, and lights it by
    /// from all around, as it would an image sky, under its sun, the brightest of the directional
    /// lights that shine, the first of those as bright: its tables made in every frame that renders
    /// the view, and its light prefiltered again as the sun turns, past a hundredth of a degree at
    /// the horizon to 1° high in the sky, or the viewer rises or falls past 100 m, rather than as
    /// the sun brightens. The sun's illuminance is taken outside the atmosphere, and the light that
    /// reaches the view's lit shading is dimmed by the transmittance from the viewer toward the
    /// sun, and once it's set is none, when it casts no shadows either. What it lights is hazed by
    /// how far it is from the viewer, in the world's units, meters.
    pub atmosphere: Option<Atmosphere>,
    /// The view's depth of field, if any, as [`DepthOfField`] says: its colors, as drawn and
    /// antialiased, blurred by their circles of confusion from its depth, before motion blur, bloom
    /// and its canvas, so that a game's UI isn't blurred. Under a perspective alone, as without one
    /// there's no lens.
    pub depth_of_field: Option<DepthOfField>,
    /// The view's motion blur, if any, as [`MotionBlur`] says: its colors, after depth of field,
    /// blurred by how far what they show moved since the frame before, from the view's motion,
    /// before bloom and its canvas, so that a game's UI isn't blurred.
    pub motion_blur: Option<MotionBlur>,
    /// What a camera's lens and film do to the view, as [`Lens`] says: after bloom and the grade,
    /// before its canvas, so that a game's UI shows none of it. [`Lens::NONE`] costs nothing.
    pub lens: Lens,
    /// Where the game's view shows in the frame. The meshes are drawn into an image of the view's
    /// own, its size, which the frame then shows there, under the UI; or, if the main window's UI
    /// paints a [`ViewCallback`], where that is instead.
    pub viewport: Viewport,
    /// What the view sees the world from.
    pub viewpoint: Viewpoint,
    /// What every position the frame draws is relative to, in `f32`, the viewpoint's among them,
    /// which [`Viewpoint::relative_to`] takes from it: a point near the viewpoint, so that what's
    /// near it draws as precisely far from the world's origin as at it. Fog's height and the
    /// atmosphere's altitude are the world's, absolute.
    pub origin: DVec3,
    /// Whether the view is cut to `viewpoint` from elsewhere since the frame before, rather than
    /// moved there, as a tool's look set or a camera switched: what takes the camera's motion from
    /// the frame before, motion blur, takes it as standing still.
    pub cut: bool,
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
    pub outline: Option<Outline<'a>>,
    /// Whether the frame reads the view back as it renders it, outline and all, which
    /// [`Renderer::view_read`] then gives.
    pub read_view: bool,
    /// Whether the frame lists the images it makes that [`Show::Image`] finds, which
    /// [`Renderer::images`] then gives.
    pub list_images: bool,
    /// What the view shows in place of its light, if anything, to see how it's made, as [`Show`]
    /// says: drawn without antialiasing's offset, nor the exposure metered, as it adapts on as it
    /// did. A part of shading shows nothing after the view's passes, no fog, depth of field, motion
    /// blur, bloom, grade or lens; without the sky, cleared to black; tonemapped only where it's
    /// light. An image shows from a frame drawn as the game's is but for its antialiasing, so that
    /// those passes make theirs. The frames after it antialias afresh, and reflect what the last
    /// frame that showed the view's light kept. What the canvas draws shows over a part of shading,
    /// but not over an image.
    pub show: Option<Show<'a>>,
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
    /// Draw meshlets through mesh shaders, or not, rather than as the GPU can, for tests and
    /// measurements of each way: through mesh shaders where the GPU has them, and otherwise
    /// through vertex shaders, as `shaders/model.slang` says. Mesh shaders asked for that the GPU
    /// lacks keep the renderer from starting (`gpu.no_mesh_shaders`).
    pub mesh_shaders: Option<bool>,
    /// Shade models' opaque and masked materials, where the view draws them by their meshlets,
    /// from a visibility buffer, as `shaders/visibility.slang` says, or, for tests and
    /// measurements of each way, as they draw, as everything else draws.
    pub visibility_buffer: bool,
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

/// The game's view's images and passes in a frame's graph.
struct ViewPasses {
    /// What moves skinned meshes' vertices before the passes that draw them, if the frame skins
    /// any.
    skin: Option<SkinPlan>,
    /// What writes emitters' particles' births before the passes that draw them, if the frame
    /// draws any.
    particles: Option<ParticlePlan>,
    /// What culls the view's opaque runs and their shadows' casters, if it draws any.
    cull: Option<CullPlan>,
    /// What bins the lights into the view's clusters, if a light is binned.
    clusters: Option<ClusterPlan>,
    /// What draws the shadows the view shows, if a light casts them.
    shadows: Option<ShadowPasses>,
    /// What the view draws into, of high dynamic range.
    hdr: ImageId,
    /// What it shows, tonemapped.
    color: ImageId,
    /// How far what's opaque in it moved in the world since the frame before.
    motion: ImageId,
    /// What draws the view: what its culling's first pass keeps, or all of it, if it culls nothing;
    /// into its visibility buffer, if it has one.
    first: PassId,
    /// What draws the rest over that, if it culls; into its visibility buffer, if it has one.
    second: Option<PassId>,
    /// What shades the view from its visibility buffer, and draws the rest over that, if it has
    /// one.
    shaded: Option<ShadedPasses>,
    /// What computes its ambient occlusion and traces its reflections, if it has either.
    early: Option<EarlyPasses>,
    /// What fogs what's opaque in it, and draws what blends over that, if it's in fog.
    fog: Option<FogPasses>,
    /// What resolves it, antialiased, if it's antialiased.
    taa: Option<taa::Plan>,
    /// What antialiases it by SMAA, if it's so antialiased.
    smaa: Option<smaa::Plan>,
    /// What keeps its colors, as drawn and antialiased, for the next frame's reflections, if it
    /// reflects.
    keep: Option<reflections::Keep>,
    /// What meters it, if its exposure adapts.
    exposure: Option<exposure::Plan>,
    /// What blurs it by its depth of field, and by its motion, if it's blurred so.
    focus: Option<depth_of_field::Plan>,
    blur: Option<motion_blur::Plan>,
    /// What blooms and grades it, if it does either.
    bloom: Option<bloom::Plan>,
    /// What lays a lens's effects over it, if it has any.
    lens: Option<lens::Plan>,
    /// What draws its canvas, if it has one, in a pass of its own, if the view is antialiased,
    /// kept for reflections, metered, blurred, bloomed, graded or seen through a lens; otherwise
    /// the view's last pass draws the canvas.
    canvas: Option<PassId>,
    /// What shows an image the frame makes in its place, if it shows one.
    show: Option<show::Plan>,
    /// What it shows, drawn, antialiased, blurred, bloomed, graded and seen through a lens, with
    /// its canvas over that, or an image the frame makes in its place, which the tonemap pass
    /// samples.
    shown: ImageId,
    /// What tonemaps it, and how.
    tonemap: PassId,
    tonemapper: Tonemapper,
    /// What reads the view's IDs at the picks' pixels, if the frame reads picks.
    read_ids: Option<PassId>,
    /// What outlines what the view shows of a set of IDs, if anything.
    outline: Option<OutlinePlan>,
    /// What reads the view back, if the frame does.
    read_view: Option<PassId>,
}

/// The game's view's visibility buffer, in a frame's graph, which the runs its culling draws by
/// their meshlets with the opaque and masked model shaders draw into, in the view's two passes;
/// the compute pass that then shades the view from it; and the pass after, which draws everything
/// else: the sky, the culled runs drawn otherwise, as their shaders shade them, what blends and the
/// canvas.
struct ShadedPasses {
    visibility: ImageId,
    shade: PassId,
    rest: PassId,
}

/// The game's view's ambient occlusion and reflections, in a frame's graph, which need the depth of
/// what's opaque in the view before any of it shades: beside a visibility buffer, which has the
/// depth of what it draws, the pass that draws the depth alone of the culled runs drawn otherwise,
/// if any may be, into an image of its own; reflecting, the compute pass that finds the view's
/// normals from its visibility buffer, as its shading turns them, and those normals, and the pass
/// that draws the normals of the culled runs whose depth alone was drawn, where they're nearest in
/// it, and those, which the reflections' rays are traced about; the occlusion and the reflections,
/// if the view has them, from the depths; and without a visibility buffer, whose view's first and
/// second passes draw the depth alone of every culled run, the pass that draws the view after,
/// shaded, as those would have.
struct EarlyPasses {
    depth: Option<PassId>,
    normals: Option<(PassId, ImageId)>,
    drawn_normals: Option<(PassId, ImageId)>,
    ao: Option<ao::Plan>,
    reflections: Option<reflections::Plan>,
    view: Option<PassId>,
}

impl EarlyPasses {
    /// What the passes that shade the view sample of them: the ambient occlusion and the
    /// reflections, those the view has.
    fn sampled(&self) -> [Option<ImageId>; 3] {
        let reflected = (self.reflections.as_ref()).map(reflections::Plan::sampled);
        let [colors, spread] = [0, 1].map(|i| reflected.map(|images| images[i]));
        [self.ao.as_ref().map(ao::Plan::image), colors, spread]
    }
}

/// The game's view's fog, in a frame's graph: the pass that fogs its colors once what's opaque has
/// drawn, the sky or the clear color among it, from its depth, and the pass after, which draws what
/// blends over them, fogging itself, and the canvas, where the view's last pass draws it.
struct FogPasses {
    plan: fog::Plan,
    blended: PassId,
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

/// `pass`, taking the instances and commands of what `cull` keeps, if the frame culls.
fn culled<'g>(cull: Option<&CullPlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match cull {
        Some(cull) => cull.drawn(pass),
        None => pass,
    }
}

/// `pass`, reading the skinned meshes' vertices where the skin pass moved them, if it moves any.
fn skinned<'g>(skin: Option<&SkinPlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match skin {
        Some(skin) => pass.read_buffer(skin.buffer),
        None => pass,
    }
}

/// `pass`, reading the particles' pool, if the frame draws particles.
fn particled<'g>(particles: Option<&ParticlePlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match particles {
        Some(particles) => particles.drawn(pass),
        None => pass,
    }
}

/// `pass`, sampling each of `images`.
fn sampling<'g>(images: impl Iterator<Item = ImageId>, pass: RenderPass<'g>) -> RenderPass<'g> {
    images.fold(pass, RenderPass::sample)
}

/// `pass`, reading the clusters the lights are binned into, if `clusters` bins them.
fn binned<'g>(clusters: Option<&ClusterPlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match clusters {
        Some(clusters) => pass.read_buffer(clusters.buffer),
        None => pass,
    }
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

    fn render_frame(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        let _zone = gg_core::zone!();
        let data = show::showing(data);
        // From here on, the world is relative to the frame's origin, as its draws are, the
        // viewpoint and fog's height too; the atmosphere alone reads the viewpoint's altitude, the
        // world's.
        let altitude = data.viewpoint;
        // What's kept from the frame before, in its origin's space, taken into this one's, before
        // this frame's changes to the scene; once, whether or not the frame renders.
        let moved = (data.origin - self.origin).as_vec3();
        self.origin = data.origin;
        if moved != Vec3::ZERO {
            self.meshes.moved(moved);
            self.particles.moved(moved);
            self.taa.moved(moved);
            self.ao_history.moved(moved);
            self.reflected.moved(moved);
            self.motion_blur.moved(moved);
        }
        let data = RenderData {
            viewpoint: data.viewpoint.relative_to(data.origin),
            fog: (data.fog).map(|fog| Fog {
                height: (f64::from(fog.height) - data.origin.y) as f32,
                ..fog
            }),
            ..data
        };
        // egui sends each texture change once, so changes wait for a frame that renders.
        for (_, window) in windows.iter_mut() {
            self.ui.queue(window.textures);
        }
        self.ui.queue(ui.textures);
        // The frame about to begin; one with nothing to render into begins none, and the next is
        // of the same number.
        let number = self.frames.submitted() + 1;
        // Before the frame begins, which a failure would then present unrendered.
        self.meshes.add_shaders(data.uploads, number)?;
        // Every frame, whether it renders or draws anything, so that a viewpoint that can't
        // project always panics. A view without an area draws nothing, so its aspect ratio doesn't
        // matter.
        let [width, height] = data.viewport.size;
        let unjittered = data.viewpoint.view_projection(data.viewport.aspect());
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
        // A view antialiased temporally is drawn off by the frame's offset, everything in it but its
        // canvas, and culled as it's drawn; one antialiased by SMAA is drawn as it is.
        let antialiased = renders_view && data.antialiasing == Antialiasing::Taa;
        let smaa = renders_view && data.antialiasing == Antialiasing::Smaa;
        if smaa && self.smaa.is_none() {
            self.smaa = Some(SmaaPass::new(&self.device)?);
        }
        let image_shown = match data.show {
            Some(Show::Image(name)) if renders_view => Some(name),
            _ => None,
        };
        if image_shown.is_some() && self.show.is_none() {
            self.show = Some(ShowPass::new(&self.device)?);
        }
        if image_shown.is_none() {
            self.missing = None;
        }
        let (jitter, view_projection) = if antialiased {
            self.history.prepare(&self.device, data.viewport.size)?;
            let jitter = taa::normalized(self.taa.offset(), data.viewport.size);
            (jitter, taa::jitter(jitter) * unjittered)
        } else {
            (Vec2::ZERO, unjittered)
        };
        if let Output::Offscreen(image, size) = &mut self.output
            && image.size() != *size
        {
            *image = ColorImage::new(&self.device, size[0], size[1], OFFSCREEN)?;
        }
        // The atmosphere the view shows in place of an image sky, if any, under its sun, and the
        // lights the frame shades with: its sun's dimmed by the air between it and the viewer.
        let atmosphere = (data.atmosphere.filter(|_| renders_view))
            .map(|atmosphere| atmosphere::Seen::new(atmosphere, data.lights, &altitude));
        let shining = match &atmosphere {
            Some(seen) if seen.dims() => {
                self.sunlit.clear();
                self.sunlit.extend_from_slice(data.lights);
                seen.dim(&mut self.sunlit);
                &self.sunlit[..]
            }
            _ => data.lights,
        };
        // What the sky's light is prefiltered from, and what a texel of it of 1 stands for: the
        // atmosphere, of a sun of an illuminance of 1; or the sky the view shows, while its
        // texture's slot shows an image of its own, with that image's size, how many times the
        // slot has come to show another, and what its light keeps each texel to, leaving out a
        // sun that a directional light stands for.
        let sky = match &atmosphere {
            Some(seen) => Some((Source::Atmosphere(seen.lit()), seen.illuminance())),
            None => data.sky.filter(|_| renders_view).and_then(|sky| {
                let (size, shown) = self.meshes.texture_shown(sky.texture)?;
                let cutoff = sky::cutoff(sky, shining);
                let source = Source::Image {
                    sky,
                    size,
                    shown,
                    cutoff,
                };
                Some((source, sky.luminance))
            }),
        };
        if sky.is_some() && self.sky.is_none() {
            self.sky = Some(SkyPass::new(&self.device)?);
        }
        if atmosphere.is_some() && self.atmosphere.is_none() {
            self.atmosphere = Some(AtmospherePass::new(&self.device)?);
        }
        // The view's ambient occlusion, where it darkens anything: of the light from all around,
        // which only a sky gives, under a perspective, whose depth it reads.
        let perspective = matches!(data.viewpoint.projection, Projection::Perspective { .. });
        let occluded = (data.ambient_occlusion)
            .filter(|occlusion| occlusion.darkens() && sky.is_some() && perspective);
        // Averaged over the frames by a history in images of the view's size, let go of by a frame
        // that renders the view without; against what the frame before accumulated, if it did, at
        // this size, and the view isn't cut to where it sees from.
        if occluded.is_some() {
            self.ao_history.prepare(&self.device, data.viewport.size)?;
        } else if renders_view {
            self.ao_history.forget();
        }
        let occluded_against = (self.ao_history).against(number, data.viewport.size, data.cut);
        // The view's reflections, where they show anything: in place of the sky's specular light,
        // under a perspective, whose depth they're traced through, against the colors the frame
        // before kept, which each frame that reflects keeps for the next, in images of the view's
        // size, let go of by a frame that renders the view without.
        let reflected = (data.reflections)
            .filter(|reflections| reflections.reflects() && sky.is_some() && perspective);
        if reflected.is_some() {
            self.reflected.prepare(&self.device, data.viewport.size)?;
        } else if renders_view {
            self.reflected.forget();
        }
        // Traced against the colors the frame before kept, if it kept them at this size, and how
        // that frame drew them.
        let traced = reflected.zip(self.reflected.traced_against(number, data.viewport.size));
        if renders_view {
            self.lighting.order(shining, &data.viewpoint);
        } else {
            // The pool goes undeclared, which the graph lets go of.
            self.particles.forget();
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
        let missing = &mut self.missing;
        let images = &mut self.images;
        if data.list_images {
            images.clear();
        }
        graph.clear();
        let output = graph.target(0);
        let size = data.viewport.size;
        // The antialiased view's images it resolves into and against are the targets after the
        // windows'.
        let resolved = 1 + windows_drawn;
        let resolves = if antialiased { taa::TARGETS } else { 0 };
        // The reflections' images are the targets after those, and the ambient occlusion's history's
        // after those.
        let reflections_first = resolved + resolves;
        let reflects = if reflected.is_some() { 2 } else { 0 };
        let occlusion_first = reflections_first + reflects;
        let occludes = if occluded.is_some() { 2 } else { 0 };
        // First, the atmosphere's tables, if the frame shows one, which the view samples and its
        // sky's cube is made from.
        let tables = atmosphere.as_ref().map(|_| AtmospherePass::plan(graph));
        // Then the sky's prefiltering, if this frame prefilters it: the first to render the view
        // since its texture came to show the image it shows, or since a directional light came to
        // stand for its sun or ceased to, or, while one stands for it, its luminance crossed a
        // power of two that changes what its light keeps texels to; or, of the atmosphere, since
        // its sun turned or the viewer rose or fell past what the light holds. Its images are the
        // targets after those. A frame that doesn't takes the light from those before.
        let plan = match sky {
            Some((source, _)) if !self.sky_light.holds(&source) => {
                let first = occlusion_first + occludes;
                let sampled = tables.as_ref().map(atmosphere::Plan::sky_view);
                let sky_light = &mut self.sky_light;
                Some(sky_light.plan(&self.device, graph, first, source, sampled)?)
            }
            _ => None,
        };
        let held = match (sky, &plan) {
            (Some((source, luminance)), None) => self.sky_light.data(&source, luminance),
            _ => None,
        };
        let meshes = &self.meshes;
        let skinning = &mut self.skinning;
        let particles = &mut self.particles;
        let lighting = &self.lighting;
        let taa = &self.taa;
        let visibility_buffer = self.visibility_buffer;
        let fogged = data.fog.is_some_and(|fog| fog.shows());
        let view = renders_view.then(|| {
            // The cascades of the shadows of the light that casts them, if one does, each drawn
            // before the view samples it, and before them, the culling of the view's opaque runs
            // and the cascades' casters, for the view's first pass; and after them, the binning of
            // the lights into the view's clusters, which its passes read. The culling and the
            // binning run on the compute queue where there's one, the binning beside the shadows.
            // First, the skinned meshes' vertices moved, which every pass that draws reads, and the
            // particles' births written, which the passes that draw what blends read.
            let skin_plan = skinning.plan(graph, data.draws, meshes);
            let skin = skin_plan.as_ref();
            let particle_plan = particles.plan(graph, data.draws);
            let born = particle_plan.as_ref();
            let aspect = data.viewport.aspect();
            // Of the lights as they shine outside an atmosphere, so that its sun casts them while
            // it's up however dim, and none once it's set.
            let casters = match &atmosphere {
                Some(seen) if seen.set() => shining,
                _ => data.lights,
            };
            let cascades = Cascades::fit(casters, &data.viewpoint, aspect);
            let mut cull = (meshes.cull_bounds(data.draws))
                .map(|bounds| CullPlan::new(graph, bounds, cascades.is_some()));
            let shadows = cascades.map(|cascades| {
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
                let maps = names.map(|name| graph.depth(name, [MAP_SIZE; 2]));
                let passes = std::array::from_fn(|i| {
                    let pass = skinned(skin, graph.render(labels[i]).depth(maps[i]));
                    culled(cull.as_ref(), pass).add()
                });
                ShadowPasses {
                    cascades,
                    maps,
                    passes,
                }
            });
            let clusters = lighting.plan(graph);
            let hdr = graph.hdr("view colors", size);
            let ids = graph.ids("view ids", size);
            // Drawn every frame, antialiased or not, as the IDs are, for the reason
            // ARCHITECTURE.md's Rendering gives: so that one pipeline draws each shader.
            let motion = graph.motion("view motion", size);
            let depth = graph.depth("view depth", size);
            // What the draws light by, as the frame that prefilters it leaves it, the atmosphere's
            // tables, and the shadows they're in.
            let lit = || {
                let maps = shadows.iter().flat_map(|shadows| shadows.maps);
                let tables = tables.iter().flat_map(atmosphere::Plan::sampled);
                plan.iter()
                    .flat_map(sky::Plan::sampled)
                    .chain(tables)
                    .chain(maps)
            };
            // A pass of the view's, named `label`, that draws over what the passes before it drew,
            // shading as it draws: taking what `kept` keeps, if it's given a culling, and reading
            // the skinned meshes' vertices, the particles' pool and the lights' clusters, and
            // sampling what the draws light by, and the view's ambient occlusion and reflections,
            // `early`, those it has for what the pass draws.
            let shading =
                |graph: &mut Graph, label, kept: Option<&CullPlan>, early: [Option<ImageId>; 3]| {
                    let pass = particled(born, culled(kept, skinned(skin, graph.render(label))));
                    let sampled = lit().chain(early.into_iter().flatten());
                    let pass = binned(clusters.as_ref(), sampling(sampled, pass));
                    pass.keep(hdr).keep(ids).keep(motion).keep(depth).add()
                };
            // Where the view culls runs drawn by their meshlets, those of the opaque and masked
            // model shaders draw into a visibility buffer alone, in both of its passes, and a
            // compute pass then shades the view from it, before a pass that draws the rest.
            let visibility = (cull.as_ref())
                .filter(|cull| visibility_buffer && cull.by_meshlets())
                .map(|_| graph.visibility("view visibility", size));
            // The ambient occlusion and the reflections take the depth of what's opaque before any
            // of it shades, where something is: what's drawn otherwise than into a visibility
            // buffer, if anything may be, draws its depth alone first, into an image of its own;
            // without one, in the view's first and second passes, and the view then draws all
            // again, shaded, in one pass.
            let early = cull.is_some() && (occluded.is_some() || traced.is_some());
            let by_indices = (cull.as_ref()).is_some_and(CullPlan::by_indices);
            let opaque_depth = (early && (visibility.is_none() || by_indices))
                .then(|| graph.depth("opaque depth", size));
            let forward_depth = opaque_depth.filter(|_| visibility.is_none());
            let first = match (visibility, forward_depth) {
                (Some(visibility), _) => {
                    let first = skinned(skin, graph.render(label!("view visibility")));
                    let first = culled(cull.as_ref(), first);
                    first.visibility(visibility).ids(ids).depth(depth)
                }
                (None, Some(opaque_depth)) => {
                    let first = skinned(skin, graph.render(label!("opaque depth")));
                    culled(cull.as_ref(), first).depth(opaque_depth)
                }
                (None, None) => {
                    let first = skinned(skin, graph.render(label!("view")));
                    let first = particled(born, culled(cull.as_ref(), first));
                    let first = binned(clusters.as_ref(), sampling(lit(), first));
                    first
                        .clear(hdr, data.clear_color)
                        .ids(ids)
                        .motion(motion)
                        .depth(depth)
                }
            };
            let first = first.add();
            // Where the view culls, its second pass, over its first: once the depth pyramid is
            // built from the first's depth, and the culling's second pass has culled against it,
            // what that kept, and, without a visibility buffer, the runs it doesn't cull.
            let second = cull.as_mut().map(|cull| {
                cull.second_pass(graph, forward_depth.unwrap_or(depth), size);
                match (visibility, forward_depth) {
                    (Some(visibility), _) => {
                        let second = skinned(skin, graph.render(label!("view visibility second")));
                        cull.drawn(second)
                            .keep(visibility)
                            .keep(ids)
                            .keep(depth)
                            .add()
                    }
                    (None, Some(opaque_depth)) => {
                        let label = label!("opaque depth second");
                        let second = cull.drawn(skinned(skin, graph.render(label)));
                        second.keep(opaque_depth).add()
                    }
                    (None, None) => shading(graph, label!("view second"), Some(&*cull), [None; 3]),
                }
            });
            // Beside a visibility buffer, the depth of the culled runs drawn otherwise, if any may
            // be, and reflecting, the view's normals as its shading turns them, where the buffer
            // drew; reflecting, the normals of those whose depth alone was drawn, where they're
            // nearest in it; then the occlusion and the reflections, from the nearer of the two
            // depths; and without one, the view, shaded.
            let early = early.then(|| {
                let rest = visibility.zip(opaque_depth).map(|(_, opaque_depth)| {
                    let rest = skinned(skin, graph.render(label!("opaque depth")));
                    culled(cull.as_ref(), rest).depth(opaque_depth).add()
                });
                let normals = (visibility.filter(|_| traced.is_some()).zip(cull.as_ref())).map(
                    |(visibility, cull)| {
                        let normals = graph.hdr("view normals", size);
                        let find = graph.compute(label!("view normals")).sample(visibility);
                        (cull.shaded(find).overwrite(normals).add(), normals)
                    },
                );
                let drawn_normals = opaque_depth
                    .filter(|_| traced.is_some())
                    .map(|opaque_depth| {
                        let normals = graph.hdr("opaque normals", size);
                        let draw = skinned(skin, graph.render(label!("opaque normals")));
                        let draw = culled(cull.as_ref(), draw).clear(normals, [0.0; 4]);
                        (draw.keep(opaque_depth).add(), normals)
                    });
                let alone = opaque_depth.unwrap_or(depth);
                let depths = [if visibility.is_some() { depth } else { alone }, alone];
                let ao = occluded.map(|_| {
                    let against = occluded_against;
                    AoPass::plan(graph, depths, size, occlusion_first, number, against)
                });
                // Each depth's, as they pair.
                let found = [normals, drawn_normals].map(|pair| pair.map(|(_, normals)| normals));
                let reflections = (traced)
                    .map(|_| ReflectionPass::plan(graph, reflections_first, depths, found, size));
                let mut early = EarlyPasses {
                    depth: rest,
                    normals,
                    drawn_normals,
                    ao,
                    reflections,
                    view: None,
                };
                let sampled = early.sampled().into_iter().flatten();
                early.view = visibility.is_none().then(|| {
                    let view = skinned(skin, graph.render(label!("view")));
                    let view = particled(born, culled(cull.as_ref(), view));
                    let view = binned(clusters.as_ref(), sampling(lit().chain(sampled), view));
                    let view = view.clear(hdr, data.clear_color).ids(ids).motion(motion);
                    view.depth(depth).add()
                });
                early
            });
            let sampled_early = early.as_ref().map_or([None; 3], EarlyPasses::sampled);
            let shaded = visibility.zip(cull.as_ref()).map(|(visibility, cull)| {
                let shade = graph.compute(label!("view shading")).sample(visibility);
                let shade = (lit().chain(sampled_early.into_iter().flatten()))
                    .fold(shade, ComputePass::sample);
                let shade = match &clusters {
                    Some(clusters) => shade.read_buffer(clusters.buffer),
                    None => shade,
                };
                let shade = cull.shaded(shade).overwrite(hdr).overwrite(motion).add();
                let rest = shading(graph, label!("view rest"), Some(cull), sampled_early);
                ShadedPasses {
                    visibility,
                    shade,
                    rest,
                }
            });
            // In fog, what's opaque, the sky among it, is fogged by a pass of its own over the
            // view's colors, from its depth, once it's drawn; then what blends draws over them in a
            // pass of its own, fogging itself, as a pass over the colors alone would fog it as
            // though it were as far away as what's behind it.
            let fog = fogged.then(|| {
                let sky_light = plan.iter().flat_map(sky::Plan::sampled);
                FogPasses {
                    plan: FogPass::plan(graph, size, depth, hdr, sky_light),
                    blended: shading(graph, label!("view blended"), None, [None; 3]),
                }
            });
            // Antialiased, the view's colors, which the frame drew off by its offset, are resolved
            // against the frames before into one of the antialiasing's images; and into the view's
            // resolved colors apart, if what follows writes over them in place, as the next frame
            // resolves against what the resolve alone wrote. Antialiased by SMAA, they're blended
            // into colors of their own, which keep no history. Then depth of field, into those
            // colors, and motion blur, into colors of its own, as it reads the pixels round each,
            // before bloom, as Unreal orders them; bloom and the grade, into the colors the view
            // shows; and the lens, into colors of its own, as its chromatic aberration reads the
            // pixels round each: all before the canvas, so that a game's UI is neither blurred,
            // bloomed, graded nor seen through a lens. Then the canvas, if there is one, which
            // isn't antialiased, in a pass of its own over what the view shows, where the view's
            // last pass doesn't draw it, as when any of those follows it. Reflecting, the colors as
            // antialiased, resolved or blended by SMAA, or as drawn, are kept for the next frame's
            // reflections first, before depth of field, motion blur, bloom, the grade, the lens and
            // the canvas, so that a reflection holds none of them; and where the exposure adapts,
            // they're metered next, before the same.
            let blooms = bloom::blooms(data.bloom, size);
            let grades = !data.grade.is_identity();
            let circle = (data.depth_of_field)
                .and_then(|focus| depth_of_field::Circle::new(focus, &data.viewpoint, size));
            let blur = data.motion_blur.filter(MotionBlur::blurs);
            let lensed = data.lens.shows();
            // Where the antialiasing's images are among the graph's own, which a frame that shows
            // an image, drawn without antialiasing, doesn't make, and a list of the images leaves
            // out, so that it offers only what a show finds.
            let unlisted_from = data.list_images.then(|| graph.images().count());
            let taa = antialiased.then(|| {
                // Apart where what follows writes the resolved colors in place: depth of field;
                // bloom, the grade or the canvas, unless motion blur or the lens has written colors
                // of their own before them.
                let over = blooms || grades || (!lensed && !data.canvas.is_empty());
                let apart = circle.is_some() || (blur.is_none() && over);
                let drawn = [hdr, depth, motion];
                taa.plan(graph, resolved, number, size, drawn, apart)
            });
            let smaa = smaa.then(|| SmaaPass::plan(graph, hdr, size));
            let unlisted = unlisted_from.map_or(0..0, |from| from..graph.images().count());
            let shown = taa.as_ref().map_or(hdr, taa::Plan::shown);
            let shown = smaa.as_ref().map_or(shown, smaa::Plan::shown);
            // A frame that shows something in place of the view's light keeps nothing for the next,
            // whose reflections take what the last frame that showed the view's light kept.
            let keep = (reflected)
                .filter(|_| data.show.is_none())
                .map(|_| ReflectionPass::plan_keep(graph, reflections_first, shown, size));
            // Where its exposure adapts, the view's light is metered as resolved, before the
            // camera's passes, bloom and the canvas, whose UI isn't lit: blurring keeps about the
            // mean of what it blurs, and a lens's vignette and grain shouldn't move the exposure.
            let looked = data.looked;
            let metered = data.adaptation.is_some() && data.show.is_none();
            let exposure = metered.then(|| ExposurePass::plan(graph, shown, size, looked));
            let focus =
                circle.map(|circle| DepthOfFieldPass::plan(graph, circle, shown, depth, size));
            let blur =
                blur.map(|blur| MotionBlurPass::plan(graph, blur, [shown, depth, motion], size));
            let shown = blur.as_ref().map_or(shown, motion_blur::Plan::blurred);
            let bloom =
                (blooms || grades).then(|| BloomPass::plan(graph, shown, size, blooms, grades));
            let lens = lensed.then(|| LensPass::plan(graph, data.lens, shown, size));
            let shown = lens.as_ref().map_or(shown, lens::Plan::lensed);
            let after = taa.is_some()
                || smaa.is_some()
                || keep.is_some()
                || exposure.is_some()
                || focus.is_some()
                || blur.is_some()
                || bloom.is_some()
                || lens.is_some();
            let canvas = (!data.canvas.is_empty() && after).then(|| {
                let canvas = graph.render(label!("canvas"));
                let canvas = binned(clusters.as_ref(), sampling(lit(), canvas));
                (canvas.keep(shown).keep(ids).keep(motion).keep(depth)).add()
            });
            if data.list_images {
                show::list(images, graph, unlisted);
            }
            // An image the frame makes in place of all that, the last of its name added so far.
            let show = image_shown.map(|name| {
                let found = graph.image_named(name);
                show_missing(missing, name, found.is_some());
                ShowPass::plan(graph, found, size)
            });
            let (shown, tonemapper) = match &show {
                Some(plan) => (plan.shown(), plan.tonemapper(data.tonemapper)),
                None => (shown, data.tonemapper),
            };
            let read_ids = (data.read_picks).then(|| graph.read_ids(label!("read ids"), ids));
            // Cleared as a graph's own image is first written, though the pass writes every pixel.
            let color = graph.color("view", size);
            let tonemap = (graph.render(label!("tonemap")))
                .clear(color, data.clear_color)
                .sample(shown)
                .add();
            // Into the view's image, from the IDs the picks read, so that wherever the view shows,
            // and as it's read back, it shows outlined.
            let outline = (data.outline.as_ref().is_some_and(Outline::shows))
                .then(|| OutlinePass::plan(graph, color, ids, size));
            let read_view = (data.read_view).then(|| graph.read_image(label!("read view"), color));
            ViewPasses {
                skin: skin_plan,
                particles: particle_plan,
                cull,
                clusters,
                shadows,
                hdr,
                color,
                motion,
                first,
                second,
                shaded,
                early,
                fog,
                taa,
                smaa,
                keep,
                exposure,
                focus,
                blur,
                bloom,
                lens,
                canvas,
                show,
                shown,
                tonemap,
                tonemapper,
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
        let history = antialiased.then(|| self.history.targets());
        let kept = reflected.is_some().then(|| self.reflected.targets());
        // Which noise the occlusion takes, as the history says, before the frame takes its images.
        let occlusion_noise = self.ao_history.noise(unjittered);
        let accumulated = occluded.is_some().then(|| self.ao_history.targets());
        let lit = (plan.is_some()).then(|| self.sky_light.targets());
        let targets = iter::once(target)
            .chain(others)
            .chain(history.into_iter().flatten())
            .chain(kept.into_iter().flatten())
            .chain(accumulated.into_iter().flatten())
            .chain(lit.into_iter().flatten());
        let Some(mut frame) = self.frames.begin(targets, &self.graph)? else {
            // The births the frame planned aren't written.
            self.particles.forget();
            return Ok(false);
        };
        // The frame two before has finished, and what it read with it.
        self.picks.answer(frame.ids_read());
        let (finished, read) = frame.buffer_read(mesh::COUNTS);
        if let Some(&(_, counted, culled)) =
            (self.counted.iter()).find(|&&(number, ..)| number == finished && finished > 0)
        {
            self.drawn = counted;
            if culled && let Some(culled) = mesh::counts(read) {
                self.drawn += culled;
            }
        }
        // What the meter found in that one, if it metered the view, for the exposure of the view it
        // metered, the game's own or a look's; and what lit shading multiplies light by in this
        // frame, as the exposure of the view it renders adapts.
        for (adapting, name) in self.adapting.iter_mut().zip(exposure::METERED) {
            if let Some((metered, mean)) = exposure::metered(frame.buffer_read(name)) {
                adapting.meter(metered, mean);
            }
        }
        let adapting = &mut self.adapting[usize::from(data.looked)];
        let exposure = adapting.step(data.adaptation, data.exposure, number, data.time);
        self.exposed = exposure;
        debug_assert_eq!(number, frame.number(), "the frame begun is the one planned");
        self.meshes.upload(&mut frame, data.uploads)?;
        self.ui.write_textures(&mut frame)?;
        if let Some(smaa) = &mut self.smaa {
            smaa.write_textures(&mut frame)?;
        }
        // The atmosphere's tables, and where its data is, which its sky's draw and cube read too.
        let air = match (&atmosphere, &tables) {
            (Some(seen), Some(tables)) => {
                let pass = self
                    .atmosphere
                    .as_ref()
                    .expect("made for an atmosphere to show");
                let ways = data.viewpoint.ways_from_clip(data.viewport.aspect());
                Some(pass.record(&mut frame, tables, &seen.data(ways))?)
            }
            _ => None,
        };
        if let Some(plan) = &plan {
            let pass = self.sky.as_ref().expect("made for a sky to show");
            match (&tables, air) {
                (Some(tables), Some(air)) => {
                    let made = self
                        .atmosphere
                        .as_ref()
                        .expect("made for an atmosphere to show");
                    made.cube(&mut frame, plan, tables, air);
                }
                _ => pass.image_to_cube(&mut frame, plan),
            }
            pass.prefilter(&mut frame, plan);
        }
        // Whether the frame keeps the view's colors for the next frame's reflections, and
        // accumulates its ambient occlusion for the next frame's, which it notes once it's
        // submitted.
        let keeps = (view.as_ref()).is_some_and(|view| view.keep.is_some());
        let accumulates = (view.as_ref())
            .and_then(|view| view.early.as_ref())
            .is_some_and(|early| early.ao.is_some());
        // What the view shows, how much of its area that covers, what its draws that weren't
        // culled drew, and whether the frame culled any.
        let (counts, view, culled) = match (view, pixels_shown) {
            (Some(view), _) => {
                // The skinned meshes moved, and the particles born, before anything draws them.
                let (skinning, meshes) = (&self.skinning, &mut self.meshes);
                skinning.record(&mut frame, view.skin.as_ref(), data.draws, meshes)?;
                let emitted = (view.particles.as_ref())
                    .map(|plan| self.particles.record(&mut frame, plan, data.draws))
                    .transpose()?;
                // Facing the view, along its right and its up.
                let facing = data.viewpoint.rotation;
                let (right, up) = (facing * Vec3::X, facing * Vec3::Y);
                let born = emitted.map(|emitted| self.particles.draws(emitted, right, up));
                let born = born.as_ref();
                // Every pass's instances, copied once, culled for the view and the cascades.
                let uploaded = MeshPass::upload_draws(&mut frame, data.draws)?;
                let on_canvas = MeshPass::upload_draws(&mut frame, data.canvas)?;
                let culled = match &view.cull {
                    Some(plan) => {
                        let seen = CullView {
                            clip_from_world: view_projection,
                            size,
                            clamped: false,
                        };
                        let mut cascades = [seen; CASCADES];
                        let count = match &view.shadows {
                            Some(shadows) => {
                                for (i, cascade) in cascades.iter_mut().enumerate() {
                                    *cascade = CullView {
                                        clip_from_world: shadows.cascades.clip_from_world(i),
                                        size: [MAP_SIZE; 2],
                                        clamped: true,
                                    };
                                }
                                CASCADES
                            }
                            None => 0,
                        };
                        let cascades = &cascades[..count];
                        (self.meshes)
                            .cull(&mut frame, plan, data.draws, uploaded, seen, cascades)?
                    }
                    None => false,
                };
                // What a pass draws: what the culling's view in `slot` kept, or, if the frame
                // culls nothing, every run.
                let runs = |slot: usize| match &view.cull {
                    Some(plan) => Runs::Culled(plan, slot),
                    None => Runs::All,
                };
                let gpu_lights = self.lighting.lights();
                let lights = if gpu_lights.is_empty() {
                    0
                } else {
                    frame.upload(bytemuck::cast_slice(gpu_lights))?.address()
                };
                let shared = FrameData {
                    camera: data.viewpoint.position.as_vec3(),
                    time: data.time,
                    exposure,
                    light_count: gpu_lights.len() as u32,
                    lights,
                    materials: self.meshes.materials_address(),
                    eye: data.viewpoint.eye(),
                    sky: SkyData::default(),
                    shadows: ShadowData::default(),
                    clusters: ClusterData::default(),
                    ambient_occlusion: 0,
                    show: match data.show {
                        Some(Show::Shading(channel)) => channel.index(),
                        _ => 0,
                    },
                    fog: FogData::default(),
                    aerial: AerialData::default(),
                    reflections: ReflectionData::default(),
                };
                // What the casters and what's drawn into the visibility buffer read of the frame's
                // data, all but what the view alone samples.
                let partial = frame.upload(bytemuck::bytes_of(&shared))?.address();
                // The casters, each cascade's from the light.
                if let Some(shadows) = &view.shadows {
                    for (i, &cascade) in shadows.passes.iter().enumerate() {
                        let mut pass = (frame.render(cascade))
                            .expect("the graph's own images are always there");
                        let clip = shadows.cascades.clip_from_world(i);
                        let map = [MAP_SIZE; 2];
                        let draws = data.draws;
                        // Each cascade's slot is 1 past its index.
                        let cascade = runs(1 + i);
                        let way = Way::Shadows;
                        (self.meshes).draw(
                            &mut pass, clip, partial, map, draws, uploaded, cascade, way, None,
                        )?;
                    }
                }
                let aspect = data.viewport.aspect();
                let (binning, viewpoint) = (view.clusters.as_ref(), &data.viewpoint);
                let (view_lights, clusters) =
                    (self.lighting).bin(&mut frame, binning, lights, viewpoint, aspect);
                // The view's data, as the pass that shades it samples the sky's light, the
                // shadows' maps, the ambient occlusion and the reflections through the indices
                // `sampled` gives.
                let early = view.early.as_ref();
                let occlusion = early
                    .and_then(|early| early.ao.as_ref())
                    .map(ao::Plan::image);
                let reflections = early.and_then(|early| early.reflections.as_ref());
                let view_data = |sampled: &dyn Fn(ImageId) -> u32| {
                    let lit = match (&plan, sky) {
                        (Some(plan), Some((_, luminance))) => Some(plan.data(sampled, luminance)),
                        _ => held,
                    };
                    let shadows = (view.shadows.as_ref())
                        .map(|shadows| shadows.cascades.data(shadows.maps.map(sampled)));
                    let sky = lit.map(|sky| sky.luminance);
                    let aerial = atmosphere
                        .as_ref()
                        .zip(tables.as_ref())
                        .map(|(seen, tables)| {
                            let froxels = sampled(tables.aerial());
                            AerialData::new(unjittered, froxels, seen.illuminance())
                        });
                    let shared = FrameData {
                        lights: view_lights,
                        sky: lit.unwrap_or_default(),
                        shadows: shadows.unwrap_or_default(),
                        clusters,
                        ambient_occlusion: occlusion.map_or(0, sampled),
                        fog: FogData::new(data.fog, sky, shining, exposure),
                        aerial: aerial.unwrap_or_default(),
                        reflections: (reflections.zip(traced))
                            .map(|(plan, (settings, _))| plan.data(sampled, &settings))
                            .unwrap_or_default(),
                        ..shared
                    };
                    (shared, lit)
                };
                // The ambient occlusion's and the reflections' view, from its space to its clip
                // space, offset as the frame draws it; and the reflections', from its space to the
                // world.
                let clip_from_view = taa::jitter(jitter) * data.viewpoint.projection_matrix(aspect);
                let world_from_view = Mat4::from_rotation_translation(
                    data.viewpoint.rotation,
                    data.viewpoint.position.as_vec3(),
                );
                let occlusion_seen = occluded.map(|settings| ao::Seen {
                    clip_from_view,
                    world_from_view,
                    settings,
                    noise: occlusion_noise,
                });
                let reflections_seen =
                    traced.map(
                        |(settings, (kept_clip_from_world, kept))| reflections::Seen {
                            clip_from_view,
                            world_from_view,
                            kept_clip_from_world,
                            kept_scale: exposure::rescale(exposure, kept),
                            settings,
                        },
                    );
                // The occlusion and the reflections, those `early` has, the next of the frame's
                // passes.
                let (ao_pass, reflection_pass) = (&self.ao, &self.reflections);
                let record_early = |frame: &mut gg_gpu::Frame<'_>, early: &EarlyPasses| {
                    if let (Some(plan), Some(seen)) = (&early.ao, &occlusion_seen) {
                        ao_pass.record(frame, plan, seen)?;
                    }
                    if let (Some(plan), Some(seen)) = (&early.reflections, &reflections_seen) {
                        reflection_pass.record(frame, plan, seen)?;
                    }
                    Ok::<_, Error>(())
                };
                // The depth alone of the culled runs `runs` keeps, for the ambient occlusion and the
                // reflections, or their normals, as `way` says.
                let meshes = &self.meshes;
                let draw_alone = |pass: &mut gg_gpu::Pass<'_, '_>, runs: Runs<'_>, way| {
                    let (draws, at) = (data.draws, view_projection);
                    meshes.draw(pass, at, partial, size, draws, uploaded, runs, way, None)
                };
                let draw_depth =
                    |pass: &mut gg_gpu::Pass<'_, '_>, runs: Runs<'_>, beside_visibility| {
                        draw_alone(pass, runs, Way::Depth { beside_visibility })
                    };
                // Reflecting, in pass `pass`, the normals of the culled runs `plan` kept in both of
                // the view's passes whose depth alone was drawn, where they're nearest in it: all
                // of them, or, `beside_visibility`, those drawn otherwise than into it.
                let draw_normals =
                    |frame: &mut gg_gpu::Frame<'_>, pass, plan: &CullPlan, beside_visibility| {
                        let mut pass =
                            (frame.render(pass)).expect("the graph's own images are always there");
                        for slot in [FIRST, plan.second()] {
                            let way = Way::Normals { beside_visibility };
                            draw_alone(&mut pass, Runs::Culled(plan, slot), way)?;
                        }
                        Ok::<_, Error>(())
                    };
                let shades = view.shaded.as_ref().zip(view.cull.as_ref());
                // Beside a visibility buffer, what's drawn into it, in the view's two passes, then
                // the ambient occlusion and the reflections, if any, and the view's shading, which
                // the view's data is uploaded in, then the pass the rest draws in; or else, with
                // ambient occlusion or reflections, the depth of what the view's two passes keep,
                // the occlusion and the reflections, and the pass that draws all of it, shaded; or
                // else the view's first pass, which takes the culled runs it kept.
                let (mut pass, shared, frame_data, lit) = if let Some((shaded, plan)) = shades {
                    let mut pass = (frame.render(view.first))
                        .expect("the graph's own images are always there");
                    let (drawn, visible) = (data.draws, Way::Visibility);
                    let first = Runs::Culled(plan, FIRST);
                    (self.meshes).draw(
                        &mut pass,
                        view_projection,
                        partial,
                        size,
                        drawn,
                        uploaded,
                        first,
                        visible,
                        None,
                    )?;
                    drop(pass);
                    self.meshes.cull_second(&mut frame, plan)?;
                    let second = view.second.expect("a view that culls has a second pass");
                    let mut pass =
                        (frame.render(second)).expect("the graph's own images are always there");
                    let second = Runs::Culled(plan, plan.second());
                    (self.meshes).draw(
                        &mut pass,
                        view_projection,
                        partial,
                        size,
                        drawn,
                        uploaded,
                        second,
                        visible,
                        None,
                    )?;
                    drop(pass);
                    if let Some(early) = &view.early {
                        if let Some(rest) = early.depth {
                            let mut pass = (frame.render(rest))
                                .expect("the graph's own images are always there");
                            for slot in [FIRST, plan.second()] {
                                draw_depth(&mut pass, Runs::Culled(plan, slot), true)?;
                            }
                        }
                        if let Some((find, normals)) = early.normals {
                            let mut pass = frame.compute(find);
                            let view = Normals {
                                clip_from_world: view_projection,
                                size,
                                visibility: shaded.visibility,
                                normals,
                                frame: partial,
                            };
                            self.meshes.find_normals(&mut pass, plan, &view)?;
                        }
                        if let Some((drawn, _)) = early.drawn_normals {
                            draw_normals(&mut frame, drawn, plan, true)?;
                        }
                        record_early(&mut frame, early)?;
                    }
                    let mut shade = frame.compute(shaded.shade);
                    let (frame_data, lit) = view_data(&|image| shade.sampled(image));
                    let shared = shade.upload(bytemuck::bytes_of(&frame_data))?.address();
                    let seen = Shaded {
                        clip_from_world: view_projection,
                        clear: data.clear_color,
                        size,
                        visibility: shaded.visibility,
                        colors: view.hdr,
                        motion: view.motion,
                        frame: shared,
                    };
                    self.meshes.shade(&mut shade, plan, &seen)?;
                    drop(shade);
                    let pass = (frame.render(shaded.rest))
                        .expect("the graph's own images are always there");
                    (pass, shared, frame_data, lit)
                } else if let (Some(early), Some(plan)) = (&view.early, &view.cull) {
                    let mut pass = (frame.render(view.first))
                        .expect("the graph's own images are always there");
                    draw_depth(&mut pass, Runs::Culled(plan, FIRST), false)?;
                    drop(pass);
                    self.meshes.cull_second(&mut frame, plan)?;
                    let second = view.second.expect("a view that culls has a second pass");
                    let mut pass =
                        (frame.render(second)).expect("the graph's own images are always there");
                    draw_depth(&mut pass, Runs::Culled(plan, plan.second()), false)?;
                    drop(pass);
                    if let Some((drawn, _)) = early.drawn_normals {
                        draw_normals(&mut frame, drawn, plan, false)?;
                    }
                    record_early(&mut frame, early)?;
                    let shading = early.view.expect(
                        "the view draws after the occlusion and the reflections without a \
                         visibility buffer",
                    );
                    let mut pass =
                        (frame.render(shading)).expect("the graph's own images are always there");
                    let (frame_data, lit) = view_data(&|image| pass.sampled(image));
                    let shared = pass.upload(bytemuck::bytes_of(&frame_data))?.address();
                    (pass, shared, frame_data, lit)
                } else {
                    let mut pass = (frame.render(view.first))
                        .expect("the graph's own images are always there");
                    let (frame_data, lit) = view_data(&|image| pass.sampled(image));
                    let shared = pass.upload(bytemuck::bytes_of(&frame_data))?.address();
                    (pass, shared, frame_data, lit)
                };
                // Behind everything, in place of the clear color: the atmosphere's sky, or the
                // image sky; unless the view shows a part of shading in place of its light.
                let aspect = data.viewport.aspect();
                let draws_sky = !matches!(data.show, Some(Show::Shading(_)));
                if draws_sky
                    && let (Some(seen), Some(tables), Some(air)) = (&atmosphere, &tables, air)
                {
                    let made = self
                        .atmosphere
                        .as_ref()
                        .expect("made for an atmosphere to show");
                    let drawn = atmosphere::Drawn {
                        atmosphere: air,
                        transmittance: pass.sampled(tables.transmittance()),
                        sky_view: pass.sampled(tables.sky_view()),
                        scale: seen.illuminance() * exposure,
                        disk: seen.disk(),
                    };
                    made.draw(&mut pass, &data.viewpoint, aspect, jitter, &drawn);
                } else if draws_sky && let Some(SkyData { sky, luminance, .. }) = lit {
                    let drawn = self.sky.as_ref().expect("made for a sky to show");
                    let scale = luminance * exposure;
                    drawn.draw(&mut pass, &data.viewpoint, aspect, jitter, sky, scale);
                }
                let way = Way::Shaded {
                    beside_visibility: shades.is_some(),
                };
                // In fog, a view that culls nothing has nothing opaque to draw before the fog, but
                // the sky, as what it draws all blends.
                let mut counts = if view.fog.is_some() && view.cull.is_none() {
                    DrawCounts::default()
                } else {
                    (self.meshes).draw(
                        &mut pass,
                        view_projection,
                        shared,
                        size,
                        data.draws,
                        uploaded,
                        runs(FIRST),
                        way,
                        born,
                    )?
                };
                // What the first pass didn't draw, over it: after the depth pyramid, from the
                // first's depth, and the culling's second pass, against it, in a pass of its own;
                // or, beside a visibility buffer or after the ambient occlusion, in the same pass,
                // as its culling has run.
                if let (Some(plan), Some(second)) = (&view.cull, view.second) {
                    if shades.is_none() && view.early.is_none() {
                        drop(pass);
                        self.meshes.cull_second(&mut frame, plan)?;
                        pass = (frame.render(second))
                            .expect("the graph's own images are always there");
                    }
                    // In fog, what blends draws after the fog's pass.
                    let rest = if view.fog.is_some() {
                        Runs::Culled(plan, plan.second())
                    } else {
                        Runs::CulledThenRest(plan, plan.second())
                    };
                    counts += (self.meshes).draw(
                        &mut pass,
                        view_projection,
                        shared,
                        size,
                        data.draws,
                        uploaded,
                        rest,
                        way,
                        born,
                    )?;
                }
                // In fog, what's opaque fogged, the sky among it, and then what blends over it, in
                // a pass of its own, fogging itself.
                if let Some(fog) = &view.fog {
                    drop(pass);
                    self.fog
                        .record(&mut frame, &fog.plan, view_projection, shared);
                    pass = (frame.render(fog.blended))
                        .expect("the graph's own images are always there");
                    let rest = match &view.cull {
                        Some(plan) => Runs::Rest(plan),
                        None => Runs::All,
                    };
                    counts += (self.meshes).draw(
                        &mut pass,
                        view_projection,
                        shared,
                        size,
                        data.draws,
                        uploaded,
                        rest,
                        way,
                        born,
                    )?;
                }
                // The canvas, over the view's last pass's; or, after what follows that, over what
                // the view shows, in its own pass, if it has one; without the view's ambient
                // occlusion or reflections, which are of the depth the view's own passes draw, or
                // its fog or its atmosphere's haze, as it's drawn in the view's own space.
                let canvas_data = if frame_data.ambient_occlusion == 0
                    && view.fog.is_none()
                    && frame_data.aerial == AerialData::default()
                    && frame_data.reflections == ReflectionData::default()
                {
                    shared
                } else {
                    let unoccluded = FrameData {
                        ambient_occlusion: 0,
                        fog: FogData::default(),
                        aerial: AerialData::default(),
                        reflections: ReflectionData::default(),
                        ..frame_data
                    };
                    pass.upload(bytemuck::bytes_of(&unoccluded))?.address()
                };
                let draw_canvas = |pass: &mut gg_gpu::Pass<'_, '_>| {
                    pass.begin_label(c"canvas");
                    let canvas = canvas_projection(size);
                    let on = data.canvas;
                    let all = Runs::All;
                    let drawn = meshes.draw(
                        pass,
                        canvas,
                        canvas_data,
                        size,
                        on,
                        on_canvas,
                        all,
                        way,
                        None,
                    );
                    pass.end_label();
                    drawn
                };
                if view.taa.is_none() && view.smaa.is_none() && view.canvas.is_none() {
                    counts += draw_canvas(&mut pass)?;
                }
                drop(pass);
                let drawn = taa::Drawn {
                    unjittered,
                    clip_from_world: view_projection,
                    jitter,
                    exposure,
                };
                if let Some(plan) = &view.taa {
                    self.taa.resolve(&mut frame, plan, drawn)?;
                }
                if let Some(plan) = &view.smaa {
                    let smaa = self
                        .smaa
                        .as_ref()
                        .expect("made by the frame that planned it");
                    smaa.record(&mut frame, plan, data.tonemapper, &data.grade);
                }
                if let Some(keep) = &view.keep {
                    self.reflections.keep(&mut frame, keep);
                }
                if let (Some(plan), Some(adaptation)) = (&view.exposure, &data.adaptation) {
                    self.exposure
                        .record(&mut frame, plan, exposure, adaptation)?;
                }
                if let Some(plan) = &view.focus {
                    self.depth_of_field.record(&mut frame, plan);
                }
                if let Some(plan) = &view.blur {
                    self.motion_blur.record(&mut frame, plan, drawn, data.cut)?;
                }
                if let Some(plan) = &view.bloom {
                    self.bloom.record(&mut frame, plan, data.bloom, &data.grade);
                }
                if let Some(plan) = &view.lens {
                    self.lens.record(&mut frame, plan);
                }
                if let Some(canvas) = view.canvas {
                    let mut pass =
                        (frame.render(canvas)).expect("the graph's own images are always there");
                    counts += draw_canvas(&mut pass)?;
                }
                if let Some(plan) = &view.show {
                    let pass = self
                        .show
                        .as_ref()
                        .expect("made by the frame that planned it");
                    pass.record(&mut frame, plan);
                }
                if let Some(read) = view.read_ids {
                    let picked = self.picks.take(frame.number(), Some(size));
                    frame.read_ids(read, picked)?;
                }
                let mut pass =
                    (frame.render(view.tonemap)).expect("the graph's own images are always there");
                let shown = pass.sampled(view.shown);
                self.tonemap.draw(&mut pass, shown, view.tonemapper);
                drop(pass);
                if let (Some(plan), Some(outline)) = (&view.outline, data.outline) {
                    self.outline.record(&mut frame, plan, outline)?;
                }
                if let Some(read) = view.read_view {
                    self.view_read = frame.number();
                    frame.read_image(read)?;
                }
                (counts, Some((Shown::Image(view.color), size)), culled)
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
                    false,
                )
            }
            (None, None) => {
                if data.read_picks {
                    self.picks.take(frame.number(), None);
                }
                (DrawCounts::default(), None, false)
            }
        };
        // What the GPU counts of the frame comes back with the frame two later, which adds it.
        self.counted[(number % 2) as usize] = (number, counts, culled);
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
        if let Some(plan) = &plan {
            self.sky_light.prefiltered(plan);
        }
        if keeps {
            self.reflected.kept(number, size, unjittered, self.exposed);
        } else if data.show.is_some() && traced.is_some() {
            self.reflected.carry(number);
        }
        if accumulates {
            (self.ao_history).accumulated(number, size, view_projection, unjittered);
        }
        self.waited = self.frames.waited();
        Ok(true)
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
