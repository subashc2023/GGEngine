//! A frame: which passes it runs, planned into its render graph, and their recording.

use std::iter;

use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result, label};
use gg_gpu::{
    ColorImage, ComputePass, Frame, Graph, ImageId, PassId, RenderPass, Target,
    Texture as GpuTexture, TextureFormat,
};
use tracing::warn;

use crate::ao::AoPass;
use crate::atmosphere::{AerialData, AtmospherePass};
use crate::bloom::BloomPass;
use crate::cluster::{ClusterData, ClusterPlan, Lighting};
use crate::depth_of_field::DepthOfFieldPass;
use crate::exposure::ExposurePass;
use crate::fog::{FogData, FogPass};
use crate::lens::LensPass;
use crate::mesh::{CullPlan, CullView, FIRST, FrameData, MeshPass, Normals, Runs, Shaded, Way};
use crate::motion_blur::MotionBlurPass;
use crate::particles::{ParticlePass, ParticlePlan};
use crate::pick::Picks;
use crate::reflections::{ReflectionData, ReflectionPass};
use crate::shadow::{self, CASCADES, Cascades, MAP_SIZE, ShadowData};
use crate::show::ShowPass;
use crate::skin::{SkinPlan, Skinning};
use crate::sky::{SkyData, SkyPass, Source};
use crate::smaa::SmaaPass;
use crate::taa::TaaPass;
use crate::tonemap::TonemapPass;
use crate::trace::{self, TracePass};
use crate::upscale::{self, UpscalePass};
use crate::view::{OutlinePass, OutlinePlan};
use crate::{
    AmbientOcclusion, Antialiasing, Channel, DrawCounts, Fog, FrameImage, Light, LightKind,
    MotionBlur, OFFSCREEN, Outline, Output, Pixels, Projection, Reflections, RenderData, Renderer,
    Show, Tonemapper, Ui, Viewport, ao, atmosphere, bloom, depth_of_field, exposure, fog, lens,
    mesh, motion_blur, reflections, show, sky, smaa, taa, ui,
};

/// Warns that the view shows image `name`, which the frame doesn't make, unless it's `found`.
/// `missing` names the last image warned of, so it warns once while frames go on asking.
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

/// From the canvas's space ([`RenderData::canvas`]) of a view `size` pixels across and down to
/// Vulkan's clip space, Y down: every point at the near plane, where reversed depth is 1, so every
/// depth test passes.
pub(crate) fn canvas_projection(size: [u32; 2]) -> Mat4 {
    let [width, height] = size.map(|side| side as f32);
    Mat4::from_cols(
        Vec4::new(2.0 / width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0 / height, 0.0, 0.0),
        Vec4::ZERO,
        Vec4::new(-1.0, 1.0, 1.0, 1.0),
    )
}

/// The game's view's images and passes in a frame's graph.
struct ViewPasses {
    skin: Option<SkinPlan>,
    particles: Option<ParticlePlan>,
    /// Culls the view's opaque runs and the shadows' casters.
    cull: Option<CullPlan>,
    clusters: Option<ClusterPlan>,
    shadows: Option<ShadowPasses>,
    /// What the view draws into, in HDR.
    hdr: ImageId,
    /// What it shows, tonemapped.
    color: ImageId,
    /// How far what's opaque in it moved in the world since the frame before.
    motion: ImageId,
    /// Draws what the culling's first pass keeps, or everything if it culls nothing; into the
    /// visibility buffer if there is one.
    first: PassId,
    /// Draws the rest over that, if it culls; into the visibility buffer if there is one.
    second: Option<PassId>,
    /// Shades the view from its visibility buffer, if it has one, and draws the rest over that.
    shaded: Option<ShadedPasses>,
    /// Ambient occlusion and reflections, if it has either.
    early: Option<EarlyPasses>,
    /// Fogs what's opaque, then draws what blends over it.
    fog: Option<FogPasses>,
    taa: Option<taa::Plan>,
    smaa: Option<smaa::Plan>,
    /// Keeps its colors, drawn and antialiased, for the next frame's reflections.
    keep: Option<reflections::Keep>,
    /// Meters it, if its exposure adapts.
    exposure: Option<exposure::Plan>,
    focus: Option<depth_of_field::Plan>,
    blur: Option<motion_blur::Plan>,
    /// Blooms and grades it, if it does either.
    bloom: Option<bloom::Plan>,
    lens: Option<lens::Plan>,
    /// Upscales it to its own size, if it renders below, and draws the canvas there.
    upscale: Option<Upscale>,
    /// Draws the canvas in its own pass if antialiasing, keeping for reflections, metering, blurs,
    /// bloom, the grade or a lens follow the view's last pass, and it isn't upscaled. Otherwise
    /// that pass, or the upscale's, draws it.
    canvas: Option<PassId>,
    /// Shows an image the frame makes in the view's place, or black for paths traced without ray
    /// queries.
    show: Option<show::Plan>,
    /// Traces paths in the view's place.
    trace: Option<trace::Plan>,
    /// What the tonemap pass samples: the view after all its passes, with its canvas, or an image
    /// shown in its place.
    shown: ImageId,
    tonemap: PassId,
    tonemapper: Tonemapper,
    read_ids: Option<PassId>,
    outline: Option<OutlinePlan>,
    read_view: Option<PassId>,
}

/// The game's view's upscaling to its own size, in a frame's graph: the pass, the colors and IDs it
/// samples, and those it draws.
struct Upscale {
    pass: PassId,
    sampled: [ImageId; 2],
    colors: ImageId,
    ids: ImageId,
}

/// The game's view's visibility buffer, in a frame's graph. The culled runs of the opaque and
/// masked model shaders draw into it by their meshlets, in the view's two passes. A compute pass
/// then shades the view from it, and the pass after draws everything else: the sky, culled runs
/// drawn otherwise, what blends and the canvas.
struct ShadedPasses {
    visibility: ImageId,
    shade: PassId,
    rest: PassId,
}

/// The game's view's ambient occlusion and reflections, in a frame's graph. They need the depth of
/// what's opaque before any of it shades.
///
/// - Beside a visibility buffer, which has its draws' depth, the culled runs drawn otherwise, if
///   any may be, draw their depth alone into an image of their own.
/// - Reflecting, a compute pass finds the visibility buffer's normals, as its shading turns them,
///   and the depth-only runs draw theirs where they're nearest. The rays are traced about these.
/// - Reflecting, the culled runs draw the depth of their far sides ([`Way::BackFaces`]).
/// - Without a visibility buffer, the view's two passes draw every culled run's depth alone, and a
///   pass after draws the view shaded.
struct EarlyPasses {
    depth: Option<PassId>,
    normals: Option<(PassId, ImageId)>,
    drawn_normals: Option<(PassId, ImageId)>,
    back: Option<(PassId, ImageId)>,
    ao: Option<ao::Plan>,
    reflections: Option<reflections::Plan>,
    view: Option<PassId>,
}

impl EarlyPasses {
    /// What the view's shading passes sample: the ambient occlusion and reflections the view has.
    fn sampled(&self) -> [Option<ImageId>; 3] {
        let reflected = (self.reflections.as_ref()).map(reflections::Plan::sampled);
        let [colors, surfaces] = [0, 1].map(|i| reflected.map(|images| images[i]));
        [self.ao.as_ref().map(ao::Plan::image), colors, surfaces]
    }
}

/// The game's view's fog, in a frame's graph: a pass fogging what's opaque, the sky or clear color
/// included, from the view's depth, then a pass drawing what blends, and the canvas where the
/// view's last pass draws it. What blends fogs itself, as a pass over the colors would fog it as
/// though it were as far as what's behind it.
struct FogPasses {
    plan: fog::Plan,
    blended: PassId,
}

/// The shadows' cascades, fitted to the view, their maps and the passes drawing them, before the
/// view's pass samples them.
struct ShadowPasses {
    cascades: Cascades,
    maps: [ImageId; CASCADES],
    passes: [PassId; CASCADES],
}

/// What the frame shows as the game's view: an image of its graph's, or a texture by index.
enum Shown {
    Image(ImageId),
    Texture(u32),
}

/// What a frame decided of the game's view before planning its passes. Each field is the local of
/// its name in [`Renderer::render_frame`].
#[derive(Clone, Copy)]
struct Viewing<'a> {
    /// What the view renders at before upscaling: `view_size` by the render scale.
    size: [u32; 2],
    /// The view's own size, which the canvas, outline and picks take.
    view_size: [u32; 2],
    /// Whether `size` is below `view_size`, so the view is upscaled before its canvas.
    upscaled: bool,
    number: u64,
    unjittered: Mat4,
    view_projection: Mat4,
    jitter: Vec2,
    antialiased: bool,
    smaa: bool,
    image_shown: Option<&'a str>,
    /// Whether the view shows paths traced: `Some(true)` on a device with ray queries.
    path_traced: Option<bool>,
    atmosphere: Option<atmosphere::Seen>,
    shining: &'a [Light],
    sky: Option<(Source, f32)>,
    tables: &'a Option<atmosphere::Plan>,
    plan: &'a Option<sky::Plan>,
    held: Option<SkyData>,
    fogged: bool,
    visibility_buffer: bool,
    occluded: Option<AmbientOcclusion>,
    occluded_against: Option<Mat4>,
    occlusion_size: [u32; 2],
    reflected: Option<Reflections>,
    traced: Option<(Reflections, (Mat4, f32))>,
    resolved: usize,
    reflections_first: usize,
    occlusion_first: usize,
}

/// The renderer's passes that record the game's view, borrowed apart from its frames, graph and
/// targets, as the frame being recorded holds those.
struct Recorder<'a> {
    meshes: &'a mut MeshPass,
    bloom: &'a BloomPass,
    tonemap: &'a TonemapPass,
    outline: &'a OutlinePass,
    fog: &'a FogPass,
    taa: &'a mut TaaPass,
    smaa: &'a Option<SmaaPass>,
    upscale: &'a Option<UpscalePass>,
    show: &'a Option<ShowPass>,
    trace: &'a mut Option<TracePass>,
    sky: &'a Option<SkyPass>,
    atmosphere: &'a Option<AtmospherePass>,
    view_read: &'a mut u64,
    picks: &'a mut Picks,
    lighting: &'a Lighting,
    skinning: &'a Skinning,
    particles: &'a mut ParticlePass,
    ao: &'a AoPass,
    reflections: &'a ReflectionPass,
    exposure: &'a ExposurePass,
    depth_of_field: &'a DepthOfFieldPass,
    motion_blur: &'a mut MotionBlurPass,
    lens: &'a LensPass,
}

/// `pass`, taking what `cull` keeps, if the frame culls.
fn culled<'g>(cull: Option<&CullPlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match cull {
        Some(cull) => cull.drawn(pass),
        None => pass,
    }
}

/// `pass`, reading the skinned meshes' vertices where the skin pass moved them, if it moved any.
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

fn sampling<'g>(images: impl Iterator<Item = ImageId>, pass: RenderPass<'g>) -> RenderPass<'g> {
    images.fold(pass, RenderPass::sample)
}

/// `pass`, reading the lights' clusters, if `clusters` bins them.
fn binned<'g>(clusters: Option<&ClusterPlan>, pass: RenderPass<'g>) -> RenderPass<'g> {
    match clusters {
        Some(clusters) => pass.read_buffer(clusters.buffer),
        None => pass,
    }
}

impl Renderer {
    pub(super) fn render_frame(
        &mut self,
        data: RenderData<'_>,
        ui: Ui<'_>,
        windows: &mut [(u64, Ui<'_>)],
    ) -> Result<bool> {
        let _zone = gg_core::zone!();
        let data = show::showing(data);
        // From here on, the world is relative to the frame's origin, as are its draws, the
        // viewpoint and fog's height. Only the atmosphere reads the viewpoint's altitude, the
        // world's.
        let altitude = data.viewpoint;
        // What's kept from the frame before, moved into this one's origin, before this frame's
        // changes to the scene; once, whether or not the frame renders.
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
        // The frame about to begin. One with nothing to render into begins none, and the next has
        // the same number.
        let number = self.frames.submitted() + 1;
        // Before the frame begins: a failure after would present it unrendered.
        self.meshes.add_shaders(data.uploads, number)?;
        // Every frame, rendered or not, so a viewpoint that can't project always panics. A view
        // without an area draws nothing, so its aspect ratio doesn't matter.
        let [width, height] = data.viewport.size;
        let unjittered = data.viewpoint.view_projection(data.viewport.aspect());
        // A view without an area renders nothing, nor does one showing pixels, which go into a
        // texture of their size, remade as that changes.
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
        // What the view renders at before it's upscaled to its own size.
        let rendered = upscale::rendered(data.viewport.size, data.render_scale);
        let upscaled = renders_view && rendered != data.viewport.size;
        if upscaled && self.upscale.is_none() {
            self.upscale = Some(UpscalePass::new(&self.device)?);
        }
        // Antialiased temporally, the view but its canvas is drawn off by the frame's offset, and
        // culled as drawn. By SMAA, it's drawn as it is.
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
        // Paths traced on a device with ray queries; else black, warned of once while frames ask.
        let samples = match data.show {
            Some(Show::PathTraced { samples }) if renders_view => Some(samples),
            _ => None,
        };
        let ray_query = self.device.ray_query();
        let path_traced = samples.map(|_| ray_query);
        if path_traced == Some(false) {
            if !self.no_ray_query {
                warn!(
                    code = "renderer.no_ray_query",
                    gpu = self.device.name(),
                    "the view shows paths traced, but the GPU has no ray queries: it shows black"
                );
                self.no_ray_query = true;
            }
            if self.show.is_none() {
                self.show = Some(ShowPass::new(&self.device)?);
            }
        } else {
            self.no_ray_query = false;
        }
        if path_traced == Some(true) && self.trace.is_none() {
            self.trace = Some(TracePass::new(&self.device)?);
        }
        let (jitter, view_projection) = if antialiased {
            self.history.prepare(&self.device, rendered)?;
            let jitter = taa::normalized(self.taa.offset(), rendered);
            (jitter, taa::jitter(jitter) * unjittered)
        } else {
            (Vec2::ZERO, unjittered)
        };
        if let Output::Offscreen(image, size) = &mut self.output
            && image.size() != *size
        {
            *image = ColorImage::new(&self.device, size[0], size[1], OFFSCREEN)?;
        }
        // The atmosphere shown in place of an image sky, if any.
        let atmosphere = (data.atmosphere.filter(|_| renders_view))
            .map(|atmosphere| atmosphere::Seen::new(atmosphere, data.lights, &altitude));
        // What the sky's light is prefiltered from, and what a texel of 1 stands for: the
        // atmosphere, of a sun of illuminance 1; or the image sky, while its slot shows its own
        // image, with that image's size and how many times the slot has changed image.
        let sky = match &atmosphere {
            Some(seen) => Some((Source::Atmosphere(seen.lit()), seen.illuminance())),
            None => data.sky.filter(|_| renders_view).and_then(|sky| {
                let (size, shown) = self.meshes.texture_shown(sky.texture)?;
                Some((Source::Image { sky, size, shown }, sky.luminance))
            }),
        };
        // The frame's lights: the atmosphere's sun dimmed by the air toward the viewer; or the
        // image sky's sun, once read back, in place of the directional lights, shadowing as far as
        // the brightest of them would.
        let image_sun = sky.and_then(|(source, luminance)| {
            let reach = shadow::caster(data.lights).map(|(_, _, distance)| distance);
            Some(self.sky_sun.of(&source)?.light(luminance, reach))
        });
        let shining = match (&atmosphere, image_sun) {
            (Some(seen), _) if seen.dims() => {
                self.sunlit.clear();
                self.sunlit.extend_from_slice(data.lights);
                seen.dim(&mut self.sunlit);
                &self.sunlit[..]
            }
            (_, Some(sun)) => {
                self.sunlit.clear();
                self.sunlit.push(sun);
                let others =
                    (data.lights.iter()).filter(|light| light.kind != LightKind::Directional);
                self.sunlit.extend(others);
                &self.sunlit[..]
            }
            _ => data.lights,
        };
        if sky.is_some() && self.sky.is_none() {
            self.sky = Some(SkyPass::new(&self.device)?);
        }
        if atmosphere.is_some() && self.atmosphere.is_none() {
            self.atmosphere = Some(AtmospherePass::new(&self.device)?);
        }
        // Ambient occlusion darkens only light from all around, which only a sky gives, under a
        // perspective, whose depth it reads.
        let perspective = matches!(data.viewpoint.projection, Projection::Perspective { .. });
        let occluded = (data.ambient_occlusion)
            .filter(|occlusion| occlusion.darkens() && sky.is_some() && perspective);
        // Averaged over frames in a history of the occlusion's size, let go by a frame rendering
        // the view without it. Against what the frame before accumulated, if it did at this size
        // and the view isn't cut.
        let occlusion_size = ao::occlusion_size(rendered, self.half_size_occlusion);
        if occluded.is_some() {
            self.ao_history.prepare(&self.device, occlusion_size)?;
        } else if renders_view {
            self.ao_history.forget();
        }
        let occluded_against = (self.ao_history).against(number, rendered, data.cut);
        // Under a perspective, reflections trace through its depth against the colors the frame
        // before kept. Each reflecting frame keeps them for the next, in images of the view's size,
        // let go by a frame rendering the view without reflections.
        let reflected = (data.reflections)
            .filter(|reflections| reflections.reflects() && sky.is_some() && perspective);
        if reflected.is_some() {
            self.reflected.prepare(&self.device, rendered)?;
        } else if renders_view {
            self.reflected.forget();
        }
        // Against the colors the frame before kept, if it kept them at this size, and how it drew
        // them.
        let traced = reflected.zip(self.reflected.traced_against(number, rendered));
        if renders_view {
            self.lighting.order(shining, &data.viewpoint);
        } else {
            // The pool goes undeclared, which the graph lets go of.
            self.particles.forget();
        }
        // The frame's passes. The view's images are its own, at its size, so it never shows
        // stretched. A resizing drag remakes them every frame, which growing them in steps and
        // rendering into part of them wouldn't.
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
        // The antialiasing's resolve images are the targets after the windows'.
        let resolved = 1 + windows_drawn;
        let resolves = if antialiased { taa::TARGETS } else { 0 };
        // The reflections' images are the targets after those, then the ambient occlusion's
        // history's.
        let reflections_first = resolved + resolves;
        let reflects = if reflected.is_some() { 3 } else { 0 };
        let occlusion_first = reflections_first + reflects;
        let occludes = if occluded.is_some() { 2 } else { 0 };
        // First the atmosphere's tables, if the frame shows one, which the view samples and its
        // sky's cube is made from.
        let tables = atmosphere.as_ref().map(|_| AtmospherePass::plan(graph));
        // Then the sky's prefiltering, in the first frame to render the view since: its texture
        // came to show its image, or it turned; or, for the atmosphere, its sun turned or the
        // viewer rose or fell past what the light holds. Its images are the targets after those.
        // Other frames reuse the light.
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
        let viewing = Viewing {
            size: rendered,
            view_size: size,
            upscaled,
            number,
            unjittered,
            view_projection,
            jitter,
            antialiased,
            smaa,
            image_shown,
            path_traced,
            atmosphere,
            shining,
            sky,
            tables: &tables,
            plan: &plan,
            held,
            fogged,
            visibility_buffer,
            occluded,
            occluded_against,
            occlusion_size,
            reflected,
            traced,
            resolved,
            reflections_first,
            occlusion_first,
        };
        let view = renders_view.then(|| {
            plan_view(
                graph, meshes, skinning, particles, lighting, taa, missing, images, &data, &viewing,
            )
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
        // What the meter found in that frame, for the view it metered, the game's or a look's; and
        // what lit shading multiplies light by in this frame, as its view's exposure adapts.
        for (adapting, name) in self.adapting.iter_mut().zip(exposure::METERED) {
            if let Some((metered, mean)) = exposure::metered(frame.buffer_read(name)) {
                adapting.meter(metered, mean);
            }
        }
        // The image sky's sun that frame found, which lights from the next frame on.
        if let (Some(sun), Some((_, luminance))) = (self.sky_sun.read(&frame), sky) {
            sky::report(sun, luminance, data.lights);
        }
        let adapting = &mut self.adapting[usize::from(data.looked)];
        let exposure = adapting.step(data.adaptation, data.exposure, number, data.time);
        self.exposed = exposure;
        debug_assert_eq!(number, frame.number(), "the frame begun is the one planned");
        self.meshes.upload(&mut frame, data.uploads)?;
        if let (Some(samples), Some(true), Some(trace)) = (samples, path_traced, &mut self.trace) {
            let seen = trace::Seen {
                number,
                size,
                clip_from_world: unjittered,
                lights: bytemuck::cast_slice(self.lighting.lights()),
                sky: sky.map(|(_, luminance)| luminance),
                prefiltered: plan.is_some(),
                exposure,
                uploaded: self.meshes.uploaded(),
                samples,
            };
            trace.prepare(&mut frame, &self.meshes, data.draws, &seen)?;
        }
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
                _ => pass.image_to_cube(&mut frame, plan)?,
            }
            pass.prefilter(&mut frame, plan);
        }
        // Whether the frame keeps the view's colors for the next frame's reflections and
        // accumulates its ambient occlusion for the next's, noted once it's submitted.
        let keeps = (view.as_ref()).is_some_and(|view| view.keep.is_some());
        let accumulates = (view.as_ref())
            .and_then(|view| view.early.as_ref())
            .is_some_and(|early| early.ao.is_some());
        // What the view shows, the area it covers, what its unculled draws drew, and whether the
        // frame culled any.
        let (counts, view, culled) = match (view, pixels_shown) {
            (Some(view), _) => {
                let color = view.color;
                let recorder = Recorder {
                    meshes: &mut self.meshes,
                    bloom: &self.bloom,
                    tonemap: &self.tonemap,
                    outline: &self.outline,
                    fog: &self.fog,
                    taa: &mut self.taa,
                    smaa: &self.smaa,
                    upscale: &self.upscale,
                    show: &self.show,
                    trace: &mut self.trace,
                    sky: &self.sky,
                    atmosphere: &self.atmosphere,
                    view_read: &mut self.view_read,
                    picks: &mut self.picks,
                    lighting: &self.lighting,
                    skinning: &self.skinning,
                    particles: &mut self.particles,
                    ao: &self.ao,
                    reflections: &self.reflections,
                    exposure: &self.exposure,
                    depth_of_field: &self.depth_of_field,
                    motion_blur: &mut self.motion_blur,
                    lens: &self.lens,
                };
                let (counts, culled) = recorder.record_view(
                    &mut frame,
                    view,
                    &data,
                    &viewing,
                    exposure,
                    air,
                    occlusion_noise,
                )?;
                (counts, Some((Shown::Image(color), size)), culled)
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
        // The GPU's counts come back with the frame two later, which adds them.
        self.counted[(number % 2) as usize] = (number, counts, culled);
        self.traced = (path_traced == Some(true))
            .then(|| self.trace.as_ref().map(TracePass::taken))
            .flatten();
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
            self.sky_sun.prefiltered(plan, number);
        }
        if keeps {
            self.reflected
                .kept(number, rendered, unjittered, self.exposed);
        } else if data.show.is_some() && traced.is_some() {
            self.reflected.carry(number);
        }
        if accumulates {
            (self.ao_history).accumulated(number, rendered, view_projection, unjittered);
        }
        self.waited = self.frames.waited();
        Ok(true)
    }
}

/// Plans the game's view's passes into `graph`, as `viewing` says the frame renders the view.
#[expect(
    clippy::too_many_arguments,
    reason = "the renderer's parts it plans by, each borrowed apart"
)]
fn plan_view(
    graph: &mut Graph,
    meshes: &MeshPass,
    skinning: &mut Skinning,
    particles: &mut ParticlePass,
    lighting: &Lighting,
    taa: &TaaPass,
    missing: &mut Option<String>,
    images: &mut Vec<FrameImage>,
    data: &RenderData<'_>,
    viewing: &Viewing<'_>,
) -> ViewPasses {
    let Viewing {
        size,
        view_size,
        upscaled,
        number,
        antialiased,
        smaa,
        image_shown,
        path_traced,
        atmosphere,
        shining,
        tables,
        plan,
        fogged,
        visibility_buffer,
        occluded,
        occluded_against,
        occlusion_size,
        reflected,
        traced,
        resolved,
        reflections_first,
        occlusion_first,
        ..
    } = *viewing;
    // Before the view's first pass: the skinning and the particles' births, which the drawing
    // passes read; the culling of the view's opaque runs and the cascades' casters; the shadows;
    // then the lights' binning. The culling and the binning run on the compute queue where there's
    // one, the binning beside the shadows.
    let skin_plan = skinning.plan(graph, data.draws, meshes);
    let skin = skin_plan.as_ref();
    let particle_plan = particles.plan(graph, data.draws);
    let born = particle_plan.as_ref();
    let aspect = data.viewport.aspect();
    // From the lights as they shine, an image sky's sun among them, but outside a risen
    // atmosphere, so its sun casts shadows while it's up however dim, and none once it's set.
    let casters = match &atmosphere {
        Some(seen) if !seen.set() => data.lights,
        _ => shining,
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
    // Drawn every frame, antialiased or not, as the IDs are, so one pipeline draws each shader:
    // ARCHITECTURE.md's Rendering says why.
    let motion = graph.motion("view motion", size);
    let depth = graph.depth("view depth", size);
    // What the draws light by: the sky's light as its prefiltering frame left it, the atmosphere's
    // tables, and the shadows.
    let lit = || {
        let maps = shadows.iter().flat_map(|shadows| shadows.maps);
        let tables = tables.iter().flat_map(atmosphere::Plan::sampled);
        plan.iter()
            .flat_map(sky::Plan::sampled)
            .chain(tables)
            .chain(maps)
    };
    // A view pass named `label`, drawing over those before it and shading as it draws, with all
    // that needs: `kept`'s culled runs, the skinned vertices, the particles' pool, the lights'
    // clusters, what the draws light by, and `early`'s ambient occlusion and reflections.
    let shading =
        |graph: &mut Graph, label, kept: Option<&CullPlan>, early: [Option<ImageId>; 3]| {
            let pass = particled(born, culled(kept, skinned(skin, graph.render(label))));
            let sampled = lit().chain(early.into_iter().flatten());
            let pass = binned(clusters.as_ref(), sampling(sampled, pass));
            pass.keep(hdr).keep(ids).keep(motion).keep(depth).add()
        };
    let visibility = (cull.as_ref())
        .filter(|cull| visibility_buffer && cull.by_meshlets())
        .map(|_| graph.visibility("view visibility", size));
    let early = cull.is_some() && (occluded.is_some() || traced.is_some());
    let by_indices = (cull.as_ref()).is_some_and(CullPlan::by_indices);
    let opaque_depth =
        (early && (visibility.is_none() || by_indices)).then(|| graph.depth("opaque depth", size));
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
    // Where the view culls, its second pass, over its first: once the depth pyramid is built from
    // the first's depth and the culling's second pass has culled against it, what that kept, and,
    // without a visibility buffer, the runs it doesn't cull.
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
    let early = early.then(|| {
        let rest = visibility.zip(opaque_depth).map(|(_, opaque_depth)| {
            let rest = skinned(skin, graph.render(label!("opaque depth")));
            culled(cull.as_ref(), rest).depth(opaque_depth).add()
        });
        let normals = (visibility.filter(|_| traced.is_some()).zip(cull.as_ref())).map(
            |(visibility, cull)| {
                let normals = graph.normals("view normals", size);
                let find = graph.compute(label!("view normals")).sample(visibility);
                (cull.shaded(find).overwrite(normals).add(), normals)
            },
        );
        let drawn_normals = opaque_depth
            .filter(|_| traced.is_some())
            .map(|opaque_depth| {
                let normals = graph.normals("opaque normals", size);
                let draw = skinned(skin, graph.render(label!("opaque normals")));
                let draw = culled(cull.as_ref(), draw).clear(normals, [0.0; 4]);
                (draw.keep(opaque_depth).add(), normals)
            });
        let back = traced.is_some().then(|| {
            let back = graph.depth("opaque back depth", size);
            let draw = skinned(skin, graph.render(label!("opaque back faces")));
            (culled(cull.as_ref(), draw).depth(back).add(), back)
        });
        let alone = opaque_depth.unwrap_or(depth);
        let depths = [if visibility.is_some() { depth } else { alone }, alone];
        let ao = occluded.map(|_| {
            let against = occluded_against;
            AoPass::plan(
                graph,
                depths,
                size,
                occlusion_size,
                occlusion_first,
                number,
                against,
            )
        });
        // Each depth's, as they pair.
        let found = [normals, drawn_normals].map(|pair| pair.map(|(_, normals)| normals));
        // The back faces are drawn whenever the view traces reflections.
        let reflections = back.map(|(_, back)| {
            ReflectionPass::plan(graph, reflections_first, depths, back, found, size)
        });
        let mut early = EarlyPasses {
            depth: rest,
            normals,
            drawn_normals,
            back,
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
        let shade =
            (lit().chain(sampled_early.into_iter().flatten())).fold(shade, ComputePass::sample);
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
    let fog = fogged.then(|| {
        let sky_light = plan.iter().flat_map(sky::Plan::sampled);
        FogPasses {
            plan: FogPass::plan(graph, size, depth, hdr, sky_light),
            blended: shading(graph, label!("view blended"), None, [None; 3]),
        }
    });
    // The passes after the draws, in `RenderData`'s order, and what each writes into:
    //
    // - TAA: one of its images, and the view's colors apart if what follows writes over them in
    //   place, as the next frame resolves against what the resolve alone wrote. SMAA: colors of its
    //   own, with no history.
    // - Depth of field: those colors. Motion blur and the lens: their own, as they read the pixels
    //   round each.
    // - Bloom and the grade: the colors the view shows.
    // - The canvas: its own pass over what the view shows, unless the view's last pass draws it.
    //
    // Reflections keep the colors before all of these, so a reflection holds none of them. Depth of
    // field and motion blur come before bloom, as Unreal orders them.
    let blooms = bloom::blooms(data.bloom, size);
    let grades = !data.grade.is_identity();
    let circle = (data.depth_of_field)
        .and_then(|focus| depth_of_field::Circle::new(focus, &data.viewpoint, size));
    let blur = data.motion_blur.filter(MotionBlur::blurs);
    let lensed = data.lens.shows();
    // Where the antialiasing's images start among the graph's own. A frame showing an image, drawn
    // without antialiasing, doesn't make them, so the image list leaves them out and offers only
    // what a show finds.
    let unlisted_from = data.list_images.then(|| graph.images().count());
    let taa = antialiased.then(|| {
        // Apart where what follows writes the resolved colors in place: depth of field; or bloom,
        // the grade or the canvas, unless motion blur or the lens wrote colors of their own before
        // them.
        let over = blooms || grades || (!lensed && !upscaled && !data.canvas.is_empty());
        let apart = circle.is_some() || (blur.is_none() && over);
        let drawn = [hdr, depth, motion];
        taa.plan(graph, resolved, number, size, drawn, apart)
    });
    let smaa = smaa.then(|| SmaaPass::plan(graph, hdr, size));
    let unlisted = unlisted_from.map_or(0..0, |from| from..graph.images().count());
    let shown = taa.as_ref().map_or(hdr, taa::Plan::shown);
    let shown = smaa.as_ref().map_or(shown, smaa::Plan::shown);
    // A frame showing something in place of the view's light keeps nothing. The next reflects what
    // the last frame showing that light kept.
    let keep = (reflected)
        .filter(|_| data.show.is_none())
        .map(|_| ReflectionPass::plan_keep(graph, reflections_first, shown, size));
    // Metered as resolved, before the camera's passes, bloom and the canvas, whose UI isn't lit:
    // blurring keeps about the mean of what it blurs, and a lens's vignette and grain shouldn't
    // move the exposure.
    let looked = data.looked;
    let metered = data.adaptation.is_some() && data.show.is_none();
    let exposure = metered.then(|| ExposurePass::plan(graph, shown, size, looked));
    let focus = circle.map(|circle| DepthOfFieldPass::plan(graph, circle, shown, depth, size));
    let blur = blur.map(|blur| MotionBlurPass::plan(graph, blur, [shown, depth, motion], size));
    let shown = blur.as_ref().map_or(shown, motion_blur::Plan::blurred);
    let bloom = (blooms || grades).then(|| BloomPass::plan(graph, shown, size, blooms, grades));
    let lens = lensed.then(|| LensPass::plan(graph, data.lens, shown, size));
    let shown = lens.as_ref().map_or(shown, lens::Plan::lensed);
    // Upscaled to the view's size, with IDs, motion and depth of its own for the canvas, which
    // draws in the same pass.
    let upscale = upscaled.then(|| {
        let colors = graph.hdr("view upscaled", view_size);
        let ids_upscaled = graph.ids("view ids upscaled", view_size);
        let motion = graph.motion("view motion upscaled", view_size);
        let depth = graph.depth("view depth upscaled", view_size);
        let pass = graph.render(label!("upscale")).sample(shown).sample(ids);
        let pass = if data.canvas.is_empty() {
            pass
        } else {
            binned(clusters.as_ref(), sampling(lit(), pass))
        };
        let pass = pass.clear(colors, data.clear_color).ids(ids_upscaled);
        Upscale {
            pass: pass.motion(motion).depth(depth).add(),
            sampled: [shown, ids],
            colors,
            ids: ids_upscaled,
        }
    });
    let (shown, ids) = upscale
        .as_ref()
        .map_or((shown, ids), |up| (up.colors, up.ids));
    let after = taa.is_some()
        || smaa.is_some()
        || keep.is_some()
        || exposure.is_some()
        || focus.is_some()
        || blur.is_some()
        || bloom.is_some()
        || lens.is_some();
    let canvas = (!data.canvas.is_empty() && after && !upscaled).then(|| {
        let canvas = graph.render(label!("canvas"));
        let canvas = binned(clusters.as_ref(), sampling(lit(), canvas));
        (canvas.keep(shown).keep(ids).keep(motion).keep(depth)).add()
    });
    if data.list_images {
        show::list(images, graph, unlisted);
    }
    // An image the frame makes in place of all that, the last of its name added so far; or paths
    // traced, sampling the sky's light where the frame prefilters it, or black without ray queries.
    let show = match (image_shown, path_traced) {
        (Some(name), _) => {
            let found = graph.image_named(name);
            show_missing(missing, name, found.is_some());
            Some(ShowPass::plan(graph, found, view_size))
        }
        (None, Some(false)) => Some(ShowPass::plan(graph, None, view_size)),
        (None, _) => None,
    };
    let trace = (path_traced == Some(true)).then(|| {
        let sky = plan.iter().flat_map(sky::Plan::sampled);
        TracePass::plan(graph, view_size, sky)
    });
    let (shown, tonemapper) = match (&show, &trace) {
        (Some(plan), _) => (plan.shown(), plan.tonemapper(data.tonemapper)),
        (None, Some(plan)) => (plan.shown(), data.tonemapper),
        (None, None) => (shown, data.tonemapper),
    };
    let read_ids = (data.read_picks).then(|| graph.read_ids(label!("read ids"), ids));
    // Cleared as a graph's own image is first written, though the pass writes every pixel.
    let color = graph.color("view", view_size);
    let tonemap = (graph.render(label!("tonemap")))
        .clear(color, data.clear_color)
        .sample(shown)
        .add();
    // Into the view's image, from the picks' IDs, so the view shows outlined wherever it shows and
    // as it's read back.
    let outline = (data.outline.as_ref().is_some_and(Outline::shows))
        .then(|| OutlinePass::plan(graph, color, ids, view_size));
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
        upscale,
        canvas,
        show,
        trace,
        shown,
        tonemap,
        tonemapper,
        read_ids,
        outline,
        read_view,
    }
}

impl Recorder<'_> {
    /// Records `view`'s passes into `frame`, as `viewing` says the frame renders the view. Returns
    /// what its unculled draws drew, and whether the frame culled any.
    #[expect(
        clippy::too_many_arguments,
        reason = "what recording the view takes, each apart"
    )]
    fn record_view(
        mut self,
        frame: &mut Frame<'_>,
        view: ViewPasses,
        data: &RenderData<'_>,
        viewing: &Viewing<'_>,
        exposure: f32,
        air: Option<u64>,
        occlusion_noise: u32,
    ) -> Result<(DrawCounts, bool)> {
        let Viewing {
            size,
            view_size,
            unjittered,
            view_projection,
            jitter,
            atmosphere,
            shining,
            sky,
            tables,
            plan,
            held,
            occluded,
            traced,
            ..
        } = *viewing;
        // The skinned meshes moved, and the particles born, before anything draws them.
        let (skinning, meshes) = (&self.skinning, &mut self.meshes);
        skinning.record(frame, view.skin.as_ref(), data.draws, meshes)?;
        let emitted = (view.particles.as_ref())
            .map(|plan| self.particles.record(frame, plan, data.draws))
            .transpose()?;
        // Facing the view, along its right and its up.
        let facing = data.viewpoint.rotation;
        let (right, up) = (facing * Vec3::X, facing * Vec3::Y);
        let born = emitted.map(|emitted| self.particles.draws(emitted, right, up));
        let born = born.as_ref();
        // Every pass's instances, copied once, culled for the view and the cascades.
        let uploaded = MeshPass::upload_draws(frame, data.draws)?;
        let on_canvas = MeshPass::upload_draws(frame, data.canvas)?;
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
                (self.meshes).cull(frame, plan, data.draws, uploaded, seen, cascades)?
            }
            None => false,
        };
        // What a pass draws: what the culling's view in `slot` kept, or every run if the frame
        // culls nothing.
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
        // What the casters and the visibility buffer's draws read of the frame's data: all but what
        // the view alone samples.
        let partial = frame.upload(bytemuck::bytes_of(&shared))?.address();
        // The casters, each cascade's from the light.
        if let Some(shadows) = &view.shadows {
            for (i, &cascade) in shadows.passes.iter().enumerate() {
                let mut pass =
                    (frame.render(cascade)).expect("the graph's own images are always there");
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
            (self.lighting).bin(frame, binning, lights, viewpoint, aspect);
        // The view's data, as its shading pass samples the sky's light, the shadows' maps, the
        // ambient occlusion and the reflections through `sampled`'s indices.
        let early = view.early.as_ref();
        let occlusion = early
            .and_then(|early| early.ao.as_ref())
            .map(ao::Plan::image);
        let reflections = early.and_then(|early| early.reflections.as_ref());
        let sky_data = |sampled: &dyn Fn(ImageId) -> u32| match (&plan, sky) {
            (Some(plan), Some((_, luminance))) => Some(plan.data(sampled, luminance)),
            _ => held,
        };
        let view_data = |sampled: &dyn Fn(ImageId) -> u32| {
            let lit = sky_data(sampled);
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
        // The path tracer's: of what the view samples, the sky's light alone, which its pass
        // samples. Paths find shadows, occlusion and reflections by their rays, and leave the haze
        // out. The lights as uploaded, not as the binning pass copies them, which it doesn't wait
        // for.
        let traced_data = |sampled: &dyn Fn(ImageId) -> u32| FrameData {
            sky: sky_data(sampled).unwrap_or_default(),
            ..shared
        };
        // The ambient occlusion's and reflections' view-to-clip matrix, off by the frame's offset
        // as it's drawn, and the reflections' view-to-world.
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
                    rays: matches!(data.show, Some(Show::Shading(Channel::ReflectionRays))),
                },
            );
        // The occlusion and reflections `early` has: the frame's next passes.
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
        // The depth alone of the culled runs `runs` keeps, for the ambient occlusion and
        // reflections, or their normals, as `way` says.
        let meshes = &self.meshes;
        let draw_alone = |pass: &mut gg_gpu::Pass<'_, '_>, runs: Runs<'_>, way| {
            let (draws, at) = (data.draws, view_projection);
            meshes.draw(pass, at, partial, size, draws, uploaded, runs, way, None)
        };
        let draw_depth = |pass: &mut gg_gpu::Pass<'_, '_>, runs: Runs<'_>, beside_visibility| {
            draw_alone(pass, runs, Way::Depth { beside_visibility })
        };
        // Reflecting, in `pass`, the culled runs `plan` kept in both view passes, as `way` draws
        // them: their normals or their far sides' depth.
        let draw_kept = |frame: &mut gg_gpu::Frame<'_>, pass, plan: &CullPlan, way| {
            let mut pass = (frame.render(pass)).expect("the graph's own images are always there");
            for slot in [FIRST, plan.second()] {
                draw_alone(&mut pass, Runs::Culled(plan, slot), way)?;
            }
            Ok::<_, Error>(())
        };
        // The normals of the culled runs whose depth alone was drawn, where they're nearest: all of
        // them, or, `beside_visibility`, those drawn other than into it; then the far sides.
        let draw_reflected = |frame: &mut gg_gpu::Frame<'_>,
                              early: &EarlyPasses,
                              plan: &CullPlan,
                              beside_visibility| {
            if let Some((drawn, _)) = early.drawn_normals {
                draw_kept(frame, drawn, plan, Way::Normals { beside_visibility })?;
            }
            if let Some((back, _)) = early.back {
                draw_kept(frame, back, plan, Way::BackFaces)?;
            }
            Ok::<_, Error>(())
        };
        let shades = view.shaded.as_ref().zip(view.cull.as_ref());
        // Beside a visibility buffer, as ShadedPasses says; else with ambient occlusion or
        // reflections, as EarlyPasses says; else the view's first pass alone, taking the culled
        // runs it kept.
        let (mut pass, shared, frame_data, lit) = if let Some((shaded, plan)) = shades {
            let mut pass =
                (frame.render(view.first)).expect("the graph's own images are always there");
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
            self.meshes.cull_second(frame, plan)?;
            let second = view.second.expect("a view that culls has a second pass");
            let mut pass = (frame.render(second)).expect("the graph's own images are always there");
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
                    let mut pass =
                        (frame.render(rest)).expect("the graph's own images are always there");
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
                draw_reflected(frame, early, plan, true)?;
                record_early(frame, early)?;
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
            let pass =
                (frame.render(shaded.rest)).expect("the graph's own images are always there");
            (pass, shared, frame_data, lit)
        } else if let (Some(early), Some(plan)) = (&view.early, &view.cull) {
            let mut pass =
                (frame.render(view.first)).expect("the graph's own images are always there");
            draw_depth(&mut pass, Runs::Culled(plan, FIRST), false)?;
            drop(pass);
            self.meshes.cull_second(frame, plan)?;
            let second = view.second.expect("a view that culls has a second pass");
            let mut pass = (frame.render(second)).expect("the graph's own images are always there");
            draw_depth(&mut pass, Runs::Culled(plan, plan.second()), false)?;
            drop(pass);
            draw_reflected(frame, early, plan, false)?;
            record_early(frame, early)?;
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
            let mut pass =
                (frame.render(view.first)).expect("the graph's own images are always there");
            let (frame_data, lit) = view_data(&|image| pass.sampled(image));
            let shared = pass.upload(bytemuck::bytes_of(&frame_data))?.address();
            (pass, shared, frame_data, lit)
        };
        // Behind everything, in place of the clear color: the atmosphere's sky or the image sky,
        // unless the view shows a part of shading.
        let aspect = data.viewport.aspect();
        let draws_sky = !matches!(data.show, Some(Show::Shading(_)));
        if draws_sky && let (Some(seen), Some(tables), Some(air)) = (&atmosphere, &tables, air) {
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
        } else if draws_sky
            && lit.is_some()
            && let Some((Source::Image { sky, .. }, _)) = sky
        {
            let drawn = self.sky.as_ref().expect("made for a sky to show");
            drawn.draw(&mut pass, &data.viewpoint, aspect, jitter, &sky, exposure);
        }
        let way = Way::Shaded {
            beside_visibility: shades.is_some(),
        };
        // In fog, a view that culls nothing has nothing opaque to draw before the fog but the sky,
        // as all it draws blends.
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
        // What the first pass didn't draw, over it: in its own pass, after the depth pyramid from
        // the first's depth and the culling's second pass against it; or in the same pass, beside a
        // visibility buffer or after the ambient occlusion, as its culling has run.
        if let (Some(plan), Some(second)) = (&view.cull, view.second) {
            if shades.is_none() && view.early.is_none() {
                drop(pass);
                self.meshes.cull_second(frame, plan)?;
                pass = (frame.render(second)).expect("the graph's own images are always there");
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
        // In fog, what's opaque is fogged, the sky included, then what blends draws over it in its
        // own pass, fogging itself.
        if let Some(fog) = &view.fog {
            drop(pass);
            self.fog.record(frame, &fog.plan, view_projection, shared);
            pass = (frame.render(fog.blended)).expect("the graph's own images are always there");
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
        // The canvas, over the view's last pass, or in its own pass over what the view shows, after
        // what follows. It has no ambient occlusion or reflections, which are of the view's own
        // depth, and no fog or haze, as it's in the view's own space.
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
            let canvas = canvas_projection(view_size);
            let on = data.canvas;
            let all = Runs::All;
            let drawn = meshes.draw(
                pass,
                canvas,
                canvas_data,
                view_size,
                on,
                on_canvas,
                all,
                way,
                None,
            );
            pass.end_label();
            drawn
        };
        // Else the canvas draws after the resolve or the upscale.
        let canvas_later = view.taa.is_some() || view.smaa.is_some() || view.upscale.is_some();
        if !canvas_later && view.canvas.is_none() {
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
            self.taa.resolve(frame, plan, drawn)?;
        }
        if let Some(plan) = &view.smaa {
            let smaa = self
                .smaa
                .as_ref()
                .expect("made by the frame that planned it");
            smaa.record(frame, plan, data.tonemapper, &data.grade);
        }
        if let Some(keep) = &view.keep {
            self.reflections.keep(frame, keep);
        }
        if let (Some(plan), Some(adaptation)) = (&view.exposure, &data.adaptation) {
            self.exposure.record(frame, plan, exposure, adaptation)?;
        }
        if let Some(plan) = &view.focus {
            self.depth_of_field.record(frame, plan);
        }
        if let Some(plan) = &view.blur {
            self.motion_blur.record(frame, plan, drawn, data.cut)?;
        }
        if let Some(plan) = &view.bloom {
            self.bloom.record(frame, plan, data.bloom, &data.grade);
        }
        if let Some(plan) = &view.lens {
            self.lens.record(frame, plan);
        }
        if let Some(plan) = &view.upscale {
            let mut pass =
                (frame.render(plan.pass)).expect("the graph's own images are always there");
            let upscale = self
                .upscale
                .as_ref()
                .expect("made by the frame that planned it");
            let sampled = plan.sampled.map(|image| pass.sampled(image));
            upscale.draw(&mut pass, sampled, size, view_size);
            if !data.canvas.is_empty() {
                counts += draw_canvas(&mut pass)?;
            }
        }
        if let Some(canvas) = view.canvas {
            let mut pass = (frame.render(canvas)).expect("the graph's own images are always there");
            counts += draw_canvas(&mut pass)?;
        }
        if let Some(plan) = &view.show {
            let pass = self
                .show
                .as_ref()
                .expect("made by the frame that planned it");
            pass.record(frame, plan);
        }
        if let Some(plan) = &view.trace {
            let trace = (self.trace.as_mut()).expect("made by the frame that planned it");
            trace.record(frame, plan, traced_data)?;
        }
        if let Some(read) = view.read_ids {
            let picked = self.picks.take(frame.number(), Some(view_size));
            frame.read_ids(read, picked)?;
        }
        let mut pass =
            (frame.render(view.tonemap)).expect("the graph's own images are always there");
        let shown = pass.sampled(view.shown);
        self.tonemap.draw(&mut pass, shown, view.tonemapper);
        drop(pass);
        if let (Some(plan), Some(outline)) = (&view.outline, data.outline) {
            self.outline.record(frame, plan, outline)?;
        }
        if let Some(read) = view.read_view {
            *self.view_read = frame.number();
            frame.read_image(read)?;
        }
        Ok((counts, culled))
    }
}
