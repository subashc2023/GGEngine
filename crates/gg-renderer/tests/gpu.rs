//! GPU tests of rendering, against reference images. They run on whatever GPU the loader offers,
//! which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another: those whose
//! names contain an argument that isn't a flag, as libtest picks them, or all of them without one.
//! `GG_UPDATE_REFERENCES=1` writes each test's output as its new reference instead of comparing.

// For setting the environment; see the SAFETY comment.
#![allow(unsafe_code)]

use std::f32::consts::{FRAC_PI_2, PI};
use std::fs::{self, File};
use std::io::BufWriter;
use std::num::NonZeroU64;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{self, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{
    AlphaMode, Blend, Circle, DrawCounts, Draws, Glyph, Grid, GridLines, Line, Material, Mesh,
    MeshDraw, Meshes, Mipmaps, ModelVertex, Outline, Pick, Pixels, Projection, RenderData,
    Renderer, Sampler, Settings, Shaders, Sprite, SpriteSheet, Texels, Texture, Textures, Ui,
    UploadPath, Uploads, ViewCallback, Viewpoint, Viewport, Wrap,
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
        models_match_their_reference,
        mixed_textures_match_their_reference,
        picks_answer_what_their_frame_drew,
        picks_wait_through_frames_that_read_none,
    );
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
            a_large_texture_draws_in_the_frame_that_adds_it,
            replaced_meshes_and_textures_draw_from_the_frame_that_takes_them,
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

/// How the tests' renderers are set up: under validation, uploading by `uploads`, if given, rather
/// than by the GPU's best way.
fn settings(uploads: Option<UploadPath>) -> Settings<'static> {
    Settings {
        app_name: "renderer gpu tests",
        validation: true,
        gpu: None,
        vsync: false,
        uploads,
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
    let renderer = Renderer::headless(&settings(uploads), width, height).expect("a renderer");
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

/// What a test adds meshes, shaders and textures to, and the uploads that take them to its
/// renderer; and no draws, for a frame's canvas.
#[derive(Default)]
struct Stores {
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    uploads: Uploads,
    none: Draws,
}

impl Stores {
    /// A frame of `draws` into `viewport`, as `viewpoint` sees them, cleared to [`CLEAR`], that
    /// draws nothing on its canvas and picks and outlines nothing, with the uploads of what's been
    /// added since the last frame's.
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
            uploads,
            none,
        } = self;
        uploads.take(meshes, shaders, textures);
        RenderData {
            clear_color: CLEAR,
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

/// A view that fills a frame of `size` pixels.
fn filling(size: [u32; 2]) -> Viewport {
    Viewport {
        origin: [0, 0],
        size,
    }
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
/// frames, which outnumber those in flight, so that the image shows a mesh drawn in the frame that
/// adds it beside those that earlier frames copied to the GPU. The view renders into an image of
/// its own, which the frame shows at the view's place, so that one shown elsewhere changes
/// thousands of pixels.
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
/// each other; the one with alpha is added in the last frame, which the image is read back from,
/// so that it shows a texture sampled in the frame that adds it. Its texels are opaque, half, a
/// quarter and wholly transparent, so that a shader blended as opaque, or as though its alpha were
/// multiplied in, changes thousands of pixels; and the textures are sRGB, so that colors sampled
/// without decoding come out brighter.
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
    // More frames than are in flight.
    for _ in 0..3 {
        let data = RenderData {
            canvas: &canvas,
            ..stores.data(viewport, viewpoint, &draws)
        };
        render_without_ui(&mut renderer, data);
    }
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
    // Each pixel as a frame without an outline has it: an outline of ID 0 would go round every
    // draw's, inside its edges, and one wider than 0 round the disc and the square.
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
    let drew = DrawCounts {
        draws: 3,
        instances: 6 + 1 + BIG,
        triangles: 2 * (6 + 1 + BIG),
    };
    assert_eq!(renderer.drawn(), drew, "each run is one instanced draw");
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
    let mut shown = None;
    for (frame, viewport) in views.into_iter().enumerate() {
        let data = stores.data(viewport, viewpoint, &draws);
        render_without_ui(&mut renderer, data);
        match frame {
            1 => {
                assert_eq!(renderer.drawn(), DrawCounts::default());
                let pixels = read_back(&mut renderer);
                // `CLEAR`, encoded.
                let clear = [39, 48, 63, 255];
                for pixel in pixels.as_chunks::<4>().0 {
                    let close = pixel.iter().zip(clear).all(|(&a, b)| a.abs_diff(b) <= 1);
                    assert!(close, "a shut view left {pixel:?}, not the clear color");
                }
            }
            3 => {
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
    check(renderer, "depth", WIDTH, HEIGHT);
}

/// Fills the view with a texture of 4,096 by 4,096 texels of one color, 64 MiB, in the frame that
/// adds it, and checks that every pixel shows that color: staged, it takes more than the least
/// staging buffer, 16 MiB, so that staging grows to hold it, and every texel the view samples must
/// have been copied, which a transfer queue takes milliseconds to do.
fn a_large_texture_draws_in_the_frame_that_adds_it(path: UploadPath) {
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
    let pixels = renderer
        .read_back()
        .expect("the frame's pixels")
        .expect("a headless renderer's frame can be read back");
    shut_down(renderer);
    for pixel in pixels.as_chunks::<4>().0 {
        let close = pixel
            .iter()
            .zip(COLOR)
            .all(|(&got, want)| got.abs_diff(want) <= 1);
        assert!(close, "read back {pixel:?}, expected {COLOR:?}");
    }
}

/// A textured square that covers a view of `SIZE` pixels a side, red, whose mesh and texture are
/// replaced while the frames before may still draw them, over more frames than are in flight: the
/// mesh by one that covers the view's left half, the texture by a green one of another size, with
/// mip levels. The frame that takes them draws them, and validation, synchronization's among it,
/// sees the old ones go only once the GPU is done with them.
fn replaced_meshes_and_textures_draw_from_the_frame_that_takes_them(path: UploadPath) {
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
        // Reading back waits for the GPU, so not between the first frame and the replacements.
        match frame {
            0 => assert_eq!(left_and_right(&mut renderer), [red, red], "red"),
            3 | 5 => {
                let [left, right] = left_and_right(&mut renderer);
                assert_eq!(left, green, "green on the left");
                assert!(![red, green].contains(&right), "the clear color, {right:?}");
            }
            _ => {}
        }
    }
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
        };
        let corners = [
            corner([-0.5, -0.5], [0.0, 1.0], colors[0]),
            corner([0.5, -0.5], [1.0, 1.0], colors[1]),
            corner([0.5, 0.5], [1.0, 0.0], colors[2]),
            corner([-0.5, 0.5], [0.0, 0.0], colors[3]),
        ];
        (stores.meshes)
            .add(name, &corners, &[0, 1, 2, 2, 3, 0])
            .expect("a mesh")
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
        texture: cutout,
        nearest: true,
        wrap: Wrap::Clamp,
        ..material([1.0; 4], AlphaMode::Mask { cutoff: 0.5 }, false)
    };
    let blended = material([0.9, 0.1, 0.8, 0.5], AlphaMode::Blend, false);
    let tinted = opaque([1.0, 0.5, 1.0, 1.0], false);
    let mut draw =
        |mesh, material: &Material| MeshDraw::with_material(mesh, material, &mut stores.shaders);
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
    // Red and green facing one way, blue and yellow mirrored, then one each.
    let drawn = DrawCounts {
        draws: 7,
        instances: 9,
        triangles: 18,
    };
    assert_eq!(renderer.drawn(), drawn);
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
    // A unit square, counterclockwise seen from +Z, its texture's top left at its top left.
    let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
        position: Vec3::new(x, y, 0.0),
        uv: Vec2::from(uv),
        color: Vec4::ONE,
    };
    let corners = [
        corner([-0.5, -0.5], [0.0, 1.0]),
        corner([0.5, -0.5], [1.0, 1.0]),
        corner([0.5, 0.5], [1.0, 0.0]),
        corner([-0.5, 0.5], [0.0, 0.0]),
    ];
    let square = (stores.meshes)
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let nearest = Material {
        texture: quadrants,
        nearest: true,
        wrap: Wrap::Clamp,
        ..Material::default()
    };
    let linear = Material {
        texture: checker,
        wrap: Wrap::Clamp,
        ..Material::default()
    };
    let squares = [nearest, linear]
        .map(|material| MeshDraw::with_material(square, &material, &mut stores.shaders));
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
    let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
    render_without_ui(&mut renderer, data);
    let instances = (columns * rows) as usize;
    let drawn = DrawCounts {
        draws: 2,
        instances: 2 * instances,
        triangles: 4 * instances,
    };
    assert_eq!(renderer.drawn(), drawn, "each run is one instanced draw");
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
    // Counterclockwise seen from +Z, its texture's top left at its top left.
    let corner = |[x, y]: [f32; 2], uv: [f32; 2]| ModelVertex {
        position: Vec3::new(x, y, 0.0),
        uv: Vec2::from(uv),
        color: Vec4::ONE,
    };
    let model_corners = [
        corner([-0.5, -0.5], [0.0, 1.0]),
        corner([0.5, -0.5], [1.0, 1.0]),
        corner([0.5, 0.5], [1.0, 0.0]),
        corner([-0.5, 0.5], [0.0, 0.0]),
    ];
    let model_square = (stores.meshes)
        .add("model square", &model_corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let model = MeshDraw::with_material(model_square, &Material::default(), &mut stores.shaders);
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
    let data = stores.data(filling([WIDTH, HEIGHT]), viewpoint, &draws);
    render_without_ui(&mut renderer, data);
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
    let center = |renderer: &mut Renderer| {
        let pixels = (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back");
        let at = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    };
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
        let data = stores.data(filling([SIZE, SIZE]), Viewpoint::default(), &draws);
        render_without_ui(renderer, data);
        (renderer.read_back())
            .expect("the frame's pixels")
            .expect("a headless renderer's frame can be read back")
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
