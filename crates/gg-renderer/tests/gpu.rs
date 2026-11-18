//! GPU tests against reference images, on the loader's GPU (lavapipe in Linux CI).
//!
//! Their main turns off the loader's implicit layers before any thread exists, then runs the
//! tests whose names contain a non-flag argument, or all. Renderers share an instance, except in
//! tests of how one is made. `GG_UPDATE_REFERENCES=1` writes each output as its new reference.

// For setting the environment; see the SAFETY comment.
#![allow(unsafe_code)]

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2};
use std::fs::{self, File};
use std::io::BufWriter;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytemuck::NoUninit;
use gg_core::math::{self, DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_gpu::harness::{self, Tests};
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

/// Runs each test by name; `uploading by` passes and names the upload path.
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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    // SAFETY: no other thread exists yet.
    if unsafe { harness::begin("renderer-gpu-tests", &dir) }.is_none() {
        return ExitCode::SUCCESS;
    }

    let mut tests = Tests::from_args();
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
        reflections_blur_in_place_at_any_view_size,
        a_rough_reflection_slides_with_the_view,
        a_rough_balls_reflection_holds_still_as_the_view_turns,
        a_rough_reflection_takes_in_its_own_surface_alone,
        normals_images_show_as_normals,
        the_back_depth_holds_closed_meshes_far_sides,
        the_traced_reflection_is_unblurred,
        reflection_rays_tell_why_a_ray_misses,
        views_show_parts_of_shading_and_images_in_place_of_light,
        the_light_shows_as_what_surfaces_diffuse_and_what_they_reflect,
        a_small_smooth_balls_highlight_holds_as_it_moves_within_a_pixel,
        a_masked_textures_coverage_holds_as_it_shrinks,
        a_bumpy_planes_highlight_holds_as_it_shrinks,
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
    // Models' shading again, by meshlets through the visibility buffer, against the same
    // references.
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
        normals_images_show_as_normals,
        the_back_depth_holds_closed_meshes_far_sides,
        views_show_parts_of_shading_and_images_in_place_of_light,
        the_light_shows_as_what_surfaces_diffuse_and_what_they_reflect,
        a_small_smooth_balls_highlight_holds_as_it_moves_within_a_pixel,
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
    // Through mesh shaders where the GPU has them, and through vertex shaders.
    for mesh_shaders in [true, false] {
        let settings = Settings {
            mesh_shaders: Some(mesh_shaders),
            ..settings(None)
        };
        if let Err(error) = Renderer::headless_on(&harness::instance(), &settings, 1, 1) {
            assert_eq!(error.code(), "gpu.no_mesh_shaders", "{error:#}");
            println!("tests drawing meshlets through mesh shaders skipped: {error}");
            continue;
        }
        let through = if mesh_shaders { "mesh" } else { "vertex" };
        let name = format!("meshlets_draw_as_their_indices_do through {through} shaders");
        tests.run(&name, || meshlets_draw_as_their_indices_do(mesh_shaders));
    }
    // By the GPU's own upload path alone: the renderer streams through the same calls on every
    // path, and gg-gpu's tests make them on each.
    let path = renderer(1, 1).upload_path();
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
    tests.finish()
}

/// The tests' clear color, linear RGBA.
const CLEAR: [f32; 4] = [0.02, 0.03, 0.05, 1.0];

/// Bytes a frame: a test's larger assets take several frames.
const SMALL_BUDGET: u64 = 64 << 10;

/// Validating, uploading by `uploads` if given, else the GPU's best way.
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

fn renderer(width: u32, height: u32) -> Renderer {
    renderer_uploading(width, height, None)
}

/// [`renderer`], uploading by `uploads` if given.
fn renderer_uploading(width: u32, height: u32, uploads: Option<UploadPath>) -> Renderer {
    renderer_within(width, height, uploads, DEFAULT_UPLOAD_BUDGET)
}

/// [`renderer_uploading`], copying at most `budget` bytes a frame.
fn renderer_within(width: u32, height: u32, uploads: Option<UploadPath>, budget: u64) -> Renderer {
    let settings = Settings {
        upload_budget: budget,
        ..settings(uploads)
    };
    let instance = harness::instance();
    let renderer = Renderer::headless_on(&instance, &settings, width, height).expect("a renderer");
    if let Some(uploads) = uploads {
        assert_eq!(
            renderer.upload_path(),
            uploads,
            "the renderer uploads as asked"
        );
    }
    renderer
}

fn shader(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shaders")
        .join(file)
}

/// A test's assets, their uploads, what its frames shade with, and no draws, for the canvas.
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
    /// A frame cleared to [`CLEAR`], with the stores' lighting and the uploads since the last.
    /// No canvas, picks, outline or post-processing; time 0, a fixed exposure of 1.
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

/// Renders until every mesh and texture added is drawable, which through a transfer queue takes
/// more than a frame.
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

fn filling(size: [u32; 2]) -> Viewport {
    Viewport {
        origin: [0, 0],
        size,
    }
}

fn pixel_at(frame: &[u8], width: u32, [x, y]: [u32; 2]) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    <[u8; 4]>::try_from(&frame[at..at + 4]).expect("a pixel")
}

/// Within 1 a channel, as sRGB encoding may round.
fn close(pixel: [u8; 4], color: [u8; 4]) -> bool {
    pixel.iter().zip(color).all(|(&a, b)| a.abs_diff(b) <= 1)
}

/// `CLEAR`, sRGB-encoded, as a frame reads back.
const CLEARED: [u8; 4] = [39, 48, 63, 255];

/// A linear channel in 8-bit sRGB, as the frame's target encodes it.
fn encoded(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * math::libm::powf(linear, 1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// A unit square of positions, as `flat_color.slang` takes them.
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

fn metallic(metallic: f32, roughness: f32) -> Workflow {
    Workflow::MetallicRoughness {
        metallic,
        roughness,
    }
}

/// A model's mesh of one level.
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

/// Whether [`add_model`] cuts meshlets, for `main`'s second run of the shading tests. A global, as
/// the tests run in turn.
static CUT_MESHLETS: AtomicBool = AtomicBool::new(false);

/// A model's unit square facing +Z, counterclockwise, its texture upright, its tangent +X.
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

/// A window of egui widgets against its reference.
fn ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let draws = Draws::default();

    let egui = egui::Context::default();
    // No animation, so the first frames show the window as it stays.
    egui.global_style_mut(|style| style.animation_time = 0.0);
    let mut value = 0.25;
    // egui sizes a new window from the frame before: the second is the first settled.
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

/// Two squares and a vertex-colored triangle, each of its own vertex type, shader and transform,
/// in part of the image, against its reference. Pixels that differ:
/// - matrices transposed or misordered, or meshes out of order: thousands (the first square,
///   turned and scaled, under the triangle);
/// - a draw given another's parameters: a thousand (the second, a `MeshDraw` joining the first's
///   instanced draw);
/// - the view's image shown off its place: thousands.
///
/// The triangle comes in the last of more frames than are in flight, streamed after the rest.
fn meshes_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 40 pixels a unit.
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
    // Positions, and a color a draw.
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
    // Off the corner, to show where the view is.
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

/// A checkerboard tiled on a square, then a texture with alpha over it and the clear color, each
/// draw naming its texture and sampler, against its reference. What shows a fault:
/// - sharp tiles, twice each way: a sampler ignored clamps and blurs;
/// - a third color at the top left: a flip moves it;
/// - the white texture in slot 0, never drawn: wrong slots show it or each other;
/// - texels opaque, half, a quarter and wholly transparent: blending as opaque or premultiplied
///   changes thousands of pixels;
/// - sRGB: sampled undecoded, colors come out brighter.
///
/// The one with alpha comes in the last frame, streamed after the other.
fn textures_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
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
    // A unit square, its texture upright, over texture coordinates 0 to `extent`.
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

    // sRGB: red and blue, yellow at the top left.
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
    // Pixels 40 to 120, 10 a texel.
    let sampling = Sampling {
        texture: checkerboard,
        sampler: Sampler::NearestRepeat.index(),
    };
    draws.push(tiled, textured, place(-1.0), sampling);
    for frame in 0..3 {
        if frame == 2 {
            // White opaque and half; red a quarter opaque, and nothing.
            let pixels = [
                [255, 255, 255, 255],
                [255, 255, 255, 128],
                [255, 0, 0, 64],
                [0, 0, 0, 0],
            ];
            let translucent = (stores.textures)
                .add("translucent", [2, 2], pixels.as_flattened(), Mipmaps::None)
                .expect("a texture");
            // Over the checkerboard's right half and the clear color, 40 pixels a texel.
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

/// Sprites of a 4 by 4 sheet against their reference: whole, a quarter, flipped, tiled 3 by 2,
/// tinted half transparent, and one of a color alone. What shows a fault:
/// - a color per texel: coordinates flipped, turned or misplaced change hundreds of pixels;
/// - a half transparent quarter: alpha dropped changes hundreds more;
/// - the color-only sprite, the white texture tinted: a tint dropped, or another default, shows.
fn sprites_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // sRGB, red growing rightward, green downward; the bottom left quarter half transparent.
    let sheet: Vec<u8> = (0..16)
        .flat_map(|i| {
            let (x, y) = (i % 4, i / 4);
            let alpha = if x < 2 && y >= 2 { 128 } else { 255 };
            [40 + 60 * x, 40 + 60 * y, 120, alpha]
        })
        .collect();
    let sheet = stores.textures.add("sheet", [4, 4], &sheet, Mipmaps::None);
    let sheet = sheet.expect("a texture");
    // Every sprite's edges fall between pixels.
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
    // Along the top, 60 pixels across: 15 pixels a texel whole, 30 the quarter.
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
    // Tiled 8 pixels a texel, so no pixel's center is on a texel's edge; the tinted one over the
    // orange's right third, 20 pixels.
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

/// Circles against their reference: a disc, a ring a fifth of its radius thick, a disc faded over
/// half its radius, an ellipse, a half transparent disc over a sprite's corner, and discs 8 and 3
/// pixels across, as smooth. Drawing the whole quad, or dropping alpha, changes hundreds of pixels.
fn circles_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let colored = |color: [f32; 4]| Circle {
        color: Vec4::from(color),
        ..Circle::default()
    };
    let mut draws = Draws::default();
    // First, under the purple disc.
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

/// Glyphs of a distance field this test computes, a ring and a square, against their reference:
/// rings 8 to 120 pixels across, a quarter of a pixel a texel to nearly four, each smoothed over a
/// pixel; and squares with square corners, turned, half transparent and small. Drawing the whole
/// quad would change hundreds of pixels.
fn text_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 160;
    /// Each cell's texels across and down, and how far their distances reach.
    const CELL: u32 = 32;
    const RANGE: f32 = 8.0;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // Signed distances inward from the edges, in texels, at each texel's middle.
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
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    let place = |[x, y]: [f32; 2], size: f32, angle: f32| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(size, size, 1.0),
            Quat::from_rotation_z(angle),
            Vec3::new(x, y, 0.0),
        )
    };
    let mut draws = Draws::default();
    // First, under the half transparent square.
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

/// A view's canvas over its world, against its reference: squares in its corners, a half
/// transparent bar and a disc, over a near opaque square and a nearer sprite. Canvas drawn at the
/// far plane, under the square, makes 3,360 pixels differ.
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
    // 0.7 ahead, where the view is about 0.81 by 1.08: all but its right edge.
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
    // 0.55 ahead, about 15 pixels across.
    let in_front = Mat4::from_scale_rotation_translation(
        Vec3::new(0.08, 0.08, 1.0),
        Quat::IDENTITY,
        Vec3::new(0.04, 0.0, 3.45),
    );
    draws.push_sprite(in_front, cyan);
    // In pixels from the view's bottom left.
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

/// Outlines of IDs 3 and 7, 3 pixels wide, against their reference: round a disc and a square of
/// 7; round 7's corner and a square of 3 over it as one shape; stopping at the view's edge; and
/// not round squares of no entity and of 5. The band and glow are read off the disc's row.
/// - Outlines of no IDs, or 0, NaN and negative widths, change no pixel.
/// - Picks are read back first: synchronization validation sees the outline's ID reads wait.
/// - The view read back shows the outline as the frame does.
fn outlines_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
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
    // A frame without an outline, everything drawable.
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
    // Left of the disc (first pixel x = 30) along its middle row, red blended as light from 39 to
    // 255 by alpha: the band solid to 3 pixels, smoothed over the next; then the glow, half the
    // band's alpha at its inner edge, falling by the square to 0 at twice the width.
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

/// Grids against their references, set up as the engine's grid sets them for its camera. On the
/// ground in perspective: antialiased, each power of ten fading into the next, all toward the
/// horizon. In XY, orthographic: crisp, a pixel wide, the line at x = 10 brighter.
fn grids_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let red = Vec3::new(0.8, 0.1, 0.1);
    let green = Vec3::new(0.1, 0.7, 0.1);
    let blue = Vec3::new(0.1, 0.3, 0.9);
    // Showing the horizon; the square a thousand times the eye's height.
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
    // 16 pixels a unit; the square twice the view's width.
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

/// Lines in perspective against their reference: 1, 2, 4 and 8 pixels wide, and half a pixel,
/// drawn a pixel wide at half brightness; a turned square's outline, meeting at its corners; one
/// receding, as wide all along; one to behind the camera, cut at the near plane, else mirrored
/// through the view's middle; one behind an opaque square and one in front; and a wide half
/// transparent one over a sprite.
fn lines_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // At z = 0, 34.6 pixels a unit.
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

/// Three runs, each one instanced draw, against their reference: sprites, a mesh band, and sprites
/// in reverse over it, so one run read as another shows.
/// - Each sprite's texture, tint, scale and turn differ from its neighbors': one drawn with a
///   neighbor's transform or parameters, or not at all, fails the comparison.
/// - The last run has 20,000 sprites, all but six out of view, drawn by the last frame alone, in
///   the first's slot. An arena merges what it grew into once its slot comes round, so the big
///   run's megabyte lands in a buffer grown mid-frame.
fn batches_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    const BIG: usize = 20_000;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // sRGB.
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
    let looks = [
        (white, Vec4::new(0.9, 0.4, 0.1, 1.0)),
        (Some(quadrants), Vec4::ONE),
        (Some(checker), Vec4::new(0.3, 0.8, 1.0, 1.0)),
        (white, Vec4::new(0.6, 0.2, 0.9, 1.0)),
        (Some(quadrants), Vec4::new(1.0, 1.0, 1.0, 0.5)),
        (Some(checker), Vec4::new(1.0, 0.9, 0.3, 1.0)),
    ];
    // Each turned 25° more than the one before, wider or taller by turns.
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
    // Positions, and a color a draw.
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
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
                // The first and last three are seen.
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

/// A sprite sheet's cells, packed edge to edge, magnified, against their reference. Each cell's
/// hue differs from its neighbors', so sampling past a cell's edge shows. Linear and nearest:
/// cells, turned or flipped, side by side, a rectangle across cells, and one cutting texels 0.3
/// in. Linear: a cell of a sheet whose edges' coordinates round to just past them; the sheet tiled
/// past its edge, its edge texels not blending with those they'd wrap to; a rectangle of no width.
///
/// Every edge falls between pixels: on a pixel's center, a nearest sampler takes the neighbor or
/// not as the GPU rounds. So the linear sprites check the shader's bounds, the same for both. The
/// odd sizes, 8 by 4 and 46 by 2, check that each upload path copies rows whole.
fn sheets_match_their_reference(path: UploadPath) {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer_uploading(WIDTH, HEIGHT, Some(path));
    let mut stores = Stores::default();
    // 40 pixels a unit, around the origin.
    let viewpoint = Viewpoint {
        projection: Projection::Orthographic {
            height: 5.0,
            near: -1.0,
            far: 1.0,
        },
        ..Viewpoint::default()
    };
    // 4 by 2 cells, sRGB, each lightest at its top left and darkest at its bottom right.
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
    // 23 cells, yellow and blue by turns. Cell 14's edges, texels 28 and 30, have coordinates
    // that times 46 come to 27.999998 and 30.000002.
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
    // 16 pixels a texel.
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
    // Texels 1 to 5 across, 1 to 3 down.
    let pixel_cells = SpriteSheet {
        cell: [1, 1],
        ..sheet
    };
    let across = pixel_cells.sprite([1, 1], [4, 2]);
    draws.push_sprite(place([-1.8, -0.6], [1.6, 0.8], 0.0), across);
    // 20 pixels a texel: 6 pixels of each edge show the texel it cuts.
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
    let between = strip.sprite([14, 0], [1, 1]);
    draws.push_sprite(place([-2.0, -1.8], [1.2, 0.8], 0.0), between);
    // Texels 6 to 10 across, past the sheet's right edge.
    let tiled = Sprite {
        texture: Some(sheet.texture),
        rect: [Vec2::new(0.75, 0.0), Vec2::new(1.25, 1.0)],
        sampler: Sampler::LinearRepeat,
        ..Sprite::default()
    };
    draws.push_sprite(place([-0.4, -1.8], [1.2, 0.8], 0.0), tiled);
    // On the edge between texels 3 and 4.
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

/// The `meshes` scene and a band of a texel a pixel, in a view resized each frame: whole, shut
/// (the clear color alone), then sizes where each side changes alone. The 160 by 160 frame is the
/// reference, and the same size again must match it. Pixels that differ:
/// - the image left its first size: 15,217;
/// - the aspect ratio from the whole image: 8,917;
/// - the view at the image's corner: 22,662; past its place: 38,400;
/// - the image remade a frame late, or for a new width alone: 8,523;
/// - shown a pixel off: the band's 3,200.
///
/// The fourth frame remakes the image the third may still use: validation sees it destroyed only
/// once the GPU is done.
fn views_match_their_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
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
    // Shifted a texel any way, each shows another color.
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
    // Along the last view's bottom, a texel a pixel, centers on centers.
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
    // Everything drawable first.
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
                // The shut view's nothing, two frames later.
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

/// Depth against its reference: opaque squares crossing, hiding one drawn later, and stacking in
/// draw order at one z as in 2D; a sprite hidden by an opaque square in front; and translucent
/// sprites, which test depth without writing it. The first frame's near square covers most; later
/// frames, their depth cleared, don't remember it. Pixels that differ if opaque squares don't write
/// depth: 2,957; if sprites write it: 1,118; if they don't test it: 1,444.
fn depth_matches_its_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
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

/// Ten instances of a square with two levels of detail, against their reference: level 0 its two
/// triangles; level 1 another, its lower left half, whose error spans a pixel 6.4 from the camera.
/// Near ones show whole squares, far ones halves; picks read each one's ID back.
/// - Five opaque, near and far mixed: drawn by level, two draws, not four.
/// - Five blended, drawn in order: four draws.
/// - Counted: 6 draws, 10 instances, 15 triangles; level 0 alone would make 20 triangles, level 1
///   10, in 2 draws each.
fn levels_of_detail_match_their_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    // 100 / distance pixels a unit.
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
    // Level 1's half is neither of level 0's, so a wrong start shows.
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
    // Center in pixels, distance, color, level. Level 1's error spans 0.05 * 100 / (distance -
    // 1.41) pixels: 1.6 at 4.5, 0.66 at 9.
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
    // ID; pixels inside and outside level 1's triangle, 3 or more off level 0's diagonal at 12;
    // color; level.
    let mut shows = Vec::new();
    for (id, (shader, ([x, y], depth, color, level))) in (1..).zip(instances) {
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
    // Everything drawable first; then three frames, more than the first's picks need.
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
        // Blended over the clear color in linear space.
        let over = |channel: usize| color[channel] * color.w + CLEAR[channel] * (1.0 - color.w);
        let expected = [0, 1, 2].map(over).map(encoded);
        let shown = pixel(inside);
        let close = (shown.iter().zip(expected)).all(|(&got, want)| got.abs_diff(want) <= 3);
        assert!(close, "instance {id} shows {shown:?}, not {expected:?}");
        let other = pixel(outside);
        match level {
            0 => assert_eq!(other, shown, "instance {id}'s other half"),
            _ => assert_eq!(other, clear, "instance {id}'s other half"),
        }
    }
    compare("levels", WIDTH, HEIGHT, &frame);
}

/// A 64 MiB texture of one color, streamed over four frames: once drawable, every pixel shows it,
/// so each part copied whole rows.
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
    // The whole view, 2 units square.
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
    // Still streaming, with whatever else isn't drawable yet.
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

/// A pixel of [`large_assets_stream_over_frames_within_the_budget`]: the clear color while its
/// mesh isn't drawable, the stand-in while its texture isn't, then its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    Clear,
    StandIn,
    Own,
}

/// Streams a texture of colors, 256 KiB, one of data, 96 KiB, and a mesh, 324 KiB, at 64 KiB a
/// frame, reading back a pixel of each every frame. In order:
/// - colors: the clear color while its mesh isn't drawable, the stand-in's white, then its own;
/// - data: the clear color until its own, as its stand-in's zeros are transparent, which shaders
///   discard, as text does;
/// - the mesh: the clear color, then its own.
///
/// What's streaming counts down, and nothing undrawable is counted drawn.
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
    // Opaque, so data's zeros show black.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0, 0.0, 1.0],
        [0.5, -0.5, 0.0, 1.0, 1.0],
        [0.5, 0.5, 0.0, 1.0, 0.0],
        [-0.5, 0.5, 0.0, 0.0, 0.0],
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
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
        // Alpha isn't read: where the zeros draw, it's the view pass's.
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
    // Each pixel's phases in order, the white stand-in's for at least a frame.
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
    // Bytes left fall from under the total to 0.
    assert!(streaming[0].bytes < total, "by {path}: {:?}", streaming[0]);
    for pair in streaming.windows(2) {
        assert!(
            pair[1].bytes <= pair[0].bytes && pair[1].assets <= pair[0].assets,
            "by {path}: {pair:?}"
        );
    }
    let last = streaming.last().expect("a frame");
    assert_eq!((last.assets, last.bytes), (0, 0), "by {path}");
    // The dense mesh isn't counted until drawable.
    assert_eq!(*instances.last().expect("a frame"), 3, "by {path}");
    assert!(instances[0] < 3, "by {path}: {instances:?}");
}

/// Replaces a texture and a mesh with larger ones, streamed over frames: each frame draws the old,
/// never the stand-in, until the new is drawable. The new mesh covers the left half alone.
fn a_reload_draws_the_old_until_the_new_is_drawable(path: UploadPath) {
    const SIDE: u32 = 16;
    const GRID: u32 = 96;
    let mut renderer = renderer_within(SIDE, SIDE, Some(path), SMALL_BUDGET);
    let mut stores = Stores::default();
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, 2 units square: positions, then texture coordinates.
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
    // The left half.
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

/// A BC7 block of one sRGB `color` of odd channels: mode 6, both endpoints `color` (7 bits a
/// channel plus a shared low bit), every index 0. Bits from the least: the mode, bit 6 set; each
/// channel's two endpoints, RGBA; the shared bits; the indices.
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

/// BC7 textures sample as their blocks decode: one level of 13 by 5 texels, whose blocks cover
/// its edges; a chain, sampled trilinearly at its third level; then RGBA8 in place of the first,
/// and BC7 in place of that.
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
    // The whole view, 2 units square: positions, then texture coordinates.
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
    // 16x16 blocks, 8x8, 4x4, 2x2, then three of one; the first a color, the rest another.
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

/// A mesh and texture replaced while earlier frames may still draw them: the new draw once there,
/// and validation, synchronization's too, sees the old go only once the GPU is done.
fn replaced_meshes_and_textures_draw_once_they_are_there(path: UploadPath) {
    const SIZE: u32 = 16;
    let mut renderer = renderer_uploading(SIZE, SIZE, Some(path));
    let mut stores = Stores::default();
    // Positions, then texture coordinates.
    let textured = (stores.shaders)
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, 2 units square.
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
        // Reading back waits for the GPU, so not just before the replacements.
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

/// Removed, a texture shows its stand-in, and a mesh or shader draws nothing and counts no draw.
/// Removing again does nothing; an image or vertices for one removed are dropped. Removed while it
/// streams, a texture shows the stand-in at once; replaced, the replacement streams from the start.
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
    // A quadrant each, the last for a texture that streams.
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

    // Removed after a frame streams part of it.
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

    // Replaced after a frame streams part of it.
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

/// Models with materials against their reference: squares wound as glTF's fronts, facing the
/// camera, turned away, mirrored (so clockwise), both, and double-sided, each culled or drawn as
/// that says; a masked square, mirrored, after the mirrored ones on another pipeline that keeps
/// their faces; a blended square over an opaque one; and corner colors times the material's.
/// Each run of one mesh and shader drawing the same faces is one draw.
fn models_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Viewpoint::default()
    };
    // As `model_square`, `colors` counterclockwise from its bottom left.
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
    // The left column at least half opaque, the right less.
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
    // Cut into meshlets, the two turned away single-sided squares are culled by their cones.
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
    // Past the frame they became drawable in, which drew meshlets by their indices.
    render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
    let drew = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    assert_eq!(drew, drawn);
    check(renderer, "models", WIDTH, HEIGHT);
}

/// A skinned strip of two joints, at its foot and middle, against its reference: its top turned,
/// slid, and given no skin this frame, straight up as bound. Each copy is its own mesh, posed
/// apart, as each skinned model's instance is.
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
    // The lowest two rows by joint 0, the middle half by each, the rest by joint 1.
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

/// Particles against their reference, between squares: a fountain blended by alpha, a fire adding
/// light past 1 against drag, and a burst of spinning sparks of a texture's cells. The second
/// frame's births lie along the fountain's move; the first's are carried on by their ages alone.
/// A third frame at the same time without births, as a paused world's, draws the same.
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
    // Births every `every` ticks from `time`, placed between `from`, at `then`, and `to`, at `now`.
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

/// A run of sprites and a run of model squares, 4 pixels each, alternating two textures and
/// samplers, against their reference. An instance is half a subgroup of 32, so subgroups mix
/// textures: indexing without `NonUniformResourceIndex` makes 3,844 pixels differ on AMD's driver.
fn mixed_textures_match_their_reference() {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 64;
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
    // sRGB; the checker blends under a linear filter.
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

/// Picks answer, two frames on, what drew on top in their own frame: IDs hold a kind and the
/// frame, and two overlapping sprites swap each frame. Through transparent texels, past a circle's
/// edge, on a layer's sprite of no entity and on the clear, they answer what's behind, or 0.
/// Picks outside the view are answered 0 at once; two frames after one without picks, none are.
fn picks_answer_what_their_frame_drew() {
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 64;
    // IDs' high halves; `TOP`, the pair's later.
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
    // A unit a pixel.
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
    // In view pixels from its top left.
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
    // Each pick's pixel and what's on top there, or 0.
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

/// A frame that reads no picks still answers earlier ones. Picks it asks wait for a frame that
/// reads them, which answers the one outside the view at once, and the other two frames later
/// with what it drew.
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

/// Picks along the grid's X axis, drawn over a square at its depth, answer the square: the grid,
/// for seeing by, leaves the IDs under it.
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
    // The axis shows on one of its two rows, away from the major lines.
    let plain = pixel_at(&frame, SIZE, [8, 8]);
    assert!(
        (rows.iter()).any(|&y| !close(pixel_at(&frame, SIZE, [8, y]), plain)),
        "the grid's X axis draws over the square"
    );
    // Answered once their frame is done.
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

/// Every mip level of two textures against their reference. The first, alpha-weighted, has
/// quadrants checkered at 1, 2, 4 and 8 texels, each level evening out one more; its green and
/// clear quadrant blends to half-transparent green, none of the clear's color. The second, 5 by 3,
/// has odd sides, so its levels take parts of three texels. Validation can't see a level unfilled,
/// as the bindless set samples unchecked, or filled from the wrong bytes; both show here.
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
    // sRGB.
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

/// A view painted by a `ViewCallback` between two of the UI's rects, against its reference. Its
/// clip cuts the view and its outline. Shown at `data.viewport`'s origin too, unclipped, or over
/// the later rect, thousands of pixels change. A view of no area draws the UI alone.
fn a_view_in_the_ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
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
    // Across the clip's right edge.
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
        // A point a pixel.
        let clipped = painter.with_clip_rect(rect([40.0, 30.0], [240.0, 170.0]));
        clipped.add(egui::PaintCallback {
            rect: rect([40.0, 20.0], [280.0, 180.0]),
            callback: Arc::new(ViewCallback),
        });
        let green = egui::Color32::from_rgb(60, 200, 60);
        painter.rect_filled(rect([120.0, 80.0], [180.0, 120.0]), 4.0, green);
    });
    let primitives = egui.tessellate(output.shapes, output.pixels_per_point);
    // Without the UI, whose texture changes come once.
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
            // The clear color in the callback's clip; red in the red rect.
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

/// Gray and orange from 0.05 to 16 times white through each tonemapper, against their reference.
/// Gray at 0.5 and 2 maps as each formula says; AgX's is left to the reference.
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
    // Gray at 0.5 and at 2.
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

/// Colors past a half float's most (red, which may round to infinite; infinite red and white) show
/// as white through PBR Neutral, and NaN as black, under each antialiasing, frame after frame. No
/// pixel but at the NaN square is darker than the clear color in a channel and brighter in none,
/// as one blended with NaN would be. Beside the red, TAA's filter spreads some of its light:
/// redder, and darker in green and blue, which PBR Neutral takes more off.
fn colors_past_a_half_float_show_as_its_brightest() {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 16;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
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
            // Left of the NaN square and its neighbor, against the corner's clear color.
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
/// A bright square in the world blooms evenly, fading, and one on the canvas, after the bloom, not
/// at all, with or without TAA; without bloom, nothing spreads. No light passes the three levels'
/// reach: each downsample into level k adds 2^(k + 1) pixels (13 taps, 2 texels out), each
/// upsample from level i + 1 and the final blend 3.5 × 2^i (a bilinear tent 2 texels out), 52.5
/// in all.
fn bloom_spreads_light_round_what_is_bright_and_no_further() {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    // Pixels from the square's edges.
    const REACH: f32 = 52.5;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let flat = (stores.shaders)
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let square = square(&mut stores.meshes);
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
        // Enough frames for TAA's history to settle.
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
    // Red `d` pixels right, left, below and above the square.
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
    // Whether pixel `at` overlaps the reach round the square, on pixels 63 and 64.
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

/// A lone pixel of 65,504 blooms under half as much, 6 to 24 pixels out, as a 4 by 4 square of the
/// same light: Karis's average weighs the first downsample's boxes by 1 / (1 + luminance), so a
/// firefly doesn't bloom into a blot.
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

/// Grades without a tonemapper, each channel within a step of `Grade::graded`, which `grade.rs`
/// checks. The identity and a grade a hair off change nothing; the canvas isn't graded. Against
/// their reference.
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
    // Red over the first square's top left 4 by 4, clear of its middle.
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
    // Each square within a step of what `grade` makes of it on the CPU.
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

/// PBR Neutral's dither breaks a gradient of 2.3 steps of 8 bits into many changes a row, not
/// rounding's 2 band edges, and keeps each 16 columns' mean within a tenth of a step. Black and
/// white stay exact.
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
    // The gradient, or, 0.04 less, what PBR Neutral makes of it.
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
    // Black, and 65,504, which PBR Neutral rolls off to within a millionth of white.
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
    // How often each row's gray changes.
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
    // The gradient in 8-bit steps, sRGB-encoded.
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

/// A uniform sky's exposure adapting, frames 0.1 s apart: the exposure given until the first
/// metering returns in frame 3, 3 stops past the luminance's log2; down 2 stops a second, up 4;
/// compensation in its own frame; off, the exposure given. Each frame's sky shows its luminance
/// times the exposure.
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
    // Checks a frame's exposure value against `expected`, and its sky against 2^`shown` times it.
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

/// A sky 8 stops brighter above the horizon than below, adapting at once, meters the halves' mean,
/// the bright or the dark as its percentiles say. A white canvas, drawn after the meter, and TAA
/// and SMAA, whose colors it reads, change nothing. What's darker than the bins isn't metered.
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
    // The third frame's exposure value, which the first's metering sets, and its middle pixel.
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
    // A white canvas over the view, drawn after the meter, isn't metered.
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

/// The game's view of a bright sky and a tool's look at a dark one, adapting at once: each settles
/// at its own sky's. A frame of the game's view during the look, as a capture is, takes the game's
/// exposure; its metering, two frames later, leaves the look's alone.
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
    // A frame's exposure value and sky pixel, the look's if `looked`.
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

/// Fine sky checkers a hundredfold apart under TAA, whose clip passes much history at such
/// contrast. Held at a quarter of its settled exposure, then adapting, the third frame jumps 4
/// times brighter, yet lands as near the settled view as settled frames do: the resolve scales its
/// history by the change, or it ghosts.
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

/// A lit scene and an unlit square, adapting as the lights drop 8 stops, against a reference of
/// four frames: settled; the drop; a second later, at 4 stops a second; settled again, within a
/// step of 8 bits of the first. The unlit square never changes; a frame at the settled exposure,
/// not adapting, matches.
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

/// What `frame` renders once its exposure moves under a ten-thousandth of a stop over three frames,
/// as a settled one does, its meterings two frames late. Panics past 100 frames.
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

/// The depth of field tests' scene: squares at 2, 4 and 8 over stripes at 12, through 20°
/// vertically (a 68 mm lens on full frame); and each square's middle.
struct FocusScene {
    draws: Draws,
    viewpoint: Viewpoint,
    near: Vec3,
    focused: Vec3,
    far: Vec3,
    /// The same view: a red square at 2, and beside it a yellow one at `past`, just past the focus
    /// (a 2.5-pixel circle), within the tiles the red one's circle reaches, over white at 200.
    beside: Draws,
    past: Vec3,
}

/// `FocusScene`: red, green and blue squares, each over the next one's left edge.
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

/// [`focus_scene`] focused at 4 through f/1.4, circles up to 7.2 pixels (6.3 at infinity): in
/// focus stays sharp, nearer spreads and fades in from its edge, farther blurs, and a larger circle
/// behind never spreads past a nearer one's. Alike under TAA and SMAA, TAA's edges shimmering no
/// more than docs/DEFECTS.md says. f/1000, an orthographic projection and the canvas blur nothing.
fn depth_of_field_blurs_what_is_off_its_focus() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    let size = [WIDTH, HEIGHT];
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let scene = focus_scene(&mut stores);
    // A white canvas square, 8 pixels from the bottom left.
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
    // Every frame drawn: one, or under TAA, enough to settle and a cycle of the offsets after.
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
    // 5 pixels into the yellow square, twice its circle, no blue: the white behind (a 6.2-pixel
    // circle) stops at its own, though the red's widens the gather there to 3.6 texels.
    let [x, y] = pixel_of(viewpoint, size, scene.past + Vec3::new(0.065, 0.0, 0.0));
    let inside = pixel_at(&beside, WIDTH, [x - 5, y]);
    assert!(
        inside[2] <= 8,
        "the white behind the square just past the focus spreads 5 pixels into it: {inside:?}"
    );
    // Under TAA, on the focused square's edges, no channel moves over 16 further across the
    // offsets' cycle than without depth of field. The edge's own pixels take the circle behind in
    // some frames, about 10 further, as docs/DEFECTS.md says.
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
    // The green square, 128 pixels round its middle, as sharp as without depth of field but where
    // the red's blur reaches.
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
    // The red square, 102 pixels round its middle: its light 2 pixels out from its left and top
    // edges, less 2 in, and 4 pixels into the green.
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
    // The blue square, 102 pixels round its middle: its last pixel and the second past it blur.
    let [bx, by] = pixel_of(viewpoint, size, scene.far);
    for p in [[bx + 51, by], [bx + 53, by]] {
        assert!(
            differs(at(&blurred, p), at(&sharp, p), 20),
            "the blue square's edge doesn't blur at {p:?}: {:?}",
            at(&blurred, p)
        );
    }
    // 4 pixels in from its right edge, past its own circle, it stays blue: the stripes' larger
    // circle doesn't spread over it.
    let inside = [bx + 47, by];
    assert!(
        !differs(at(&blurred, inside), at(&sharp, inside), 8),
        "the stripes behind the blue square spread over it to {inside:?}: {:?}",
        at(&blurred, inside)
    );
    // Along a row below the squares, the stripes' least and most come nearer.
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

/// An unlit plate over stripes, the shutter open half the frame, tiles of 6 pixels capping the
/// blur. A moving plate blurs at its side edges alone; a turning camera blurs all but its top edge;
/// a cut, as from a tool's look, blurs only the plate's own motion. The canvas never blurs; TAA and
/// SMAA blur alike. The moving and turning frames, against their reference.
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
    // A white canvas square, 8 pixels from the bottom left.
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
    // The last of 4 frames, cut to if `cut`.
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
    // Cut to, the turn is a jump and blurs nothing; the plate's own motion still blurs it.
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
    // The plate, 0.45 right of the middle at the last frame, spans 65 pixels each way. A pixel past
    // its side edges, over 40 rows, it differs from sharp by a tenth on average in linear light,
    // past the filter's jitter.
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
    // Cut to while turning, the moving plate still blurs by its own motion; the far stripes don't.
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
    // The stripes' contrast along a row above the plate, right of where they end.
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
    // 3 pixels inside the plate's top edge, unchanged; 3 above, none of its red.
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

/// A lens without a tonemapper: the vignette by the cos⁴ law, `(1 + (r / R)²)⁻²`, R the corners'
/// distance; chromatic aberration, red out and blue in by 4% of the distance from the middle;
/// grain of at most a fifth, its deviation within a tenth of the triangular law's, each frame's own
/// and alike across renderers. The canvas shows as drawn, also over grain 1 on light of 65,504.
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
    // A white canvas square, 8 pixels from the bottom left.
    let mut canvas = Draws::default();
    let placed = Mat4::from_translation(Vec3::new(16.0, 16.0, 0.0))
        * Mat4::from_scale(Vec3::new(16.0, 16.0, 1.0));
    canvas.push_sprite(placed, Sprite::default());
    // `frames` frames from a renderer of its own, uploading through the graphics queue, whose
    // copies are drawable at once: frame numbers, and so the grain, match each run.
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
    // Off the canvas: the triangular law's deviation, √(1/6), times the grain's most, 0.5 × 0.2.
    // Rounding to 8 bits, about a hundredth of it, can't cross the bound; half or twice the grain
    // would.
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
    // Over light of a half float's most, grain keeps it there: the canvas would otherwise blend
    // over infinities into NaN, which shows black.
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

/// `frames`, each `width` by `height`, side by side in a row, as a reference holds them.
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

/// A shader shows the frame's data: the camera's position left, time and exposure right. Adapting,
/// the exposure is the first frame's metering, two frames later.
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

/// Model squares lit head-on, each middle against glTF's BRDF by hand: under `E` = π lux and
/// roughness 1 (GGX lobe 1/π, visibility 1/(4 cos)), a surface shows `(1 - f0) c cos + f0 / 4`.
/// They cover unlit, emission, normal and surface maps in each format, and maps never filled. A
/// point of 4π candela 2 above lights the first as the sun did, also with a range, by
/// KHR_lights_punctual's window; a spot, alike inside its inner cone, not at all outside its outer.
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
    // BC7's mode 6 holds odd channels exactly; nearly `tilted`. The shader ignores Z.
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

/// Normal-mapped squares lit from above and in front. A back face whose map tilts toward +Y shows
/// as a front tilted toward -Y, as glTF flips a back face's whole tangent space, bitangent too. A
/// front mirrored across X, winding the other way, shows as unmirrored, double-sided or not.
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
    // Past the frame the mesh became drawable in, which drew it by its indices.
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

/// A model's sphere of `radius` about the origin, `rings` from pole to pole and `segments` around;
/// normals out, tangents eastward along latitude.
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

/// Opaque spheres of one color, rough 0 to 1 left to right and metallic 0 to 1 top to bottom, lit
/// from the upper left front, in perspective, through PBR Neutral, against their reference.
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

/// A floor and spheres under 81 points in a grid past the views' edges (some shine in from
/// outside), four spots, a far point without a range and a shadowing sun. In perspective and
/// orthographically, against references drawn with every light shading every fragment.
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

/// Squares under a uniform sky of 0.5, no lights. Fdez-Agüera's multiple scattering keeps a white
/// surface's reflection whole, so every white one shows 0.5; a smooth colored metal, its color of
/// it; a half occlusion map, half; unlit, its color. A new sky image prefilters again once it
/// shows.
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
    // 2 times its luminance, 0.25: 0.5, below where brighter values clamp.
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

/// A sky of a color each way, through a narrow view: each diagonal round the horizon shows its
/// middle-row column (the center faces +X, a quarter across -Z); up and down, the top and bottom
/// rows; at luminance times exposure. A BC6H block of one color shows as the GPU decodes it.
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
    // BC6H mode 11, bits from the lowest: the mode, 3, in 5 bits; two equal endpoints, red, green
    // and blue in 10 bits each; every index 0. Unsigned, a 10-bit endpoint other than 0 and 1023
    // decodes to the half float of bits 31 times it plus 15: about 0.125, 0.0625 and 0.03125.
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

/// `lit_spheres_match_their_reference`'s spheres lit by a sky alone, which shows behind them: a
/// blue zenith to a pale horizon over dark ground, a bright sun patch high to the upper left.
/// Through PBR Neutral, against their reference.
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

/// The ambient occlusion tests' scene: a floor, walls, spheres ([`BALL`] the large one), a masked
/// panel and a skinned strip, posed a unit along -Z from where it was bound. A view beside a
/// visibility buffer draws the back wall, a game's shader's, and the strip otherwise.
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

/// [`occlusion_scene`] with ambient occlusion and without: it darkens the sky's light near what
/// hides it, the back wall's, the posed strip's and the panel's cut-out depths included, and
/// hardly elsewhere; the back wall reads none. Against its reference, with and without TAA.
/// Settled in a still view, each frame matches the one 8 before, as TAA's resolve asks of a still
/// pixel it keeps unclipped.
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
    // Past the frame the meshes became drawable in, which drew them by their indices.
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
    // Settled, the frame 8 on draws it again, as noise off an 8-frame cycle, or an unsettled
    // history, wouldn't.
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

/// [`occlusion_scene`] seen first looking up, so culling finds most of it outside the view, then as
/// the other tests see it: that frame draws them in the second pass, depth too, and darkens where
/// the floor meets the large sphere. A pick of the sphere in that frame finds it.
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

/// [`occlusion_scene`] in a still view: the occlusion's history averages each frame's noise, with
/// TAA or without, rather than fixing grain on the screen. Settled, a frame keeps at most 0.8 of
/// the first frame's grain, at least 0.4 of which is the first frame's alone. On an RTX 4090,
/// without TAA and with: kept 0.68 and 0.61, left 0.56 and 0.61; a fixed noise, 1 and 0.69, 0 and
/// 0.63.
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
    // Enough for the resolve to settle on the offsets' mean: 400 frames settle no further.
    let antialiased = render(on, Antialiasing::Taa, 128);
    let [width, height] = [WIDTH as usize, HEIGHT as usize];
    let greens = |frame: &[u8]| -> Vec<f64> {
        (frame.as_chunks::<4>().0.iter())
            .map(|pixel| f64::from(pixel[1]))
            .collect()
    };
    // A summed-area table, past a row and a column of 0s.
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
    // Grain: the 3 by 3 mean less the 9 by 9's. The small box averages away rounding to a byte;
    // both follow the occlusion's falloff.
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
    // Darkened floor pixels whose 9 by 9, widened by 2, is all floor, as the resolve blends in
    // what's beside an edge.
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

/// [`floor_and_ball`]'s sphere rolling 1 and 3 pixels a frame, against it settled where it ends:
/// at most 400 floor pixels behind it differ by over 2 levels, as the history is bounded by each
/// frame's own range round each pixel. On an RTX 4090: 1,843 and 2,791 unbounded, 265 and 289
/// bounded; ahead, where the occlusion comes a few frames late, 448 and 546, by at most 8 levels.
fn ambient_occlusion_leaves_no_trail_behind_what_moves() {
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 256;
    const SIZE: [u32; 2] = [WIDTH, HEIGHT];
    const FRAMES: usize = 48;
    let [width, height] = [WIDTH as usize, HEIGHT as usize];
    let unit = HEIGHT as f32 / (2.0 * FLOOR_ABOVE * math::libm::tanf(FLOOR_FOV / 2.0));
    let viewpoint = floor_seen(0, 1, SIZE);
    // Where the sphere is at `frame`, rolling `speed` pixels a frame to the view's middle at
    // `FRAMES`.
    let at = |speed: f32, frame: usize| {
        let x = (frame as f32 - FRAMES as f32) * speed / unit;
        Vec3::new(x, FLOOR_BALL, 0.0)
    };
    // The last of frames `first` to `last`, the sphere stopping where it ends up.
    let run = |speed: f32, first: usize, last: usize| {
        let mut renderer = renderer(WIDTH, HEIGHT);
        let mut stores = Stores {
            ambient_occlusion: Some(AmbientOcclusion::default()),
            ..Stores::default()
        };
        let (floor, round) = floor_and_ball(&mut stores);
        let draws = |frame: usize| floor_with_balls(&floor, &round, [at(speed, frame.min(FRAMES))]);
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
    // The sphere resting where it ends up, alike at either speed.
    let rested = run(1.0, FRAMES, FRAMES + 64);
    for speed in [1.0, 3.0] {
        let rolled = run(speed, 0, FRAMES);
        // Floor pixels behind the sphere, 4 or more off it, differing by more than 2 levels.
        let ball = floor_balls_shown(viewpoint, SIZE, [at(speed, FRAMES)].into_iter(), 4);
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

/// What draws a floor of gray 0.8 and a sphere of [`FLOOR_BALL`] of it, under a sky of 0.5 every
/// way, set in the stores.
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

/// [`floor_and_ball`]'s floor, its spheres in rows [`FLOOR_BALLS_APART`] apart.
fn occlusion_floor(stores: &mut Stores) -> Draws {
    let (floor, round) = floor_and_ball(stores);
    floor_with_balls(&floor, &round, floor_balls())
}

/// Where frame `frame` sees [`occlusion_floor`] from: [`FLOOR_ABOVE`], straight down, panning along
/// X so the floor moves a pixel left every `over` frames in a view of `size`.
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

/// Which pixels, row after row, of a view of `size` from `viewpoint`, a [`floor_seen`], show a
/// sphere of [`FLOOR_BALL`] at one of `centers`, or come within `margin` pixels of one.
fn floor_balls_shown(
    viewpoint: Viewpoint,
    size: [u32; 2],
    centers: impl Iterator<Item = Vec3>,
    margin: usize,
) -> Vec<bool> {
    let [width, height] = size.map(|side| side as f32);
    let tan = math::libm::tanf(FLOOR_FOV / 2.0);
    let mut shows = vec![false; (size[0] * size[1]) as usize];
    for center in centers {
        let to = center - viewpoint.position.as_vec3();
        // Only pixels between the corners of the sphere's bounding box, seen on the plane a unit
        // below the eye, and one more each way, can show it. Testing every pixel took 1.4 s a
        // call in a debug build.
        let r = FLOOR_BALL;
        assert!(-to.y > r, "the spheres are below the eye");
        let depths = [-to.y - r, -to.y + r];
        let pixels = |offset: f32, scale: f32, side: f32| {
            let corners = [offset - r, offset + r].map(|at| depths.map(|depth| at / depth));
            let seen = corners.as_flattened().iter().copied();
            let low = seen.clone().fold(f32::MAX, f32::min);
            let high = seen.fold(f32::MIN, f32::max);
            let pixel = |s: f32| (s / scale + 1.0) * side / 2.0 - 0.5;
            let first = (pixel(low).floor() - 1.0).max(0.0) as u32;
            let last = (pixel(high).ceil() + 1.0).clamp(0.0, side - 1.0) as u32;
            first..last + 1
        };
        for y in pixels(to.z, tan, height) {
            for x in pixels(to.x, tan * width / height, width) {
                let ndc = Vec2::new(
                    (x as f32 + 0.5) / width * 2.0 - 1.0,
                    (y as f32 + 0.5) / height * 2.0 - 1.0,
                );
                // Looking down -Y, the view's up along -Z: its pixels' rows run down along +Z.
                let way = Vec3::new(ndc.x * tan * width / height, -1.0, ndc.y * tan).normalize();
                let along = to.dot(way);
                if (to - way * along).length_squared() <= r * r {
                    shows[(y * size[0] + x) as usize] = true;
                }
            }
        }
    }
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

/// [`occlusion_floor`] panning a pixel a frame and every 8, with TAA and without: between frames 8
/// pixels apart, at the same TAA offset, darkened floor points change by 0.35 of a level at most on
/// average.
/// On an RTX 4090, noise fixed on the screen gave 1.11 and 1.07 without TAA, 0.50 and 0.55 with;
/// the history, 0.24 and 0.27, and 0.21 and 0.24.
fn ambient_occlusion_holds_to_the_world_as_the_view_pans() {
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 256;
    const SIZE: [u32; 2] = [WIDTH, HEIGHT];
    // The pixels a point moves between the frames compared.
    const MOVED: usize = 8;
    // The occlusion's reach, 54 pixels here: its default radius times XeGTAO's 1.457. XeGTAO
    // repeats the view's edge pixels past it, so points within reach of an edge change as it pans,
    // by 4 to 10 levels on average.
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
                // Darkened, 6 pixels clear of the spheres' edges, out of reach of the view's in
                // both frames, and shown for 48 frames, entering from the right.
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

/// [`occlusion_scene`] under a black sky and a shadowing sun renders alike with ambient occlusion
/// and without: it darkens the sky's light alone, never the lights'. Its passes neither run nor
/// take memory without a sky, under an orthographic projection, or at strength 0; with a sky,
/// under perspective, they do.
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

/// The reflection tests' scene: three metal floor strips, rough as `roughness` says, normal-mapped
/// by `normals` if given, under a red sphere, a green one and a blue panel. `mirrored` is the red
/// sphere's middle in the floor's mirror; `open`, a point whose mirror sees sky alone.
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

/// [`reflection_scene`] with reflections: the first frame, with no colors kept, renders as without;
/// the next mirrors the red sphere, and the sky where the mirror sees sky alone. Against its
/// reference. TAA and SMAA, whose blended colors are kept, still mirror the sphere, also after the
/// camera moves past what SMAA's last frame kept.
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
    // The camera moved farther than the sphere is wide: what a ray finds comes from where it
    // showed the frame before.
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
    // The sky twice as bright, the exposure an eighth: the mirrored sphere too a quarter as
    // bright, as reflections take the kept colors, exposed, as they are.
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

/// [`reflection_scene`] adapting at once from the exposure its kept colors were drawn at: the third
/// frame, about 2 stops darker, mirrors the sphere as the settled frames do, the kept colors scaled
/// by the exposure's change.
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

/// [`reflection_scene`] under a sky of luminance 0, as an atmosphere's after sunset, lit by a sun
/// alone, exposed at a half. The mirror shows the lit red sphere, though the sky's specular light
/// that reflections replace is none; without reflections, next to nothing.
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

/// [`reflection_scene`] renders alike with reflections and without:
/// - every surface rougher than their roughness (strips 0.8, shapes 0.4 and 0.5, against 0.3),
///   which take the sky's light alone;
/// - under an orthographic projection, which they don't take;
/// - without a sky, whose specular light they'd replace.
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

/// [`reflection_scene`]'s smooth strips, normal-mapped about 12° toward the camera, drawn each way
/// the view draws: by meshlets into the visibility buffer, and by indices, alone and beside a ball
/// cut into meshlets. The floor mirrors the red sphere where the mapped normal says, not where a
/// flat mirror would: rays trace about the shading normal, not the depth's.
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
    // The map's normal in the world, as shading.slang's `shading_normal` bends the floor's (+Y)
    // along its tangent (+X) and bitangent (the square's +Y, -Z).
    let mapped = Vec2::new(128.0, 100.0) / 255.0 * 2.0 - 1.0;
    let up = (1.0 - mapped.length_squared()).sqrt();
    let tilted = (Vec3::X * mapped.x - Vec3::Z * mapped.y + Vec3::Y * up).normalize();
    // The point of the first strip whose camera ray, mirrored about `normal`, passes nearest the
    // red sphere's middle.
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

/// [`reflection_scene`]'s blue panel in a rough strip's mirror, the reflection alone, 380 pixels
/// across and 384: halvings of the first drop a column from 95, of the second none. The narrower
/// view shows all 2 pixels further left, the mirror's middle too. Sampled as if levels that dropped
/// columns still spanned the view, the blur would shift the narrower's right, more the farther
/// right: 25 pixels here.
fn reflections_blur_in_place_at_any_view_size() {
    const HEIGHT: u32 = 256;
    // Where the panel's mirror centers across in the view's right half, weighed by how much bluer
    // than red and green it shows, and that weight.
    let center = |width: u32| {
        let mut renderer = renderer(width, HEIGHT);
        let mut stores = Stores::default();
        let viewport = filling([width, HEIGHT]);
        let scene = reflection_scene(&mut stores, [0.05, 0.05, 0.4], None);
        let (viewpoint, draws) = (scene.viewpoint, &scene.draws);
        settle(&mut renderer, &mut stores, viewport, viewpoint, draws);
        let reflection = Channel::Reflection;
        let shown = reflection_alone(
            &mut renderer,
            &mut stores,
            viewport,
            viewpoint,
            draws,
            reflection,
        );
        shut_down(renderer);
        let (mut sum, mut weight) = (0.0, 0.0);
        for (i, [r, g, b]) in (0_u32..).zip(shown) {
            let x = i % width;
            if x >= width / 2 {
                let blue = (b - r.max(g)).max(0.0);
                sum += blue * (x as f32 + 0.5);
                weight += blue;
            }
        }
        (sum / weight, weight)
    };
    let (narrower, seen) = center(380);
    let (wider, _) = center(384);
    assert!(seen > 5.0, "the panel's mirror weighs {seen}");
    assert!(
        (narrower + 2.0 - wider).abs() < 1.0,
        "the panel's mirror centers at {narrower} 380 pixels across, at {wider} 384 across"
    );
}

/// The blur tests' scene, for a view 380 by 256: a metal floor, rough as `roughness` says, before
/// an unlit wall of `wall` 9 units ahead, and an unlit square of `color` facing the camera, `size`
/// across and up from `at`, which looks on from 1.2 up, as over [`reflection_scene`]'s.
fn backdrop_scene(
    stores: &mut Stores,
    roughness: f32,
    wall: Vec4,
    at: Vec3,
    size: Vec2,
    color: Vec4,
) -> (Draws, Viewpoint) {
    let sky = sky_texture(&mut stores.textures, [128, 64], |_, _| [0.5; 3]);
    stores.sky = Some(Sky {
        texture: sky,
        luminance: 1.0,
    });
    let square = model_square(&mut stores.meshes);
    let mut draws = Draws::default();
    let mut add = |stores: &mut Stores, material: &Material, transform| {
        let material = stores.materials.add(material);
        let draw =
            MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(transform, &draw, 0);
    };
    let floor = Material {
        color: Vec4::new(0.9, 0.9, 0.9, 1.0),
        workflow: metallic(1.0, roughness),
        ..Material::default()
    };
    let flat = Quat::from_rotation_x(-FRAC_PI_2);
    let under = Vec3::new(0.0, 0.0, -3.0);
    let floor_at = Mat4::from_scale_rotation_translation(Vec3::new(12.0, 14.0, 1.0), flat, under);
    add(stores, &floor, floor_at);
    let unlit = |color| Material {
        color,
        unlit: true,
        ..Material::default()
    };
    let standing = |size: Vec2, at: Vec3| {
        let middle = at + Vec3::Y * size.y / 2.0;
        Mat4::from_scale_rotation_translation(size.extend(1.0), Quat::IDENTITY, middle)
    };
    let behind = Vec3::new(0.0, 0.0, -5.0);
    add(stores, &unlit(wall), standing(Vec2::new(12.0, 4.0), behind));
    add(stores, &unlit(color), standing(size, at));
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 1.2, 4.0),
        rotation: Quat::from_rotation_x(-0.2),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
    };
    (draws, viewpoint)
}

/// Renders a frame of `draws` from `viewpoint` keeping the view's colors, and then the reflection
/// alone, traced against them, as linear light: as shading takes it, or as traced (`channel`).
fn reflection_alone(
    renderer: &mut Renderer,
    stores: &mut Stores,
    viewport: Viewport,
    viewpoint: Viewpoint,
    draws: &Draws,
    channel: Channel,
) -> Vec<[f32; 3]> {
    for show in [None, Some(Show::Shading(channel))] {
        let data = RenderData {
            reflections: Some(Reflections::default()),
            show,
            ..stores.data(viewport, viewpoint, draws)
        };
        render_without_ui(renderer, data);
    }
    let frame = frame_pixels(renderer);
    let pixels = frame.as_chunks::<4>().0.iter();
    pixels
        .map(|pixel| [0, 1, 2].map(|c| decoded(pixel[c])))
        .collect()
}

/// [`backdrop_scene`]'s white bar, 0.1 wide and 1.5 up, before a black wall, in the floor's mirror
/// as the camera steps right 2 pixels at a time over 16: on a rough floor, the reflection alone, the
/// mirror's middle moves each step as a smooth floor's does. Levels of 2 by 2 means would hold it at
/// a coarse texel's middle, then jump it to the next: a grid the reflection steps across as the view
/// moves. A blur as wide as each pixel's own lobe, which varies, can hold it a pixel off a smooth
/// floor's, as it does here, where it blurs about 14 pixels.
fn a_rough_reflection_slides_with_the_view() {
    const WIDTH: u32 = 380;
    const HEIGHT: u32 = 256;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let (black, white) = (Vec4::new(0.0, 0.0, 0.0, 1.0), Vec4::ONE);
    let (at, size) = (Vec3::new(0.5, 1.5, -3.5), Vec2::new(0.1, 1.5));
    let (rough, viewpoint) = backdrop_scene(&mut stores, 0.3, black, at, size, white);
    let (smooth, _) = backdrop_scene(&mut stores, 0.05, black, at, size, white);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &rough);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &smooth);
    // Where the bar's mirror centers across, weighed by its red light.
    let mut center = |draws: &Draws, viewpoint| {
        let reflection = Channel::Reflection;
        let shown = reflection_alone(
            &mut renderer,
            &mut stores,
            viewport,
            viewpoint,
            draws,
            reflection,
        );
        let (mut sum, mut weight) = (0.0, 0.0);
        for (i, [r, _, _]) in (0_u32..).zip(shown) {
            sum += r * ((i % WIDTH) as f32 + 0.5);
            weight += r;
        }
        sum / weight
    };
    // Two pixels at the bar's mirror, about 7.8 units off, where a unit a unit away is 128 /
    // tan(30°) pixels.
    let step = 2.0 * 7.8 / (128.0 / math::libm::tanf(PI / 6.0));
    let centers: Vec<_> = (0..8)
        .map(|k| {
            let moved = Viewpoint {
                position: viewpoint.position + DVec3::X * f64::from(k as f32 * step),
                ..viewpoint
            };
            (center(&rough, moved), center(&smooth, moved))
        })
        .collect();
    let (first, last) = (centers[0].1, centers[7].1);
    assert!(
        (first - last).abs() > 8.0,
        "the bar's mirror moved from {first} to {last}"
    );
    for (k, pair) in centers.windows(2).enumerate() {
        let [(rough, smooth), (rough_next, smooth_next)] = [pair[0], pair[1]];
        let [moved, moved_smooth] = [rough_next - rough, smooth_next - smooth];
        assert!(
            (moved - moved_smooth).abs() < 0.75,
            "at step {k}, the bar's mirror moves {moved} on a rough floor, {moved_smooth} on a \
             smooth one: {centers:?}"
        );
    }
    shut_down(renderer);
}

/// A metal ball 0.5 rough, 86 pixels across, beside a white unlit ball 0.5 off, over
/// [`backdrop_scene`]'s floor, too rough to reflect, and black wall, as the view turns right 2
/// pixels at a time over 64: the reflection alone, the white ball's light on the metal one neither
/// brightens and dims nor wanders over it, past a steady drift as perspective stretches the view's
/// sides. A flat mirror's blur, 3 times as wide on this ball, read levels whose texels span a
/// third of it, and swam as it moved under them.
fn a_rough_balls_reflection_holds_still_as_the_view_turns() {
    const WIDTH: u32 = 380;
    const HEIGHT: u32 = 256;
    const STEPS: usize = 32;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let black = Vec4::new(0.0, 0.0, 0.0, 1.0);
    // The backdrop's square, small and as black as the wall.
    let (at, size) = (Vec3::new(0.0, 0.0, -4.0), Vec2::splat(0.1));
    let (mut draws, viewpoint) = backdrop_scene(&mut stores, 0.7, black, at, size, black);
    // A dark sky: the white ball's light alone, not the floor's too.
    let sky = stores.sky.expect("the scene's sky");
    stores.sky = Some(Sky {
        luminance: 0.0,
        ..sky
    });
    let center = Vec3::new(0.0, 0.5, 1.5);
    let metal = Material {
        color: Vec4::ONE,
        workflow: metallic(1.0, 0.5),
        ..Material::default()
    };
    let white = Material {
        color: Vec4::ONE,
        unlit: true,
        ..Material::default()
    };
    for (radius, x, material) in [(0.5, 0.0, metal), (0.4, 1.4, white)] {
        let ball = sphere(&mut stores.meshes, radius, 24, 48);
        let material = stores.materials.add(&material);
        let draw = MeshDraw::with_material(ball, material, &stores.materials, &mut stores.shaders);
        draws.push_draw(Mat4::from_translation(center + Vec3::X * x), &draw, 0);
    }
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // Two pixels at the view's middle, where a unit a unit away is 128 / tan(30°) pixels.
    let step = 2.0 * math::libm::tanf(PI / 6.0) / 128.0;
    let seen: Vec<_> = (0..STEPS)
        .map(|k| {
            let turned = Viewpoint {
                rotation: Quat::from_rotation_y(-step * k as f32) * viewpoint.rotation,
                ..viewpoint
            };
            let shown = reflection_alone(
                &mut renderer,
                &mut stores,
                viewport,
                turned,
                &draws,
                Channel::Reflection,
            );
            // The ball's light, and how far right of its middle that light centers.
            let (mut light, mut across) = (0.0, 0.0);
            for (i, [r, _, _]) in (0_u32..).zip(shown) {
                light += r;
                across += r * ((i % WIDTH) as f32 + 0.5);
            }
            let clip = turned.view_projection(WIDTH as f32 / HEIGHT as f32) * center.extend(1.0);
            let middle = (clip.x / clip.w + 1.0) / 2.0 * WIDTH as f32;
            (light, across / light - middle)
        })
        .collect();
    // How far the values stray, at most, from the parabola that fits them best over the steps: a
    // swim's cycles, which a steady drift doesn't have.
    let stray = |values: &[f32]| {
        let middle = (STEPS - 1) as f32 / 2.0;
        let xs = || (0..STEPS).map(|k| k as f32 - middle);
        let mean_square = xs().map(|x| x * x).sum::<f32>() / STEPS as f32;
        // 1, x and x² less its mean are orthogonal over steps even about the middle: each fits
        // apart.
        let basis = |x: f32| [1.0, x, x * x - mean_square];
        let fit: [f32; 3] = std::array::from_fn(|i| {
            let (along, squares) = xs()
                .zip(values)
                .fold((0.0, 0.0), |(along, squares), (x, v)| {
                    let b = basis(x)[i];
                    (along + b * v, squares + b * b)
                });
            along / squares
        });
        xs().zip(values)
            .map(|(x, v)| (v - basis(x).iter().zip(fit).map(|(b, c)| b * c).sum::<f32>()).abs())
            .fold(0.0, f32::max)
    };
    let lights: Vec<_> = seen.iter().map(|(light, _)| *light).collect();
    let mean = lights.iter().sum::<f32>() / STEPS as f32;
    let pulse = stray(&lights) / mean;
    let wander = stray(&seen.iter().map(|(_, off)| *off).collect::<Vec<_>>());
    assert!(
        mean > 10.0,
        "the metal ball shows {mean} of the white ball's light"
    );
    assert!(
        pulse < 0.09 && wander < 0.6,
        "as the view turns, the ball's light strays {pulse} of its mean and its middle {wander} \
         pixels from steady: {seen:?}"
    );
    shut_down(renderer);
}

/// [`backdrop_scene`]'s rough floor mirroring the wall beside a post 0.6 wide, as blue as the wall,
/// whose own rays turn back toward the view and find nothing: the reflection alone shows the wall's
/// mirror as bright 3 pixels beside the post as 60 pixels further off. Blurring in the post's
/// pixels, of another surface, would dim it beside the post by their trust of 0.
fn a_rough_reflection_takes_in_its_own_surface_alone() {
    const WIDTH: u32 = 380;
    const HEIGHT: u32 = 256;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let (at, blue) = (Vec3::new(0.5, 0.0, -1.0), Vec4::new(0.2, 0.4, 0.9, 1.0));
    let (draws, viewpoint) = backdrop_scene(&mut stores, 0.4, blue, at, Vec2::splat(0.6), blue);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let reflection = Channel::Reflection;
    let shown = reflection_alone(
        &mut renderer,
        &mut stores,
        viewport,
        viewpoint,
        &draws,
        reflection,
    );
    // The post's left side, halfway up.
    let [x, y] = pixel_of(viewpoint, [WIDTH, HEIGHT], at + Vec3::new(-0.3, 0.3, 0.0));
    let blue = |x: u32| shown[(y * WIDTH + x) as usize][2];
    let (beside, off) = (blue(x - 3), blue(x - 63));
    assert!(
        off > 0.1 && beside > 0.9 * off,
        "the wall's mirror shows {beside} blue beside the post, {off} further off"
    );
    shut_down(renderer);
}

/// [`backdrop_scene`]'s floor and wall in the normals reflections trace from, the visibility
/// buffer's or the model shaders', shown as [`Channel::Normal`] shows a normal: up, and toward the
/// view. Shown as light, the axes of 0 would be black.
fn normals_images_show_as_normals() {
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 64;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
    let (at, size) = (Vec3::new(0.0, 0.0, -1.0), Vec2::splat(0.6));
    let (draws, viewpoint) = backdrop_scene(&mut stores, 0.1, gray, at, size, gray);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let name = if CUT_MESHLETS.load(Ordering::Relaxed) {
        "view normals"
    } else {
        "opaque normals"
    };
    // The first frame keeps the colors the second traces against, which makes the normals.
    for show in [None, Some(Show::Image(name))] {
        let data = RenderData {
            reflections: Some(Reflections::default()),
            show,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
    let shown = frame_pixels(&mut renderer);
    for (what, point, normal) in [
        ("floor", Vec3::new(-1.5, 0.0, 1.0), [128, 255, 128, 255]),
        ("wall", Vec3::new(-3.0, 3.0, -5.0), [128, 128, 255, 255]),
    ] {
        let got = pixel_at(&shown, WIDTH, pixel_of(viewpoint, [WIDTH, HEIGHT], point));
        assert!(
            close(got, normal),
            "{name} shows the {what} as {got:?}, not {normal:?}"
        );
    }
    shut_down(renderer);
}

/// [`backdrop_scene`]'s floor and wall under two balls 0.5 in radius, the right one drawn mirrored,
/// in the depth of what's opaque and of its back faces, as [`Show::Image`] shows depth: at each
/// ball's middle, as far as its front and its back. The floor and wall hide nothing facing away, so
/// they have no back depth.
fn the_back_depth_holds_closed_meshes_far_sides() {
    const WIDTH: u32 = 96;
    const HEIGHT: u32 = 64;
    const RADIUS: f32 = 0.5;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
    let (at, size) = (Vec3::new(3.0, 0.0, -4.0), Vec2::splat(0.1));
    let (mut draws, viewpoint) = backdrop_scene(&mut stores, 0.1, gray, at, size, gray);
    let ball = sphere(&mut stores.meshes, RADIUS, 24, 48);
    let white = stores.materials.add(&Material {
        color: Vec4::ONE,
        unlit: true,
        ..Material::default()
    });
    let draw = MeshDraw::with_material(ball, white, &stores.materials, &mut stores.shaders);
    let centers = [Vec3::new(-0.8, 0.5, 2.5), Vec3::new(0.8, 0.5, 2.5)];
    for (center, x) in centers.into_iter().zip([1.0, -1.0]) {
        let scale = Vec3::new(x, 1.0, 1.0);
        let transform = Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, center);
        draws.push_draw(transform, &draw, 0);
    }
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // The first frame keeps the colors the second traces against, which draws the back faces.
    let mut shown = |name| {
        for show in [None, Some(Show::Image(name))] {
            let data = RenderData {
                reflections: Some(Reflections::default()),
                show,
                ..stores.data(viewport, viewpoint, &draws)
            };
            render_without_ui(&mut renderer, data);
        }
        frame_pixels(&mut renderer)
    };
    let (front, back) = (shown("view depth"), shown("opaque back depth"));
    // A depth is the near plane's distance over the distance ahead.
    let Projection::Perspective { near, .. } = viewpoint.projection else {
        unreachable!("the backdrop's view has perspective");
    };
    let shade = |ahead: f32| 255.0 * (1.0 + math::libm::log2f(near / ahead) / 24.0);
    let eye = viewpoint.position.as_vec3();
    for center in centers {
        let at = pixel_of(viewpoint, [WIDTH, HEIGHT], center);
        let way = center - eye;
        let ahead = way.normalize().dot(viewpoint.rotation * Vec3::NEG_Z);
        let sides = [
            ("front", &front, way.length() - RADIUS),
            ("back", &back, way.length() + RADIUS),
        ];
        for (side, image, along) in sides {
            let got = f32::from(pixel_at(image, WIDTH, at)[0]);
            let expected = shade(along * ahead);
            assert!(
                (got - expected).abs() <= 2.0,
                "the ball at {center} shows its {side} as {got}, not {expected}"
            );
        }
    }
    // The floor between the balls, and the wall over them.
    for (what, point) in [
        ("floor", Vec3::new(0.0, 0.0, 1.0)),
        ("wall", Vec3::new(-3.0, 3.0, -5.0)),
    ] {
        let at = pixel_of(viewpoint, [WIDTH, HEIGHT], point);
        let (front, back) = (pixel_at(&front, WIDTH, at), pixel_at(&back, WIDTH, at));
        assert!(
            front[0] > 0 && back == [0, 0, 0, 255],
            "the {what} shows as {front:?} in front, {back:?} behind"
        );
    }
    shut_down(renderer);
}

/// [`a_rough_reflection_slides_with_the_view`]'s white bar, in a floor of roughness 0.4's mirror:
/// as traced, about as wide as the bar shows (3 pixels); as shading blurs it, several times wider.
/// Counted along the row through the mirror's middle, the pixels over a tenth of its brightest.
fn the_traced_reflection_is_unblurred() {
    const WIDTH: u32 = 380;
    const HEIGHT: u32 = 256;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let (black, white) = (Vec4::new(0.0, 0.0, 0.0, 1.0), Vec4::ONE);
    let (at, size) = (Vec3::new(0.5, 1.5, -3.5), Vec2::new(0.1, 1.5));
    let (draws, viewpoint) = backdrop_scene(&mut stores, 0.4, black, at, size, white);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    let mirrored = Vec3::new(0.5, -2.25, -3.5);
    let [_, y] = pixel_of(viewpoint, [WIDTH, HEIGHT], mirrored);
    let mut wide = |channel| {
        let shown = reflection_alone(
            &mut renderer,
            &mut stores,
            viewport,
            viewpoint,
            &draws,
            channel,
        );
        let row = &shown[(y * WIDTH) as usize..((y + 1) * WIDTH) as usize];
        let brightest = row.iter().map(|[r, _, _]| *r).fold(0.0, f32::max);
        let wide = row.iter().filter(|[r, _, _]| *r > brightest / 10.0).count();
        (wide, brightest)
    };
    let traced = wide(Channel::TracedReflection);
    let blurred = wide(Channel::Reflection);
    assert!(
        traced.1 > 0.1 && traced.0 <= 5 && blurred.0 >= 12,
        "the bar's mirror shows (pixels, brightest) {traced:?} as traced, {blurred:?} blurred"
    );
    shut_down(renderer);
}

/// [`backdrop_scene`]'s smooth floor's rays, as [`Channel::ReflectionRays`] colors them. Green: one
/// to the wall, found and trusted. Red: one passing under a square standing over the floor, then
/// behind its depth. Blue: one from near the camera, over the wall, out of the view.
fn reflection_rays_tell_why_a_ray_misses() {
    const WIDTH: u32 = 380;
    const HEIGHT: u32 = 256;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut stores = Stores::default();
    let viewport = filling([WIDTH, HEIGHT]);
    let gray = Vec4::new(0.5, 0.5, 0.5, 1.0);
    let (at, size) = (Vec3::new(0.0, 1.0, -1.0), Vec2::ONE);
    let (draws, viewpoint) = backdrop_scene(&mut stores, 0.1, gray, at, size, gray);
    settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
    // The first frame keeps the colors the second's trace reads.
    for show in [None, Some(Show::Shading(Channel::ReflectionRays))] {
        let data = RenderData {
            reflections: Some(Reflections::default()),
            show,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
    let shown = frame_pixels(&mut renderer);
    // The floor where it mirrors the wall 2 up, and the point under the square; and near the camera.
    let pixel = |at| pixel_at(&shown, WIDTH, pixel_of(viewpoint, [WIDTH, HEIGHT], at));
    let wall = pixel(Vec3::new(-3.0, -2.0, -5.0));
    let behind = pixel(Vec3::new(0.0, -0.6, -1.0));
    let near = pixel(Vec3::new(-1.0, 0.0, 2.3));
    assert!(
        wall[1] > 200 && wall[0] < 50 && behind[0] > 200 && behind[1] < 50,
        "the wall's ray shows {wall:?}, the one behind the square {behind:?}"
    );
    assert!(close(near, [0, 0, 255, 255]), "the near ray shows {near:?}");
    shut_down(renderer);
}

/// Squares under a sky of 0.5, occlusion maps of a half: diffuse darkened by the half, specular by
/// Lagarde and de Rousiers' specular occlusion, of the way to the eye and the roughness. A rough
/// dielectric darkens by a half; a smooth metal head-on, less; grazing, more. A smooth dielectric
/// mixes the two by its reflected share, `f`, which a black one without a map shows.
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
    // What a dielectric reflects head-on, as the black square shows: Fresnel's 0.04 and multiple
    // scattering.
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

/// Squares through a frame that would otherwise antialias, bloom and vignette, each showing a part
/// of shading, as bytes over 255, or an image the frame makes, black where nothing drew and for an
/// image it doesn't make. A frame of light after matches one before within 1: it's at TAA's next
/// offset, as showing frames resolve none, and lavapipe rounds some meshlet pixels otherwise there.
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
    // The unlit square, the second, shows its base color alone.
    let parts = [
        (
            Channel::BaseColor,
            [encoded_color, encoded_color, encoded_color],
        ),
        (Channel::Normal, [up, [0; 3], up]),
        (Channel::SurfaceNormal, [up, [0; 3], up]),
        (Channel::Metallic, [[value(0.25); 3], [0; 3], [0; 3]]),
        (
            Channel::Roughness,
            [[value(0.75); 3], [0; 3], [value(0.5); 3]],
        ),
        (Channel::Occlusion, [[255; 3], [0; 3], [255; 3]]),
        (Channel::Emission, [[encoded(0.5), 0, 0], [0; 3], [0; 3]]),
        // 0.04 mixed with the color by how metallic it is, and the specular color.
        (
            Channel::SpecularColor,
            [
                [encoded(0.155), encoded(0.0925), encoded(0.28)],
                [0; 3],
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
    // Images of the passes after the view's, drawn without antialiasing, at the middle square,
    // which the vignette leaves bright.
    for name in ["bloom 1", "view lensed"] {
        let shown = frame(Some(Show::Image(name)));
        let got = square(&shown, 1);
        assert!(
            got[..3].iter().any(|&channel| channel > 8),
            "{name} shows the middle square as {got:?}"
        );
    }
    // And the sky behind, in the colors as drawn: its gray of luminance 0.25, not the clear color.
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
    // The images a show finds, each name once: showing an image, the game's frame's but the show's
    // own; showing shading, none after the view's passes; antialiased, none of antialiasing's.
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
    // From the viewpoint, so each highlight faces it.
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::NEG_Z,
        color: Vec3::splat(1.0),
        range: None,
        shadow_distance: None,
    }];
    // A dielectric, a metal and an unlit square.
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
    // Each square's linear light in `channel`, untonemapped.
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
    // The dielectric's highlight, about 0.05, plus the sky's 0.01.
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

/// A mirror-smooth ball 8 pixels across, lit from the view, reflects about the same light wherever
/// it falls within a pixel: its roughness widens by how far its normal turns across each pixel, so
/// its highlight, unwidened far narrower than a pixel, spreads over pixels rather than lighting
/// one only where its center hits the highlight.
fn a_small_smooth_balls_highlight_holds_as_it_moves_within_a_pixel() {
    const SIZE: u32 = 32;
    let mut renderer = renderer(SIZE, SIZE);
    let mut stores = Stores::default();
    // 16 pixels a unit; the view's center between four pixels.
    let viewpoint = Viewpoint {
        position: DVec3::new(0.0, 0.0, 5.0),
        projection: Projection::Orthographic {
            height: 2.0,
            near: 0.5,
            far: 10.0,
        },
        ..Viewpoint::default()
    };
    let ball = sphere(&mut stores.meshes, 0.25, 24, 48);
    let material = stores.materials.add(&Material {
        color: Vec4::ONE,
        workflow: metallic(1.0, 0.0),
        ..Material::default()
    });
    let draw = MeshDraw::with_material(ball, material, &stores.materials, &mut stores.shaders);
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::NEG_Z,
        color: Vec3::splat(0.05),
        range: None,
        shadow_distance: None,
    }];
    let viewport = filling([SIZE, SIZE]);
    // The red light the ball reflects, summed over the view, with its center `offset` pixels right
    // of and below the view's: a pixel's center at 0.5.
    let mut reflected = |offset: f32| {
        let mut draws = Draws::default();
        let at = Vec3::new(offset, -offset, 0.0) / 16.0;
        draws.push_draw(Mat4::from_translation(at), &draw, 1);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let data = RenderData {
            show: Some(Show::Shading(Channel::SpecularLight)),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        let pixels = frame.as_chunks::<4>().0.iter();
        pixels.map(|pixel| decoded(pixel[0])).sum::<f32>()
    };
    let sums = [0.0, 0.25, 0.5, 0.75].map(&mut reflected);
    let mean = sums.iter().sum::<f32>() / 4.0;
    assert!(mean > 0.1, "the ball reflects {sums:?}");
    for sum in sums {
        assert!(
            (sum / mean - 1.0).abs() < 0.1,
            "the ball reflects {sums:?} as it moves by quarters of a pixel"
        );
    }
    shut_down(renderer);
}

/// A masked texture of scattered leaves covers about the same share of its square at 8 pixels
/// across as at 64, its texels': its levels' alpha is scaled to keep the share at the cutoff,
/// where averaged alpha would fall under it and draw almost nothing.
fn a_masked_textures_coverage_holds_as_it_shrinks() {
    const SIZE: u32 = 64;
    let mut renderer = renderer(SIZE, SIZE);
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
    // Three texels in ten opaque, scattered by a hash (lowbias32).
    let hash = |mut x: u32| {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    };
    let leaves: Vec<u8> = (0..SIZE * SIZE)
        .flat_map(|i| [255, 255, 255, if hash(i) % 10 < 3 { 255 } else { 0 }])
        .collect();
    let mipmaps = Mipmaps::Coverage { cutoff: 128 };
    let leaves = stores
        .textures
        .add("leaves", [SIZE, SIZE], &leaves, mipmaps);
    let material = stores.materials.add(&Material {
        color_texture: Sampled::of(leaves.expect("a texture")),
        alpha: AlphaMode::Mask { cutoff: 0.5 },
        unlit: true,
        ..Material::default()
    });
    let square = model_square(&mut stores.meshes);
    let draw = MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
    let viewport = filling([SIZE, SIZE]);
    // The share of the square's pixels drawn, `across` pixels a side, on pixels' edges.
    let mut covered = |across: u32| {
        let mut draws = Draws::default();
        let scale = Vec3::splat(across as f32 / SIZE as f32);
        draws.push_draw(Mat4::from_scale(scale), &draw, 1);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        let frame = frame_pixels(&mut renderer);
        let drawn = frame
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] > 128);
        drawn.count() as f32 / (across * across) as f32
    };
    let [large, small] = [SIZE, 8].map(&mut covered);
    assert!(large > 0.25, "the leaves cover {large} at {SIZE} pixels");
    assert!(
        (small - large).abs() < 0.1,
        "the leaves cover {large} at {SIZE} pixels and {small} at 8"
    );
    shut_down(renderer);
}

/// A bumpy plane's highlight, lit and seen head on, keeps its brightness at 16 pixels across, its
/// normal map's fifth level, as at 256, its first: the surface map's levels are widened by the
/// normals' spread under each texel ([`gg_renderer::widen_by_normals`]). Unwidened, the averaged
/// normals are flat and the small plane's highlight several times brighter. Seen orthographically,
/// only the levels differ.
fn a_bumpy_planes_highlight_holds_as_it_shrinks() {
    const SIZE: u32 = 256;
    let mut renderer = renderer(SIZE, SIZE);
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
    // Slopes across and down of about 0.15, each the sum of three uniform hashes (lowbias32).
    let hash = |mut x: u32| {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    };
    let slope = |i: u32| {
        let uniform = |n: u32| (hash(3 * i + n) as f32 / u32::MAX as f32) - 0.5;
        let slope = 0.3 * (uniform(0) + uniform(1) + uniform(2));
        ((slope + 1.0) / 2.0 * 255.0).round() as u8
    };
    let normals: Vec<u8> = (0..SIZE * SIZE)
        .flat_map(|i| [slope(2 * i), slope(2 * i + 1), 255, 255])
        .collect();
    // Roughness 0.3, not metallic.
    let mut surface = [255, 77, 0, 255].repeat((SIZE * SIZE) as usize);
    gg_renderer::add_map_mipmaps(&mut surface, [SIZE, SIZE]);
    let paired = gg_renderer::widen_by_normals(&mut surface, [SIZE; 2], &normals, [SIZE; 2], 1.0);
    assert!(paired);
    let textures = &mut stores.textures;
    let normals = textures.add("bumps", [SIZE; 2], &normals, Texels::Map(Map::Normal));
    let surface = textures.add(
        "rough",
        [SIZE; 2],
        &surface,
        Texels::MapLevels(Map::Surface),
    );
    let material = stores.materials.add(&Material {
        color: Vec4::ONE,
        workflow: metallic(1.0, 1.0),
        surface_texture: Sampled::of(surface.expect("a texture")),
        normal_texture: Some(Sampled::of(normals.expect("a texture"))),
        ..Material::default()
    });
    let square = model_square(&mut stores.meshes);
    let draw = MeshDraw::with_material(square, material, &stores.materials, &mut stores.shaders);
    stores.lights = vec![Light {
        kind: LightKind::Directional,
        position: Vec3::ZERO,
        direction: Vec3::NEG_Z,
        color: Vec3::ONE,
        range: None,
        shadow_distance: None,
    }];
    let viewport = filling([SIZE, SIZE]);
    // The mean red light the plane reflects, `across` pixels a side, on pixels' edges.
    let mut reflected = |across: u32| {
        let mut draws = Draws::default();
        let scale = Vec3::splat(across as f32 / SIZE as f32);
        draws.push_draw(Mat4::from_scale(scale), &draw, 1);
        settle(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let data = RenderData {
            show: Some(Show::Shading(Channel::SpecularLight)),
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
        let frame = frame_pixels(&mut renderer);
        let pixels = frame.as_chunks::<4>().0.iter();
        pixels.map(|pixel| decoded(pixel[0])).sum::<f32>() / (across * across) as f32
    };
    let [large, small] = [SIZE, 16].map(&mut reflected);
    assert!(large > 0.01, "the plane reflects {large} at {SIZE} pixels");
    assert!(
        (small / large - 1.0).abs() < 0.25,
        "the plane reflects {large} at {SIZE} pixels and {small} at 16"
    );
    shut_down(renderer);
}

/// KHR_materials_pbrSpecularGlossiness shades as its metallic-roughness equal: diffuse times 1 less
/// the specular's strongest channel, roughness 1 less glossiness, a map's sRGB specular and alpha
/// glossiness as factors.
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
    // Specular about 0.5 (sRGB) and glossiness 0.6, as a map holds them.
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

/// A sky's sun texel past 10⁷ lights a floor through the sky's light alone; with a light for the
/// sun, the sky's light leaves it out, the floor as under the sunless sky. Prefiltered again as the
/// light comes and goes.
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
    // The texel's 6,260 of 10⁶ each, over about 1.07 × 10⁻⁴ sr, light the floor about as much as
    // the sky's 0.1; each texel past 10⁷ the sky's light keeps to it, some 10⁻⁴ of that.
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

/// The share of light `fog` lets through from `distance` along `way` (`None`: infinitely far), by
/// 100,000 midpoints in doubles: an unbounded way up ends at e^-40 of the start's density, one down
/// never. At least what its opacity leaves.
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

/// Henyey-Greenstein's phase function of `g` at cosine `cos`, per steradian.
fn henyey_greenstein(g: f32, cos: f32) -> f32 {
    let spread = 1.0 + g * g - 2.0 * g * cos;
    (1.0 - g * g) / (4.0 * PI * spread * math::libm::sqrtf(spread))
}

/// A rectangle of one linear color, blended by alpha, facing along `axis` at `at`, from `least` to
/// `most` on the other two axes in order.
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

/// Asserts each of `pixels` shows the `patches` its way meets over the clear color, fogged by
/// [`fog_through`] and lit by the fog's luminance and `sun`'s Henyey-Greenstein lobe, within 2 a
/// channel. `sun`: illuminance and the way toward it.
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
        // From the camera, or orthographic, from its plane.
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

/// A frame of `draws` and `canvas` in `fog`: its pixels.
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

/// Height fog, lit by its luminance and a sun's lobe, dims and lights each pixel as a numeric
/// integral says: quads before and past a black wall; fog starting past it; looking up and down;
/// orthographic; nothing opaque, the canvas unfogged; antialiased; and from 100 units above, at
/// e^-100 of the floor's density.
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
    // Every texel inside a glyph's edges, so a glyph fills its quad.
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
    // The circle half transparent, so what's behind it shows how many times it's fogged.
    let colors = [
        Vec4::new(0.9, 0.2, 0.1, 1.0),
        Vec4::new(0.2, 0.8, 0.3, 0.5),
        Vec4::new(0.3, 0.4, 0.9, 1.0),
        Vec4::new(0.9, 0.8, 0.2, 1.0),
    ];
    let patches: Vec<Patch> = (quads.iter().zip(colors))
        .map(|(&at, color)| Patch::facing(at, SIDE, color))
        .collect();
    // `models`, the quads and, if `particle`, a particle born alike each frame, so it draws after a
    // frame without it.
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
    // Antialiased, at pixels away from edges.
    let flat = [[8, 10], [8, 40], [56, 5], [56, 40]];
    for _ in 0..16 {
        let (renderer, stores) = (&mut renderer, &mut stores);
        fogged_frame(renderer, stores, SIZE, looking, &draws, &none, sunlit, taa);
    }
    let frame = frame_pixels(&mut renderer);
    assert_fogged(&frame, SIZE, &looking, &sunlit, sunlight, &walled, &flat);
    // Orthographic, looking a little up out of the fog; no particle, which faces the view.
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
    // Nothing opaque, so the fog's pass fogs the clear color alone; the canvas is unfogged.
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

/// Every kind of draw fog fogs, under a sky and a shadowing sun. Its particles are born in
/// [`FoggedScene::settle`]'s last frame, so later frames at the same time draw alike.
struct FoggedScene {
    renderer: Renderer,
    stores: Stores,
    viewport: Viewport,
    viewpoint: Viewpoint,
    /// Each with its transform and ID, the glass last.
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

    /// Its models, the glass last, then the rest; the particles' births if `born`.
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

    /// Its pixels in `fog`, its particles born in an earlier frame.
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

/// Fog of no density or opacity draws [`FoggedScene`] as no fog, though no opacity runs the fog's
/// pass; fog that takes light changes it. Likewise with ambient occlusion, the sky fogged alike, 8
/// frames apart as a still view cycles 8 frames' noise.
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
    // The frame `apart` frames from now, in `fog`, those before it without.
    let later = |scene: &mut FoggedScene, apart: usize, fog| {
        for _ in 1..apart {
            scene.render(None);
        }
        scene.render(fog)
    };
    let mut fogged = Vec::new();
    for occlusion in [None, Some(AmbientOcclusion::default())] {
        scene.stores.ambient_occlusion = occlusion;
        let apart = if occlusion.is_some() {
            for _ in 0..128 {
                scene.render(None);
            }
            8
        } else {
            1
        };
        let without = scene.render(None);
        for (name, fog) in [("no density", thin), ("no opacity", clear)] {
            assert!(
                later(&mut scene, apart, Some(fog)) == without,
                "fog of {name} changes what's drawn, with ambient occlusion {occlusion:?}"
            );
        }
        let frame = later(&mut scene, apart, Some(foggy));
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

/// [`FoggedScene`] in a low height fog against its reference image.
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

/// Earth's atmosphere at noon and sunset against their references, from 2 m up under 120,000 lux
/// outside the air: a blue noon sky, paler low; an orange sunset glow, red dim light, bluer ambient
/// light. Ambient occlusion darkens it, as an image sky's.
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
        // With ambient occlusion.
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

/// The paper's optical depth from `height` m up along zenith cosine `mu`, of Rayleigh, Mie and
/// ozone extinction in millionths per meter, by 20,000 midpoints.
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

/// The sun's disk from 1 km up, 0°, 60° and 85° from the zenith, of luminance 1 outside the air,
/// shows the paper's transmittance, which the GPU's table holds.
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

/// Along a level way from 2 m up, `far` m through the paper's atmosphere, 20,000 midpoints in
/// doubles: the mean transmittance; single scattering from a zenith sun of 1; and the ground's, of
/// its albedo, from half of every way round, whatever the phase.
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
            // From below, half of every way round, whatever the phase function.
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

/// Walls 5 and 20 km off from 2 m up: an emissive one under a set sun shows the paper's mean
/// transmittance within 8%; a black one under a zenith sun shows bluer, brighter in-scattering
/// farther, above single scattering but within a tenth past it plus the ground's, as the paper's
/// multiple scattering holds it.
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
        // A black metal reflects nothing either way.
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

/// A floor lit by the sky alone as the sun sinks 0.5° to 4.5° below the horizon, 0.05° a frame
/// (`atmosphere.ron`'s at 60 fps): each frame's fall within 6% of the last's, allowing for 8-bit
/// steps and the multiple scattering table's texels. Prefiltering too seldom stepped tens of
/// percent.
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

/// An image sky draws byte for byte as before an atmosphere took its place and left: its light
/// prefiltered again, the sun undimmed.
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

/// Places [`model_square`] facing up, `size` across, centered at `at`.
fn facing_up(size: f32, at: Vec3) -> Mat4 {
    let turned = Quat::from_rotation_x(-FRAC_PI_2);
    Mat4::from_scale_rotation_translation(Vec3::new(size, size, 1.0), turned, at)
}

/// A white 1-lux directional light along `direction`, shadowing `shadow_distance` ahead, if given.
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

/// Orthographic, down from `at` onto a view `across` units square, its top toward -Z, seeing 1 to
/// `far` below.
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

/// A white floor and the first `plates` of three unit plates 2 above it, facing up: opaque, masked
/// opaque, and masked transparent, which draws nothing.
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

/// [`floor_and_plates`] under a light down 2 for each 1 along +X: the opaque plates shadow the
/// floor a unit along +X, the transparent one none. Shadow maps take memory only when cast;
/// synchronization validation sees each drawn before and after the view samples it.
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
    // The floor's pixel at `x`, `z` from the origin, the view's center.
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

/// Plates lit as in [`shadows_fall_where_their_casters_put_them`]: one in view; one past the side,
/// kept by the cascades for its shadow; one 200 units off, culled by both; one across the other
/// side. A square whose sphere is 10 units off its origin draws. GPU counts leave out the culled.
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
    // The floor, the plates in view and across its side (one run), and the square.
    let kept = DrawCounts {
        draws: 3,
        instances: 4,
        triangles: 8,
    };
    assert_eq!(drawn, kept);
    let frame = frame_pixels(&mut renderer);
    shut_down(renderer);
    // The floor's pixel at `x`, `z` from the origin, the view's center.
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

/// [`culling_keeps_what_each_view_sees`]'s draws held by the renderer's scene, over two frames
/// that move, remove and add a plate, draw, cull and count bit for bit as pushed.
fn the_scene_draws_as_draws_do() {
    const SIZE: u32 = 128;
    // Each frame's instances: slot (its ID less 1), x and z; the floor's slot is 9.
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

/// A never-culled wall hides a plate behind it from the depth pyramid; plates beside, half behind
/// and in front draw. The frame the wall goes draws it as a renderer that never saw the wall; once
/// back, it's hidden and uncounted again.
fn occlusion_culling_draws_what_shows() {
    occlusion_culled(false);
}

/// As [`occlusion_culling_draws_what_shows`] with ambient occlusion: two depth-only passes, then a
/// shaded draw without a visibility buffer.
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
            // Behind the wall; beside it; half behind its edge (the camera's line past x 0.5 at
            // z = 0 meets z = -3 at x 0.8); and in front.
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
    // The first frame finds nothing shown before, so its second pass draws all; the next frame's
    // second pass finds red hidden; the one after draws it in neither.
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
        // The sky shows a frame after its texture is drawable.
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

/// Blending squares pushed before opaque ones: opaque draws go first, so one in front blends over
/// its square and one behind is hidden.
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

/// [`floor_and_plates`]'s plate from above, its cascade's texels 2 pixels across: moved a pixel
/// along X, the view shows every pixel a pixel over, shadow edges too, as the texels stay put in
/// the world.
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

/// A floor under lights 0°, 40°, 70° and 80° from straight down draws as without shadows, in and
/// past every cascade: no shadow acne.
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
        // Turned a little, so the light's texels don't line up with the floor's edges.
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

/// [`model_square`] with vertex alpha from opaque at its left edge to transparent at its right:
/// masked, it draws its left half.
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

/// Cuts `mesh`'s levels into meshlets as the cooker does, more simply, each with the sphere round
/// its vertices' box and a cone as `meshopt_computeClusterBounds` makes it, widened for rounding.
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

/// Meshlets of two-level spheres, opaque and masked, mirrored, stretched (which their cones don't
/// turn with) and partly outside, through mesh or vertex shaders: as by indices, each channel
/// within 1, fewer triangles counted, and as their reference, via the visibility buffer too.
/// Lavapipe's mesh shaders round a few pixels otherwise, where the normal's change widens roughness.
fn meshlets_draw_as_their_indices_do(mesh_shaders: bool) {
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 128;
    let render = |meshlets: bool, visibility_buffer: bool| {
        let settings = Settings {
            mesh_shaders: Some(mesh_shaders),
            visibility_buffer,
            ..settings(None)
        };
        let mut renderer = Renderer::headless_on(&harness::instance(), &settings, WIDTH, HEIGHT)
            .expect("a renderer");
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
        // Past the frame the mesh became drawable in, which drew it by its indices, with no room
        // planned for its meshlets.
        render_without_ui(&mut renderer, stores.data(viewport, viewpoint, &draws));
        let counts = drawn_again(&mut renderer, &mut stores, viewport, viewpoint, &draws);
        let pixels = frame_pixels(&mut renderer);
        shut_down(renderer);
        (counts, pixels)
    };
    let (by_indices, indexed) = render(false, false);
    let (by_meshlets, cut) = render(true, false);
    for at in (0..HEIGHT).flat_map(|y| (0..WIDTH).map(move |x| [x, y])) {
        let (got, was) = (pixel_at(&cut, WIDTH, at), pixel_at(&indexed, WIDTH, at));
        assert!(
            close(got, was),
            "meshlets draw {at:?} as {got:?}, indices as {was:?}"
        );
    }
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

/// Shadowed models against their reference: a sphere, a post and a plate masked to its left half,
/// shadowed toward the view, a dim unshadowed front light showing the shade; the sphere shades its
/// own far side.
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
    // A post 0.4 across and 1.6 tall, of four squares facing out.
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
    // The dim front light, then the shadowing one.
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

/// The antialiasing tests' light, from the upper left, in front.
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

/// The antialiasing tests' viewpoint, `moved` from 4 in front of the origin, a quarter turn
/// across: about 19 pixels a unit there at 64 a side.
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

/// The mean linear-light difference over every channel of frames `a` and `b`.
fn linear_distance(a: &[u8], b: &[u8]) -> f32 {
    assert_eq!(a.len(), b.len(), "frames of a size");
    let sum: f32 = (a.iter().zip(b))
        .map(|(&a, &b)| (decoded(a) - decoded(b)).abs())
        .sum();
    sum / a.len() as f32
}

/// `pixels`, `size` a side, averaged down `by` each way in linear light, as that many samples a
/// pixel show it.
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

/// `frames`, each `size` pixels a side, in a row, as a reference holds them.
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

/// `frames` frames, [`EDGES_SIZE`] times `scale` a side: a lit sphere and a turned square, edges at
/// every angle, under a canvas square 12 pixels a side, 8 from the bottom left at scale 1.
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

/// [`edges_seen`] by TAA against its reference of frames 1, 2, 4 and 16: by the 16th, nearer the
/// 4-times view averaged down than the unaliased view; the canvas unaliased.
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

/// What the antialiasing tests move.
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
    /// Its draw, added to `stores`, and shape: a sphere, or a square 1.3 across turned so its edges
    /// cross the pixels aslant.
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

/// A scene instance moving with the camera resolves as though both stood still: within 0.0015 of
/// the still view on average, where taken as still in the world it's 0.004 off. For each [`Moved`].
fn antialiasing_follows_what_moves() {
    const SIZE: u32 = 64;
    const FRAMES: u32 = 20;
    let step = Vec3::new(0.21, 0.13, -0.05);
    let render = |moved: Moved, moving: bool| {
        let mut renderer = renderer(SIZE, SIZE);
        let mut stores = Stores::default();
        let (draw, shape) = moved.draw(&mut stores);
        stores.lights = vec![front_light()];
        // The frame's camera and instance, from their start; still ones where the moving ones end.
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

/// A scene 1,000 km out, drawn from a near origin with fog, ambient occlusion, TAA, motion blur and
/// particles, the view and a sphere moving, draws each frame as at the world's origin within
/// rounding, through the origin's move halfway.
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
    // Each frame's pixels of the scene at `far`, drawn from `origins[0]` until `MOVED_AT`, then
    // `origins[1]`.
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
        // A uniform sky, whose light ambient occlusion darkens.
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
        // The scene, and a wall of pushed stripes behind, which the camera's motion alone blurs.
        let stripes = |frame: u32| {
            let mut draws = Draws::default();
            draws.draw_scene();
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
            draws
        };
        let draws = |frame: u32| {
            let mut draws = stripes(frame);
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
            if frame == 0 {
                // Frames until the meshes are drawable, a count that differs between renders, so
                // they keep nothing for the next frame: the frames compared follow from none.
                let settling = stripes(0);
                let settled = (0..200).any(|_| {
                    let data = RenderData {
                        origin: origin(0),
                        ambient_occlusion: None,
                        ..stores.data(viewport, viewpoint(0), &settling)
                    };
                    render_without_ui(&mut renderer, data);
                    renderer.streaming().is_empty()
                });
                assert!(settled, "still streaming after 200 frames");
            }
            let draws = draws(frame);
            let data = RenderData {
                antialiasing: Antialiasing::Taa,
                motion_blur: Some(MotionBlur { shutter: 0.5 }),
                fog: Some(fog),
                origin: origin(frame),
                ..stores.data(viewport, viewpoint(frame), &draws)
            };
            render_without_ui(&mut renderer, data);
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
    // Within the rounding of points 1,500 or so from their origin: 0.0005 apart on average, and a
    // few pixels. Leaving one kept state as it was puts the move's frame 0.0038 apart (motion),
    // 0.015 (TAA's history), 0.02 (motion blur's) or 26 pixels (particles).
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

/// A plate crossing a still view, antialiased, against its reference: no trail 2 pixels from it,
/// and no pixel outside the clear color's and the plate's range, as a filter's undershoot is.
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
        // No lights, so it's one color wherever it is.
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
    // Where the plate is in the last frame, and was in the fourth, unaliased.
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
    // Each channel's least and most of the plate's color, well inside it, and the clear color.
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

/// The antialiasing tests' backdrop: unlit 0.8 in linear light behind the origin, filling the view,
/// drawn by `flat`, `flat_color.slang`.
fn bright_plate(draws: &mut Draws, square: Mesh<[f32; 3]>, flat: Shader<[f32; 3], Vec4>) {
    let behind =
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.5)) * Mat4::from_scale(Vec3::splat(4.0));
    draws.push(square, flat, behind, Vec4::new(0.8, 0.8, 0.8, 1.0));
}

/// [`bright_plate`], sRGB-encoded, as a frame reads back.
const PLATE: [u8; 4] = [231, 231, 231, 255];

/// Sub-pixel detail from [`seeing_from`], added to `stores`: dark bars a third of a pixel wide,
/// aslant across [`bright_plate`], which each of the 8 offsets draws over pixels the others don't.
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

/// [`thin_bars`] still, antialiased: settled, no channel moves more than 2 levels over 16 frames.
/// Clipping to the pixels round it, which hold the bars at some offsets only, moved 24.
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
    // Each channel's range over the frames, and where it's widest.
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

/// [`thin_bars`] as the view pans and stops: each pixel settles on the offsets' mean gradually,
/// under 1% of pixels landing at once from 12 levels off to within 2 (the clip's flicker lands 11
/// by chance). Settling at once jumped 350.
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

/// Still dark squares, a thousandth to a fiftieth in linear light, where an sRGB level spans the
/// least light: antialiased, their insides show as unaliased within a level. Keeping the offsets'
/// draws in thousandths was off by 2.
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

/// A dark speck crossing a bright plate before a still camera at 0.25, 1 and 3 pixels a frame,
/// antialiased, leaves no trail 3 pixels past its edge, drawn with its motion (`moving.slang`) and
/// without (`flat_color.slang`, as a shadow crossing what's still).
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
            // The speck's middle in the world, in a frame of its crossing from 0.
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
            for (i, frame) in (0..STILL).map(|_| 0).chain(0..=moves).enumerate() {
                let draws = drawing(&mut stores, frame);
                let data = RenderData {
                    antialiasing: Antialiasing::Taa,
                    ..stores.data(viewport, viewpoint, &draws)
                };
                render_without_ui(&mut renderer, data);
                if i < STILL {
                    continue;
                }
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

/// Pixels a unit at the origin from [`seeing_from`], in a `size` view.
fn pixels_a_unit(size: u32) -> f32 {
    size as f32 / (2.0 * 4.0 * math::libm::tanf(PI / 8.0))
}

/// A view panning `across` and `down` whole pixels every `over` frames at the origin's depth, so
/// each pixel's point is at a pixel again then.
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

/// [`slats_seen`]'s slats, as a slatted table's sides from 9 m: height, spacing and rise per pixel,
/// so one sample a pixel shows dashes 5 long every 14; their light; the plate's.
const SLAT_TALL: f32 = 0.36;
const SLATS_APART: f32 = 2.5;
const SLAT_RISE: f32 = 1.0 / 14.0;
const SLAT_LIGHT: f32 = 0.05;
const SLATS_PLATE: f32 = 0.8;

/// A dark speck crossing [`slats_seen`]'s view at `speed` pixels a frame, from 8 in and 2 above the
/// middle, drawn with its motion or without.
#[derive(Clone, Copy, Debug)]
struct Speck {
    speed: f32,
    returns_motion: bool,
}

/// Half a [`Speck`]'s side, in units, 3.9 from the camera: about 3 pixels across at 64 a side.
const SPECK_HALF: f32 = 0.08;

impl Speck {
    /// Where its middle shows, in pixels, in the `kept`th kept frame of a `size` view.
    fn seen(self, kept: usize, size: u32) -> [f32; 2] {
        [8.0 + self.speed * kept as f32, size as f32 / 2.0 - 2.0]
    }
}

/// Frames `frames` of a `size` view, antialiased, panning by `pan` past dark sub-pixel slats right
/// before a bright plate, `speck` crossing them.
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
        // Right behind the slats, so the two move alike.
        let behind = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.01))
            * Mat4::from_scale(Vec3::splat(800.0));
        draws.push(square, flat, behind, Vec3::splat(SLATS_PLATE).extend(1.0));
        // The slats across the view, each fixed in the world.
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

/// Frames a point must have shown for the panning tests to measure it, as one come into view since
/// has a history as young.
const AGED: usize = 48;

/// Whether (`x`, `y`) of a view panning by `pan` has shown for [`AGED`] frames, 2 or more pixels
/// inside a `size` view.
fn aged(pan: Pan, size: u32, [x, y]: [i32; 2]) -> bool {
    let old = |at: i32, moves: i32| at * pan.over as i32 >= AGED as i32 * moves;
    let inside = 2..size as i32 - 2;
    inside.contains(&x) && inside.contains(&y) && old(x, pan.across) && old(y, pan.down)
}

/// How points change as the view pans, over `samples` of `frames` `pan.over` apart, each [`aged`]
/// pixel of the first against where its point shows later: the points, those whose most-changing
/// channel ranges over 8 levels, and the mean range.
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

/// [`slats_seen`] panning by two [`Pan`]s: [`aged`] points hold, a mean range under 5 levels and
/// under 1 in 20 over 8, the frame at least the slats' cover darker than the plate (1.5 times
/// still, as the resolve weighs by luminance). Clipping to neighbors alone gave mean ranges of 8.4
/// and 12, 46% and 63% over 8, and 0.19 and 0.36 of that darkness.
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

/// A [`Speck`] crossing [`slats_seen`]'s flickering slats leaves no trail: under 10 pixels a frame,
/// 3 past its edge, over 2 levels darker, as a change ends the flicker evidence (keeping it left
/// 100). With its motion, its middle is at most 90; without, 102 and 193.
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

/// A 4-pixel square blinking every 5 frames over a dark plate in a still view leaves no ghost: each
/// last frame off shows the plate within a level. Its edge looks like flicker, but stood still and
/// changed at an offset, which offsets alone don't; taking it for flicker kept 135.
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

/// [`edges_seen`] by SMAA against its reference beside the unaliased view: as near the averaged
/// 4-times view as TAA's 16th frame, alike each frame, the canvas unaliased.
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

/// A unit disk of `segments` triangles in the XY plane facing +Z, of positions alone, for
/// `flat_color.slang`.
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

/// A disk by SMAA against the view 8 times larger averaged down: edge pixels show its light times
/// their coverage within bounds, per eighth and each pixel (unaliased: 0.2 off), alike every way.
/// Disks of 0.25; 0.02, which only a display-encoded luma finds; 1, which PBR Neutral's shoulder
/// would darken; 16 under PBR Neutral, which linear blending would brighten; and 2 under PBR
/// Neutral graded by 0.5 after SMAA.
fn smaa_blends_by_how_much_of_each_pixel_the_edge_covers() {
    const SIZE: u32 = 96;
    const LARGER: u32 = 8;
    // Each frame's green in linear light, a frame per `antialiasing` from one renderer, as SMAA
    // keeps nothing between frames.
    let render = |light: f32, tonemapper, slope: f32, scale: u32, antialiasing: &[Antialiasing]| {
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
        let frames: Vec<Vec<f32>> = (antialiasing.iter())
            .map(|&antialiasing| {
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
                let green = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| decoded(pixel[1]));
                green.collect()
            })
            .collect();
        shut_down(renderer);
        frames
    };
    // SMAA's and the unaliased frame's mean distance from coverage, per eighth from the right and
    // overall, over pixels 2% to 98% covered in the larger view; and SMAA's worst pixel.
    let distances = |light: f32, tonemapper, slope: f32| {
        let modes = [Antialiasing::Smaa, Antialiasing::None];
        let [smaa, aliased] =
            <[Vec<f32>; 2]>::try_from(render(light, tonemapper, slope, 1, &modes))
                .expect("a frame each");
        let [larger] = <[Vec<f32>; 1]>::try_from(render(
            light,
            tonemapper,
            slope,
            LARGER,
            &[Antialiasing::None],
        ))
        .expect("a frame");
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
    // SMAA's bounds: more for the dim disk, which 8 bits hold coarsely, and the bright ones under
    // PBR Neutral, whose toe darkens dim partly covered pixels.
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
        // The disk is symmetric: its edges across and down blend alike, as do both diagonals.
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

/// A gray square turned 40°, by SMAA, against the view 8 times larger averaged down: its stepped
/// diagonal edges, which SMAA's diagonal patterns take, show their coverage within 0.08 on average
/// (0.11 without those patterns), and within 0.3 away from the corners.
fn smaa_blends_along_diagonals_by_how_much_they_cover() {
    const SIZE: u32 = 96;
    const LARGER: u32 = 8;
    const TURNED: f32 = 40.0;
    const SIDE: f32 = 1.6;
    let gray = 0.25;
    // The square at `scale` times the size: its pixels' green in linear light.
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
    // The corners' distance from the middle, in pixels: half the view is 4 · tan(π/8) units at 4
    // away.
    let half = SIZE as f32 / 2.0;
    let corner = SIDE / SQRT_2 * half / (4.0 * math::libm::tanf(PI / 8.0));
    // Each 2% to 98% covered pixel's distance from its coverage, on the falling edges (outward
    // normals 40° and 220°) and the rising ones.
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

/// A turned square of red 65,504 over white as bright, graded so only a half float's two brightest
/// values show as 1: under SMAA all red still shows as 1, as blends reach the brightest within a
/// write's rounding; its green edges blend.
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

/// What the view drew of `draws`, which comes back as the frame two later begins: renders them
/// twice more, as they were.
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

/// A frame that asks reads its offset view back as it shows; one that doesn't reads nothing. A
/// frame into an offscreen image of another size fills it with the view.
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

/// Pixels show in the view as far as they reach, the rest clear: new, again, larger than the view,
/// and again at a size no frame wrote, which shows nothing; draws don't.
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

/// A shader rewritten from red to green replaces its pipeline while earlier frames may still use
/// the old: the next frame draws green, and validation sees the old destroyed only once the GPU is
/// done.
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

/// A frame fills a pipeline cache; a renderer started from it renders alike, validation silent.
/// lavapipe's cache is a stub that holds nothing (Mesa's lvp_pipeline_cache.c), so there it isn't
/// seen to fill.
fn pipelines_go_through_the_cache() {
    const SIZE: u32 = 16;
    let cached = |data: &[u8]| {
        let settings = Settings {
            pipeline_cache: Some(data),
            ..settings(None)
        };
        Renderer::headless(&settings, SIZE, SIZE).expect("a renderer")
    };
    // A triangle, which takes a pipeline of its own.
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

/// A renderer asked to capture doesn't start without RenderDoc's layer, which the tests turn off
/// with every implicit layer.
fn capturing_needs_renderdoc() {
    let settings = Settings {
        capture: NonZeroU64::new(1),
        ..settings(None)
    };
    let error = (Renderer::headless(&settings, 16, 16).err())
        .expect("a renderer that can't capture doesn't start");
    assert_eq!(error.code(), "renderer.no_renderdoc", "{error:#}");
}

/// Compares `renderer`'s last frame with reference image `name`, shutting it down as [`shut_down`]
/// does.
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
    harness::check(&validation);
}

/// Compares RGBA8 `pixels` with reference image `name`, allowing for GPUs: a pixel matches within
/// 16 a channel, and 1 in 100 may not. A mismatch writes `<name>.actual.png` in
/// `CARGO_TARGET_TMPDIR`.
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
