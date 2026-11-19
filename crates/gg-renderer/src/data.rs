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
    /// The pixels of `rect`, in points of `pixels_per_point` pixels, rounded as egui rounds its
    /// clip rectangles.
    pub fn from_points(rect: epaint::Rect, pixels_per_point: f32) -> Self {
        let min = (rect.min.to_vec2() * pixels_per_point).round();
        let max = (rect.max.to_vec2() * pixels_per_point).round();
        // Casts saturate, so a rect without an area has none in pixels either.
        Self {
            origin: [min.x as u32, min.y as u32],
            size: [(max.x - min.x) as u32, (max.y - min.y) as u32],
        }
    }

    /// Width over height: the aspect ratio the game's view is projected with.
    pub fn aspect(&self) -> f32 {
        let [width, height] = self.size;
        width as f32 / height as f32
    }
}

/// What a frame drew in the game's view, as [`Renderer::drawn`] tells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    /// Instanced draws: one per run of consecutive draws of one mesh and shader with the same
    /// faces. A mesh with levels of detail counts one per level its run drew at, or, if it blends
    /// or is on the canvas, one per stretch in a row at a level. Plus one per emitter.
    pub draws: usize,
    /// Meshes drawn: one per draw pushed to [`Draws`] that culling kept, and one per slot of an
    /// emitter's ring, with a particle or not.
    pub instances: usize,
    /// Their triangles at the levels drawn, culled ones included: the GPU's work.
    pub triangles: usize,
}

impl std::ops::AddAssign for DrawCounts {
    fn add_assign(&mut self, other: Self) {
        self.draws += other.draws;
        self.instances += other.instances;
        self.triangles += other.triangles;
    }
}

/// What's been added to, replaced in or removed from the registries since last taken, and what's
/// changed in a [`Scene`].
///
/// The registries are [`Meshes`], [`Shaders`], [`Textures`] and [`Materials`], in order. Before it
/// draws, a frame makes the shaders' pipelines, queues the meshes and textures to stream, writes
/// the materials and the scene into their tables, and applies the removals.
///
/// Taking moves the changes out of the registries, which go on handing out handles, so a renderer
/// on another thread renders what they handed out before. It keeps its memory, so a frame that adds
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
    /// Takes what's changed in `meshes`, `shaders`, `textures` and `materials` since last taken,
    /// after what it holds.
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

    /// Takes what's changed in `scene` since last taken, after what it holds. A frame writes it to
    /// the GPU before drawing the scene, so one that doesn't render leaves it for the next.
    pub fn take_scene(&mut self, scene: &mut Scene) {
        self.scene.append(&mut scene.changes);
    }
}

/// The default [`Settings::upload_budget`]: 16 MiB, about a millisecond over PCIe, and one staging
/// allocation.
pub const DEFAULT_UPLOAD_BUDGET: u64 = 16 << 20;

/// What's streaming to the GPU, as [`Renderer::streaming`] tells: `assets`, the meshes and textures
/// queued, being copied, or copied but not yet drawable, and `bytes`, what's left to copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Streaming {
    pub assets: usize,
    pub bytes: u64,
}

impl Streaming {
    /// Whether every mesh and texture added is drawable or removed.
    pub fn is_empty(&self) -> bool {
        self.assets == 0
    }
}

/// What a frame draws besides UI: plain data the engine extracts each frame.
///
/// The view's passes run in this order, so a game's UI on the canvas shows none of those before it:
///
/// 1. `draws`: what's opaque, its fog, then what blends.
/// 2. Antialiasing.
/// 3. Reflections keep the colors, and the exposure is metered.
/// 4. Depth of field, then motion blur.
/// 5. Bloom and the grade.
/// 6. The lens.
/// 7. `canvas`.
/// 8. The tonemapper, then the outline.
pub struct RenderData<'a> {
    /// What the view clears to, tonemapped with what it draws, and the frame around it, as it is.
    /// Linear RGBA.
    pub clear_color: [f32; 4],
    /// How the view's HDR colors map into 0 to 1. [`Tonemapper::None`] clamps, showing that range
    /// as drawn; the others dither against banding in the view's 8 bits.
    pub tonemapper: Tonemapper,
    /// [`Antialiasing::Taa`] resolves each frame against the frames before if the last resolved the
    /// view at its size, and from this frame alone otherwise. [`Antialiasing::Smaa`] works on each
    /// frame alone. [`Antialiasing::None`] draws each pixel's center.
    pub antialiasing: Antialiasing,
    /// What lit shading multiplies light by, unless `adaptation` adapts it. Light in physical
    /// units, as lux and candela, times this is what the tonemapper maps. Unlit draws (sprites,
    /// text, unlit materials) keep their colors. Shaders read it from the frame's data
    /// ([`Shaders::add`]).
    pub exposure: f32,
    /// How the exposure adapts to what the view sees, as [`Adaptation`] says. `None` exposes every
    /// frame at `exposure`.
    ///
    /// - Each frame that renders the view meters it. The frame two later reads that back as it
    ///   begins, so none waits, and shades at the last frame's exposure moved toward the last
    ///   metering, by how far `time` moved.
    /// - Until a metering comes back, frames shade at `exposure`, within the bounds, then take the
    ///   metering's at once.
    /// - A change to the bounds, the compensation or the speeds applies in its frame. One to the
    ///   percentiles drops earlier meterings until one made with them comes back, taken at once.
    /// - Every reader takes the adapted exposure, shaders through the frame's data included.
    ///   Antialiasing's history and the colors reflections trace against are scaled by its change.
    pub adaptation: Option<Adaptation>,
    /// Whether the view is seen from where a tool looks (the tool API's look) rather than the
    /// game's camera. Exposure adapts apart for each, so a tool looking round a dark room doesn't
    /// expose the game's view, or a capture of it, for the room. A frame of the game's view while a
    /// tool looks, as a capture, takes the game's exposure as last adapted, moved toward its last
    /// metering.
    pub looked: bool,
    /// The view's time in seconds, which shaders read ([`Shaders::add`]).
    pub time: f32,
    /// The view's lights, which shaders read ([`Shaders::add`]). Without lights or a sky, models'
    /// materials draw unlit, by base color alone. The brightest directional light with a shadow
    /// distance casts shadows ([`Light::shadow_distance`]). Under an image sky with a sun, its sun
    /// lights in place of the directional lights, as [`Sky`] says.
    pub lights: &'a [Light],
    /// What the view shows behind the world, in place of the clear color, and is lit by from all
    /// around; shaders read it too. It applies from the first frame that renders the view once its
    /// texture's slot shows its own image, which that frame prefilters, and again at each new
    /// image. While the slot shows a stand-in, there's no sky and no sky light. A shining
    /// directional light stands for the sky's sun, which the sky's light then leaves out ([`Sky`]).
    pub sky: Option<Sky>,
    /// How much the view blooms, 0 (none) to 1: lerped toward a weighted mean of ever blurrier
    /// copies, which spreads light round what's bright without adding any. A view under 32 pixels
    /// on a side doesn't bloom.
    pub bloom: f32,
    /// How the view's light is graded ([`Grade`]). The identity costs nothing.
    pub grade: Grade,
    /// How ambient occlusion darkens the sky's light from all around ([`AmbientOcclusion`]).
    /// Computed only in a frame whose view a sky lights, through a perspective, where something
    /// opaque draws.
    pub ambient_occlusion: Option<AmbientOcclusion>,
    /// The view's height fog ([`Fog`]). A pass fogs what's opaque, the sky or clear color included,
    /// from the view's depth. What blends fogs itself as it draws: models, particles, sprites,
    /// circles and text, but not lines and grids, which are for seeing by, nor the canvas. `None`,
    /// or fog of no density, fogs nothing.
    pub fog: Option<Fog>,
    /// How the view reflects what it shows ([`Reflections`]), in place of the sky's specular light
    /// on what's opaque. Traced through the view's depth against the colors the frame before kept,
    /// antialiased. Only a frame whose view a sky lights, through a perspective, keeps colors; it
    /// traces too if the frame before kept them at its size and something opaque draws.
    pub reflections: Option<Reflections>,
    /// The atmosphere, shown behind the world in place of `sky` and lighting the view as an image
    /// sky would, under its sun: the brightest shining directional light, the first of equals.
    ///
    /// - Its tables are made each frame that renders the view. Its light is prefiltered again as
    ///   the sun turns, past a hundredth of a degree at the horizon to 1° high in the sky, or as
    ///   the viewer rises or falls 100 m, not as the sun brightens.
    /// - The sun's illuminance is taken outside the atmosphere. Lit shading gets it dimmed by the
    ///   transmittance from the viewer toward the sun, and none once it's set, when it casts no
    ///   shadows either.
    /// - What it lights is hazed by its distance from the viewer, in world units, meters.
    pub atmosphere: Option<Atmosphere>,
    /// The view's depth of field ([`DepthOfField`]): blurred by circles of confusion from its
    /// depth. Under a perspective alone, as without one there's no lens.
    pub depth_of_field: Option<DepthOfField>,
    /// The view's motion blur ([`MotionBlur`]): blurred by how far what it shows moved since the
    /// frame before.
    pub motion_blur: Option<MotionBlur>,
    /// What a camera's lens and film do to the view ([`Lens`]). [`Lens::NONE`] costs nothing.
    pub lens: Lens,
    /// Where the game's view shows in the frame. The meshes draw into the view's own image, at its
    /// size, which the frame shows there under the UI, or where the main window's UI paints a
    /// [`ViewCallback`].
    pub viewport: Viewport,
    /// What the view sees the world from.
    pub viewpoint: Viewpoint,
    /// What every position the frame draws is relative to, in `f32`, the viewpoint's included
    /// ([`Viewpoint::relative_to`]). A point near the viewpoint keeps what's near it as precise far
    /// from the world's origin as at it. Fog's height and the atmosphere's altitude are absolute.
    pub origin: DVec3,
    /// Whether the view cut to `viewpoint` since the frame before, as a tool's look or a camera
    /// switch does, rather than moved there. Motion blur then takes the camera as still. A game's
    /// camera teleported isn't a cut: nothing tells the engine it jumped.
    pub cut: bool,
    /// The meshes, shaders and textures added, replaced or removed since a frame last took them.
    /// The frame takes them first.
    pub uploads: &'a mut Uploads,
    /// What to draw.
    pub draws: &'a Draws,
    /// What draws after `draws` in the view's own space, as a game's UI does. Each transform takes
    /// a draw's space to the view's pixels, from its bottom left, +Y up, at its near plane, so each
    /// draw shows over everything before it.
    pub canvas: &'a Draws,
    /// At most [`MAX_PICKS`], read and answered as [`Pick`] says.
    pub picks: &'a [Pick],
    /// Whether the frame reads picks. One that doesn't, as one rendering another view than they
    /// were asked of, leaves `picks` and those waiting for the next that does, and still answers
    /// those the frame two before read.
    pub read_picks: bool,
    /// What the frame outlines in the view after reading the picks, if anything. It's drawn into
    /// the view's image, so the view read back shows it.
    pub outline: Option<Outline<'a>>,
    /// Whether the frame reads the view back, outline and all, for [`Renderer::view_read`].
    pub read_view: bool,
    /// Whether the frame lists the images [`Show::Image`] finds, for [`Renderer::images`].
    pub list_images: bool,
    /// What the view shows in place of its light, to see how it's made ([`Show`]). It's drawn
    /// without antialiasing's jitter, and isn't metered: exposure adapts on as it did.
    ///
    /// - A part of shading skips the passes after the view's draws (fog, depth of field, motion
    ///   blur, bloom, grade and lens) and the sky, cleared to black. It's tonemapped only where
    ///   it's light. The canvas draws over it.
    /// - An image comes from a frame drawn as the game's is but for antialiasing, so those passes
    ///   make their images. The canvas doesn't draw over it.
    /// - The frames after antialias afresh, and reflect what the last frame showing the view's
    ///   light kept.
    pub show: Option<Show<'a>>,
    /// Pixels the view shows in place of `draws`, as another process's game: each onto one of the
    /// frame's from the top left, as far as they reach, the rest the clear color. The frame then
    /// draws nothing in the view and answers its picks 0. Pixels to show again that no frame wrote
    /// at their size show nothing.
    pub pixels: Option<Pixels<'a>>,
}

/// Pixels for [`RenderData::pixels`].
///
/// It holds their width and height, and tightly packed rows of sRGB-encoded RGBA8, top row first.
/// `None` bytes show those the last rendering frame wrote, if it wrote some of this size.
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
    /// Upload meshes and textures this way rather than the GPU's best, to test or measure each. A
    /// way the GPU lacks keeps the renderer from starting (`gpu.no_upload_path`).
    pub uploads: Option<UploadPath>,
    /// Draw meshlets through mesh shaders or not, rather than as the GPU can, to test or measure
    /// each; `shaders/model.slang` says how. Mesh shaders the GPU lacks keep the renderer from
    /// starting (`gpu.no_mesh_shaders`).
    pub mesh_shaders: Option<bool>,
    /// Trace rays through ray queries or not, rather than as the GPU can, to test each: what
    /// [`Show::PathTraced`] needs. Ray queries the GPU lacks keep the renderer from starting
    /// (`gpu.no_ray_query`).
    pub ray_query: Option<bool>,
    /// Shade models' opaque and masked materials drawn by meshlets from a visibility buffer, as
    /// `shaders/visibility.slang` says, or, to test or measure each, as everything else draws.
    pub visibility_buffer: bool,
    /// The most bytes a frame copies to the GPU while streaming: [`DEFAULT_UPLOAD_BUDGET`] unless a
    /// test or measurement wants another.
    ///
    /// - A frame copies parts of the queue in order while under budget, cutting the part that would
    ///   cross it, but never below the smallest whole part (a row of a texture's level, or 16 bytes
    ///   of a buffer), so each frame progresses.
    /// - Through a transfer queue, a frame that begins with two or more earlier frames' copies
    ///   unfinished copies nothing. What's in flight stays within three frames' budgets however far
    ///   the frames outpace the copy engine.
    pub upload_budget: u64,
    /// Make every pipeline through a pipeline cache holding this, if [`Renderer::pipeline_cache`]
    /// gave it on the same GPU and driver: its pipelines are made in a fraction of the time. Other
    /// data starts it empty, as `Device::load_pipeline_cache` says. `None` makes pipelines without
    /// one.
    pub pipeline_cache: Option<&'a [u8]>,
    /// Capture this frame through RenderDoc, counting rendered frames from 1, and log the capture's
    /// path. RenderDoc's layer must be active, as under `renderdoccmd capture`, or the renderer
    /// doesn't start (`renderer.no_renderdoc`).
    pub capture: Option<NonZeroU64>,
}
