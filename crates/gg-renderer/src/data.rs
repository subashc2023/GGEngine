//! The plain data a renderer is given each frame and at its start, and what it reports back.

use std::num::NonZeroU64;

use gg_core::math::DVec3;

use crate::{
    Adaptation, AmbientOcclusion, Antialiasing, Atmosphere, DepthOfField, Draws, Fog, Grade, Lens,
    Light, Materials, Meshes, MotionBlur, Outline, Pick, Reflections, Scene, Shaders, Show, Sky,
    Textures, Tonemapper, UploadPath, Viewpoint, mesh, model, shader, texture,
};
#[cfg(doc)]
use crate::{MAX_PICKS, Renderer, ViewCallback};

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
    /// the frame before, motion blur, takes it as standing still. A game's camera teleported isn't
    /// cut to: nothing tells the engine it jumped.
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
