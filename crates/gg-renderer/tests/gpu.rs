//! GPU tests of rendering, against reference images. They run on whatever GPU the loader offers,
//! which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another: those whose
//! names contain an argument that isn't a flag, as libtest picks them, or all of them without one.
//! `GG_UPDATE_REFERENCES=1` writes each test's output as its new reference instead of comparing.

// For setting the environment; see the SAFETY comment.
#![allow(unsafe_code)]

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2};
use std::fs::{self, File};
use std::io::BufWriter;
use std::num::NonZeroU64;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytemuck::NoUninit;
use gg_core::math::{self, DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{
    Adaptation, AlphaMode, AmbientOcclusion, Antialiasing, Atmosphere, Births, Blend, Bursts,
    Channel, Circle, DEFAULT_UPLOAD_BUDGET, DepthOfField, DrawCounts, Draws, Emission, Fog,
    FrameImage, Glyph, Grade, Grid, GridLines, ImageKind, Lens, Light, LightKind, Line, Lod, Lods,
    MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Map, Material, Materials, Mesh, MeshDraw, Meshes,
    Meshlet, Meshlets, Mipmaps, ModelMesh, ModelVertex, MotionBlur, Outline, PackedVertex,
    ParticleShape, Particles, Pick, Pixels, Projection, Reflections, RenderData, Renderer, Sampled,
    Sampler, Scene, Settings, Shader, Shaders, Show, SkinVertex, Sky, Sprite, SpriteSheet, Texels,
    Texture, Textures, Tonemapper, Ui, UploadPath, Uploads, ViewCallback, Viewpoint, Viewport,
    Workflow, Wrap,
};

/// Runs each test through `tests`, by the name it's written with; after `uploading by`, giving
/// each the way of uploading named, as its name says.
macro_rules! run {
    ($tests:ident $(, $test:ident)+ $(,)?) => {
        $($tests.run(stringify!($test), $test);)+
    };
    ($tests:ident uploading by $path:ident $(, $test:ident)+ $(,)?) => {
        $($tests.run(&format!("{} uploading by {}", stringify!($test), $path), || $test($path));)+
    };
    ($tests:ident by meshlets $(, $test:ident)+ $(,)?) => {
        $($tests.run(&format!("{} by meshlets", stringify!($test)), $test);)+
    };
}

fn main() -> ExitCode {
    if gg_core::profile::ENABLED {
        panic!(
            "the GPU tests set the environment before any other thread exists, and Tracy's start \
             before main; run them without the tracy feature"
        );
    }
    // CI runners without a GPU or a Vulkan driver, Windows' and macOS's, set this. Linux CI runs
    // the tests on lavapipe instead.
    if std::env::var_os("GG_SKIP_GPU_TESTS").is_some_and(|value| !value.is_empty()) {
        println!("GPU tests skipped: GG_SKIP_GPU_TESTS is set");
        return ExitCode::SUCCESS;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers, and can
    // add validation errors of their own. Unless `RUST_LOG` is set, only warnings and errors are
    // logged, and of the loader's and other layers' messages, such as its warning for each
    // implicit layer it leaves out, only errors, so that a passing run's output is its tests'
    // results rather than what each instance, device and pipeline logs as it's made.
    // SAFETY: no other thread exists yet to read the environment.
    unsafe {
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        if std::env::var_os("RUST_LOG").is_none() {
            std::env::set_var("RUST_LOG", "warn,gg_gpu::vulkan::general=error");
        }
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    gg_core::log::init("renderer-gpu-tests", &dir, true);

    let mut tests = Tests::new();
    run!(
        tests,
        ui_matches_its_reference,
        sprites_match_their_reference,
        batches_match_their_reference,
        circles_match_their_reference,
        text_matches_its_reference,
        the_canvas_matches_its_reference,
        outlines_match_their_reference,
        tonemappers_match_their_reference,
        colors_past_a_half_float_show_as_its_brightest,
        bloom_spreads_light_round_what_is_bright_and_no_further,
        a_lone_bright_pixel_blooms_less_than_a_square_of_its_light,
        grades_transform_as_they_say,
        dithering_keeps_a_gradients_mean_and_breaks_its_bands,
        exposure_adapts_frame_by_frame_from_a_bright_view_to_a_dark_one,
        exposure_meters_between_its_percentiles,
        a_looks_exposure_adapts_apart_from_the_games_own,
        an_exposure_jump_under_antialiasing_leaves_no_ghost,
        adapted_frames_match_their_reference,
        depth_of_field_blurs_what_is_off_its_focus,
        motion_blur_smears_what_moves_and_the_turning_camera,
        lenses_vignette_fringe_and_grain_the_view,
        shaders_read_the_frames_data,
        lights_shade_as_gltf_defines,
        back_faces_flip_their_normal_maps,
        lit_spheres_match_their_reference,
        many_lights_match_their_reference,
        a_uniform_sky_lights_white_as_itself,
        the_sky_shows_each_way_behind_the_world,
        sky_lit_spheres_match_their_reference,
        specular_glossiness_shades_as_its_metallic_roughness_equal,
        a_skys_sun_lights_once,
        atmospheres_match_their_references,
        the_suns_disk_shows_the_transmittance,
        the_haze_dims_and_lights_what_is_far,
        the_skys_light_falls_smoothly_through_dusk,
        an_image_sky_renders_as_it_did_before_an_atmosphere,
        ambient_occlusion_darkens_the_skys_light_near_what_hides_it,
        ambient_occlusion_leaves_the_lights_alone,
        ambient_occlusion_darkens_what_comes_into_view_in_the_second_pass,
        ambient_occlusion_averages_its_noise_away,
        ambient_occlusion_holds_to_the_world_as_the_view_pans,
        ambient_occlusion_leaves_no_trail_behind_what_moves,
        specular_occlusion_dims_the_skys_reflection_by_the_way_to_the_eye,
        reflections_mirror_what_the_view_shows,
        reflections_follow_an_exposures_jump,
        reflections_mirror_what_lights_light_under_a_dark_sky,
        reflections_leave_what_they_dont_take_as_it_is,
        reflections_mirror_about_the_shading_normal,
        views_show_parts_of_shading_and_images_in_place_of_light,
        the_light_shows_as_what_surfaces_diffuse_and_what_they_reflect,
        fog_dims_and_lights_as_its_integral_says,
        fog_of_no_density_or_opacity_changes_nothing,
        fogged_scenes_match_their_reference,
        shadows_fall_where_their_casters_put_them,
        culling_keeps_what_each_view_sees,
        the_scene_draws_as_draws_do,
        occlusion_culling_draws_what_shows,
        occlusion_culling_draws_what_shows_with_ambient_occlusion,
        what_blends_goes_over_what_is_opaque_behind_it_in_any_order,
        shadows_hold_still_as_the_view_moves,
        surfaces_cast_no_shadows_on_themselves,
        shadowed_models_match_their_reference,
        lines_match_their_reference,
        grids_match_their_reference,
        views_match_their_reference,
        views_read_back_as_they_render,
        pixels_show_in_the_view,
        a_view_in_the_ui_matches_its_reference,
        replaced_shaders_draw_from_the_next_frame,
        pipelines_go_through_the_cache,
        capturing_needs_renderdoc,
        depth_matches_its_reference,
        levels_of_detail_match_their_reference,
        models_match_their_reference,
        skinned_meshes_move_with_their_joints,
        particles_match_their_reference,
        mixed_textures_match_their_reference,
        picks_answer_what_their_frame_drew,
        picks_wait_through_frames_that_read_none,
        picks_see_through_the_grid,
        removed_assets_draw_nothing_or_their_stand_in,
        antialiasing_smooths_edges_frame_by_frame,
        antialiasing_follows_what_moves,
        a_far_scene_draws_as_at_the_origin_through_its_origins_move,
        antialiasing_leaves_no_trail,
        antialiasing_steadies_what_is_thinner_than_a_pixel,
        antialiasing_settles_without_a_jump_once_the_view_stops,
        antialiasing_keeps_still_dark_colors_as_drawn,
        antialiasing_leaves_no_trail_behind_dark_specks,
        antialiasing_steadies_what_is_thinner_than_a_pixel_as_the_view_moves,
        antialiasing_leaves_no_trail_behind_what_crosses_flickering_pixels,
        antialiasing_keeps_no_ghost_of_what_blinks_in_a_still_view,
        smaa_smooths_edges_in_a_frame,
        smaa_blends_by_how_much_of_each_pixel_the_edge_covers,
        smaa_blends_along_diagonals_by_how_much_they_cover,
        smaa_blends_light_up_to_a_half_floats_brightest,
    );
    // The tests of models' shading again, their models cut into meshlets, which the view draws
    // into its visibility buffer and shades from it, against the references they made drawn by
    // their indices and shaded as they draw.
    CUT_MESHLETS.store(true, Ordering::Relaxed);
    run!(
        tests by meshlets,
        lights_shade_as_gltf_defines,
        back_faces_flip_their_normal_maps,
        lit_spheres_match_their_reference,
        many_lights_match_their_reference,
        a_uniform_sky_lights_white_as_itself,
        sky_lit_spheres_match_their_reference,
        specular_glossiness_shades_as_its_metallic_roughness_equal,
        a_skys_sun_lights_once,
        atmospheres_match_their_references,
        ambient_occlusion_darkens_the_skys_light_near_what_hides_it,
        ambient_occlusion_leaves_the_lights_alone,
        ambient_occlusion_darkens_what_comes_into_view_in_the_second_pass,
        specular_occlusion_dims_the_skys_reflection_by_the_way_to_the_eye,
        reflections_mirror_what_the_view_shows,
        reflections_mirror_what_lights_light_under_a_dark_sky,
        reflections_leave_what_they_dont_take_as_it_is,
        reflections_mirror_about_the_shading_normal,
        views_show_parts_of_shading_and_images_in_place_of_light,
        the_light_shows_as_what_surfaces_diffuse_and_what_they_reflect,
        fog_dims_and_lights_as_its_integral_says,
        fog_of_no_density_or_opacity_changes_nothing,
        fogged_scenes_match_their_reference,
        adapted_frames_match_their_reference,
        surfaces_cast_no_shadows_on_themselves,
        shadowed_models_match_their_reference,
        models_match_their_reference,
        mixed_textures_match_their_reference,
        picks_answer_what_their_frame_drew,
        the_scene_draws_as_draws_do,
    );
    CUT_MESHLETS.store(false, Ordering::Relaxed);
    // Through mesh shaders, where the GPU has them, and through vertex shaders, as every GPU can.
    for mesh_shaders in [true, false] {
        let settings = Settings {
            mesh_shaders: Some(mesh_shaders),
            ..settings(None)
        };
        if let Err(error) = Renderer::headless(&settings, 1, 1) {
            assert_eq!(error.code(), "gpu.no_mesh_shaders", "{error:#}");
            println!("tests drawing meshlets through mesh shaders skipped: {error}");
            continue;
        }
        let through = if mesh_shaders { "mesh" } else { "vertex" };
        let name = format!("meshlets_draw_as_their_indices_do through {through} shaders");
        tests.run(&name, || meshlets_draw_as_their_indices_do(mesh_shaders));
    }
    // Once for each way of uploading that the GPU has.
    for path in UploadPath::ALL {
        if let Err(error) = Renderer::headless(&settings(Some(path)), 1, 1) {
            assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
            println!("tests uploading by {path} skipped: {error}");
            continue;
        }
        run!(
            tests uploading by path,
            meshes_match_their_reference,
            textures_match_their_reference,
            sheets_match_their_reference,
            a_large_texture_draws_once_streamed,
            large_assets_stream_over_frames_within_the_budget,
            a_reload_draws_the_old_until_the_new_is_drawable,
            bc7_textures_sample_as_their_blocks,
            replaced_meshes_and_textures_draw_once_they_are_there,
            mipmaps_match_their_reference,
        );
    }
    tests.finish()
}

/// Runs the tests the arguments pick, one after another, and counts how they go.
struct Tests {
    /// The arguments that aren't libtest's flags, nor the values they take: a test runs if its
    /// name contains one of them, or if there are none.
    filters: Vec<String>,
    passed: usize,
    failed: usize,
}

impl Tests {
    fn new() -> Self {
        let mut filters = Vec::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let takes_value = matches!(
                arg.as_str(),
                "--test-threads"
                    | "--skip"
                    | "--logfile"
                    | "--format"
                    | "--color"
                    | "--shuffle-seed"
                    | "-Z"
            );
            if takes_value {
                args.next();
            } else if !arg.starts_with('-') {
                filters.push(arg);
            }
        }
        Self {
            filters,
            passed: 0,
            failed: 0,
        }
    }

    /// Runs `test` if the filters pick it, and prints whether it passed. A test that fails goes
    /// no further, and the log shows its panic, but the tests after it still run.
    fn run(&mut self, name: &str, test: impl FnOnce()) {
        let picked = self.filters.is_empty()
            || (self.filters.iter()).any(|filter| name.contains(filter.as_str()));
        if !picked {
            return;
        }
        if panic::catch_unwind(AssertUnwindSafe(test)).is_ok() {
            self.passed += 1;
            println!("test {name} ... ok");
        } else {
            self.failed += 1;
            println!("test {name} ... FAILED");
        }
    }

    /// Prints how many tests passed and failed, and fails the run if any failed.
    fn finish(self) -> ExitCode {
        let Self { passed, failed, .. } = self;
        println!("test result: {passed} passed; {failed} failed");
        if failed == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

/// What the tests clear to, in linear RGBA.
const CLEAR: [f32; 4] = [0.02, 0.03, 0.05, 1.0];

/// A budget of bytes a frame copies that a test's larger assets take several frames of.
const SMALL_BUDGET: u64 = 64 << 10;

/// How the tests' renderers are set up: under validation, uploading by `uploads`, if given, rather
/// than by the GPU's best way, within the default budget.
fn settings(uploads: Option<UploadPath>) -> Settings<'static> {
    Settings {
        app_name: "renderer gpu tests",
        validation: true,
        gpu: None,
        vsync: false,
        uploads,
        mesh_shaders: None,
        visibility_buffer: true,
        upload_budget: DEFAULT_UPLOAD_BUDGET,
        pipeline_cache: None,
        capture: None,
    }
}

/// A headless renderer of `width` by `height` pixels, under validation.
fn renderer(width: u32, height: u32) -> Renderer {
    renderer_uploading(width, height, None)
}

/// A renderer like [`renderer`]'s, that uploads by `uploads`, if given.
fn renderer_uploading(width: u32, height: u32, uploads: Option<UploadPath>) -> Renderer {
    renderer_within(width, height, uploads, DEFAULT_UPLOAD_BUDGET)
}

/// A renderer like [`renderer_uploading`]'s, whose frames copy at most `budget` bytes each.
fn renderer_within(width: u32, height: u32, uploads: Option<UploadPath>, budget: u64) -> Renderer {
    let settings = Settings {
        upload_budget: budget,
        ..settings(uploads)
    };
    let renderer = Renderer::headless(&settings, width, height).expect("a renderer");
    assert!(
        renderer.validation().enabled(),
        "GPU tests run under validation; install the Vulkan SDK's validation layer"
    );
    assert!(
        renderer.validation().synchronization(),
        "GPU tests run under synchronization validation; install a newer Vulkan SDK, whose \
         validation layer takes VK_EXT_layer_settings"
    );
    if let Some(uploads) = uploads {
        assert_eq!(
            renderer.upload_path(),
            uploads,
            "the renderer uploads as asked"
        );
    }
    renderer
}

/// A fixture shader's path.
fn shader(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shaders")
        .join(file)
}

/// What a test adds meshes, shaders, textures and materials to, and the uploads that take them to
/// its renderer; the lights, the sky and the ambient occlusion its frames shade with; and no draws,
/// for a frame's canvas.
#[derive(Default)]
struct Stores {
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    materials: Materials,
    scene: Scene,
    uploads: Uploads,
    lights: Vec<Light>,
    sky: Option<Sky>,
    atmosphere: Option<Atmosphere>,
    ambient_occlusion: Option<AmbientOcclusion>,
    reflections: Option<Reflections>,
    none: Draws,
}

impl Stores {
    /// A frame of `draws` into `viewport`, as `viewpoint` sees them, cleared to [`CLEAR`], that
    /// draws nothing on its canvas, picks and outlines nothing and isn't antialiased, bloomed,
    /// graded or tonemapped, at time 0 with an exposure of 1 that doesn't adapt and the stores'
    /// lights, sky and ambient occlusion, with the uploads of what's been added, and what's changed
    /// in the scene, since the last frame's.
    fn data<'a>(
        &'a mut self,
        viewport: Viewport,
        viewpoint: Viewpoint,
        draws: &'a Draws,
    ) -> RenderData<'a> {
        let Self {
            meshes,
            shaders,
            textures,
            materials,
            scene,
            uploads,
            lights,
            sky,
            atmosphere,
            ambient_occlusion,
            reflections,
            none,
        } = self;
        uploads.take(meshes, shaders, textures, materials);
        uploads.take_scene(scene);
        RenderData {
            clear_color: CLEAR,
            tonemapper: Tonemapper::None,
            antialiasing: Antialiasing::None,
            exposure: 1.0,
            adaptation: None,
            looked: false,
            time: 0.0,
            lights,
            sky: *sky,
            atmosphere: *atmosphere,
            bloom: 0.0,
            grade: Grade::IDENTITY,
            ambient_occlusion: *ambient_occlusion,
            fog: None,
            reflections: *reflections,
            depth_of_field: None,
            motion_blur: None,
            lens: Lens::NONE,
            viewport,
            viewpoint,
            origin: DVec3::ZERO,
            cut: false,
            uploads,
            draws,
            canvas: none,
            picks: &[],
            read_picks: true,
            outline: None,
            read_view: false,
            list_images: false,
            pixels: None,
            show: None,
        }
    }
}

/// Renders a frame of `draws` into `viewport`, as `viewpoint` sees them, with the uploads taken
/// since the last, and then frames until every mesh and texture added is drawable, which through
/// a transfer queue takes frames after the one that takes them: so that what's compared or read
/// back after shows them all.
fn settle(
    renderer: &mut Renderer,
    stores: &mut Stores,
    viewport: Viewport,
    viewpoint: Viewpoint,
    draws: &Draws,
) {
    for _ in 0..200 {
        render_without_ui(renderer, stores.data(viewport, viewpoint, draws));
        if renderer.streaming().is_empty() {
            return;
        }
    }
    panic!(
        "still streaming after 200 frames: {:?}",
        renderer.streaming()
    );
}

/// A view that fills a frame of `size` pixels.
fn filling(size: [u32; 2]) -> Viewport {
    Viewport {
        origin: [0, 0],
        size,
    }
}

/// The pixel at (`x`, `y`) of `frame`, `width` pixels wide: RGBA8.
fn pixel_at(frame: &[u8], width: u32, [x, y]: [u32; 2]) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    <[u8; 4]>::try_from(&frame[at..at + 4]).expect("a pixel")
}

/// Whether `pixel` is `color`, each channel within 1, which sRGB's encoding may round by.
fn close(pixel: [u8; 4], color: [u8; 4]) -> bool {
    pixel.iter().zip(color).all(|(&a, b)| a.abs_diff(b) <= 1)
}

/// `CLEAR`, sRGB-encoded, as a frame reads back.
const CLEARED: [u8; 4] = [39, 48, 63, 255];

/// The 8-bit sRGB of a linear channel, as a frame's target encodes what shaders write.
fn encoded(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * math::libm::powf(linear, 1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// A unit square around the origin, of positions alone, as `flat_color.slang` takes them.
fn square(meshes: &mut Meshes) -> Mesh<[f32; 3]> {
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh")
}

/// A metallic-roughness material's workflow.
fn metallic(metallic: f32, roughness: f32) -> Workflow {
    Workflow::MetallicRoughness {
        metallic,
        roughness,
    }
}

/// A unit square around the origin, a model's, counterclockwise seen from +Z, which it faces, its
/// texture's top left at its top left, its tangent along +X.
/// A model's mesh of `vertices` and `indices`, packed, of one level, all of them.
fn add_model(
    meshes: &mut Meshes,
    name: &str,
    vertices: &[ModelVertex],
    indices: &[u32],
) -> Mesh<PackedVertex> {
    let packed = ModelMesh::pack(name, vertices, indices.to_vec(), Vec::new());
    let mut packed = packed.expect("a mesh");
    if CUT_MESHLETS.load(Ordering::Relaxed) {
        cut_meshlets(&mut packed);
    }
    meshes.add_model(name, &packed).expect("a mesh")
}

/// Whether [`add_model`] cuts each model's mesh into meshlets, as a game's package holds them,
/// which views draw into their visibility buffers and shade from them: as `main` runs the tests of
/// models' shading again, against the same references. The tests run one after another.
static CUT_MESHLETS: AtomicBool = AtomicBool::new(false);

fn model_square(meshes: &mut Meshes) -> Mesh<PackedVertex> {
    let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
        position: Vec3::new(x, y, 0.0),
        uv: Vec2::from(uv),
        color: Vec4::ONE,
        normal: Vec3::Z,
        tangent: Vec4::new(1.0, 0.0, 0.0, 1.0),
    };
    let corners = [
        corner([-0.5, -0.5], [0.0, 1.0]),
        corner([0.5, -0.5], [1.0, 1.0]),
        corner([0.5, 0.5], [1.0, 0.0]),
        corner([-0.5, 0.5], [0.0, 0.0]),
    ];
    add_model(meshes, "model square", &corners, &[0, 1, 2, 2, 3, 0])
}

/// Draws a window of egui widgets offscreen, and compares it with its reference image.
fn ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let draws = Draws::default();

    let egui = egui::Context::default();
    // Without animation, the first frames show the window as it stays.
    egui.global_style_mut(|style| style.animation_time = 0.0);
    let mut value = 0.25;
    // egui sizes a new window from what it drew in the frame before, so the second frame is
    // the first that's settled.
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH as f32, HEIGHT as f32),
            )),
            time: Some(0.0),
            ..Default::default()
        };
        let mut output = egui.run_pass(input, |ctx| {
            egui::Window::new("GGEngine")
                .fixed_pos([16.0, 16.0])
                .show(ctx, |ui| {
                    ui.label("Rendered by gg-renderer");
                    let _ = ui.button("A button");
                    ui.add(egui::Slider::new(&mut value, 0.0..=1.0));
                });
        });
        let primitives = egui.tessellate(output.shapes, output.pixels_per_point);
        let ui = Ui {
            primitives: &primitives,
            textures: &mut output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        };
        let data = stores.data(filling([WIDTH, HEIGHT]), Viewpoint::default(), &draws);
        assert!(
            renderer.render(data, ui, &mut []).expect("a frame"),
            "an offscreen image can always be rendered into"
        );
    }
    check(renderer, "ui", WIDTH, HEIGHT);
}

/// Draws two squares in the colors their draws give, then a triangle in red, green and blue over
/// the first, each mesh with its own vertex type and shader and placed by a transform of its own,
/// into part of an offscreen image, as a camera away from the origin sees them, and compares it
/// with its reference image. The first square is turned and scaled, and the triangle moved and
/// scaled to cover much of it, so that a matrix pushed transposed or multiplied in the wrong
/// order, or the meshes drawn out of order, change thousands of pixels; the second square is
/// clear of them, so that a draw given another's parameters changes a thousand; it's drawn through
/// a `MeshDraw`, which holds its mesh, shader and parameters without their types, as an entity's
/// does, and joins the first square's instanced draw. The triangle is added in the last of the
/// frames, which outnumber those in flight, and the frames go on until it's drawable, so that the
/// image shows a mesh streamed later beside those that earlier frames streamed to the GPU. The
/// view renders into an image of its own, which the frame shows at the view's place, so that one
/// shown elsewhere changes thousands of pixels.
fn meshes_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, which is 160 pixels, so 40 pixels a unit both
    // ways; the view's center is (1, 0.5).
    let viewpoint = Viewpoint {
        position: DVec3::new(1.0, 0.5, 0.0),
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let transform = |scale, turn: f32, [x, y]: [f32; 2]| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_z(math::to_radians(turn)),
            Vec3::new(x, y, 0.0),
        )
    };
    // Positions, and a color for each draw.
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    draws.push(square, flat, transform(2.5, 30.0, [0.5, 0.5]), blue);
    let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
    let second = MeshDraw::new(square, flat, orange);
    draws.push_draw(transform(0.8, 0.0, [3.2, 1.8]), &second, 0);
    // Off the image's corner, so that the image shows where the view is.
    let viewport = Viewport {
        origin: [40, 20],
        size: [240, 160],
    };
    for frame in 0..3 {
        if frame == 2 {
            // Positions, then colors; no parameters.
            let colored = (stores.shaders)
                .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
                .expect("a shader");
            let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
            // Pointing up.
            let vertices = [
                corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
                corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
                corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
            ];
            let triangle = (stores.meshes)
                .add("triangle", &vertices, &[0, 1, 2])
                .expect("a mesh");
            draws.push(triangle, colored, transform(2.0, 0.0, [1.0, 0.5]), ());
        }
        let data = stores.data(viewport, viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    check(renderer, "meshes", WIDTH, HEIGHT);
}

/// What `textured.slang` takes with each draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Sampling {
    texture: Texture,
    sampler: u32,
}

/// Draws a checkerboard texture repeated across a square, then a texture with alpha over part of
/// it and part of the clear color, into an offscreen image, and compares it with its reference
/// image. Each draw's parameters give its texture and sampler. The checkerboard repeats twice each
/// way, sharp, so that a sampler ignored would clamp it and blur it, and its top left is a third
/// color, so that an image flipped moves it. The engine's white texture, never drawn here, takes
/// slot 0, so that textures put in the wrong slots, or sampled through the wrong ones, show it or
/// each other; the one with alpha is added in the last frame, after which the frames go on until
/// it's drawable, so that the image shows a texture streamed later beside one streamed earlier.
/// Its texels are opaque, half, a quarter and wholly transparent, so that a shader blended as
/// opaque, or as though its alpha were multiplied in, changes thousands of pixels; and the textures
/// are sRGB, so that colors sampled without decoding come out brighter.
fn textures_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // Positions, then texture coordinates.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Alpha)
        .expect("a shader");
    // A unit square, its texture's top left at its top left, over texture coordinates 0 to
    // `extent`.
    let mut square = |name, extent: f32| {
        let corners = [
            [-0.5, -0.5, 0.0, 0.0, extent],
            [0.5, -0.5, 0.0, extent, extent],
            [0.5, 0.5, 0.0, extent, 0.0],
            [-0.5, 0.5, 0.0, 0.0, 0.0],
        ];
        (stores.meshes)
            .add(name, &corners, &[0, 1, 2, 2, 3, 0])
            .expect("a mesh")
    };
    let tiled = square("tiled square", 2.0);
    let plain = square("square", 1.0);
    // 2 units across, at `x`.
    let place =
        |x| Mat4::from_translation(Vec3::new(x, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(2.0));

    // 4 by 4 texels, sRGB-encoded: a checkerboard of red and blue, with yellow at the top left.
    let checkerboard: Vec<u8> = (0..16)
        .flat_map(|i| match (i % 4, i / 4) {
            (0, 0) => [250, 220, 60, 255],
            (x, y) if (x + y) % 2 == 0 => [200, 60, 40, 255],
            _ => [40, 90, 200, 255],
        })
        .collect();
    let checkerboard = (stores.textures)
        .add("checkerboard", [4, 4], &checkerboard, Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    // x from -2 to 0, pixels 40 to 120, each texel 10 pixels across.
    let sampling = Sampling {
        texture: checkerboard,
        sampler: Sampler::NearestRepeat.index(),
    };
    draws.push(tiled, textured, place(-1.0), sampling);
    for frame in 0..3 {
        if frame == 2 {
            // White, opaque and half transparent along the top; red a quarter opaque, then
            // nothing, along the bottom.
            let pixels = [
                [255, 255, 255, 255],
                [255, 255, 255, 128],
                [255, 0, 0, 64],
                [0, 0, 0, 0],
            ];
            let translucent = (stores.textures)
                .add("translucent", [2, 2], pixels.as_flattened(), Mipmaps::None)
                .expect("a texture");
            // x from -1 to 1, over the checkerboard's right half and the clear color, each texel
            // 40 pixels across.
            let sampling = Sampling {
                texture: translucent,
                sampler: Sampler::NearestClamp.index(),
            };
            draws.push(plain, textured, place(0.0), sampling);
        }
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "textures", WIDTH, HEIGHT);
}

/// Draws sprites into an offscreen image and compares it with its reference image: along the top,
/// a sheet of 4 by 4 texels whole, its top right quarter, and the whole sheet flipped left to
/// right; along the bottom, the sheet repeated three times across and twice down, a sprite of only
/// a color, and the sheet tinted and half transparent over part of that sprite and part of the
/// clear color. Each of the sheet's texels has a color of its own, so that texture coordinates
/// flipped, turned or taken from the wrong part of the rectangle change hundreds of pixels; and its
/// bottom left quarter is half transparent, so that a sprite shader that drops the texture's alpha,
/// or draws opaque, changes hundreds more. The sprite of only a color shows the engine's white
/// texture tinted, so that a tint dropped, or a default texture other than white, changes it all.
fn sprites_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // 4 by 4 texels, sRGB-encoded, red growing to the right and green downward; the bottom left
    // quarter is half transparent.
    let sheet: Vec<u8> = (0..16)
        .flat_map(|i| {
            let (x, y) = (i % 4, i / 4);
            let alpha = if x < 2 && y >= 2 { 128 } else { 255 };
            [40 + 60 * x, 40 + 60 * y, 120, alpha]
        })
        .collect();
    let sheet = stores.textures.add("sheet", [4, 4], &sheet, Mipmaps::None);
    let sheet = sheet.expect("a texture");
    // `width` by `height` units, centered at (`x`, `y`). Every sprite's edges fall between pixels.
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let whole = Sprite {
        texture: Some(sheet),
        sampler: Sampler::NearestClamp,
        ..Sprite::default()
    };
    let mut draws = Draws::default();
    // Along the top, each 60 pixels across: the whole sheet, 15 pixels a texel; its top right
    // quarter, 30 pixels a texel; and the whole sheet flipped.
    draws.push_sprite(place([-2.0, 1.0], [1.5, 1.5]), whole);
    let quarter = Sprite {
        rect: [Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.5)],
        ..whole
    };
    draws.push_sprite(place([0.0, 1.0], [1.5, 1.5]), quarter);
    let flipped = Sprite {
        rect: [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)],
        ..whole
    };
    draws.push_sprite(place([2.0, 1.0], [1.5, 1.5]), flipped);
    // Along the bottom: the sheet tiled, 8 pixels a texel, so that no pixel's center is on a
    // texel's edge; orange; and the sheet tinted, over the orange sprite's right third, 20 pixels.
    let tiled = Sprite {
        rect: [Vec2::ZERO, Vec2::new(3.0, 2.0)],
        sampler: Sampler::NearestRepeat,
        ..whole
    };
    draws.push_sprite(place([-1.75, -1.0], [2.4, 1.6]), tiled);
    let orange = Sprite {
        color: Vec4::new(0.9, 0.4, 0.1, 1.0),
        ..Sprite::default()
    };
    draws.push_sprite(place([0.5, -1.0], [1.5, 1.5]), orange);
    let tinted = Sprite {
        color: Vec4::new(0.3, 0.8, 1.0, 0.5),
        ..whole
    };
    draws.push_sprite(place([1.5, -1.0], [1.5, 1.5]), tinted);
    // More frames than are in flight.
    for _ in 0..3 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "sprites", WIDTH, HEIGHT);
}

/// Draws circles into an offscreen image and compares it with its reference image: along the top,
/// a white disc, an orange ring a fifth of its radius thick and a cyan disc faded over half its
/// radius; along the bottom, a green ellipse, scaled unevenly, a purple disc half transparent over
/// an orange sprite's corner and the clear color, and white discs 8 and 3 pixels across, their
/// edges smoothed as much as the rest's. Each circle's quad shows what's behind it outside the
/// circle, so that a shader that draws its whole quad, or drops its alpha, changes hundreds of
/// pixels.
fn circles_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // `width` by `height` units, centered at (`x`, `y`).
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let colored = |color: [f32; 4]| Circle {
        color: Vec4::from(color),
        ..Circle::default()
    };
    let mut draws = Draws::default();
    // First, for the purple disc to go over.
    let orange = Sprite {
        color: Vec4::new(0.9, 0.4, 0.1, 1.0),
        ..Sprite::default()
    };
    draws.push_sprite(place([0.25, -1.25], [1.0, 1.0]), orange);
    let ring = Circle {
        thickness: 0.2,
        ..colored([0.9, 0.5, 0.1, 1.0])
    };
    let faded = Circle {
        fade: 0.5,
        ..colored([0.1, 0.8, 0.9, 1.0])
    };
    for (at, size, circle) in [
        ([-2.0, 1.0], [1.5, 1.5], Circle::default()),
        ([0.0, 1.0], [1.5, 1.5], ring),
        ([2.0, 1.0], [1.5, 1.5], faded),
        ([-1.75, -1.0], [2.2, 1.2], colored([0.2, 0.8, 0.3, 1.0])),
        ([0.75, -0.75], [1.2, 1.2], colored([0.6, 0.2, 0.9, 0.5])),
        ([2.0, -1.0], [0.2, 0.2], Circle::default()),
        ([2.5, -1.0], [0.075, 0.075], Circle::default()),
    ] {
        draws.push_circle(&mut stores.shaders, place(at, size), circle);
    }
    // More frames than are in flight.
    for _ in 0..3 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "circles", WIDTH, HEIGHT);
}

/// Draws glyphs from a field of data whose distances this test computes, a ring's and a square's,
/// as a font's field holds a glyph's in each channel, and compares them with their reference image:
/// along the top, rings on quads from 8 to 120 pixels across, from a quarter of a pixel a texel of
/// the field to nearly four, each smoothed over a pixel; along the bottom, squares, one turned, one
/// half transparent over an orange sprite, their corners square, and a small white one. What's
/// fully transparent draws nothing, so a shader that draws its whole quad changes hundreds of pixels.
fn text_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 160;
    /// Each cell's texels across and down, and how far their distances reach.
    const CELL: u32 = 32;
    const RANGE: f32 = 8.0;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // The ring's and the square's signed distances from their edges, inward, in texels, from each
    // texel's middle: two cells side by side.
    let ring = |x: f32, y: f32| {
        let from = Vec2::new(x, y).length();
        (12.0 - from).min(from - 6.0)
    };
    let square = |x: f32, y: f32| {
        let outside = Vec2::new(x.abs() - 11.0, y.abs() - 11.0);
        let past = outside.max(Vec2::ZERO).length();
        if past > 0.0 {
            -past
        } else {
            -outside.max_element()
        }
    };
    let mut pixels = Vec::with_capacity((CELL * 2 * CELL * 4) as usize);
    for y in 0..CELL {
        for cell in [&ring as &dyn Fn(f32, f32) -> f32, &square] {
            for x in 0..CELL {
                let middle = CELL as f32 / 2.0;
                let distance = cell(x as f32 + 0.5 - middle, y as f32 + 0.5 - middle);
                let byte = ((distance / RANGE + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels.extend_from_slice(&[byte; 4]);
            }
        }
    }
    let field = stores
        .textures
        .add("field", [CELL * 2, CELL], pixels, Texels::Data);
    let field = field.expect("a field");
    let glyph = |cell: f32, color: [f32; 4]| Glyph {
        color: Vec4::from(color),
        field,
        rect: [Vec2::new(cell / 2.0, 0.0), Vec2::new(cell / 2.0 + 0.5, 1.0)],
        range: RANGE,
    };
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 8 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // `size` units across and up, centered at (`x`, `y`), turned by `angle`.
    let place = |[x, y]: [f32; 2], size: f32, angle: f32| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(size, size, 1.0),
            Quat::from_rotation_z(angle),
            Vec3::new(x, y, 0.0),
        )
    };
    let mut draws = Draws::default();
    // First, for the half transparent square to go over.
    let orange = Sprite {
        color: Vec4::new(0.9, 0.4, 0.1, 1.0),
        ..Sprite::default()
    };
    draws.push_sprite(place([1.25, -1.25], 1.0, 0.0), orange);
    let white = [1.0; 4];
    let rings = [
        (-3.4, 0.2),
        (-3.0, 0.4),
        (-2.4, 0.8),
        (-1.2, 1.6),
        (1.2, 3.0),
    ];
    let mut glyphs: Vec<(Mat4, Glyph, u64)> = (rings.iter())
        .map(|&(x, size)| (place([x, 1.0], size, 0.0), glyph(0.0, white), 0))
        .collect();
    glyphs.extend(
        [
            (
                place([-3.0, -1.0], 1.2, 0.0),
                glyph(1.0, [0.2, 0.8, 0.3, 1.0]),
            ),
            (
                place([-1.25, -1.0], 1.2, 0.5),
                glyph(1.0, [0.1, 0.8, 0.9, 1.0]),
            ),
            (
                place([0.75, -0.75], 1.2, 0.0),
                glyph(1.0, [0.6, 0.2, 0.9, 0.5]),
            ),
            (place([3.0, -1.0], 0.6, 0.0), glyph(1.0, white)),
        ]
        .map(|(at, glyph)| (at, glyph, 0)),
    );
    draws.extend_glyphs(&mut stores.shaders, glyphs.into_iter());
    // More frames than are in flight.
    for _ in 0..3 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "text", WIDTH, HEIGHT);
}

/// Draws on the canvas of a view in part of a frame, over the world, and compares it with its
/// reference image: squares 16 pixels across in the view's corners, red at its bottom left, green
/// at its bottom right, blue at its top left and yellow at its top right, a white bar half
/// transparent across its middle and an orange disc above its left, all over an opaque gray square
/// near a camera with perspective, which covers the view but its right edge, and a cyan sprite
/// nearer still, which the bar goes over too. Canvas draws at the far plane, which the square would
/// hide, make 3,360 pixels differ.
fn the_canvas_matches_its_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = Viewport {
        origin: [40, 20],
        size: [160, 120],
    };
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    // 0.7 in front of the camera, where its view is about 0.81 high and 1.08 across, shifted left.
    let near = Mat4::from_scale_rotation_translation(
        Vec3::new(1.0, 0.85, 1.0),
        Quat::IDENTITY,
        Vec3::new(-0.1, 0.0, 3.3),
    );
    draws.push(square, flat, near, Vec4::new(0.4, 0.4, 0.4, 1.0));
    let cyan = Sprite {
        color: Vec4::new(0.1, 0.8, 0.9, 1.0),
        ..Sprite::default()
    };
    // 0.55 in front of the camera, about 15 pixels across.
    let in_front = Mat4::from_scale_rotation_translation(
        Vec3::new(0.08, 0.08, 1.0),
        Quat::IDENTITY,
        Vec3::new(0.04, 0.0, 3.45),
    );
    draws.push_sprite(in_front, cyan);
    // `size` pixels across and up, centered at (`x`, `y`) from the view's bottom left.
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let colored = |color: [f32; 4]| Sprite {
        color: Vec4::from(color),
        ..Sprite::default()
    };
    let mut canvas = Draws::default();
    for (at, color) in [
        ([8.0, 8.0], [0.9, 0.1, 0.1, 1.0]),
        ([152.0, 8.0], [0.1, 0.8, 0.2, 1.0]),
        ([8.0, 112.0], [0.1, 0.2, 0.9, 1.0]),
        ([152.0, 112.0], [0.9, 0.8, 0.1, 1.0]),
    ] {
        canvas.push_sprite(place(at, [16.0, 16.0]), colored(color));
    }
    canvas.push_sprite(
        place([80.0, 60.0], [120.0, 20.0]),
        colored([1.0, 1.0, 1.0, 0.5]),
    );
    let orange = Circle {
        color: Vec4::new(0.9, 0.5, 0.1, 1.0),
        ..Circle::default()
    };
    canvas.push_circle(
        &mut stores.shaders,
        place([40.0, 90.0], [24.0, 24.0]),
        orange,
    );
    // More frames than are in flight, and until everything is drawable.
    for _ in 0..200 {
        let data = RenderData {
            canvas: &canvas,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        if renderer.streaming().is_empty() {
            break;
        }
    }
    assert!(renderer.streaming().is_empty());
    check(renderer, "canvas", WIDTH, HEIGHT);
}

/// Outlines what the view shows of the IDs 3 and 7, 3 pixels wide, and compares it with its
/// reference image: a disc and a square of ID 7, which it goes round alike; the square's corner
/// under a square of ID 3, which it goes round together with the square, as one shape, and the
/// square's bottom past the view's, where it stops; and beside them a square of no entity and one
/// of ID 5, which it leaves alone. The band, the glow and where they end are read off the disc's
/// row. First, outlines of no IDs, and 0, NaN and negative pixels wide, change no pixel of a frame
/// without one; and the
/// frames that outline read picks back first, which synchronization validation sees the outline's
/// reads of the IDs wait for. The outline is drawn into the view, so the view read back shows it
/// as the frame does.
fn outlines_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // `width` by `height` units, centered at (`x`, `y`).
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let colored = |[r, g, b]: [f32; 3]| Sprite {
        color: Vec4::new(r, g, b, 1.0),
        ..Sprite::default()
    };
    let mut draws = Draws::default();
    let disc = Circle {
        color: Vec4::new(0.2, 0.6, 0.9, 1.0),
        ..Circle::default()
    };
    // 60 pixels across, its middle at (60, 50).
    let disc = (place([-1.5, 0.75], [1.5, 1.5]), disc, 7);
    draws.extend_circles(&mut stores.shaders, [disc].into_iter());
    draws.extend_sprites(
        [
            // From (120, 110) to (180, 170), past the view's bottom.
            (place([0.75, -1.5], [1.5, 1.5]), colored([0.3, 0.7, 0.3]), 7),
            // From (160, 98) to (192, 130), over the square's corner.
            (place([1.4, -0.85], [0.8, 0.8]), colored([0.8, 0.3, 0.3]), 3),
            (
                place([-1.5, -1.25], [0.8, 0.8]),
                colored([0.6, 0.6, 0.6]),
                0,
            ),
            (place([2.3, 1.0], [0.8, 0.8]), colored([0.8, 0.8, 0.2]), 5),
        ]
        .into_iter(),
    );
    let orange = Vec4::new(1.0, 0.35, 0.02, 1.0);
    let outline = |ids, width| Outline {
        ids,
        color: orange,
        width,
    };
    // Each pixel as a frame without an outline has it, once everything is drawable: an outline of
    // the square and the disc's ID, wider than 0, would go round them.
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    let mut plain = None;
    let nothing = [
        outline(&[], 3.0),
        outline(&[7], 0.0),
        outline(&[7], f32::NAN),
        outline(&[3, 7], -1.0),
    ];
    for outlined in [None].into_iter().chain(nothing.map(Some)) {
        let data = RenderData {
            outline: outlined,
            ..stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let pixels = (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back");
        let Some(plain) = &plain else {
            plain = Some(pixels);
            continue;
        };
        let differing = (pixels.iter().zip(plain))
            .filter(|&(&a, &b)| a.abs_diff(b) > 1)
            .count();
        assert_eq!(differing, 0, "{outlined:?} outlined something");
    }
    // More frames than are in flight, each picking the disc's middle first.
    let picks = [Pick {
        tag: 1,
        pixel: [60, 50],
    }];
    for _ in 0..3 {
        let data = RenderData {
            picks: &picks,
            outline: Some(outline(&[3, 7], 3.0)),
            read_view: true,
            ..stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
    let read = view_read(&renderer);
    let pixels = frame_pixels(&mut renderer);
    assert_eq!(read, Some(([WIDTH, HEIGHT], pixels.clone())));
    // Out from the disc's left edge along its middle row: its ID's first pixel is x = 30, so a
    // pixel `away` to the left of it is that far from the nearest. Red goes from the clear color's
    // 39 to the outline's 255 as its alpha rises, blended as light: the band solid to 3 pixels,
    // smoothed over the next, whose alpha is 0 there; then the glow, half the band's alpha at the
    // band's inner edge and falling by the square of the way left to twice the width, 6 pixels.
    let linear = |c: f32| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            math::libm::powf((c + 0.055) / 1.055, 2.4)
        }
    };
    let encode = |l: f32| {
        let c = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * math::libm::powf(l, 1.0 / 2.4) - 0.055
        };
        (c * 255.0).round()
    };
    let (clear, orange_red) = (linear(39.0), 1.0);
    for (away, alpha) in [
        (1, 1.0),
        (2, 1.0),
        (3, 1.0),
        (4, 0.5 * (2.0f32 / 3.0) * (2.0 / 3.0)),
        (5, 0.5 * (1.0f32 / 3.0) * (1.0 / 3.0)),
        (6, 0.0),
        (9, 0.0),
    ] {
        let red = f32::from(pixels[((50 * WIDTH + 30 - away) * 4) as usize]);
        let want = encode(clear + alpha * (orange_red - clear));
        assert!(
            (red - want).abs() <= 2.0,
            "{away} pixels from the disc, red is {red}, not {want}"
        );
    }
    shut_down(renderer);
    compare("outlines", WIDTH, HEIGHT, &pixels);
}

/// Draws grids, each a frame of its own, and compares each with its reference image: on the ground
/// as a camera with perspective sees it from above, its lines anti-aliased, each power of ten's
/// fading out into the next's farther off, and all of them toward the horizon, the X axis red and
/// the Z axis blue; and in XY, facing it orthographically, its lines crisp, a pixel each, a unit
/// apart, the X axis red, the Y axis green, and the line ten units along X brighter. Each grid's
/// square and parameters are those the engine's grid gives its camera.
fn grids_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let red = Vec3::new(0.8, 0.1, 0.1);
    let green = Vec3::new(0.1, 0.7, 0.1);
    let blue = Vec3::new(0.1, 0.3, 0.9);
    // 2 over the ground, looking 22.5° down, seeing a sixth of a turn from the view's bottom to its
    // top, which shows the horizon; on a square around its foot a thousand times that across.
    let above = Viewpoint {
        position: DVec3::new(0.3, 2.0, 3.0),
        rotation: Quat::from_rotation_x(-PI / 8.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
    };
    let around_foot = Mat4::from_cols(
        Vec4::new(2000.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 2000.0, 0.0),
        Vec4::Y,
        Vec4::new(0.3, 0.0, 3.0, 1.0),
    );
    let ground = Grid {
        first: red,
        first_center: 0.3,
        second: blue,
        second_center: 3.0,
        eye: Vec3::new(0.0, 2.0, 0.0),
        coarsest: 3.0,
        normal: Vec3::Y,
        lines: GridLines::Perspective,
    };
    // 10 units from the view's bottom to its top, 16 pixels a unit, over (5, 1.5); on a square
    // twice the view's width across and its height up.
    let facing = Viewpoint {
        position: DVec3::new(5.0, 1.5, 0.0),
        projection: Projection::Orthographic {
            height: 10.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let over_view = Mat4::from_cols(
        Vec4::new(30.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 10.0, 0.0, 0.0),
        Vec4::Z,
        Vec4::new(5.0, 1.5, 0.0, 1.0),
    );
    let upright = Grid {
        first: red,
        first_center: 5.0,
        second: green,
        second_center: 1.5,
        eye: Vec3::NEG_Z,
        coarsest: 3.0,
        normal: Vec3::Z,
        lines: GridLines::Crisp,
    };
    for (name, viewpoint, transform, grid) in [
        ("grid_perspective", above, around_foot, ground),
        ("grid_crisp", facing, over_view, upright),
    ] {
        let mut draws = Draws::default();
        draws.push_grid(&mut stores.meshes, &mut stores.shaders, transform, grid);
        // More frames than are in flight.
        for _ in 0..3 {
            let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
            render_without_ui(&mut renderer, data);
        }
        settle(
            &mut renderer,
            &mut stores,
            filling([WIDTH, HEIGHT]),
            viewpoint,
            &draws,
        );
        compare(name, WIDTH, HEIGHT, &frame_pixels(&mut renderer));
    }
    shut_down(renderer);
}

/// Draws lines into an offscreen image, as a camera with perspective sees them, and compares it with
/// its reference image: across the top, lines 1, 2, 4 and 8 pixels wide, and one half a pixel wide,
/// which draws a pixel wide and half as bright; a square's outline, turned, whose sides meet at its
/// corners; a line from near the camera to far away, as wide all along it; a line on to behind the
/// camera, cut where it crosses the near plane, without which it would show mirrored through the
/// view's middle; a line behind an opaque square, which hides its middle, and another in front of
/// the square; and a line 10 pixels wide, half transparent, over a sprite.
fn lines_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 in front of the origin, seeing a sixth of a turn from the view's bottom to its top: at z =
    // 0, about 4.6 units, 34.6 pixels a unit.
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    let mut line = |draws: &mut Draws,
                    transform: Mat4,
                    [from, to]: [[f32; 3]; 2],
                    color: [f32; 4],
                    width: f32| {
        let drawn = Line {
            from: Vec3::from(from),
            to: Vec3::from(to),
            color: Vec4::from(color),
            width,
        };
        draws.push_line(&mut stores.shaders, transform, drawn, 0);
    };
    for (y, color, width) in [
        (2.0, [0.9, 0.9, 0.9, 1.0], 1.0),
        (1.75, [0.9, 0.3, 0.3, 1.0], 2.0),
        (1.5, [0.3, 0.9, 0.3, 1.0], 4.0),
        (1.2, [0.3, 0.5, 0.9, 1.0], 8.0),
        (0.9, [0.9, 0.9, 0.9, 1.0], 0.5),
    ] {
        let ends = [[-3.2, y, 0.0], [-0.8, y, 0.0]];
        line(&mut draws, Mat4::IDENTITY, ends, color, width);
    }
    let receding = [[-1.0, -0.8, 2.5], [-1.0, -0.8, -40.0]];
    line(
        &mut draws,
        Mat4::IDENTITY,
        receding,
        [0.1, 0.8, 0.9, 1.0],
        3.0,
    );
    let behind = [[1.0, -1.0, 0.0], [1.0, -1.0, 8.0]];
    line(
        &mut draws,
        Mat4::IDENTITY,
        behind,
        [0.9, 0.2, 0.8, 1.0],
        4.0,
    );
    let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
    let hiding =
        Mat4::from_translation(Vec3::new(2.2, -1.4, 0.5)) * Mat4::from_scale(Vec3::splat(0.8));
    draws.push(square, flat, hiding, gray);
    let hidden = [[1.5, -1.4, 0.0], [2.9, -1.4, 0.0]];
    line(
        &mut draws,
        Mat4::IDENTITY,
        hidden,
        [0.9, 0.5, 0.1, 1.0],
        4.0,
    );
    let shown = [[1.8, -1.0, 1.0], [2.5, -1.7, 1.0]];
    line(&mut draws, Mat4::IDENTITY, shown, [0.9, 0.5, 0.1, 1.0], 4.0);
    let orange = Sprite {
        color: Vec4::new(0.9, 0.4, 0.1, 1.0),
        ..Sprite::default()
    };
    draws.push_sprite(Mat4::from_translation(Vec3::new(0.2, 0.2, 0.0)), orange);
    let across = [[-0.6, -0.4, 0.0], [1.0, 0.8, 0.0]];
    line(
        &mut draws,
        Mat4::IDENTITY,
        across,
        [0.2, 0.4, 1.0, 0.5],
        10.0,
    );
    // A square 1.2 across, turned 30°.
    let turned = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.2),
        Quat::from_rotation_z(PI / 6.0),
        Vec3::new(1.8, 1.3, 0.0),
    );
    let yellow = Vec4::new(0.9, 0.8, 0.1, 1.0);
    draws.push_rect(&mut stores.shaders, turned, yellow, 3.0, 0);
    // More frames than are in flight.
    for _ in 0..3 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "lines", WIDTH, HEIGHT);
}

/// Draws three runs into an offscreen image, each one instanced draw, which `Renderer::drawn`
/// counts, and compares it with its reference image: six sprites along the top; a band, a mesh with
/// a shader of its own; and sprites along the bottom, over the band's lower edge. The sprites cycle
/// through the engine's white texture, tinted, and two textures of their own, in tints of their
/// own, scaled unevenly and turned, each 32 by 24 pixels, so that a sprite drawn with its
/// neighbor's transform or parameters, or not at all, changes more pixels than the comparison
/// allows; the bottom row's are the top's in reverse, so that one run read as another changes them
/// too. The last run has 20,000 sprites: its first three and last three along the bottom, and the
/// rest out of the view, in a look of their own. Only the last frame draws it, in the slot of the
/// first. A frame's arena merges the buffers it grew into once its slot comes round again, so only
/// a frame that needs more than its slot has held grows it: the last frame's first run fills a
/// little of the slot's one buffer, and the big run's transforms and parameters, more than a
/// megabyte, land in a buffer grown in the middle of the frame.
fn batches_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    /// The last run's sprites.
    const BIG: usize = 20_000;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // 2 by 2 texels, sRGB-encoded: red and yellow over green and blue; and a checkerboard of light
    // and dark.
    let quadrants = [
        [220, 50, 40, 255],
        [240, 200, 40, 255],
        [40, 170, 70, 255],
        [40, 80, 220, 255],
    ];
    let quadrants = (stores.textures)
        .add("quadrants", [2, 2], quadrants.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let (light, dark) = ([240, 240, 240, 255], [20, 20, 20, 255]);
    let checker = [light, dark, dark, light];
    let checker = (stores.textures)
        .add("checker", [2, 2], checker.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let white = Sprite::default().texture;
    // Orange, the quadrants as they are, the checker in cyan, violet, the quadrants half
    // transparent, and the checker in yellow.
    let looks = [
        (white, Vec4::new(0.9, 0.4, 0.1, 1.0)),
        (Some(quadrants), Vec4::ONE),
        (Some(checker), Vec4::new(0.3, 0.8, 1.0, 1.0)),
        (white, Vec4::new(0.6, 0.2, 0.9, 1.0)),
        (Some(quadrants), Vec4::new(1.0, 1.0, 1.0, 0.5)),
        (Some(checker), Vec4::new(1.0, 0.9, 0.3, 1.0)),
    ];
    // The sprite at `place` of six along a row at height `y`, from x = -2.5 to 2.5, in `look`,
    // turned 25 degrees more than the one before it, and wider or taller by turns.
    let sprite = |place: usize, y: f32, look: usize| {
        let (texture, color) = looks[look];
        let size = if place.is_multiple_of(2) {
            Vec3::new(0.8, 0.6, 1.0)
        } else {
            Vec3::new(0.6, 0.8, 1.0)
        };
        let transform = Mat4::from_scale_rotation_translation(
            size,
            Quat::from_rotation_z(math::to_radians(place as f32 * 25.0)),
            Vec3::new(place as f32 - 2.5, y, 0.0),
        );
        let sprite = Sprite {
            color,
            texture,
            sampler: Sampler::NearestClamp,
            ..Sprite::default()
        };
        (transform, sprite)
    };
    // Positions, and a color for each draw.
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // Across the middle, from y = -0.45 to 0.45, in slate.
    let band = Mat4::from_scale(Vec3::new(5.8, 0.9, 1.0));
    let slate = Vec4::new(0.25, 0.3, 0.4, 1.0);
    let away = Mat4::from_translation(Vec3::new(100.0, 100.0, 0.0));
    let unseen = Sprite {
        color: Vec4::new(1.0, 0.0, 1.0, 1.0),
        texture: Some(quadrants),
        ..Sprite::default()
    };
    let mut draws = Draws::default();
    // More frames than are in flight, the last in the slot of the first.
    for frame in 0..3 {
        draws.clear();
        for place in 0..6 {
            let (transform, sprite) = sprite(place, 1.2, place);
            draws.push_sprite(transform, sprite);
        }
        draws.push(square, flat, band, slate);
        if frame == 2 {
            for i in 0..BIG {
                // The first three and the last three are seen, at the bottom's six places.
                let place = match i {
                    0..3 => i,
                    _ if i >= BIG - 3 => i + 6 - BIG,
                    _ => {
                        draws.push_sprite(away, unseen);
                        continue;
                    }
                };
                let (transform, sprite) = sprite(place, -0.7, 5 - place);
                draws.push_sprite(transform, sprite);
            }
        }
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    let drew = DrawCounts {
        draws: 3,
        instances: 6 + 1 + BIG,
        triangles: 2 * (6 + 1 + BIG),
    };
    let viewport = filling([WIDTH, HEIGHT]);
    let drawn = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    assert_eq!(drawn, drew, "each run is one instanced draw");
    check(renderer, "batches", WIDTH, HEIGHT);
}

/// Draws a sprite sheet's cells, packed edge to edge, into an offscreen image, magnified, and
/// compares it with its reference image. Each cell is 2 by 2 texels in a hue unlike those of the
/// cells beside it, so that a sample of a neighbor's texel shows. In four rows: filtered linearly,
/// two cells, two side by side, which blend into each other across their seam, and one flipped; by
/// the nearest texel, the same, the first two turned a quarter and a half; a rectangle of pixels
/// across cells, filtered linearly, and a rectangle that cuts texels 0.3 of a texel from its edges,
/// by the nearest texel and linearly, whose slivers of those texels still show; and filtered
/// linearly, a cell of a sheet 46 texels across, whose edges' texture coordinates times 46 round
/// to just past them, the sheet tiled past its edge by a repeating sampler, whose outermost texels
/// don't blend with those they'd wrap to, and a rectangle of no width, which shows the texel to its
/// right. Every edge, and every texel's edge, falls between pixels: whether a nearest sampler takes
/// a neighbor's texel where an edge lies on a pixel's center depends on how a GPU rounds, so the
/// linear sprites check the bounds the shader keeps to, which are the same for both. The sheets'
/// odd sizes, 8 by 4 and 46 by 2, check that each way of uploading copies their rows whole.
fn sheets_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 5 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 5.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // 4 by 2 cells, sRGB-encoded: red, cyan, yellow and blue over green, magenta, slate and orange,
    // each lightest at its top left and darkest at its bottom right.
    let hues: [[u8; 3]; 8] = [
        [220, 40, 40],
        [40, 200, 220],
        [230, 210, 40],
        [40, 60, 220],
        [40, 190, 60],
        [210, 50, 200],
        [60, 60, 70],
        [240, 140, 30],
    ];
    let pixels: Vec<u8> = (0..32_usize)
        .flat_map(|i| {
            let (x, y) = (i % 8, i / 8);
            let shade = 1.0 - 0.2 * (x % 2 + y % 2) as f32;
            let [r, g, b] = hues[y / 2 * 4 + x / 2].map(|c| (f32::from(c) * shade) as u8);
            [r, g, b, 255]
        })
        .collect();
    let sheet = SpriteSheet {
        texture: (stores.textures.add("sheet", [8, 4], &pixels, Mipmaps::None)).expect("a texture"),
        size: [8, 4],
        cell: [2, 2],
    };
    // 23 cells of 2 by 2 texels, yellow and blue by turns, sRGB-encoded. Cell 14's edges are
    // texels 28 and 30, whose texture coordinates times 46 come to 27.999998 and 30.000002.
    let strip: Vec<u8> = (0..92_usize)
        .flat_map(|i| {
            if i % 46 / 2 % 2 == 0 {
                [230, 210, 40, 255]
            } else {
                [40, 60, 220, 255]
            }
        })
        .collect();
    let strip = SpriteSheet {
        texture: (stores.textures.add("strip", [46, 2], &strip, Mipmaps::None)).expect("a texture"),
        size: [46, 2],
        cell: [2, 2],
    };
    // `width` by `height` units, turned `turn` counterclockwise, centered at (`x`, `y`).
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2], turn: f32| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(width, height, 1.0),
            Quat::from_rotation_z(turn),
            Vec3::new(x, y, 0.0),
        )
    };
    let mut flipped = sheet.sprite([2, 0], [1, 1]);
    let [top_left, bottom_right] = flipped.rect;
    flipped.rect = [
        Vec2::new(bottom_right.x, top_left.y),
        Vec2::new(top_left.x, bottom_right.y),
    ];
    let mut draws = Draws::default();
    // Each cell 0.8 units, 32 pixels, across: 16 pixels a texel.
    for (y, sampler, turns) in [
        (1.8, Sampler::LinearClamp, [0.0, 0.0]),
        (0.6, Sampler::NearestClamp, [FRAC_PI_2, PI]),
    ] {
        let cells = [
            (
                sheet.sprite([1, 0], [1, 1]),
                [-2.2, y],
                [0.8, 0.8],
                turns[0],
            ),
            (
                sheet.sprite([2, 1], [1, 1]),
                [-1.0, y],
                [0.8, 0.8],
                turns[1],
            ),
            (sheet.sprite([1, 1], [2, 1]), [0.6, y], [1.6, 0.8], 0.0),
            (flipped, [2.2, y], [0.8, 0.8], 0.0),
        ];
        for (sprite, at, size, turn) in cells {
            draws.push_sprite(place(at, size, turn), Sprite { sampler, ..sprite });
        }
    }
    // Texels 1 to 5 across and 1 to 3 down, 16 pixels a texel.
    let pixel_cells = SpriteSheet {
        cell: [1, 1],
        ..sheet
    };
    let across = pixel_cells.sprite([1, 1], [4, 2]);
    draws.push_sprite(place([-1.8, -0.6], [1.6, 0.8], 0.0), across);
    // Texels 1.7 to 3.3 across and 0.7 to 2.3 down, 20 pixels a texel, so that 6 pixels of each
    // edge show the texel the edge cuts.
    let cut = Sprite {
        texture: Some(sheet.texture),
        rect: [
            Vec2::new(1.7 / 8.0, 0.7 / 4.0),
            Vec2::new(3.3 / 8.0, 2.3 / 4.0),
        ],
        ..Sprite::default()
    };
    for (x, sampler) in [(0.0, Sampler::NearestClamp), (1.6, Sampler::LinearClamp)] {
        draws.push_sprite(place([x, -0.6], [0.8, 0.8], 0.0), Sprite { sampler, ..cut });
    }
    // 24 pixels a texel across.
    let between = strip.sprite([14, 0], [1, 1]);
    draws.push_sprite(place([-2.0, -1.8], [1.2, 0.8], 0.0), between);
    // Texels 6 to 10 across, past the sheet's right edge, and all 4 down: 12 pixels a texel across
    // and 8 down.
    let tiled = Sprite {
        texture: Some(sheet.texture),
        rect: [Vec2::new(0.75, 0.0), Vec2::new(1.25, 1.0)],
        sampler: Sampler::LinearRepeat,
        ..Sprite::default()
    };
    draws.push_sprite(place([-0.4, -1.8], [1.2, 0.8], 0.0), tiled);
    // On the edge between texels 3 and 4, down the whole sheet.
    let line = Sprite {
        texture: Some(sheet.texture),
        rect: [Vec2::new(0.5, 0.0), Vec2::new(0.5, 1.0)],
        ..Sprite::default()
    };
    draws.push_sprite(place([0.8, -1.8], [0.4, 0.8], 0.0), line);
    // More frames than are in flight.
    for _ in 0..3 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "sheets", WIDTH, HEIGHT);
}

/// Draws the `meshes` test's squares and triangle, then along the bottom of the last view a band
/// one texel a pixel, into a view that changes size from frame to frame, and compares the view as
/// it shows in the frame with its reference image. The view is the whole image, then shut, which
/// draws nothing and leaves the frame its clear color alone, then 160 by 180 pixels, then 160 by
/// 160 at [100, 20], 40 pixels a unit, as the reference shows it from the frame that gives it that
/// size; then 200 by 140, 200 by 160, and 160 by 160 again for three frames, which must show it as
/// before. Each side changes alone somewhere, so that an image made again only when one side
/// changes shows too. Against the reference, an image left the size it first had makes 15,217
/// pixels differ, an aspect ratio taken from the whole image's shape 8,917, the view shown at the
/// image's corner 22,662, shown past its place 38,400, and its image made again a frame late, or
/// only for a new width, 8,523; shown a pixel off, across, down or both, the band's texels, of
/// four colors, change its 3,200 pixels. The fourth frame makes the view's image again while the
/// third may still draw with the one before, which validation sees destroyed only once the GPU is
/// done with it.
fn views_match_their_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top; the view's center is (1, 0.5).
    let viewpoint = Viewpoint {
        position: DVec3::new(1.0, 0.5, 0.0),
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let transform = |scale, turn: f32, [x, y]: [f32; 2]| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_z(math::to_radians(turn)),
            Vec3::new(x, y, 0.0),
        )
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let colored = (stores.shaders)
        .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
    let vertices = [
        corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
        corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
        corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let triangle = (stores.meshes)
        .add("triangle", &vertices, &[0, 1, 2])
        .expect("a mesh");
    // White, red, blue and black, each unlike the others, so that a texel shifted to its neighbor's
    // place, across, down or both, shows another color.
    let tile = [
        [255, 255, 255, 255],
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        [0, 0, 0, 255],
    ];
    let tile = (stores.textures)
        .add("tile", [2, 2], tile.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    draws.push(square, flat, transform(2.5, 30.0, [0.5, 0.5]), blue);
    let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
    draws.push(square, flat, transform(0.8, 0.0, [3.2, 1.8]), orange);
    draws.push(triangle, colored, transform(2.0, 0.0, [1.0, 0.5]), ());
    // Across the last view's bottom, 4 units by half a unit: 160 by 20 pixels, the tile repeated
    // to a texel each, whose centers are the pixels' centers.
    let band = Sprite {
        texture: Some(tile),
        rect: [Vec2::ZERO, Vec2::new(80.0, 10.0)],
        sampler: Sampler::NearestRepeat,
        ..Sprite::default()
    };
    let along = Mat4::from_translation(Vec3::new(1.0, -1.25, 0.0))
        * Mat4::from_scale(Vec3::new(4.0, 0.5, 1.0));
    draws.push_sprite(along, band);

    let view = |origin, size| Viewport { origin, size };
    let views = [
        view([0, 0], [WIDTH, HEIGHT]),
        view([100, 20], [0, 160]),
        view([40, 10], [160, 180]),
        view([100, 20], [160, 160]),
        view([100, 20], [200, 140]),
        view([100, 20], [200, 160]),
        view([100, 20], [160, 160]),
        view([100, 20], [160, 160]),
        view([100, 20], [160, 160]),
    ];
    let read_back = |renderer: &mut Renderer| {
        (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back")
    };
    // Everything drawable first, in the view the frames start with.
    settle(&mut renderer, &mut stores, views[0], viewpoint, &draws);
    let mut shown = None;
    for (frame, viewport) in views.into_iter().enumerate() {
        let data = stores.data(viewport, viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        match frame {
            1 => {
                let pixels = read_back(&mut renderer);
                // `CLEAR`, encoded.
                let clear = [39, 48, 63, 255];
                for pixel in pixels.as_chunks::<4>().0 {
                    let close = pixel.iter().zip(clear).all(|(&a, b)| a.abs_diff(b) <= 1);
                    assert!(close, "a shut view left {pixel:?}, not the clear color");
                }
            }
            3 => {
                // What the shut view drew, nothing, which comes back two frames later.
                assert_eq!(renderer.drawn(), DrawCounts::default());
                let pixels = read_back(&mut renderer);
                compare("views", WIDTH, HEIGHT, &pixels);
                shown = Some(pixels);
            }
            _ => {}
        }
    }
    // Two squares, a triangle and a sprite's quad.
    let drawn = DrawCounts {
        draws: 3,
        instances: 4,
        triangles: 7,
    };
    assert_eq!(renderer.drawn(), drawn);
    assert!(
        Some(read_back(&mut renderer)) == shown,
        "the view showed otherwise once it was 160 by 160 again and kept that size"
    );
    check(renderer, "views", WIDTH, HEIGHT);
}

/// Draws squares and sprites into an offscreen image, as a camera with perspective sees them, and
/// compares it with its reference image: two opaque squares that cross, each hiding the half of
/// the other behind it; a nearer opaque square drawn before a farther one, which it hides; two
/// opaque squares in one plane facing the camera, the later over the earlier, as draws at one z
/// stack in 2D; a translucent sprite drawn after an opaque square in front of it, which hides the
/// middle of it; and two translucent sprites, the nearer drawn first, which the farther goes over,
/// since what blends tests depth without writing it. In the first frame alone, an opaque square
/// nearer than the rest covers most of them, which the later frames, their depth cleared, don't
/// remember. Against the reference, opaque squares that don't write depth make 2,957 pixels
/// differ, sprites that write it 1,118, and sprites that don't test it 1,444.
fn depth_matches_its_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 in front of the origin, seeing a sixth of a turn from the view's bottom to its top: at z =
    // 0, about 4.6 units.
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // `size` across at `at`, turned `turn` degrees about Y.
    let placed = |size: f32, turn: f32, at: [f32; 3]| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(size),
            Quat::from_rotation_y(math::to_radians(turn)),
            Vec3::from(at),
        )
    };
    let color = |r, g, b| Vec4::new(r, g, b, 1.0);
    let translucent = |color: [f32; 4]| Sprite {
        color: Vec4::from(color),
        ..Sprite::default()
    };
    let scene = |occluded: bool| {
        let mut draws = Draws::default();
        if occluded {
            let gray = color(0.5, 0.5, 0.5);
            draws.push(square, flat, placed(3.0, 0.0, [0.0, 0.0, 2.0]), gray);
        }
        let crossing = [(0.0, color(0.8, 0.1, 0.1)), (60.0, color(0.1, 0.7, 0.1))];
        for (turn, color) in crossing {
            draws.push(square, flat, placed(1.4, turn, [-1.8, 0.8, 0.0]), color);
        }
        let blue = color(0.1, 0.2, 0.8);
        draws.push(square, flat, placed(1.0, 0.0, [1.4, 0.9, 1.0]), blue);
        let yellow = color(0.8, 0.7, 0.1);
        draws.push(square, flat, placed(1.6, 0.0, [1.9, 0.6, -1.0]), yellow);
        let white = color(0.9, 0.9, 0.9);
        draws.push(square, flat, placed(0.8, 0.0, [-0.3, 1.6, 0.0]), white);
        let purple = color(0.4, 0.1, 0.6);
        draws.push(square, flat, placed(0.8, 0.0, [0.1, 1.4, 0.0]), purple);
        let magenta = color(0.8, 0.1, 0.6);
        draws.push(square, flat, placed(1.0, 0.0, [-1.5, -1.2, 0.3]), magenta);
        let behind = translucent([1.0, 1.0, 1.0, 0.6]);
        draws.push_sprite(placed(2.0, 0.0, [-1.95, -1.55, -0.8]), behind);
        let nearer = translucent([0.1, 0.9, 0.9, 0.5]);
        draws.push_sprite(placed(1.2, 0.0, [1.2, -1.1, 0.5]), nearer);
        let farther = translucent([0.9, 0.5, 0.1, 0.5]);
        draws.push_sprite(placed(1.4, 0.0, [1.6, -0.8, -0.5]), farther);
        draws
    };
    for frame in 0..3 {
        let draws = scene(frame == 0);
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &scene(false),
    );
    check(renderer, "depth", WIDTH, HEIGHT);
}

/// Draws ten instances of a square with two levels of detail, as a camera with perspective sees
/// them, and compares it with its reference image. The square is 2 across: level 0 is its two
/// triangles, and level 1 one other triangle on its own, the lower left half, whose error spans a
/// pixel where its sphere's nearest point is 5 away, 6.4 from the camera. Along the top, five
/// instances through an opaque shader, 4 to 12 away, in the order near, far, near, far, far, so
/// that drawing them by level takes two draws, not four; along the bottom, five through a shader
/// that blends, far, near, near, far, near, which draw in that order, so that the same takes four.
/// Each is a color of its own, and an ID, which picks read back. The near ones show whole squares,
/// and the far ones the lower left halves of theirs; the draws and triangles counted are the
/// levels' (6 draws, 10 instances, 15 triangles), where the first level alone makes 20 triangles
/// and 2 draws, and the second 10 and 2.
fn levels_of_detail_match_their_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // A quarter turn from the view's bottom to its top, over 200 pixels, and as far across over
    // 320: 100 pixels a unit where the distance ahead is 1, and 100 / distance where it is more.
    let viewpoint = Viewpoint {
        projection: Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let flat = |stores: &mut Stores, name, blend| {
        (stores.shaders)
            .add::<[f32; 3], Vec4>(name, &shader("flat_color.slang"), blend)
            .expect("a shader")
    };
    let opaque = flat(&mut stores, "flat color", Blend::Opaque);
    let blended = flat(&mut stores, "flat color blended", Blend::Alpha);
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    // Counterclockwise: level 0's two halves, the lower right and the upper left, then level 1's
    // lower left half, which neither of them is, so that the wrong start shows.
    let indices = [0, 1, 2, 2, 3, 0, 3, 0, 1];
    let lods = Lods {
        levels: vec![
            Lod {
                start: 0,
                count: 6,
                error: 0.0,
            },
            Lod {
                start: 6,
                count: 3,
                error: 0.05,
            },
        ],
        center: Vec3::ZERO,
        radius: SQRT_2,
    };
    let square = (stores.meshes)
        .add_lods("square", &corners, &indices, lods)
        .expect("a mesh");
    // Each instance's center in the view's pixels, its distance ahead, color and level. The
    // second level's error spans 0.05 * 100 / (distance - 1.41) pixels: 1.6 at 4.5, 0.66 at 9.
    let blue = Vec4::new(0.1, 0.2, 0.8, 1.0);
    let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
    let green = Vec4::new(0.1, 0.7, 0.2, 1.0);
    let yellow = Vec4::new(0.8, 0.7, 0.1, 1.0);
    let violet = Vec4::new(0.6, 0.2, 0.9, 1.0);
    let see_through = |color: Vec4| Vec4::new(color.x, color.y, color.z, 0.6);
    let top = [
        ([30.0, 55.0], 4.5, orange, 0),
        ([95.0, 55.0], 9.0, green, 1),
        ([160.0, 55.0], 4.0, blue, 0),
        ([225.0, 55.0], 12.0, yellow, 1),
        ([290.0, 55.0], 10.0, violet, 1),
    ];
    let bottom = [
        ([30.0, 150.0], 9.0, see_through(violet), 1),
        ([95.0, 150.0], 4.5, see_through(orange), 0),
        ([160.0, 150.0], 4.0, see_through(green), 0),
        ([225.0, 150.0], 12.0, see_through(blue), 1),
        ([290.0, 150.0], 5.0, see_through(yellow), 0),
    ];
    let rows = [(opaque, top), (blended, bottom)];
    let instances = (rows.into_iter()).flat_map(|(shader, row)| row.map(|row| (shader, row)));
    let mut draws = Draws::default();
    let mut picks = Vec::new();
    // What each instance shows, by its ID, its draw's number from 1: its pixels at (-0.7, -0.2) in
    // the square's own space, inside level 1's triangle, and at (0.2, 0.7), outside it, both off
    // the diagonal that level 0's halves meet at, by 3 pixels or more at a distance of 12; its
    // color; and its level.
    let mut shows = Vec::new();
    for (id, (shader, ([x, y], depth, color, level))) in (1..).zip(instances) {
        // The world point that shows at the center, `depth` ahead.
        let at = Vec3::new(
            (x - 160.0) * depth / 100.0,
            (100.0 - y) * depth / 100.0,
            -depth,
        );
        let draw = MeshDraw::new(square, shader, color);
        draws.push_draw(Mat4::from_translation(at), &draw, id);
        let pixels = |[mesh_x, mesh_y]: [f32; 2]| {
            let at = [x + mesh_x * 100.0 / depth, y - mesh_y * 100.0 / depth];
            at.map(|side| side as u32)
        };
        let (inside, outside) = (pixels([-0.7, -0.2]), pixels([0.2, 0.7]));
        picks.push(Pick {
            tag: id,
            pixel: inside,
        });
        shows.push((id, inside, outside, color, level));
    }
    // Everything drawable first; then three frames, more than the picks of the first need to be
    // answered.
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    let drew = DrawCounts {
        draws: 6,
        instances: 10,
        triangles: 15,
    };
    for frame in 0..3 {
        let asked: &[Pick] = if frame == 0 { &picks } else { &[] };
        let data = RenderData {
            picks: asked,
            ..stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
    assert_eq!(
        renderer.drawn(),
        drew,
        "the first frame's, as the third began"
    );
    let found: Vec<(u64, u64)> = shows.iter().map(|&(id, ..)| (id, id)).collect();
    assert_eq!(renderer.picked(), found, "each pick found its instance");

    let frame = frame_pixels(&mut renderer);
    shut_down(renderer);
    let pixel = |[x, y]: [u32; 2]| {
        let at = ((y * WIDTH + x) * 4) as usize;
        <[u8; 3]>::try_from(&frame[at..at + 3]).expect("a pixel")
    };
    let clear = pixel([2, 2]);
    for (id, inside, outside, color, level) in shows {
        // Its color over the clear color, as the target blends them, in linear space.
        let over = |channel: usize| color[channel] * color.w + CLEAR[channel] * (1.0 - color.w);
        let expected = [0, 1, 2].map(over).map(encoded);
        let shown = pixel(inside);
        let close = (shown.iter().zip(expected)).all(|(&got, want)| got.abs_diff(want) <= 3);
        assert!(close, "instance {id} shows {shown:?}, not {expected:?}");
        // Level 0 draws the square's other half, and level 1 leaves it clear.
        let other = pixel(outside);
        match level {
            0 => assert_eq!(other, shown, "instance {id}'s other half"),
            _ => assert_eq!(other, clear, "instance {id}'s other half"),
        }
    }
    compare("levels", WIDTH, HEIGHT, &frame);
}

/// Fills the view with a texture of 4,096 by 4,096 texels of one color, 64 MiB, which the default
/// budget streams over four frames, and checks that every pixel shows that color once it's
/// drawable: every texel the view samples must have been copied, over the frames, each part a
/// whole number of rows.
fn a_large_texture_draws_once_streamed(path: UploadPath) {
    const SIDE: u32 = 32;
    const TEXELS: u32 = 4096;
    // sRGB-encoded.
    const COLOR: [u8; 4] = [200, 80, 40, 255];
    let mut renderer = renderer_uploading(SIDE, SIDE, Some(path));
    let mut stores = Stores::default();
    // Positions, then texture coordinates.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, which is 2 units high and as wide.
    let corners = [
        [-1.0, -1.0, 0.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let pixels = COLOR.repeat((TEXELS * TEXELS) as usize);
    let large = (stores.textures)
        .add("large", [TEXELS, TEXELS], &pixels, Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    let sampling = Sampling {
        texture: large,
        sampler: Sampler::NearestClamp.index(),
    };
    draws.push(square, textured, Mat4::IDENTITY, sampling);
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let data = stores.data(filling([SIDE, SIDE]), viewpoint, &draws);
    render_without_ui(&mut renderer, data);
    // The texture streams on, with whatever else of the first frame's isn't drawable yet.
    let streaming = renderer.streaming();
    assert!(
        streaming.assets >= 1 && streaming.bytes < 64 << 20 && streaming.bytes >= 48 << 20,
        "a frame of the default budget copied: {streaming:?}"
    );
    settle(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    let pixels = frame_pixels(&mut renderer);
    shut_down(renderer);
    for pixel in pixels.as_chunks::<4>().0 {
        assert!(
            close(*pixel, COLOR),
            "read back {pixel:?}, expected {COLOR:?}"
        );
    }
}

/// What a frame shows at a pixel of [`large_assets_stream_over_frames_within_the_budget`]: the
/// clear color, where its mesh isn't drawable; the stand-in's white or zeros, where its mesh draws
/// a texture that isn't; or the asset's own color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    Clear,
    StandIn,
    Own,
}

/// Streams assets larger than a budget of 64 KiB a frame over several frames, by `path`: a square
/// textured with a 256 by 256 texture of colors, 256 KiB; another with a 192 by 128 texture of
/// data, 96 KiB, which shaders read as stored and the frame's target then encodes; and a mesh of
/// 9,216 vertices and 54,150 indices, 324 KiB, in a flat color. Each frame reads back a pixel of
/// each: the square of colors shows the stand-in, white, until the frame that shows its texture,
/// having shown the clear color before its mesh was drawable, if at all; the square of data shows
/// the clear color until its texture is there, since its stand-in's zeros are fully transparent,
/// which the shader discards, as text does, where white would show; the large mesh shows the clear
/// color until it's drawable; and each shows its own color exactly from then on. What's streaming
/// counts down, and nothing that isn't drawable is counted drawn.
fn large_assets_stream_over_frames_within_the_budget(path: UploadPath) {
    const SIDE: u32 = 64;
    const GRID: u32 = 96;
    // sRGB-encoded, and data.
    const COLORS: [[u8; 4]; 2] = [[200, 80, 40, 255], [40, 160, 220, 255]];
    let mut renderer = renderer_within(SIDE, SIDE, Some(path), SMALL_BUDGET);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    // Positions, then texture coordinates; opaque, so that the zeros of data show as black.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // A unit square, its texture over it.
    let corners = [
        [-0.5, -0.5, 0.0, 0.0, 1.0],
        [0.5, -0.5, 0.0, 1.0, 1.0],
        [0.5, 0.5, 0.0, 1.0, 0.0],
        [-0.5, 0.5, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    // A grid of GRID by GRID vertices over the unit square, two triangles a cell.
    let vertices: Vec<[f32; 3]> = (0..GRID * GRID)
        .map(|i| {
            let (x, y) = (i % GRID, i / GRID);
            let along = |at: u32| at as f32 / (GRID - 1) as f32 - 0.5;
            [along(x), along(y), 0.0]
        })
        .collect();
    let indices: Vec<u32> = (0..(GRID - 1) * (GRID - 1))
        .flat_map(|cell| {
            let (x, y) = (cell % (GRID - 1), cell / (GRID - 1));
            let at = |dx, dy| (y + dy) * GRID + x + dx;
            [at(0, 0), at(1, 0), at(1, 1), at(1, 1), at(0, 1), at(0, 0)]
        })
        .collect();
    let dense = (stores.meshes)
        .add("dense", &vertices, &indices)
        .expect("a mesh");
    let large = COLORS[0].repeat(256 * 256);
    let large = (stores.textures)
        .add("large", [256, 256], &large, Mipmaps::None)
        .expect("a texture");
    let field = COLORS[1].repeat(192 * 128);
    let field = (stores.textures)
        .add("field", [192, 128], &field, Texels::Data)
        .expect("a texture");
    let mesh_bytes = (vertices.len() * 12 + indices.len() * 4) as u64;
    let total = mesh_bytes + 256 * 256 * 4 + 192 * 128 * 4;
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // The textured squares in the top quadrants, the dense mesh across the bottom half.
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let mut draws = Draws::default();
    let nearest = Sampler::NearestClamp.index();
    let sampling = |texture| Sampling {
        texture,
        sampler: nearest,
    };
    draws.push(
        square,
        textured,
        place([-0.5, 0.5], [1.0, 1.0]),
        sampling(large),
    );
    draws.push(
        square,
        textured,
        place([0.5, 0.5], [1.0, 1.0]),
        sampling(field),
    );
    let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
    draws.push(dense, flat, place([0.0, -0.5], [2.0, 1.0]), gray);
    // What each pixel shows frame by frame, and what the frames count drawn and streaming.
    let mut shown: [Vec<Shows>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut streaming = Vec::new();
    let mut instances = Vec::new();
    for _ in 0..200 {
        let data = stores.data(filling([SIDE, SIDE]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        streaming.push(renderer.streaming());
        instances.push(renderer.drawn().instances);
        let frame = frame_pixels(&mut renderer);
        let zeros = [0, 0, 0, 0];
        // Alpha isn't read where the zeros draw, as the view's pass leaves it.
        let rgb = |pixel: [u8; 4]| [pixel[0], pixel[1], pixel[2], 255];
        let shows = |at: [u32; 2], own: [u8; 4], stand_in: [u8; 4]| {
            let pixel = rgb(pixel_at(&frame, SIDE, at));
            if close(pixel, CLEARED) {
                Shows::Clear
            } else if close(pixel, rgb(stand_in)) {
                Shows::StandIn
            } else if close(pixel, own) {
                Shows::Own
            } else {
                panic!("by {path}: {pixel:?} at {at:?} is none of the colors expected")
            }
        };
        let data = COLORS[1].map(|channel| encoded(f32::from(channel) / 255.0));
        shown[0].push(shows([16, 16], COLORS[0], [255; 4]));
        shown[1].push(shows([48, 16], rgb(data), zeros));
        shown[2].push(shows([32, 48], [188, 188, 188, 255], [255; 4]));
        if renderer.streaming().is_empty() {
            break;
        }
    }
    // What each frame drew comes back two frames later.
    for _ in 0..2 {
        let data = stores.data(filling([SIDE, SIDE]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        instances.push(renderer.drawn().instances);
    }
    let instances = &instances[2..];
    shut_down(renderer);
    let frames = streaming.len();
    assert!(
        frames >= (total / SMALL_BUDGET) as usize && frames < 200,
        "by {path}: {frames} frames for {total} bytes"
    );
    // Each pixel goes through its phases in order, the white stand-in's for at least a frame, and
    // ends on its own color.
    for (i, shown) in shown.iter().enumerate() {
        let phases: Vec<Shows> = shown.chunk_by(|a, b| a == b).map(|run| run[0]).collect();
        let expected: &[Shows] = match (i, phases.first()) {
            (0, Some(Shows::Clear)) => &[Shows::Clear, Shows::StandIn, Shows::Own],
            (0, _) => &[Shows::StandIn, Shows::Own],
            _ => &[Shows::Clear, Shows::Own],
        };
        assert_eq!(phases, expected, "by {path}: pixel {i} showed {shown:?}");
    }
    assert!(
        shown[0].contains(&Shows::StandIn),
        "by {path}: the texture never showed its stand-in: {:?}",
        shown[0]
    );
    // The bytes still to copy fall from under the total, by at most the budget's worth of whole
    // parts a frame, to 0, as the assets go.
    assert!(streaming[0].bytes < total, "by {path}: {:?}", streaming[0]);
    for pair in streaming.windows(2) {
        assert!(
            pair[1].bytes <= pair[0].bytes && pair[1].assets <= pair[0].assets,
            "by {path}: {pair:?}"
        );
    }
    let last = streaming.last().expect("a frame");
    assert_eq!((last.assets, last.bytes), (0, 0), "by {path}");
    // Until the dense mesh is drawable, its instance isn't counted.
    assert_eq!(*instances.last().expect("a frame"), 3, "by {path}");
    assert!(instances[0] < 3, "by {path}: {instances:?}");
}

/// Replaces a texture and a mesh with larger ones, 256 KiB and 324 KiB, which a budget of 64 KiB
/// streams over several frames, by `path`, and reads back a pixel of each every frame: the square
/// shows the old texture, red, never white, until the frame that shows the new, green; and the
/// mesh, a square that covers the view, keeps covering its right half until the frame that takes
/// the new, which covers the left half alone.
fn a_reload_draws_the_old_until_the_new_is_drawable(path: UploadPath) {
    const SIDE: u32 = 16;
    const GRID: u32 = 96;
    let mut renderer = renderer_within(SIDE, SIDE, Some(path), SMALL_BUDGET);
    let mut stores = Stores::default();
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, which is 2 units high and as wide: positions, then texture coordinates.
    let whole = [
        [-1.0, -1.0, 0.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &whole, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
    let texture = (stores.textures)
        .add("color", [2, 2], red.repeat(4), Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    let sampling = Sampling {
        texture,
        sampler: Sampler::NearestClamp.index(),
    };
    draws.push(square, textured, Mat4::IDENTITY, sampling);
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    settle(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    let left_and_right = |renderer: &mut Renderer| {
        let frame = frame_pixels(renderer);
        [[SIDE / 4, SIDE / 2], [SIDE * 3 / 4, SIDE / 2]].map(|at| pixel_at(&frame, SIDE, at))
    };
    assert_eq!(left_and_right(&mut renderer), [red, red], "by {path}");
    // The left half, as a grid of GRID by GRID vertices, two triangles a cell.
    let vertices: Vec<[f32; 5]> = (0..GRID * GRID)
        .map(|i| {
            let (x, y) = (i % GRID, i / GRID);
            let along = |at: u32| at as f32 / (GRID - 1) as f32;
            [
                along(x) - 1.0,
                along(y) * 2.0 - 1.0,
                0.0,
                along(x),
                1.0 - along(y),
            ]
        })
        .collect();
    let indices: Vec<u32> = (0..(GRID - 1) * (GRID - 1))
        .flat_map(|cell| {
            let (x, y) = (cell % (GRID - 1), cell / (GRID - 1));
            let at = |dx, dy| (y + dy) * GRID + x + dx;
            [at(0, 0), at(1, 0), at(1, 1), at(1, 1), at(0, 1), at(0, 0)]
        })
        .collect();
    (stores.meshes)
        .replace("square", square, &vertices, &indices)
        .expect("the mesh replaced");
    (stores.textures)
        .replace(
            "color",
            texture,
            [256, 256],
            green.repeat(256 * 256),
            Mipmaps::None,
        )
        .expect("the texture replaced");
    let (mut texture_at, mut mesh_at) = (None, None);
    for frame in 0..200 {
        let data = stores.data(filling([SIDE, SIDE]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        let [left, right] = left_and_right(&mut renderer);
        if left == green && texture_at.is_none() {
            texture_at = Some(frame);
        }
        if close(right, CLEARED) && mesh_at.is_none() {
            mesh_at = Some(frame);
        }
        let expected_left = if texture_at.is_some() { green } else { red };
        assert_eq!(left, expected_left, "by {path}, frame {frame}");
        let expected_right = if mesh_at.is_some() {
            CLEARED
        } else {
            expected_left
        };
        assert!(
            close(right, expected_right),
            "by {path}, frame {frame}: {right:?}"
        );
        if renderer.streaming().is_empty() {
            break;
        }
    }
    assert!(
        texture_at.is_some_and(|at| at >= 3) && mesh_at.is_some_and(|at| at >= 3),
        "by {path}: the texture showed at {texture_at:?} and the mesh at {mesh_at:?}"
    );
    assert_eq!(left_and_right(&mut renderer)[0], green, "by {path}");
    shut_down(renderer);
}

/// A BC7 block of 4 by 4 texels all of one sRGB-encoded `color`, whose channels are odd: mode 6,
/// whose endpoints are 7 bits a channel and a bit shared by them all, the low one, with both
/// endpoints `color` and every texel's index 0. Its 128 bits go least significant first: the mode,
/// its 7th bit set; then each channel's two endpoints, red, green, blue and alpha; then the shared
/// bits, then the indices.
fn bc7_block(color: [u8; 4]) -> [u8; 16] {
    assert!(color.iter().all(|channel| channel % 2 == 1), "{color:?}");
    let mut bits = 1u128 << 6;
    for (i, channel) in color.into_iter().enumerate() {
        let high = u128::from(channel >> 1);
        bits |= high << (7 + 14 * i) | high << (14 + 14 * i);
    }
    // Both endpoints' shared low bits.
    bits |= 0b11 << 63;
    bits.to_le_bytes()
}

/// A square of BC7 textures, sampled as their blocks decode, on the way of uploading `path`: one of
/// a level, 13 by 5 texels, whose blocks cover its edges, filling a view of `SIDE` pixels a side at
/// its nearest texels; then one of a full chain, 64 texels a side, whose first level is one color
/// and the rest another, filling it trilinearly, so that it samples its third level; then the
/// first replaced by RGBA8, and that by BC7 of a chain, each sampled once the frames have streamed
/// it.
fn bc7_textures_sample_as_their_blocks(path: UploadPath) {
    const SIDE: u32 = 16;
    const COLORS: [[u8; 4]; 4] = [
        [201, 81, 41, 255],
        [21, 161, 61, 255],
        [41, 91, 221, 255],
        [233, 211, 33, 255],
    ];
    let mut renderer = renderer_uploading(SIDE, SIDE, Some(path));
    let mut stores = Stores::default();
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, which is 2 units high and as wide: positions, then texture coordinates.
    let corners = [
        [-1.0, -1.0, 0.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let blocks = |count: usize, color| bc7_block(color).repeat(count);
    let one = Texels::Bc7 { mipmapped: false };
    let chain = Texels::Bc7 { mipmapped: true };
    // 4x2 blocks.
    let level = (stores.textures)
        .add("level", [13, 5], blocks(8, COLORS[0]), one)
        .expect("a texture");
    // Its first level 16x16 blocks, then 8x8, 4x4, 2x2 and three of a block each.
    let rest = [64, 16, 4, 1, 1, 1]
        .map(|count| blocks(count, COLORS[2]))
        .concat();
    let levels = [blocks(256, COLORS[1]), rest].concat();
    let chained = (stores.textures)
        .add("chain", [64, 64], levels, chain)
        .expect("a texture");
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let mut frame = |stores: &mut Stores, texture, sampler, expected: [u8; 4], what: &str| {
        let mut draws = Draws::default();
        let sampling = Sampling { texture, sampler };
        draws.push(square, textured, Mat4::IDENTITY, sampling);
        settle(
            &mut renderer,
            stores,
            filling([SIDE, SIDE]),
            viewpoint,
            &draws,
        );
        let pixels = frame_pixels(&mut renderer);
        for pixel in pixels.as_chunks::<4>().0 {
            assert!(
                close(*pixel, expected),
                "{what}: read back {pixel:?}, expected {expected:?}"
            );
        }
    };
    let nearest = Sampler::NearestClamp.index();
    frame(&mut stores, level, nearest, COLORS[0], "a level");
    let trilinear = Wrap::Clamp.anisotropic();
    frame(
        &mut stores,
        chained,
        trilinear,
        COLORS[2],
        "a chain's third level",
    );
    let replaced =
        (stores.textures).replace("level", level, [2, 2], COLORS[3].repeat(4), Mipmaps::None);
    replaced.expect("RGBA8 in place of BC7");
    frame(
        &mut stores,
        level,
        nearest,
        COLORS[3],
        "RGBA8 in place of BC7",
    );
    let replaced = (stores.textures).replace(
        "level",
        level,
        [5, 5],
        [blocks(4, COLORS[1]), blocks(2, COLORS[1])].concat(),
        chain,
    );
    replaced.expect("BC7 in place of RGBA8");
    frame(
        &mut stores,
        level,
        nearest,
        COLORS[1],
        "BC7 in place of RGBA8",
    );
    shut_down(renderer);
}

/// A textured square that covers a view of `SIZE` pixels a side, red, whose mesh and texture are
/// replaced while the frames before may still draw them, over more frames than are in flight: the
/// mesh by one that covers the view's left half, the texture by a green one of another size, with
/// mip levels. The frames draw them once they're there, and validation, synchronization's among
/// it, sees the old ones go only once the GPU is done with them.
fn replaced_meshes_and_textures_draw_once_they_are_there(path: UploadPath) {
    const SIZE: u32 = 16;
    let mut renderer = renderer_uploading(SIZE, SIZE, Some(path));
    let mut stores = Stores::default();
    // Positions, then texture coordinates.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, which is 2 units high and as wide.
    let whole = [
        [-1.0, -1.0, 0.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 0.0, 0.0],
    ];
    let indices = [0, 1, 2, 2, 3, 0];
    let square = (stores.meshes)
        .add("square", &whole, &indices)
        .expect("a mesh");
    let red = [255, 0, 0, 255].repeat(4);
    let texture = (stores.textures)
        .add("color", [2, 2], &red, Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    let sampling = Sampling {
        texture,
        sampler: Sampler::NearestClamp.index(),
    };
    draws.push(square, textured, Mat4::IDENTITY, sampling);
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // The pixels a quarter of the way across and three quarters of the way, halfway down.
    let left_and_right = |renderer: &mut Renderer| {
        let pixels = (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back");
        let at = |x: u32| {
            let at = ((SIZE / 2 * SIZE + x) * 4) as usize;
            <[u8; 4]>::try_from(&pixels[at..at + 4]).expect("a pixel")
        };
        [at(SIZE / 4), at(SIZE * 3 / 4)]
    };
    let (red, green) = ([255, 0, 0, 255], [0, 255, 0, 255]);
    for frame in 0..6 {
        if frame == 3 {
            let left_half = whole.map(|[x, y, z, u, v]| [x.min(0.0), y, z, u, v]);
            (stores.meshes)
                .replace("square", square, &left_half, &indices)
                .expect("the mesh replaced");
            let pixels = green.repeat(4 * 4);
            (stores.textures)
                .replace("color", texture, [4, 4], &pixels, Mipmaps::Averaged)
                .expect("the texture replaced");
        }
        let data = stores.data(filling([SIZE, SIZE]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        // Reading back waits for the GPU, so not between the frames before the replacements and
        // the frame that takes them.
        match frame {
            0 => {
                settle(
                    &mut renderer,
                    &mut stores,
                    filling([SIZE, SIZE]),
                    viewpoint,
                    &draws,
                );
                assert_eq!(left_and_right(&mut renderer), [red, red], "red");
            }
            3 | 5 => {
                settle(
                    &mut renderer,
                    &mut stores,
                    filling([SIZE, SIZE]),
                    viewpoint,
                    &draws,
                );
                let [left, right] = left_and_right(&mut renderer);
                assert_eq!(left, green, "green on the left");
                assert!(![red, green].contains(&right), "the clear color, {right:?}");
            }
            _ => {}
        }
    }
    shut_down(renderer);
}

/// Removes a texture, a mesh and a shader, each drawn by a square of its own, and checks what the
/// next frame shows: the square of the texture removed shows its stand-in, white, and those of the
/// mesh and the shader removed show nothing, the clear color, which the frame counts no draw for.
/// Removing each again does nothing; an image or vertices for one removed are dropped, as a
/// shader compiled again for one would be; a texture removed while it streams, within a budget of
/// 64 KiB, shows the stand-in at once, with nothing left streaming; and one replaced while it
/// streams drops what was streaming for its replacement, which streams from the start.
fn removed_assets_draw_nothing_or_their_stand_in() {
    const SIDE: u32 = 64;
    let mut renderer = renderer_within(SIDE, SIDE, None, SMALL_BUDGET);
    let mut stores = Stores::default();
    let flat = |stores: &mut Stores, name| {
        (stores.shaders)
            .add::<[f32; 3], Vec4>(name, &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader")
    };
    let (flat_a, flat_b) = (flat(&mut stores, "flat a"), flat(&mut stores, "flat b"));
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0, 0.0, 1.0],
        [0.5, -0.5, 0.0, 1.0, 1.0],
        [0.5, 0.5, 0.0, 1.0, 0.0],
        [-0.5, 0.5, 0.0, 0.0, 0.0],
    ];
    let indices = [0, 1, 2, 2, 3, 0];
    let textured_square = (stores.meshes)
        .add("textured square", &corners, &indices)
        .expect("a mesh");
    let (plain_a, plain_b) = (square(&mut stores.meshes), square(&mut stores.meshes));
    let red = [255, 0, 0, 255];
    let texture = (stores.textures)
        .add("red", [2, 2], red.repeat(4), Mipmaps::None)
        .expect("a texture");
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // Four quadrants: the textured square, the two flat ones, and one for a texture that streams.
    let place = |[x, y]: [f32; 2]| Mat4::from_translation(Vec3::new(x, y, 0.0));
    let sampling = |texture| Sampling {
        texture,
        sampler: Sampler::NearestClamp.index(),
    };
    let scene = |streamed: Option<Texture>| {
        let mut draws = Draws::default();
        draws.push(
            textured_square,
            textured,
            place([-0.5, 0.5]),
            sampling(texture),
        );
        let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
        draws.push(plain_a, flat_a, place([0.5, 0.5]), gray);
        draws.push(plain_b, flat_b, place([-0.5, -0.5]), gray);
        if let Some(streamed) = streamed {
            draws.push(
                textured_square,
                textured,
                place([0.5, -0.5]),
                sampling(streamed),
            );
        }
        draws
    };
    let pixels = [[16, 16], [48, 16], [16, 48], [48, 48]];
    let shown = |renderer: &mut Renderer| {
        let frame = frame_pixels(renderer);
        pixels.map(|at| pixel_at(&frame, SIDE, at))
    };
    // The gray, linear 0.5, encodes to 187.5, which drivers round either way.
    let shows = |renderer: &mut Renderer, colors: [[u8; 4]; 4], what: &str| {
        let shown = shown(renderer);
        let matched = (shown.iter().zip(colors)).all(|(&pixel, color)| close(pixel, color));
        assert!(matched, "{what}: {shown:?}, not {colors:?}");
    };
    let (white, gray) = ([255; 4], [188, 188, 188, 255]);
    let draws = scene(None);
    settle(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    shows(&mut renderer, [red, gray, gray, CLEARED], "drawn");
    let drawn = drawn_again(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    assert_eq!(drawn.instances, 3);

    stores.textures.remove(texture);
    stores.textures.remove(texture);
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    let removed = [white, gray, gray, CLEARED];
    shows(&mut renderer, removed, "the texture removed");
    let drawn = drawn_again(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    assert_eq!(drawn.instances, 3);
    stores.meshes.remove(plain_a);
    stores.meshes.remove(plain_a);
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    let removed = [white, CLEARED, gray, CLEARED];
    shows(&mut renderer, removed, "the mesh removed");
    let drawn = drawn_again(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    assert_eq!(drawn.instances, 2);
    stores.shaders.remove(flat_b);
    stores.shaders.remove(flat_b);
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    let removed = [white, CLEARED, CLEARED, CLEARED];
    shows(&mut renderer, removed, "the shader removed");
    let drawn = drawn_again(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    assert_eq!(drawn.instances, 1);
    // An image and vertices for what's removed change nothing.
    (stores.textures)
        .replace("red", texture, [2, 2], red.repeat(4), Mipmaps::None)
        .expect("checked, then dropped");
    let quarter = corners.map(|[x, y, z, ..]| [x / 2.0, y / 2.0, z]);
    (stores.meshes)
        .replace("plain a", plain_a, &quarter, &indices)
        .expect("checked, then dropped");
    settle(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    shows(&mut renderer, removed, "dropped");

    // A texture of 256 KiB, removed after the first frame streams a part of it: its stand-in
    // shows at once, and nothing of it streams on.
    let green = [0, 255, 0, 255];
    let streamed = (stores.textures)
        .add(
            "streamed",
            [256, 256],
            green.repeat(256 * 256),
            Mipmaps::None,
        )
        .expect("a texture");
    let draws = scene(Some(streamed));
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    assert_eq!(renderer.streaming().assets, 1);
    assert_eq!(
        shown(&mut renderer)[3],
        white,
        "the stand-in while it streams"
    );
    stores.textures.remove(streamed);
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    assert!(
        renderer.streaming().is_empty(),
        "{:?}",
        renderer.streaming()
    );
    for _ in 0..3 {
        render_without_ui(
            &mut renderer,
            stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
        );
        assert_eq!(shown(&mut renderer)[3], white, "removed while it streamed");
    }

    // A texture replaced after the first frame streams a part of it: the replacement alone
    // streams, from the start, and shows once it's there.
    let blue = [0, 0, 255, 255];
    let replaced = (stores.textures)
        .add(
            "replaced",
            [256, 256],
            green.repeat(256 * 256),
            Mipmaps::None,
        )
        .expect("a texture");
    let draws = scene(Some(replaced));
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    let before = renderer.streaming();
    assert!(
        before.assets == 1 && before.bytes < 256 * 256 * 4,
        "{before:?}"
    );
    (stores.textures)
        .replace(
            "replaced",
            replaced,
            [256, 256],
            blue.repeat(256 * 256),
            Mipmaps::None,
        )
        .expect("the texture replaced");
    render_without_ui(
        &mut renderer,
        stores.data(filling([SIDE, SIDE]), viewpoint, &draws),
    );
    let after = renderer.streaming();
    assert!(
        after.assets == 1
            && after.bytes < 256 * 256 * 4
            && after.bytes >= 256 * 256 * 4 - SMALL_BUDGET,
        "the replacement alone, a part in: {after:?}"
    );
    assert_eq!(
        shown(&mut renderer)[3],
        white,
        "the stand-in until the replacement"
    );
    settle(
        &mut renderer,
        &mut stores,
        filling([SIDE, SIDE]),
        viewpoint,
        &draws,
    );
    assert_eq!(shown(&mut renderer)[3], blue, "the replacement");
    shut_down(renderer);
}

/// Draws models' meshes with materials into an offscreen image, as a camera with perspective sees
/// them, and compares it with its reference image. Along the top, single-sided squares whose
/// corners wind counterclockwise seen from in front, as glTF's fronts do: one facing the camera,
/// drawn; one turned away, culled; one mirrored by a negative scale, whose fronts then wind
/// clockwise, drawn; one mirrored and turned away, culled; and a double-sided one turned away,
/// drawn. Along the bottom: a masked square, mirrored, drawn after the mirrored ones by another
/// pipeline, which leaves the faces set as they were, showing only its texture's texels at least
/// half opaque; a blended square half over an opaque one behind it; and a square whose corners'
/// colors its material's color multiplies. Each run of one mesh with one shader that draws the
/// same faces is one draw, which `DrawCounts` counts with its triangles.
fn models_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 in front of the origin, seeing a sixth of a turn from the view's bottom to its top: at z =
    // 0, about 4.6 units.
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    // A unit square, counterclockwise seen from +Z, its texture's top left at its top left, in
    // `colors` from its bottom left counterclockwise.
    let mut square = |name, colors: [Vec4; 4]| {
        let corner = |[x, y]: [f32; 2], uv: [f32; 2], color| ModelVertex {
            position: Vec3::new(x, y, 0.0),
            uv: Vec2::from(uv),
            color,
            normal: Vec3::Z,
            tangent: Vec4::X,
        };
        let corners = [
            corner([-0.5, -0.5], [0.0, 1.0], colors[0]),
            corner([0.5, -0.5], [1.0, 1.0], colors[1]),
            corner([0.5, 0.5], [1.0, 0.0], colors[2]),
            corner([-0.5, 0.5], [0.0, 0.0], colors[3]),
        ];
        add_model(&mut stores.meshes, name, &corners, &[0, 1, 2, 2, 3, 0])
    };
    let plain = square("square", [Vec4::ONE; 4]);
    let (red_corner, green_corner) = (Vec4::new(1.0, 0.0, 0.0, 1.0), Vec4::new(0.0, 1.0, 0.0, 1.0));
    let blue_corner = Vec4::new(0.0, 0.0, 1.0, 1.0);
    let corners = [red_corner, green_corner, blue_corner, Vec4::ONE];
    let colored = square("colored square", corners);
    // Red and blue at least half opaque on the left, green and yellow less so on the right.
    let cutout = [
        [255, 64, 64, 255],
        [64, 255, 64, 40],
        [64, 64, 255, 200],
        [255, 255, 64, 100],
    ];
    let cutout = (stores.textures)
        .add("cutout", [2, 2], cutout.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let material = |color: [f32; 4], alpha, double_sided| Material {
        color: Vec4::from(color),
        alpha,
        double_sided,
        ..Material::default()
    };
    // 0.9 across at (x, y, z), turned a half turn about Y if `turned`, and mirrored left to right
    // if `mirrored`.
    let placed = |[x, y, z]: [f32; 3], turned: bool, mirrored: bool| {
        let turn = if turned { PI } else { 0.0 };
        let scale = Vec3::new(if mirrored { -0.9 } else { 0.9 }, 0.9, 0.9);
        Mat4::from_translation(Vec3::new(x, y, z))
            * Mat4::from_scale(scale)
            * Mat4::from_rotation_y(turn)
    };
    let opaque = |color, double_sided| material(color, AlphaMode::Opaque, double_sided);
    let red = opaque([0.8, 0.1, 0.1, 1.0], false);
    let green = opaque([0.1, 0.7, 0.1, 1.0], false);
    let blue = opaque([0.1, 0.2, 0.8, 1.0], false);
    let yellow = opaque([0.8, 0.7, 0.1, 1.0], false);
    let white = opaque([0.9, 0.9, 0.9, 1.0], true);
    let cyan = opaque([0.1, 0.7, 0.8, 1.0], false);
    let masked = Material {
        color_texture: Sampled {
            texture: cutout,
            nearest: true,
            wrap: Wrap::Clamp,
        },
        ..material([1.0; 4], AlphaMode::Mask { cutoff: 0.5 }, false)
    };
    let blended = material([0.9, 0.1, 0.8, 0.5], AlphaMode::Blend, false);
    let tinted = opaque([1.0, 0.5, 1.0, 1.0], false);
    let (materials, shaders) = (&mut stores.materials, &mut stores.shaders);
    let mut draw = |mesh, material: &Material| {
        let material = materials.add(material);
        MeshDraw::with_material(mesh, material, materials, shaders)
    };
    let scene = [
        (draw(plain, &red), placed([-2.0, 0.9, 0.0], false, false)),
        (draw(plain, &green), placed([-1.0, 0.9, 0.0], true, false)),
        (draw(plain, &blue), placed([0.0, 0.9, 0.0], false, true)),
        (draw(plain, &yellow), placed([1.0, 0.9, 0.0], true, true)),
        (draw(plain, &masked), placed([-1.6, -0.9, 0.0], false, true)),
        (draw(plain, &white), placed([2.0, 0.9, 0.0], true, false)),
        (draw(plain, &cyan), placed([0.0, -0.9, -0.2], false, false)),
        (
            draw(plain, &blended),
            placed([0.3, -0.7, 0.2], false, false),
        ),
        (
            draw(colored, &tinted),
            placed([1.6, -0.9, 0.0], false, false),
        ),
    ];
    let mut draws = Draws::default();
    for (draw, transform) in &scene {
        draws.push_draw(*transform, draw, 0);
    }
    for _ in 0..2 {
        let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    // Red and green facing one way, blue and yellow mirrored, then one each: cut into meshlets,
    // green's and yellow's, turned away, aren't drawn, as their cones cull them.
    let turned_away = if CUT_MESHLETS.load(Ordering::Relaxed) {
        4
    } else {
        0
    };
    let drawn = DrawCounts {
        draws: 7,
        instances: 9,
        triangles: 18 - turned_away,
    };
    let viewport = filling([WIDTH, HEIGHT]);
    // Past the frame the meshes came to be drawable in, which planned no room for their meshlets
    // and drew them by their indices.
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let drew = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    assert_eq!(drew, drawn);
    check(renderer, "models", WIDTH, HEIGHT);
}

/// Draws three copies of a skinned strip, unlit, in a column of quads up from a joint at its foot,
/// the rest moved by a joint at its middle, as an orthographic view sees them, and compares them
/// with their reference image: on the left, its upper half turned an eighth about its middle; in
/// the middle, slid half a unit right; and on the right, given no skin this frame, as it was bound,
/// straight up. Its lower half moves with the joint at its foot alone, which stays put, and a row
/// of vertices at its middle half with each. Each copy is a mesh of its own, posed apart, as each
/// instance of a skinned model is.
fn skinned_meshes_move_with_their_joints() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 4.0,
            near: 0.1,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    // Rows from y = -1 to 1, half a unit apart, each of two corners, a quarter of a unit either
    // side of x = 0: the lowest two moved by joint 0 alone, the middle half by each, the rest by
    // joint 1.
    let rows = [-1.0, -0.5, 0.0, 0.5, 1.0];
    let mut vertices = Vec::new();
    let mut skin = Vec::new();
    for (row, y) in rows.into_iter().enumerate() {
        let weights = match row {
            0 | 1 => [1.0, 0.0, 0.0, 0.0],
            2 => [0.5, 0.5, 0.0, 0.0],
            _ => [0.0, 1.0, 0.0, 0.0],
        };
        for x in [-0.25, 0.25] {
            vertices.push(ModelVertex {
                position: Vec3::new(x, y, 0.0),
                uv: Vec2::ZERO,
                color: Vec4::ONE,
                normal: Vec3::Z,
                tangent: Vec4::X,
            });
            skin.push(SkinVertex::new([0, 1, 0, 0], weights));
        }
    }
    let indices: Vec<u32> = (0..4)
        .flat_map(|row| {
            let at = 2 * row;
            [at, at + 1, at + 3, at + 3, at + 2, at]
        })
        .collect();
    let reach = [Vec3::new(-1.5, -1.5, -0.5), Vec3::new(1.5, 1.5, 0.5)];
    let strip = ModelMesh::pack_skinned("strip", &vertices, indices, Vec::new(), skin, reach)
        .expect("a mesh");
    let copies = ["bent strip", "slid strip", "straight strip"]
        .map(|name| stores.meshes.add_model(name, &strip).expect("a mesh"));
    let material = Material {
        color: Vec4::new(0.9, 0.5, 0.1, 1.0),
        unlit: true,
        double_sided: true,
        ..Material::default()
    };
    let material = stores.materials.add(&material);
    let (materials, shaders) = (&mut stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    for (copy, x) in copies.into_iter().zip([-1.6, 0.0, 1.6]) {
        let draw = MeshDraw::with_material(copy, material, materials, shaders);
        draws.push_draw(Mat4::from_translation(Vec3::new(x, 0.0, 0.0)), &draw, 0);
    }
    // About the strip's middle, where joint 1 was bound.
    let bent = Mat4::from_rotation_z(PI / 4.0);
    let slid = Mat4::from_translation(Vec3::new(0.5, 0.0, 0.0));
    draws.skin(copies[0], [Mat4::IDENTITY, bent]);
    draws.skin(copies[1], [Mat4::IDENTITY, slid]);
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    render_without_ui(
        &mut renderer,
        stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws),
    );
    check(renderer, "skinned", WIDTH, HEIGHT);
}

/// Emitters' particles over frames, as a camera with perspective sees them over a dark square and
/// under a square in front of the fire's base, against their reference image: on the left, a
/// fountain of discs that go over what's behind them by their alpha, thrown up a cone and pulled
/// down; in the middle, a fire of glowing discs that add their light, past 1, rising from a sphere
/// against drag; and on the right, a burst of sparks, spinning squares of a texture's cells. The
/// first frame bears most of the fountain's and the fire's, the second the rest, the fountain having
/// moved right between them, along which its births between them are born, and the burst; so the
/// second shows the first's births as their ages alone carry them on. A third frame at the same time,
/// without births, as a paused world's are, draws the same.
fn particles_match_their_reference() {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 96;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let at = |x: f32, y: f32| Mat4::from_translation(Vec3::new(x, y, 0.0));
    let mut squares = Draws::default();
    let back =
        Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0)) * Mat4::from_scale(Vec3::splat(8.0));
    squares.push(square, flat, back, Vec4::new(0.08, 0.08, 0.1, 1.0));
    let front = Mat4::from_translation(Vec3::new(0.0, -0.9, 0.5))
        * Mat4::from_scale(Vec3::new(0.6, 0.3, 1.0));
    squares.push(square, flat, front, Vec4::new(0.3, 0.3, 0.32, 1.0));
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &squares);
    let disc = Emission {
        shape: ParticleShape::Point,
        speed: [0.0; 2],
        lifetime: [1.0; 2],
        spin: [0.0; 2],
        acceleration: Vec3::ZERO,
        drag: 0.0,
        size: [0.1; 2],
        color: [Vec4::ONE; 2],
        texture: None,
        cells: [1, 1],
        additive: false,
    };
    let fountain = Emission {
        shape: ParticleShape::Cone {
            angle: 0.35,
            radius: 0.05,
        },
        speed: [2.6, 3.2],
        lifetime: [1.0, 1.2],
        acceleration: Vec3::new(0.0, -6.0, 0.0),
        drag: 0.2,
        size: [0.12, 0.06],
        color: [Vec4::new(0.4, 0.7, 1.0, 0.9), Vec4::new(0.2, 0.4, 1.0, 0.0)],
        ..disc
    };
    let fire = Emission {
        shape: ParticleShape::Sphere { radius: 0.15 },
        speed: [0.1, 0.4],
        lifetime: [0.5, 0.9],
        acceleration: Vec3::new(0.0, 3.0, 0.0),
        drag: 1.5,
        size: [0.35, 0.1],
        color: [
            Vec4::new(4.0, 1.6, 0.4, 0.8),
            Vec4::new(1.5, 0.2, 0.05, 0.0),
        ],
        additive: true,
        ..disc
    };
    let sparks = Emission {
        speed: [1.5, 2.5],
        lifetime: [0.6, 1.0],
        spin: [-6.0, 6.0],
        acceleration: Vec3::new(0.0, -3.0, 0.0),
        color: [Vec4::new(3.0, 2.0, 0.6, 1.0), Vec4::new(2.0, 0.6, 0.1, 0.0)],
        texture: Some(Texture::WHITE),
        cells: [2, 1],
        additive: true,
        ..disc
    };
    // Each emitter's particles at `now`, of `births`, `bursts` and the most it has alive.
    let drawn =
        |id: u64, now: f64, most: u32, births: Births, bursts: Bursts, emission| Particles {
            id,
            fresh: false,
            most,
            now,
            tick: 1.0 / 64.0,
            births,
            bursts,
            emission,
        };
    // `count` births from `first`, every `every` ticks from `time`, each where the emitter was
    // between `from`, as a frame at `then` drew it, and `to`, as a frame at `now` does.
    let births = |first, count, (time, every), (from, to), (then, now): (f64, f64)| Births {
        first,
        count,
        time,
        every,
        from,
        to,
        along: ((time - then) / (now - then)) as f32,
        step: (every / (now - then)) as f32,
    };
    let (fountain_at, moved, fire_at, sparks_at) =
        (at(-1.6, -1.2), at(-1.3, -1.2), at(0.0, -1.0), at(1.6, 0.3));
    let burst = Bursts {
        first: 0,
        count: 1,
        size: 48,
        skip: 0,
        time: 40.0,
        at: sparks_at,
    };
    // The first frame, at 32 ticks; the second, at 48; and the third, at 48 again, without births.
    let frames = [
        (
            32.0,
            births(0, 129, (0.0, 0.25), (fountain_at, fountain_at), (0.0, 32.0)),
            births(0, 161, (0.0, 0.2), (fire_at, fire_at), (0.0, 32.0)),
            Bursts::none(sparks_at),
        ),
        (
            48.0,
            births(129, 64, (32.25, 0.25), (fountain_at, moved), (32.0, 48.0)),
            births(161, 80, (32.2, 0.2), (fire_at, fire_at), (32.0, 48.0)),
            burst,
        ),
        (
            48.0,
            Births::none(moved),
            Births::none(fire_at),
            Bursts::none(sparks_at),
        ),
    ];
    let frame = |(now, spray, flame, burst): (f64, Births, Births, Bursts)| {
        let mut draws = Draws::default();
        draws.push(square, flat, back, Vec4::new(0.08, 0.08, 0.1, 1.0));
        draws.push(square, flat, front, Vec4::new(0.3, 0.3, 0.32, 1.0));
        draws.push_particles(drawn(1, now, 308, spray, Bursts::none(moved), fountain));
        draws.push_particles(drawn(2, now, 288, flame, Bursts::none(fire_at), fire));
        draws.push_particles(drawn(3, now, 48, Births::none(sparks_at), burst, sparks));
        draws
    };
    for drawn in &frames[..2] {
        let draws = frame(*drawn);
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    }
    let born = frame_pixels(&mut renderer);
    let draws = frame(frames[2]);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    assert!(
        frame_pixels(&mut renderer) == born,
        "a frame at the same time without births draws the same"
    );
    check(renderer, "particles", WIDTH, HEIGHT);
}

/// Draws a run of sprites across the top half of an offscreen image, then a run of models' squares
/// across the bottom half, each 4 pixels across, abutting in rows, and compares it with its
/// reference image. Each run is one instanced draw, whose instances alternate in a checkerboard
/// between two textures, the first sampled at its nearest texels and the second filtered linearly.
/// An instance is four of the blocks of 2 by 2 pixels that GPUs shade together, half of what a
/// subgroup of 32 holds, so a run's subgroups mix instances, with both textures and both samplers:
/// a GPU that sampled one instance's texture or sampler for a whole subgroup, as one may where the
/// sample isn't decorated as non-uniform, changes thousands of pixels. Against the reference,
/// shaders that index without `NonUniformResourceIndex` make 3,844 differ on AMD's driver.
fn mixed_textures_match_their_reference() {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 64;
    /// A sprite's and a square's side, in pixels.
    const SIDE: u32 = 4;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // A unit a pixel, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // 2 by 2 texels, sRGB-encoded: red and yellow over green and blue; and a checkerboard of light
    // and dark, which a linear filter blends between its texels.
    let quadrants = [
        [220, 50, 40, 255],
        [240, 200, 40, 255],
        [40, 170, 70, 255],
        [40, 80, 220, 255],
    ];
    let quadrants = (stores.textures)
        .add("quadrants", [2, 2], quadrants.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let (light, dark) = ([240, 240, 240, 255], [20, 20, 20, 255]);
    let checker = [light, dark, dark, light];
    let checker = (stores.textures)
        .add("checker", [2, 2], checker.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let sprites = [
        Sprite {
            texture: Some(quadrants),
            sampler: Sampler::NearestClamp,
            ..Sprite::default()
        },
        Sprite {
            texture: Some(checker),
            sampler: Sampler::LinearClamp,
            ..Sprite::default()
        },
    ];
    let square = model_square(&mut stores.meshes);
    let nearest = Material {
        color_texture: Sampled {
            texture: quadrants,
            nearest: true,
            wrap: Wrap::Clamp,
        },
        ..Material::default()
    };
    let linear = Material {
        color_texture: Sampled {
            wrap: Wrap::Clamp,
            ..Sampled::of(checker)
        },
        ..Material::default()
    };
    let squares = [nearest, linear].map(|material| {
        let material = stores.materials.add(&material);
        MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders)
    });
    let (columns, rows) = (WIDTH / SIDE, HEIGHT / 2 / SIDE);
    // At `column` and `row` of the half whose top edge is at `top`, from the half's top left.
    let place = |column: u32, row: u32, top: f32| {
        let side = SIDE as f32;
        let x = (column as f32 + 0.5) * side - WIDTH as f32 / 2.0;
        let y = top - (row as f32 + 0.5) * side;
        Mat4::from_translation(Vec3::new(x, y, 0.0)) * Mat4::from_scale(Vec3::new(side, side, 1.0))
    };
    let mut draws = Draws::default();
    for row in 0..rows {
        for column in 0..columns {
            let sprite = sprites[((column + row) % 2) as usize];
            draws.push_sprite(place(column, row, HEIGHT as f32 / 2.0), sprite);
        }
    }
    for row in 0..rows {
        for column in 0..columns {
            let square = &squares[((column + row) % 2) as usize];
            draws.push_draw(place(column, row, 0.0), square, 0);
        }
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    let instances = (columns * rows) as usize;
    let drawn = DrawCounts {
        draws: 2,
        instances: 2 * instances,
        triangles: 4 * instances,
    };
    let viewport = filling([WIDTH, HEIGHT]);
    let drew = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    assert_eq!(drew, drawn, "each run is one instanced draw");
    check(renderer, "mixed_textures", WIDTH, HEIGHT);
}

/// Draws a scene of IDs into a view off the frame's corner in six frames, each draw's ID its kind
/// in its high half and its frame's number in its low, so that an ID from another frame shows; two
/// sprites that overlap trade places each frame, the later on top. Frames 1, 2 and 4 pick the same
/// pixels, and each pick is answered as the frame two after the one that asked it begins, with
/// what drew on top there in its own frame: the later sprite where they overlap, and the other
/// alone; a game's mesh; the mesh through a sprite's wholly transparent texel, and the sprite
/// through its opaque one; a half transparent sprite over the mesh; a model; a circle in its middle,
/// and the clear at its quad's corner, outside it; a line of its own ID; 0 for a layer's sprite over
/// the mesh, beside the line, and for the clear, where a sprite of the odd frames alone drew in the
/// frame before. Picks outside the view are answered 0 in the frame that asks them. Frame 3 picks
/// nothing, so frame 5 answers nothing.
fn picks_answer_what_their_frame_drew() {
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 64;
    // What draws, each its ID's high half; and the pair's top, whichever is later.
    const MESH: u64 = 1;
    const MODEL: u64 = 2;
    const FIRST: u64 = 3;
    const SECOND: u64 = 4;
    const HALVED: u64 = 5;
    const BLENDED: u64 = 6;
    const TOP: u64 = 7;
    const ODD: u64 = 8;
    const CIRCLE: u64 = 9;
    const LINE: u64 = 10;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // A unit a pixel, around the origin, of the view's 64 by 48 pixels.
    let viewport = Viewport {
        origin: [16, 8],
        size: [64, 48],
    };
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 48.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // Over the view's pixels from `[x0, y0]` to `[x1, y1]`, from its top left.
    let over = |[x0, y0]: [f32; 2], [x1, y1]: [f32; 2]| {
        let center = Vec3::new((x0 + x1) / 2.0 - 32.0, 24.0 - (y0 + y1) / 2.0, 0.0);
        Mat4::from_translation(center) * Mat4::from_scale(Vec3::new(x1 - x0, y1 - y0, 1.0))
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let mesh = MeshDraw::new(
        square(&mut stores.meshes),
        flat,
        Vec4::new(0.2, 0.3, 0.8, 1.0),
    );
    let model_square = model_square(&mut stores.meshes);
    let material = stores.materials.add(&Material::default());
    let model = MeshDraw::with_material(
        model_square,
        material,
        &stores.materials,
        &mut stores.shaders,
    );
    // Opaque on the left, wholly transparent on the right.
    let half = [[200, 40, 40, 255], [0, 0, 0, 0]];
    let half = (stores.textures)
        .add("half", [2, 1], half.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let halved = Sprite {
        texture: Some(half),
        sampler: Sampler::NearestClamp,
        ..Sprite::default()
    };
    let tinted = |color: [f32; 4]| Sprite {
        color: Vec4::from(color),
        ..Sprite::default()
    };
    let id = |kind: u64, frame: u64| kind << 32 | frame;
    let scene = |frame: u64, shaders: &mut Shaders| {
        let mut draws = Draws::default();
        let circle = Circle {
            color: Vec4::new(0.8, 0.8, 0.2, 1.0),
            ..Circle::default()
        };
        let circle = (over([2.0, 28.0], [18.0, 44.0]), circle, id(CIRCLE, frame));
        draws.extend_circles(shaders, [circle].into_iter());
        // In view pixels from (42, 30) to (62, 30), 3 wide.
        let line = Line {
            from: Vec3::new(10.0, -6.0, 0.0),
            to: Vec3::new(30.0, -6.0, 0.0),
            color: Vec4::new(0.9, 0.3, 0.9, 1.0),
            width: 3.0,
        };
        draws.push_line(shaders, Mat4::IDENTITY, line, id(LINE, frame));
        draws.push_draw(over([20.0, 2.0], [40.0, 22.0]), &mesh, id(MESH, frame));
        draws.push_draw(over([44.0, 2.0], [56.0, 14.0]), &model, id(MODEL, frame));
        let first = (over([2.0, 2.0], [12.0, 12.0]), tinted([0.9, 0.4, 0.1, 1.0]));
        let second = (over([6.0, 6.0], [16.0, 16.0]), tinted([0.1, 0.8, 0.3, 1.0]));
        let pair = [(first, FIRST), (second, SECOND)];
        let pair = if frame.is_multiple_of(2) {
            [pair[1], pair[0]]
        } else {
            pair
        };
        let pair = pair.map(|((at, sprite), kind)| (at, sprite, id(kind, frame)));
        draws.extend_sprites(pair.into_iter());
        let halved = (over([24.0, 4.0], [32.0, 12.0]), halved, id(HALVED, frame));
        let blended = tinted([1.0, 1.0, 1.0, 0.5]);
        let blended = (
            over([34.0, 14.0], [38.0, 20.0]),
            blended,
            id(BLENDED, frame),
        );
        draws.extend_sprites([halved, blended].into_iter());
        // A layer's, of no entity.
        draws.push_sprite(over([22.0, 14.0], [28.0, 20.0]), tinted([1.0; 4]));
        if !frame.is_multiple_of(2) {
            let odd = tinted([0.6, 0.6, 0.2, 1.0]);
            let odd = (over([56.0, 36.0], [64.0, 44.0]), odd, id(ODD, frame));
            draws.extend_sprites([odd].into_iter());
        }
        draws
    };
    // The pixels each frame that picks picks, and what draws on top there, or 0.
    let pixels = [
        ([8, 8], TOP),
        ([3, 3], FIRST),
        ([22, 4], MESH),
        ([30, 8], MESH),
        ([25, 8], HALVED),
        ([36, 16], BLENDED),
        ([24, 16], 0),
        ([50, 8], MODEL),
        ([60, 40], ODD),
        ([10, 36], CIRCLE),
        ([3, 29], 0),
        ([50, 30], LINE),
        ([50, 33], 0),
        ([64, 10], 0),
        ([10, 48], 0),
    ];
    let tag = |frame: u64, i: usize| frame * 100 + i as u64;
    // The answers to the picks of frame `frame` inside the view, or outside it.
    let answers = |frame: u64, inside: bool| -> Vec<(u64, u64)> {
        let top = if frame.is_multiple_of(2) {
            FIRST
        } else {
            SECOND
        };
        let seen = |&(_, &([x, y], _)): &(usize, &([u32; 2], u64))| (x < 64 && y < 48) == inside;
        (pixels.iter().enumerate().filter(seen))
            .map(|(i, &(_, kind))| match kind {
                0 => (tag(frame, i), 0),
                ODD if frame.is_multiple_of(2) => (tag(frame, i), 0),
                TOP => (tag(frame, i), id(top, frame)),
                kind => (tag(frame, i), id(kind, frame)),
            })
            .collect()
    };
    // Everything drawable first.
    let first = scene(0, &mut stores.shaders);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &first);
    let mut answered = Vec::new();
    for frame in 1..=6 {
        let draws = scene(frame, &mut stores.shaders);
        let picks: Vec<Pick> = if [1, 2, 4].contains(&frame) {
            let pick = |(i, &(pixel, _)): (usize, &([u32; 2], u64))| Pick {
                tag: tag(frame, i),
                pixel,
            };
            pixels.iter().enumerate().map(pick).collect()
        } else {
            Vec::new()
        };
        let data = RenderData {
            picks: &picks,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        answered.push(renderer.picked().to_vec());
    }
    let expected = [
        answers(1, false),
        answers(2, false),
        answers(1, true),
        [answers(2, true), answers(4, false)].concat(),
        Vec::new(),
        answers(4, true),
    ];
    for (frame, (answered, expected)) in (1..).zip(answered.iter().zip(&expected)) {
        assert_eq!(answered, expected, "frame {frame}'s answers");
    }
    shut_down(renderer);
}

/// Six frames of a square that fills the view, its ID its frame's number. Frame 1 picks inside
/// the view; frame 2 inside and outside it, reading no picks, nor does frame 3, which still answers
/// frame 1's; frame 4 reads frame 2's, answering the one outside at once, and frame 6 answers the
/// other with what frame 4 drew.
fn picks_wait_through_frames_that_read_none() {
    let mut renderer = renderer(8, 8);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = MeshDraw::new(square(&mut stores.meshes), flat, Vec4::ONE);
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let pick = |tag, pixel| Pick { tag, pixel };
    let frames = [
        (vec![pick(1, [1, 1])], true, vec![]),
        (vec![pick(2, [3, 3]), pick(3, [8, 1])], false, vec![]),
        (vec![], false, vec![(1, 1)]),
        (vec![], true, vec![(3, 0)]),
        (vec![], true, vec![]),
        (vec![], true, vec![(2, 4)]),
    ];
    // The square drawable first.
    let mut first = Draws::default();
    first.push_draw(Mat4::from_scale(Vec3::splat(2.0)), &square, 0);
    settle(
        &mut renderer,
        &mut stores,
        filling([8, 8]),
        viewpoint,
        &first,
    );
    for (frame, (picks, read_picks, expected)) in (1..).zip(frames) {
        let mut draws = Draws::default();
        draws.push_draw(Mat4::from_scale(Vec3::splat(2.0)), &square, frame);
        let data = RenderData {
            picks: &picks,
            read_picks,
            ..stores.data(filling([8, 8]), viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        assert_eq!(renderer.picked(), expected, "frame {frame}'s answers");
    }
    shut_down(renderer);
}

/// Draws a square that fills the view and the engine's grid over it, facing it orthographically, a
/// pixel a unit, its X axis across the view's middle at the square's depth, where the grid draws
/// over what's at its depth: picks along the axis, on the two rows it may take, answer the square,
/// as the grid, which is for seeing by, leaves the IDs of what's under it.
fn picks_see_through_the_grid() {
    const SIZE: u32 = 32;
    const SQUARE: u64 = 7;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    let square = MeshDraw::new(square(&mut stores.meshes), flat, blue);
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: SIZE as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let over_view = Mat4::from_cols(
        Vec4::new(2.0 * SIZE as f32, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * SIZE as f32, 0.0, 0.0),
        Vec4::Z,
        Vec4::W,
    );
    let grid = Grid {
        first: Vec3::new(0.8, 0.1, 0.1),
        first_center: 0.0,
        second: Vec3::new(0.1, 0.7, 0.1),
        second_center: 0.0,
        eye: Vec3::NEG_Z,
        coarsest: 3.0,
        normal: Vec3::Z,
        lines: GridLines::Crisp,
    };
    let mut draws = Draws::default();
    draws.push_draw(Mat4::from_scale(Vec3::splat(SIZE as f32)), &square, SQUARE);
    draws.push_grid(&mut stores.meshes, &mut stores.shaders, over_view, grid);
    let viewport = filling([SIZE, SIZE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let rows = [SIZE / 2 - 1, SIZE / 2];
    let picks: Vec<Pick> = (rows.iter())
        .flat_map(|&y| (0..SIZE).map(move |x| [x, y]))
        .enumerate()
        .map(|(tag, pixel)| Pick {
            tag: tag as u64,
            pixel,
        })
        .collect();
    let data = RenderData {
        picks: &picks,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    let frame = frame_pixels(&mut renderer);
    // The axis shows over the square on one of its rows, away from the lines ten units apart.
    let plain = pixel_at(&frame, SIZE, [8, 8]);
    assert!(
        (rows.iter()).any(|&y| !close(pixel_at(&frame, SIZE, [8, y]), plain)),
        "the grid's X axis draws over the square"
    );
    // Answered once the frame that drew them is done, frames in flight later.
    let mut answered = renderer.picked().to_vec();
    for _ in 0..3 {
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        answered.extend_from_slice(renderer.picked());
    }
    let expected: Vec<_> = (0..picks.len() as u64).map(|tag| (tag, SQUARE)).collect();
    assert_eq!(answered, expected, "the picks along the grid's X axis");
    shut_down(renderer);
}

/// What `level.slang` takes with each draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct Level {
    texture: Texture,
    sampler: u32,
    level: f32,
}

/// Draws every mip level of two textures, uploaded by `path`, side by side into an offscreen image,
/// each at its nearest texels over a square of 64 by 64 pixels, and compares it with its reference
/// image. The first is 64 by 64 texels, alpha-weighted, in quadrants checkered in squares 1, 2, 4
/// and 8 texels across, so that each of its first five levels, made by averaging the level before,
/// evens out one more quadrant; one quadrant's squares are green and clear, which its levels blend
/// to half-transparent green rather than taking in the clear squares' color. The second is 5 by 3
/// texels, averaged, of odd sides, whose levels take three texels' parts each. Validation can't see
/// a level left unfilled, which the bindless set samples unchecked, nor one filled from the wrong
/// bytes, which both show here.
fn mipmaps_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 64;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // A unit a pixel, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let leveled = (stores.shaders)
        .add::<[f32; 5], Level>("level", &shader("level.slang"), Blend::Alpha)
        .expect("a shader");
    let corners = [
        [-32.0, -32.0, 0.0, 0.0, 1.0],
        [32.0, -32.0, 0.0, 1.0, 1.0],
        [32.0, 32.0, 0.0, 1.0, 0.0],
        [-32.0, 32.0, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    // Checkered in squares 1 texel across at the top left, 2 at the top right, 4 at the bottom
    // left and 8 at the bottom right, each of two colors, sRGB-encoded.
    let checkers: Vec<u8> = (0..64 * 64)
        .flat_map(|i| {
            let (x, y) = (i % 64, i / 64);
            let (cell, colors) = match (x < 32, y < 32) {
                (true, true) => (1, [[255, 255, 255, 255], [0, 0, 0, 255]]),
                (false, true) => (2, [[230, 40, 40, 255], [40, 60, 230, 255]]),
                (true, false) => (4, [[40, 200, 60, 255], [255, 0, 255, 0]]),
                (false, false) => (8, [[250, 210, 40, 255], [40, 40, 40, 255]]),
            };
            colors[(x / cell + y / cell) % 2]
        })
        .collect();
    let checkers = (stores.textures)
        .add("checkers", [64, 64], &checkers, Mipmaps::AlphaWeighted)
        .expect("a texture");
    // A color for each texel.
    let odd: Vec<u8> = (0..15u32)
        .flat_map(|i| [i * 17, 255 - i * 13, i * 71 % 256, 255].map(|c| c as u8))
        .collect();
    let odd = (stores.textures)
        .add("odd", [5, 3], &odd, Mipmaps::Averaged)
        .expect("a texture");
    let mut draws = Draws::default();
    let levels = (0..7)
        .map(|level| (checkers, level))
        .chain((0..3).map(|level| (odd, level)));
    for (i, (texture, level)) in levels.enumerate() {
        let x = i as f32 * 64.0 + 32.0 - WIDTH as f32 / 2.0;
        let level = Level {
            texture,
            sampler: Sampler::NearestClamp.index(),
            level: level as f32,
        };
        draws.push(
            square,
            leveled,
            Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            level,
        );
    }
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    check(renderer, "mipmaps", WIDTH, HEIGHT);
}

/// Draws a view where the UI paints a `ViewCallback`, among the UI's shapes rather than under them
/// all, and compares the frame with its reference image: a red rect under the view, which shows
/// where the callback's clip rect cuts the view's edges, and a green one over it. The view is a
/// blue square that fills it, under a triangle in red, green and blue, and a yellow square of ID 7
/// across the clip's right edge, outlined, its outline cut there too. The callback is off
/// `data.viewport`'s origin, the frame's corner, where the frame shows the view without one, so
/// that the view shown there too, unclipped, or over the green rect, changes thousands of pixels.
/// A frame whose view has no area draws the UI alone.
fn a_view_in_the_ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 4 units from the view's bottom to its top, which is 160 pixels: 40 pixels a unit.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let colored = (stores.shaders)
        .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
    let vertices = [
        corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
        corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
        corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let triangle = (stores.meshes)
        .add("triangle", &vertices, &[0, 1, 2])
        .expect("a mesh");
    let mut draws = Draws::default();
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    draws.push(square, flat, Mat4::from_scale(Vec3::splat(10.0)), blue);
    draws.push(triangle, colored, Mat4::from_scale(Vec3::splat(3.0)), ());
    // From (220, 60) to (260, 100) in the frame, across the clip's right edge.
    let yellow = Sprite {
        color: Vec4::new(0.9, 0.8, 0.2, 1.0),
        ..Sprite::default()
    };
    let across = Mat4::from_translation(Vec3::new(2.0, 0.5, 0.0));
    draws.extend_sprites([(across, yellow, 7)].into_iter());
    let outline = Outline {
        ids: &[7],
        color: Vec4::new(1.0, 0.35, 0.02, 1.0),
        width: 3.0,
    };

    let egui = egui::Context::default();
    let rect = |[x0, y0]: [f32; 2], [x1, y1]: [f32; 2]| {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    };
    let input = egui::RawInput {
        screen_rect: Some(rect([0.0, 0.0], [WIDTH as f32, HEIGHT as f32])),
        ..Default::default()
    };
    let mut output = egui.run_ui(input, |ui| {
        let painter = ui.painter();
        let red = egui::Color32::from_rgb(200, 60, 60);
        painter.rect_filled(rect([8.0, 8.0], [208.0, 128.0]), 0.0, red);
        // 240 by 160 points, a pixel each, clipped by 10 points top and bottom and 40 on the right.
        let clipped = painter.with_clip_rect(rect([40.0, 30.0], [240.0, 170.0]));
        clipped.add(egui::PaintCallback {
            rect: rect([40.0, 20.0], [280.0, 180.0]),
            callback: Arc::new(ViewCallback),
        });
        let green = egui::Color32::from_rgb(60, 200, 60);
        painter.rect_filled(rect([120.0, 80.0], [180.0, 120.0]), 4.0, green);
    });
    let primitives = egui.tessellate(output.shapes, output.pixels_per_point);
    // Everything drawable first, without the UI, whose texture changes come once.
    settle(
        &mut renderer,
        &mut stores,
        filling([240, 160]),
        viewpoint,
        &draws,
    );
    for size in [[240, 160], [0, 0]] {
        let ui = Ui {
            primitives: &primitives,
            textures: &mut output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        };
        let data = RenderData {
            outline: Some(outline),
            ..stores.data(filling(size), viewpoint, &draws)
        };
        assert!(
            renderer.render(data, ui, &mut []).expect("a frame"),
            "an offscreen image can always be rendered into"
        );
        let pixels = (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back");
        if size == [0, 0] {
            // In the callback's clip, clear of the rects, the clear color, encoded; in the red
            // rect, red.
            let pixel = |x: u32, y: u32| {
                let at = ((y * WIDTH + x) * 4) as usize;
                [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
            };
            for (x, y, want) in [(220, 150, [39, 48, 63, 255]), (100, 50, [200, 60, 60, 255])] {
                let got = pixel(x, y);
                let close = got.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 1);
                assert!(close, "at ({x}, {y}), {got:?} rather than {want:?}");
            }
        } else {
            compare("view_in_ui", WIDTH, HEIGHT, &pixels);
        }
    }
    shut_down(renderer);
}

/// Squares of gray, above, and of orange, below, each brighter than the one to its left, from
/// 0.05 to 16 times as bright as white, mapped by each tonemapper in turn, from the top, compared
/// with their reference image: a row of the view for each. Gray at 0.5 and 2, past the 1 that an
/// 8-bit image clamps to, comes out as each formula gives it, but AgX's, which the reference keeps.
fn tonemappers_match_their_reference() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    const BRIGHTNESS: [f32; 8] = [0.05, 0.18, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // 2 units from the view's bottom to its top, and 8 across: a unit is 32 pixels both ways.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let mut draws = Draws::default();
    for (column, brightness) in (0..).zip(BRIGHTNESS) {
        let x = column as f32 - 3.5;
        for (y, color) in [(0.5, Vec3::ONE), (-0.5, Vec3::new(1.0, 0.35, 0.05))] {
            let at = Mat4::from_translation(Vec3::new(x, y, 0.0));
            draws.push(square, flat, at, (color * brightness).extend(1.0));
        }
    }
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // Gray at 0.5 and at 2, the middle of the third and fifth squares above.
    let grays = [[80, 16], [144, 16]];
    // Stephen Hill's fit of ACES's transforms, whose matrices keep gray gray.
    let aces = |v: f32| {
        (v * (v + 0.024_578_6) - 0.000_090_537) / (v * (0.983_729 * v + 0.432_951) + 0.238_081)
    };
    let mut rows = Vec::new();
    for tonemapper in [
        Tonemapper::None,
        Tonemapper::PbrNeutral,
        Tonemapper::Aces,
        Tonemapper::Agx,
    ] {
        let data = RenderData {
            tonemapper,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        // PBR Neutral takes 0.04 off, and rolls 1.96 off to 1 - 0.24² / (1.96 + 0.24 - 0.76).
        let expected = match tonemapper {
            Tonemapper::None => Some([0.5, 1.0]),
            Tonemapper::PbrNeutral => Some([0.46, 0.96]),
            Tonemapper::Aces => Some([aces(0.5), aces(2.0)]),
            Tonemapper::Agx => None,
        };
        for (pixel, gray) in grays.into_iter().zip(expected.into_iter().flatten()) {
            let gray = encoded(gray);
            let got = pixel_at(&frame, WIDTH, pixel);
            assert!(
                close(got, [gray, gray, gray, 255]),
                "{tonemapper:?} maps gray to {got:?} at {pixel:?}, not {gray}"
            );
        }
        rows.extend(frame);
    }
    shut_down(renderer);
    compare("tonemappers", WIDTH, HEIGHT * 4, &rows);
}

/// Squares of a game's shader past the most a half float holds: red, which the view's colors hold
/// as that most or as infinite, as the GPU rounds it; and red, and white, infinite, as a shader's
/// floats can be; and one of no number. Mapped by PBR Neutral, which rolls a color that bright off
/// to white, the first three show white, and the last as none, black, without antialiasing, with
/// TAA and with SMAA, frame after frame, rather than as no number, black, or cyan where red alone
/// is infinite; and no pixel but those of the square of no number and next to it, which blend with
/// its black, shows darker than the clear color, as one that blends with no number would: darker in
/// a channel, and brighter in none. A pixel beside the red is redder, as TAA's filter spreads a
/// little of its light, and darker in green and blue, as PBR Neutral takes more off a color whose
/// least channel is greater.
fn colors_past_a_half_float_show_as_its_brightest() {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 16;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // 2 units from the view's bottom to its top, and 8 across: a unit is 8 pixels both ways.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let mut draws = Draws::default();
    let colors = [
        Vec3::new(1.0e6, 0.0, 0.0),
        Vec3::new(f32::INFINITY, 0.0, 0.0),
        Vec3::INFINITY,
        Vec3::NAN,
    ];
    for (x, color) in [-3.0, -1.0, 1.0, 3.0].into_iter().zip(colors) {
        let at = Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        draws.push(square, flat, at, color.extend(1.0));
    }
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let shown = [[255; 4], [255; 4], [255; 4], [0, 0, 0, 255]];
    for antialiasing in [Antialiasing::None, Antialiasing::Taa, Antialiasing::Smaa] {
        for frame in 0..4 {
            let data = RenderData {
                tonemapper: Tonemapper::PbrNeutral,
                antialiasing,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(&mut renderer, data);
            let pixels = frame_pixels(&mut renderer);
            for (middle, color) in [[8, 8], [24, 8], [40, 8], [56, 8]].into_iter().zip(shown) {
                let got = pixel_at(&pixels, WIDTH, middle);
                assert!(
                    close(got, color),
                    "{antialiasing:?}, frame {frame}: {got:?} at {middle:?}, not {color:?}"
                );
            }
            // Left of the square of no number, from 52 to 60 across, and the pixel next to it,
            // against the clear color as the corner shows it, mapped.
            let clear = pixel_at(&pixels, WIDTH, [0, 0]);
            let dark = (0..HEIGHT)
                .flat_map(|y| (0..51).map(move |x| [x, y]))
                .map(|at| (at, pixel_at(&pixels, WIDTH, at)))
                .find(|(_, pixel)| {
                    let darker = |(&got, clear): (&u8, u8)| got < clear.saturating_sub(1);
                    let brighter = |(&got, clear): (&u8, u8)| got > clear.saturating_add(1);
                    pixel.iter().zip(clear).any(darker) && !pixel.iter().zip(clear).any(brighter)
                });
            assert!(
                dark.is_none(),
                "{antialiasing:?}, frame {frame}: {dark:?} is darker than the clear color, {clear:?}"
            );
        }
    }
    shut_down(renderer);
}
/// A square of 2 by 2 pixels 64 times as bright as white in the world, left, and the same on the
/// canvas, right, in a view of 256 by 128, whose bloom has three levels. Without antialiasing, the
/// world's spreads its light round it, the same each way, less the further, while the canvas's,
/// after the bloom, doesn't spread at all; and no light goes further than the levels reach: each
/// downsample into level k reaches 2^(k + 1) pixels further, as its 13 taps read 2 texels of the
/// level before past the texel's own, and each upsample from level i + 1, and the blend into the
/// view from level 1, 3.5 × 2^i pixels, as its tent's bilinear taps read texels within 2 of its
/// own of the level below: 52.5 pixels in all, past which the view is its clear color, as bloom
/// adds no light. Without bloom, nothing spreads; with antialiasing, the world's light spreads and
/// the canvas's doesn't alike. Compared, with bloom and without antialiasing, with its reference.
fn bloom_spreads_light_round_what_is_bright_and_no_further() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    // How far, in pixels, light can go from the square's edges, as above.
    const REACH: f32 = 52.5;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // A unit to a pixel, from the view's middle.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let bright = Vec4::new(64.0, 64.0, 64.0, 1.0);
    let mut draws = Draws::default();
    // Over pixels 63 and 64 each way.
    let at = Mat4::from_scale_rotation_translation(
        Vec3::new(2.0, 2.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(-64.0, 0.0, 0.0),
    );
    draws.push(square, flat, at, bright);
    // Over pixels 191 and 192 across and 63 and 64 down.
    let mut canvas = Draws::default();
    let on_canvas = Mat4::from_translation(Vec3::new(192.0, 64.0, 0.0))
        * Mat4::from_scale(Vec3::new(2.0, 2.0, 1.0));
    canvas.push_sprite(
        on_canvas,
        Sprite {
            color: bright,
            ..Sprite::default()
        },
    );
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let render = |renderer: &mut Renderer, stores: &mut Stores, bloom, antialiasing| {
        let mut pixels = Vec::new();
        // Antialiased, enough frames for the history to settle.
        for _ in 0..16 {
            let data = RenderData {
                bloom,
                antialiasing,
                canvas: &canvas,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(renderer, data);
            pixels = frame_pixels(renderer);
        }
        pixels
    };
    let background = [CLEAR[0], CLEAR[1], CLEAR[2]].map(encoded);
    let background = [background[0], background[1], background[2], 255];
    let red = |pixels: &[u8], at: [u32; 2]| pixel_at(pixels, WIDTH, at)[0];
    // The square's light, `d` pixels right of it, left, below and above, in red.
    let around = |pixels: &[u8], d: u32| {
        [
            red(pixels, [64 + d, 64]),
            red(pixels, [63 - d, 64]),
            red(pixels, [64, 64 + d]),
            red(pixels, [64, 63 - d]),
        ]
    };
    let unbloomed = render(&mut renderer, &mut stores, 0.0, Antialiasing::None);
    for d in [2, 8] {
        assert!(
            close(pixel_at(&unbloomed, WIDTH, [64 + d, 64]), background),
            "without bloom, the square's light spreads {d} pixels"
        );
    }
    let bloomed = render(&mut renderer, &mut stores, 0.25, Antialiasing::None);
    let mut nearer = u8::MAX;
    for d in [2, 4, 8, 16, 24] {
        let light = around(&bloomed, d);
        assert!(
            light.iter().all(|&l| l.abs_diff(light[0]) <= 1),
            "the square's light {d} pixels away isn't the same each way: {light:?}"
        );
        assert!(
            light[0] > background[0] && light[0] < nearer,
            "the square's light {d} pixels away is {}, not between the background's {} and \
             {nearer} nearer",
            light[0],
            background[0]
        );
        nearer = light[0];
    }
    // Whether pixel `at` overlaps the reach round the square, which spans pixels 63 and 64 each way.
    let reached = |at: u32| (at + 1) as f32 > 63.0 - REACH && (at as f32) < 65.0 + REACH;
    for y in 0..HEIGHT {
        for x in 0..WIDTH * 5 / 8 {
            let got = pixel_at(&bloomed, WIDTH, [x, y]);
            assert!(
                (reached(x) && reached(y)) || close(got, background),
                "the square's light reaches ({x}, {y}), past the levels' reach: {got:?}"
            );
        }
    }
    let canvas_light = |pixels: &[u8]| {
        for (x, y) in [(194, 64), (189, 64), (192, 67), (192, 60), (200, 64)] {
            let got = pixel_at(pixels, WIDTH, [x, y]);
            assert!(
                close(got, background),
                "the canvas's square blooms to ({x}, {y}): {got:?}"
            );
        }
        assert_eq!(red(pixels, [192, 64]), 255, "the canvas's square shows");
    };
    canvas_light(&bloomed);
    let antialiased = render(&mut renderer, &mut stores, 0.25, Antialiasing::Taa);
    canvas_light(&antialiased);
    let light = around(&antialiased, 8);
    assert!(
        light.iter().all(|&l| l > background[0]),
        "antialiased, the square's light doesn't spread: {light:?}"
    );
    shut_down(renderer);
    compare("bloom", WIDTH, HEIGHT, &bloomed);
}

/// A lone pixel as bright as a half float holds, 65,504, in the middle of a view of 128 by 128,
/// blooms far less than a square of 4 by 4 pixels there holding the same light, a 16th of it each,
/// as Karis's average weighs each box of the first downsample by 1 over 1 plus its luminance, so
/// that a pixel far brighter than those round it, a firefly, doesn't bloom into a blot: the light
/// round each, from 6 pixels out to 24, past the square, less the clear color's, the lone pixel's
/// under half the square's. Without the average, the two would spread their light alike.
fn a_lone_bright_pixel_blooms_less_than_a_square_of_its_light() {
    const SIZE: u32 = 128;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // A unit to a pixel, from the view's middle.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: SIZE as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let brightest = 65_504.0;
    // A square `side` pixels a side round `middle`, holding `brightest` in all.
    let lit = |side: f32, middle: Vec2| {
        let mut draws = Draws::default();
        let at = Mat4::from_scale_rotation_translation(
            Vec3::new(side, side, 1.0),
            Quat::IDENTITY,
            middle.extend(0.0),
        );
        let color = Vec3::splat(brightest / (side * side)).extend(1.0);
        draws.push(square, flat, at, color);
        draws
    };
    let viewport = filling([SIZE, SIZE]);
    let background = decoded(encoded(CLEAR[0]));
    let mut spread = |draws: &Draws| {
        settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
        let data = RenderData {
            bloom: 0.0005,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(&mut renderer, data);
        let pixels = frame_pixels(&mut renderer);
        let mut light = 0.0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let far = x.abs_diff(64).max(y.abs_diff(64));
                if (6..=24).contains(&far) {
                    light += decoded(pixel_at(&pixels, SIZE, [x, y])[0]) - background;
                }
            }
        }
        light
    };
    // Pixel (64, 64) alone, and pixels 62 to 65 each way.
    let lone = spread(&lit(1.0, Vec2::new(0.5, -0.5)));
    let patch = spread(&lit(4.0, Vec2::ZERO));
    shut_down(renderer);
    assert!(patch > 1.0, "the square's light hardly spreads: {patch}");
    assert!(
        lone < 0.5 * patch,
        "a lone pixel spreads {lone} of light, against {patch} from a square of its light"
    );
}

/// Squares of grays and colors, saturated and dark ones among them, graded each way in turn,
/// without a tonemapper, so that what's graded shows as the grade makes it, each channel within a
/// step of what `Grade::graded` makes of it on the CPU: saturation 0, which makes them grays of
/// their luminance, and 2, which keeps grays and takes some colors' channels to none; a slope of a
/// half; contrast of 1.5; ASC CDL's slope, offset and power, each a channel's; the showcase's
/// golden hour; the white balance at its coolest and greenest, and warmer; the CPU's own checks
/// (`grade.rs`) say those are what each should make. The identity, and a grade a hair from it,
/// leave them as they were. A square on the canvas isn't graded. The rows, graded each way, are
/// compared with their reference.
fn grades_transform_as_they_say() {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 8;
    const COLORS: [Vec3; 16] = [
        Vec3::new(0.18, 0.18, 0.18),
        Vec3::new(0.5, 0.5, 0.5),
        Vec3::new(0.8, 0.1, 0.05),
        Vec3::new(0.1, 0.6, 0.2),
        Vec3::new(0.05, 0.1, 0.7),
        Vec3::new(0.9, 0.4, 0.1),
        Vec3::new(0.02, 0.01, 0.03),
        Vec3::new(0.9, 0.9, 0.6),
        Vec3::new(0.3, 0.05, 0.6),
        Vec3::new(0.65, 0.55, 0.05),
        Vec3::new(0.02, 0.4, 0.45),
        Vec3::new(0.33, 0.27, 0.21),
        Vec3::new(0.7, 0.02, 0.3),
        Vec3::new(0.12, 0.12, 0.35),
        Vec3::new(0.45, 0.8, 0.6),
        Vec3::new(0.004, 0.06, 0.01),
    ];
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // A unit to a pixel, from the view's middle.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let mut draws = Draws::default();
    for (i, color) in COLORS.into_iter().enumerate() {
        let at = Mat4::from_scale_rotation_translation(
            Vec3::new(8.0, 8.0, 1.0),
            Quat::IDENTITY,
            Vec3::new(i as f32 * 8.0 + 4.0 - WIDTH as f32 / 2.0, 0.0, 0.0),
        );
        draws.push(square, flat, at, color.extend(1.0));
    }
    // Red, over the top left 4 by 4 of the first square, which leaves its middle.
    let red = Vec4::new(0.8, 0.1, 0.05, 1.0);
    let mut canvas = Draws::default();
    let corner = Mat4::from_translation(Vec3::new(2.0, 6.0, 0.0))
        * Mat4::from_scale(Vec3::new(4.0, 4.0, 1.0));
    canvas.push_sprite(
        corner,
        Sprite {
            color: red,
            ..Sprite::default()
        },
    );
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut graded = |grade: Grade, canvas: &Draws| {
        let data = RenderData {
            grade,
            canvas,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        frame_pixels(&mut renderer)
    };
    let none = Draws::default();
    // Each square's middle pixel.
    let middles = |pixels: &[u8]| -> Vec<[u8; 4]> {
        (0..COLORS.len() as u32)
            .map(|i| pixel_at(pixels, WIDTH, [i * 8 + 4, 4]))
            .collect()
    };
    // Each square shows what `grade` makes of its color on the CPU, each channel within a step.
    let shows = |pixels: &[u8], what: &str, grade: Grade| {
        for (got, color) in middles(pixels).into_iter().zip(COLORS) {
            let want = grade.graded(color).to_array().map(encoded);
            let near = (got.iter().zip(want)).all(|(&g, w)| g.abs_diff(w) <= 1);
            assert!(near, "{what} shows {color} as {got:?}, not {want:?}");
        }
    };
    let mut rows = Vec::new();
    let grades = [
        ("the identity", Grade::IDENTITY),
        (
            "saturation 0",
            Grade {
                saturation: 0.0,
                ..Grade::IDENTITY
            },
        ),
        (
            "saturation 2",
            Grade {
                saturation: 2.0,
                ..Grade::IDENTITY
            },
        ),
        (
            "a slope of a half",
            Grade {
                slope: Vec3::splat(0.5),
                ..Grade::IDENTITY
            },
        ),
        (
            "contrast 1.5",
            Grade {
                contrast: 1.5,
                ..Grade::IDENTITY
            },
        ),
        (
            "ASC CDL",
            Grade {
                slope: Vec3::new(0.8, 1.1, 0.9),
                offset: Vec3::new(0.05, 0.0, 0.02),
                power: Vec3::new(1.2, 0.9, 1.0),
                ..Grade::IDENTITY
            },
        ),
        (
            "a golden hour",
            Grade {
                temperature: 0.35,
                tint: 0.05,
                contrast: 1.15,
                saturation: 1.25,
                slope: Vec3::new(1.05, 1.0, 0.95),
                offset: Vec3::new(0.0, 0.0, 0.01),
                power: Vec3::ONE,
            },
        ),
        (
            "the coolest and greenest white balance",
            Grade {
                temperature: -1.0,
                tint: -1.0,
                ..Grade::IDENTITY
            },
        ),
        (
            "a warmer white balance",
            Grade {
                temperature: 0.5,
                ..Grade::IDENTITY
            },
        ),
    ];
    for (what, grade) in grades {
        let pixels = graded(grade, &none);
        shows(&pixels, what, grade);
        rows.push(pixels);
    }
    let ungraded = &rows[0];
    for (got, color) in middles(ungraded).into_iter().zip(COLORS) {
        let want = color.to_array().map(encoded);
        let near = (got.iter().zip(want)).all(|(&g, w)| g.abs_diff(w) <= 1);
        assert!(near, "ungraded, {color} shows as {got:?}, not {want:?}");
    }
    for got in middles(&rows[1]) {
        assert!(got[0] == got[1] && got[1] == got[2], "{got:?} isn't gray");
    }
    let [r, _, b, _] = middles(&rows[8])[0];
    assert!(
        r > b + 4,
        "a warmer white balance makes mid gray {r} red and {b} blue"
    );
    let nearly = graded(
        Grade {
            contrast: 1.0 + 1.0e-6,
            ..Grade::IDENTITY
        },
        &none,
    );
    let differs = (nearly.iter().zip(ungraded)).filter(|(a, b)| a.abs_diff(**b) > 1);
    assert_eq!(
        differs.count(),
        0,
        "a grade a hair from the identity changes the view"
    );
    let gray_under_canvas = graded(
        Grade {
            saturation: 0.0,
            ..Grade::IDENTITY
        },
        &canvas,
    );
    let want = red.truncate().to_array().map(encoded);
    let got = pixel_at(&gray_under_canvas, WIDTH, [1, 1]);
    let near = (got.iter().zip(want)).all(|(&g, w)| g.abs_diff(w) <= 1);
    assert!(
        near,
        "the canvas's red is graded to {got:?}, not left {want:?}"
    );
    let [r, g, b, _] = middles(&gray_under_canvas)[0];
    assert!(r == g && g == b, "the world under the canvas isn't graded");
    shut_down(renderer);
    compare("grades", WIDTH, HEIGHT * rows.len() as u32, &rows.concat());
}

/// A gradient of gray across a view of 256 by 64, a column of pixels a shade, from 0.2 to 0.21,
/// which PBR Neutral takes 0.04 off, about 2.3 steps of the view's 8 bits from end to end. Dithered,
/// as PBR Neutral's view is, each row changes from pixel to pixel many times over, rather than at
/// the 2 edges of the bands rounding leaves, as without a tonemapper, which doesn't dither;
/// and the mean of each 16 columns is the gradient's within a tenth of a step, as rounding's
/// isn't. Black and white stay exactly as they are, the noise kept from reaching past them.
fn dithering_keeps_a_gradients_mean_and_breaks_its_bands() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // A unit to a pixel, from the view's middle.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let shade = |x: u32| 0.2 + 0.01 * (x as f32 + 0.5) / WIDTH as f32;
    // What PBR Neutral makes of the gradient, or what's drawn without a tonemapper for the same.
    let gradient = |less: f32| {
        let mut draws = Draws::default();
        for x in 0..WIDTH {
            let at = Mat4::from_scale_rotation_translation(
                Vec3::new(1.0, HEIGHT as f32, 1.0),
                Quat::IDENTITY,
                Vec3::new(x as f32 + 0.5 - WIDTH as f32 / 2.0, 0.0, 0.0),
            );
            draws.push(square, flat, at, Vec3::splat(shade(x) - less).extend(1.0));
        }
        draws
    };
    let (drawn, mapped) = (gradient(0.0), gradient(0.04));
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &drawn);
    let data = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        ..stores.data(viewport, viewpoint, &drawn)
    };
    render_without_ui(&mut renderer, data);
    let dithered = frame_pixels(&mut renderer);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &mapped));
    let rounded = frame_pixels(&mut renderer);
    // Black, left, and as bright as a half float holds, right, which PBR Neutral rolls off to
    // within a millionth of white.
    let mut ends = Draws::default();
    for (x, gray) in [(-0.25, 0.0), (0.25, 65_504.0)] {
        let at = Mat4::from_scale_rotation_translation(
            Vec3::new(WIDTH as f32 / 2.0, HEIGHT as f32, 1.0),
            Quat::IDENTITY,
            Vec3::new(x * WIDTH as f32, 0.0, 0.0),
        );
        ends.push(square, flat, at, Vec3::splat(gray).extend(1.0));
    }
    let data = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        ..stores.data(viewport, viewpoint, &ends)
    };
    render_without_ui(&mut renderer, data);
    let ended = frame_pixels(&mut renderer);
    shut_down(renderer);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let want = if x < WIDTH / 2 { 0 } else { 255 };
            let got = pixel_at(&ended, WIDTH, [x, y]);
            assert_eq!(got, [want, want, want, 255], "dithered at ({x}, {y})");
        }
    }
    // How many times each row's gray changes from one pixel to the next.
    let changes = |pixels: &[u8]| -> Vec<usize> {
        let gray = |x, y| pixel_at(pixels, WIDTH, [x, y])[1];
        let row = |y| (1..WIDTH).filter(|&x| gray(x, y) != gray(x - 1, y)).count();
        (0..HEIGHT).map(row).collect()
    };
    // At its bands' 2 edges, where the GPU's rounding may waver between pixels.
    let banded = changes(&rounded).into_iter().max().expect("rows");
    assert!(banded <= 6, "rounded, a row changes {banded} times");
    let least = changes(&dithered).into_iter().min().expect("rows");
    assert!(least > 64, "dithered, a row changes only {least} times");
    // The gradient, in steps of the view's 8 bits, encoded as the view is.
    let exact = |x: u32| {
        let linear = shade(x) - 0.04;
        255.0 * (1.055 * math::libm::powf(linear, 1.0 / 2.4) - 0.055)
    };
    let mean_of = |pixels: &[u8], columns: std::ops::Range<u32>| {
        let sum: f32 = (columns.clone())
            .flat_map(|x| (0..HEIGHT).map(move |y| [x, y]))
            .map(|at| f32::from(pixel_at(pixels, WIDTH, at)[1]))
            .sum();
        sum / (columns.len() as u32 * HEIGHT) as f32
    };
    let mut rounding_strays = 0.0_f32;
    for start in (0..WIDTH).step_by(16) {
        let columns = start..start + 16;
        let want = columns.clone().map(exact).sum::<f32>() / 16.0;
        let got = mean_of(&dithered, columns.clone());
        assert!(
            (got - want).abs() < 0.1,
            "dithered, columns {columns:?} average {got}, not {want}"
        );
        rounding_strays = rounding_strays.max((mean_of(&rounded, columns) - want).abs());
    }
    assert!(
        rounding_strays > 0.2,
        "rounding keeps the mean within {rounding_strays}, which tells dithering nothing"
    );
}

/// A view of a uniform sky alone, whose exposure adapts, frame by frame a tenth of a second apart:
/// its first two frames, before a metering comes back, at the exposure given, within the bounds;
/// the third at what the first's metering called for, at once: the sky's luminance, in the middle of
/// one of the meter's bins, 3 stops on. The sky 8 stops darker, two more frames at that, whose
/// meterings were of the bright sky, then down at its speed, 2 stops a second, until it holds at
/// what the dark sky calls for; then back up at 4, faster. A change of compensation shows in the
/// frame it's made, a stop brighter; a change of percentiles, whose meter finds the same of a uniform
/// sky, takes the compensation back at once too; and turned off, the exposure is the one given.
/// Each frame's exposure is within a thousandth of a
/// stop of what that makes it, and its sky shows the sky's luminance times it, as lit shading would
/// multiply light by it.
fn exposure_adapts_frame_by_frame_from_a_bright_view_to_a_dark_one() {
    const SIZE: u32 = 32;
    const DT: f32 = 0.1;
    // The middles of two of the meter's bins, in eighths of a stop from 2^-12, 8 stops apart.
    const BRIGHT: f32 = 12.0625;
    const DARK: f32 = 4.0625;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [1.0; 3]);
    let lit = |stores: &mut Stores, log2: f32| {
        stores.sky = Some(Sky {
            texture: sky,
            luminance: math::libm::exp2f(log2),
        });
    };
    lit(&mut stores, BRIGHT);
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint::default();
    let draws = Draws::default();
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let adaptation = Adaptation {
        min: -6.0,
        max: 22.0,
        compensation: 0.0,
        speed_up: 4.0,
        speed_down: 2.0,
        low_percentile: 0.1,
        high_percentile: 0.9,
    };
    let given = gg_renderer::light_scale(10.0);
    let mut time = 0.0;
    // A frame's exposure value, checked against `expected`, and its sky against the luminance it
    // shows, its log2 `shown`, times its exposure.
    let mut frame = |stores: &mut Stores, adaptation, shown: f32, expected: f32| {
        time += DT;
        let data = RenderData {
            exposure: given,
            adaptation,
            time,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let exposure = renderer.exposure();
        let ev100 = -math::libm::log2f(1.2 * exposure);
        assert!(
            (ev100 - expected).abs() < 1e-3,
            "at {time}s, the exposure value is {ev100}, not {expected}"
        );
        let sky = (math::libm::exp2f(shown) * exposure).min(1.0);
        let pixel = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
        let expected = encoded(sky);
        assert!(
            pixel[..3]
                .iter()
                .all(|&channel| channel.abs_diff(expected) <= 1),
            "at {time}s, the sky shows {pixel:?}, not {expected}"
        );
    };
    let on = Some(adaptation);
    for _ in 0..2 {
        frame(&mut stores, on, BRIGHT, 10.0);
    }
    let bright = BRIGHT + 3.0;
    for _ in 0..3 {
        frame(&mut stores, on, BRIGHT, bright);
    }
    lit(&mut stores, DARK);
    for _ in 0..2 {
        frame(&mut stores, on, DARK, bright);
    }
    let dark = DARK + 3.0;
    let mut expected = bright;
    while expected > dark {
        expected = (expected - adaptation.speed_down * DT).max(dark);
        frame(&mut stores, on, DARK, expected);
    }
    frame(&mut stores, on, DARK, dark);
    lit(&mut stores, BRIGHT);
    for _ in 0..2 {
        frame(&mut stores, on, BRIGHT, dark);
    }
    while expected < bright {
        expected = (expected + adaptation.speed_up * DT).min(bright);
        frame(&mut stores, on, BRIGHT, expected);
    }
    let brighter = Some(Adaptation {
        compensation: 1.0,
        ..adaptation
    });
    for _ in 0..3 {
        frame(&mut stores, brighter, BRIGHT, bright - 1.0);
    }
    let narrower = Some(Adaptation {
        low_percentile: 0.3,
        ..adaptation
    });
    frame(&mut stores, narrower, BRIGHT, bright);
    frame(&mut stores, None, BRIGHT, 10.0);
    shut_down(renderer);
}

/// A view half a bright sky above the horizon and half a dark one below, each of luminances in the
/// middle of one of the meter's bins, 8 stops apart, whose exposure adapts as fast as can be: it
/// takes the mean of the two, between its tenth and ninetieth percentiles, each half 40% of the
/// pixels; the bright half alone between its sixtieth and its ninetieth; the dark alone below its
/// fortieth. A canvas of white over the whole view, which draws after the meter, changes nothing
/// of it. Antialiased, temporally or by SMAA, whose colors the meter reads in place of the view's
/// as drawn, it meters the halves alike, as without. Over a sky black above the horizon, the dark
/// half alone, as what's darker than the meter's bins isn't metered.
fn exposure_meters_between_its_percentiles() {
    const SIZE: u32 = 32;
    // The middles of two of the meter's bins, 8 stops apart.
    const BRIGHT: f32 = 10.0625;
    const DARK: f32 = 2.0625;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let [bright, dark] = [BRIGHT, DARK].map(math::libm::exp2f);
    // Fine enough that the horizon's texels blend within no pixel.
    let size = [512, 256];
    let halves = sky_texture(&mut stores.textures, size, |_, y| {
        [if y < size[1] / 2 { bright } else { dark }; 3]
    });
    let black = sky_texture(&mut stores.textures, size, |_, y| {
        [if y < size[1] / 2 { 0.0 } else { dark }; 3]
    });
    let shown = |stores: &mut Stores, texture| {
        stores.sky = Some(Sky {
            texture,
            luminance: 1.0,
        });
    };
    shown(&mut stores, black);
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint {
        projection: Projection::Perspective {
            fov: 1.5,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let draws = Draws::default();
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    shown(&mut stores, halves);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let instant = Adaptation {
        min: -6.0,
        max: 22.0,
        speed_up: 1000.0,
        speed_down: 1000.0,
        ..Adaptation::default()
    };
    let mut time = 0.0;
    let none = Draws::default();
    // The exposure value of the third frame of `adaptation`, with `canvas` over the view,
    // antialiased as `antialiasing` says, which the first's metering sets, and that frame's middle
    // pixel.
    let mut metered =
        |stores: &mut Stores, adaptation: Adaptation, canvas: &Draws, antialiasing| {
            for _ in 0..3 {
                time += 0.1;
                let data = RenderData {
                    adaptation: Some(adaptation),
                    time,
                    canvas,
                    antialiasing,
                    ..stores.data(viewport, viewpoint, &draws)
                };
                render_without_ui(&mut renderer, data);
            }
            let ev100 = -math::libm::log2f(1.2 * renderer.exposure());
            let middle = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
            (ev100, middle)
        };
    let cases = [
        (0.1, 0.9, (BRIGHT + DARK) / 2.0),
        (0.6, 0.9, BRIGHT),
        (0.0, 0.4, DARK),
    ];
    for (low_percentile, high_percentile, mean) in cases {
        let adaptation = Adaptation {
            low_percentile,
            high_percentile,
            ..instant
        };
        let (ev100, _) = metered(&mut stores, adaptation, &none, Antialiasing::None);
        assert!(
            (ev100 - (mean + 3.0)).abs() < 1e-3,
            "between {low_percentile} and {high_percentile}, {ev100}, not {}",
            mean + 3.0
        );
    }
    // A canvas of white over the whole view, which draws after the meter, meters nothing.
    let mut canvas = Draws::default();
    let whole = Mat4::from_scale_rotation_translation(
        Vec3::new(SIZE as f32, SIZE as f32, 1.0),
        Quat::IDENTITY,
        Vec3::new(SIZE as f32 / 2.0, SIZE as f32 / 2.0, 0.0),
    );
    canvas.push_sprite(whole, Sprite::default());
    let (ev100, pixel) = metered(&mut stores, instant, &canvas, Antialiasing::None);
    let mean = (BRIGHT + DARK) / 2.0;
    assert!(
        (ev100 - (mean + 3.0)).abs() < 1e-3,
        "under a canvas of white, {ev100}, not {}",
        mean + 3.0
    );
    assert_eq!(pixel, [255; 4], "the canvas shows over the view");
    let mut antialiased = [Antialiasing::Taa, Antialiasing::Smaa].map(|antialiasing| {
        let (ev100, _) = metered(&mut stores, instant, &none, antialiasing);
        assert!(
            (ev100 - (mean + 3.0)).abs() < 1e-3,
            "antialiased as {antialiasing:?}, {ev100}, not {}",
            mean + 3.0
        );
        ev100
    });
    antialiased.sort_by(f32::total_cmp);
    assert!(
        antialiased[1] - antialiased[0] < 1e-3,
        "temporally and by SMAA, {antialiased:?}"
    );
    shown(&mut stores, black);
    let (ev100, _) = metered(&mut stores, instant, &none, Antialiasing::None);
    assert!(
        (ev100 - (DARK + 3.0)).abs() < 1e-3,
        "under a black sky, {ev100}, not {}",
        DARK + 3.0
    );
    shut_down(renderer);
}

/// The game's own view of a bright sky and a tool's look of a dark one, as the tool API's look
/// round a dark room while the game's camera is outdoors, each exposure adapting as fast as can
/// be: each settles at what its own sky calls for; a frame of the game's own view while the look
/// stays, as a capture of it is, shows the bright sky at the game's own exposure; and what that
/// frame metered, back two frames later, leaves the look's exposure where it was.
fn a_looks_exposure_adapts_apart_from_the_games_own() {
    const SIZE: u32 = 32;
    // The middles of two of the meter's bins, 8 stops apart.
    const BRIGHT: f32 = 12.0625;
    const DARK: f32 = 4.0625;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [1.0; 3]);
    let lit = |stores: &mut Stores, log2: f32| {
        stores.sky = Some(Sky {
            texture: sky,
            luminance: math::libm::exp2f(log2),
        });
    };
    lit(&mut stores, BRIGHT);
    let viewport = filling([SIZE, SIZE]);
    let draws = Draws::default();
    settle(
        &mut renderer,
        &mut stores,
        viewport,
        Viewpoint::default(),
        &draws,
    );
    let instant = Adaptation {
        min: -6.0,
        max: 22.0,
        speed_up: 1000.0,
        speed_down: 1000.0,
        ..Adaptation::default()
    };
    let mut time = 0.0;
    // The exposure value of a frame of the game's own view of the bright sky, or of the look's of
    // the dark one, and its sky's pixel.
    let mut frame = |stores: &mut Stores, looked: bool| {
        time += 0.1;
        lit(stores, if looked { DARK } else { BRIGHT });
        let data = RenderData {
            exposure: gg_renderer::light_scale(10.0),
            adaptation: Some(instant),
            looked,
            time,
            ..stores.data(viewport, Viewpoint::default(), &draws)
        };
        render_without_ui(&mut renderer, data);
        let ev100 = -math::libm::log2f(1.2 * renderer.exposure());
        let pixel = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
        (ev100, pixel)
    };
    let near = |got: f32, want: f32| (got - want).abs() < 1e-3;
    for _ in 0..4 {
        frame(&mut stores, false);
    }
    let (own, shown) = frame(&mut stores, false);
    assert!(
        near(own, BRIGHT + 3.0),
        "the game's own view settles at {own}"
    );
    for _ in 0..4 {
        frame(&mut stores, true);
    }
    let (looked, _) = frame(&mut stores, true);
    assert!(near(looked, DARK + 3.0), "the look settles at {looked}");
    let (captured, pixel) = frame(&mut stores, false);
    assert!(
        near(captured, own),
        "a frame of the game's own view takes {captured}, not {own}"
    );
    assert!(
        close(pixel, shown),
        "it shows the sky as {pixel:?}, not {shown:?}"
    );
    for i in 0..3 {
        let (after, _) = frame(&mut stores, true);
        assert!(
            near(after, looked),
            "{i} frames on, the look takes {after}, not {looked}"
        );
    }
    shut_down(renderer);
}

/// A sky of fine checkers, one a hundredth as bright as the other, under temporal antialiasing,
/// whose clipping lets much of the history through where what's round a pixel differs that much:
/// settled at the exposure it
/// adapts to; then settled at a quarter of that, without adapting; then adapting again, whose third
/// frame takes the first metering at once, 4 times brighter. That frame shows what the view settled
/// at before, as near as one settled frame is to the one before it, as the resolve scales its
/// history by the change of exposure, rather than a blend of mostly the dimmer history, a ghost.
fn an_exposure_jump_under_antialiasing_leaves_no_ghost() {
    const SIZE: u32 = 32;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [256, 128], |x, y| {
        [if (x + y) % 2 == 0 { 1.0 } else { 0.01 }; 3]
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 4000.0,
    });
    let viewport = filling([SIZE, SIZE]);
    // About a texel a pixel.
    let viewpoint = Viewpoint {
        projection: Projection::Perspective {
            fov: 0.8,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let draws = Draws::default();
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut time = 0.0;
    let mut frame = |stores: &mut Stores, exposure: f32, adaptation: Option<Adaptation>| {
        time += 0.1;
        let data = RenderData {
            antialiasing: Antialiasing::Taa,
            exposure,
            adaptation,
            time,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        (frame_pixels(&mut renderer), renderer.exposure())
    };
    let adapting = Some(Adaptation::default());
    let given = gg_renderer::light_scale(12.0);
    let mut last = frame(&mut stores, given, adapting);
    let mut before = last.clone();
    for _ in 0..40 {
        before = last;
        last = frame(&mut stores, given, adapting);
    }
    let (settled, adapted) = last;
    let still = linear_distance(&before.0, &settled);
    for _ in 0..30 {
        frame(&mut stores, adapted / 4.0, None);
    }
    for _ in 0..2 {
        frame(&mut stores, adapted / 4.0, adapting);
    }
    let (jumped, exposure) = frame(&mut stores, adapted / 4.0, adapting);
    let ratio = exposure / adapted;
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "it jumped to {ratio} of what it settled at"
    );
    let ghost = linear_distance(&jumped, &settled);
    assert!(
        ghost < 2.0 * still + 0.002,
        "the frame of the jump is {ghost} from what it settled at, one settled frame {still} from \
         the one before"
    );
    shut_down(renderer);
}

/// A white floor, three spheres on it, white, red and blue, and an unlit square of orange over
/// them, under a sun and a sky, whose exposure adapts, against its reference of four frames side by
/// side: settled under a sun of 100,000 lux and a sky of 4,000 candela per square meter; the first
/// with both 8 stops darker, its lit pixels dark and its unlit square as bright as ever; a second
/// later, coming down at its speed, 4 stops a second; and settled. Everything lit 8 stops darker,
/// its meter's mean is 8 stops lower, so that it settles 8 stops lower, showing what it showed:
/// within a step of 8 bits of the first. And a frame drawn at the exposure it settled at, without
/// adapting, shows the same.
fn adapted_frames_match_their_reference() {
    const SIZE: u32 = 64;
    const DT: f32 = 0.1;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, y| {
        if y < 16 {
            [0.6, 0.8, 1.0]
        } else {
            [0.3, 0.25, 0.2]
        }
    });
    let at = |stores: &mut Stores, darker: f32| {
        let dimmed = math::libm::exp2f(-darker);
        stores.sky = Some(Sky {
            texture: sky,
            luminance: 4000.0 * dimmed,
        });
        stores.lights = vec![Light {
            color: Vec3::splat(100_000.0 * dimmed),
            ..sun(Vec3::new(1.0, -2.0, -1.0), None)
        }];
    };
    at(&mut stores, 0.0);
    let square = model_square(&mut stores.meshes);
    let ball = sphere(&mut stores.meshes, 0.5, 16, 32);
    let mut draws = Draws::default();
    let mut drawn = |stores: &mut Stores, mesh, color: Vec4, unlit, at: Mat4| {
        let material = stores.materials.add(&Material {
            color,
            workflow: metallic(0.0, 0.6),
            unlit,
            ..Material::default()
        });
        let draw = MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(at, &draw, 0);
    };
    drawn(
        &mut stores,
        square,
        Vec4::ONE,
        false,
        facing_up(8.0, Vec3::ZERO),
    );
    for (x, color) in [
        (-1.2, [0.9; 3]),
        (0.0, [0.8, 0.1, 0.1]),
        (1.2, [0.1, 0.2, 0.8]),
    ] {
        let color = Vec3::from(color).extend(1.0);
        let at = Mat4::from_translation(Vec3::new(x, 0.5, 0.0));
        drawn(&mut stores, ball, color, false, at);
    }
    let orange = Vec4::new(0.8, 0.4, 0.1, 1.0);
    let over = Mat4::from_scale_rotation_translation(
        Vec3::splat(0.6),
        Quat::IDENTITY,
        Vec3::new(0.0, 1.8, 0.5),
    );
    drawn(&mut stores, square, orange, true, over);
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.0, 5.0),
        rotation: Quat::from_rotation_x(-0.3),
        projection: Projection::Perspective {
            fov: 0.9,
            near: 0.1,
        },
    };
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let adaptation = Adaptation {
        speed_down: 4.0,
        ..Adaptation::default()
    };
    let mut time = 0.0;
    let mut frame = |stores: &mut Stores| {
        time += DT;
        let data = RenderData {
            tonemapper: Tonemapper::PbrNeutral,
            exposure: gg_renderer::light_scale(15.0),
            adaptation: Some(adaptation),
            time,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        (frame_pixels(&mut renderer), renderer.exposure())
    };
    let (bright, bright_exposure) = settled(&mut stores, &mut frame);
    at(&mut stores, 8.0);
    let (first, _) = frame(&mut stores);
    for _ in 1..10 {
        frame(&mut stores);
    }
    let (second, _) = frame(&mut stores);
    let (dark, dark_exposure) = settled(&mut stores, &mut frame);
    let stops = math::libm::log2f(dark_exposure / bright_exposure);
    assert!((stops - 8.0).abs() < 1e-3, "settled {stops} stops apart");
    let unlit = pixel_of(viewpoint, [SIZE, SIZE], Vec3::new(0.0, 1.8, 0.5));
    let shown = pixel_at(&bright, SIZE, unlit);
    for frame in [&first, &second, &dark] {
        let pixel = pixel_at(frame, SIZE, unlit);
        assert_eq!(
            pixel, shown,
            "the unlit square shows otherwise at another exposure"
        );
    }
    let floor = pixel_of(viewpoint, [SIZE, SIZE], Vec3::new(2.5, 0.0, 1.5));
    let [lit, dimmed] = [&bright, &first].map(|frame| decoded(pixel_at(frame, SIZE, floor)[1]));
    assert!(
        dimmed < lit / 8.0,
        "the floor shows {dimmed} 8 stops darker, against {lit}"
    );
    let apart = (bright.iter().zip(&dark))
        .filter(|(a, b)| a.abs_diff(**b) > 1)
        .count();
    assert_eq!(apart, 0, "settled lit 8 stops darker, it shows otherwise");
    let unadapted = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        exposure: dark_exposure,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, unadapted);
    assert!(
        frame_pixels(&mut renderer) == dark,
        "adapting shows otherwise than the exposure"
    );
    let frames = side_by_side(&[&bright, &first, &second, &dark], SIZE);
    shut_down(renderer);
    compare("adapted", SIZE * 4, SIZE, &frames);
}

/// What `frame` renders of `stores` once the exposure it gives, after its pixels, moves less than a
/// ten-thousandth of a stop over three frames, as an adapting exposure that has settled does, whose
/// meterings come back two frames later: within 100 frames.
fn settled(
    stores: &mut Stores,
    frame: &mut impl FnMut(&mut Stores) -> (Vec<u8>, f32),
) -> (Vec<u8>, f32) {
    let mut last = frame(stores).1;
    let mut still = 0;
    for _ in 0..100 {
        let next = frame(stores);
        still = if math::libm::log2f(next.1 / last).abs() < 1e-4 {
            still + 1
        } else {
            0
        };
        if still == 2 {
            return next;
        }
        last = next.1;
    }
    panic!("the exposure still moves after 100 frames: {last}");
}

/// What the depth of field's tests see: squares of flat colors at 2, 4 and 8 ahead of a camera
/// whose field of view is 20° from bottom to top, a 68 mm lens's on a full-frame camera's sensor,
/// over stripes 12 ahead; and where each of the squares' middles shows.
struct FocusScene {
    draws: Draws,
    viewpoint: Viewpoint,
    near: Vec3,
    focused: Vec3,
    far: Vec3,
    /// Another scene of the same view: a red square 0.1 across at 2 ahead, and beside it, just past
    /// the focus, a yellow one at `past`, whose circle at f/1.4 is 2.5 pixels, 20 pixels a side,
    /// within the tiles the red square's circle reaches, over white 200 ahead.
    beside: Draws,
    past: Vec3,
}

/// The depth of field's tests' scene, in `stores`: a red square 0.2 across at 2 ahead, its right
/// edge over a green one's left edge, the green 0.5 across at 4 ahead, its right edge over a blue
/// one's left edge, the blue 0.8 across at 8 ahead, and stripes of light and dark gray a tenth of a
/// unit wide 12 ahead, filling the view behind them.
fn focus_scene(stores: &mut Stores) -> FocusScene {
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let viewpoint = Viewpoint {
        projection: Projection::Perspective {
            fov: math::to_radians(20.0),
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let (near, focused, far) = (
        Vec3::new(-0.195, 0.03, -2.0),
        Vec3::new(0.0, 0.0, -4.0),
        Vec3::new(0.78, -0.1, -8.0),
    );
    let mut draws = Draws::default();
    let placed = |at: Vec3, side: f32| {
        Mat4::from_scale_rotation_translation(Vec3::new(side, side, 1.0), Quat::IDENTITY, at)
    };
    draws.push(
        square,
        flat,
        placed(near, 0.2),
        Vec4::new(0.9, 0.1, 0.1, 1.0),
    );
    draws.push(
        square,
        flat,
        placed(focused, 0.5),
        Vec4::new(0.1, 0.8, 0.2, 1.0),
    );
    draws.push(
        square,
        flat,
        placed(far, 0.8),
        Vec4::new(0.1, 0.2, 0.9, 1.0),
    );
    for stripe in -40..40 {
        let at = Vec3::new(0.05 + 0.1 * stripe as f32, 0.0, -12.0);
        let at =
            Mat4::from_scale_rotation_translation(Vec3::new(0.1, 5.0, 1.0), Quat::IDENTITY, at);
        let shade = if stripe % 2 == 0 { 0.6 } else { 0.05 };
        draws.push(square, flat, at, Vec4::new(shade, shade, shade, 1.0));
    }
    // 2.5 = 6.3 (1 - 4 / 6.63), of what's infinitely far's 6.3 pixels; 154 pixels a unit there.
    let past = Vec3::new(-0.208, 0.0, -6.63);
    let mut beside = Draws::default();
    let red = Vec4::new(0.9, 0.1, 0.1, 1.0);
    beside.push(square, flat, placed(Vec3::new(-0.137, 0.0, -2.0), 0.1), red);
    beside.push(
        square,
        flat,
        placed(past, 0.13),
        Vec4::new(1.0, 0.9, 0.0, 1.0),
    );
    beside.push(
        square,
        flat,
        placed(Vec3::new(0.0, 0.0, -200.0), 200.0),
        Vec4::ONE,
    );
    FocusScene {
        draws,
        viewpoint,
        near,
        focused,
        far,
        beside,
        past,
    }
}

/// [`focus_scene`] in focus at 4, through f/1.4, without antialiasing, in a view of 640 by 360, whose
/// circles of confusion's radius is at most 7.2 pixels: 6.3 pixels for what's infinitely far, 3.2 at
/// 8, 4.2 at 12, and past the most at 2. The green square, in focus, is as it is without depth of
/// field, every pixel within 2 of it, its edges too, the blue square's blur behind its right edge not
/// spreading over it; the red square, before the focus, spreads over what's round it, over the green
/// square's edge too, and fades in from its own edge; the blue square's right edge and the stripes
/// behind blur, the stripes' contrast falling, but the stripes, of a larger circle, don't spread
/// into the blue square past its own circle; nor does white far behind a square just past the
/// focus beside a red one before it, whose circle widens what's gathered there. Antialiased, the
/// square in focus's edges shimmer over the offsets' cycle no more than docs/DEFECTS.md says; by
/// SMAA, the square in focus stays as sharp and the red one spreads alike. A
/// focus that blurs nothing, at f/1000, and depth of
/// field under an orthographic projection, which has no lens, change no pixel; nor does what's on
/// the canvas, drawn after it, blur. With antialiasing, the green square stays as sharp. Compared,
/// without antialiasing, with its reference.
fn depth_of_field_blurs_what_is_off_its_focus() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    let size = [WIDTH, HEIGHT];
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = focus_scene(&mut stores);
    // A white square on the canvas, 16 pixels a side, 8 from the bottom left.
    let mut canvas = Draws::default();
    let placed = Mat4::from_translation(Vec3::new(16.0, 16.0, 0.0))
        * Mat4::from_scale(Vec3::new(16.0, 16.0, 1.0));
    canvas.push_sprite(placed, Sprite::default());
    let viewport = filling(size);
    let viewpoint = scene.viewpoint;
    settle(
        &mut renderer,
        &mut stores,
        viewport,
        viewpoint,
        &scene.draws,
    );
    let focus = DepthOfField {
        focus_distance: 4.0,
        f_number: 1.4,
    };
    // Each frame of the view, of one frame, or antialiased, of enough for the history to settle and
    // a cycle of the offsets after.
    let mut frames = |depth_of_field, viewpoint, antialiasing| {
        let count = if antialiasing == Antialiasing::Taa {
            24
        } else {
            1
        };
        let mut drawn = Vec::new();
        for _ in 0..count {
            let data = RenderData {
                depth_of_field,
                antialiasing,
                canvas: &canvas,
                ..stores.data(viewport, viewpoint, &scene.draws)
            };
            render_without_ui(&mut renderer, data);
            drawn.push(frame_pixels(&mut renderer));
        }
        drawn
    };
    let last = |mut drawn: Vec<Vec<u8>>| drawn.pop().expect("a frame");
    let sharp = last(frames(None, viewpoint, Antialiasing::None));
    let blurred = last(frames(Some(focus), viewpoint, Antialiasing::None));
    let still = DepthOfField {
        f_number: 1000.0,
        ..focus
    };
    assert!(
        last(frames(Some(still), viewpoint, Antialiasing::None)) == sharp,
        "a focus that blurs nothing changes the view"
    );
    let flat = Viewpoint {
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.0,
            far: 20.0,
        },
        ..viewpoint
    };
    assert!(
        last(frames(Some(focus), flat, Antialiasing::None))
            == last(frames(None, flat, Antialiasing::None)),
        "depth of field under an orthographic projection changes the view"
    );
    let antialiased_frames = frames(Some(focus), viewpoint, Antialiasing::Taa);
    let antialiased = antialiased_frames.last().expect("a frame").clone();
    let antialiased_sharp_frames = frames(None, viewpoint, Antialiasing::Taa);
    let antialiased_sharp = antialiased_sharp_frames.last().expect("a frame").clone();
    let smaa = last(frames(Some(focus), viewpoint, Antialiasing::Smaa));
    let smaa_sharp = last(frames(None, viewpoint, Antialiasing::Smaa));
    let data = RenderData {
        depth_of_field: Some(focus),
        ..stores.data(viewport, viewpoint, &scene.beside)
    };
    render_without_ui(&mut renderer, data);
    let beside = frame_pixels(&mut renderer);
    shut_down(renderer);
    // The yellow square just past the focus, 5 pixels in from its right edge, twice its own
    // circle, is as yellow as it is, its blue none, though the red square's circle widens what each
    // texel there gathers to 3.6 texels: the white behind, of a circle of 6.2 pixels, doesn't
    // spread over it past its own circle, where it would reach 5 pixels in.
    let [x, y] = pixel_of(viewpoint, size, scene.past + Vec3::new(0.065, 0.0, 0.0));
    let inside = pixel_at(&beside, WIDTH, [x - 5, y]);
    assert!(
        inside[2] <= 8,
        "the white behind the square just past the focus spreads 5 pixels into it: {inside:?}"
    );
    // Antialiased, across the square in focus's edges over the stripes and the blue square, each
    // pixel's channels move no more than 16 further over a cycle of the offsets, the last 8 frames,
    // than they do without depth of field: the edge's own pixels, whose depth the offsets move the
    // edge across, take the circle of what's behind them in some frames, as docs/DEFECTS.md says,
    // about 10 further, and no others shimmer.
    let [gx, gy] = pixel_of(viewpoint, size, scene.focused);
    let spread = |drawn: &[Vec<u8>], p: [u32; 2]| {
        let cycle = drawn[drawn.len() - 8..]
            .iter()
            .map(|frame| pixel_at(frame, WIDTH, p));
        let (least, most) = cycle.fold(([u8::MAX; 4], [0; 4]), |(least, most), pixel| {
            (
                std::array::from_fn(|c| least[c].min(pixel[c])),
                std::array::from_fn(|c| most[c].max(pixel[c])),
            )
        });
        (0..3).map(|c| most[c] - least[c]).max().expect("channels")
    };
    let across = (gy - 68..gy - 59).map(|y| [gx + 20, y]);
    let across = across.chain((gy + 59..gy + 68).map(|y| [gx + 20, y]));
    for p in across.chain((gx + 59..gx + 68).map(|x| [x, gy])) {
        let (focused, unfocused) = (
            spread(&antialiased_frames, p),
            spread(&antialiased_sharp_frames, p),
        );
        assert!(
            focused <= unfocused + 16,
            "antialiased, the edge in focus at {p:?} moves {focused} over the offsets' cycle, \
             against {unfocused} without depth of field"
        );
    }
    let at = |pixels: &[u8], [x, y]: [u32; 2]| pixel_at(pixels, WIDTH, [x, y]);
    let differs =
        |a: [u8; 4], b: [u8; 4], by: u8| a.iter().zip(b).any(|(&a, b)| a.abs_diff(b) > by);
    // The green square spans 128 pixels each way round its middle: within it, but where the red
    // square's blur reaches, as sharp as it is without depth of field.
    let [gx, gy] = pixel_of(viewpoint, size, scene.focused);
    for y in gy - 62..gy + 62 {
        for x in gx - 34..gx + 63 {
            let (got, want) = (at(&blurred, [x, y]), at(&sharp, [x, y]));
            assert!(
                !differs(got, want, 2),
                "the square in focus at ({x}, {y}) is {got:?}, not {want:?}"
            );
            let (got, want) = (at(&antialiased, [x, y]), at(&antialiased_sharp, [x, y]));
            assert!(
                !differs(got, want, 2),
                "antialiased, the square in focus at ({x}, {y}) is {got:?}, not {want:?}"
            );
            let (got, want) = (at(&smaa, [x, y]), at(&smaa_sharp, [x, y]));
            assert!(
                !differs(got, want, 2),
                "by SMAA, the square in focus at ({x}, {y}) is {got:?}, not {want:?}"
            );
        }
    }
    // The red square spans 102 pixels each way round its middle: 2 pixels out from its left edge
    // and its top edge, its light, as more red than green; 2 pixels in, less, as the background
    // shows through its blurred edge; and 4 pixels into the green square from the red's right edge,
    // its light.
    let [rx, ry] = pixel_of(viewpoint, size, scene.near);
    let redness = |pixels: &[u8], p| {
        let [r, g, ..] = at(pixels, p);
        i32::from(r) - i32::from(g)
    };
    for (outside, inside) in [
        ([rx - 53, ry], [rx - 49, ry]),
        ([rx, ry - 53], [rx, ry - 49]),
    ] {
        assert!(
            redness(&blurred, outside) > redness(&sharp, outside) + 30,
            "the red square's light doesn't spread out to {outside:?}: {:?} against {:?}",
            at(&blurred, outside),
            at(&sharp, outside)
        );
        assert!(
            redness(&smaa, outside) > redness(&smaa_sharp, outside) + 30,
            "by SMAA, the red square's light doesn't spread out to {outside:?}"
        );
        assert!(
            redness(&blurred, inside) + 20 < redness(&sharp, inside),
            "the red square at {inside:?} doesn't fade toward its edge: {:?} against {:?}",
            at(&blurred, inside),
            at(&sharp, inside)
        );
    }
    let over = [rx + 55, ry];
    assert!(
        redness(&blurred, over) > redness(&sharp, over) + 20,
        "the red square's light doesn't spread over the square in focus at {over:?}: {:?} against \
         {:?}",
        at(&blurred, over),
        at(&sharp, over)
    );
    // The blue square spans 102 pixels each way round its middle: its last pixel and the second
    // past its right edge, between its blue and the stripes', as it blurs.
    let [bx, by] = pixel_of(viewpoint, size, scene.far);
    for p in [[bx + 51, by], [bx + 53, by]] {
        assert!(
            differs(at(&blurred, p), at(&sharp, p), 20),
            "the blue square's edge doesn't blur at {p:?}: {:?}",
            at(&blurred, p)
        );
    }
    // 4 pixels in from its right edge, past its own circle, it stays blue, as the stripes behind
    // it, of a larger circle, don't spread over it.
    let inside = [bx + 47, by];
    assert!(
        !differs(at(&blurred, inside), at(&sharp, inside), 8),
        "the stripes behind the blue square spread over it to {inside:?}: {:?}",
        at(&blurred, inside)
    );
    // The stripes, along a row below the squares: their least and most, nearer each other.
    let contrast = |pixels: &[u8]| {
        let row = (0..WIDTH).map(|x| at(pixels, [x, HEIGHT - 40])[1]);
        let (least, most) = row.fold((u8::MAX, 0), |(l, m), v| (l.min(v), m.max(v)));
        most - least
    };
    assert!(
        contrast(&blurred) + 30 < contrast(&sharp),
        "the stripes keep their contrast: {} against {}",
        contrast(&blurred),
        contrast(&sharp)
    );
    let on_canvas = |pixels: &[u8]| region(pixels, WIDTH, [8, HEIGHT - 24], [16, 16]);
    assert!(on_canvas(&blurred) == on_canvas(&sharp), "the canvas blurs");
    compare("depth_of_field", WIDTH, HEIGHT, &blurred);
}

/// A plate of the scene, unlit, 1 across, at 4 ahead of a camera whose field of view is 45° from
/// bottom to top, in a view of 384 by 216, whose tiles' side, the farthest it blurs a pixel either
/// way, is 6 pixels, over stripes of flat colors 10 ahead, light and dark, 0.2 wide, without
/// antialiasing; through a shutter open half the frame. The plate moving 0.3 to the right each
/// frame, about 20 pixels, its left and right edges blur out over the stripes, up to 5 pixels, as
/// the filter keeps what moves alike at a depth alike, the plate's own pixels, mostly as they are;
/// its top and bottom edges and the stripes, still, staying as they are without motion blur; standing
/// still, as the frames before it, no pixel changes. The camera turning 4° about its up each frame,
/// about 18 pixels at the middle, everything blurs across, the stripes' contrast falling, while
/// the plate's top edge stays sharp; but cut to, as from a tool's look, the turn is a jump, which
/// blurs nothing, the view as it is without motion blur, while the moving plate still blurs by its
/// own motion. What's on the canvas, drawn after it, doesn't blur. Antialiased, temporally or by
/// SMAA, the moving plate blurs too, and by SMAA, which draws no offset, what stands still far from
/// it is as it was. Compared, the moving plate's frame and the turning camera's side by side, with
/// their reference.
fn motion_blur_smears_what_moves_and_the_turning_camera() {
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 216;
    let size = [WIDTH, HEIGHT];
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let plate = model_square(&mut stores.meshes);
    let material = stores.materials.add(&Material {
        color: Vec4::new(0.95, 0.75, 0.2, 1.0),
        workflow: metallic(0.0, 0.7),
        ..Material::default()
    });
    let draw = MeshDraw::with_material(plate, material, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    draws.draw_scene();
    for stripe in -30..30 {
        let at = Vec3::new(0.1 + 0.2 * stripe as f32, 0.0, -10.0);
        let at =
            Mat4::from_scale_rotation_translation(Vec3::new(0.2, 12.0, 1.0), Quat::IDENTITY, at);
        let shade = if stripe % 2 == 0 { 0.5 } else { 0.05 };
        draws.push(square, flat, at, Vec4::new(shade, shade, shade * 1.2, 1.0));
    }
    // A white square on the canvas, 16 pixels a side, 8 from the bottom left.
    let mut canvas = Draws::default();
    let placed = Mat4::from_translation(Vec3::new(16.0, 16.0, 0.0))
        * Mat4::from_scale(Vec3::new(16.0, 16.0, 1.0));
    canvas.push_sprite(placed, Sprite::default());
    let viewport = filling(size);
    let seen = |turn: f32| Viewpoint {
        rotation: Quat::from_rotation_y(math::to_radians(turn)),
        projection: Projection::Perspective {
            fov: FRAC_PI_4,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, -4.0));
    stores.scene.set(0, at(0.0), &draw, 1);
    settle(&mut renderer, &mut stores, viewport, seen(0.0), &draws);
    let blur = MotionBlur { shutter: 0.5 };
    // The last of 4 frames of the plate at `x(frame)` and the camera turned `turn(frame)`, the
    // last cut to, if `cut`.
    let mut render =
        |x: &dyn Fn(f32) -> f32, turn: &dyn Fn(f32) -> f32, motion_blur, antialiasing, cut| {
            let mut pixels = Vec::new();
            for frame in 0..4 {
                stores.scene.set(0, at(x(frame as f32)), &draw, 1);
                let data = RenderData {
                    motion_blur,
                    antialiasing,
                    canvas: &canvas,
                    cut: cut && frame == 3,
                    ..stores.data(viewport, seen(turn(frame as f32)), &draws)
                };
                render_without_ui(&mut renderer, data);
                pixels = frame_pixels(&mut renderer);
            }
            pixels
        };
    let none = Antialiasing::None;
    let (moving, still) = (|frame: f32| 0.3 * frame - 0.45, |_| 0.45);
    let (turning, facing) = (|frame: f32| 4.0 * frame - 6.0, |_| 6.0);
    let sharp = render(&moving, &|_| 0.0, None, none, false);
    let blurred = render(&moving, &|_| 0.0, Some(blur), none, false);
    assert!(
        render(&still, &|_| 0.0, Some(blur), none, false)
            == render(&still, &|_| 0.0, None, none, false),
        "motion blur changes a view where nothing moves"
    );
    let turned_sharp = render(&still, &turning, None, none, false);
    let turned = render(&still, &turning, Some(blur), none, false);
    let unturned = render(&still, &facing, Some(blur), none, false);
    // Cut to, the camera's turn since the frame before is a jump, which blurs nothing; the plate's
    // own motion still blurs it.
    let jumped = render(&still, &turning, Some(blur), none, true);
    let moved_jumped = render(&moving, &turning, Some(blur), none, true);
    let moved_turned_sharp = render(&moving, &turning, None, none, false);
    let antialiased = render(&moving, &|_| 0.0, Some(blur), Antialiasing::Taa, false);
    let antialiased_sharp = render(&moving, &|_| 0.0, None, Antialiasing::Taa, false);
    let smaa = render(&moving, &|_| 0.0, Some(blur), Antialiasing::Smaa, false);
    let smaa_sharp = render(&moving, &|_| 0.0, None, Antialiasing::Smaa, false);
    shut_down(renderer);
    assert!(
        jumped == turned_sharp,
        "a camera cut to blurs by its jump from the frame before"
    );
    let pixel = |pixels: &[u8], [x, y]: [u32; 2]| pixel_at(pixels, WIDTH, [x, y]);
    let differs =
        |a: [u8; 4], b: [u8; 4], by: u8| a.iter().zip(b).any(|(&a, b)| a.abs_diff(b) > by);
    // The plate, 0.45 right of the middle in the last frame, spans 65 pixels each way: a pixel out
    // from its left and right edges, along the 40 rows round its middle, its color, as it blurs over
    // the stripes, apart from theirs by a tenth on average in linear light, past the noise that
    // jitters the filter's samples from pixel to pixel.
    let [px, py] = pixel_of(seen(0.0), size, Vec3::new(0.45, 0.0, -4.0));
    let apart = |a: &[u8], b: &[u8], x: u32| {
        let rows = py - 20..py + 20;
        let sum: f32 = (rows.clone())
            .map(|y| {
                let (a, b) = (pixel(a, [x, y]), pixel(b, [x, y]));
                (0..3)
                    .map(|c| (decoded(a[c]) - decoded(b[c])).abs())
                    .sum::<f32>()
                    / 3.0
            })
            .sum();
        sum / rows.len() as f32
    };
    for x in [px - 34, px + 34] {
        let blurs = apart(&blurred, &sharp, x);
        assert!(blurs > 0.1, "the moving plate's edge blurs {blurs} at {x}");
        let blurs = apart(&antialiased, &antialiased_sharp, x);
        assert!(
            blurs > 0.1,
            "antialiased, the moving plate's edge blurs {blurs} at {x}"
        );
        let blurs = apart(&smaa, &smaa_sharp, x);
        assert!(
            blurs > 0.1,
            "by SMAA, the moving plate's edge blurs {blurs} at {x}"
        );
    }
    for p in [[px, py - 34], [px, py - 30], [px, py + 30], [px, py + 34]] {
        assert!(
            !differs(pixel(&blurred, p), pixel(&sharp, p), 4),
            "the moving plate's top or bottom edge blurs at {p:?}: {:?} against {:?}",
            pixel(&blurred, p),
            pixel(&sharp, p)
        );
    }
    // Far from the plate's tiles, the stripes stand still and are as they were.
    let corner = |pixels: &[u8]| region(pixels, WIDTH, [0, 0], [96, 64]);
    assert!(
        corner(&blurred) == corner(&sharp),
        "what stands still far from the moving plate blurs"
    );
    // By SMAA, which draws no offset, so too.
    assert!(
        corner(&smaa) == corner(&smaa_sharp),
        "by SMAA, what stands still far from the moving plate blurs"
    );
    // Cut to as the camera turns, the moving plate's edges blur by its own motion, a pixel out
    // from each, and the stripes far from it are as they were.
    let edge = |x: f32| pixel_of(seen(6.0), size, Vec3::new(x, 0.0, -4.0))[0];
    for x in [edge(-0.05) - 1, edge(0.95) + 1] {
        let blurs = apart(&moved_jumped, &moved_turned_sharp, x);
        assert!(
            blurs > 0.1,
            "cut to, the moving plate's edge blurs {blurs} at {x}"
        );
    }
    assert!(
        corner(&moved_jumped) == corner(&moved_turned_sharp),
        "cut to, what stands still far from the moving plate blurs"
    );
    let on_canvas = |pixels: &[u8]| region(pixels, WIDTH, [8, HEIGHT - 24], [16, 16]);
    assert!(
        on_canvas(&blurred) == on_canvas(&sharp) && on_canvas(&turned) == on_canvas(&sharp),
        "the canvas blurs"
    );
    // The stripes, along a row above the plate, right of where the clear color shows past their
    // end, their least and most nearer each other as the camera turns, and as they were where it
    // stops turning.
    let contrast = |pixels: &[u8]| {
        let row = (WIDTH / 4..WIDTH).map(|x| pixel(pixels, [x, 20])[1]);
        let (least, most) = row.fold((u8::MAX, 0), |(l, m), v| (l.min(v), m.max(v)));
        most - least
    };
    assert!(
        contrast(&turned) + 40 < contrast(&turned_sharp),
        "the stripes keep their contrast as the camera turns: {} against {}",
        contrast(&turned),
        contrast(&turned_sharp)
    );
    assert!(
        contrast(&unturned) == contrast(&turned_sharp),
        "the stripes blur where the camera stands still"
    );
    // 3 pixels below the plate's top edge, its color as it is, and 3 above, none of it, whose red is
    // much more than its blue, where the stripes' is less.
    let top = pixel_of(seen(6.0), size, Vec3::new(0.45, 0.5, -4.0));
    let (inside, above) = ([top[0], top[1] + 3], [top[0], top[1] - 3]);
    assert!(
        !differs(pixel(&turned, inside), pixel(&turned_sharp, inside), 4),
        "the plate's top edge blurs into it as the camera turns about its up, at {inside:?}"
    );
    let [r, _, b, _] = pixel(&turned, above);
    assert!(
        r < b + 10,
        "the plate's top edge blurs past it as the camera turns about its up, at {above:?}"
    );
    compare(
        "motion_blur",
        WIDTH * 2,
        HEIGHT,
        &side_by_side_of(&[&blurred, &turned], WIDTH, HEIGHT),
    );
}

/// A lens over a view of 256 by 144, a unit to a pixel, without a tonemapper, so that what it does
/// shows as it makes it. Its vignette of 1 over a uniform gray darkens each pixel by the cos⁴ law,
/// `(1 + (r / R)²)⁻²` of its distance from the middle over the corners', within a step of that,
/// a quarter at the corners. Its chromatic aberration of 4, over a white rectangle on black left
/// of the middle, parts red out from the middle by 4% of each pixel's way from it and blue in: red
/// alone 3 pixels out from its left edge, blue alone just out from its right one, nearer the
/// middle, white within. Its grain of 0.2 over the gray scales each pixel's light by at most a fifth
/// either way, its mean within half a percent of the gray's, its standard deviation within a tenth
/// of the triangular law's, every frame's noise its own, and the
/// same of a frame's number from one renderer to another. A white square on the canvas, after it,
/// shows as it's drawn through all three, and over grain of 1 on light at a half float's
/// brightest, 65,504, which the grain keeps within it. The three together, over gray with white squares round
/// the view, are compared with their reference.
fn lenses_vignette_fringe_and_grain_the_view() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 144;
    let size = [WIDTH, HEIGHT];
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let viewport = filling(size);
    // From the view's top left, a unit to a pixel.
    let rectangle = |min: [f32; 2], max: [f32; 2]| {
        let middle = Vec2::new(min[0] + max[0], min[1] + max[1]) * 0.5;
        let at = Vec3::new(
            middle.x - WIDTH as f32 / 2.0,
            HEIGHT as f32 / 2.0 - middle.y,
            0.0,
        );
        let side = Vec3::new(max[0] - min[0], max[1] - min[1], 1.0);
        Mat4::from_scale_rotation_translation(side, Quat::IDENTITY, at)
    };
    let white = Vec4::ONE;
    // A white square on the canvas, 16 pixels a side, 8 from the bottom left.
    let mut canvas = Draws::default();
    let placed = Mat4::from_translation(Vec3::new(16.0, 16.0, 0.0))
        * Mat4::from_scale(Vec3::new(16.0, 16.0, 1.0));
    canvas.push_sprite(placed, Sprite::default());
    // `frames` frames of a renderer of its own, of `draws` over `clear` through `lens`, antialiased
    // as `antialiasing` says. It uploads through the graphics queue, whose copies are drawable in
    // the frame that makes them, so that its frames are of the same numbers each run, as the grain
    // is: through a transfer queue, it settles in as many frames as the copies take to finish.
    let render_as =
        |draw: &dyn Fn(&mut Stores) -> Draws, clear: f32, lens, frames: usize, antialiasing| {
            let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(UploadPath::GraphicsQueue));
            let mut stores = Stores::default();
            let draws = draw(&mut stores);
            settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
            let mut drawn = Vec::new();
            for _ in 0..frames {
                let data = RenderData {
                    clear_color: [clear, clear, clear, 1.0],
                    lens,
                    antialiasing,
                    canvas: &canvas,
                    ..stores.data(viewport, viewpoint, &draws)
                };
                render_without_ui(&mut renderer, data);
                drawn.push(frame_pixels(&mut renderer));
            }
            shut_down(renderer);
            drawn
        };
    let render = |draw: &dyn Fn(&mut Stores) -> Draws, clear, lens, frames| {
        render_as(draw, clear, lens, frames, Antialiasing::None)
    };
    let nothing = |_: &mut Stores| Draws::default();
    let flat = |stores: &mut Stores| {
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        (flat, square(&mut stores.meshes))
    };
    let on_canvas = |pixels: &[u8]| region(pixels, WIDTH, [8, HEIGHT - 24], [16, 16]);
    let canvas_white = vec![255; 16 * 16 * 4];
    let vignetted = Lens {
        vignette: 1.0,
        ..Lens::NONE
    };
    let vignette = render(&nothing, 0.5, vignetted, 1).remove(0);
    // Antialiased too, the lens reading what the resolve writes for the frame after, or what SMAA
    // blends.
    let antialiased = render_as(&nothing, 0.5, vignetted, 4, Antialiasing::Taa).remove(3);
    let smaa = render_as(&nothing, 0.5, vignetted, 1, Antialiasing::Smaa).remove(0);
    for [x, y] in [
        [0, 0],
        [255, 143],
        [128, 72],
        [40, 100],
        [200, 20],
        [128, 0],
    ] {
        let middle = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - Vec2::new(128.0, 72.0);
        let away = middle.length() / Vec2::new(128.0, 72.0).length();
        let lit = 1.0 / (1.0 + away * away);
        let want = encoded(0.5 * lit * lit);
        for (frame, way) in [
            (&vignette, "drawn"),
            (&antialiased, "antialiased"),
            (&smaa, "by SMAA"),
        ] {
            let got = pixel_at(frame, WIDTH, [x, y]);
            assert!(
                (0..3).all(|c| got[c].abs_diff(want) <= 1),
                "{way}, the vignette makes ({x}, {y}) {got:?}, not {want}"
            );
        }
    }
    for frame in [&vignette, &antialiased, &smaa] {
        assert!(on_canvas(frame) == canvas_white, "the canvas is vignetted");
    }
    let fringed = Lens {
        chromatic_aberration: 4.0,
        ..Lens::NONE
    };
    let bar = |stores: &mut Stores| {
        let (flat, square) = flat(stores);
        let mut draws = Draws::default();
        draws.push(square, flat, rectangle([16.0, 16.0], [64.0, 48.0]), white);
        draws
    };
    let fringe = render(&bar, 0.0, fringed, 1).remove(0);
    let at = |[x, y]: [u32; 2]| pixel_at(&fringe, WIDTH, [x, y]);
    let [r, g, b, _] = at([13, 32]);
    assert!(
        r > 200 && g < 30 && b < 30,
        "out from the left edge, {:?}",
        [r, g, b]
    );
    let [r, g, b, _] = at([65, 32]);
    assert!(
        b > 200 && r < 30 && g < 30,
        "out from the right edge, {:?}",
        [r, g, b]
    );
    assert!(
        at([40, 32]).iter().all(|&c| c > 240),
        "within, {:?}",
        at([40, 32])
    );
    assert!(on_canvas(&fringe) == canvas_white, "the canvas is fringed");
    let grained = Lens {
        grain: 0.2,
        ..Lens::NONE
    };
    let grain = render(&nothing, 0.5, grained, 2);
    let again = render(&nothing, 0.5, grained, 2);
    assert!(
        grain == again,
        "the grain of a frame's number differs from run to run"
    );
    assert!(
        grain[0] != grain[1],
        "the grain is the same from frame to frame"
    );
    let light: Vec<f32> = (grain[1].as_chunks::<4>().0.iter())
        .map(|pixel| decoded(pixel[1]))
        .collect();
    let mean = light.iter().sum::<f32>() / light.len() as f32;
    assert!((mean - 0.5).abs() < 0.005, "the grain's mean is {mean}");
    // But the canvas's, which keeps its white.
    let canvas_pixels = 16 * 16;
    let strays = light
        .iter()
        .filter(|&&l| !(0.39..=0.61).contains(&l))
        .count();
    assert!(
        strays <= canvas_pixels,
        "{strays} pixels stray past a fifth of the gray's light"
    );
    // The grain's spread, of the pixels but the canvas's: within a tenth of the triangular law's
    // standard deviation, √(1/6), times the most it scales the gray's light by, 0.5 × 0.2, which
    // the 8 bits' rounding, about a hundredth of it, can't cross either way, and grain of half or
    // twice as much would.
    let gray: Vec<f32> = (light.iter().enumerate())
        .filter(|&(i, _)| {
            let (x, y) = (i as u32 % WIDTH, i as u32 / WIDTH);
            !((8..24).contains(&x) && (HEIGHT - 24..HEIGHT - 8).contains(&y))
        })
        .map(|(_, &l)| l)
        .collect();
    let count = gray.len() as f32;
    let middle = gray.iter().sum::<f32>() / count;
    let squares: f32 = gray.iter().map(|&l| (l - middle) * (l - middle)).sum();
    let deviation = (squares / count).sqrt();
    let law = 0.5 * 0.2 * (1.0f32 / 6.0).sqrt();
    assert!(
        (deviation / law - 1.0).abs() < 0.1,
        "the grain's standard deviation is {deviation}, against the triangular law's {law}"
    );
    assert!(
        on_canvas(&grain[1]) == canvas_white,
        "the canvas is grained"
    );
    // Over light as bright as a half float holds, grain that would scale it past that keeps it
    // there, as the canvas, blending over it, would otherwise blend over infinities into no number,
    // which shows black.
    let brightest = Lens {
        grain: 1.0,
        ..Lens::NONE
    };
    let bright = render(&nothing, 65_504.0, brightest, 1).remove(0);
    assert!(
        on_canvas(&bright) == canvas_white,
        "the canvas over grained light at a half float's brightest isn't white"
    );
    let all = Lens {
        vignette: 0.6,
        chromatic_aberration: 2.0,
        grain: 0.1,
    };
    let squares = |stores: &mut Stores| {
        let (flat, square) = flat(stores);
        let mut draws = Draws::default();
        for [x, y] in [
            [24.0, 20.0],
            [216.0, 20.0],
            [24.0, 108.0],
            [216.0, 108.0],
            [120.0, 56.0],
        ] {
            draws.push(square, flat, rectangle([x, y], [x + 16.0, y + 16.0]), white);
        }
        draws
    };
    let together = render(&squares, 0.3, all, 1).remove(0);
    compare("lens", WIDTH, HEIGHT, &together);
}

/// `frames`, each read back `width` by `height` pixels, side by side, as a reference holds frames
/// in a row.
fn side_by_side_of(frames: &[&[u8]], width: u32, height: u32) -> Vec<u8> {
    let row = (width * 4) as usize;
    (0..height as usize)
        .flat_map(|y| {
            frames
                .iter()
                .flat_map(move |frame| &frame[y * row..(y + 1) * row])
        })
        .copied()
        .collect()
}

/// A square that covers a view, with a shader that shows the frame's data as colors: where the
/// camera is, left of it, and the time and the exposure, right of it. Its exposure adapting, from
/// the same, the shader shows what the first frame's metering, back two frames later, calls for.
fn shaders_read_the_frames_data() {
    const SIZE: u32 = 16;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let shader = (stores.shaders)
        .add::<[f32; 3], Vec4>("frame data", &shader("frame_data.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    let huge = Mat4::from_scale(Vec3::splat(100.0));
    draws.push(square, shader, huge, Vec4::ZERO);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.1, 0.2, 0.3),
        ..Viewpoint::default()
    };
    let viewport = filling([SIZE, SIZE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let data = RenderData {
        time: 0.4,
        exposure: 0.6,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    let frame = frame_pixels(&mut renderer);
    for (pixel, [r, g, b]) in [([4, 8], [0.1, 0.2, 0.3]), ([12, 8], [0.4, 0.6, 0.0])] {
        let expected = [encoded(r), encoded(g), encoded(b), 255];
        let got = pixel_at(&frame, SIZE, pixel);
        assert!(
            close(got, expected),
            "{got:?} at {pixel:?}, not {expected:?}"
        );
    }
    // The first two frames before a metering comes back, the third after.
    for i in 0..3 {
        let data = RenderData {
            time: 0.4,
            exposure: 0.6,
            adaptation: Some(Adaptation::default()),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let exposure = renderer.exposure();
        let moved = (exposure / 0.6 - 1.0).abs();
        assert!(
            if i < 2 { moved < 1e-5 } else { moved > 0.5 },
            "frame {i} exposed at {exposure}"
        );
        let expected = [encoded(0.4), encoded(exposure), encoded(0.0), 255];
        let got = pixel_at(&frame_pixels(&mut renderer), SIZE, [12, 8]);
        assert!(close(got, expected), "{got:?}, not {expected:?}");
    }
    shut_down(renderer);
}

/// A row of squares, each a model's, lit head-on, seen without perspective, so that the way to
/// the eye is the light's way back at every pixel; each pixel at a square's middle checked against
/// what glTF's BRDF gives, worked out by hand. Lit by `E` = π lux, a surface whose normal is `cos`
/// from the light's way, rough 1, so that the GGX lobe is 1/π and the visibility 1/(4 cos), shows
/// `(1 - f0) c cos + f0 / 4`, `f0` 0.04 for a dielectric: 0.49 for gray of 0.5 facing the light.
/// The squares: a dielectric; one unlit, its color alone; one black and metallic, which reflects
/// nothing, emitting 0.25; one normal-mapped toward +X, by RGBA8, by BC5, and by a map never
/// filled, which stands in flat; and one whose factor is metallic, its surface map taking that to
/// a dielectric's by its blue of 0, by RGBA8, by BC7, and by a map never filled, which stands in
/// white, leaving a metal of 0.5, whose `f0` is 0.5, showing 0.125; and one normal-mapped toward
/// +X by BC7, of X, Y and Z, as a model's DDS image holds a normal map. Then a point light of 4π
/// candela 2 units over the first square lights it as the directional light did; with a range of
/// 2.5, by the window KHR_lights_punctual recommends, `1 - (2 / 2.5)^4`; and a spot over it
/// lights it alike within its inner cone, and nothing outside its outer.
fn lights_shade_as_gltf_defines() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 10;
    const WIDTH: u32 = SIDE * SQUARES;
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let square = model_square(&mut stores.meshes);
    // A level of 4 by 4 texels, X tilted toward +X by 60°, as a normal map holds it.
    let tilted: [u8; 4] = [238, 128, 255, 255];
    let textures = &mut stores.textures;
    let mut map = |name, texel: [u8; 4], map| {
        let pixels = [texel; 16];
        let added = textures.add(name, [4, 4], pixels.as_flattened(), Texels::Map(map));
        Sampled::of(added.expect("a map"))
    };
    let normals = map("normals", tilted, Map::Normal);
    let dielectric_surface = map("surface", [0, 255, 0, 255], Map::Surface);
    // BC5's two channels, each a block of its endpoints and every index 0.
    let mut bc5 = [0; 16];
    bc5[..2].fill(tilted[0]);
    bc5[8..10].fill(tilted[1]);
    let normal_map = Texels::CompressedMap {
        map: Map::Normal,
        bc5: true,
        mipmapped: false,
    };
    let compressed_normals = textures.add("bc5 normals", [4, 4], &bc5[..], normal_map);
    let compressed_normals = Sampled::of(compressed_normals.expect("a BC5 map"));
    // BC7's mode 6 holds odd channels exactly: X and Y tilted toward +X, nearly as `tilted`, and
    // Z, which the shader ignores.
    let bc7_tilted: [u8; 4] = [239, 129, 255, 255];
    let bc7_normal_map = Texels::CompressedMap {
        map: Map::Normal,
        bc5: false,
        mipmapped: false,
    };
    let block = bc7_block(bc7_tilted);
    let bc7_normals = textures.add("bc7 normals", [4, 4], &block[..], bc7_normal_map);
    let bc7_normals = Sampled::of(bc7_normals.expect("a BC7 normal map"));
    // And metallic 1/255.
    let surface_map = Texels::CompressedMap {
        map: Map::Surface,
        bc5: false,
        mipmapped: false,
    };
    let block = bc7_block([1, 255, 1, 255]);
    let compressed_surface = textures.add("bc7 surface", [4, 4], &block[..], surface_map);
    let compressed_surface = Sampled::of(compressed_surface.expect("a BC7 map"));
    let reserve = |textures: &mut Textures, name, map| {
        let reserved = textures.reserve(name, [4, 4], Texels::Map(map));
        Sampled::of(reserved.expect("a slot"))
    };
    let unfilled_normals = reserve(textures, "unfilled normals", Map::Normal);
    let unfilled_surface = reserve(textures, "unfilled surface", Map::Surface);
    let gray = Material {
        color: Vec4::new(0.5, 0.5, 0.5, 1.0),
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let metal = Material {
        workflow: metallic(1.0, 1.0),
        ..gray
    };
    let materials = [
        gray,
        Material {
            unlit: true,
            ..gray
        },
        Material {
            color: Vec4::new(0.0, 0.0, 0.0, 1.0),
            emissive: Vec3::splat(0.25),
            ..metal
        },
        Material {
            normal_texture: Some(normals),
            ..gray
        },
        Material {
            normal_texture: Some(compressed_normals),
            ..gray
        },
        Material {
            normal_texture: Some(unfilled_normals),
            ..gray
        },
        Material {
            surface_texture: dielectric_surface,
            ..metal
        },
        Material {
            surface_texture: compressed_surface,
            ..metal
        },
        Material {
            surface_texture: unfilled_surface,
            ..metal
        },
        Material {
            normal_texture: Some(bc7_normals),
            ..gray
        },
    ];
    let at = |i: u32| Vec3::new(i as f32 - (SQUARES - 1) as f32 / 2.0, 0.0, 0.0);
    let mut draws = Draws::default();
    for (i, material) in (0..).zip(&materials) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(Mat4::from_translation(at(i)), &draw, 0);
    }
    let pi = Vec3::splat(PI);
    let sun = Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::NEG_Z,
        color: pi,
        range: None,
        shadow_distance: None,
    };
    stores.lights = vec![sun];
    let viewport = filling([WIDTH, SIDE]);
    // The unfilled maps' textures never stream, so frames are rendered until the rest have.
    for _ in 0..4 {
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    }
    // The shaded value of gray of 0.5, dielectric, whose normal is `cos` from the light's way.
    let lit = |cos: f32| 0.96 * 0.5 * cos + 0.01;
    // A tilted normal as the shader makes it from the map's X and Y.
    let cos_of = |tilted: [u8; 4]| {
        let [x, y] = [tilted[0], tilted[1]].map(|byte| f32::from(byte) / 255.0 * 2.0 - 1.0);
        let z = (1.0 - x * x - y * y).sqrt();
        z / (x * x + y * y + z * z).sqrt()
    };
    let tilted_cos = cos_of(tilted);
    let flat = {
        let flat = 128.0_f32 / 255.0 * 2.0 - 1.0;
        let z = (1.0 - 2.0 * flat * flat).sqrt();
        z / (2.0 * flat * flat + z * z).sqrt()
    };
    // A metallic of 1/255: f0 a 255th of the way from 0.04 to 0.5, and as much less diffuse.
    let barely = 1.0 / 255.0;
    let f0 = 0.04 + (0.5 - 0.04) * barely;
    let barely_metallic = 0.5 * (1.0 - barely) * (1.0 - f0) + f0 / 4.0;
    let expected = [
        lit(1.0),
        0.5,
        0.25,
        lit(tilted_cos),
        lit(tilted_cos),
        lit(flat),
        lit(1.0),
        barely_metallic,
        0.125,
        lit(cos_of(bc7_tilted)),
    ];
    let shown = |renderer: &mut Renderer, i: u32| {
        let frame = frame_pixels(renderer);
        pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2])
    };
    let gray_of = |value: f32| {
        let byte = encoded(value);
        [byte, byte, byte, 255]
    };
    for (i, value) in (0..).zip(expected) {
        let got = shown(&mut renderer, i);
        assert!(
            close(got, gray_of(value)),
            "square {i} shows {got:?}, not {value} encoded, {:?}",
            gray_of(value)
        );
    }
    let over_first = at(0) + Vec3::new(0.0, 0.0, 2.0);
    let point = Light {
        kind: LightKind::Point,
        position: over_first,
        color: Vec3::splat(4.0 * PI),
        ..sun
    };
    let reach2 = (2.0_f32 / 2.5) * (2.0 / 2.5);
    let window = 1.0 - reach2 * reach2;
    let spot = Light {
        kind: LightKind::Spot {
            inner: 0.1,
            outer: 0.2,
        },
        ..point
    };
    let cases = [
        (point, [(0, lit(1.0))]),
        (
            Light {
                range: Some(2.5),
                ..point
            },
            [(0, lit(1.0) * window)],
        ),
        (spot, [(0, lit(1.0))]),
    ];
    for (light, checks) in cases {
        stores.lights = vec![light];
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        for (i, value) in checks {
            let got = shown(&mut renderer, i);
            assert!(
                close(got, gray_of(value)),
                "{light:?}: square {i} shows {got:?}, not {value} encoded"
            );
        }
    }
    // Outside the spot's outer cone, the dielectric of the surface map shows nothing.
    assert!(close(shown(&mut renderer, 6), [0, 0, 0, 255]));
    shut_down(renderer);
}

/// Five squares, each a model's, normal-mapped, seen without perspective and lit from above and in
/// front, the first four of a double-sided material: the back of one turned a half turn about Y,
/// its map tilted toward its texture's +Y, shows as the front of one whose map tilts as far toward
/// -Y, as glTF flips a back face's whole tangent space, its bitangent too; and not as the front of
/// one whose map tilts toward +Y, which faces the light more. The front of one mirrored across X,
/// whose corners wind the other way as seen, shows as it does unmirrored, double-sided or not.
fn back_faces_flip_their_normal_maps() {
    const SIDE: u32 = 16;
    const WIDTH: u32 = 5 * SIDE;
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let square = model_square(&mut stores.meshes);
    // Levels of 4 by 4 texels, Y of -127/255 and 127/255, as a normal map holds them: about 30°.
    let mut map = |name, y: u8| {
        let pixels = [[128, y, 255, 255]; 16];
        let texels = Texels::Map(Map::Normal);
        let added = stores
            .textures
            .add(name, [4, 4], pixels.as_flattened(), texels);
        Sampled::of(added.expect("a map"))
    };
    let (down, up) = (map("tilted down", 64), map("tilted up", 191));
    let at = |i: f32| Mat4::from_translation(Vec3::new(i - 2.0, 0.0, 0.0));
    let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let squares = [
        (down, true, at(0.0)),
        (up, true, at(1.0) * Mat4::from_rotation_y(PI)),
        (up, true, at(2.0)),
        (down, true, at(3.0) * mirrored),
        (down, false, at(4.0) * mirrored),
    ];
    let mut draws = Draws::default();
    for (normals, double_sided, transform) in squares {
        let material = stores.materials.add(&Material {
            color: Vec4::new(0.5, 0.5, 0.5, 1.0),
            workflow: metallic(0.0, 1.0),
            double_sided,
            normal_texture: Some(normals),
            ..Material::default()
        });
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(transform, &draw, 0);
    }
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::new(0.0, -1.0, -1.0).normalize(),
        color: Vec3::splat(PI),
        range: None,
        shadow_distance: None,
    }];
    let viewport = filling([WIDTH, SIDE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // Past the frame the mesh came to be drawable in, which planned no room for its meshlets and
    // drew it by its indices.
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let frame = frame_pixels(&mut renderer);
    let [down, back, up, mirrored, mirrored_one_side] =
        [0, 1, 2, 3, 4].map(|i| pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2]));
    assert!(
        close(back, down),
        "the back shows {back:?}, not {down:?}, as the front tilted down does"
    );
    assert!(
        !close(back, up),
        "the back shows {back:?}, as the front tilted up does"
    );
    for (sided, shown) in [("double", mirrored), ("single", mirrored_one_side)] {
        assert!(
            close(shown, down),
            "the {sided}-sided front mirrored shows {shown:?}, not {down:?}, as it does unmirrored"
        );
    }
    shut_down(renderer);
}

/// A sphere of `radius` about the origin, a model's, of `rings` rings from pole to pole and
/// `segments` around, its normals out and its tangents along its lines of latitude, eastward.
fn sphere(meshes: &mut Meshes, radius: f32, rings: u32, segments: u32) -> Mesh<PackedVertex> {
    let (vertices, indices) = sphere_of(radius, rings, segments);
    add_model(meshes, "sphere", &vertices, &indices)
}

/// The vertices and indices of [`sphere`]'s mesh.
fn sphere_of(radius: f32, rings: u32, segments: u32) -> (Vec<ModelVertex>, Vec<u32>) {
    use math::libm::{cosf, sinf};
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let polar = PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let around = 2.0 * PI * segment as f32 / segments as f32;
            let normal = Vec3::new(
                sinf(polar) * cosf(around),
                cosf(polar),
                -sinf(polar) * sinf(around),
            );
            vertices.push(ModelVertex {
                position: normal * radius,
                uv: Vec2::new(segment as f32 / segments as f32, ring as f32 / rings as f32),
                color: Vec4::ONE,
                normal,
                tangent: Vec4::new(-sinf(around), 0.0, -cosf(around), 1.0),
            });
        }
    }
    let mut indices = Vec::new();
    let across = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let at = ring * across + segment;
            let (below, next) = (at + across, at + 1);
            indices.extend_from_slice(&[at, below, next, next, below, below + 1]);
        }
    }
    (vertices, indices)
}

/// Spheres of one color, opaque and rough 0 to 1 from left to right, metallic 0 to 1 from top to
/// bottom, lit by a light from the upper left, in front, as a camera with perspective sees them,
/// tonemapped by PBR Neutral, against their reference image.
fn lit_spheres_match_their_reference() {
    const SIZE: u32 = 200;
    const STEPS: u32 = 5;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 9.0),
        projection: Projection::Perspective {
            fov: PI / 6.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let sphere = sphere(&mut stores.meshes, 0.42, 24, 48);
    let mut draws = Draws::default();
    let step = |i: u32| i as f32 / (STEPS - 1) as f32;
    for row in 0..STEPS {
        for column in 0..STEPS {
            let material = Material {
                color: Vec4::new(0.9, 0.45, 0.2, 1.0),
                workflow: metallic(step(row), step(column)),
                ..Material::default()
            };
            let material = stores.materials.add(&material);
            let draw =
                MeshDraw::with_material(sphere, material, &stores.materials, &mut stores.shaders);
            let middle = (STEPS - 1) as f32 / 2.0;
            let at = Vec3::new(column as f32 - middle, middle - row as f32, 0.0);
            draws.push_draw(Mat4::from_translation(at), &draw, 0);
        }
    }
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::new(1.0, -1.0, -1.0).normalize(),
        color: Vec3::splat(4.0),
        range: None,
        shadow_distance: None,
    }];
    let viewport = filling([SIZE, SIZE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let data = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    check(renderer, "lit_spheres", SIZE, SIZE);
}

/// A floor and spheres under many lights: 81 points of six colors, each 0.8 over the floor and
/// reaching 2.2, in a grid 1.5 apart that runs past what the views see, so that some shine into
/// them from outside; four white spots, high over the floor, shining down; a point without a
/// range, far off; and a sun among them, casting shadows. Seen with perspective, then without,
/// against their reference images, which every light shading each fragment drew.
fn many_lights_match_their_reference() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 144;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let square = model_square(&mut stores.meshes);
    let ball = sphere(&mut stores.meshes, 0.6, 16, 32);
    let floor = Material {
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let shiny = Material {
        color: Vec4::new(0.8, 0.8, 0.8, 1.0),
        workflow: metallic(0.0, 0.4),
        ..Material::default()
    };
    let mut draws = Draws::default();
    let floor = stores.materials.add(&floor);
    let floor = MeshDraw::with_material(square, floor, &stores.materials, &mut stores.shaders);
    draws.push_draw(facing_up(24.0, Vec3::ZERO), &floor, 0);
    let shiny = stores.materials.add(&shiny);
    let ball = MeshDraw::with_material(ball, shiny, &stores.materials, &mut stores.shaders);
    for at in [
        [-2.5, -1.0],
        [0.0, 1.5],
        [2.5, -1.0],
        [-1.0, -3.5],
        [3.5, 2.5],
    ] {
        let at = Vec3::new(at[0], 0.6, at[1]);
        draws.push_draw(Mat4::from_translation(at), &ball, 0);
    }
    let colors = [
        Vec3::new(1.0, 0.2, 0.2),
        Vec3::new(0.2, 1.0, 0.2),
        Vec3::new(0.2, 0.3, 1.0),
        Vec3::new(1.0, 0.9, 0.2),
        Vec3::new(1.0, 0.2, 1.0),
        Vec3::new(0.2, 1.0, 1.0),
    ];
    let mut lights = Vec::new();
    for row in 0..9 {
        for column in 0..9 {
            let at = Vec3::new(column as f32 * 1.5 - 6.0, 0.8, row as f32 * 1.5 - 6.0);
            lights.push(Light {
                kind: LightKind::Point,
                position: at,
                direction: Vec3::NEG_Y,
                color: colors[(row * 9 + column) % colors.len()],
                range: Some(2.2),
                shadow_distance: None,
            });
        }
        // The sun among them, rather than first, as a world's lights come in any order.
        if row == 4 {
            lights.push(sun(Vec3::new(0.4, -1.0, -0.3), Some(40.0)));
        }
    }
    for at in [[-3.0, -3.0], [3.0, -3.0], [-3.0, 3.0], [3.0, 3.0]] {
        lights.push(Light {
            kind: LightKind::Spot {
                inner: 0.3,
                outer: 0.5,
            },
            position: Vec3::new(at[0], 4.0, at[1]),
            direction: Vec3::NEG_Y,
            color: Vec3::splat(3.0),
            range: Some(7.0),
            shadow_distance: None,
        });
    }
    lights.push(Light {
        kind: LightKind::Point,
        position: Vec3::new(0.0, 3.0, -9.0),
        direction: Vec3::NEG_Y,
        color: Vec3::new(2.0, 1.4, 0.8),
        range: None,
        shadow_distance: None,
    });
    stores.lights = lights;
    let viewport = filling([WIDTH, HEIGHT]);
    let tilt = -math::libm::atan2f(7.0, 10.0);
    let perspective = Viewpoint {
        position: DVec3::new(0.0, 7.0, 10.0),
        rotation: Quat::from_rotation_x(tilt),
        projection: Projection::Perspective {
            fov: FRAC_PI_4,
            near: 0.5,
        },
    };
    settle(&mut renderer, &mut stores, viewport, perspective, &draws);
    compare("many_lights", WIDTH, HEIGHT, &frame_pixels(&mut renderer));
    let orthographic = looking_down(Vec3::new(0.0, 10.0, 0.0), 12.0, 20.0);
    render_without_ui(&mut renderer, stores.data(viewport, orthographic, &draws));
    check(renderer, "many_lights_orthographic", WIDTH, HEIGHT);
}

/// A sky's texture of `size` texels, each `texel(x, y)`'s linear RGB, as [`sky_pixels`] makes them.
fn sky_texture(
    textures: &mut Textures,
    size: [u32; 2],
    texel: impl Fn(u32, u32) -> [f32; 3],
) -> Texture {
    textures
        .add("sky", size, sky_pixels(size, texel), Texels::Hdr)
        .expect("a sky")
}

/// The pixels of a sky's image of `size` texels, each `texel(x, y)`'s linear RGB, as
/// [`Texels::Hdr`] takes them, alpha 1.
fn sky_pixels(size: [u32; 2], texel: impl Fn(u32, u32) -> [f32; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let [r, g, b] = texel(x, y);
            for channel in [r, g, b, 1.0] {
                pixels.extend(half::f16::from_f32(channel).to_ne_bytes());
            }
        }
    }
    pixels
}

/// Squares, each a model's, seen head-on without perspective under a sky of 0.5 every way, with
/// no lights, prefiltered from the frame after its texture shows: as Fdez-Agüera's scattering many
/// times over keeps what a white surface reflects whole, every white one shows the sky's 0.5,
/// dielectric or metal, smooth or rough; a smooth metal of another color its color of it, as the
/// split sum's scale and bias give; one whose occlusion map holds a half, a half of it; and an
/// unlit one its color alone. The sky shows behind them, at its luminance times the exposure. Its
/// texture given another image, of half the light, the frame after that shows prefilters it again.
fn a_uniform_sky_lights_white_as_itself() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 7;
    const WIDTH: u32 = SIDE * (SQUARES + 1);
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    // 2 times its luminance, 0.25: 0.5, which shows as it is, unlike 1, which a brighter value
    // would clamp to.
    let sky = sky_texture(&mut stores.textures, [128, 64], |_, _| [2.0; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 0.25,
    });
    let half_occluded = stores.textures.add(
        "occlusion",
        [4, 4],
        [[128, 255, 255, 255]; 16].as_flattened(),
        Texels::Map(Map::Surface),
    );
    let half_occluded = Sampled::of(half_occluded.expect("an occlusion map"));
    let square = model_square(&mut stores.meshes);
    let white = Material {
        color: Vec4::ONE,
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let tint = Vec4::new(1.0, 0.5, 0.25, 1.0);
    let materials = [
        white,
        Material {
            workflow: metallic(0.0, 0.0),
            ..white
        },
        Material {
            workflow: metallic(1.0, 0.5),
            ..white
        },
        Material {
            workflow: metallic(1.0, 1.0),
            ..white
        },
        Material {
            color: tint,
            workflow: metallic(1.0, 0.0),
            ..white
        },
        Material {
            occlusion_texture: half_occluded,
            ..white
        },
        Material {
            color: Vec4::new(0.3, 0.3, 0.3, 1.0),
            unlit: true,
            ..white
        },
    ];
    let at = |i: u32| Vec3::new(i as f32 - SQUARES as f32 / 2.0, 0.0, 0.0);
    let mut draws = Draws::default();
    for (i, material) in (0..).zip(&materials) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(Mat4::from_translation(at(i)), &draw, 0);
    }
    let viewport = filling([WIDTH, SIDE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let frame = frame_pixels(&mut renderer);
    let occluded = 0.5 * 128.0 / 255.0;
    let expected = [
        [0.5; 3],
        [0.5; 3],
        [0.5; 3],
        [0.5; 3],
        [0.5 * tint.x, 0.5 * tint.y, 0.5 * tint.z],
        [occluded; 3],
        [0.3; 3],
        // The sky, past the squares.
        [0.5; 3],
    ];
    for (i, rgb) in (0..).zip(expected) {
        let [r, g, b] = rgb.map(encoded);
        let got = pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2]);
        // A level's rounding and the table's sampling may take a smooth metal's reflection a
        // little off its color.
        let near = got
            .iter()
            .zip([r, g, b, 255])
            .all(|(&a, b)| a.abs_diff(b) <= 2);
        assert!(near, "square {i} shows {got:?}, not {rgb:?} encoded");
    }
    // Its texture given another image, of half the light, it's prefiltered again once that shows.
    let dimmer = [[1.0_f32, 1.0, 1.0, 1.0].map(|value| half::f16::from_f32(value).to_ne_bytes())];
    let dimmer = dimmer.as_flattened().as_flattened().repeat(128 * 64);
    let replaced = stores
        .textures
        .replace("sky", sky, [128, 64], dimmer, Texels::Hdr);
    replaced.expect("another image");
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let frame = frame_pixels(&mut renderer);
    let quarter = encoded(0.25);
    for i in [0, 3, SQUARES] {
        let got = pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2]);
        assert!(
            close(got, [quarter, quarter, quarter, 255]),
            "square {i} shows {got:?} under the dimmer sky"
        );
    }
    shut_down(renderer);
}

/// A sky of a color a way, seen through a narrow perspective each way in turn: each of the four
/// ways between the axes round the horizon, each its column's color in the image's middle row, as
/// the image's center column faces +X, a quarter of the way across -Z; straight up its top row's,
/// and straight down its bottom row's; at its luminance times the exposure. Its texture given a
/// BC6H block of one color instead, the sky shows that color as the GPU decodes it.
fn the_sky_shows_each_way_behind_the_world() {
    const SIZE: u32 = 3;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let colors = [
        [0.25, 0.0, 0.0],
        [0.0, 0.25, 0.0],
        [0.0, 0.0, 0.25],
        [0.25, 0.25, 0.0],
    ];
    let (top, bottom) = ([0.0, 0.25, 0.25], [0.125, 0.125, 0.125]);
    let sky = sky_texture(&mut stores.textures, [4, 3], |x, y| match y {
        0 => top,
        1 => colors[x as usize],
        _ => bottom,
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 8.0,
    });
    let draws = Draws::default();
    let viewport = filling([SIZE, SIZE]);
    // Facing `way`, from anywhere: the sky is as far as can be.
    let looking = |way: Vec3| Viewpoint {
        position: DVec3::new(5.0, -3.0, 2.0),
        rotation: Quat::from_rotation_arc(Vec3::NEG_Z, way),
        projection: Projection::Perspective {
            fov: 0.1,
            near: 0.5,
        },
    };
    settle(
        &mut renderer,
        &mut stores,
        viewport,
        looking(Vec3::X),
        &draws,
    );
    let ways = [
        (Vec3::new(-1.0, 0.0, -1.0), colors[0]),
        (Vec3::new(1.0, 0.0, -1.0), colors[1]),
        (Vec3::new(1.0, 0.0, 1.0), colors[2]),
        (Vec3::new(-1.0, 0.0, 1.0), colors[3]),
        (Vec3::Y, top),
        (Vec3::NEG_Y, bottom),
    ];
    for (way, color) in ways {
        let data = RenderData {
            exposure: 0.5,
            ..stores.data(viewport, looking(way.normalize()), &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        // Of 8 times the exposure's 0.5.
        let [r, g, b] = color.map(|channel: f32| encoded(channel * 4.0));
        let got = pixel_at(&frame, SIZE, [1, 1]);
        assert!(
            close(got, [r, g, b, 255]),
            "looking {way}, {got:?}, not {color:?} times 4 encoded"
        );
    }
    // A block of BC6H's mode 11, its bits from the lowest: the mode, 3, in 5 bits; both endpoints,
    // the same, each red, green and blue in 10 bits; and every texel's index, 0, picking the first.
    // Unsigned, an endpoint of 10 bits that's neither 0 nor 1023 decodes to the 16-bit float of
    // bits 31 times it plus 15: about 0.125, 0.0625 and 0.03125.
    let [r, g, b] = [396_u128, 363, 330];
    let block = (3 | r << 5 | g << 15 | b << 25 | r << 35 | g << 45 | b << 55).to_le_bytes();
    let replaced = (stores.textures).replace("sky", sky, [4, 4], &block[..], Texels::Bc6h);
    replaced.expect("a compressed sky");
    settle(
        &mut renderer,
        &mut stores,
        viewport,
        looking(Vec3::X),
        &draws,
    );
    let data = RenderData {
        exposure: 0.5,
        ..stores.data(viewport, looking(Vec3::X), &draws)
    };
    render_without_ui(&mut renderer, data);
    let frame = frame_pixels(&mut renderer);
    let color = [r, g, b].map(|endpoint| half::f16::from_bits(31 * endpoint as u16 + 15).to_f32());
    let [r, g, b] = color.map(|channel| encoded(channel * 4.0));
    let got = pixel_at(&frame, SIZE, [1, 1]);
    assert!(
        close(got, [r, g, b, 255]),
        "a BC6H sky shows {got:?}, not {color:?} times 4 encoded"
    );
    shut_down(renderer);
}

/// Spheres of one color, opaque and rough 0 to 1 from left to right, metallic 0 to 1 from top to
/// bottom, lit by a sky alone, a gradient from a blue zenith to a pale horizon over dark ground,
/// with a bright patch where a sun would be, high to the upper left, which shows behind them, as a
/// camera with perspective sees them, tonemapped by PBR Neutral, against their reference image.
fn sky_lit_spheres_match_their_reference() {
    const SIZE: u32 = 200;
    const STEPS: u32 = 5;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 9.0),
        projection: Projection::Perspective {
            fov: PI / 6.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    let [width, height] = [128, 64];
    let sky = sky_texture(&mut stores.textures, [width, height], |x, y| {
        // How far up, from -1 straight down to 1 straight up, and round, from 0 at +X.
        let up = 1.0 - 2.0 * (y as f32 + 0.5) / height as f32;
        let round = (x as f32 + 0.5) / width as f32;
        let sun = (round - 0.15).abs() < 0.04 && (up - 0.6).abs() < 0.08;
        if sun {
            [40.0, 36.0, 30.0]
        } else if up > 0.0 {
            let pale = 1.0 - up;
            [0.3 + 0.6 * pale, 0.5 + 0.4 * pale, 1.0]
        } else {
            [0.12, 0.09, 0.06]
        }
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let sphere = sphere(&mut stores.meshes, 0.42, 24, 48);
    let mut draws = Draws::default();
    let step = |i: u32| i as f32 / (STEPS - 1) as f32;
    for row in 0..STEPS {
        for column in 0..STEPS {
            let material = Material {
                color: Vec4::new(0.9, 0.45, 0.2, 1.0),
                workflow: metallic(step(row), step(column)),
                ..Material::default()
            };
            let material = stores.materials.add(&material);
            let draw =
                MeshDraw::with_material(sphere, material, &stores.materials, &mut stores.shaders);
            let middle = (STEPS - 1) as f32 / 2.0;
            let at = Vec3::new(column as f32 - middle, middle - row as f32, 0.0);
            draws.push_draw(Mat4::from_translation(at), &draw, 0);
        }
    }
    let viewport = filling([SIZE, SIZE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let data = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    check(renderer, "sky_lit_spheres", SIZE, SIZE);
}

/// The scene the tests of ambient occlusion see, under the stores' sky and lights, as `viewpoint`
/// sees it, a camera with perspective in front of it and above: a floor, a model's, 6 units across
/// and white; a wall on its left, a model's too, facing +X at -1.5; a wall at its back, of a game's
/// shader of one color, facing +Z at -1, which a view beside a visibility buffer draws otherwise;
/// a sphere of 0.5 on the floor, of ID [`BALL`], and one of 0.3 near the left wall; a masked panel
/// 0.8 across, facing up 0.3 over the floor, cut out in its middle half each way; and a skinned
/// strip, standing, which its joints carry a unit along -Z from where it was bound, of a skinned
/// mesh, which a view beside a visibility buffer draws otherwise too. And the world's points where
/// the floor meets the large sphere, the corner of the walls and the back wall alone, out in the
/// open, up the back wall, the middle of the panel's hole, and where the floor meets the strip, as
/// it's posed and as it was bound.
struct OcclusionScene {
    draws: Draws,
    viewpoint: Viewpoint,
    ball: Vec3,
    contact: Vec3,
    corner: Vec3,
    by_back_wall: Vec3,
    open: Vec3,
    back_wall: Vec3,
    hole: Vec3,
    strip: Vec3,
    unposed: Vec3,
}

/// The ID of [`occlusion_scene`]'s large sphere.
const BALL: u64 = 7;

fn occlusion_scene(stores: &mut Stores) -> OcclusionScene {
    let plain = square(&mut stores.meshes);
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let model = model_square(&mut stores.meshes);
    let ball = sphere(&mut stores.meshes, 1.0, 24, 48);
    // Opaque, but for its middle half each way.
    let holed: Vec<[u8; 4]> = (0..16)
        .map(|i: u32| {
            let inside = |at: u32| (1..3).contains(&at);
            let alpha = if inside(i % 4) && inside(i / 4) {
                0
            } else {
                255
            };
            [255, 255, 255, alpha]
        })
        .collect();
    let holed = (stores.textures).add(
        "holed",
        [4, 4],
        holed.as_flattened(),
        Texels::Color(Mipmaps::None),
    );
    let holed = Sampled::of(holed.expect("a texture"));
    // A strip from y = -1 to 1 and x = -0.25 to 0.25, bound to joint 0.
    let mut vertices = Vec::new();
    for y in [-1.0, 1.0] {
        for x in [-0.25, 0.25] {
            vertices.push(ModelVertex {
                position: Vec3::new(x, y, 0.0),
                uv: Vec2::ZERO,
                color: Vec4::ONE,
                normal: Vec3::Z,
                tangent: Vec4::X,
            });
        }
    }
    let skin = vec![SkinVertex::new([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]); 4];
    let reach = [Vec3::splat(-1.5), Vec3::splat(1.5)];
    let indices = vec![0, 1, 3, 3, 2, 0];
    let strip = ModelMesh::pack_skinned("strip", &vertices, indices, Vec::new(), skin, reach);
    let strip = (stores.meshes).add_model("strip", &strip.expect("a mesh"));
    let strip = strip.expect("a mesh");
    let add = |materials: &mut Materials, material: Material| materials.add(&material);
    let gray = |color: f32, roughness: f32| Material {
        color: Vec4::new(color, color, color, 1.0),
        workflow: metallic(0.0, roughness),
        ..Material::default()
    };
    let floor = add(&mut stores.materials, gray(0.8, 1.0));
    let walls = add(&mut stores.materials, gray(0.7, 1.0));
    let round = add(&mut stores.materials, gray(0.8, 0.5));
    let panel = Material {
        color_texture: holed,
        alpha: AlphaMode::Mask { cutoff: 0.5 },
        ..gray(0.6, 1.0)
    };
    let panel = add(&mut stores.materials, panel);
    let (materials, shaders) = (&stores.materials, &mut stores.shaders);
    let floor = MeshDraw::with_material(model, floor, materials, shaders);
    let wall = MeshDraw::with_material(model, walls, materials, shaders);
    let round = MeshDraw::with_material(ball, round, materials, shaders);
    let panel = MeshDraw::with_material(model, panel, materials, shaders);
    let standing = MeshDraw::with_material(strip, walls, materials, shaders);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(6.0, Vec3::ZERO), &floor, 0);
    let placed = |scale: Vec3, rotation: Quat, at: Vec3| {
        Mat4::from_scale_rotation_translation(scale, rotation, at)
    };
    let left = Quat::from_rotation_y(FRAC_PI_2);
    let left = placed(Vec3::new(6.0, 4.0, 1.0), left, Vec3::new(-1.5, 2.0, 0.0));
    draws.push_draw(left, &wall, 0);
    let back = placed(
        Vec3::new(6.0, 4.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(0.0, 2.0, -1.0),
    );
    draws.push(plain, flat, back, Vec4::new(0.35, 0.4, 0.5, 1.0));
    let ball = Vec3::new(0.3, 0.5, 0.0);
    for (radius, at, id) in [(0.5, ball, BALL), (0.3, Vec3::new(-1.15, 0.3, 0.5), 0)] {
        draws.push_draw(placed(Vec3::splat(radius), Quat::IDENTITY, at), &round, id);
    }
    let hole = Vec3::new(-0.7, 0.3, 1.6);
    draws.push_draw(facing_up(0.8, hole), &panel, 0);
    // Bound standing at 0.8 along Z, and posed a unit along -Z from there.
    draws.push_draw(
        Mat4::from_translation(Vec3::new(2.2, 1.0, 0.8)),
        &standing,
        0,
    );
    draws.skin(strip, [Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0))]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.4, 3.4),
        rotation: Quat::from_rotation_x(-0.6),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
    };
    OcclusionScene {
        draws,
        viewpoint,
        ball,
        contact: Vec3::new(0.3, 0.0, 0.4),
        corner: Vec3::new(-1.35, 0.0, -0.85),
        by_back_wall: Vec3::new(1.6, 0.0, -0.85),
        open: Vec3::new(1.8, 0.0, 1.2),
        back_wall: Vec3::new(0.5, 1.6, -1.0),
        hole,
        strip: Vec3::new(2.2, 0.0, -0.05),
        unposed: Vec3::new(2.2, 0.0, 0.95),
    }
}

/// The pixel of a view `size` pixels across and down that `viewpoint` sees `at` at.
fn pixel_of(viewpoint: Viewpoint, size: [u32; 2], at: Vec3) -> [u32; 2] {
    let [width, height] = size.map(|side| side as f32);
    let clip = viewpoint.view_projection(width / height) * at.extend(1.0);
    let ndc = Vec2::new(clip.x, clip.y) / clip.w;
    let pixel = (ndc + 1.0) / 2.0 * Vec2::new(width, height);
    [pixel.x as u32, pixel.y as u32]
}

/// [`occlusion_scene`] lit by a sky alone, of 0.5 every way, with ambient occlusion and without:
/// where the floor meets the large sphere, and in the corner of the walls, the sky's light is
/// darkened, and where the floor meets the back wall, a game's shader's, and the skinned strip as
/// it's posed, whose depth occludes too, as a view beside a visibility buffer draws them
/// otherwise; through the panel's hole, the floor under it, as the depth of a masked material is
/// cut out as it is; out in the open, and where the strip was bound, it's as it is without; and the
/// back wall, which reads none, shows its color either way. Against its reference image, without
/// antialiasing and antialiased alike. In a still view, the occlusion takes 8 frames' noise in
/// turn, which its history averages: once it settles, each frame draws the view as the frame 8
/// before it did, as temporal antialiasing's resolve asks of a still pixel it keeps unclipped.
fn ambient_occlusion_darkens_the_skys_light_near_what_hides_it() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.5; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let scene = occlusion_scene(&mut stores);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    // Past the frame the meshes came to be drawable in, which planned no room for their meshlets
    // and drew them by their indices.
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, draws));
    let without = frame_pixels(&mut renderer);
    let on = Some(AmbientOcclusion::default());
    let data = RenderData {
        ambient_occlusion: on,
        ..stores.data(viewport, viewpoint, draws)
    };
    render_without_ui(&mut renderer, data);
    let with = frame_pixels(&mut renderer);
    let shown = |frame: &[u8], at: Vec3| {
        let pixel = pixel_at(frame, WIDTH, pixel_of(viewpoint, [WIDTH, HEIGHT], at));
        i32::from(pixel[1])
    };
    for (name, at, least) in [
        ("where the floor meets the sphere", scene.contact, 12),
        ("in the corner of the walls", scene.corner, 20),
        (
            "where the floor meets the back wall",
            scene.by_back_wall,
            12,
        ),
        ("through the panel's hole", scene.hole, 12),
        ("where the floor meets the strip", scene.strip, 12),
    ] {
        let (lit, darkened) = (shown(&without, at), shown(&with, at));
        assert!(
            darkened <= lit - least,
            "{name}, {darkened} with ambient occlusion, not {least} darker than {lit} without"
        );
    }
    for (name, at) in [
        ("out in the open", scene.open),
        ("where the strip was bound", scene.unposed),
    ] {
        let (lit, darkened) = (shown(&without, at), shown(&with, at));
        assert!(
            (lit - darkened).abs() <= 6,
            "{name}, {darkened} with ambient occlusion, not near {lit} without"
        );
    }
    let wall = |frame: &[u8]| {
        let pixel = pixel_of(viewpoint, [WIDTH, HEIGHT], scene.back_wall);
        pixel_at(frame, WIDTH, pixel)
    };
    assert_eq!(
        wall(&with),
        wall(&without),
        "the back wall reads no occlusion"
    );
    compare("ambient_occlusion", WIDTH, HEIGHT, &with);
    let frame = |renderer: &mut Renderer, stores: &mut Stores, antialiasing| {
        let data = RenderData {
            ambient_occlusion: on,
            antialiasing,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(renderer, data);
    };
    // Settled, the frame 8 after one draws it again: the noise and the history have both come
    // round, as noise that changed other than with its 8 frames, or a history that hadn't settled,
    // wouldn't.
    for _ in 0..128 {
        frame(&mut renderer, &mut stores, Antialiasing::None);
    }
    let first = frame_pixels(&mut renderer);
    for _ in 0..8 {
        frame(&mut renderer, &mut stores, Antialiasing::None);
    }
    let again = frame_pixels(&mut renderer);
    let changed = (first.as_chunks::<4>().0.iter())
        .zip(again.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 0, "pixels differ 8 frames on in a still view");
    for _ in 0..16 {
        frame(&mut renderer, &mut stores, Antialiasing::Taa);
    }
    check(renderer, "ambient_occlusion", WIDTH, HEIGHT);
}

/// [`occlusion_scene`] under a sky of 0.5 every way, with ambient occlusion, seen first looking up,
/// so that the culling finds the floor, the spheres, the panel and the strip outside the view, and
/// then as the other tests see it: that frame draws them in the view's second pass, their depth
/// alone too, and darkens where the floor meets the large sphere as a frame without ambient
/// occlusion doesn't; and a pick of the large sphere there, in that frame, finds it.
fn ambient_occlusion_darkens_what_comes_into_view_in_the_second_pass() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.5; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    stores.ambient_occlusion = Some(AmbientOcclusion::default());
    let scene = occlusion_scene(&mut stores);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    let up = Viewpoint {
        rotation: Quat::from_rotation_x(1.2),
        ..viewpoint
    };
    settle(&mut renderer, &mut stores, viewport, up, draws);
    drawn_again(&mut renderer, &mut stores, viewport, up, draws);
    let size = [WIDTH, HEIGHT];
    let pick = Pick {
        tag: 1,
        pixel: pixel_of(viewpoint, size, scene.ball),
    };
    let data = RenderData {
        picks: &[pick],
        ..stores.data(viewport, viewpoint, draws)
    };
    render_without_ui(&mut renderer, data);
    let entered = frame_pixels(&mut renderer);
    // The pick, answered as the frame two after it begins.
    drawn_again(&mut renderer, &mut stores, viewport, viewpoint, draws);
    assert_eq!(renderer.picked(), [(1, BALL)], "the pick of the sphere");
    stores.ambient_occlusion = None;
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, draws));
    let lit = frame_pixels(&mut renderer);
    let shown = |frame: &[u8]| pixel_at(frame, WIDTH, pixel_of(viewpoint, size, scene.contact))[1];
    let (lit, darkened) = (i32::from(shown(&lit)), i32::from(shown(&entered)));
    assert!(
        darkened <= lit - 12,
        "where the floor meets the sphere, {darkened} as it comes into view, not 12 darker than \
         {lit} without ambient occlusion"
    );
    shut_down(renderer);
}

/// [`occlusion_scene`] under a sky of 0.5 every way, with ambient occlusion, in a still view: its
/// history averages each frame's noise with the frames' before, antialiased temporally or not,
/// rather than keep one noise every frame, which would stay where it is on the screen, a grain over
/// what's occluded. Grain is measured where the floor is darkened, away from its edges, as what's
/// left of the mean of the 3 by 3 pixels round each once the 9 by 9's is taken from it. Settled,
/// without antialiasing and antialiased, a frame keeps at most 0.8 of the grain of the first frame
/// with ambient occlusion, whose history holds nothing, and at least 0.4 of that grain is the first
/// frame's alone, left once the settled frame is taken from it. Before the history, one noise every
/// frame without antialiasing kept all of it and left none, and antialiased, each offset's own kept
/// 0.69 and left 0.63; the history keeps 0.68 and 0.61 and leaves 0.56 and 0.61, on an RTX 4090.
/// The floor is darkened as much either way, within a tenth.
fn ambient_occlusion_averages_its_noise_away() {
    const WIDTH: u32 = 768;
    const HEIGHT: u32 = 512;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.5; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let scene = occlusion_scene(&mut stores);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    let on = Some(AmbientOcclusion::default());
    // The last of `frames` frames, the first of which resolves nothing as antialiasing changes.
    let mut render = |ambient_occlusion, antialiasing, frames| {
        for _ in 0..frames {
            let data = RenderData {
                ambient_occlusion,
                antialiasing,
                ..stores.data(viewport, viewpoint, draws)
            };
            render_without_ui(&mut renderer, data);
        }
        frame_pixels(&mut renderer)
    };
    let lit = render(None, Antialiasing::None, 2);
    // The first frame since frames without ambient occlusion, whose history holds nothing.
    let still = render(on, Antialiasing::None, 1);
    let settled = render(on, Antialiasing::None, 64);
    // Three cycles of the offsets for each to draw each pixel alike, and more for the resolve to
    // settle on their mean, as 400 frames do no further.
    let antialiased = render(on, Antialiasing::Taa, 128);
    let [width, height] = [WIDTH as usize, HEIGHT as usize];
    let greens = |frame: &[u8]| -> Vec<f64> {
        (frame.as_chunks::<4>().0.iter())
            .map(|pixel| f64::from(pixel[1]))
            .collect()
    };
    // Each pixel's sum of `values` up and left of it, past a row and a column of 0s, so that a
    // box's sum is of its four corners.
    let summed = |values: &[f64]| {
        let row = width + 1;
        let mut sums = vec![0.0; row * (height + 1)];
        for y in 0..height {
            for x in 0..width {
                sums[(y + 1) * row + x + 1] =
                    values[y * width + x] + sums[(y + 1) * row + x] + sums[y * row + x + 1]
                        - sums[y * row + x];
            }
        }
        sums
    };
    // The mean of what `sums` sums over the pixels `reach` or less from `[x, y]` each way.
    let mean = |sums: &[f64], [x, y]: [usize; 2], reach: usize| {
        let row = width + 1;
        let [left, top, right, bottom] = [x - reach, y - reach, x + reach + 1, y + reach + 1];
        let side = (2 * reach + 1) as f64;
        (sums[bottom * row + right] - sums[bottom * row + left] - sums[top * row + right]
            + sums[top * row + left])
            / (side * side)
    };
    // The grain is what's left of the 3 by 3 pixels' mean round each once the 9 by 9's is taken
    // from it: of the noise's blotches a few pixels across, without the pixels' own rounding to a
    // byte, which the smaller box averages, or how the occlusion falls off, which both follow.
    const NEAR: usize = 1;
    const FAR: usize = 4;
    let lit = greens(&lit);
    let open = pixel_of(viewpoint, [WIDTH, HEIGHT], scene.open).map(|side| side as usize);
    let floor = lit[open[1] * width + open[0]];
    let off_floor: Vec<f64> = (lit.iter())
        .map(|&green| f64::from(u8::from((green - floor).abs() > 1.0)))
        .collect();
    let off_floor = summed(&off_floor);
    let (still, settled, antialiased) = (greens(&still), greens(&settled), greens(&antialiased));
    // The floor's pixels whose 9 by 9 is the floor's, and 2 more round it, as the resolve's 3 by 3
    // and its offsets blend what's beside an edge in; of those, where the occlusion darkens it.
    let margin = FAR + 2;
    let darkened: Vec<[usize; 2]> = (margin..height - margin)
        .flat_map(|y| (margin..width - margin).map(move |x| [x, y]))
        .filter(|&[x, y]| {
            mean(&off_floor, [x, y], margin) == 0.0 && still[y * width + x] <= floor - 6.0
        })
        .collect();
    assert!(
        darkened.len() >= 200,
        "{} of the floor's pixels darkened, too few to measure",
        darkened.len()
    );
    let grain = |frame: &[f64]| {
        let sums = summed(frame);
        let off = |&at: &[usize; 2]| (mean(&sums, at, NEAR) - mean(&sums, at, FAR)).abs();
        darkened.iter().map(off).sum::<f64>() / darkened.len() as f64
    };
    let darkness = |frame: &[f64]| {
        let by = |&[x, y]: &[usize; 2]| lit[y * width + x] - frame[y * width + x];
        darkened.iter().map(by).sum::<f64>() / darkened.len() as f64
    };
    let (still_grain, still_darkness) = (grain(&still), darkness(&still));
    for (name, settled) in [
        ("without antialiasing", &settled),
        ("antialiased", &antialiased),
    ] {
        let kept = grain(settled);
        let apart: Vec<f64> = still.iter().zip(settled).map(|(a, b)| a - b).collect();
        let left = grain(&apart);
        assert!(
            kept <= 0.8 * still_grain,
            "{name}, settled, the floor's grain is {kept:.3}, not 0.8 of the first frame's \
             {still_grain:.3}: the noise stays from frame to frame"
        );
        assert!(
            left >= 0.4 * still_grain,
            "{name}, {left:.3} of the first frame's grain of {still_grain:.3} is its alone, not \
             0.4 of it: the settled frame shares its noise"
        );
        let darkened = darkness(settled);
        assert!(
            (darkened - still_darkness).abs() <= 0.1 * still_darkness,
            "{name}, the floor darkened by {darkened:.2} settled, not near {still_darkness:.2} in \
             the first frame"
        );
    }
    shut_down(renderer);
}

/// [`floor_and_ball`]'s floor, and its sphere rolling across it under a still view from
/// [`FLOOR_ABOVE`] straight down, 1 and 3 pixels a frame for 48 frames: its last frame, without
/// antialiasing, against a view of the sphere resting where it ends up, once settled. Behind the
/// sphere, at most 400 pixels of the floor differ by more than 2 levels: the occlusion's history,
/// which follows the camera alone, is bounded by what each frame's own terms round each pixel range
/// over. Unbounded, 1,843 and 2,791 did on an RTX 4090, and bounded, 265 and 289; ahead of the
/// sphere, where the occlusion it brings comes a few frames late, 448 and 546, by at most 8 levels.
fn ambient_occlusion_leaves_no_trail_behind_what_moves() {
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 256;
    const SIZE: [u32; 2] = [WIDTH, HEIGHT];
    const FRAMES: usize = 48;
    let [width, height] = [WIDTH as usize, HEIGHT as usize];
    let unit = HEIGHT as f32 / (2.0 * FLOOR_ABOVE * math::libm::tanf(FLOOR_FOV / 2.0));
    let viewpoint = floor_seen(0, 1, SIZE);
    for speed in [1.0, 3.0] {
        // Ending in the middle of the view.
        let at = |frame: usize| {
            let x = (frame as f32 - FRAMES as f32) * speed / unit;
            Vec3::new(x, FLOOR_BALL, 0.0)
        };
        // The last of the frames from `first` to `last`, the sphere at `at` of each, but no further
        // than where it ends up.
        let run = |first: usize, last: usize| {
            let mut renderer = renderer(WIDTH, HEIGHT);
            let mut stores = Stores {
                ambient_occlusion: Some(AmbientOcclusion::default()),
                ..Stores::default()
            };
            let (floor, round) = floor_and_ball(&mut stores);
            let draws = |frame: usize| floor_with_balls(&floor, &round, [at(frame.min(FRAMES))]);
            let viewport = filling(SIZE);
            settle(
                &mut renderer,
                &mut stores,
                viewport,
                viewpoint,
                &draws(first),
            );
            for frame in first..=last {
                let drawn = draws(frame);
                render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &drawn));
            }
            let pixels = frame_pixels(&mut renderer);
            shut_down(renderer);
            (pixels.as_chunks::<4>().0.iter())
                .map(|pixel| f64::from(pixel[1]))
                .collect::<Vec<f64>>()
        };
        let (rolled, rested) = (run(0, FRAMES), run(FRAMES, FRAMES + 64));
        // Of the floor behind the sphere, the half of the view it came from, 4 pixels or more from
        // it, the pixels that differ by more than 2 levels.
        let ball = floor_balls_shown(viewpoint, SIZE, [at(FRAMES)].into_iter(), 4);
        let behind = (0..height)
            .flat_map(|y| (0..width / 2).map(move |x| y * width + x))
            .filter(|&at| !ball[at] && (rolled[at] - rested[at]).abs() > 2.0)
            .count();
        assert!(
            behind <= 400,
            "rolling {speed} pixels a frame, {behind} of the floor's pixels behind the sphere \
             differ by more than 2 levels from where it rests, not at most 400: its occlusion \
             leaves a trail"
        );
    }
}

/// How high above [`occlusion_floor`] the view looks straight down at it, and its field of view.
const FLOOR_ABOVE: f32 = 3.0;
const FLOOR_FOV: f32 = PI / 3.0;
/// The radius of [`occlusion_floor`]'s spheres, and how far apart they stand.
const FLOOR_BALL: f32 = 0.25;
const FLOOR_BALLS_APART: f32 = 0.8;

/// What draws a floor of a gray of 0.8 and a sphere of [`FLOOR_BALL`] of it, under a sky of 0.5
/// every way, which the stores light by.
fn floor_and_ball(stores: &mut Stores) -> (MeshDraw, MeshDraw) {
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.5; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let model = model_square(&mut stores.meshes);
    let ball = sphere(&mut stores.meshes, 1.0, 24, 48);
    let gray = Material {
        color: Vec4::new(0.8, 0.8, 0.8, 1.0),
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let gray = stores.materials.add(&gray);
    let (materials, shaders) = (&stores.materials, &mut stores.shaders);
    let floor = MeshDraw::with_material(model, gray, materials, shaders);
    let round = MeshDraw::with_material(ball, gray, materials, shaders);
    (floor, round)
}

/// [`floor_and_ball`]'s floor, `floor`, and its sphere, `round`, standing on it centered at `at`.
fn floor_with_balls(
    floor: &MeshDraw,
    round: &MeshDraw,
    at: impl IntoIterator<Item = Vec3>,
) -> Draws {
    let mut draws = Draws::default();
    draws.push_draw(facing_up(40.0, Vec3::ZERO), floor, 0);
    for at in at {
        let scale = Vec3::splat(FLOOR_BALL);
        let placed = Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, at);
        draws.push_draw(placed, round, 0);
    }
    draws
}

/// Where [`occlusion_floor`]'s spheres' centers are.
fn floor_balls() -> impl Iterator<Item = Vec3> + Clone {
    (-8..=8).flat_map(|x| {
        (-3..=3).map(move |z| {
            Vec3::new(x as f32, 0.0, z as f32) * FLOOR_BALLS_APART + Vec3::Y * FLOOR_BALL
        })
    })
}

/// [`floor_and_ball`]'s floor, with its spheres standing on it [`FLOOR_BALLS_APART`] apart, in rows
/// along X and Z, each darkening the floor round it.
fn occlusion_floor(stores: &mut Stores) -> Draws {
    let (floor, round) = floor_and_ball(stores);
    floor_with_balls(&floor, &round, floor_balls())
}

/// [`occlusion_floor`] seen from [`FLOOR_ABOVE`] straight down, panning along X so that the floor
/// moves a pixel to the left every `over` frames in a view `size` pixels: where frame `frame` sees
/// from.
fn floor_seen(frame: usize, over: usize, size: [u32; 2]) -> Viewpoint {
    let unit = size[1] as f32 / (2.0 * FLOOR_ABOVE * math::libm::tanf(FLOOR_FOV / 2.0));
    Viewpoint {
        position: Vec3::new(frame as f32 / (over as f32 * unit), FLOOR_ABOVE, 0.0).as_dvec3(),
        rotation: Quat::from_rotation_x(-FRAC_PI_2),
        projection: Projection::Perspective {
            fov: FLOOR_FOV,
            near: 0.1,
        },
    }
}

/// Which pixels of a view `size` pixels that `viewpoint`, [`floor_seen`]'s, sees from show a sphere
/// of [`FLOOR_BALL`] centered at one of `centers`, or come within `margin` pixels of one, row after
/// row.
fn floor_balls_shown(
    viewpoint: Viewpoint,
    size: [u32; 2],
    centers: impl Iterator<Item = Vec3> + Clone,
    margin: usize,
) -> Vec<bool> {
    let [width, height] = size.map(|side| side as f32);
    let tan = math::libm::tanf(FLOOR_FOV / 2.0);
    let hits = |x: u32, y: u32| {
        let ndc = Vec2::new(
            (x as f32 + 0.5) / width * 2.0 - 1.0,
            (y as f32 + 0.5) / height * 2.0 - 1.0,
        );
        // Looking down -Y, the view's up along -Z: its pixels' rows run down along +Z.
        let way = Vec3::new(ndc.x * tan * width / height, -1.0, ndc.y * tan).normalize();
        centers.clone().any(|center| {
            let to = center - viewpoint.position.as_vec3();
            let along = to.dot(way);
            (to - way * along).length_squared() <= FLOOR_BALL * FLOOR_BALL
        })
    };
    let shows: Vec<bool> = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| [x, y]))
        .map(|[x, y]| hits(x, y))
        .collect();
    // Widened by `margin` each way, across and then down.
    let [width, height] = size.map(|side| side as usize);
    let near =
        |line: usize, length: usize| line.saturating_sub(margin)..(line + margin + 1).min(length);
    let across: Vec<bool> = (0..width * height)
        .map(|at| near(at % width, width).any(|x| shows[at - at % width + x]))
        .collect();
    (0..width * height)
        .map(|at| near(at / width, height).any(|y| across[y * width + at % width]))
        .collect()
}

/// [`occlusion_floor`] as [`floor_seen`] pans across it, antialiased temporally and not, a pixel a
/// frame and a pixel every 8: between frames 8 pixels of panning apart, at the same offset of
/// antialiasing's 8, the points of the floor that ambient occlusion darkens, where each shows in
/// each, change by at most 0.35 of a level on average, as only noise that doesn't move with the
/// floor changes them: without ambient occlusion, they're the same. Noise that stayed on the screen
/// as the view moved under it, the same every frame without antialiasing and each offset's own
/// under it, as before the occlusion had a history, changed them by 1.11 and 1.07 without
/// antialiasing and 0.50 and 0.55 antialiased, panning a pixel a frame and every 8, on an RTX 4090;
/// the history leaves 0.24 and 0.27, and 0.21 and 0.24.
fn ambient_occlusion_holds_to_the_world_as_the_view_pans() {
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 256;
    const SIZE: [u32; 2] = [WIDTH, HEIGHT];
    // The pixels a point moves between the frames compared.
    const MOVED: usize = 8;
    // How far the occlusion reaches round a point of the floor, in pixels: its default radius times
    // XeGTAO's 1.457, 0.73 of the world's units, 54 pixels in this view. XeGTAO takes what's past
    // the view's edges as the edges' own pixels, so that what occludes a point within its reach of
    // one changes as the view pans: in a slow pan, by 4 to 10 levels on average within 32 pixels of
    // the view's sides, against a few tenths away from them, with the history or without.
    const REACH: usize = 56;
    let [width, height] = [WIDTH as usize, HEIGHT as usize];
    let greens = |frame: &[u8]| -> Vec<f64> {
        (frame.as_chunks::<4>().0.iter())
            .map(|pixel| f64::from(pixel[1]))
            .collect()
    };
    for antialiasing in [Antialiasing::None, Antialiasing::Taa] {
        for over in [1, 8] {
            let apart = MOVED * over;
            // From frame 96, so that every history has settled, four pairs a cycle of the offsets
            // apart.
            let firsts = [96, 104, 112, 120];
            let compared: Vec<usize> = (firsts.iter())
                .flat_map(|&first| [first, first + apart])
                .collect();
            let mut renderer = renderer(WIDTH, HEIGHT);
            let mut stores = Stores::default();
            let draws = occlusion_floor(&mut stores);
            let viewport = filling(SIZE);
            settle(
                &mut renderer,
                &mut stores,
                viewport,
                floor_seen(0, over, SIZE),
                &draws,
            );
            let mut shown = std::collections::HashMap::new();
            for frame in 0..=firsts[3] + apart {
                let data = RenderData {
                    ambient_occlusion: Some(AmbientOcclusion::default()),
                    antialiasing,
                    ..stores.data(viewport, floor_seen(frame, over, SIZE), &draws)
                };
                render_without_ui(&mut renderer, data);
                if compared.contains(&frame) {
                    shown.insert(frame, greens(&frame_pixels(&mut renderer)));
                }
            }
            // Without ambient occlusion or antialiasing, which carry nothing from frame to frame.
            let mut lit = std::collections::HashMap::new();
            for &frame in &compared {
                let data = stores.data(viewport, floor_seen(frame, over, SIZE), &draws);
                render_without_ui(&mut renderer, data);
                lit.insert(frame, greens(&frame_pixels(&mut renderer)));
            }
            shut_down(renderer);
            let (mut points, mut changed, mut lit_changed) = (0usize, 0.0, 0usize);
            for &first in &firsts {
                // Clear of the spheres by 6 pixels, so that their edges, and how the offsets draw
                // them, don't count; past the occlusion's reach of the view's edges in both
                // frames; shown for 48 frames, entering from the right; and darkened.
                let viewpoint = floor_seen(first, over, SIZE);
                let ball = floor_balls_shown(viewpoint, SIZE, floor_balls(), 6);
                let right = (width - REACH).min(width - 2 - 48 / over);
                let (lit_first, shown_first) = (&lit[&first], &shown[&first]);
                let (lit_then, shown_then) = (&lit[&(first + apart)], &shown[&(first + apart)]);
                for y in REACH..height - REACH {
                    for x in MOVED + REACH..right {
                        let at = y * width + x;
                        if shown_first[at] > lit_first[at] - 4.0 || ball[at] {
                            continue;
                        }
                        let then = at - MOVED;
                        points += 1;
                        changed += (shown_first[at] - shown_then[then]).abs();
                        lit_changed += usize::from(lit_first[at] != lit_then[then]);
                    }
                }
            }
            let case = format!("{antialiasing:?}, panning a pixel every {over} frames");
            assert!(
                points >= 10_000,
                "{case}, {points} points darkened, too few"
            );
            assert_eq!(
                lit_changed, 0,
                "{case}, points change without ambient occlusion: the view doesn't pan by whole \
                 pixels"
            );
            let changed = changed / points as f64;
            assert!(
                changed <= 0.35,
                "{case}, the floor's darkened points change by {changed:.3} on average as the view \
                 pans 8 pixels, not at most 0.35: the occlusion's noise stays on the screen"
            );
        }
    }
}

/// [`occlusion_scene`] lit by a sun alone, casting shadows, under a sky that's black every way,
/// renders the same with ambient occlusion as without: it darkens the sky's light alone, and never
/// the lights'. And none of its passes run, nor take any memory, in a frame without a sky, under
/// an orthographic projection, or with a strength of 0, but with a sky, under perspective, they do.
fn ambient_occlusion_leaves_the_lights_alone() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.0; 3]);
    let sky = Sky {
        texture: sky,
        luminance: 1.0,
    };
    stores.sky = Some(sky);
    stores.lights = vec![Light {
        color: Vec3::splat(2.0),
        ..sun(Vec3::new(1.0, -2.0, -1.0), Some(20.0))
    }];
    let scene = occlusion_scene(&mut stores);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    let on = Some(AmbientOcclusion::default());
    // A frame's, and what the graph's own images and buffers take.
    let frame = |renderer: &mut Renderer,
                 stores: &mut Stores,
                 ambient_occlusion: Option<AmbientOcclusion>,
                 sky: Option<Sky>,
                 viewpoint: Viewpoint| {
        let data = RenderData {
            ambient_occlusion,
            sky,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(renderer, data);
        renderer.transient_bytes()
    };
    let without = frame(&mut renderer, &mut stores, None, Some(sky), viewpoint);
    let lit = frame_pixels(&mut renderer);
    let above = looking_down(Vec3::new(0.0, 6.0, 0.0), 6.0, 10.0);
    let weak = Some(AmbientOcclusion {
        strength: 0.0,
        ..AmbientOcclusion::default()
    });
    for (name, ambient_occlusion, sky, viewpoint) in [
        ("without a sky", on, None, viewpoint),
        ("under an orthographic projection", on, Some(sky), above),
        ("of a strength of 0", weak, Some(sky), viewpoint),
    ] {
        let bytes = frame(
            &mut renderer,
            &mut stores,
            ambient_occlusion,
            sky,
            viewpoint,
        );
        assert_eq!(bytes, without, "ambient occlusion takes memory {name}");
    }
    let with = frame(&mut renderer, &mut stores, on, Some(sky), viewpoint);
    assert!(
        with > without,
        "ambient occlusion takes no memory, {with} bytes"
    );
    let occluded = frame_pixels(&mut renderer);
    let differ = (lit.iter().zip(&occluded)).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "ambient occlusion changes the lights' light");
    shut_down(renderer);
}

/// The scene the tests of reflections see, under a sky of a gradient from a blue zenith to a pale
/// horizon over dark ground, of a luminance of 1, as `viewpoint` sees it, a camera with perspective
/// low in front of it: a floor, models' squares, of a metal of 0.9 gray, in three strips along Z
/// 2.4 units across each, from -3.6 along X, rough `roughness` each, from Z of 3 to -5; a red sphere
/// of 0.5 on the first, a green one on the second and a blue panel a unit across and 1.6 high
/// standing on the third, facing the camera; the strips normal-mapped by `normals`, if given. And
/// the world's points where the red sphere's middle is, mirrored in the floor, and a point of the
/// first strip whose mirror shows the sky alone.
struct ReflectionScene {
    draws: Draws,
    viewpoint: Viewpoint,
    mirrored: Vec3,
    open: Vec3,
}

fn reflection_scene(
    stores: &mut Stores,
    roughness: [f32; 3],
    normals: Option<Sampled>,
) -> ReflectionScene {
    let [width, height] = [128, 64];
    let sky = sky_texture(&mut stores.textures, [width, height], |_, y| {
        let up = 1.0 - 2.0 * (y as f32 + 0.5) / height as f32;
        if up > 0.0 {
            let pale = 1.0 - up;
            [0.25 + 0.45 * pale, 0.4 + 0.35 * pale, 0.9 - 0.05 * pale]
        } else {
            [0.12, 0.09, 0.06]
        }
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let square = model_square(&mut stores.meshes);
    let ball = sphere(&mut stores.meshes, 0.5, 24, 48);
    let add = |materials: &mut Materials, color: Vec4, workflow| {
        materials.add(&Material {
            color,
            workflow,
            ..Material::default()
        })
    };
    let mut draws = Draws::default();
    let turned = Quat::from_rotation_x(-FRAC_PI_2);
    for (i, roughness) in (0..).zip(roughness) {
        let floor = stores.materials.add(&Material {
            color: Vec4::new(0.9, 0.9, 0.9, 1.0),
            workflow: metallic(1.0, roughness),
            normal_texture: normals,
            ..Material::default()
        });
        let floor = MeshDraw::with_material(square, floor, &stores.materials, &mut stores.shaders);
        let at = Vec3::new(-2.4 + 2.4 * i as f32, 0.0, -1.0);
        let strip = Mat4::from_scale_rotation_translation(Vec3::new(2.4, 8.0, 1.0), turned, at);
        draws.push_draw(strip, &floor, 0);
    }
    let shapes = [
        (Vec4::new(0.8, 0.1, 0.1, 1.0), Vec3::new(-2.4, 0.5, -1.0)),
        (Vec4::new(0.1, 0.7, 0.15, 1.0), Vec3::new(0.0, 0.5, -1.5)),
    ];
    for (color, at) in shapes {
        let material = add(&mut stores.materials, color, metallic(0.0, 0.4));
        let draw = MeshDraw::with_material(ball, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(Mat4::from_translation(at), &draw, 0);
    }
    let blue = add(
        &mut stores.materials,
        Vec4::new(0.1, 0.2, 0.85, 1.0),
        metallic(0.0, 0.5),
    );
    let panel = MeshDraw::with_material(square, blue, &stores.materials, &mut stores.shaders);
    let standing = Mat4::from_scale_rotation_translation(
        Vec3::new(1.0, 1.6, 1.0),
        Quat::IDENTITY,
        Vec3::new(2.4, 0.8, -1.2),
    );
    draws.push_draw(standing, &panel, 0);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 1.2, 4.0),
        rotation: Quat::from_rotation_x(-0.2),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
    };
    ReflectionScene {
        draws,
        viewpoint,
        mirrored: Vec3::new(-2.4, -0.5, -1.0),
        open: Vec3::new(-1.6, 0.0, 1.5),
    }
}

/// [`reflection_scene`], its floor's strips mirror-smooth, glossy and less so, under its sky alone,
/// with reflections and without. A frame with reflections after one without has no colors kept to
/// trace against, and renders as one without; the frame after shows, where the red sphere's middle
/// is mirrored in the floor, the red sphere, where a frame without shows the sky's light; and where
/// the floor's mirror shows the sky alone, the sky's light as a frame without does. Against its
/// reference image, without antialiasing; and antialiased, temporally and by SMAA, whose blended
/// colors the frames keep in place of the resolve's, the frames after still mirror the sphere, as
/// they do once the camera moves past what SMAA's last frame kept.
fn reflections_mirror_what_the_view_shows() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = reflection_scene(&mut stores, [0.05, 0.2, 0.4], None);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, draws));
    let without = frame_pixels(&mut renderer);
    let on = Some(Reflections::default());
    let reflecting = |renderer: &mut Renderer, stores: &mut Stores, antialiasing| {
        let data = RenderData {
            reflections: on,
            antialiasing,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(renderer, data);
        frame_pixels(renderer)
    };
    let first = reflecting(&mut renderer, &mut stores, Antialiasing::None);
    assert!(
        first == without,
        "a frame without colors kept to trace against renders as one without reflections"
    );
    let with = reflecting(&mut renderer, &mut stores, Antialiasing::None);
    let shown = |frame: &[u8], at: Vec3| {
        pixel_at(frame, WIDTH, pixel_of(viewpoint, [WIDTH, HEIGHT], at)).map(i32::from)
    };
    let [r, g, b, _] = shown(&with, scene.mirrored);
    assert!(
        r > g + 40 && r > b + 40,
        "the mirrored sphere shows [{r}, {g}, {b}], not red"
    );
    let [r, _, b, _] = shown(&without, scene.mirrored);
    assert!(
        b > r,
        "without reflections, the mirror shows [{r}, _, {b}], not the sky"
    );
    let (open, sky) = (shown(&with, scene.open), shown(&without, scene.open));
    let near = open.iter().zip(sky).all(|(a, b)| (a - b).abs() <= 2);
    assert!(
        near,
        "the sky's mirror shows {open:?} with reflections, not {sky:?}"
    );
    compare("reflections", WIDTH, HEIGHT, &with);
    for _ in 0..12 {
        reflecting(&mut renderer, &mut stores, Antialiasing::Taa);
    }
    let resolved = frame_pixels(&mut renderer);
    let [r, g, b, _] = shown(&resolved, scene.mirrored);
    assert!(
        r > g + 40 && r > b + 40,
        "antialiased, the mirrored sphere shows [{r}, {g}, {b}], not red"
    );
    let mut blended = Vec::new();
    for _ in 0..3 {
        blended = reflecting(&mut renderer, &mut stores, Antialiasing::Smaa);
    }
    let [r, g, b, _] = shown(&blended, scene.mirrored);
    assert!(
        r > g + 40 && r > b + 40,
        "antialiased by SMAA, the mirrored sphere shows [{r}, {g}, {b}], not red"
    );
    // The camera moved since the frame before, farther than the sphere is wide on screen: what a
    // ray finds is taken from where it showed then.
    let moved = Viewpoint {
        position: viewpoint.position + DVec3::new(0.6, 0.0, 0.0),
        ..viewpoint
    };
    let data = RenderData {
        reflections: on,
        ..stores.data(viewport, moved, draws)
    };
    render_without_ui(&mut renderer, data);
    let after = frame_pixels(&mut renderer);
    let pixel = pixel_of(moved, [WIDTH, HEIGHT], scene.mirrored);
    let [r, g, b, _] = pixel_at(&after, WIDTH, pixel).map(i32::from);
    assert!(
        r > g + 40 && r > b + 40,
        "the camera moved, the mirrored sphere shows [{r}, {g}, {b}], not red"
    );
    // The sky twice as bright and the exposure an eighth: the view's light a quarter of what it
    // was, the mirrored sphere's too, as reflections take the colors kept, exposed, as they are.
    let sky = stores.sky.expect("the scene's sky");
    stores.sky = Some(Sky {
        luminance: 2.0,
        ..sky
    });
    for _ in 0..2 {
        let data = RenderData {
            reflections: on,
            exposure: 0.125,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(&mut renderer, data);
    }
    let quartered = frame_pixels(&mut renderer);
    let mirrored = pixel_of(viewpoint, [WIDTH, HEIGHT], scene.mirrored);
    let (was, now) = (
        pixel_at(&with, WIDTH, mirrored),
        pixel_at(&quartered, WIDTH, mirrored),
    );
    for channel in 0..3 {
        let [was, now] = [was[channel], now[channel]].map(decoded);
        assert!(
            (now - was / 4.0).abs() < 0.005,
            "a quarter of the light, the mirrored sphere's channel {channel} is {now}, not a \
             quarter of {was}"
        );
    }
    shut_down(renderer);
}

/// [`reflection_scene`] reflecting what it shows, its exposure adapting as fast as can be from 1,
/// at which the frames before it were drawn and their colors kept: its third frame takes the first
/// metering at once, about 2 stops darker, and shows the mirrored sphere as the frames settled after
/// it do, the colors the frame before kept scaled by the exposure's change, rather than as bright
/// as the frame before kept it.
fn reflections_follow_an_exposures_jump() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = reflection_scene(&mut stores, [0.05, 0.2, 0.4], None);
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    let instant = Adaptation {
        speed_up: 1000.0,
        speed_down: 1000.0,
        ..Adaptation::default()
    };
    let mirrored = pixel_of(viewpoint, [WIDTH, HEIGHT], scene.mirrored);
    let mut time = 0.0;
    let mut frame = |stores: &mut Stores, adaptation| {
        time += 0.1;
        let data = RenderData {
            reflections: Some(Reflections::default()),
            adaptation,
            time,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(&mut renderer, data);
        let pixel = pixel_at(&frame_pixels(&mut renderer), WIDTH, mirrored);
        (renderer.exposure(), pixel.map(decoded))
    };
    for _ in 0..2 {
        frame(&mut stores, None);
    }
    let shown: Vec<_> = (0..6).map(|_| frame(&mut stores, Some(instant))).collect();
    let (before, jumped, settled) = (shown[1], shown[2], shown[5]);
    assert!(
        jumped.0 < before.0 / 2.0 && (jumped.0 / settled.0 - 1.0).abs() < 0.01,
        "the exposures {:?}",
        shown.iter().map(|frame| frame.0).collect::<Vec<_>>()
    );
    let off = (jumped.1.iter().zip(settled.1)).map(|(a, b)| (a - b).abs());
    assert!(
        off.clone().all(|off| off < 0.005),
        "the mirrored sphere shows {:?} as the exposure jumps, not {:?}",
        jumped.1,
        settled.1
    );
    shut_down(renderer);
}

/// [`reflection_scene`] under a sky of a luminance of 0, as an atmosphere's is once its sun has set,
/// lit by a sun alone, exposed at a half: the floor's mirror shows the red sphere, lit, though the
/// sky's specular light that reflections take the place of is none, where a frame without
/// reflections shows next to nothing.
fn reflections_mirror_what_lights_light_under_a_dark_sky() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = reflection_scene(&mut stores, [0.05, 0.2, 0.4], None);
    let sky = stores.sky.expect("the scene's sky");
    stores.sky = Some(Sky {
        luminance: 0.0,
        ..sky
    });
    // From above and behind the camera, onto the spheres' fronts.
    stores.lights = vec![Light {
        color: Vec3::splat(6.0),
        ..sun(Vec3::new(0.3, -1.0, -0.6), None)
    }];
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    let mut frame = |reflections| {
        let data = RenderData {
            reflections,
            exposure: 0.5,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(&mut renderer, data);
        frame_pixels(&mut renderer)
    };
    let without = frame(None);
    // The first frame keeps the colors the second traces against.
    frame(Some(Reflections::default()));
    let with = frame(Some(Reflections::default()));
    let pixel = pixel_of(viewpoint, [WIDTH, HEIGHT], scene.mirrored);
    let [r, g, b, _] = pixel_at(&with, WIDTH, pixel).map(i32::from);
    assert!(
        r > g + 40 && r > b + 40,
        "under a dark sky, the mirrored sphere shows [{r}, {g}, {b}], not red"
    );
    let [r, g, b, _] = pixel_at(&without, WIDTH, pixel);
    assert!(
        r.max(g).max(b) < 24,
        "without reflections, the mirror under a dark sky shows [{r}, {g}, {b}]"
    );
    shut_down(renderer);
}

/// [`reflection_scene`] reflecting what it shows renders as it does without reflections: every
/// surface of it rougher than reflections' roughness, its floor's strips 0.8 and its shapes 0.4
/// and 0.5 against 0.3, which take the sky's light alone; under an orthographic projection, which
/// reflections don't take; and without a sky, whose specular light they'd take the place of.
fn reflections_leave_what_they_dont_take_as_it_is() {
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 64;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = reflection_scene(&mut stores, [0.8; 3], None);
    let reflections = Reflections {
        roughness: 0.3,
        ..Reflections::default()
    };
    let draws = &scene.draws;
    let viewport = filling([WIDTH, HEIGHT]);
    let sky = stores.sky;
    settle(&mut renderer, &mut stores, viewport, scene.viewpoint, draws);
    let above = looking_down(Vec3::new(0.0, 6.0, 0.0), 8.0, 10.0);
    for (name, viewpoint, sky) in [
        (
            "on a floor rougher than their roughness",
            scene.viewpoint,
            sky,
        ),
        ("under an orthographic projection", above, sky),
        ("without a sky", scene.viewpoint, None),
    ] {
        let mut frame = |reflections| {
            let data = RenderData {
                reflections,
                sky,
                ..stores.data(viewport, viewpoint, draws)
            };
            render_without_ui(&mut renderer, data);
            frame_pixels(&mut renderer)
        };
        let without = frame(None);
        // The first frame keeps the colors the second traces against.
        frame(Some(reflections));
        let with = frame(Some(reflections));
        assert!(with == without, "reflections change the frame {name}");
    }
    shut_down(renderer);
}

/// [`reflection_scene`], its floor's strips mirror-smooth and normal-mapped to face about 12°
/// toward the camera, each way the view draws them: in the run by meshlets, by their meshlets, into
/// the view's visibility buffer, their normals found from it; and otherwise by their indices, their
/// normals drawn after their depth alone, alone and again beside a ball cut into meshlets, which the
/// visibility buffer takes. Where the mirror of the map's normal shows the red sphere's middle, the
/// floor shows the red sphere, which it doesn't without reflections; and where a flat mirror would
/// show it, it doesn't, as the rays are traced about the shading's normal, its normal map's among
/// it, and not about the depth's, which is the flat floor's.
fn reflections_mirror_about_the_shading_normal() {
    mirror_about_the_shading_normal(false);
    if !CUT_MESHLETS.load(Ordering::Relaxed) {
        mirror_about_the_shading_normal(true);
    }
}

/// [`reflections_mirror_about_the_shading_normal`]'s check, its models cut as [`add_model`] cuts
/// them, and, `beside_cut`, a ball cut into meshlets, high and to the right of what it checks.
fn mirror_about_the_shading_normal(beside_cut: bool) {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 128;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // A Y of 100/255, as a normal map holds it, about -0.22: toward the square's -Y.
    let pixels = [[128, 100, 255, 255]; 16];
    let texels = Texels::Map(Map::Normal);
    let added = (stores.textures).add("tilted", [4, 4], pixels.as_flattened(), texels);
    let map = Sampled::of(added.expect("a map"));
    let mut scene = reflection_scene(&mut stores, [0.05; 3], Some(map));
    if beside_cut {
        let (vertices, indices) = sphere_of(0.4, 12, 24);
        let packed = ModelMesh::pack("cut ball", &vertices, indices, Vec::new());
        let mut packed = packed.expect("a mesh");
        cut_meshlets(&mut packed);
        let ball = stores
            .meshes
            .add_model("cut ball", &packed)
            .expect("a mesh");
        let white = stores.materials.add(&Material::default());
        let ball = MeshDraw::with_material(ball, white, &stores.materials, &mut stores.shaders);
        let high = Mat4::from_translation(Vec3::new(1.2, 2.0, -3.0));
        scene.draws.push_draw(high, &ball, 0);
    }
    let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
    let mut frame = |reflections| {
        let data = RenderData {
            reflections,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(&mut renderer, data);
        frame_pixels(&mut renderer)
    };
    let without = frame(None);
    // The first frame keeps the colors the second traces against.
    frame(Some(Reflections::default()));
    let with = frame(Some(Reflections::default()));
    // The map's normal in the world, as shading.slang's `shading_normal` bends the floor's, +Y, along
    // its tangent, +X, and its bitangent, the square's +Y, -Z.
    let mapped = Vec2::new(128.0, 100.0) / 255.0 * 2.0 - 1.0;
    let up = (1.0 - mapped.length_squared()).sqrt();
    let tilted = (Vec3::X * mapped.x - Vec3::Z * mapped.y + Vec3::Y * up).normalize();
    // The point of the first strip whose mirror, facing `normal`, shows the red sphere's middle:
    // whose ray from the camera, mirrored, passes nearest it.
    let middle = Vec3::new(-2.4, 0.5, -1.0);
    let mirror = |normal: Vec3| {
        let mut nearest = (f32::INFINITY, Vec3::ZERO);
        for i in 0..=240 {
            for j in 0..=400 {
                let at = Vec3::new(-3.6 + 0.01 * i as f32, 0.0, -1.0 + 0.01 * j as f32);
                let way = (at - viewpoint.position.as_vec3()).normalize();
                let ray = way - 2.0 * way.dot(normal) * normal;
                let along = (middle - at).dot(ray).max(0.0);
                let apart = (at + ray * along - middle).length();
                if apart < nearest.0 {
                    nearest = (apart, at);
                }
            }
        }
        nearest.1
    };
    let shown = |frame: &[u8], at: Vec3| {
        pixel_at(frame, WIDTH, pixel_of(viewpoint, [WIDTH, HEIGHT], at)).map(i32::from)
    };
    let red = |[r, g, b, _]: [i32; 4]| r > g + 40 && r > b + 40;
    let [bent, flat] = [tilted, Vec3::Y].map(mirror);
    let [seen, unseen] = [shown(&with, bent), shown(&without, bent)];
    assert!(
        red(seen) && !red(unseen),
        "where the map's normal mirrors the red sphere, the floor shows {seen:?}, and without \
         reflections {unseen:?}"
    );
    let seen = shown(&with, flat);
    assert!(
        !red(seen),
        "where a flat mirror shows the red sphere, the floor shows {seen:?}"
    );
    shut_down(renderer);
}

/// Squares of a white metal, smooth, and of a white dielectric, rough, whose occlusion maps hold a
/// half, under a sky of 0.5 every way, without perspective: the sky's diffuse light darkened by the
/// map's half, and its specular light by Lagarde and de Rousiers' specular occlusion of it, of the
/// way to the eye and the roughness. The rough one, whose lobe gathers from all around as the
/// diffuse does, shows the sky's light darkened by a half; the smooth metal, head-on, less
/// darkened, and turned to face the view at a grazing angle, more. And a smooth white dielectric
/// whose map holds a half, head-on, shows its diffuse light darkened by the half and its specular
/// light by the specular occlusion, each by its share: the share it reflects, `f`, which a smooth
/// black dielectric without a map shows, diffusing nothing, as `f` of the sky's light.
fn specular_occlusion_dims_the_skys_reflection_by_the_way_to_the_eye() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 5;
    const WIDTH: u32 = SIDE * SQUARES;
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let sky = sky_texture(&mut stores.textures, [128, 64], |_, _| [2.0; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 0.25,
    });
    let half = stores.textures.add(
        "occlusion",
        [4, 4],
        [[128, 255, 255, 255]; 16].as_flattened(),
        Texels::Map(Map::Surface),
    );
    let half = Sampled::of(half.expect("an occlusion map"));
    let square = model_square(&mut stores.meshes);
    let occluded = |workflow| Material {
        color: Vec4::ONE,
        workflow,
        occlusion_texture: half,
        ..Material::default()
    };
    // Turned about X to face the view at a cosine of a quarter, its height on screen a quarter.
    let grazing = 0.25_f32;
    let turned = Quat::from_rotation_x(math::libm::acosf(grazing));
    let black = Material {
        color: Vec4::new(0.0, 0.0, 0.0, 1.0),
        workflow: metallic(0.0, 0.0),
        ..Material::default()
    };
    let squares = [
        (occluded(metallic(0.0, 1.0)), Quat::IDENTITY),
        (occluded(metallic(1.0, 0.0)), Quat::IDENTITY),
        (occluded(metallic(1.0, 0.0)), turned),
        (black, Quat::IDENTITY),
        (occluded(metallic(0.0, 0.0)), Quat::IDENTITY),
    ];
    let mut draws = Draws::default();
    for (i, (material, rotation)) in (0..).zip(&squares) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        let at = Vec3::new(i as f32 - (SQUARES - 1) as f32 / 2.0, 0.0, 0.0);
        draws.push_draw(Mat4::from_rotation_translation(*rotation, at), &draw, 0);
    }
    let viewport = filling([WIDTH, SIDE]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let frame = frame_pixels(&mut renderer);
    // The shader's, in f64: a smooth surface is as rough as the least roughness shading takes.
    let occlusion = 128.0 / 255.0;
    let specular = |n_dot_v: f64, roughness: f64| {
        let alpha = roughness * roughness;
        let power = math::libm::exp2(-16.0 * alpha - 1.0);
        (math::libm::pow(n_dot_v + occlusion, power) - 1.0 + occlusion).clamp(0.0, 1.0)
    };
    let pixel = |i: u32| pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2]);
    // What a dielectric reflects head-on of the sky's light, which the black square shows of its
    // 0.5: a few hundredths, Fresnel's 0.04 and its scattering many times over.
    let f = f64::from(decoded(pixel(3)[0])) / 0.5;
    assert!(
        (0.03..0.06).contains(&f),
        "a dielectric reflects {f} head-on"
    );
    let head_on = specular(1.0, 0.045);
    let expected = [
        occlusion,
        head_on,
        specular(f64::from(grazing), 0.045),
        f,
        (1.0 - f) * occlusion + f * head_on,
    ];
    assert!(expected[1] > occlusion + 0.15 && expected[2] < occlusion - 0.1);
    for (i, share) in (0..).zip(expected) {
        let value = encoded(0.5 * share as f32);
        let got = pixel(i);
        let near = got
            .iter()
            .zip([value, value, value, 255])
            .all(|(&a, b)| a.abs_diff(b) <= 2);
        assert!(
            near,
            "square {i} shows {got:?}, not 0.5 times {share} encoded, {value}"
        );
    }
    shut_down(renderer);
}

/// Squares lit by a sky and a light, each showing in place of its light a part of what shading
/// takes, as its material holds it, its values as their bytes over 255, through a frame that would
/// otherwise antialias, bloom and vignette them; or an image the frame makes, black where nothing
/// drew, and wholly black for an image it doesn't make. The sky doesn't show behind them. A frame
/// of their light, after, shows as one before, each channel within 1: it's drawn at antialiasing's
/// next offset, as the frames that showed parts resolved none, and where meshlets draw, lavapipe
/// rounds some pixels otherwise at another offset.
fn views_show_parts_of_shading_and_images_in_place_of_light() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 3;
    const WIDTH: u32 = SIDE * SQUARES;
    const HEIGHT: u32 = 2 * SIDE;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 2.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let sky = sky_texture(&mut stores.textures, [128, 64], |_, _| [1.0; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 0.25,
    });
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::new(0.0, -1.0, -1.0).normalize(),
        color: Vec3::splat(1.0),
        range: None,
        shadow_distance: None,
    }];
    let color = Vec4::new(0.5, 0.25, 1.0, 1.0);
    let squares = [
        Material {
            color,
            workflow: metallic(0.25, 0.75),
            emissive: Vec3::new(0.5, 0.0, 0.0),
            ..Material::default()
        },
        Material {
            color,
            unlit: true,
            ..Material::default()
        },
        Material {
            color,
            workflow: Workflow::SpecularGlossiness {
                specular: Vec3::splat(0.04),
                glossiness: 0.5,
            },
            ..Material::default()
        },
    ];
    let square = model_square(&mut stores.meshes);
    let mut draws = Draws::default();
    for (i, material) in (0_u32..).zip(&squares) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        let at = Vec3::new(i as f32 - (SQUARES - 1) as f32 / 2.0, 0.0, 0.0);
        draws.push_draw(Mat4::from_translation(at), &draw, u64::from(i) + 1);
    }
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut frame = |show| {
        let data = RenderData {
            show,
            antialiasing: Antialiasing::Taa,
            bloom: 1.0,
            lens: Lens {
                vignette: 1.0,
                ..Lens::NONE
            },
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        frame_pixels(&mut renderer)
    };
    let lit = frame(None);
    let square = |frame: &[u8], i: u32| pixel_at(frame, WIDTH, [i * SIDE + SIDE / 2, SIDE]);
    let above = |frame: &[u8]| pixel_at(frame, WIDTH, [SIDE / 2, 2]);
    let near = |got: [u8; 4], [r, g, b]: [u8; 3]| {
        got.iter()
            .zip([r, g, b, 255])
            .all(|(&a, b)| a.abs_diff(b) <= 1)
    };
    let value = |value: f32| (value * 255.0).round() as u8;
    let encoded_color = [encoded(0.5), encoded(0.25), encoded(1.0)];
    let up = [128, 128, 255];
    let parts = [
        (
            Channel::BaseColor,
            [encoded_color, encoded_color, encoded_color],
        ),
        (Channel::Normal, [up; 3]),
        (Channel::SurfaceNormal, [up; 3]),
        (Channel::Metallic, [[value(0.25); 3], [255; 3], [0; 3]]),
        (
            Channel::Roughness,
            [[value(0.75); 3], [255; 3], [value(0.5); 3]],
        ),
        (Channel::Occlusion, [[255; 3]; 3]),
        (Channel::Emission, [[encoded(0.5), 0, 0], [0; 3], [0; 3]]),
        // 0.04 mixed with the color by how metallic it is, and the specular color.
        (
            Channel::SpecularColor,
            [
                [encoded(0.155), encoded(0.0925), encoded(0.28)],
                encoded_color,
                [encoded(0.04); 3],
            ],
        ),
    ];
    for (channel, expected) in parts {
        let shown = frame(Some(Show::Shading(channel)));
        for (i, expected) in (0..).zip(expected) {
            let got = square(&shown, i);
            assert!(
                near(got, expected),
                "{channel:?} shows square {i} as {got:?}, not {expected:?}"
            );
        }
        assert_eq!(above(&shown), [0, 0, 0, 255], "{channel:?} shows a sky");
    }
    // Light, which the lit square takes and the unlit one doesn't.
    for channel in [Channel::DirectLight, Channel::SkyLight] {
        let shown = frame(Some(Show::Shading(channel)));
        let [lit_square, unlit] = [square(&shown, 0), square(&shown, 1)];
        assert!(
            lit_square[..3].iter().any(|&channel| channel > 8) && unlit == [0, 0, 0, 255],
            "{channel:?} shows the lit square as {lit_square:?} and the unlit one as {unlit:?}"
        );
    }
    let depth = frame(Some(Show::Image("view depth")));
    let [r, g, b, _] = square(&depth, 0);
    assert!(r > 0 && r == g && g == b, "the depth shows {:?}", [r, g, b]);
    assert_eq!(above(&depth), [0, 0, 0, 255], "where nothing drew");
    let ids = frame(Some(Show::Image("view ids")));
    let colors = [0, 1, 2].map(|i| square(&ids, i));
    assert!(
        colors[0] != colors[1] && colors[1] != colors[2] && colors.iter().all(|c| c[..3] != [0; 3]),
        "the IDs show as {colors:?}"
    );
    assert_eq!(above(&ids), [0, 0, 0, 255], "where no ID drew");
    // Images the passes after the view's make, from a frame drawn as the game's is but for its
    // antialiasing: bloom's first level, and the colors through the lens, at the middle square,
    // which the vignette leaves bright.
    for name in ["bloom 1", "view lensed"] {
        let shown = frame(Some(Show::Image(name)));
        let got = square(&shown, 1);
        assert!(
            got[..3].iter().any(|&channel| channel > 8),
            "{name} shows the middle square as {got:?}"
        );
    }
    // And the sky behind what's drawn, in the colors as drawn: its gray, of a luminance of 0.25,
    // rather than the clear color.
    let colors = frame(Some(Show::Image("view colors")));
    let [r, g, b, _] = above(&colors);
    assert!(
        r == g && g == b && r > 100,
        "the sky shows behind the colors as {:?}",
        [r, g, b]
    );
    let missing = frame(Some(Show::Image("no such image")));
    assert!(
        missing.chunks(4).all(|pixel| pixel == [0, 0, 0, 255]),
        "an image the frame doesn't make shows black"
    );
    // Antialiased afresh, at the offset after the one `lit` was drawn at.
    let again = frame(None);
    for at in (0..HEIGHT).flat_map(|y| (0..WIDTH).map(move |x| [x, y])) {
        let (got, was) = (pixel_at(&again, WIDTH, at), pixel_at(&lit, WIDTH, at));
        assert!(
            close(got, was),
            "the light shows again at {at:?} as {got:?}, not {was:?}"
        );
    }
    // The images a frame asked to list them makes that a show finds, each name once: as the game's
    // frame makes them while the view shows an image, bloom's and the lens's among them, but not
    // the show's own; and while it shows a part of shading, nothing after the view's passes; and
    // while it shows its light, antialiased, not the antialiasing's, which a show doesn't make.
    let mut listed = |show, antialiasing| {
        let data = RenderData {
            show,
            antialiasing,
            bloom: 1.0,
            lens: Lens {
                vignette: 1.0,
                ..Lens::NONE
            },
            list_images: true,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        renderer.images().to_vec()
    };
    let named =
        |listed: &[FrameImage], name| listed.iter().find(|image| image.name == name).copied();
    let images = listed(Some(Show::Image("view depth")), Antialiasing::Taa);
    let depth = FrameImage {
        name: "view depth",
        kind: ImageKind::Depth,
        size: [WIDTH, HEIGHT],
    };
    assert_eq!(named(&images, "view depth"), Some(depth), "{images:?}");
    let kind = |name| named(&images, name).map(|image| image.kind);
    assert_eq!(kind("view ids"), Some(ImageKind::Ids), "{images:?}");
    assert_eq!(kind("view colors"), Some(ImageKind::Hdr), "{images:?}");
    let bloom = named(&images, "bloom 1").map(|image| image.size);
    assert_eq!(bloom, Some([WIDTH / 2, HEIGHT / 2]), "{images:?}");
    assert!(named(&images, "view lensed").is_some(), "{images:?}");
    assert!(named(&images, "view shown").is_none(), "the show's own");
    let mut names: Vec<_> = images.iter().map(|image| image.name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "each once: {images:?}");
    let shading = listed(Some(Show::Shading(Channel::Roughness)), Antialiasing::Taa);
    assert!(named(&shading, "view depth").is_some(), "{shading:?}");
    assert!(named(&shading, "bloom 1").is_none(), "{shading:?}");
    for antialiasing in [Antialiasing::Taa, Antialiasing::Smaa] {
        let lit = listed(None, antialiasing);
        assert!(
            named(&lit, "bloom 1").is_some(),
            "{antialiasing:?}: {lit:?}"
        );
        let names = [
            "view resolved",
            "smaa edges",
            "smaa weights",
            "view antialiased",
        ];
        for name in names {
            assert!(named(&lit, name).is_none(), "{antialiasing:?}: {lit:?}");
        }
    }
    shut_down(renderer);
}

fn the_light_shows_as_what_surfaces_diffuse_and_what_they_reflect() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 3;
    const WIDTH: u32 = SIDE * SQUARES;
    const HEIGHT: u32 = 2 * SIDE;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 2.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let sky = sky_texture(&mut stores.textures, [128, 64], |_, _| [1.0; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 0.25,
    });
    // From the viewpoint, so that each square's highlight faces it, and its specular light is as
    // much as a smooth surface's can be.
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::NEG_Z,
        color: Vec3::splat(1.0),
        range: None,
        shadow_distance: None,
    }];
    // A dielectric, a metal, which diffuses nothing, and what's unlit, which takes no light.
    let squares = [
        Material {
            color: Vec4::new(0.5, 0.5, 0.5, 1.0),
            workflow: metallic(0.0, 0.5),
            ..Material::default()
        },
        Material {
            color: Vec4::new(0.4, 0.4, 0.4, 1.0),
            workflow: metallic(1.0, 0.5),
            ..Material::default()
        },
        Material {
            color: Vec4::new(0.5, 0.5, 0.5, 1.0),
            unlit: true,
            ..Material::default()
        },
    ];
    let square = model_square(&mut stores.meshes);
    let mut draws = Draws::default();
    for (i, material) in (0_u32..).zip(&squares) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        let at = Vec3::new(i as f32 - (SQUARES - 1) as f32 / 2.0, 0.0, 0.0);
        draws.push_draw(Mat4::from_translation(at), &draw, u64::from(i) + 1);
    }
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // Each square's linear light, as the view shows `channel`, through no tonemapper.
    let mut light = |channel| {
        let data = RenderData {
            show: Some(Show::Shading(channel)),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        [0, 1, 2].map(|i| {
            let [r, g, b, _] = pixel_at(&frame, WIDTH, [i * SIDE + SIDE / 2, SIDE]);
            [r, g, b].map(decoded)
        })
    };
    let [direct, sky, reflection, diffuse, specular] = [
        Channel::DirectLight,
        Channel::SkyLight,
        Channel::Reflection,
        Channel::DiffuseLight,
        Channel::SpecularLight,
    ]
    .map(&mut light);
    // Within the 8-bit rounding of the five values.
    for i in 0..2 {
        for c in 0..3 {
            let parts = direct[i][c] + sky[i][c] + reflection[i][c];
            let split = diffuse[i][c] + specular[i][c];
            assert!(
                (parts - split).abs() <= 0.01,
                "square {i} diffuses {:?} and reflects {:?}, which add up to {split}, not {parts}, \
                 its lights' {:?}, its sky's {:?} and its reflection's {:?}",
                diffuse[i],
                specular[i],
                direct[i],
                sky[i],
                reflection[i]
            );
        }
    }
    // The dielectric's highlight, about 0.05 of the light, besides the sky's, about 0.01.
    assert!(
        specular[0].iter().all(|&c| c > 0.04),
        "the dielectric reflects {:?}",
        specular[0]
    );
    assert_eq!(diffuse[1], [0.0; 3], "the metal diffuses nothing");
    assert!(
        specular[1].iter().all(|&c| c > 0.3),
        "the metal reflects {:?}",
        specular[1]
    );
    assert_eq!(
        [diffuse[2], specular[2]],
        [[0.0; 3]; 2],
        "what's unlit takes no light"
    );
    shut_down(renderer);
}

/// Spheres of KHR_materials_pbrSpecularGlossiness's materials, lit by a sky, a directional light and
/// a point light, each showing as its metallic-roughness equal does: a dielectric, which reflects
/// 0.04 head-on and diffuses its base color, as one that diffuses that over 0.96, as much as its
/// specular color's strongest channel takes off; a metal as one that diffuses nothing and reflects
/// its color; each as rough as 1 less its glossiness. Then one whose specular color and glossiness
/// come from its map, of colors, its specular color sRGB-encoded and its glossiness in alpha as
/// stored, as one whose factors are those.
fn specular_glossiness_shades_as_its_metallic_roughness_equal() {
    const SIDE: u32 = 40;
    const SPHERES: u32 = 5;
    const WIDTH: u32 = SIDE * SPHERES;
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let [width, height] = [64, 32];
    let sky = sky_texture(&mut stores.textures, [width, height], |_, y| {
        let up = 1.0 - 2.0 * (y as f32 + 0.5) / height as f32;
        if up > 0.0 {
            [0.4, 0.6, 1.0]
        } else {
            [0.15, 0.1, 0.05]
        }
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let sun = Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::new(1.0, -1.0, -1.0).normalize(),
        color: Vec3::splat(2.0),
        range: None,
        shadow_distance: None,
    };
    let point = Light {
        kind: LightKind::Point,
        position: Vec3::new(-1.0, 1.0, 2.0),
        color: Vec3::splat(3.0),
        ..sun
    };
    stores.lights = vec![sun, point];
    // A specular color of about 0.5, sRGB-encoded, and a glossiness of 0.6, as a map holds them.
    let [specular_byte, glossiness_byte] = [188, 153];
    let specular = math::libm::powf((f32::from(specular_byte) / 255.0 + 0.055) / 1.055, 2.4);
    let glossiness = f32::from(glossiness_byte) / 255.0;
    let texel = [specular_byte, specular_byte, specular_byte, glossiness_byte];
    let map = stores.textures.add(
        "specular-glossiness",
        [4, 4],
        [texel; 16].as_flattened(),
        Texels::Color(Mipmaps::None),
    );
    let map = Sampled::of(map.expect("a map"));
    let color = Vec4::new(0.9, 0.45, 0.2, 1.0);
    let glossy = |color: Vec4, specular: Vec3, glossiness: f32| Material {
        color,
        workflow: Workflow::SpecularGlossiness {
            specular,
            glossiness,
        },
        ..Material::default()
    };
    let rough = |metallic_: f32, roughness: f32| Material {
        color,
        workflow: metallic(metallic_, roughness),
        ..Material::default()
    };
    let diffused = (color / 0.96).with_w(1.0);
    let black = Vec4::new(0.0, 0.0, 0.0, 1.0);
    let pairs = [
        (rough(0.0, 0.3), glossy(diffused, Vec3::splat(0.04), 0.7)),
        (rough(0.0, 1.0), glossy(diffused, Vec3::splat(0.04), 0.0)),
        (rough(1.0, 0.3), glossy(black, color.truncate(), 0.7)),
        (rough(1.0, 0.8), glossy(black, color.truncate(), 0.2)),
        (
            glossy(color, Vec3::splat(specular), glossiness),
            Material {
                surface_texture: map,
                ..glossy(color, Vec3::ONE, 1.0)
            },
        ),
    ];
    let sphere = sphere(&mut stores.meshes, 0.42, 24, 48);
    let at = |i: u32| Vec3::new(i as f32 - (SPHERES - 1) as f32 / 2.0, 0.0, 0.0);
    let viewport = filling([WIDTH, SIDE]);
    let mut frames = Vec::new();
    for second in [false, true] {
        let mut draws = Draws::default();
        for (i, &(equal, material)) in (0..).zip(&pairs) {
            let material = stores
                .materials
                .add(if second { &material } else { &equal });
            let draw =
                MeshDraw::with_material(sphere, material, &stores.materials, &mut stores.shaders);
            draws.push_draw(Mat4::from_translation(at(i)), &draw, 0);
        }
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        frames.push(frame_pixels(&mut renderer));
    }
    let middle = |frame: &[u8], i: u32| pixel_at(frame, WIDTH, [i * SIDE + SIDE / 2, SIDE / 2]);
    assert_ne!(
        middle(&frames[0], 0),
        middle(&frames[0], 2),
        "a dielectric and a metal show alike: the spheres didn't draw"
    );
    let pixels = |frame: &[u8]| frame.as_chunks::<4>().0.to_vec();
    let differ = (pixels(&frames[0]).into_iter().zip(pixels(&frames[1])))
        .enumerate()
        .find(|&(_, (equal, glossy))| !close(glossy, equal));
    if let Some((i, (equal, glossy))) = differ {
        let [x, y] = [i as u32 % WIDTH, i as u32 / WIDTH];
        panic!(
            "sphere {}'s pixel ({x}, {y}) shows {glossy:?} of specular-glossiness, where its \
             equal shows {equal:?}",
            x / SIDE
        );
    }
    shut_down(renderer);
}

/// A white floor seen from above under a sky of 10⁵ candela per square meter with a sun in it, a
/// texel past 10⁷, as a sun's disk is, 45° up: lit by a directional light from the sun too, it
/// shows as under the sky without its sun and the light, the sky's light leaving the sun out; lit
/// by the sky alone, its sun lights it through the sky's light, the floor much brighter than under
/// the sky without it. The sky's light is prefiltered again as the light comes and goes.
fn a_skys_sun_lights_once() {
    const SIZE: u32 = 8;
    const SKY: [u32; 2] = [512, 256];
    const SUN: [u32; 2] = [256, 64];
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 5.0, 0.0),
        rotation: Quat::from_rotation_x(-FRAC_PI_2),
        projection: Projection::Orthographic {
            height: 1.0,
            near: 0.5,
            far: 10.0,
        },
    };
    // The texel's 6,260 of 10⁶ each, about 1.07 × 10⁻⁴ steradians, light the floor about as much
    // as the sky's 0.1 does; each of the texels past 10⁷ that the sky's light keeps to it, some
    // 10⁻⁴ of that.
    let texel = |sun: bool| {
        move |x, y| {
            if sun && [x, y] == SUN {
                [6260.0; 3]
            } else {
                [0.1; 3]
            }
        }
    };
    let sky = sky_texture(&mut stores.textures, SKY, texel(true));
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0e6,
    });
    // Toward the sun's texel's center, as the image maps ways.
    let across = ((SUN[0] as f32 + 0.5) / SKY[0] as f32 - 0.5) * 2.0 * PI;
    let down = (SUN[1] as f32 + 0.5) / SKY[1] as f32 * PI;
    let (sin, cos) = (math::libm::sinf, math::libm::cosf);
    let to_sun = Vec3::new(cos(across) * sin(down), cos(down), sin(across) * sin(down));
    let sun = Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: -to_sun,
        color: Vec3::splat(6.0e5),
        range: None,
        shadow_distance: None,
    };
    let square = model_square(&mut stores.meshes);
    let white = Material {
        color: Vec4::ONE,
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let white = stores.materials.add(&white);
    let floor = MeshDraw::with_material(square, white, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(2.0, Vec3::ZERO), &floor, 0);
    let viewport = filling([SIZE, SIZE]);
    // The sky at 0.2 on the floor.
    let floor_shows = |renderer: &mut Renderer, stores: &mut Stores| {
        let data = RenderData {
            exposure: 2.0e-6,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(renderer, data);
        pixel_at(&frame_pixels(renderer), SIZE, [SIZE / 2, SIZE / 2])
    };
    stores.lights = vec![sun];
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let lit_with_sun = floor_shows(&mut renderer, &mut stores);
    stores.lights.clear();
    let with_sun = floor_shows(&mut renderer, &mut stores);
    let without = sky_pixels(SKY, texel(false));
    let replaced = (stores.textures).replace("sky", sky, SKY, without, Texels::Hdr);
    replaced.expect("a sky without its sun");
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let without_sun = floor_shows(&mut renderer, &mut stores);
    stores.lights = vec![sun];
    let lit_without_sun = floor_shows(&mut renderer, &mut stores);
    assert!(
        close(lit_with_sun, lit_without_sun),
        "lit by the light that stands for its sun, the floor shows {lit_with_sun:?} under the sky \
         with its sun and {lit_without_sun:?} under the sky without"
    );
    assert!(
        with_sun[0] >= without_sun[0] + 32,
        "lit by the sky alone, the floor shows {with_sun:?} under the sky with its sun, too near \
         {without_sun:?} under the sky without"
    );
    assert!(
        lit_without_sun[0] >= without_sun[0] + 32,
        "the light lights the floor, {lit_without_sun:?}, much as the sun does: {without_sun:?} \
         without it"
    );
    shut_down(renderer);
}

/// The share of the light from `distance` along `way` from `origin`, or from infinitely far for
/// `None`, that `fog` lets through: e to the minus the integral of its density along the way, past
/// its start, by the midpoint rule over 100,000 steps, in doubles; upward to where its density is
/// e^-40 of what it is at its start, and downward without end; and at least what its opacity leaves.
fn fog_through(fog: &Fog, origin: Vec3, way: Vec3, distance: Option<f32>) -> f32 {
    let (falloff, start) = (f64::from(fog.falloff), f64::from(fog.start));
    let least = 1.0 - fog.opacity;
    let end = match distance {
        Some(distance) => f64::from(distance),
        None if way.y > 0.0 => start + 40.0 / (falloff * f64::from(way.y)),
        None => return least,
    };
    if end <= start {
        return 1.0;
    }
    const STEPS: u32 = 100_000;
    let step = (end - start) / f64::from(STEPS);
    let density = |along: f64| {
        let y = f64::from(origin.y) + f64::from(way.y) * along;
        f64::from(fog.density) * math::libm::exp(-falloff * (y - f64::from(fog.height)))
    };
    let depth: f64 = (0..STEPS)
        .map(|i| density(start + (f64::from(i) + 0.5) * step) * step)
        .sum();
    (math::libm::exp(-depth) as f32).max(least)
}

/// What Henyey-Greenstein's phase function of `g` gives a cosine of `cos` between the way light
/// goes and the way it's scattered, per steradian.
fn henyey_greenstein(g: f32, cos: f32) -> f32 {
    let spread = 1.0 + g * g - 2.0 * g * cos;
    (1.0 - g * g) / (4.0 * PI * spread * math::libm::sqrtf(spread))
}

/// A rectangle of one color, as it's drawn, in linear RGB and alpha, which it blends by, facing
/// along an axis, at `at` along it, from `least` to `most` along the other two, in order, which a
/// pixel's way meets or misses.
#[derive(Clone, Copy, Debug)]
struct Patch {
    /// 1 for one facing along Y, 2 for one facing along Z.
    axis: usize,
    at: f32,
    least: Vec2,
    most: Vec2,
    color: Vec4,
}

impl Patch {
    /// One facing along Z, `side` across, its center at `center`.
    fn facing(center: Vec3, side: f32, color: Vec4) -> Self {
        let half = Vec2::splat(side / 2.0);
        Self {
            axis: 2,
            at: center.z,
            least: center.truncate() - half,
            most: center.truncate() + half,
            color,
        }
    }

    /// How far along `way` from `origin` it is, if the way meets it.
    fn met(&self, origin: Vec3, way: Vec3) -> Option<f32> {
        let along = (self.at - origin[self.axis]) / way[self.axis];
        let point = origin + way * along;
        let across = if self.axis == 2 {
            Vec2::new(point.x, point.y)
        } else {
            Vec2::new(point.x, point.z)
        };
        let inside = across.cmpge(self.least).all() && across.cmple(self.most).all();
        (along > 0.0 && inside).then_some(along)
    }
}

/// Asserts that each of `pixels` of `frame`, a view `size` pixels across and down that `viewpoint`
/// sees, in `fog`, lit by `sun`, a directional light's illuminance and the way toward it, if one
/// shines, shows what the pixel's way meets of `patches`, each by its alpha over what's behind it,
/// over the clear color, as far as can be: each as much of it as the fog lets through, as
/// [`fog_through`] integrates it, and the fog's color of the light that reaches it, its luminance
/// and the sun's by Henyey-Greenstein's lobe, by as much as the fog takes out; each channel within
/// 2 of that, sRGB-encoded.
fn assert_fogged(
    frame: &[u8],
    size: [u32; 2],
    viewpoint: &Viewpoint,
    fog: &Fog,
    sun: Option<(Vec3, Vec3)>,
    patches: &[Patch],
    pixels: &[[u32; 2]],
) {
    let [width, height] = size.map(|side| side as f32);
    let world_from_clip = viewpoint.view_projection(width / height).inverse();
    let clear = Vec3::new(CLEAR[0], CLEAR[1], CLEAR[2]);
    for &[x, y] in pixels {
        let clip = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / Vec2::new(width, height) * 2.0 - 1.0;
        let near = world_from_clip.project_point3(clip.extend(1.0));
        // From the camera, or without perspective, the one way it looks, from its plane.
        let (origin, way) = match viewpoint.projection {
            Projection::Perspective { .. } => {
                let eye = viewpoint.position.as_vec3();
                (eye, (near - eye).normalize())
            }
            Projection::Orthographic { .. } => {
                let way = viewpoint.rotation * Vec3::NEG_Z;
                (
                    near - way * (near - viewpoint.position.as_vec3()).dot(way),
                    way,
                )
            }
        };
        let sunlight = sun.map_or(Vec3::ZERO, |(light, to_sun)| {
            light * fog.sun * henyey_greenstein(fog.sharpness, way.dot(to_sun))
        });
        let light = fog.color * (fog.luminance + sunlight);
        let fogged = |distance: Option<f32>, color: Vec3| {
            let through = fog_through(fog, origin, way, distance);
            color * through + light * (1.0 - through)
        };
        // The farthest first.
        let mut met: Vec<(f32, Vec4)> = (patches.iter())
            .filter_map(|patch| Some((patch.met(origin, way)?, patch.color)))
            .collect();
        met.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut expected = fogged(None, clear);
        for (at, color) in met {
            expected = fogged(Some(at), color.truncate()) * color.w + expected * (1.0 - color.w);
        }
        let [r, g, b] = expected.to_array().map(encoded);
        let got = pixel_at(frame, size[0], [x, y]);
        let near = got
            .iter()
            .zip([r, g, b, 255])
            .all(|(&a, b)| a.abs_diff(b) <= 2);
        assert!(
            near,
            "pixel ({x}, {y}) shows {got:?} in {fog:?} from {viewpoint:?}, not {expected} \
             encoded, {:?}",
            [r, g, b]
        );
    }
}

/// Renders a frame of `draws` and `canvas` into a view `size` pixels across and down, as
/// `viewpoint` sees them, in `fog`, antialiased as `antialiasing` says: its pixels.
#[expect(clippy::too_many_arguments, reason = "a frame's every part apart")]
fn fogged_frame(
    renderer: &mut Renderer,
    stores: &mut Stores,
    size: [u32; 2],
    viewpoint: Viewpoint,
    draws: &Draws,
    canvas: &Draws,
    fog: Fog,
    antialiasing: Antialiasing,
) -> Vec<u8> {
    let data = RenderData {
        fog: Some(fog),
        canvas,
        antialiasing,
        ..stores.data(filling(size), viewpoint, draws)
    };
    render_without_ui(renderer, data);
    frame_pixels(renderer)
}

/// In height fog lit by a luminance of its own, and then by a sun too, in a lobe round it, each
/// pixel shows what's behind it as much as the fog lets through, and the fog's light, its color of
/// what reaches it, by as much as it takes out, as an integral of the fog's density along the
/// pixel's way finds them, numerically: through a camera with perspective, a black wall, unlit, 10
/// units ahead and left of it, the clear color right of it, as far as can be, and, unlit, each of
/// a color of its own, a sprite before the wall, and a circle, a glyph and a particle past it, which
/// fog themselves at their quads' corners. Past the fog's start alone, so that the wall nearer than
/// it shows black; upward out of the fog, to the sky; and looking down, where the fog has no end,
/// all of the clear color but what its opacity lets through. Through a camera without perspective,
/// looking a little up; in a view of nothing opaque, all of what it draws blending, which draws its
/// canvas unfogged; antialiased; and from 100 units above the fog's height, looking down on a floor
/// at it, where the fog's density at the camera is e^-100 of the floor's.
fn fog_dims_and_lights_as_its_integral_says() {
    const SIZE: [u32; 2] = [64, 48];
    /// What's drawn on the engine's quad, across.
    const SIDE: f32 = 1.5;
    let mut renderer = renderer(SIZE[0], SIZE[1]);
    let mut stores = Stores::default();
    let square = model_square(&mut stores.meshes);
    let black = Material {
        color: Vec4::new(0.0, 0.0, 0.0, 1.0),
        unlit: true,
        ..Material::default()
    };
    let black = stores.materials.add(&black);
    let model = MeshDraw::with_material(square, black, &stores.materials, &mut stores.shaders);
    let wall = Mat4::from_scale_rotation_translation(
        Vec3::new(60.0, 120.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(-30.0, 0.0, -10.0),
    );
    let wall_patch = Patch {
        axis: 2,
        at: -10.0,
        least: Vec2::new(-60.0, -60.0),
        most: Vec2::new(0.0, 60.0),
        color: Vec4::new(0.0, 0.0, 0.0, 1.0),
    };
    let floor = facing_up(400.0, Vec3::ZERO);
    let floor_patch = Patch {
        axis: 1,
        at: 0.0,
        least: Vec2::splat(-200.0),
        most: Vec2::splat(200.0),
        color: Vec4::new(0.0, 0.0, 0.0, 1.0),
    };
    // A field whose every texel is inside a glyph's edges, so that a glyph fills its quad.
    let field = stores
        .textures
        .add("field", [4, 4], vec![255; 64], Texels::Data);
    let field = field.expect("a field");
    let quads = [
        Vec3::new(-2.0, 1.2, -6.0),
        Vec3::new(2.0, 2.6, -12.0),
        Vec3::new(3.5, 1.0, -14.0),
        Vec3::new(1.5, 0.6, -16.0),
    ];
    // The circle half transparent, so that what it's over shows how many times it's fogged.
    let colors = [
        Vec4::new(0.9, 0.2, 0.1, 1.0),
        Vec4::new(0.2, 0.8, 0.3, 0.5),
        Vec4::new(0.3, 0.4, 0.9, 1.0),
        Vec4::new(0.9, 0.8, 0.2, 1.0),
    ];
    let patches: Vec<Patch> = (quads.iter().zip(colors))
        .map(|(&at, color)| Patch::facing(at, SIDE, color))
        .collect();
    // The models placed by `models`, then the sprite, the circle and the glyph, and the particle,
    // if `particle`, born in each frame alike, so that a frame after one without it draws it too.
    let drawn = |stores: &mut Stores, models: &[Mat4], particle: bool| {
        let mut draws = Draws::default();
        for &placed in models {
            draws.push_draw(placed, &model, 0);
        }
        let placed = |at: Vec3| Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(SIDE));
        let [sprite, circle, glyph, particle_color] = colors;
        let sprite = Sprite {
            color: sprite,
            ..Sprite::default()
        };
        draws.push_sprite(placed(quads[0]), sprite);
        let circle = Circle {
            color: circle,
            thickness: 1.0,
            fade: 0.0,
        };
        draws.push_circle(&mut stores.shaders, placed(quads[1]), circle);
        let glyph = Glyph {
            color: glyph,
            field,
            rect: [Vec2::ZERO, Vec2::ONE],
            range: 4.0,
        };
        let glyphs = std::iter::once((placed(quads[2]), glyph, 0));
        draws.extend_glyphs(&mut stores.shaders, glyphs);
        if particle {
            let at = Mat4::from_translation(quads[3]);
            let emission = Emission {
                shape: ParticleShape::Point,
                speed: [0.0; 2],
                lifetime: [10.0; 2],
                spin: [0.0; 2],
                acceleration: Vec3::ZERO,
                drag: 0.0,
                size: [SIDE; 2],
                color: [particle_color; 2],
                texture: Some(Texture::WHITE),
                cells: [1, 1],
                additive: false,
            };
            let births = Births {
                first: 0,
                count: 1,
                time: 0.0,
                every: 1.0,
                from: at,
                to: at,
                along: 0.0,
                step: 1.0 / 16.0,
            };
            draws.push_particles(Particles {
                id: 9,
                fresh: false,
                most: 64,
                now: 16.0,
                tick: 1.0 / 64.0,
                births,
                bursts: Bursts::none(at),
                emission,
            });
        }
        draws
    };
    let looking = Viewpoint {
        position: DVec3::new(0.0, 2.0, 0.0),
        projection: Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let draws = drawn(&mut stores, &[wall], true);
    settle(&mut renderer, &mut stores, filling(SIZE), looking, &draws);
    let none = Draws::default();
    let low = Fog {
        density: 0.08,
        falloff: 0.3,
        height: 1.0,
        start: 10.5,
        opacity: 1.0,
        color: Vec3::ONE,
        luminance: 1.0,
        ..Fog::default()
    };
    let sunlit = Fog {
        density: 0.3,
        falloff: 0.15,
        height: 3.0,
        start: 0.0,
        opacity: 0.7,
        color: Vec3::new(1.0, 0.8, 0.6),
        luminance: 0.25,
        sun: 1.0,
        sharpness: 0.5,
    };
    let to_sun = Vec3::new(1.0, 0.2, -1.0).normalize();
    let sun_light = Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: -to_sun,
        color: Vec3::splat(0.8),
        range: None,
        shadow_distance: None,
    };
    let sunlight = Some((sun_light.color, to_sun));
    let (unaliased, taa) = (Antialiasing::None, Antialiasing::Taa);
    let grid: Vec<[u32; 2]> = [8, 30, 56]
        .into_iter()
        .flat_map(|x| [1, 10, 20, 26, 34, 46].map(|y| [x, y]))
        .collect();
    // The grid's pixels, and those `viewpoint` sees `quads`' centers at.
    let seen = |viewpoint: &Viewpoint, quads: &[Vec3]| {
        let centers = quads.iter().map(|&at| pixel_of(*viewpoint, SIZE, at));
        grid.iter().copied().chain(centers).collect::<Vec<_>>()
    };
    let mut walled = vec![wall_patch];
    walled.extend_from_slice(&patches);
    // Through perspective, before and after its start, then lit by the sun too.
    let pixels = seen(&looking, &quads);
    for (fog, sun) in [(low, None), (sunlit, sunlight)] {
        stores.lights = sun.map_or(Vec::new(), |_| vec![sun_light]);
        let (renderer, stores) = (&mut renderer, &mut stores);
        let frame = fogged_frame(
            renderer, stores, SIZE, looking, &draws, &none, fog, unaliased,
        );
        assert_fogged(&frame, SIZE, &looking, &fog, sun, &walled, &pixels);
    }
    // Antialiased, each pixel resolved from frames drawn a fraction of a pixel off, away from edges.
    let flat = [[8, 10], [8, 40], [56, 5], [56, 40]];
    for _ in 0..16 {
        let (renderer, stores) = (&mut renderer, &mut stores);
        fogged_frame(renderer, stores, SIZE, looking, &draws, &none, sunlit, taa);
    }
    let frame = frame_pixels(&mut renderer);
    assert_fogged(&frame, SIZE, &looking, &sunlit, sunlight, &walled, &flat);
    // Without perspective, looking a little up, every way rising out of the fog; the particle,
    // which faces the view, aside.
    let orthographic = Viewpoint {
        rotation: Quat::from_rotation_x(0.3),
        projection: Projection::Orthographic {
            height: 12.0,
            near: 0.1,
            far: 100.0,
        },
        ..looking
    };
    let draws = drawn(&mut stores, &[wall], false);
    let frame = fogged_frame(
        &mut renderer,
        &mut stores,
        SIZE,
        orthographic,
        &draws,
        &none,
        sunlit,
        unaliased,
    );
    let pixels = seen(&orthographic, &quads[..3]);
    let flat_walled = &walled[..4];
    assert_fogged(
        &frame,
        SIZE,
        &orthographic,
        &sunlit,
        sunlight,
        flat_walled,
        &pixels,
    );
    // Nothing opaque, all blending, the fog's pass fogging the clear color alone; and the canvas,
    // a sprite in the view's bottom left corner, unfogged.
    let draws = drawn(&mut stores, &[], true);
    let mut canvas = Draws::default();
    let cornered =
        Mat4::from_translation(Vec3::new(4.0, 4.0, 0.0)) * Mat4::from_scale(Vec3::splat(6.0));
    let orange = Vec4::new(0.8, 0.5, 0.2, 1.0);
    let sprite = Sprite {
        color: orange,
        ..Sprite::default()
    };
    canvas.push_sprite(cornered, sprite);
    let frame = fogged_frame(
        &mut renderer,
        &mut stores,
        SIZE,
        looking,
        &draws,
        &canvas,
        sunlit,
        unaliased,
    );
    let pixels = seen(&looking, &quads);
    assert_fogged(&frame, SIZE, &looking, &sunlit, sunlight, &patches, &pixels);
    let [r, g, b] = orange.truncate().to_array().map(encoded);
    let corner = pixel_at(&frame, SIZE[0], [4, SIZE[1] - 5]);
    assert!(
        close(corner, [r, g, b, 255]),
        "the canvas shows {corner:?} in fog"
    );
    // From 100 units above the fog's height, looking down on a floor at it.
    stores.lights.clear();
    let above = Viewpoint {
        position: DVec3::new(0.0, 100.0, 0.0),
        rotation: Quat::from_rotation_x(-FRAC_PI_2),
        ..looking
    };
    let high = Fog {
        density: 0.5,
        falloff: 1.0,
        height: 0.0,
        start: 0.0,
        opacity: 1.0,
        color: Vec3::ONE,
        luminance: 1.0,
        sun: 1.0,
        sharpness: 0.0,
    };
    let mut draws = Draws::default();
    draws.push_draw(floor, &model, 0);
    let frame = fogged_frame(
        &mut renderer,
        &mut stores,
        SIZE,
        above,
        &draws,
        &none,
        high,
        unaliased,
    );
    let pixels = [[32, 24], [8, 8], [56, 40]];
    assert_fogged(&frame, SIZE, &above, &high, None, &[floor_patch], &pixels);
    shut_down(renderer);
}

/// A scene of every kind of draw that fog fogs, as a camera with perspective sees it, under a sky
/// and a sun casting shadows: opaque spheres receding along a floor, the sky behind them, a glass
/// square of a material that blends, a sprite, a circle and an emitter's particles, born in the
/// frame [`FoggedScene::settle`] ends with, which the frames after, at the same time, draw alike.
struct FoggedScene {
    renderer: Renderer,
    stores: Stores,
    viewport: Viewport,
    viewpoint: Viewpoint,
    /// Its models' draws, each with its transform and ID, the glass last.
    models: Vec<(Mat4, MeshDraw, u64)>,
}

impl FoggedScene {
    fn new(width: u32, height: u32) -> Self {
        let renderer = renderer(width, height);
        let mut stores = Stores::default();
        let [sky_width, sky_height] = [64, 32];
        let sky = sky_texture(&mut stores.textures, [sky_width, sky_height], |_, y| {
            let up = 1.0 - 2.0 * (y as f32 + 0.5) / sky_height as f32;
            if up > 0.0 {
                let pale = 1.0 - up;
                [0.25 + 0.5 * pale, 0.45 + 0.35 * pale, 0.9]
            } else {
                [0.1, 0.08, 0.06]
            }
        });
        stores.sky = Some(Sky {
            texture: sky,
            luminance: 1.0,
        });
        stores.lights = vec![Light {
            kind: LightKind::Directional,
            position: Vec3::ZERO,
            direction: -Vec3::new(0.6, 0.25, -1.0).normalize(),
            color: Vec3::new(3.0, 2.8, 2.4),
            range: None,
            shadow_distance: Some(60.0),
        }];
        let viewpoint = Viewpoint {
            position: DVec3::new(0.0, 1.2, 5.0),
            rotation: Quat::from_rotation_x(-0.08),
            projection: Projection::Perspective {
                fov: 0.9,
                near: 0.1,
            },
        };
        let viewport = filling([width, height]);
        let mut models = Vec::new();
        let mut model = |stores: &mut Stores, mesh, material: &Material, placed, id| {
            let material = stores.materials.add(material);
            let draw =
                MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders);
            models.push((placed, draw, id));
        };
        let square = model_square(&mut stores.meshes);
        let ground = Material {
            color: Vec4::new(0.5, 0.48, 0.45, 1.0),
            workflow: metallic(0.0, 0.9),
            ..Material::default()
        };
        let floor = facing_up(80.0, Vec3::new(0.0, 0.0, -20.0));
        model(&mut stores, square, &ground, floor, 1);
        let ball = sphere(&mut stores.meshes, 0.5, 16, 32);
        let colors = [
            Vec4::new(0.8, 0.2, 0.15, 1.0),
            Vec4::new(0.2, 0.6, 0.25, 1.0),
            Vec4::new(0.2, 0.3, 0.8, 1.0),
            Vec4::new(0.85, 0.75, 0.3, 1.0),
        ];
        for i in 0..8 {
            let material = Material {
                color: colors[i % colors.len()],
                workflow: metallic(0.0, 0.4),
                ..Material::default()
            };
            let side = if i % 2 == 0 { -1.3 } else { 1.3 };
            let at = Mat4::from_translation(Vec3::new(side, 0.5, -3.0 * i as f32));
            model(&mut stores, ball, &material, at, 2 + i as u64);
        }
        let glass = Material {
            color: Vec4::new(0.6, 0.8, 1.0, 0.35),
            workflow: metallic(0.0, 0.1),
            alpha: AlphaMode::Blend,
            double_sided: true,
            ..Material::default()
        };
        let pane = Mat4::from_scale_rotation_translation(
            Vec3::new(1.2, 1.2, 1.0),
            Quat::IDENTITY,
            Vec3::new(0.3, 0.9, 1.5),
        );
        model(&mut stores, square, &glass, pane, 20);
        Self {
            renderer,
            stores,
            viewport,
            viewpoint,
            models,
        }
    }

    /// The scene's draws: its models, the glass, which blends, last, then the rest, which blends,
    /// with the particles' births of the frame if `born`.
    fn draws(&mut self, born: bool) -> Draws {
        let stores = &mut self.stores;
        let mut draws = Draws::default();
        for (placed, draw, id) in &self.models {
            draws.push_draw(*placed, draw, *id);
        }
        draws.push_sprite(
            Mat4::from_translation(Vec3::new(-2.5, 1.5, -10.0)),
            Sprite {
                color: Vec4::new(1.0, 0.9, 0.3, 0.8),
                ..Sprite::default()
            },
        );
        let circle = Circle {
            color: Vec4::new(0.9, 0.3, 0.9, 0.9),
            ..Circle::default()
        };
        let far =
            Mat4::from_translation(Vec3::new(2.6, 1.4, -14.0)) * Mat4::from_scale(Vec3::splat(1.5));
        draws.push_circle(&mut stores.shaders, far, circle);
        let at = Mat4::from_translation(Vec3::new(-0.2, 0.2, -2.5));
        let disc = Emission {
            shape: ParticleShape::Cone {
                angle: 0.3,
                radius: 0.05,
            },
            speed: [2.0, 2.6],
            lifetime: [1.0, 1.2],
            spin: [0.0; 2],
            acceleration: Vec3::new(0.0, -3.0, 0.0),
            drag: 0.2,
            size: [0.25, 0.12],
            color: [
                Vec4::new(1.0, 0.5, 0.15, 0.9),
                Vec4::new(1.0, 0.2, 0.05, 0.0),
            ],
            texture: None,
            cells: [1, 1],
            additive: false,
        };
        let births = if born {
            Births {
                first: 0,
                count: 65,
                time: 0.0,
                every: 0.25,
                from: at,
                to: at,
                along: 0.0,
                step: 0.25 / 16.0,
            }
        } else {
            Births::none(at)
        };
        draws.push_particles(Particles {
            id: 30,
            fresh: false,
            most: 96,
            now: 16.0,
            tick: 1.0 / 64.0,
            births,
            bursts: Bursts::none(at),
            emission: disc,
        });
        draws
    }

    /// Renders the scene, its particles born in an earlier frame, in `fog`: its pixels.
    fn render(&mut self, fog: Option<Fog>) -> Vec<u8> {
        let draws = self.draws(false);
        let data = RenderData {
            fog,
            tonemapper: Tonemapper::PbrNeutral,
            ..self.stores.data(self.viewport, self.viewpoint, &draws)
        };
        render_without_ui(&mut self.renderer, data);
        frame_pixels(&mut self.renderer)
    }

    /// Settles it, and has a frame give its particles their births.
    fn settle(&mut self) {
        let (viewport, viewpoint) = (self.viewport, self.viewpoint);
        let draws = self.draws(false);
        settle(
            &mut self.renderer,
            &mut self.stores,
            viewport,
            viewpoint,
            &draws,
        );
        let draws = self.draws(true);
        let data = self.stores.data(viewport, viewpoint, &draws);
        render_without_ui(&mut self.renderer, data);
    }
}

/// Fog of no density, and fog of no opacity, which takes out none of the light behind it however
/// far, draw a scene of every kind of draw that fog fogs exactly as it draws without fog, though
/// fog of no opacity has its pass fog what's opaque and draws what blends after it; fog that takes
/// some light draws it otherwise. Likewise with ambient occlusion, which has the view draw what's
/// opaque again, shaded, once the occlusion is found, after which the fog's pass fogs it: the sky,
/// which the occlusion doesn't darken, as fogged as without it. The frames compared are 8 apart,
/// once the occlusion's history has settled: a still view's occlusion takes 8 frames' noise in
/// turn, which its history averages, so that each frame draws it as the frame 8 before it did.
fn fog_of_no_density_or_opacity_changes_nothing() {
    const SIZE: [u32; 2] = [96, 64];
    let mut scene = FoggedScene::new(SIZE[0], SIZE[1]);
    scene.settle();
    let thin = Fog {
        density: 0.0,
        ..Fog::default()
    };
    let clear = Fog {
        density: 0.5,
        opacity: 0.0,
        ..Fog::default()
    };
    let foggy = Fog {
        density: 0.5,
        ..Fog::default()
    };
    // The 8th of the frames from now, in `fog`, the 7 before it without.
    let eighth = |scene: &mut FoggedScene, fog| {
        for _ in 0..7 {
            scene.render(None);
        }
        scene.render(fog)
    };
    let mut fogged = Vec::new();
    for occlusion in [None, Some(AmbientOcclusion::default())] {
        scene.stores.ambient_occlusion = occlusion;
        for _ in 0..128 {
            scene.render(None);
        }
        let without = scene.render(None);
        for (name, fog) in [("no density", thin), ("no opacity", clear)] {
            assert!(
                eighth(&mut scene, Some(fog)) == without,
                "fog of {name} changes what's drawn, with ambient occlusion {occlusion:?}"
            );
        }
        let frame = eighth(&mut scene, Some(foggy));
        assert!(
            frame != without,
            "fog changes nothing, with ambient occlusion {occlusion:?}"
        );
        fogged.push(frame);
    }
    for x in [10, 48, 86] {
        let [without, with] =
            [&fogged[0], &fogged[1]].map(|frame| pixel_at(frame, SIZE[0], [x, 2]));
        assert!(
            close(with, without),
            "the fogged sky shows {with:?} with ambient occlusion, {without:?} without"
        );
    }
    shut_down(scene.renderer);
}

/// A scene of every kind of draw that fog fogs, as [`FoggedScene`] has it, in a low height fog,
/// lit by the sky and by the sun, in a lobe round it, as a camera with perspective sees it,
/// tonemapped by PBR Neutral, against its reference image.
fn fogged_scenes_match_their_reference() {
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 112;
    let mut scene = FoggedScene::new(WIDTH, HEIGHT);
    scene.settle();
    let fog = Fog {
        density: 0.06,
        falloff: 0.5,
        height: 0.0,
        start: 0.0,
        opacity: 1.0,
        color: Vec3::ONE,
        luminance: 1.0,
        sun: 1.0,
        sharpness: 0.65,
    };
    scene.render(Some(fog));
    check(scene.renderer, "fog", WIDTH, HEIGHT);
}

/// The unit way toward a sun `elevation` above the horizon, `azimuth` round from -Z toward +X.
fn toward_sun(elevation: f32, azimuth: f32) -> Vec3 {
    let (sin, cos) = (math::libm::sinf, math::libm::cosf);
    Vec3::new(
        cos(elevation) * sin(azimuth),
        sin(elevation),
        -cos(elevation) * cos(azimuth),
    )
}

/// Earth's atmosphere at noon and at sunset, seen from 2 m above a gray floor 20 km across, which
/// its haze hides toward the horizon, under a sun of 120,000 lux outside the air, which casts
/// shadows of three spheres before the view, a dielectric, a metal and a rough white one, with a
/// white wall 600 m off, hazed, tonemapped by PBR Neutral, against their reference images: at noon
/// a blue sky, paler toward the horizon, the world lit white; at sunset an orange glow round the
/// sun, low ahead, the world lit red and dim, and the sky's light from all around bluer than the
/// sun's. The view's ambient occlusion darkens the atmosphere's light from all around, as it does
/// an image sky's.
fn atmospheres_match_their_references() {
    const WIDTH: u32 = 192;
    const HEIGHT: u32 = 108;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores {
        atmosphere: Some(Atmosphere::EARTH),
        ..Stores::default()
    };
    let square = model_square(&mut stores.meshes);
    let sphere = sphere(&mut stores.meshes, 0.5, 24, 48);
    let [gray, red, gold, white] = [
        (Vec4::new(0.5, 0.5, 0.5, 1.0), 0.0, 0.9),
        (Vec4::new(0.8, 0.1, 0.1, 1.0), 0.0, 0.3),
        (Vec4::new(1.0, 0.78, 0.34, 1.0), 1.0, 0.25),
        (Vec4::ONE, 0.0, 1.0),
    ]
    .map(|(color, metal, roughness)| {
        stores.materials.add(&Material {
            color,
            workflow: metallic(metal, roughness),
            ..Material::default()
        })
    });
    let mut draw = |mesh, material| {
        MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
    };
    let floor = draw(square, gray);
    let plastic = draw(sphere, red);
    let metal = draw(sphere, gold);
    let rough = draw(sphere, white);
    // A wall facing the view, as the square faces +Z.
    let wall = draw(square, white);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(20_000.0, Vec3::ZERO), &floor, 0);
    for (x, draw) in [(-1.3, &plastic), (0.0, &metal), (1.3, &rough)] {
        let at = Vec3::new(x, 0.5, -4.0 - x.abs() * 0.5);
        draws.push_draw(Mat4::from_translation(at), draw, 0);
    }
    let tall = Mat4::from_scale_rotation_translation(
        Vec3::new(60.0, 160.0, 1.0),
        Quat::IDENTITY,
        Vec3::new(-150.0, 80.0, -600.0),
    );
    draws.push_draw(tall, &wall, 0);
    let viewport = filling([WIDTH, HEIGHT]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.0, 0.0),
        rotation: Quat::from_rotation_x(0.12),
        projection: Projection::Perspective {
            fov: 1.3,
            near: 0.1,
        },
    };
    let skies = [
        ("atmosphere_noon", toward_sun(1.1, -0.9), 15.0),
        ("atmosphere_sunset", toward_sun(0.03, 0.3), 12.0),
    ];
    for (name, to_sun, ev100) in skies {
        stores.lights = vec![Light {
            color: Vec3::splat(120_000.0),
            ..sun(-to_sun, Some(30.0))
        }];
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let data = RenderData {
            tonemapper: Tonemapper::PbrNeutral,
            exposure: 1.0 / (1.2 * math::libm::exp2f(ev100)),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let plain = frame_pixels(&mut renderer);
        compare(name, WIDTH, HEIGHT, &plain);
        // The view's ambient occlusion darkens the atmosphere's light too, as an image sky's.
        let data = RenderData {
            tonemapper: Tonemapper::PbrNeutral,
            exposure: 1.0 / (1.2 * math::libm::exp2f(ev100)),
            ambient_occlusion: Some(AmbientOcclusion::default()),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let occluded = frame_pixels(&mut renderer);
        let darker = (plain.iter().zip(&occluded))
            .filter(|(was, now)| now < was)
            .count();
        assert!(
            darker > 100,
            "{name}: ambient occlusion darkened {darker} channels"
        );
    }
    shut_down(renderer);
}

/// The optical depth of Earth's atmosphere, as the paper models it, from `height` meters up along
/// a way of zenith cosine `mu` out of the air: its Rayleigh scattering, Mie extinction and ozone
/// absorption, in millionths per meter, thinning with height as `Atmosphere` says, summed by the
/// midpoints of 20,000 steps.
fn optical_depth(height: f32, mu: f32) -> Vec3 {
    let (ground, top) = (6_360_000.0_f64, 6_460_000.0_f64);
    let earth = Atmosphere::EARTH;
    let from = ground + f64::from(height);
    let mu = f64::from(mu);
    // To where the way leaves the atmosphere's top.
    let reach = -from * mu + (from * from * (mu * mu - 1.0) + top * top).sqrt();
    let steps = 20_000;
    let step = reach / f64::from(steps);
    let mut depth = [0.0_f64; 3];
    for i in 0..steps {
        let t = (f64::from(i) + 0.5) * step;
        let radius = (from * from + t * t + 2.0 * from * t * mu).sqrt();
        let up = (radius - ground) / 1000.0;
        let rayleigh = math::libm::exp(-up / 8.0);
        let mie = math::libm::exp(-up / 1.2);
        let ozone = (1.0 - (up - 25.0).abs() / 15.0).clamp(0.0, 1.0);
        for (channel, depth) in depth.iter_mut().enumerate() {
            let haze = f64::from(earth.mie + earth.mie_absorption);
            *depth += (f64::from(earth.rayleigh[channel]) * rayleigh
                + haze * mie
                + f64::from(earth.ozone[channel]) * ozone)
                * 1.0e-6
                * step;
        }
    }
    Vec3::new(depth[0] as f32, depth[1] as f32, depth[2] as f32)
}

/// The sun's disk, seen through a view narrower than it from 1 km up, at the zenith, 60° from it
/// and 85°, under a light of the illuminance that makes the disk's luminance 1 outside the air:
/// shows the transmittance toward it, as the paper's model gives it, which the GPU's table holds.
fn the_suns_disk_shows_the_transmittance() {
    const SIZE: u32 = 4;
    let mut renderer = renderer(SIZE, SIZE);
    let earth = Atmosphere::EARTH;
    let mut stores = Stores {
        atmosphere: Some(earth),
        ..Stores::default()
    };
    let half = earth.sun_size / 2.0;
    let solid_angle = 2.0 * PI * (1.0 - math::libm::cosf(half));
    let draws = Draws::default();
    let viewport = filling([SIZE, SIZE]);
    for zenith in [0.0_f32, 60.0, 85.0] {
        let to_sun = toward_sun(FRAC_PI_2 - math::to_radians(zenith), 0.0);
        stores.lights = vec![Light {
            color: Vec3::splat(solid_angle),
            ..sun(-to_sun, None)
        }];
        let viewpoint = Viewpoint {
            position: DVec3::new(0.0, 1000.0, 0.0),
            rotation: Quat::from_rotation_arc(Vec3::NEG_Z, to_sun),
            projection: Projection::Perspective {
                fov: half / 2.0,
                near: 0.5,
            },
        };
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let depth = optical_depth(1000.0, to_sun.y);
        let through = [depth.x, depth.y, depth.z].map(|depth| math::libm::expf(-depth));
        let got = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
        let [r, g, b] = through.map(encoded);
        let near = (got.iter().zip([r, g, b, 255])).all(|(&got, want)| got.abs_diff(want) <= 2);
        assert!(
            near,
            "{zenith}° from the zenith, the sun's disk shows {got:?}, not the transmittance \
             {through:?} encoded, {:?}",
            [r, g, b]
        );
    }
    shut_down(renderer);
}

/// Along a level way from 2 m up, `far` meters through Earth's atmosphere as the paper models it,
/// summed in doubles by the midpoints of 20,000 steps: the mean of the channels' transmittance;
/// the light scattered once toward the start, of each channel, from a sun of an illuminance of 1
/// at the zenith, a quarter turn from the way, by Rayleigh's and Mie's phase functions, the sun's
/// light dimmed along the way down to each point as it is straight up from the ground; and the
/// light of the ground below, of the atmosphere's albedo, lit by that sun, scattered once toward
/// the start, half of every way round it, whatever the phase function.
fn level_way(far: f32) -> (f32, Vec3, Vec3) {
    let (ground, earth) = (6_360_000.0_f64, Atmosphere::EARTH);
    let from = ground + 2.0;
    let steps = 20_000;
    let step = f64::from(far) / f64::from(steps);
    let down = optical_depth(2.0, 1.0);
    let anisotropy = f64::from(earth.mie_anisotropy);
    let albedo = f64::from(earth.ground_albedo.x);
    let rayleigh_phase = 3.0 / (16.0 * std::f64::consts::PI);
    let mie_phase = (1.0 - anisotropy * anisotropy)
        / (4.0 * std::f64::consts::PI * math::libm::pow(1.0 + anisotropy * anisotropy, 1.5));
    let mut depth = [0.0_f64; 3];
    let (mut from_sun, mut from_ground) = ([0.0_f64; 3], [0.0_f64; 3]);
    for i in 0..steps {
        let t = (f64::from(i) + 0.5) * step;
        let up = ((from * from + t * t).sqrt() - ground) / 1000.0;
        let rayleigh = math::libm::exp(-up / 8.0);
        let mie = math::libm::exp(-up / 1.2);
        let ozone = (1.0 - (up - 25.0).abs() / 15.0).clamp(0.0, 1.0);
        for channel in 0..3 {
            let haze = f64::from(earth.mie + earth.mie_absorption);
            let extinction = (f64::from(earth.rayleigh[channel]) * rayleigh
                + haze * mie
                + f64::from(earth.ozone[channel]) * ozone)
                * 1.0e-6;
            let through = math::libm::exp(-depth[channel] - extinction * step / 2.0);
            let sunlit = math::libm::exp(-f64::from(down[channel]));
            let (rayleigh, mie) = (
                f64::from(earth.rayleigh[channel]) * 1.0e-6 * rayleigh,
                f64::from(earth.mie) * 1.0e-6 * mie,
            );
            let phased = rayleigh * rayleigh_phase + mie * mie_phase;
            // The ground's light, from below, half of every way round the level way, whatever
            // the phase function.
            let ground = albedo / std::f64::consts::PI * sunlit * 0.5 * (rayleigh + mie);
            from_sun[channel] += through * phased * sunlit * step;
            from_ground[channel] += through * ground * step;
            depth[channel] += extinction * step;
        }
    }
    let mean = depth
        .iter()
        .map(|depth| math::libm::exp(-depth))
        .sum::<f64>()
        / 3.0;
    let light = |light: [f64; 3]| Vec3::from_array(light.map(|light| light as f32));
    (mean as f32, light(from_sun), light(from_ground))
}

/// The haze over what's far, through views narrower than walls 5 and 20 km off from 2 m up: an
/// emissive white wall under a sun set far below the horizon, whose sky is dark, shows the mean of
/// the transmittance's channels to it, as the paper's model gives it, within 8%; and a black wall
/// under a sun at the zenith shows the light scattered toward the eye on the way, bluest and
/// brighter the farther it is: more than the light scattered once from the sun, as the model gives
/// it, as the light scattered many times over adds to it, but within a tenth past that and the
/// sunlit ground's light scattered once, which the paper's light scattered many times over holds.
fn the_haze_dims_and_lights_what_is_far() {
    const SIZE: u32 = 4;
    const SUN: f32 = 120_000.0;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores {
        atmosphere: Some(Atmosphere::EARTH),
        ..Stores::default()
    };
    let square = model_square(&mut stores.meshes);
    let [glowing, black] = [
        Material {
            color: Vec4::new(0.0, 0.0, 0.0, 1.0),
            workflow: metallic(0.0, 1.0),
            emissive: Vec3::ONE,
            ..Material::default()
        },
        // A metal of no color, which reflects nothing either way.
        Material {
            color: Vec4::new(0.0, 0.0, 0.0, 1.0),
            workflow: metallic(1.0, 1.0),
            ..Material::default()
        },
    ]
    .map(|material| {
        let material = stores.materials.add(&material);
        MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders)
    });
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.0, 0.0),
        rotation: Quat::IDENTITY,
        projection: Projection::Perspective {
            fov: 0.05,
            near: 0.5,
        },
    };
    let mut shows = |stores: &mut Stores, wall: &MeshDraw, far: f32, exposure: f32| {
        let mut draws = Draws::default();
        let placed = Mat4::from_scale_rotation_translation(
            Vec3::new(far, far, 1.0),
            Quat::IDENTITY,
            Vec3::new(0.0, 2.0, -far),
        );
        draws.push_draw(placed, wall, 0);
        settle(&mut renderer, stores, viewport, viewpoint, &draws);
        let data = RenderData {
            exposure,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let pixel = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
        Vec3::from_array([pixel[0], pixel[1], pixel[2]].map(decoded))
    };
    let mut lights = Vec::new();
    for far in [5000.0, 20_000.0] {
        let (through, from_sun, from_ground) = level_way(far);
        stores.lights = vec![Light {
            color: Vec3::splat(SUN),
            ..sun(-toward_sun(-0.5, 0.0), None)
        }];
        let shown = shows(&mut stores, &glowing, far, 1.0);
        let mean = shown.element_sum() / 3.0;
        assert!(
            (mean / through - 1.0).abs() < 0.08,
            "{far} m off, the glowing wall shows {shown}, not the transmittance {through}"
        );
        stores.lights = vec![Light {
            color: Vec3::splat(SUN),
            ..sun(Vec3::NEG_Y, None)
        }];
        // Exposed to show the light scattered once from the sun and the ground at a half.
        let most = from_sun + from_ground;
        let exposure = 0.5 / (most.max_element() * SUN);
        let scattered = shows(&mut stores, &black, far, exposure) / (exposure * SUN);
        assert!(
            scattered.cmpge(from_sun).all() && scattered.cmple(1.1 * most).all(),
            "{far} m off, the black wall shows {scattered} of the light scattered toward it, not \
             between the light scattered once from the sun, {from_sun}, and that and the sunlit \
             ground's, {most}"
        );
        assert!(
            scattered.z > scattered.y && scattered.y > scattered.x,
            "{scattered}"
        );
        lights.push(scattered);
    }
    assert!(lights[1].cmpgt(lights[0]).all(), "{lights:?}");
    shut_down(renderer);
}

/// A white floor seen from above, lit by the sky's light alone through dusk, its sun 0.5° to 4.5°
/// below the horizon in steps of 0.05°, as `atmosphere.ron`'s sun, turning 3° a second, goes in
/// frames of 60 a second: its light, which falls by about half each degree, falls frame by frame
/// as smoothly as the sun goes, each frame's fall within 6% of the light of the one before's, which
/// allows for the steps of about 1% of the 8-bit image it's read from, and for the bends in the
/// light as the sun crosses the multiple scattering table's texels, rather than in steps of tens of
/// percent where the light would be prefiltered again too seldom. Each frame is exposed for the
/// light the frame before showed, and its light found by its exposure.
fn the_skys_light_falls_smoothly_through_dusk() {
    const SIZE: u32 = 4;
    const STEPS: u32 = 80;
    const STEP: f32 = 0.05;
    // What each frame's exposure is for, where the 8-bit image's steps are finest, relatively.
    const SHOWN: f32 = 0.8;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores {
        atmosphere: Some(Atmosphere::EARTH),
        ..Stores::default()
    };
    let square = model_square(&mut stores.meshes);
    let white = stores.materials.add(&Material {
        color: Vec4::ONE,
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    });
    let floor = MeshDraw::with_material(square, white, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(20.0, Vec3::ZERO), &floor, 0);
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 3.0, 0.0),
        rotation: Quat::from_rotation_x(-FRAC_PI_2),
        projection: Projection::Perspective {
            fov: 0.2,
            near: 0.1,
        },
    };
    let at = |stores: &mut Stores, elevation: f32| {
        let to_sun = toward_sun(math::to_radians(elevation), 0.0);
        stores.lights = vec![Light {
            color: Vec3::splat(120_000.0),
            ..sun(-to_sun, None)
        }];
    };
    at(&mut stores, -0.5);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut exposure = 1.0e-3;
    let mut lights = Vec::new();
    for step in 0..=STEPS {
        let elevation = -0.5 - STEP * step as f32;
        at(&mut stores, elevation);
        // The first exposed afresh until it shows.
        let tries = if step == 0 { 6 } else { 1 };
        let mut shown = 0.0;
        for _ in 0..tries {
            let data = RenderData {
                exposure,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(&mut renderer, data);
            let pixel = pixel_at(&frame_pixels(&mut renderer), SIZE, [SIZE / 2, SIZE / 2]);
            shown = decoded(pixel[1]);
            if tries > 1 {
                exposure *= SHOWN / shown.clamp(1.0e-3, 1.0);
            }
        }
        assert!(
            (0.3..0.99).contains(&shown),
            "at {elevation}°, the floor shows {shown}, out of the range its exposure was for"
        );
        lights.push(shown / exposure);
        exposure *= SHOWN / shown;
    }
    let falls: Vec<f32> = (lights.windows(2))
        .map(|pair| math::libm::logf(pair[1] / pair[0]))
        .collect();
    for (i, pair) in falls.windows(2).enumerate() {
        let elevation = -0.5 - STEP * (i + 2) as f32;
        assert!(
            (pair[1] - pair[0]).abs() < 0.06,
            "at {elevation}°, the floor's light fell by {:.1}% where the frame before it fell by \
             {:.1}%: it stepped",
            100.0 * (1.0 - math::libm::expf(pair[1])),
            100.0 * (1.0 - math::libm::expf(pair[0]))
        );
    }
    shut_down(renderer);
}
/// An image sky lights and shows its world byte for byte as it did before an atmosphere took its
/// place, under the same sun, once the atmosphere gives it back: its light is prefiltered from the
/// image again, and the sun's light is no longer dimmed by the air.
fn an_image_sky_renders_as_it_did_before_an_atmosphere() {
    const SIZE: u32 = 32;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let sky = sky_texture(&mut stores.textures, [64, 32], |_, y| {
        if y < 16 {
            [0.4, 0.6, 1.0]
        } else {
            [0.15, 0.1, 0.05]
        }
    });
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 8000.0,
    });
    stores.lights = vec![Light {
        color: Vec3::splat(100_000.0),
        ..sun(Vec3::new(1.0, -1.0, -1.0), Some(20.0))
    }];
    let sphere = sphere(&mut stores.meshes, 0.6, 16, 32);
    let white = Material {
        color: Vec4::ONE,
        workflow: metallic(0.0, 0.5),
        ..Material::default()
    };
    let white = stores.materials.add(&white);
    let ball = MeshDraw::with_material(sphere, white, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    draws.push_draw(Mat4::IDENTITY, &ball, 0);
    let viewport = filling([SIZE, SIZE]);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.5, 3.0),
        projection: Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    let frame = |renderer: &mut Renderer, stores: &mut Stores| {
        let data = RenderData {
            tonemapper: Tonemapper::PbrNeutral,
            exposure: 1.0 / (1.2 * math::libm::exp2f(15.0)),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(renderer, data);
        frame_pixels(renderer)
    };
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let before = frame(&mut renderer, &mut stores);
    stores.atmosphere = Some(Atmosphere::EARTH);
    frame(&mut renderer, &mut stores);
    let aired = frame(&mut renderer, &mut stores);
    assert_ne!(aired, before, "the atmosphere shows nothing of its own");
    stores.atmosphere = None;
    frame(&mut renderer, &mut stores);
    let after = frame(&mut renderer, &mut stores);
    assert!(
        after == before,
        "the image sky's frame differs after the atmosphere gave it back"
    );
    shut_down(renderer);
}

/// The bytes of a shadow map of a light's cascade: 2048 texels across and down, of 32-bit depth.
const SHADOW_MAP_BYTES: u64 = 2048 * 2048 * 4;

/// What places [`model_square`] facing up, `size` across, its center at `at`.
fn facing_up(size: f32, at: Vec3) -> Mat4 {
    let turned = Quat::from_rotation_x(-FRAC_PI_2);
    Mat4::from_scale_rotation_translation(Vec3::new(size, size, 1.0), turned, at)
}

/// A directional light of white, of 1 lux, shining along `direction`, casting shadows as far as
/// `shadow_distance` ahead of the view, if given.
fn sun(direction: Vec3, shadow_distance: Option<f32>) -> Light {
    Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: direction.normalize(),
        color: Vec3::ONE,
        range: None,
        shadow_distance,
    }
}

/// Straight down from `at`, without perspective, onto a square view `across` units of the world
/// across, its top toward -Z, seeing what's from 1 to `far` below it.
fn looking_down(at: Vec3, across: f32, far: f32) -> Viewpoint {
    Viewpoint {
        position: at.as_dvec3(),
        rotation: Quat::from_rotation_x(-FRAC_PI_2),
        projection: Projection::Orthographic {
            height: across,
            near: 1.0,
            far,
        },
    }
}

/// A floor, white and rough, 20 units across, and the first `plates` of three plates a unit across,
/// 2 above it, along Z at -2.5, 0 and 2.5 and at -2 along X, each facing up: opaque, masked and
/// opaque, and masked and transparent, which draws nothing.
fn floor_and_plates(stores: &mut Stores, plates: usize) -> Draws {
    let square = model_square(&mut stores.meshes);
    let white = Material {
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let masked = AlphaMode::Mask { cutoff: 0.5 };
    let materials = [
        white,
        white,
        Material {
            alpha: masked,
            ..white
        },
        Material {
            color: Vec4::new(1.0, 1.0, 1.0, 0.0),
            alpha: masked,
            ..white
        },
    ];
    let mut draws = Draws::default();
    let places = [
        facing_up(20.0, Vec3::ZERO),
        facing_up(1.0, Vec3::new(-2.0, 2.0, -2.5)),
        facing_up(1.0, Vec3::new(-2.0, 2.0, 0.0)),
        facing_up(1.0, Vec3::new(-2.0, 2.0, 2.5)),
    ];
    for (material, place) in materials.iter().zip(places).take(1 + plates) {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(place, &draw, 0);
    }
    draws
}

/// The floor and plates of [`floor_and_plates`], seen from straight above without perspective,
/// 16 pixels a unit, under a light that comes down 2 for each 1 it goes along +X, casting shadows
/// 100 units ahead: the light reaches the floor round the plates, as it does without the shadows,
/// but not where the opaque and the masked opaque ones put their shadows, from a unit along +X
/// from each, as far across as they are; the transparent one casts none. The frames' own images
/// take the four shadow maps' memory more only in a frame whose light casts them; synchronization
/// validation sees each map drawn before the view samples it, and drawn again after.
fn shadows_fall_where_their_casters_put_them() {
    const SIZE: u32 = 128;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let draws = floor_and_plates(&mut stores, 3);
    let viewpoint = looking_down(Vec3::new(0.0, 5.0, 0.0), 8.0, 21.0);
    let viewport = filling([SIZE, SIZE]);
    let direction = Vec3::new(1.0, -2.0, 0.0);
    stores.lights = vec![sun(direction, None)];
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let unshadowed = renderer.transient_bytes();
    let lit = frame_pixels(&mut renderer);
    stores.lights = vec![sun(direction, Some(100.0))];
    for _ in 0..3 {
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    }
    let shadowed = renderer.transient_bytes();
    let frame = frame_pixels(&mut renderer);
    assert!(
        unshadowed < SHADOW_MAP_BYTES && shadowed >= 4 * SHADOW_MAP_BYTES,
        "{unshadowed} bytes without the shadows, {shadowed} with"
    );
    // The floor's pixel `x` along X and `z` along Z from the origin, at the view's center.
    let at = |x: f32, z: f32| [(64.0 + 16.0 * x) as u32, (64.0 + 16.0 * z) as u32];
    let floor = pixel_at(&lit, SIZE, at(2.5, 0.0));
    assert!(floor[0] > 100 && floor[0] == floor[2], "{floor:?}");
    let dark = [0, 0, 0, 255];
    for (plate, z) in [-2.5, 0.0, 2.5].into_iter().enumerate() {
        let casts = plate < 2;
        let mut checks = vec![
            ((2.5, z), floor),
            ((-0.4, z), floor),
            ((-1.0, z + 0.6), floor),
            ((-1.0, z - 0.6), floor),
        ];
        for in_shadow in [
            (-1.4, z),
            (-1.0, z),
            (-0.6, z),
            (-1.0, z + 0.4),
            (-1.0, z - 0.4),
        ] {
            checks.push((in_shadow, if casts { dark } else { floor }));
        }
        for ((x, z), expected) in checks {
            let found = pixel_at(&frame, SIZE, at(x, z));
            assert!(
                close(found, expected),
                "plate {plate}: the floor at ({x}, {z}) is {found:?}, not {expected:?}"
            );
        }
    }
    shut_down(renderer);
}

/// A floor seen from straight above without perspective, 16 pixels a unit, 8 units across, under a
/// light that comes down 2 for each 1 it goes along +X, casting shadows 100 units ahead, and plates
/// a unit across over it, of a mesh with a sphere round its vertices: one in the view, which
/// draws and casts its shadow; one past the view's side, which the view culls but the shadows'
/// cascades keep, so that its shadow falls in the view; one 200 units away, which both cull; and
/// a red one across the view's other side, which draws where it shows. And a red square of a mesh
/// whose vertices are 10 units from its own origin, which is outside the view, its sphere round
/// its vertices there: it draws. What the view counts on the GPU leaves out those it culled.
fn culling_keeps_what_each_view_sees() {
    const SIZE: u32 = 128;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let corners = |offset: f32| {
        let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
            position: Vec3::new(x + offset, y, 0.0),
            uv: Vec2::from(uv),
            color: Vec4::ONE,
            normal: Vec3::Z,
            tangent: Vec4::new(1.0, 0.0, 0.0, 1.0),
        };
        [
            corner([-0.5, -0.5], [0.0, 1.0]),
            corner([0.5, -0.5], [1.0, 1.0]),
            corner([0.5, 0.5], [1.0, 0.0]),
            corner([-0.5, 0.5], [0.0, 0.0]),
        ]
    };
    let indices = [0, 1, 2, 2, 3, 0];
    let floor = model_square(&mut stores.meshes);
    let plate = add_model(&mut stores.meshes, "plate", &corners(0.0), &indices);
    let away = add_model(&mut stores.meshes, "away", &corners(10.0), &indices);
    let white = Material {
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let red = Material {
        color: Vec4::new(1.0, 0.0, 0.0, 1.0),
        ..white
    };
    let mut draw = |mesh, material: &Material| {
        let material = stores.materials.add(material);
        MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
    };
    let (floor, white_plate, red_plate, away) = (
        draw(floor, &white),
        draw(plate, &white),
        draw(plate, &red),
        draw(away, &red),
    );
    let mut draws = Draws::default();
    draws.push_draw(facing_up(20.0, Vec3::ZERO), &floor, 0);
    draws.push_draw(facing_up(1.0, Vec3::new(-2.0, 2.0, -2.0)), &white_plate, 0);
    draws.push_draw(facing_up(1.0, Vec3::new(-4.75, 2.0, 1.0)), &white_plate, 0);
    draws.push_draw(facing_up(1.0, Vec3::new(200.0, 2.0, 0.0)), &white_plate, 0);
    draws.push_draw(facing_up(1.0, Vec3::new(4.25, 2.0, 3.0)), &red_plate, 0);
    draws.push_draw(facing_up(1.0, Vec3::new(-10.0, 1.0, 2.5)), &away, 0);
    let viewpoint = looking_down(Vec3::new(0.0, 5.0, 0.0), 8.0, 21.0);
    let viewport = filling([SIZE, SIZE]);
    stores.lights = vec![sun(Vec3::new(1.0, -2.0, 0.0), Some(100.0))];
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let drawn = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // The floor, the plate in the view and the one across its side, of one run, and the square.
    let kept = DrawCounts {
        draws: 3,
        instances: 4,
        triangles: 8,
    };
    assert_eq!(drawn, kept);
    let frame = frame_pixels(&mut renderer);
    shut_down(renderer);
    // The floor's pixel `x` along X and `z` along Z from the origin, at the view's center.
    let at = |x: f32, z: f32| {
        pixel_at(
            &frame,
            SIZE,
            [(64.0 + 16.0 * x) as u32, (64.0 + 16.0 * z) as u32],
        )
    };
    let floor = at(2.0, -3.0);
    assert!(floor[0] > 100 && floor[0] == floor[2], "{floor:?}");
    let dark = [0, 0, 0, 255];
    let shadows = [
        ((-1.0, -2.0), "the plate's in the view"),
        ((-3.6, 1.0), "the plate's past it"),
    ];
    for ((x, z), what) in shadows {
        let found = at(x, z);
        assert!(
            close(found, dark),
            "{what} shadow at ({x}, {z}) is {found:?}"
        );
    }
    for ((x, z), what) in [
        ((3.9, 3.0), "the plate across the side"),
        ((0.0, 2.5), "the square"),
    ] {
        let found = at(x, z);
        assert!(
            found[0] > 100 && found[1] < 40 && found[2] < 40,
            "{what} at ({x}, {z}) is {found:?}, not red"
        );
    }
}

/// The floor and the plates of [`culling_keeps_what_each_view_sees`], white but one red, in two
/// frames, the second moving one plate, taking one away and adding one: held by the renderer's
/// scene, drawn where the draws place it, they draw the same frames, bit for bit, culled as the
/// same draws pushed are, counting the same.
fn the_scene_draws_as_draws_do() {
    const SIZE: u32 = 128;
    // Each instance's slot, which its ID is one past, and where it is across the floor, in each
    // frame: the floor's slot is 9.
    let frames: [&[(u32, f32, f32)]; 2] = [
        &[
            (0, -2.0, -2.0),
            (1, -4.75, 1.0),
            (2, 200.0, 0.0),
            (3, 4.25, 3.0),
        ],
        &[
            (0, -1.0, -2.0),
            (2, 200.0, 0.0),
            (3, 4.25, 3.0),
            (5, 1.0, 1.5),
        ],
    ];
    let render = |through_scene: bool| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let floor = model_square(&mut stores.meshes);
        let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
            position: Vec3::new(x, y, 0.0),
            uv: Vec2::from(uv),
            color: Vec4::ONE,
            normal: Vec3::Z,
            tangent: Vec4::new(1.0, 0.0, 0.0, 1.0),
        };
        let corners = [
            corner([-0.5, -0.5], [0.0, 1.0]),
            corner([0.5, -0.5], [1.0, 1.0]),
            corner([0.5, 0.5], [1.0, 0.0]),
            corner([-0.5, 0.5], [0.0, 0.0]),
        ];
        let plate = add_model(&mut stores.meshes, "plate", &corners, &[0, 1, 2, 2, 3, 0]);
        let white = Material {
            workflow: metallic(0.0, 1.0),
            ..Material::default()
        };
        let red = Material {
            color: Vec4::new(1.0, 0.0, 0.0, 1.0),
            ..white
        };
        let mut draw = |mesh, material: &Material| {
            let material = stores.materials.add(material);
            MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
        };
        let (floor, white_plate, red_plate) =
            (draw(floor, &white), draw(plate, &white), draw(plate, &red));
        let viewpoint = looking_down(Vec3::new(0.0, 5.0, 0.0), 8.0, 21.0);
        let viewport = filling([SIZE, SIZE]);
        stores.lights = vec![sun(Vec3::new(1.0, -2.0, 0.0), Some(100.0))];
        let mut drawn = Vec::new();
        for (i, placed) in frames.iter().enumerate() {
            let mut instances = vec![(9, facing_up(20.0, Vec3::ZERO), floor)];
            for &(slot, x, z) in placed.iter() {
                let draw = if slot == 3 { red_plate } else { white_plate };
                instances.push((slot, facing_up(1.0, Vec3::new(x, 2.0, z)), draw));
            }
            let mut draws = Draws::default();
            if through_scene {
                draws.draw_scene();
                if i == 1 {
                    stores.scene.remove(1);
                }
                for &(slot, at, draw) in &instances {
                    stores.scene.set(slot, at, &draw, u64::from(slot) + 1);
                }
            } else {
                for &(slot, at, draw) in &instances {
                    draws.push_draw(at, &draw, u64::from(slot) + 1);
                }
            }
            settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
            let counts = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
            drawn.push((counts, frame_pixels(&mut renderer)));
        }
        shut_down(renderer);
        drawn
    };
    let (scene, draws) = (render(true), render(false));
    for (i, ((scene_counts, scene), (draws_counts, draws))) in scene.iter().zip(&draws).enumerate()
    {
        assert_eq!(scene_counts, draws_counts, "frame {i}'s counts");
        assert!(scene == draws, "frame {i}'s pixels differ");
    }
}

/// A wall across the left of a view with perspective, 128 pixels a side, of a mesh without a
/// sphere, never culled, and plates a unit across of a mesh with one, flat-colored: a red plate
/// wholly behind the wall, which the depth pyramid built from the view's first pass hides, so that
/// the view doesn't draw it; a green one beside the wall and a blue one half behind its edge,
/// which it draws; and a yellow one in front of it. The frame the wall is taken away draws the red
/// plate, bit for bit as a renderer that never saw the wall draws them all; and once the wall is
/// back, the frames are as before it went, the red plate hidden again and counted out.
fn occlusion_culling_draws_what_shows() {
    occlusion_culled(false);
}

/// As [`occlusion_culling_draws_what_shows`], under a sky, with ambient occlusion, whose depth
/// alone the view's two passes draw before it draws again, shaded, without a visibility buffer.
fn occlusion_culling_draws_what_shows_with_ambient_occlusion() {
    occlusion_culled(true);
}

/// The frames of [`occlusion_culling_draws_what_shows`], under a sky of 0.5 every way, with
/// ambient occlusion, if `occluded`.
fn occlusion_culled(occluded: bool) {
    const SIZE: u32 = 128;
    let viewport = filling([SIZE, SIZE]);
    // 5 in front of the origin, seeing a sixth of a turn across: 22 pixels a unit at z = 0, the
    // wall's, and 14 at z = -3, the plates'.
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    // What a renderer draws with, and the frames with the wall and without.
    let made = || {
        let mut stores = Stores::default();
        if occluded {
            let sky = sky_texture(&mut stores.textures, [64, 32], |_, _| [0.5; 3]);
            stores.sky = Some(Sky {
                texture: sky,
                luminance: 1.0,
            });
            stores.ambient_occlusion = Some(AmbientOcclusion::default());
        }
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let wall = square(&mut stores.meshes);
        let corners = [
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ];
        let whole = Lod {
            start: 0,
            count: 6,
            error: 0.0,
        };
        let sphere = Lods {
            levels: vec![whole],
            center: Vec3::ZERO,
            // Half the square's diagonal, to its corners.
            radius: std::f32::consts::FRAC_1_SQRT_2,
        };
        let plate = (stores.meshes)
            .add_lods("plate", &corners, &[0, 1, 2, 2, 3, 0], sphere)
            .expect("a mesh");
        let scene = |walled: bool| {
            let mut draws = Draws::default();
            if walled {
                // From past the view's left edge to 0.5 along X, and past its bottom and top.
                let across = Mat4::from_scale_rotation_translation(
                    Vec3::new(4.0, 6.0, 1.0),
                    Quat::IDENTITY,
                    Vec3::new(-1.5, 0.0, 0.0),
                );
                draws.push(wall, flat, across, Vec4::new(0.5, 0.5, 0.5, 1.0));
            }
            // Behind the wall; beside it; half behind its edge, which the line from the camera
            // past it, 0.5 along X at z = 0, meets at 0.8 along X at z = -3; and in front of it.
            for (x, z, color) in [
                (-2.0, -3.0, Vec4::new(0.8, 0.1, 0.1, 1.0)),
                (2.5, -3.0, Vec4::new(0.1, 0.7, 0.1, 1.0)),
                (0.8, -3.0, Vec4::new(0.1, 0.2, 0.8, 1.0)),
                (-1.5, 2.0, Vec4::new(0.8, 0.7, 0.1, 1.0)),
            ] {
                draws.push(
                    plate,
                    flat,
                    Mat4::from_translation(Vec3::new(x, 0.0, z)),
                    color,
                );
            }
            draws
        };
        (stores, scene(true), scene(false))
    };
    let (mut fresh, mut renderer) = (renderer(SIZE, SIZE), renderer(SIZE, SIZE));
    let (mut stores, walled, open) = made();
    // The culling's first frame finds nothing shown before it, so its second pass draws all, the
    // red plate among them; the next frame's first pass draws all the first showed, and its second
    // finds the red plate hidden; the frame after draws it in neither.
    settle(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    let drawn = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    // The wall, and of one run the plates but the red.
    let kept = DrawCounts {
        draws: 2,
        instances: 4,
        triangles: 8,
    };
    assert_eq!(drawn, kept, "what the wall hides is drawn");
    let hidden = frame_pixels(&mut renderer);
    // The red plate's middle, and the blue one's right of the wall's edge.
    let (red, blue) = ([36, 64], [79, 64]);
    let gray = [encoded(0.5), encoded(0.5), encoded(0.5), 255];
    let found = pixel_at(&hidden, SIZE, red);
    assert!(
        close(found, gray),
        "the wall over the red plate is {found:?}"
    );
    let found = pixel_at(&hidden, SIZE, blue);
    let blue_shown = [encoded(0.1), encoded(0.2), encoded(0.8), 255];
    assert!(close(found, blue_shown), "the blue plate is {found:?}");
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &open));
    let shown = frame_pixels(&mut renderer);
    let found = pixel_at(&shown, SIZE, red);
    let red_shown = [encoded(0.8), encoded(0.1), encoded(0.1), 255];
    assert!(close(found, red_shown), "the red plate is {found:?}");
    let (mut fresh_stores, _, open) = made();
    settle(&mut fresh, &mut fresh_stores, viewport, viewpoint, &open);
    if occluded {
        // The sky shows from the frame after the one its texture came to be drawable in.
        render_without_ui(&mut fresh, fresh_stores.data(viewport, viewpoint, &open));
    }
    assert!(
        shown == frame_pixels(&mut fresh),
        "the frame the wall went differs from one that never saw it"
    );
    shut_down(fresh);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    let drawn = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &walled);
    assert_eq!(drawn, kept, "what the wall hides once it's back is drawn");
    assert!(
        frame_pixels(&mut renderer) == hidden,
        "the wall back draws otherwise than before it went"
    );
    shut_down(renderer);
}

/// Squares that blend, each pushed before an opaque square, seen without perspective: on the left,
/// one in front of its opaque square, and on the right, one behind. Opaque draws draw first, so the
/// one in front blends over the opaque square, as it would pushed after it, and the one behind is
/// hidden.
fn what_blends_goes_over_what_is_opaque_behind_it_in_any_order() {
    const SIZE: u32 = 64;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    // 16 pixels a unit, seeing from 2 behind the camera to 2 in front.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -2.0,
            far: 2.0,
        },
        ..Viewpoint::default()
    };
    let at = |x: f32, z: f32| Mat4::from_translation(Vec3::new(x, 0.0, z));
    let veil = Sprite {
        color: Vec4::new(1.0, 1.0, 1.0, 0.5),
        ..Sprite::default()
    };
    let red = Vec4::new(0.8, 0.1, 0.1, 1.0);
    let mut draws = Draws::default();
    draws.push_sprite(at(-1.0, 0.5), veil);
    draws.push(square, flat, at(-1.0, 0.0), red);
    draws.push_sprite(at(1.0, -0.5), veil);
    draws.push(square, flat, at(1.0, 0.0), red);
    settle(
        &mut renderer,
        &mut stores,
        filling([SIZE, SIZE]),
        viewpoint,
        &draws,
    );
    let frame = frame_pixels(&mut renderer);
    shut_down(renderer);
    let veiled = [encoded(0.9), encoded(0.55), encoded(0.55), 255];
    let found = pixel_at(&frame, SIZE, [16, 32]);
    assert!(close(found, veiled), "the square in front is {found:?}");
    let opaque = [encoded(0.8), encoded(0.1), encoded(0.1), 255];
    let found = pixel_at(&frame, SIZE, [48, 32]);
    assert!(close(found, opaque), "the square behind is {found:?}");
}

/// The floor and the opaque plate of [`floor_and_plates`], seen from straight above without
/// perspective, 16 pixels a unit, under a light that comes down from +X and +Z, casting shadows
/// as far as the view sees, 1,000 units, so that the nearest cascade's texels are 2 pixels across,
/// and the shadow's edges, filtered, a dozen: the view moved a pixel along X shows each pixel of
/// the frame before it a pixel over, the edges' too, as the cascade's texels stay put in the
/// world. Were the cascade to move with the view, by a fraction of a texel, the edges would change.
fn shadows_hold_still_as_the_view_moves() {
    const SIZE: u32 = 128;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let draws = floor_and_plates(&mut stores, 1);
    stores.lights = vec![sun(Vec3::new(1.0, -2.0, 0.7), Some(1000.0))];
    let viewport = filling([SIZE, SIZE]);
    let [before, after] = [0.0, 1.0 / 16.0].map(|x| {
        let viewpoint = looking_down(Vec3::new(x, 5.0, 0.0), 8.0, 1001.0);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        frame_pixels(&mut renderer)
    });
    let between = (before.as_chunks::<4>().0.iter())
        .filter(|pixel| pixel[0] > 8 && pixel[0] < 120)
        .count();
    assert!(between > 20, "{between} pixels of the shadow's edges");
    for y in 0..SIZE {
        for x in 0..SIZE - 1 {
            let (was, is) = (
                pixel_at(&before, SIZE, [x + 1, y]),
                pixel_at(&after, SIZE, [x, y]),
            );
            assert!(
                was.iter().zip(is).all(|(&a, b)| a.abs_diff(b) <= 2),
                "({x}, {y}) shows {is:?}, where the frame before showed {was:?} a pixel over"
            );
        }
    }
    shut_down(renderer);
}

/// A floor 400 units across, seen with perspective from 2 above it, 25° down, under lights 0°,
/// 40°, 70° and 80° from straight down, casting shadows 50 units ahead: it draws as it does without
/// the shadows, every pixel, in every cascade and past them all, as it would not if it shadowed
/// itself, as shadow acne does.
fn surfaces_cast_no_shadows_on_themselves() {
    const SIZE: u32 = 160;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let square = model_square(&mut stores.meshes);
    let white = Material {
        workflow: metallic(0.0, 1.0),
        ..Material::default()
    };
    let material = stores.materials.add(&white);
    let floor = MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(400.0, Vec3::ZERO), &floor, 0);
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.0, 0.0),
        rotation: Quat::from_rotation_x(math::to_radians(-25.0)),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
    };
    let viewport = filling([SIZE, SIZE]);
    for degrees in [0.0_f32, 40.0, 70.0, 80.0] {
        let (sin, cos) = math::libm::sincosf(math::to_radians(degrees));
        // Turned a little round, so that the light's texels don't line up with the floor's edges.
        let direction = Vec3::new(sin * 0.8, -cos, sin * 0.6);
        let [lit, shadowed] = [None, Some(50.0)].map(|distance| {
            stores.lights = vec![sun(direction, distance)];
            settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
            frame_pixels(&mut renderer)
        });
        let differ = (lit.as_chunks::<4>().0.iter())
            .zip(shadowed.as_chunks::<4>().0)
            .filter(|(a, b)| a.iter().zip(*b).any(|(&a, &b)| a.abs_diff(b) > 1))
            .count();
        assert_eq!(
            differ, 0,
            "{differ} pixels under a light {degrees}° from straight down"
        );
    }
    shut_down(renderer);
}

/// A unit square around the origin, a model's, as [`model_square`] is, opaque at its left edge and
/// transparent at its right, as its vertices' colors' alpha says: a masked material of it draws its
/// left half.
fn half_square(meshes: &mut Meshes) -> Mesh<PackedVertex> {
    let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
        position: Vec3::new(x, y, 0.0),
        uv: Vec2::from(uv),
        color: Vec4::new(1.0, 1.0, 1.0, 0.5 - x),
        normal: Vec3::Z,
        tangent: Vec4::new(1.0, 0.0, 0.0, 1.0),
    };
    let corners = [
        corner([-0.5, -0.5], [0.0, 1.0]),
        corner([0.5, -0.5], [1.0, 1.0]),
        corner([0.5, 0.5], [1.0, 0.0]),
        corner([-0.5, 0.5], [0.0, 0.0]),
    ];
    add_model(meshes, "half square", &corners, &[0, 1, 2, 2, 3, 0])
}

/// Cuts each of `mesh`'s levels into meshlets as the cooker does, more simply: its triangles in
/// their order, a meshlet ending where the next triangle would take it past a meshlet's vertices or
/// triangles, each in the sphere round its vertices' box, facing within a cone round its
/// triangles' mean normal, as `meshopt_computeClusterBounds` makes one, widened for its bytes'
/// rounding.
fn cut_meshlets(mesh: &mut ModelMesh) {
    let positions = mesh.positions();
    let mut meshlets = Meshlets::default();
    for lod in &mesh.lods.levels {
        let start = lod.start as usize;
        let triangles = mesh.indices[start..start + lod.count as usize]
            .as_chunks::<3>()
            .0;
        let first = meshlets.meshlets.len() as u32;
        let mut rest = triangles;
        while !rest.is_empty() {
            let mut vertices: Vec<u32> = Vec::new();
            let mut taken = 0;
            for corners in rest {
                let new = corners.iter().filter(|c| !vertices.contains(c)).count();
                let full = vertices.len() + new > MAX_MESHLET_VERTICES as usize;
                if full || taken == MAX_MESHLET_TRIANGLES as usize {
                    break;
                }
                for &corner in corners {
                    if !vertices.contains(&corner) {
                        vertices.push(corner);
                    }
                }
                taken += 1;
            }
            let (cut, after) = rest.split_at(taken);
            rest = after;
            let at = |vertex: u32| positions[vertex as usize];
            let (low, high) = (vertices.iter())
                .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(low, high), &v| {
                    (low.min(at(v)), high.max(at(v)))
                });
            let center = (low + high) / 2.0;
            let radius = vertices
                .iter()
                .map(|&v| at(v).distance(center))
                .fold(0.0, f32::max);
            let normals: Vec<Vec3> = (cut.iter())
                .map(|&[a, b, c]| (at(b) - at(a)).cross(at(c) - at(a)).normalize())
                .collect();
            let axis = normals.iter().sum::<Vec3>().normalize_or_zero();
            let least = normals.iter().map(|n| n.dot(axis)).fold(1.0, f32::min);
            let cone = if least <= 0.1 {
                [0, 0, 0, 127]
            } else {
                let cutoff = (1.0 - least * least).sqrt() + 0.03;
                let [x, y, z] = (axis * 127.0).round().to_array().map(|c| c as i8);
                [x, y, z, (cutoff * 127.0).ceil().min(127.0) as i8]
            };
            let place = |vertex: u32| vertices.iter().position(|&v| v == vertex).unwrap() as u8;
            meshlets.meshlets.push(Meshlet {
                vertices: meshlets.vertices.len() as u32,
                vertex_count: vertices.len() as u32,
                triangles: meshlets.triangles.len() as u32,
                triangle_count: cut.len() as u32,
                center,
                radius,
                cone,
            });
            meshlets.vertices.extend_from_slice(&vertices);
            meshlets
                .triangles
                .extend(cut.iter().flatten().map(|&corner| place(corner)));
            // Each meshlet's triangles from a word, as the cooker's are.
            let padded = meshlets.triangles.len().next_multiple_of(4);
            meshlets.triangles.resize(padded, 0);
        }
        meshlets
            .levels
            .push([first, meshlets.meshlets.len() as u32 - first]);
    }
    mesh.meshlets = meshlets;
}

/// Spheres of a mesh of two levels of detail, cut into meshlets, on a floor, lit by a sun that casts
/// their shadows, seen with perspective: opaque and masked, near enough for their finest level and
/// far enough for the coarser, mirrored, stretched along one axis, which their cones don't turn
/// with, and part past the view's side, which culls some of their meshlets. Drawn by their
/// meshlets, through mesh shaders if `mesh_shaders` and through vertex shaders otherwise, and
/// shaded as they draw, they draw bit for bit as the same spheres of the mesh without meshlets, by
/// their indices, the same instances at the same levels, counting fewer triangles, as the meshlets
/// that face away from the view or lie past its sides are culled; and match their reference. Drawn
/// into the visibility buffer and shaded from it, they draw the same, and match it too.
fn meshlets_draw_as_their_indices_do(mesh_shaders: bool) {
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 128;
    let render = |meshlets: bool, visibility_buffer: bool| {
        let settings = Settings {
            mesh_shaders: Some(mesh_shaders),
            visibility_buffer,
            ..settings(None)
        };
        let mut renderer = Renderer::headless(&settings, WIDTH, HEIGHT).expect("a renderer");
        let mut stores = Stores::default();
        let floor = model_square(&mut stores.meshes);
        // Its finest level, then every other ring and segment of it.
        let (vertices, mut indices) = sphere_of(1.0, 24, 48);
        let finest = indices.len() as u32;
        let across = 49;
        for ring in (0..24).step_by(2) {
            for segment in (0..48).step_by(2) {
                let at = ring * across + segment;
                let (below, next) = (at + 2 * across, at + 2);
                indices.extend_from_slice(&[at, below, next, next, below, below + 2]);
            }
        }
        let coarse = indices.len() as u32 - finest;
        let levels = vec![
            Lod {
                start: 0,
                count: finest,
                error: 0.0,
            },
            Lod {
                start: finest,
                count: coarse,
                error: 0.03,
            },
        ];
        let mut ball = ModelMesh::pack("ball", &vertices, indices, levels).expect("a mesh");
        if meshlets {
            cut_meshlets(&mut ball);
        }
        let ball = stores.meshes.add_model("ball", &ball).expect("a mesh");
        let add = |stores: &mut Stores, mesh, material: Material| {
            let material = stores.materials.add(&material);
            MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
        };
        let gray = Material {
            color: Vec4::new(0.7, 0.7, 0.7, 1.0),
            workflow: metallic(0.0, 0.8),
            ..Material::default()
        };
        let blue = Material {
            color: Vec4::new(0.2, 0.4, 0.9, 1.0),
            workflow: metallic(0.0, 0.3),
            ..gray
        };
        let masked = Material {
            color: Vec4::new(0.9, 0.4, 0.1, 1.0),
            alpha: AlphaMode::Mask { cutoff: 0.5 },
            ..blue
        };
        let floor = add(&mut stores, floor, gray);
        let opaque = add(&mut stores, ball, blue);
        let masked = add(&mut stores, ball, masked);
        let mut draws = Draws::default();
        draws.push_draw(facing_up(40.0, Vec3::ZERO), &floor, 1);
        let placed = [
            (Vec3::ONE, Vec3::new(0.0, 1.0, 0.0), &opaque),
            (Vec3::ONE, Vec3::new(2.4, 1.0, -1.0), &masked),
            (
                Vec3::new(-1.0, 1.0, 1.0),
                Vec3::new(-2.2, 1.0, 1.0),
                &opaque,
            ),
            (Vec3::new(1.0, 0.5, 1.0), Vec3::new(3.0, 0.5, 2.0), &masked),
            (Vec3::ONE, Vec3::new(-6.0, 1.0, -1.0), &opaque),
            (Vec3::ONE, Vec3::new(-3.0, 1.0, -14.0), &masked),
        ];
        for (i, &(scale, at, draw)) in placed.iter().enumerate() {
            let transform = Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, at);
            draws.push_draw(transform, draw, 2 + i as u64);
        }
        stores.lights = vec![Light {
            color: Vec3::splat(4.0),
            ..sun(Vec3::new(0.9, -0.8, 0.7), Some(30.0))
        }];
        let viewpoint = Viewpoint {
            position: DVec3::new(0.0, 3.0, 7.0),
            rotation: Quat::from_rotation_x(-0.35),
            projection: Projection::Perspective {
                fov: PI / 4.0,
                near: 0.1,
            },
        };
        let viewport = filling([WIDTH, HEIGHT]);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        // Past the frame the mesh came to be drawable in, which planned no room for its meshlets
        // and drew it by its indices.
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        let counts = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        (counts, pixels)
    };
    let (by_indices, indexed) = render(false, false);
    let (by_meshlets, cut) = render(true, false);
    assert!(cut == indexed, "the pixels differ");
    assert_eq!(
        (by_meshlets.draws, by_meshlets.instances),
        (by_indices.draws, by_indices.instances),
        "the same instances at the same levels"
    );
    assert!(
        by_meshlets.triangles > 0 && by_meshlets.triangles < by_indices.triangles,
        "{by_meshlets:?} against {by_indices:?}"
    );
    compare("meshlets", WIDTH, HEIGHT, &cut);
    let (seen_counts, seen) = render(true, true);
    assert_eq!(seen_counts, by_meshlets, "the same draws");
    compare("meshlets", WIDTH, HEIGHT, &seen);
}

/// A gray floor, a sphere on it, an orange post standing on it, and a masked plate, upright,
/// whose right half is cut away, seen with perspective from above and in front, under a light
/// from the upper left, behind them, casting shadows 20 units ahead, the second of two, and a dim
/// one from the front, which casts none, tonemapped by PBR Neutral: each casts its shadow on the
/// floor toward the view, the plate's of its left half alone, in which the dim light still
/// shows the floor, and the sphere's side away from the light is in its own shadow, against
/// their reference image.
fn shadowed_models_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let square = model_square(&mut stores.meshes);
    let half = half_square(&mut stores.meshes);
    let sphere = sphere(&mut stores.meshes, 0.6, 24, 48);
    let add = |stores: &mut Stores, mesh, material: Material| {
        let material = stores.materials.add(&material);
        MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
    };
    let gray = Material {
        color: Vec4::new(0.7, 0.7, 0.7, 1.0),
        workflow: metallic(0.0, 0.8),
        ..Material::default()
    };
    let orange = Material {
        color: Vec4::new(0.9, 0.4, 0.1, 1.0),
        workflow: metallic(0.0, 0.5),
        ..gray
    };
    let blue = Material {
        color: Vec4::new(0.2, 0.4, 0.9, 1.0),
        workflow: metallic(0.0, 0.3),
        ..gray
    };
    let cut = Material {
        alpha: AlphaMode::Mask { cutoff: 0.5 },
        double_sided: true,
        ..orange
    };
    let floor = add(&mut stores, square, gray);
    let ball = add(&mut stores, sphere, blue);
    let post = add(&mut stores, square, orange);
    let plate = add(&mut stores, half, cut);
    let mut draws = Draws::default();
    draws.push_draw(facing_up(20.0, Vec3::ZERO), &floor, 0);
    draws.push_draw(Mat4::from_translation(Vec3::new(0.1, 0.6, 0.2)), &ball, 0);
    // A post 0.4 across and 1.6 tall, its faces each a square turned to face out.
    for turn in 0..4 {
        let rotation = Quat::from_rotation_y(turn as f32 * FRAC_PI_2);
        let out = rotation * Vec3::new(0.0, 0.0, 0.2);
        let face = Mat4::from_scale_rotation_translation(
            Vec3::new(0.4, 1.6, 1.0),
            rotation,
            Vec3::new(1.5, 0.8, -1.0) + out,
        );
        draws.push_draw(face, &post, 0);
    }
    let upright = Mat4::from_scale_rotation_translation(
        Vec3::new(1.2, 1.0, 1.0),
        Quat::from_rotation_y(0.3),
        Vec3::new(-1.5, 0.5, -0.9),
    );
    draws.push_draw(upright, &plate, 0);
    // From the upper left, behind, so that the shadows fall toward the view; and a dim light from
    // the front, which casts none, so that what's in shadow shows.
    stores.lights = vec![
        Light {
            color: Vec3::splat(0.6),
            ..sun(Vec3::new(-0.3, -0.6, -1.0), None)
        },
        Light {
            color: Vec3::splat(4.0),
            ..sun(Vec3::new(0.9, -0.8, 0.7), Some(20.0))
        },
    ];
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 2.6, 4.2),
        rotation: Quat::from_rotation_x(-0.5),
        projection: Projection::Perspective {
            fov: PI / 4.0,
            near: 0.1,
        },
    };
    let viewport = filling([WIDTH, HEIGHT]);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let data = RenderData {
        tonemapper: Tonemapper::PbrNeutral,
        ..stores.data(viewport, viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    check(renderer, "shadowed_models", WIDTH, HEIGHT);
}

/// What the last frame drew in the view of `draws` in `viewport` from `viewpoint`, which comes back
/// as the frame two later begins: renders them twice more, as they were.
/// The light the antialiasing tests' models are lit by, from the upper left, in front.
fn front_light() -> Light {
    Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::new(1.0, -1.0, -1.0).normalize(),
        color: Vec3::splat(4.0),
        range: None,
        shadow_distance: None,
    }
}

/// What the antialiasing tests see from: 4 in front of the origin, a quarter of a turn across,
/// about 19 pixels a unit at the origin in a view 64 pixels a side; as far again as `moved`.
fn seeing_from(moved: Vec3) -> Viewpoint {
    Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0) + moved.as_dvec3(),
        projection: Projection::Perspective {
            fov: PI / 4.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    }
}

/// The linear light of an sRGB-encoded channel, as a frame reads back.
fn decoded(channel: u8) -> f32 {
    let encoded = f32::from(channel) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        math::libm::powf((encoded + 0.055) / 1.055, 2.4)
    }
}

/// How far apart `a` and `b`, frames read back, are in linear light: the mean over every channel of
/// every pixel.
fn linear_distance(a: &[u8], b: &[u8]) -> f32 {
    assert_eq!(a.len(), b.len(), "frames of a size");
    let sum: f32 = (a.iter().zip(b))
        .map(|(&a, &b)| (decoded(a) - decoded(b)).abs())
        .sum();
    sum / a.len() as f32
}

/// `pixels`, a frame read back `size` pixels a side, averaged down `by` times each way in linear
/// light, as a view of that many samples a pixel shows it.
fn averaged_down(pixels: &[u8], size: u32, by: u32) -> Vec<u8> {
    let small = size / by;
    let mut averaged = Vec::with_capacity((small * small * 4) as usize);
    for y in 0..small {
        for x in 0..small {
            for channel in 0..4 {
                let mut sum = 0.0;
                for dy in 0..by {
                    for dx in 0..by {
                        let at = ((y * by + dy) * size + x * by + dx) * 4 + channel;
                        sum += decoded(pixels[at as usize]);
                    }
                }
                averaged.push(encoded(sum / (by * by) as f32));
            }
        }
    }
    averaged
}

/// `frames`, each read back `size` pixels a side, side by side, as a reference holds frames in a
/// row.
fn side_by_side(frames: &[&[u8]], size: u32) -> Vec<u8> {
    let row = (size * 4) as usize;
    (0..size as usize)
        .flat_map(|y| {
            frames
                .iter()
                .flat_map(move |frame| &frame[y * row..(y + 1) * row])
        })
        .copied()
        .collect()
}

/// The side of the view [`edges_seen`] draws, at its own size.
const EDGES_SIZE: u32 = 64;

/// `frames` frames of the antialiasing tests' view, [`EDGES_SIZE`] times `scale` pixels a side,
/// antialiased as `antialiasing` says: a lit sphere, a model's, and a square of a game's opaque
/// shader, turned so that their edges cross the pixels at every angle, with a square on the
/// canvas, 12 pixels a side at the view's own size, 8 from its bottom left, over them.
fn edges_seen(scale: u32, antialiasing: Antialiasing, frames: usize) -> Vec<Vec<u8>> {
    let size = EDGES_SIZE * scale;
    let mut renderer = renderer(size, size);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let sphere = sphere(&mut stores.meshes, 0.6, 24, 48);
    let material = stores.materials.add(&Material {
        color: Vec4::new(0.9, 0.45, 0.2, 1.0),
        workflow: metallic(0.0, 0.5),
        ..Material::default()
    });
    let ball = MeshDraw::with_material(sphere, material, &stores.materials, &mut stores.shaders);
    let mut draws = Draws::default();
    let turned = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.3),
        Quat::from_rotation_z(0.35),
        Vec3::new(-0.5, 0.35, 0.0),
    );
    draws.push(square, flat, turned, Vec4::new(0.85, 0.85, 0.95, 1.0));
    draws.push_draw(Mat4::from_translation(Vec3::new(0.45, -0.3, 0.6)), &ball, 0);
    let mut canvas = Draws::default();
    let placed = Mat4::from_scale(Vec3::new(scale as f32, scale as f32, 1.0))
        * Mat4::from_translation(Vec3::new(14.0, 14.0, 0.0))
        * Mat4::from_scale(Vec3::new(12.0, 12.0, 1.0));
    let green = Sprite {
        color: Vec4::new(0.1, 0.8, 0.2, 1.0),
        ..Sprite::default()
    };
    canvas.push_sprite(placed, green);
    stores.lights = vec![front_light()];
    let (viewport, viewpoint) = (filling([size, size]), seeing_from(Vec3::ZERO));
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut drawn = Vec::new();
    for _ in 0..frames {
        let data = RenderData {
            antialiasing,
            canvas: &canvas,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        drawn.push(frame_pixels(&mut renderer));
    }
    shut_down(renderer);
    drawn
}

/// A view antialiased frame after frame, against its reference of frames 1, 2, 4 and 16 side by
/// side, of [`edges_seen`]'s view. By the sixteenth, its edges average what's drawn across their
/// pixels: it's nearer the view drawn at 4 times the size, without antialiasing, and averaged down
/// than the view drawn without it is; and the canvas, drawn after the view resolves, shows as it
/// does without it.
fn antialiasing_smooths_edges_frame_by_frame() {
    const SIZE: u32 = EDGES_SIZE;
    const LARGER: u32 = 4;
    const SHOWN: [usize; 4] = [1, 2, 4, 16];
    let antialiased = edges_seen(1, Antialiasing::Taa, 16);
    let shown = SHOWN.map(|frame| antialiased[frame - 1].as_slice());
    compare(
        "antialiased",
        SIZE * SHOWN.len() as u32,
        SIZE,
        &side_by_side(&shown, SIZE),
    );
    let aliased = edges_seen(1, Antialiasing::None, 1).remove(0);
    let larger = edges_seen(LARGER, Antialiasing::None, 1).remove(0);
    let truth = averaged_down(&larger, SIZE * LARGER, LARGER);
    let last = &antialiased[15];
    let (resolved, drawn) = (
        linear_distance(last, &truth),
        linear_distance(&aliased, &truth),
    );
    assert!(
        resolved < 0.6 * drawn,
        "the 16th frame is {resolved} from the view averaged down, and the view without \
         antialiasing {drawn}"
    );
    let canvas = |frame: &[u8]| region(frame, SIZE, [8, SIZE - 20], [12, 12]);
    assert!(
        canvas(last) == canvas(&aliased),
        "the canvas shows as it does without antialiasing"
    );
}

/// What a model or a game's shader draws, as the antialiasing tests draw it.
#[derive(Clone, Copy, Debug)]
enum Moved {
    /// A sphere of the model shader drawn by its indices.
    Indices,
    /// The sphere cut into meshlets, drawn into the visibility buffer and shaded from it.
    Meshlets,
    /// A square of a game's shader that returns its motion, `moving.slang`.
    Game,
}

impl Moved {
    /// What `self` draws with, added to `stores`: a sphere 0.8 across, or a square 1.3 across,
    /// turned so that its edges cross the pixels aslant.
    fn draw(self, stores: &mut Stores) -> (MeshDraw, Mat4) {
        let draw = match self {
            Self::Indices | Self::Meshlets => {
                let (vertices, indices) = sphere_of(0.8, 24, 48);
                let packed = ModelMesh::pack("sphere", &vertices, indices, Vec::new());
                let mut packed = packed.expect("a mesh");
                if matches!(self, Self::Meshlets) {
                    cut_meshlets(&mut packed);
                }
                let mesh = stores.meshes.add_model("sphere", &packed).expect("a mesh");
                let material = stores.materials.add(&Material {
                    color: Vec4::new(0.9, 0.45, 0.2, 1.0),
                    workflow: metallic(0.0, 0.5),
                    ..Material::default()
                });
                MeshDraw::with_material(mesh, material, &stores.materials, &mut stores.shaders)
            }
            Self::Game => {
                let moving = (stores.shaders)
                    .add::<[f32; 3], Vec4>("moving", &shader("moving.slang"), Blend::Opaque)
                    .expect("a shader");
                let square = square(&mut stores.meshes);
                MeshDraw::new(square, moving, Vec4::new(0.85, 0.85, 0.95, 1.0))
            }
        };
        let shape = match self {
            Self::Indices | Self::Meshlets => Mat4::IDENTITY,
            Self::Game => Mat4::from_scale_rotation_translation(
                Vec3::splat(1.3),
                Quat::from_rotation_z(0.35),
                Vec3::ZERO,
            ),
        };
        (draw, shape)
    }
}

/// What the camera moves with, antialiased: an instance of the scene, moved each frame as far as
/// the camera, so that it shows where it did, resolves as though neither moved, as its motion takes
/// each pixel's history back from where it showed the frame before: after as many frames, within
/// 0.0015 on average, in linear light, of the view of it standing still, which rounding alone keeps
/// it from, where moving it with the camera alone, as though it stood still in the world, leaves it
/// 0.004 or more apart. For the model shader drawn by its indices, and by its meshlets into the
/// visibility buffer, and a game's shader that returns its motion.
fn antialiasing_follows_what_moves() {
    const SIZE: u32 = 64;
    const FRAMES: u32 = 20;
    let step = Vec3::new(0.21, 0.13, -0.05);
    let render = |moved: Moved, moving: bool| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let (draw, shape) = moved.draw(&mut stores);
        stores.lights = vec![front_light()];
        // Where the frame's camera and instance are, from where they start: still, where the
        // moving ones end.
        let at = |frame: u32| step * if moving { frame } else { FRAMES - 1 } as f32;
        let placed = |frame| Mat4::from_translation(at(frame)) * shape;
        let mut draws = Draws::default();
        draws.draw_scene();
        let viewport = filling([SIZE, SIZE]);
        stores.scene.set(0, placed(0), &draw, 1);
        settle(
            &mut renderer,
            &mut stores,
            viewport,
            seeing_from(at(0)),
            &draws,
        );
        for frame in 0..FRAMES {
            stores.scene.set(0, placed(frame), &draw, 1);
            let data = RenderData {
                antialiasing: Antialiasing::Taa,
                ..stores.data(viewport, seeing_from(at(frame)), &draws)
            };
            render_without_ui(&mut renderer, data);
        }
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        pixels
    };
    for moved in [Moved::Indices, Moved::Meshlets, Moved::Game] {
        let (moving, still) = (render(moved, true), render(moved, false));
        let apart = linear_distance(&moving, &still);
        assert!(
            apart < 0.0015,
            "{moved:?}: the view moving is {apart} from the view standing still"
        );
    }
}

/// A scene 1,000 km from the world's origin, drawn from an origin near it, as the engine draws: a
/// lit sphere on a floor, both the scene's, and a fire's particles, under fog and ambient
/// occlusion, antialiased and motion-blurred, as the view and the sphere move along; halfway, the
/// origin moves a step on, the scene's instances set again from it, as the engine sets them. Frame
/// by frame, it draws as the same scene at the world's origin, drawn from it, does, within
/// rounding: antialiasing's, ambient occlusion's and motion blur's histories, the floor's motion,
/// none, and the particles alive all carried through the move.
fn a_far_scene_draws_as_at_the_origin_through_its_origins_move() {
    const SIZE: u32 = 96;
    const FRAMES: u32 = 12;
    const MOVED_AT: u32 = 6;
    let fire = Emission {
        shape: ParticleShape::Sphere { radius: 0.15 },
        speed: [0.1, 0.4],
        lifetime: [0.5, 0.9],
        spin: [0.0; 2],
        acceleration: Vec3::new(0.0, 3.0, 0.0),
        drag: 1.5,
        size: [0.25, 0.1],
        color: [
            Vec4::new(4.0, 1.6, 0.4, 0.8),
            Vec4::new(1.5, 0.2, 0.05, 0.0),
        ],
        texture: None,
        cells: [1, 1],
        additive: true,
    };
    // Each frame's pixels, of the scene `far` from the world's origin, drawn from the first of
    // `origins` until `MOVED_AT`, then from the second.
    let render = |far: DVec3, origins: [DVec3; 2]| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let (ball, _) = Moved::Indices.draw(&mut stores);
        let plate = model_square(&mut stores.meshes);
        let white = stores.materials.add(&Material {
            workflow: metallic(0.0, 1.0),
            ..Material::default()
        });
        let floor = MeshDraw::with_material(plate, white, &stores.materials, &mut stores.shaders);
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let square = square(&mut stores.meshes);
        stores.lights = vec![front_light()];
        // A sky alike every way, whose light the ambient occlusion darkens.
        let sky = sky_texture(&mut stores.textures, [8, 4], |_, _| [1.0; 3]);
        stores.sky = Some(Sky {
            texture: sky,
            luminance: 0.6,
        });
        stores.ambient_occlusion = Some(AmbientOcclusion::default());
        let viewport = filling([SIZE, SIZE]);
        let origin = |frame: u32| origins[usize::from(frame >= MOVED_AT)];
        // Where `at`, from the scene's own origin, is from the frame's.
        let from_origin = |frame: u32, at: Vec3| (far + at.as_dvec3() - origin(frame)).as_vec3();
        let viewpoint = |frame: u32| Viewpoint {
            position: far
                + DVec3::new(0.0, 1.0, 4.0)
                + DVec3::new(0.03, 0.01, -0.02) * f64::from(frame),
            rotation: Quat::from_rotation_x(-0.2),
            projection: Projection::Perspective {
                fov: FRAC_PI_4,
                near: 0.5,
            },
        };
        let fog = Fog {
            density: 0.08,
            falloff: 0.5,
            height: (far.y + 0.5) as f32,
            luminance: 0.3,
            ..Fog::default()
        };
        let fire_at = |frame| Mat4::from_translation(from_origin(frame, Vec3::new(-0.9, 0.2, 0.0)));
        let draws = |frame: u32| {
            let mut draws = Draws::default();
            draws.draw_scene();
            // A wall of stripes behind, of draws, which the camera's motion alone blurs.
            for stripe in -12..12 {
                let at = from_origin(frame, Vec3::new(0.1 + 0.2 * stripe as f32, 1.5, -1.5));
                let at = Mat4::from_scale_rotation_translation(
                    Vec3::new(0.2, 3.0, 1.0),
                    Quat::IDENTITY,
                    at,
                );
                let shade = if stripe % 2 == 0 { 0.5 } else { 0.05 };
                draws.push(square, flat, at, Vec4::new(shade, shade, shade * 1.2, 1.0));
            }
            let at = fire_at(frame);
            let births = Births {
                first: u64::from(frame) * 4,
                count: 4,
                time: f64::from(frame) - 0.75,
                every: 0.25,
                from: at,
                to: at,
                along: 0.25,
                step: 0.25,
            };
            draws.push_particles(Particles {
                id: 1,
                fresh: false,
                most: 64,
                now: f64::from(frame),
                tick: 1.0 / 64.0,
                births,
                bursts: Bursts::none(at),
                emission: fire,
            });
            draws
        };
        let mut drawn = Vec::new();
        for frame in 0..FRAMES {
            let ball_at = Vec3::new(0.2 + 0.02 * frame as f32, 0.4, 0.0);
            stores.scene.set(
                0,
                Mat4::from_translation(from_origin(frame, ball_at)),
                &ball,
                1,
            );
            // The floor, still, set as it comes and again as the origin moves.
            if frame == 0 || frame == MOVED_AT {
                let at = facing_up(6.0, from_origin(frame, Vec3::ZERO));
                stores.scene.set(1, at, &floor, 2);
            }
            let draws = draws(frame);
            // The first frame's again until its meshes are drawable.
            for _ in 0..200 {
                let data = RenderData {
                    antialiasing: Antialiasing::Taa,
                    motion_blur: Some(MotionBlur { shutter: 0.5 }),
                    fog: Some(fog),
                    origin: origin(frame),
                    ..stores.data(viewport, viewpoint(frame), &draws)
                };
                render_without_ui(&mut renderer, data);
                if frame > 0 || renderer.streaming().is_empty() {
                    break;
                }
            }
            drawn.push(frame_pixels(&mut renderer));
        }
        shut_down(renderer);
        drawn
    };
    let at_the_origin = render(DVec3::ZERO, [DVec3::ZERO; 2]);
    let far = DVec3::new(1.0e6 + 0.3, 1024.0, -1.0e6 - 0.7);
    let first = DVec3::new(1_000_448.0, 1024.0, -1_000_448.0);
    let moved = first + DVec3::new(1024.0, 0.0, 0.0);
    let far_out = render(far, [first, moved]);
    // Within rounding, of the scene's points 1,500 or so from the origin they're drawn from, at
    // most 0.0005 apart on average in linear light, and a few pixels; where a renderer that left
    // one of what it keeps as it was draws the frame of the move apart: by 0.0045 for the scene's
    // motion, 0.014 for antialiasing's history and 0.02 for motion blur's, or by 27 pixels for the
    // particles alive.
    for (frame, (near, far)) in at_the_origin.iter().zip(&far_out).enumerate() {
        let apart = linear_distance(near, far);
        let off = (near.chunks(4).zip(far.chunks(4)))
            .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 32))
            .count();
        assert!(
            apart < 0.001 && off <= 12,
            "frame {frame}: {apart} apart, {off} pixels off by more than 32"
        );
    }
}

/// A plate of the scene that crosses a view standing still, antialiased, against its reference of
/// frames 4, 8, 12 and 16 side by side: where it was leaves no trail, every pixel two or more from
/// where it is now showing the clear color, as its history there, of the plate, is clipped to
/// what's round it; and no pixel of any frame is darker or brighter than the clear color and the
/// plate's own, as a filter's undershoot at an edge would be.
fn antialiasing_leaves_no_trail() {
    const SIZE: u32 = 64;
    const FRAMES: usize = 16;
    const SHOWN: [usize; 4] = [4, 8, 12, 16];
    let render = |antialiasing: Antialiasing, frames: std::ops::Range<usize>| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let plate = model_square(&mut stores.meshes);
        let material = stores.materials.add(&Material {
            color: Vec4::new(0.95, 0.9, 0.6, 1.0),
            workflow: metallic(0.0, 0.7),
            ..Material::default()
        });
        // Unlit, without lights, so that it's one color wherever it is.
        let draw = MeshDraw::with_material(plate, material, &stores.materials, &mut stores.shaders);
        let placed = |frame: usize| {
            let at = Vec3::new(-1.6 + 0.15 * frame as f32, 0.1, 0.0);
            Mat4::from_scale_rotation_translation(Vec3::ONE, Quat::from_rotation_z(0.2), at)
        };
        let mut draws = Draws::default();
        draws.draw_scene();
        let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
        stores.scene.set(0, placed(frames.start), &draw, 1);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let mut drawn = Vec::new();
        for frame in frames {
            stores.scene.set(0, placed(frame), &draw, 1);
            let data = RenderData {
                antialiasing,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(&mut renderer, data);
            drawn.push(frame_pixels(&mut renderer));
        }
        shut_down(renderer);
        drawn
    };
    let antialiased = render(Antialiasing::Taa, 1..FRAMES + 1);
    let shown = SHOWN.map(|frame| antialiased[frame - 1].as_slice());
    compare(
        "antialiased_motion",
        SIZE * SHOWN.len() as u32,
        SIZE,
        &side_by_side(&shown, SIZE),
    );
    // Where the plate is in the last frame, and where it was in the fourth, drawn without
    // antialiasing.
    let now = render(Antialiasing::None, FRAMES..FRAMES + 1).remove(0);
    let then = render(Antialiasing::None, 4..5).remove(0);
    let last = &antialiased[FRAMES - 1];
    let clear = |frame: &[u8], [x, y]: [i32; 2]| {
        let inside = (0..SIZE as i32).contains(&x) && (0..SIZE as i32).contains(&y);
        !inside || close(pixel_at(frame, SIZE, [x as u32, y as u32]), CLEARED)
    };
    let (mut left, mut trailing) = (0, Vec::new());
    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            // Two or more from where the plate is now.
            let away = (-2..=2).all(|dy| (-2..=2).all(|dx| clear(&now, [x + dx, y + dy])));
            if !away || clear(&then, [x, y]) {
                continue;
            }
            left += 1;
            if !clear(last, [x, y]) {
                trailing.push([x, y]);
            }
        }
    }
    assert!(left > 100, "the plate left {left} pixels");
    assert!(trailing.is_empty(), "the plate trails at {trailing:?}");
    // The plate's color, lit, where nothing of the clear color is round it, and the clear color:
    // each channel's least and most.
    let inside = (0..SIZE * SIZE)
        .map(|i| [i % SIZE, i / SIZE])
        .find(|&[x, y]| {
            let at = |dx: i32, dy: i32| [x as i32 + dx, y as i32 + dy];
            (-3..=3).all(|dy| (-3..=3).all(|dx| !clear(&now, at(dx, dy))))
        });
    let plate = pixel_at(
        &now,
        SIZE,
        inside.expect("the plate shows, 7 pixels across"),
    );
    let bounds: [(u8, u8); 4] =
        std::array::from_fn(|c| (plate[c].min(CLEARED[c]), plate[c].max(CLEARED[c])));
    for (i, frame) in antialiased.iter().enumerate() {
        let outside = (frame.as_chunks::<4>().0.iter()).position(|pixel| {
            (pixel.iter().zip(bounds)).any(|(&v, (least, most))| {
                v < least.saturating_sub(2) || v > most.saturating_add(2)
            })
        });
        assert!(
            outside.is_none(),
            "frame {}'s pixel {outside:?} is outside the clear color's and the plate's {bounds:?}",
            i + 1
        );
    }
}

/// The bright plate the antialiasing tests' specks and bars are seen against: unlit, 0.8 in linear
/// light, behind the origin and filling the view, drawn by `flat`, `flat_color.slang`.
fn bright_plate(draws: &mut Draws, square: Mesh<[f32; 3]>, flat: Shader<[f32; 3], Vec4>) {
    let behind =
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.5)) * Mat4::from_scale(Vec3::splat(4.0));
    draws.push(square, flat, behind, Vec4::new(0.8, 0.8, 0.8, 1.0));
}

/// [`bright_plate`], sRGB-encoded, as a frame reads back.
const PLATE: [u8; 4] = [231, 231, 231, 255];

/// What's thinner than a pixel, as the antialiasing tests see it from [`seeing_from`], added to
/// `stores`: dark bars a third of a pixel wide across [`bright_plate`], aslant, which each of the 8
/// offsets draws over pixels the others don't.
fn thin_bars(stores: &mut Stores) -> Draws {
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    bright_plate(&mut draws, square, flat);
    // About 19 pixels a unit at the origin.
    for i in 0..5 {
        let bar = Mat4::from_scale_rotation_translation(
            Vec3::new(0.016, 2.6, 1.0),
            Quat::from_rotation_z(0.15 + 0.3 * i as f32),
            Vec3::new(-1.0 + 0.5 * i as f32, 0.0, 0.0),
        );
        draws.push(square, flat, bar, Vec4::new(0.01, 0.01, 0.01, 1.0));
    }
    draws
}

/// [`thin_bars`] in a still view, antialiased. Once the view has settled, over 16 frames, no pixel's
/// channel moves more than 2 levels, as rounding may: it shows the mean of what the offsets drew,
/// where a resolve that clipped its history to the pixels round it, which hold the bars at some
/// offsets and not at others, showed each offset's own, and moved 24.
fn antialiasing_steadies_what_is_thinner_than_a_pixel() {
    const SIZE: u32 = 64;
    const SETTLE: usize = 160;
    const FRAMES: usize = 16;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let draws = thin_bars(&mut stores);
    let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut drawn = Vec::new();
    for frame in 0..SETTLE + FRAMES {
        let data = RenderData {
            antialiasing: Antialiasing::Taa,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        if frame >= SETTLE {
            drawn.push(frame_pixels(&mut renderer));
        }
    }
    shut_down(renderer);
    // Each channel of each pixel: how far it moves over the frames, and where it moves most.
    let (mut worst, mut at, mut moving) = (0, 0, 0);
    for channel in (0..(SIZE * SIZE * 4) as usize).filter(|channel| channel % 4 != 3) {
        let values = drawn.iter().map(|frame| frame[channel]);
        let range = values.clone().max().unwrap_or(0) - values.min().unwrap_or(0);
        if range > 2 {
            moving += 1;
        }
        if range > worst {
            (worst, at) = (range, channel / 4);
        }
    }
    let at = [at as u32 % SIZE, at as u32 / SIZE];
    assert!(
        worst <= 2,
        "a pixel at {at:?} moves {worst} levels over {FRAMES} still frames, and {moving} channels \
         more than 2"
    );
}

/// [`thin_bars`] antialiased as the view stops after moving: panning half a pixel a frame at the
/// bars for 16 frames, then still for 160. As the store of what the offsets drew holds again, two
/// cycles after the view stops, each pixel settles on the mean of what they drew by a still pixel's
/// share of the way a frame: in no frame do more than 1% of its pixels land at once within 2 levels
/// of where they end from 12 or more away, against some 11 that the clip's flicker lands by chance
/// the frame after the view stops; a resolve that showed that mean as soon as the store held jumped
/// there, 350 at once.
fn antialiasing_settles_without_a_jump_once_the_view_stops() {
    const SIZE: u32 = 64;
    const MOVES: usize = 16;
    const STOPPED: usize = 160;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let draws = thin_bars(&mut stores);
    let viewport = filling([SIZE, SIZE]);
    // Half a pixel a frame at the bars, 4 from the camera, whose view is a quarter turn tall.
    let step = 0.5 * 2.0 * 4.0 * math::libm::tanf(PI / 8.0) / SIZE as f32;
    let at = |frame: usize| {
        let moved = frame.min(MOVES) as f32 - MOVES as f32 / 2.0;
        seeing_from(Vec3::new(step * moved, 0.0, 0.0))
    };
    settle(&mut renderer, &mut stores, viewport, at(0), &draws);
    let mut drawn = Vec::new();
    for frame in 0..=MOVES + STOPPED {
        let data = RenderData {
            antialiasing: Antialiasing::Taa,
            ..stores.data(viewport, at(frame), &draws)
        };
        render_without_ui(&mut renderer, data);
        drawn.push(frame_pixels(&mut renderer));
    }
    shut_down(renderer);
    let end = drawn.last().expect("frames");
    // How far each pixel of `frame` is from where it ends: its farthest channel's levels.
    let from_end = |frame: &[u8], at: usize| {
        (0..3)
            .map(|c| frame[at * 4 + c].abs_diff(end[at * 4 + c]))
            .max()
            .unwrap_or(0)
    };
    // How many pixels land in each frame after the view stopped.
    let landed: Vec<usize> = (drawn[MOVES..].windows(2))
        .map(|pair| {
            (0..(SIZE * SIZE) as usize)
                .filter(|&at| from_end(&pair[1], at) <= 2 && from_end(&pair[0], at) >= 12)
                .count()
        })
        .collect();
    let (after, most) = (landed.iter().enumerate())
        .max_by_key(|&(_, count)| *count)
        .map(|(after, count)| (after + 1, *count))
        .expect("frames");
    assert!(
        most <= (SIZE * SIZE / 100) as usize,
        "{most} pixels land at once within 2 levels of where they settle from 12 or more away, \
         {after} frames after the view stops: {landed:?}"
    );
}

/// Dark squares in a still view, antialiased, from a thousandth to a fiftieth in linear light, where
/// a level of sRGB spans the least light: once settled, each pixel 2 or more inside one shows it as
/// a view without antialiasing does, within a level, as one whose resolve kept what the offsets drew
/// in steps of a thousandth, and showed their mean, didn't, by 2.
fn antialiasing_keeps_still_dark_colors_as_drawn() {
    const SIZE: u32 = 64;
    const SETTLE: usize = 64;
    let render = |antialiasing: Antialiasing| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let square = square(&mut stores.meshes);
        let mut draws = Draws::default();
        // A grid of 4 by 4 squares, each 0.7 across, about 13 pixels.
        for i in 0..16 {
            let at = Vec3::new(
                -1.2 + 0.8 * (i % 4) as f32,
                -1.2 + 0.8 * (i / 4) as f32,
                0.0,
            );
            let placed = Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.7));
            let linear = 0.001 + 0.019 * i as f32 / 15.0;
            draws.push(square, flat, placed, Vec4::new(linear, linear, linear, 1.0));
        }
        let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        for _ in 0..SETTLE {
            let data = RenderData {
                antialiasing,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(&mut renderer, data);
        }
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        pixels
    };
    let (antialiased, drawn) = (render(Antialiasing::Taa), render(Antialiasing::None));
    let (mut inside, mut apart) = (0, Vec::new());
    for y in 2..SIZE - 2 {
        for x in 2..SIZE - 2 {
            let at = |dx: i32, dy: i32| [(x as i32 + dx) as u32, (y as i32 + dy) as u32];
            let center = pixel_at(&drawn, SIZE, [x, y]);
            let flat =
                (-2..=2).all(|dy| (-2..=2).all(|dx| pixel_at(&drawn, SIZE, at(dx, dy)) == center));
            if !flat || center == CLEARED {
                continue;
            }
            inside += 1;
            let shown = pixel_at(&antialiased, SIZE, [x, y]);
            if !close(shown, center) {
                apart.push(([x, y], shown, center));
            }
        }
    }
    assert!(
        inside > 1000,
        "{inside} pixels are 2 or more inside a square"
    );
    assert!(
        apart.is_empty(),
        "{} pixels inside a square show otherwise than without antialiasing, as {:?}",
        apart.len(),
        apart.first()
    );
}

/// A dark speck, about 3 pixels across, crossing a bright plate before a still camera, antialiased,
/// at a quarter of a pixel, a pixel and 3 pixels a frame, from where it stood still long enough for
/// the plate to settle: drawn by a shader that returns how far it moved, `moving.slang`, and by one
/// that doesn't, `flat_color.slang`, as nothing moves where a shadow crosses what stands still. In
/// every frame, every pixel 3 or more from the speck's middle past its edge shows the plate: where
/// it was leaves no trail, as the pixels it left are resolved against what's round them.
fn antialiasing_leaves_no_trail_behind_dark_specks() {
    const SIZE: u32 = 64;
    const STILL: usize = 48;
    // Pixels a unit at the speck's depth, 3.9 from the camera, whose view is a quarter turn tall.
    const PIXELS_A_UNIT: f32 = SIZE as f32 / (2.0 * 3.9 * 0.414_213_57);
    const HALF: f32 = 0.08;
    let dark = Vec4::new(0.01, 0.01, 0.01, 1.0);
    for speed in [0.25, 1.0, 3.0] {
        for returns_motion in [true, false] {
            let mut renderer = renderer(SIZE, SIZE);
            let mut stores = Stores::default();
            let flat = (stores.shaders)
                .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
                .expect("a shader");
            let moving = (stores.shaders)
                .add::<[f32; 3], Vec4>("moving", &shader("moving.slang"), Blend::Opaque)
                .expect("a shader");
            let square = square(&mut stores.meshes);
            let speck = MeshDraw::new(square, moving, dark);
            // Where the speck is in a frame of its crossing, from 0, in the world and in pixels.
            let moves = (40.0 / speed) as usize;
            let middle = |frame: usize| {
                let x = -1.2 + speed * frame as f32 / PIXELS_A_UNIT;
                Vec3::new(x, 0.1, 0.1)
            };
            let placed = |frame| {
                Mat4::from_translation(middle(frame)) * Mat4::from_scale(Vec3::splat(2.0 * HALF))
            };
            let drawing = |stores: &mut Stores, frame| {
                let mut draws = Draws::default();
                bright_plate(&mut draws, square, flat);
                if returns_motion {
                    stores.scene.set(0, placed(frame), &speck, 1);
                    draws.draw_scene();
                } else {
                    draws.push(square, flat, placed(frame), dark);
                }
                draws
            };
            let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
            let draws = drawing(&mut stores, 0);
            settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
            for frame in (0..STILL).map(|_| 0).chain(0..=moves) {
                let draws = drawing(&mut stores, frame);
                let data = RenderData {
                    antialiasing: Antialiasing::Taa,
                    ..stores.data(viewport, viewpoint, &draws)
                };
                render_without_ui(&mut renderer, data);
                let pixels = frame_pixels(&mut renderer);
                let seen = middle(frame) * PIXELS_A_UNIT;
                let center = [SIZE as f32 / 2.0 + seen.x, SIZE as f32 / 2.0 - seen.y];
                let reach = HALF * PIXELS_A_UNIT + 3.0;
                let trailing: Vec<[u32; 2]> = (0..SIZE * SIZE)
                    .map(|i| [i % SIZE, i / SIZE])
                    .filter(|&[x, y]| {
                        let away = |at: u32, middle: f32| (at as f32 + 0.5 - middle).abs() > reach;
                        let far = away(x, center[0]) || away(y, center[1]);
                        far && !close(pixel_at(&pixels, SIZE, [x, y]), PLATE)
                    })
                    .collect();
                assert!(
                    trailing.is_empty(),
                    "at {speed} pixels a frame, {}, frame {frame} of its crossing: the speck at \
                     {center:?} trails at {trailing:?}, {:?} the first",
                    if returns_motion {
                        "returning its motion"
                    } else {
                        "without its motion"
                    },
                    trailing.first().map(|&at| pixel_at(&pixels, SIZE, at))
                );
            }
            shut_down(renderer);
        }
    }
}

/// How many pixels a unit is across at the origin, 4 from [`seeing_from`]'s camera, whose view is a
/// quarter turn tall, in a view `size` pixels a side.
fn pixels_a_unit(size: u32) -> f32 {
    size as f32 / (2.0 * 4.0 * math::libm::tanf(PI / 8.0))
}

/// A view that moves past what stands still at the origin's depth: what it shows there moves
/// `across` pixels and `down` every `over` frames, a whole number of pixels, so that each pixel's
/// point is at a pixel again then.
#[derive(Clone, Copy, Debug)]
struct Pan {
    across: i32,
    down: i32,
    over: usize,
}

impl Pan {
    /// Where frame `frame` sees from, in a view `size` pixels a side.
    fn seeing(self, frame: usize, size: u32) -> Viewpoint {
        let moved = frame as f32 / (self.over as f32 * pixels_a_unit(size));
        seeing_from(Vec3::new(
            -(self.across as f32) * moved,
            self.down as f32 * moved,
            0.0,
        ))
    }
}

/// The slats [`slats_seen`] draws, as the side faces of a slatted table's slats show from 9 m:
/// how tall each is and how far apart they are, both in pixels, and how much they rise for each
/// pixel across, so that each frame's one sample a pixel shows each as dashes, 5 pixels long every
/// 14, that run along it as the view moves up or down; and their light and the plate's behind
/// them.
const SLAT_TALL: f32 = 0.36;
const SLATS_APART: f32 = 2.5;
const SLAT_RISE: f32 = 1.0 / 14.0;
const SLAT_LIGHT: f32 = 0.05;
const SLATS_PLATE: f32 = 0.8;

/// A dark speck that crosses [`slats_seen`]'s view, `speed` pixels a frame across it from 8 pixels
/// in, 2 above its middle, once the frames are kept: drawn by a shader that returns how far it
/// moved, or by one that doesn't.
#[derive(Clone, Copy, Debug)]
struct Speck {
    speed: f32,
    returns_motion: bool,
}

/// Half a [`Speck`]'s side, in units, 3.9 from the camera: about 3 pixels across at 64 a side.
const SPECK_HALF: f32 = 0.08;

impl Speck {
    /// Where its middle shows, in pixels, in the `kept`th frame kept of a view `size` a side.
    fn seen(self, kept: usize, size: u32) -> [f32; 2] {
        [8.0 + self.speed * kept as f32, size as f32 / 2.0 - 2.0]
    }
}

/// Frames `frames` of a view `size` pixels a side, antialiased, panning as `pan` says from frame 0
/// past dark slats at the origin's depth, thinner than a pixel, right before a bright plate. With
/// `speck` crossing them.
fn slats_seen(
    size: u32,
    pan: Pan,
    frames: std::ops::Range<usize>,
    speck: Option<Speck>,
) -> Vec<Vec<u8>> {
    let mut renderer = renderer(size, size);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let moving = (stores.shaders)
        .add::<[f32; 3], Vec4>("moving", &shader("moving.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let black = Vec4::new(0.01, 0.01, 0.01, 1.0);
    let speck_draw = MeshDraw::new(square, moving, black);
    let unit = pixels_a_unit(size);
    let apart = SLATS_APART / unit;
    let half = size as f32 / (2.0 * unit);
    let start = frames.start;
    let drawing = |stores: &mut Stores, frame: usize| {
        let camera = pan.seeing(frame, size).position.as_vec3();
        let mut draws = Draws::default();
        // Right behind the slats, so that the two show moving alike.
        let behind = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.01))
            * Mat4::from_scale(Vec3::splat(800.0));
        draws.push(square, flat, behind, Vec3::splat(SLATS_PLATE).extend(1.0));
        // The slats that cross the view: each stands where it does whatever the frame.
        let middle = ((camera.y - camera.x * SLAT_RISE) / apart).round() as i32;
        let rows = ((half * (1.0 + SLAT_RISE) + 0.2) / apart).ceil() as i32;
        for row in middle - rows..=middle + rows {
            let slat = Mat4::from_scale_rotation_translation(
                Vec3::new(800.0, SLAT_TALL / unit, 1.0),
                Quat::from_rotation_z(math::libm::atanf(SLAT_RISE)),
                Vec3::new(0.0, row as f32 * apart, 0.0),
            );
            draws.push(square, flat, slat, Vec3::splat(SLAT_LIGHT).extend(1.0));
        }
        if let Some(speck) = speck {
            let nearer = size as f32 / (2.0 * 3.9 * math::libm::tanf(PI / 8.0));
            let seen = speck.seen(frame.saturating_sub(start), size);
            let at = Vec3::new(
                camera.x + (seen[0] - size as f32 / 2.0) / nearer,
                camera.y - (seen[1] - size as f32 / 2.0) / nearer,
                0.1,
            );
            let placed =
                Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(2.0 * SPECK_HALF));
            if speck.returns_motion {
                stores.scene.set(0, placed, &speck_draw, 1);
                draws.draw_scene();
            } else {
                draws.push(square, flat, placed, black);
            }
        }
        draws
    };
    let viewport = filling([size, size]);
    let draws = drawing(&mut stores, 0);
    settle(
        &mut renderer,
        &mut stores,
        viewport,
        pan.seeing(0, size),
        &draws,
    );
    let mut drawn = Vec::new();
    for frame in 0..frames.end {
        let draws = drawing(&mut stores, frame);
        let data = RenderData {
            antialiasing: Antialiasing::Taa,
            ..stores.data(viewport, pan.seeing(frame, size), &draws)
        };
        render_without_ui(&mut renderer, data);
        if frame >= start {
            drawn.push(frame_pixels(&mut renderer));
        }
    }
    shut_down(renderer);
    drawn
}

/// How long a point has shown, in frames, where the tests of a panning view measure it: where
/// it's come into view since, its history is as young.
const AGED: usize = 48;

/// Whether the point at (`x`, `y`) of a view that pans as `pan` says has shown for [`AGED`]
/// frames, 2 pixels or more inside a view `size` a side.
fn aged(pan: Pan, size: u32, [x, y]: [i32; 2]) -> bool {
    let old = |at: i32, moves: i32| at * pan.over as i32 >= AGED as i32 * moves;
    let inside = 2..size as i32 - 2;
    inside.contains(&x) && inside.contains(&y) && old(x, pan.across) && old(y, pan.down)
}

/// How each point changes as a view pans: over `samples` of `frames`, `pan.over` apart, each
/// [`aged`] pixel of the first against where its point shows in the others. How many points; how
/// many whose channel that changes most ranges over more than 8 levels; and the mean range.
fn wander(frames: &[Vec<u8>], size: u32, pan: Pan, samples: usize) -> (usize, usize, f32) {
    let seen = |x: i32, y: i32, i: usize| [x + i as i32 * pan.across, y + i as i32 * pan.down];
    let (mut points, mut over, mut sum) = (0, 0, 0.0);
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            if !aged(pan, size, [x, y]) || !aged(pan, size, seen(x, y, samples - 1)) {
                continue;
            }
            let range = (0..3)
                .map(|channel| {
                    let values = (0..samples).map(|i| {
                        let at = seen(x, y, i).map(|v| v as u32);
                        pixel_at(&frames[i * pan.over], size, at)[channel]
                    });
                    values.clone().max().unwrap_or(0) - values.min().unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            points += 1;
            over += usize::from(range > 8);
            sum += f32::from(range);
        }
    }
    (points, over, sum / points as f32)
}

/// [`slats_seen`]'s slats antialiased as the view pans, a third of a pixel a frame across and
/// down, and 1.3 across and 0.4 down: once a point has shown for [`AGED`] frames, its pixel holds
/// as the view carries it along. Over 8 samples and over 4, each where the point is at a pixel
/// again, the pixels' mean range is under 5 levels, and under 1 in 20 ranges over more than 8; and
/// the frame is darker than the plate by at least what the slats cover of it, in linear light, as
/// a still view settles at 1.5 times that, the resolve blending colors weighed by their luminance.
/// A resolve that clips each pixel's history to the pixels round it alone, which miss a slat in
/// most frames, shows each frame's dashes in turn: a mean range of 8.4 and 12 levels, 46% and 63%
/// of the pixels over 8, and 0.19 and 0.36 of that darkness.
fn antialiasing_steadies_what_is_thinner_than_a_pixel_as_the_view_moves() {
    const WARM: usize = 96;
    let pans = [
        (
            Pan {
                across: 1,
                down: 1,
                over: 3,
            },
            96,
            8,
        ),
        (
            Pan {
                across: 9,
                down: 3,
                over: 7,
            },
            128,
            4,
        ),
    ];
    for (pan, size, samples) in pans {
        let frames = slats_seen(size, pan, WARM..WARM + samples * pan.over, None);
        let (points, over, mean) = wander(&frames, size, pan, samples);
        assert!(points > 3000, "{pan:?}: {points} points");
        assert!(
            over * 20 <= points && mean <= 5.0,
            "{pan:?}: of {points} points {over} range over more than 8 levels, and their mean \
             range is {mean}"
        );
        // The mean light of the last frame's aged pixels, below the plate's.
        let last = frames.last().expect("frames");
        let lights: Vec<f32> = (0..size * size)
            .map(|i| [i % size, i / size])
            .filter(|at| aged(pan, size, at.map(|v| v as i32)))
            .map(|at| decoded(pixel_at(last, size, at)[1]))
            .collect();
        let darker = SLATS_PLATE - lights.iter().sum::<f32>() / lights.len() as f32;
        // What the slats cover of the plate, as much darker as they are.
        let cover = SLAT_TALL * math::libm::sqrtf(1.0 + SLAT_RISE * SLAT_RISE) / SLATS_APART;
        let covered = cover * (SLATS_PLATE - SLAT_LIGHT);
        assert!(
            darker >= covered,
            "{pan:?}: the frame is {darker} darker than the plate, and the slats cover {covered}"
        );
    }
}

/// A dark speck crossing [`slats_seen`]'s slats, antialiased, as the view pans a third of a pixel
/// a frame across and down, at a quarter of a pixel a frame and at a pixel, drawn by a shader that
/// returns how far it moved and by one that doesn't, as nothing moves where a shadow crosses what
/// stands still: against the same frames without it. The slats' pixels flicker, and keep their
/// history past the pixels round them; the speck is a change, not flicker, which ends that. So it
/// leaves no trail: under 10 pixels a frame, 3 or more past its edge, show more than 2 levels
/// darker than without it, 5 at the most, where a flicker test that kept its evidence through a
/// change left 100 behind the slower speck. And one that returns its motion, whose pixels' history
/// and evidence are its own, taken where it was, shows dark, its middle no brighter than 90, where
/// one that doesn't shows as bright as 102 and 193.
fn antialiasing_leaves_no_trail_behind_what_crosses_flickering_pixels() {
    const SIZE: u32 = 64;
    const WARM: usize = 64;
    let pan = Pan {
        across: 1,
        down: 1,
        over: 3,
    };
    // Pixels a unit at the speck's depth, 3.9 from the camera.
    let unit = SIZE as f32 / (2.0 * 3.9 * math::libm::tanf(PI / 8.0));
    let reach = SPECK_HALF * unit + 3.0;
    for speed in [0.25, 1.0] {
        let frames = WARM..WARM + (40.0 / speed) as usize;
        let without = slats_seen(SIZE, pan, frames.clone(), None);
        for returns_motion in [true, false] {
            let speck = Speck {
                speed,
                returns_motion,
            };
            let with = slats_seen(SIZE, pan, frames.clone(), Some(speck));
            let (mut darker, mut faintest) = (0, 0);
            for (kept, (with, without)) in with.iter().zip(&without).enumerate() {
                let center = speck.seen(kept, SIZE);
                let away = |at: u32, middle: f32| (at as f32 + 0.5 - middle).abs() > reach;
                darker += (0..SIZE * SIZE)
                    .map(|i| [i % SIZE, i / SIZE])
                    .filter(|&[x, y]| away(x, center[0]) || away(y, center[1]))
                    .filter(|&at| {
                        let shown = pixel_at(with, SIZE, at)[1];
                        shown.saturating_add(2) < pixel_at(without, SIZE, at)[1]
                    })
                    .count();
                let middle = pixel_at(with, SIZE, center.map(|v| v as u32));
                faintest = faintest.max(middle[1]);
            }
            let how = if returns_motion {
                "returning its motion"
            } else {
                "without its motion"
            };
            assert!(
                darker <= 10 * with.len(),
                "at {speed} pixels a frame, {how}: {darker} pixels over {} frames show darker \
                 behind the speck than without it",
                with.len()
            );
            // The speck's 0.01, sRGB-encoded, is 25.
            assert!(
                !returns_motion || faintest <= 90,
                "at {speed} pixels a frame, {how}: the speck's middle shows as bright as {faintest}"
            );
        }
    }
}

/// A bright square 4 pixels a side that shows for 5 frames and then doesn't for 5, over and over,
/// before a dark plate in a still view, antialiased: once it has blinked for 100 frames, the last
/// frame of each time it doesn't show shows the plate where its middle was, within a level, as
/// the clip throws its light out. Each time it goes, its edge's pixels were drawn and are then
/// held by nothing round them, as what flickers is; but they stood still, and an offset drew them
/// otherwise than it did a cycle before, which the offsets alone don't do: a resolve that took
/// them to flicker all the same kept the square's light there, as bright as 135.
fn antialiasing_keeps_no_ghost_of_what_blinks_in_a_still_view() {
    const SIZE: u32 = 64;
    const SHOWS: usize = 5;
    /// The plate's 0.05, sRGB-encoded.
    const DARK: u8 = 63;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let drawing = |frame: usize| {
        let mut draws = Draws::default();
        let behind =
            Mat4::from_translation(Vec3::new(0.0, 0.0, -0.01)) * Mat4::from_scale(Vec3::splat(8.0));
        draws.push(square, flat, behind, Vec4::new(0.05, 0.05, 0.05, 1.0));
        if (frame / SHOWS).is_multiple_of(2) {
            let lit = Mat4::from_scale(Vec3::splat(4.0 / pixels_a_unit(SIZE)));
            draws.push(square, flat, lit, Vec4::new(0.8, 0.8, 0.8, 1.0));
        }
        draws
    };
    let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
    let draws = drawing(0);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    for frame in 0..160 {
        let draws = drawing(frame);
        let data = RenderData {
            antialiasing: Antialiasing::Taa,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        if frame < 100 || frame % (2 * SHOWS) != 2 * SHOWS - 1 {
            continue;
        }
        let pixels = frame_pixels(&mut renderer);
        let middle = region(&pixels, SIZE, [SIZE / 2 - 1, SIZE / 2 - 1], [3, 3]);
        let brightest = middle.as_chunks::<4>().0.iter().map(|pixel| pixel[1]).max();
        assert!(
            brightest <= Some(DARK + 1),
            "frame {frame}, the last of 5 without the square, shows as bright as {brightest:?} \
             where its middle was"
        );
    }
    shut_down(renderer);
}

/// [`edges_seen`]'s view antialiased by SMAA, against its reference of it without antialiasing and
/// with SMAA side by side: in its first frame, nearer the view drawn at 4 times the size, without
/// antialiasing, and averaged down than the view drawn without it is, as near as TAA's sixteenth
/// frame is; the next frame the same, as SMAA keeps nothing from frame to frame; and the canvas,
/// drawn after, as it shows without antialiasing.
fn smaa_smooths_edges_in_a_frame() {
    const SIZE: u32 = EDGES_SIZE;
    const LARGER: u32 = 4;
    let antialiased = edges_seen(1, Antialiasing::Smaa, 2);
    let aliased = edges_seen(1, Antialiasing::None, 1).remove(0);
    compare(
        "smaa",
        SIZE * 2,
        SIZE,
        &side_by_side(&[&aliased, &antialiased[0]], SIZE),
    );
    assert!(
        antialiased[0] == antialiased[1],
        "the second frame is as the first"
    );
    let larger = edges_seen(LARGER, Antialiasing::None, 1).remove(0);
    let truth = averaged_down(&larger, SIZE * LARGER, LARGER);
    let (smaa, drawn) = (
        linear_distance(&antialiased[0], &truth),
        linear_distance(&aliased, &truth),
    );
    assert!(
        smaa < 0.6 * drawn,
        "the view antialiased by SMAA is {smaa} from the view averaged down, and the view \
         without antialiasing {drawn}"
    );
    let canvas = |frame: &[u8]| region(frame, SIZE, [8, SIZE - 20], [12, 12]);
    assert!(
        canvas(&antialiased[0]) == canvas(&aliased),
        "the canvas shows as it does without antialiasing"
    );
}

/// A disk of `segments` triangles around the origin, of radius 1 in the plane of X and Y, facing
/// +Z, of positions alone, as `flat_color.slang` takes them.
fn disk(meshes: &mut Meshes, segments: u32) -> Mesh<[f32; 3]> {
    use math::libm::{cosf, sinf};
    let rim = (0..segments).map(|i| {
        let around = 2.0 * PI * i as f32 / segments as f32;
        [cosf(around), sinf(around), 0.0]
    });
    let corners: Vec<[f32; 3]> = std::iter::once([0.0, 0.0, 0.0]).chain(rim).collect();
    let indices: Vec<u32> = (0..segments)
        .flat_map(|i| [0, 1 + i, 1 + (i + 1) % segments])
        .collect();
    meshes.add("disk", &corners, &indices).expect("a mesh")
}

/// A gray disk over black, 70 pixels across in a view of 96, antialiased by SMAA in one frame,
/// against the view drawn at 8 times the size and averaged down, which shows how much of each
/// pixel the disk covers: each pixel of its edge shows the share of the disk's light that the disk
/// covers of it, as the view shows it, within bounds on average, in each eighth of the way round,
/// across, down and along both diagonals, and at every pixel, where without antialiasing it's 0.2
/// or more off on average; and those across and down alike, as those along either diagonal are.
/// Of a disk of 0.25 under no tonemapper; of 0.02, whose edges a luma encoded as a display encodes
/// it finds, and one of the view's light doesn't; of white, 1, which the clamp shows as it is, so
/// that blending it as PBR Neutral's shoulder would roll it off shows its edge too dark; of 16
/// under PBR Neutral, which blended in the view's light would show a pixel the disk covers a
/// fraction of almost as bright as the disk; and of 2 under PBR Neutral graded by a slope of a
/// half, after SMAA, which brings its light back under the shoulder.
fn smaa_blends_by_how_much_of_each_pixel_the_edge_covers() {
    const SIZE: u32 = 96;
    const LARGER: u32 = 8;
    // The disk of light `light` drawn as `antialiasing` says, `scale` times the size, graded by
    // a slope of `slope` and tonemapped by `tonemapper`: its pixels' green, in linear light.
    let render = |light: f32, tonemapper, slope: f32, scale: u32, antialiasing| {
        let size = SIZE * scale;
        let mut renderer = renderer(size, size);
        let mut stores = Stores::default();
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let disk = disk(&mut stores.meshes, 512);
        let mut draws = Draws::default();
        let color = Vec4::new(light, light, light, 1.0);
        draws.push(disk, flat, Mat4::from_scale(Vec3::splat(1.2)), color);
        let (viewport, viewpoint) = (filling([size, size]), seeing_from(Vec3::ZERO));
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let data = RenderData {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            tonemapper,
            antialiasing,
            grade: Grade {
                slope: Vec3::splat(slope),
                ..Grade::IDENTITY
            },
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        let green = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| decoded(pixel[1]));
        green.collect::<Vec<f32>>()
    };
    // The disk of `light` under `tonemapper` and `slope`, with SMAA and without antialiasing: each
    // eighth of the way round it, from the right, and all of it, the mean distance between each
    // pixel's share of the disk's light and its share in the larger view, the mean of its 8 by 8,
    // over the pixels whose share there is between 2% and 98%; and the most at any pixel, with
    // SMAA.
    let distances = |light: f32, tonemapper, slope: f32| {
        let smaa = render(light, tonemapper, slope, 1, Antialiasing::Smaa);
        let aliased = render(light, tonemapper, slope, 1, Antialiasing::None);
        let larger = render(light, tonemapper, slope, LARGER, Antialiasing::None);
        let averaged = |[x, y]: [u32; 2]| {
            let (width, by) = (SIZE * LARGER, LARGER);
            let at = |(dx, dy)| larger[((y * by + dy) * width + x * by + dx) as usize];
            let under = (0..by).flat_map(|dy| (0..by).map(move |dx| (dx, dy)));
            under.map(at).sum::<f32>() / (by * by) as f32
        };
        let disk = averaged([SIZE / 2, SIZE / 2]);
        let (mut sums, mut counts, mut worst) = ([[0.0f32; 2]; 8], [0u32; 8], 0.0f32);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let covered = averaged([x, y]) / disk;
                if !(0.02..0.98).contains(&covered) {
                    continue;
                }
                let half = SIZE as f32 / 2.0;
                let (dx, dy) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
                let eighth = (math::libm::atan2f(dy, dx) / FRAC_PI_4).round() as i32;
                let eighth = eighth.rem_euclid(8) as usize;
                let off = |frame: &[f32]| (frame[(y * SIZE + x) as usize] / disk - covered).abs();
                sums[eighth][0] += off(&smaa);
                sums[eighth][1] += off(&aliased);
                worst = worst.max(off(&smaa));
                counts[eighth] += 1;
            }
        }
        let means: Vec<[f32; 2]> = (sums.iter().zip(counts))
            .map(|(sum, count)| sum.map(|sum| sum / count as f32))
            .collect();
        let all = |i: usize| {
            let total: f32 = sums.iter().map(|sum| sum[i]).sum();
            total / counts.iter().sum::<u32>() as f32
        };
        (means, [all(0), all(1)], worst)
    };
    // Off by at most this with SMAA, on average, in each eighth and at any pixel: more for the dim
    // disk, whose light 8 bits hold coarsely, and the bright ones under PBR Neutral, whose pixels a
    // fraction covered are darkened by its toe and offset, where they're dim.
    let cases = [
        (0.25, Tonemapper::None, 1.0, [0.1, 0.11, 0.25]),
        (0.02, Tonemapper::None, 1.0, [0.11, 0.12, 0.25]),
        (1.0, Tonemapper::None, 1.0, [0.1, 0.11, 0.25]),
        (16.0, Tonemapper::PbrNeutral, 1.0, [0.12, 0.15, 0.3]),
        (2.0, Tonemapper::PbrNeutral, 0.5, [0.12, 0.15, 0.3]),
    ];
    for (light, tonemapper, slope, [most, most_eighth, most_pixel]) in cases {
        let (means, all, worst) = distances(light, tonemapper, slope);
        assert!(
            all[0] < most && all[1] > 0.2,
            "the disk of {light}'s edge shows its coverage {} off with SMAA, and {} off without \
             antialiasing, on average",
            all[0],
            all[1]
        );
        for (eighth, [antialiased, drawn]) in means.iter().enumerate() {
            assert!(
                *antialiased < most_eighth && *drawn > 0.2,
                "{eighth} eighths round from the right, the disk of {light}'s edge shows its \
                 coverage {antialiased} off with SMAA, and {drawn} off without antialiasing: \
                 {means:?}"
            );
        }
        assert!(
            worst < most_pixel,
            "a pixel of the disk of {light}'s edge shows its coverage {worst} off with SMAA"
        );
        // The disk is the same each way, so its edges across and down blend alike, as do those
        // along either diagonal, rising or falling.
        for first in [0, 1] {
            let alike = (first..8).step_by(2).map(|eighth| means[eighth][0]);
            let (least, most) = alike.fold((f32::MAX, 0.0f32), |(least, most), mean| {
                (least.min(mean), most.max(mean))
            });
            assert!(
                most < 1.2 * least,
                "the disk of {light}'s edges, every other eighth from {first}, blend unalike: \
                 {means:?}"
            );
        }
    }
}
/// A gray square over black, turned 40° in a view of 96, antialiased by SMAA in one frame, against
/// the view drawn at 8 times the size and averaged down: its edges diagonals of single steps broken
/// by steps two pixels long, falling, steeper than 45°, and rising, shallower, which SMAA's
/// diagonal patterns take, as its orthogonal ones would less nearly. Each pixel of its falling
/// edges and of its rising ones shows the share of the square's light that the square covers of it
/// within 0.08 on average, where without the diagonal patterns they're 0.11 off, and every pixel 3
/// or more from its corners, where two edges meet, within 0.3.
fn smaa_blends_along_diagonals_by_how_much_they_cover() {
    const SIZE: u32 = 96;
    const LARGER: u32 = 8;
    const TURNED: f32 = 40.0;
    const SIDE: f32 = 1.6;
    let gray = 0.25;
    // The square drawn as `antialiasing` says, `scale` times the size: its pixels' green, in
    // linear light.
    let render = |scale: u32, antialiasing| {
        let size = SIZE * scale;
        let mut renderer = renderer(size, size);
        let mut stores = Stores::default();
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let square = square(&mut stores.meshes);
        let mut draws = Draws::default();
        let turned = Mat4::from_scale_rotation_translation(
            Vec3::splat(SIDE),
            Quat::from_rotation_z(math::to_radians(TURNED)),
            Vec3::ZERO,
        );
        draws.push(square, flat, turned, Vec4::new(gray, gray, gray, 1.0));
        let (viewport, viewpoint) = (filling([size, size]), seeing_from(Vec3::ZERO));
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let data = RenderData {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            antialiasing,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        let green = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| decoded(pixel[1]));
        green.collect::<Vec<f32>>()
    };
    let smaa = render(1, Antialiasing::Smaa);
    let larger = render(LARGER, Antialiasing::None);
    // How far the square's corners are from its middle, the view's, in pixels: half the view is
    // 4 · tan(π/8) units, 4 away, under a field of view of π/4.
    let half = SIZE as f32 / 2.0;
    let corner = SIDE / SQRT_2 * half / (4.0 * math::libm::tanf(PI / 8.0));
    // Each pixel the square covers 2% to 98% of: how far its share of the square's light is from
    // that, on the falling edges, whose outward normals are 40° and 220°, and on the rising ones.
    let (mut offs, mut worst) = ([Vec::new(), Vec::new()], 0.0f32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let under = (0..LARGER).flat_map(|dy| (0..LARGER).map(move |dx| (dx, dy)));
            let width = SIZE * LARGER;
            let at = |(dx, dy)| larger[((y * LARGER + dy) * width + x * LARGER + dx) as usize];
            let covered = under.map(at).sum::<f32>() / (LARGER * LARGER) as f32 / gray;
            if !(0.02..0.98).contains(&covered) {
                continue;
            }
            let off = (smaa[(y * SIZE + x) as usize] / gray - covered).abs();
            // From the middle, Y up.
            let at = Vec2::new(x as f32 + 0.5 - half, half - (y as f32 + 0.5));
            let around = math::to_degrees(math::libm::atan2f(at.y, at.x));
            let side = ((around - TURNED) / 90.0).round() as i32;
            offs[side.rem_euclid(2) as usize].push(off);
            let near_corner = (0..4).any(|k| {
                let way = math::to_radians(TURNED + 45.0 + 90.0 * k as f32);
                let corner = corner * Vec2::new(math::libm::cosf(way), math::libm::sinf(way));
                at.distance(corner) < 3.0
            });
            if !near_corner {
                worst = worst.max(off);
            }
        }
    }
    for (offs, edges) in offs.iter().zip(["falling", "rising"]) {
        let mean = offs.iter().sum::<f32>() / offs.len() as f32;
        assert!(
            offs.len() > 50 && mean < 0.08,
            "the square's {edges} edges, {} pixels, show their coverage {mean} off on average",
            offs.len()
        );
    }
    assert!(
        worst < 0.3,
        "a pixel of the square's edges away from its corners shows its coverage {worst} off"
    );
}
/// A square of red as bright as a half float holds, 65,504, turned, over white as bright, under no
/// tonemapper, graded by an offset of -65,470 in red, which shows red of the brightest two a half
/// float holds, 65,472 and 65,504, as 1, and red of 65,471 or less as less: antialiased by SMAA,
/// every pixel's red shows as 1, as their blends take light back up to the brightest a half float
/// holds, within the step a write of it to the view's colors may round down by, while its edges'
/// green blends.
fn smaa_blends_light_up_to_a_half_floats_brightest() {
    const SIZE: u32 = 64;
    const BRIGHTEST: f32 = 65504.0;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    let turned = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.6),
        Quat::from_rotation_z(0.3),
        Vec3::ZERO,
    );
    draws.push(square, flat, turned, Vec4::new(BRIGHTEST, 0.0, 0.0, 1.0));
    let (viewport, viewpoint) = (filling([SIZE, SIZE]), seeing_from(Vec3::ZERO));
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mut render = |antialiasing| {
        let data = RenderData {
            clear_color: [BRIGHTEST, BRIGHTEST, BRIGHTEST, 1.0],
            antialiasing,
            grade: Grade {
                offset: Vec3::new(-65470.0, 0.0, 0.0),
                ..Grade::IDENTITY
            },
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        frame_pixels(&mut renderer)
    };
    let aliased = render(Antialiasing::None);
    let antialiased = render(Antialiasing::Smaa);
    shut_down(renderer);
    let (aliased, antialiased) = (aliased.as_chunks::<4>().0, antialiased.as_chunks::<4>().0);
    let dim = |frame: &[[u8; 4]]| frame.iter().position(|pixel| pixel[0] != 255);
    assert_eq!(
        dim(aliased),
        None,
        "without antialiasing, every pixel's red shows as 1"
    );
    let dim = dim(antialiased);
    assert!(
        dim.is_none(),
        "pixel {dim:?}'s red, blended by SMAA, is dimmer than either side's: {:?}",
        dim.map(|i| antialiased[i])
    );
    let blended = (aliased.iter().zip(antialiased))
        .filter(|(a, b)| a[1] != b[1])
        .count();
    assert!(blended > 20, "SMAA blended {blended} pixels' green");
}
fn drawn_again(
    renderer: &mut Renderer,
    stores: &mut Stores,
    viewport: Viewport,
    viewpoint: Viewpoint,
    draws: &Draws,
) -> DrawCounts {
    for _ in 0..2 {
        render_without_ui(renderer, stores.data(viewport, viewpoint, draws));
    }
    renderer.drawn()
}

/// Renders a frame of `data`, without UI.
fn render_without_ui(renderer: &mut Renderer, data: RenderData<'_>) {
    let mut textures = egui::TexturesDelta::default();
    let ui = Ui {
        primitives: &[],
        textures: &mut textures,
        pixels_per_point: 1.0,
    };
    assert!(
        renderer.render(data, ui, &mut []).expect("a frame"),
        "an offscreen image can always be rendered into"
    );
}

/// The `size` pixels at `origin` of a frame `width` pixels wide, rows of RGBA8, tightly packed.
fn region(frame: &[u8], width: u32, origin: [u32; 2], size: [u32; 2]) -> Vec<u8> {
    let row = |y: u32| {
        let start = ((origin[1] + y) * width + origin[0]) as usize * 4;
        &frame[start..start + size[0] as usize * 4]
    };
    (0..size[1]).flat_map(row).copied().collect()
}

/// The pixels of a headless renderer's last frame.
fn frame_pixels(renderer: &mut Renderer) -> Vec<u8> {
    (renderer.read_back())
        .expect("the frame's pixels")
        .expect("a headless renderer's frame can be read back")
}

/// The view a frame read back, as `renderer` gives it, if it read one.
fn view_read(renderer: &Renderer) -> Option<([u32; 2], Vec<u8>)> {
    let read = renderer.view_read().expect("a wait for the frame")?;
    let [width, height] = read.size();
    let mut pixels = vec![0; (width * height * 4) as usize];
    read.read_into(&mut pixels).expect("the view's pixels");
    Some((read.size(), pixels))
}

/// Draws a square in a view at an offset in the frame, which a frame that asks reads back, as it
/// shows in the frame, and one that doesn't reads nothing back; then has a frame render into an
/// offscreen image of another size, which its view fills.
fn views_read_back_as_they_render() {
    let mut renderer = renderer(64, 48);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    draws.push(square, flat, Mat4::IDENTITY, Vec4::new(1.0, 0.5, 0.0, 1.0));
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let viewport = Viewport {
        origin: [8, 4],
        size: [40, 30],
    };
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    for read_view in [true, false] {
        let data = RenderData {
            read_view,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        let shown = region(&frame, 64, viewport.origin, viewport.size);
        let orange = |&[r, g, b, _]: &[u8; 4]| r > 240 && (150..220).contains(&g) && b < 20;
        assert!(
            shown.as_chunks::<4>().0.iter().any(orange),
            "the square shows"
        );
        let read = view_read(&renderer);
        assert_eq!(read, read_view.then_some((viewport.size, shown)));
    }
    renderer.resize(32, 16);
    let data = RenderData {
        read_view: true,
        ..stores.data(filling([32, 16]), viewpoint, &draws)
    };
    render_without_ui(&mut renderer, data);
    let frame = frame_pixels(&mut renderer);
    assert_eq!(
        frame.len(),
        32 * 16 * 4,
        "the frame's image is the new size"
    );
    assert_eq!(view_read(&renderer), Some(([32, 16], frame)));
    shut_down(renderer);
}

/// Shows pixels in the view, as far as they reach of it, the rest of it the clear color: new ones,
/// the same again, ones larger than the view, and ones to show again at a size no frame wrote,
/// which show nothing. Nothing is drawn in the view meanwhile.
fn pixels_show_in_the_view() {
    let mut renderer = renderer(48, 32);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
    let mut draws = Draws::default();
    let huge = Mat4::from_scale(Vec3::splat(100.0));
    draws.push(square, flat, huge, Vec4::new(1.0, 1.0, 1.0, 1.0));
    let viewport = filling([48, 32]);
    let viewpoint = Viewpoint::default();
    let nothing = Draws::default();
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &nothing));
    let clear = frame_pixels(&mut renderer);
    // Each pixel its own, so that a pixel out of its place shows.
    let pattern = |[width, height]: [u32; 2]| -> Vec<u8> {
        let pixel = |i: u32| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255];
        (0..width * height).flat_map(pixel).collect()
    };
    let small = pattern([16, 8]);
    let large = pattern([64, 40]);
    for (size, bytes) in [
        ([16, 8], Some(&small[..])),
        ([16, 8], None),
        ([64, 40], Some(&large[..])),
        ([20, 10], None),
    ] {
        let data = RenderData {
            pixels: Some(Pixels { size, bytes }),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        let expected = match size {
            [16, 8] => {
                let mut expected = clear.clone();
                for y in 0..8 {
                    let at = (y * 48 * 4) as usize;
                    let row = (y * 16 * 4) as usize;
                    expected[at..at + 64].copy_from_slice(&small[row..row + 64]);
                }
                expected
            }
            [64, 40] => region(&large, 64, [0, 0], [48, 32]),
            _ => clear.clone(),
        };
        let off = (frame.iter().zip(&expected)).filter(|&(&a, &b)| a.abs_diff(b) > 1);
        assert_eq!(
            off.count(),
            0,
            "{size:?} pixels, {}",
            bytes.map_or("again", |_| "new")
        );
    }
    shut_down(renderer);
}

/// What [`replaced_shaders_draw_from_the_next_frame`] writes as its shader, in the color `COLOR`
/// stands for, of no entity.
const REPLACED: &str = r#"
struct Vertex {
    float3 position;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID)
    : SV_Position {
    float4 world = mul(push.transforms[instance], float4(push.vertices[index].position, 1.0));
    return mul(push.clip_from_world, world);
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main() {
    Targets out;
    out.color = COLOR;
    out.id = uint2(0, 0);
    return out;
}
"#;

/// A square that covers a view of `SIZE` pixels a side, with a shader written to a file of the
/// test's own: red, then, compiled again from the file rewritten, green, which replaces it while
/// frames before may still draw with the old pipeline, over more frames than are in flight. The
/// frame after draws green, and validation sees the old pipeline destroyed only once the GPU is
/// done with it.
fn replaced_shaders_draw_from_the_next_frame() {
    const SIZE: u32 = 16;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replaced.slang");
    let write = |color: &str| fs::write(&path, REPLACED.replace("COLOR", color));
    write("float4(1.0, 0.0, 0.0, 1.0)").expect("the shader's file");
    let shader = (stores.shaders)
        .add::<[f32; 3], ()>("replaced", &path, Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let mut draws = Draws::default();
    draws.push(square, shader, Mat4::IDENTITY, ());
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let center = |renderer: &mut Renderer| pixel_at(&frame_pixels(renderer), SIZE, [SIZE / 2; 2]);
    // The square drawable first.
    settle(
        &mut renderer,
        &mut stores,
        filling([SIZE, SIZE]),
        viewpoint,
        &draws,
    );
    for frame in 0..6 {
        if frame == 3 {
            write("float4(0.0, 1.0, 0.0, 1.0)").expect("the shader's file");
            let compiled = stores.shaders.sources()[0].compile();
            (stores.shaders).replace(compiled.expect("the shader compiles again"));
        }
        let data = stores.data(filling([SIZE, SIZE]), viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        // Reading back waits for the GPU, so not between the first frame and the replacement.
        match frame {
            0 => assert_eq!(center(&mut renderer), [255, 0, 0, 255], "red"),
            3 | 5 => assert_eq!(center(&mut renderer), [0, 255, 0, 255], "green"),
            _ => {}
        }
    }
    shut_down(renderer);
}

/// Renders a frame with a pipeline cache, which the driver fills as the renderer makes the
/// frame's pipelines, then starts a renderer with what it holds, which renders the frame alike;
/// validation has nothing to say. lavapipe's cache is a stub that holds nothing (Mesa's
/// lvp_pipeline_cache.c), so there it isn't seen to fill.
fn pipelines_go_through_the_cache() {
    const SIZE: u32 = 16;
    let cached = |data: &[u8]| {
        let settings = Settings {
            pipeline_cache: Some(data),
            ..settings(None)
        };
        Renderer::headless(&settings, SIZE, SIZE).expect("a renderer")
    };
    // A triangle in the color its draw gives, which takes a pipeline of its own.
    let frame = |renderer: &mut Renderer| {
        let mut stores = Stores::default();
        let flat = (stores.shaders)
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = (stores.meshes)
            .add("triangle", &corners, &[0, 1, 2])
            .expect("a mesh");
        let mut draws = Draws::default();
        let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
        draws.push(triangle, flat, Mat4::IDENTITY, orange);
        settle(
            renderer,
            &mut stores,
            filling([SIZE, SIZE]),
            Viewpoint::default(),
            &draws,
        );
        frame_pixels(renderer)
    };
    let mut first = cached(&[]);
    let made = first.pipeline_cache().expect("the cache's data");
    let made = made.expect("a cache");
    let pixels = frame(&mut first);
    let filled = first.pipeline_cache().expect("the cache's data");
    let filled = filled.expect("a cache");
    if !first.gpu_name().contains("llvmpipe") {
        assert!(
            filled.len() > made.len(),
            "the cache holds {} bytes once the triangle's pipeline is made, as it did before",
            filled.len()
        );
    }
    let mut second = cached(&filled);
    assert_eq!(frame(&mut second), pixels);
    shut_down(first);
    shut_down(second);
}

/// A renderer asked to capture a frame doesn't start on a GPU that RenderDoc's layer isn't active
/// on, as it isn't here: the tests turn implicit layers off, RenderDoc's among them where it's
/// installed.
fn capturing_needs_renderdoc() {
    let settings = Settings {
        capture: NonZeroU64::new(1),
        ..settings(None)
    };
    let error = (Renderer::headless(&settings, 16, 16).err())
        .expect("a renderer that can't capture doesn't start");
    assert_eq!(error.code(), "renderer.no_renderdoc", "{error:#}");
}

/// Reads back the last frame `renderer` rendered, shuts it down as [`shut_down`] does, and
/// compares the frame with reference image `name`.
fn check(mut renderer: Renderer, name: &str, width: u32, height: u32) {
    let pixels = renderer
        .read_back()
        .expect("the frame's pixels")
        .expect("a headless renderer's frame can be read back");
    shut_down(renderer);
    compare(name, width, height, &pixels);
}

/// Drops `renderer`, and checks that validation reported nothing.
fn shut_down(renderer: Renderer) {
    let validation = renderer.validation();
    drop(renderer);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Compares RGBA8 `pixels` with the reference image `name`, allowing for how GPUs differ in
/// rasterizing and filtering: a pixel matches if no channel is off by more than 16, and up to
/// one pixel in a hundred may not match. Writes the pixels beside the reference when they don't
/// match, to look at.
fn compare(name: &str, width: u32, height: u32, pixels: &[u8]) {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/references")
        .join(format!("{name}.png"));
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        write_png(&reference, width, height, pixels);
        println!("wrote {}", reference.display());
        return;
    }
    let (reference_size, expected) = read_png(&reference);
    assert_eq!(
        reference_size,
        [width, height],
        "{name}'s reference is a different size"
    );
    let mismatched = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0)
        .filter(|(got, want)| got.iter().zip(*want).any(|(&a, &b)| a.abs_diff(b) > 16))
        .count();
    let allowed = (width * height) as usize / 100;
    if mismatched > allowed {
        let actual = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
        write_png(&actual, width, height, pixels);
        panic!(
            "{mismatched} pixels differ from {name}'s reference, more than the {allowed} allowed; \
             the output is at {}",
            actual.display()
        );
    }
}

fn read_png(path: &Path) -> ([u32; 2], Vec<u8>) {
    let file = File::open(path).unwrap_or_else(|error| {
        panic!(
            "can't open {}: {error}; GG_UPDATE_REFERENCES=1 writes it",
            path.display()
        )
    });
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("a PNG");
    let mut pixels = vec![
        0;
        reader
            .output_buffer_size()
            .expect("a PNG's size fits in memory")
    ];
    let info = reader.next_frame(&mut pixels).expect("the PNG's pixels");
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "references are 8-bit RGBA"
    );
    pixels.truncate(info.buffer_size());
    ([info.width, info.height], pixels)
}

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) {
    let file = File::create(path).expect("a file for the image");
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(pixels))
        .expect("the image written");
}
