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
use gg_core::math::{self, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{
    AlphaMode, Blend, Circle, DEFAULT_UPLOAD_BUDGET, DrawCounts, Draws, Glyph, Grid, GridLines,
    Light, LightKind, Line, Lod, Lods, MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Map, Material,
    Materials, Mesh, MeshDraw, Meshes, Meshlet, Meshlets, Mipmaps, ModelMesh, ModelVertex, Outline,
    PackedVertex, Pick, Pixels, Projection, RenderData, Renderer, Sampled, Sampler, Scene,
    Settings, Shaders, Sky, Sprite, SpriteSheet, Texels, Texture, Textures, Tonemapper, Ui,
    UploadPath, Uploads, ViewCallback, Viewpoint, Viewport, Workflow, Wrap,
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
        shaders_read_the_frames_data,
        lights_shade_as_gltf_defines,
        lit_spheres_match_their_reference,
        many_lights_match_their_reference,
        a_uniform_sky_lights_white_as_itself,
        the_sky_shows_each_way_behind_the_world,
        sky_lit_spheres_match_their_reference,
        specular_glossiness_shades_as_its_metallic_roughness_equal,
        a_skys_sun_lights_once,
        shadows_fall_where_their_casters_put_them,
        culling_keeps_what_each_view_sees,
        the_scene_draws_as_draws_do,
        occlusion_culling_draws_what_shows,
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
        mixed_textures_match_their_reference,
        picks_answer_what_their_frame_drew,
        picks_wait_through_frames_that_read_none,
        removed_assets_draw_nothing_or_their_stand_in,
    );
    // The tests of models' shading again, their models cut into meshlets, which the view draws
    // into its visibility buffer and shades from it, against the references they made drawn by
    // their indices and shaded as they draw.
    CUT_MESHLETS.store(true, Ordering::Relaxed);
    run!(
        tests by meshlets,
        lights_shade_as_gltf_defines,
        lit_spheres_match_their_reference,
        many_lights_match_their_reference,
        a_uniform_sky_lights_white_as_itself,
        sky_lit_spheres_match_their_reference,
        specular_glossiness_shades_as_its_metallic_roughness_equal,
        a_skys_sun_lights_once,
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
/// its renderer; the lights and the sky its frames shade with; and no draws, for a frame's canvas.
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
    none: Draws,
}

impl Stores {
    /// A frame of `draws` into `viewport`, as `viewpoint` sees them, cleared to [`CLEAR`], that
    /// draws nothing on its canvas, picks and outlines nothing and isn't tonemapped, at time 0
    /// with an exposure of 1 and the stores' lights and sky, with the uploads of what's been added,
    /// and what's changed in the scene, since the last frame's.
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
            none,
        } = self;
        uploads.take(meshes, shaders, textures, materials);
        uploads.take_scene(scene);
        RenderData {
            clear_color: CLEAR,
            tonemapper: Tonemapper::None,
            exposure: 1.0,
            time: 0.0,
            lights,
            sky: *sky,
            viewport,
            viewpoint,
            uploads,
            draws,
            canvas: none,
            picks: &[],
            read_picks: true,
            outline: None,
            read_view: false,
            pixels: None,
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
    let mut packed = ModelMesh::pack(vertices, indices.to_vec(), Vec::new());
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
        position: Vec3::new(1.0, 0.5, 0.0),
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
        position: Vec3::new(0.0, 0.0, 4.0),
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

/// Outlines what the view shows of one ID, 3 pixels wide, and compares it with its reference image:
/// a disc and a square of ID 7, which it goes round alike; the square's corner under a square of ID
/// 3, along whose edges it goes across the square, and its bottom past the view's, where it stops;
/// and beside them a square of no entity and one of ID 5, which it leaves alone. First, an outline
/// of ID 0, and outlines 0 and NaN pixels wide, change no pixel of a frame without one; and the
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
    let outline = |id, width| Outline {
        id,
        color: orange,
        width,
    };
    // Each pixel as a frame without an outline has it, once everything is drawable: an outline of
    // ID 0 would go round every draw's, inside its edges, and one wider than 0 round the disc and
    // the square.
    settle(
        &mut renderer,
        &mut stores,
        filling([WIDTH, HEIGHT]),
        viewpoint,
        &draws,
    );
    let mut plain = None;
    let nothing = [outline(0, 3.0), outline(7, 0.0), outline(7, f32::NAN)];
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
            outline: Some(outline(7, 3.0)),
            read_view: true,
            ..stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
    let read = view_read(&renderer);
    assert_eq!(read, Some(([WIDTH, HEIGHT], frame_pixels(&mut renderer))));
    check(renderer, "outlines", WIDTH, HEIGHT);
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
        position: Vec3::new(0.3, 2.0, 3.0),
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
        position: Vec3::new(5.0, 1.5, 0.0),
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
        position: Vec3::new(0.0, 0.0, 4.0),
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
        position: Vec3::new(1.0, 0.5, 0.0),
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
        position: Vec3::new(0.0, 0.0, 4.0),
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
        position: Vec3::new(0.0, 0.0, 4.0),
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
        id: 7,
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

/// A square that covers a view, with a shader that shows the frame's data as colors: where the
/// camera is, left of it, and the time and the exposure, right of it.
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
        position: Vec3::new(0.1, 0.2, 0.3),
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
/// 2.5, by the window KHR_lights_punctual recommends, `(1 - (2 / 2.5)^4)^2`; and a spot over it
/// lights it alike within its inner cone, and nothing outside its outer.
fn lights_shade_as_gltf_defines() {
    const SIDE: u32 = 16;
    const SQUARES: u32 = 10;
    const WIDTH: u32 = SIDE * SQUARES;
    let mut renderer = renderer(WIDTH, SIDE);
    let mut stores = Stores::default();
    let viewpoint = Viewpoint {
        position: Vec3::new(0.0, 0.0, 5.0),
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
    let window = (1.0 - reach2 * reach2) * (1.0 - reach2 * reach2);
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
        position: Vec3::new(0.0, 0.0, 9.0),
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
        position: Vec3::new(0.0, 7.0, 10.0),
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
        position: Vec3::new(0.0, 0.0, 5.0),
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
        position: Vec3::new(5.0, -3.0, 2.0),
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
        position: Vec3::new(0.0, 0.0, 9.0),
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
        position: Vec3::new(0.0, 0.0, 5.0),
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
        position: Vec3::new(0.0, 5.0, 0.0),
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
        position: at,
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
    const SIZE: u32 = 128;
    let viewport = filling([SIZE, SIZE]);
    // 5 in front of the origin, seeing a sixth of a turn across: 22 pixels a unit at z = 0, the
    // wall's, and 14 at z = -3, the plates'.
    let viewpoint = Viewpoint {
        position: Vec3::new(0.0, 0.0, 5.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.1,
        },
        ..Viewpoint::default()
    };
    // What a renderer draws with, and the frames with the wall and without.
    let made = || {
        let mut stores = Stores::default();
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
        position: Vec3::new(0.0, 2.0, 0.0),
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
        let mut ball = ModelMesh::pack(&vertices, indices, levels);
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
            position: Vec3::new(0.0, 3.0, 7.0),
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
        position: Vec3::new(0.0, 2.6, 4.2),
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
