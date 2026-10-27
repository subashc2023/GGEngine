//! GPU tests of rendering, against reference images. They run on whatever GPU the loader offers,
//! which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another.
//! `GG_UPDATE_REFERENCES=1` writes each test's output as its new reference instead of comparing.

// For setting the environment; see the SAFETY comment.
#![allow(unsafe_code)]

use std::f32::consts::{FRAC_PI_2, PI};
use std::fs::{self, File};
use std::io::BufWriter;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::math::{self, Mat4, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{
    AlphaMode, Blend, Camera, DrawCounts, Draws, Material, MeshDraw, Meshes, Mipmaps, ModelVertex,
    Projection, RenderData, Renderer, Sampler, Settings, Shaders, Sprite, SpriteSheet, Texture,
    Textures, Ui, UploadPath, Uploads, ViewCallback, Viewport, Wrap,
};

fn main() {
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
        return;
    }
    // Overlays such as Steam's and OBS's load into every instance as implicit layers, and can
    // add validation errors of their own.
    // SAFETY: no other thread exists yet to read the environment.
    unsafe { std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~") };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    gg_core::log::init("renderer-gpu-tests", &dir);

    run("ui_matches_its_reference", ui_matches_its_reference);
    run(
        "sprites_match_their_reference",
        sprites_match_their_reference,
    );
    run(
        "batches_match_their_reference",
        batches_match_their_reference,
    );
    run("views_match_their_reference", views_match_their_reference);
    run(
        "a_view_in_the_ui_matches_its_reference",
        a_view_in_the_ui_matches_its_reference,
    );
    run(
        "replaced_shaders_draw_from_the_next_frame",
        replaced_shaders_draw_from_the_next_frame,
    );
    run(
        "pipelines_go_through_the_cache",
        pipelines_go_through_the_cache,
    );
    run("capturing_needs_renderdoc", capturing_needs_renderdoc);
    run("depth_matches_its_reference", depth_matches_its_reference);
    run("models_match_their_reference", models_match_their_reference);
    run(
        "mixed_textures_match_their_reference",
        mixed_textures_match_their_reference,
    );
    // Once for each way of uploading that the GPU has.
    for path in UploadPath::ALL {
        if let Err(error) = Renderer::headless(&settings(Some(path)), 1, 1) {
            assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
            println!("tests uploading by {path} skipped: {error}");
            continue;
        }
        let each = |name: &str, test: fn(UploadPath)| {
            run(&format!("{name} uploading by {path}"), || test(path));
        };
        each("meshes_match_their_reference", meshes_match_their_reference);
        each(
            "textures_match_their_reference",
            textures_match_their_reference,
        );
        each("sheets_match_their_reference", sheets_match_their_reference);
        each(
            "a_large_texture_draws_in_the_frame_that_adds_it",
            a_large_texture_draws_in_the_frame_that_adds_it,
        );
        each(
            "mipmaps_match_their_reference",
            mipmaps_match_their_reference,
        );
    }
}

fn run(name: &str, test: impl FnOnce()) {
    test();
    println!("test {name} ... ok");
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

/// Draws a window of egui widgets offscreen, and compares it with its reference image.
fn ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera: Camera::default(),
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top, which is 160 pixels, so 40 pixels a unit both
    // ways; the view's center is (1, 0.5).
    let camera = Camera {
        position: Vec3::new(1.0, 0.5, 0.0),
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    let transform = |scale, turn: f32, [x, y]: [f32; 2]| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_z(math::to_radians(turn)),
            Vec3::new(x, y, 0.0),
        )
    };
    // Positions, and a color for each draw.
    let flat = shaders
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let mut draws = Draws::default();
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    draws.push(square, flat, transform(2.5, 30.0, [0.5, 0.5]), blue);
    let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
    let second = MeshDraw::new(square, flat, orange);
    draws.push_draw(transform(0.8, 0.0, [3.2, 1.8]), &second);
    for frame in 0..3 {
        if frame == 2 {
            // Positions, then colors; no parameters.
            let colored = shaders
                .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
                .expect("a shader");
            let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
            // Pointing up.
            let vertices = [
                corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
                corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
                corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
            ];
            let triangle = meshes
                .add("triangle", &vertices, &[0, 1, 2])
                .expect("a mesh");
            draws.push(triangle, colored, transform(2.0, 0.0, [1.0, 0.5]), ());
        }
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            // Off the image's corner, so that the image shows where the view is.
            viewport: Viewport {
                origin: [40, 20],
                size: [240, 160],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    // Positions, then texture coordinates.
    let textured = shaders
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
        meshes
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
    let checkerboard = textures
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
            let translucent = textures
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
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
    let sheet = textures.add("sheet", [4, 4], &sheet, Mipmaps::None);
    let sheet = sheet.expect("a texture");
    // `width` by `height` units, centered at (`x`, `y`). Every sprite's edges fall between pixels.
    let place = |[x, y]: [f32; 2], [width, height]: [f32; 2]| {
        Mat4::from_translation(Vec3::new(x, y, 0.0))
            * Mat4::from_scale(Vec3::new(width, height, 1.0))
    };
    let whole = Sprite {
        texture: sheet,
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
        render_without_ui(&mut renderer, data);
    }
    check(renderer, "sprites", WIDTH, HEIGHT);
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    // 2 by 2 texels, sRGB-encoded: red and yellow over green and blue; and a checkerboard of light
    // and dark.
    let quadrants = [
        [220, 50, 40, 255],
        [240, 200, 40, 255],
        [40, 170, 70, 255],
        [40, 80, 220, 255],
    ];
    let quadrants = textures
        .add("quadrants", [2, 2], quadrants.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let (light, dark) = ([240, 240, 240, 255], [20, 20, 20, 255]);
    let checker = [light, dark, dark, light];
    let checker = textures
        .add("checker", [2, 2], checker.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let white = Sprite::default().texture;
    // Orange, the quadrants as they are, the checker in cyan, violet, the quadrants half
    // transparent, and the checker in yellow.
    let looks = [
        (white, Vec4::new(0.9, 0.4, 0.1, 1.0)),
        (quadrants, Vec4::ONE),
        (checker, Vec4::new(0.3, 0.8, 1.0, 1.0)),
        (white, Vec4::new(0.6, 0.2, 0.9, 1.0)),
        (quadrants, Vec4::new(1.0, 1.0, 1.0, 0.5)),
        (checker, Vec4::new(1.0, 0.9, 0.3, 1.0)),
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
    let flat = shaders
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    // Across the middle, from y = -0.45 to 0.45, in slate.
    let band = Mat4::from_scale(Vec3::new(5.8, 0.9, 1.0));
    let slate = Vec4::new(0.25, 0.3, 0.4, 1.0);
    let away = Mat4::from_translation(Vec3::new(100.0, 100.0, 0.0));
    let unseen = Sprite {
        color: Vec4::new(1.0, 0.0, 1.0, 1.0),
        texture: quadrants,
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 5 units from the view's bottom to its top, 40 pixels a unit, and 6 across, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 5.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
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
        texture: (textures.add("sheet", [8, 4], &pixels, Mipmaps::None)).expect("a texture"),
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
        texture: (textures.add("strip", [46, 2], &strip, Mipmaps::None)).expect("a texture"),
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
        texture: sheet.texture,
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
        texture: sheet.texture,
        rect: [Vec2::new(0.75, 0.0), Vec2::new(1.25, 1.0)],
        sampler: Sampler::LinearRepeat,
        ..Sprite::default()
    };
    draws.push_sprite(place([-0.4, -1.8], [1.2, 0.8], 0.0), tiled);
    // On the edge between texels 3 and 4, down the whole sheet.
    let line = Sprite {
        texture: sheet.texture,
        rect: [Vec2::new(0.5, 0.0), Vec2::new(0.5, 1.0)],
        ..Sprite::default()
    };
    draws.push_sprite(place([0.8, -1.8], [0.4, 0.8], 0.0), line);
    // More frames than are in flight.
    for _ in 0..3 {
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top; the view's center is (1, 0.5).
    let camera = Camera {
        position: Vec3::new(1.0, 0.5, 0.0),
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    let transform = |scale, turn: f32, [x, y]: [f32; 2]| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_z(math::to_radians(turn)),
            Vec3::new(x, y, 0.0),
        )
    };
    let flat = shaders
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let colored = shaders
        .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
    let vertices = [
        corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
        corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
        corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let triangle = meshes
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
    let tile = textures
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
        texture: tile,
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport,
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 in front of the origin, seeing a sixth of a turn from the view's bottom to its top: at z =
    // 0, about 4.6 units.
    let camera = Camera {
        position: Vec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Camera::default()
    };
    let flat = shaders
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // Positions, then texture coordinates.
    let textured = shaders
        .add::<[f32; 5], Sampling>("textured", &shader("textured.slang"), Blend::Opaque)
        .expect("a shader");
    // The whole view, which is 2 units high and as wide.
    let corners = [
        [-1.0, -1.0, 0.0, 0.0, 1.0],
        [1.0, -1.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 0.0, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let pixels = COLOR.repeat((TEXELS * TEXELS) as usize);
    let large = textures
        .add("large", [TEXELS, TEXELS], &pixels, Mipmaps::None)
        .expect("a texture");
    let mut draws = Draws::default();
    let sampling = Sampling {
        texture: large,
        sampler: Sampler::NearestClamp.index(),
    };
    draws.push(square, textured, Mat4::IDENTITY, sampling);
    uploads.take(&mut meshes, &mut shaders, &mut textures);
    let data = RenderData {
        clear_color: CLEAR,
        viewport: Viewport {
            origin: [0, 0],
            size: [SIDE, SIDE],
        },
        camera: Camera {
            projection: Projection::Orthographic {
                height: 2.0,
                near: -1.0,
                far: 1.0,
            },
            ..Camera::default()
        },
        uploads: &mut uploads,
        draws: &draws,
    };
    render_without_ui(&mut renderer, data);
    let validation = renderer.validation();
    let pixels = renderer
        .read_back()
        .expect("the frame's pixels")
        .expect("a headless renderer's frame can be read back");
    drop(renderer);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
    for pixel in pixels.as_chunks::<4>().0 {
        let close = pixel
            .iter()
            .zip(COLOR)
            .all(|(&got, want)| got.abs_diff(want) <= 1);
        assert!(close, "read back {pixel:?}, expected {COLOR:?}");
    }
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 in front of the origin, seeing a sixth of a turn from the view's bottom to its top: at z =
    // 0, about 4.6 units.
    let camera = Camera {
        position: Vec3::new(0.0, 0.0, 4.0),
        projection: Projection::Perspective {
            fov: PI / 3.0,
            near: 0.5,
        },
        ..Camera::default()
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
        meshes
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
    let cutout = textures
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
        |mesh, material: &Material| MeshDraw::with_material(mesh, material, &mut shaders);
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
        draws.push_draw(*transform, draw);
    }
    for _ in 0..2 {
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // A unit a pixel, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    // 2 by 2 texels, sRGB-encoded: red and yellow over green and blue; and a checkerboard of light
    // and dark, which a linear filter blends between its texels.
    let quadrants = [
        [220, 50, 40, 255],
        [240, 200, 40, 255],
        [40, 170, 70, 255],
        [40, 80, 220, 255],
    ];
    let quadrants = textures
        .add("quadrants", [2, 2], quadrants.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let (light, dark) = ([240, 240, 240, 255], [20, 20, 20, 255]);
    let checker = [light, dark, dark, light];
    let checker = textures
        .add("checker", [2, 2], checker.as_flattened(), Mipmaps::None)
        .expect("a texture");
    let sprites = [
        Sprite {
            texture: quadrants,
            sampler: Sampler::NearestClamp,
            ..Sprite::default()
        },
        Sprite {
            texture: checker,
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
    let square = meshes
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
    let squares =
        [nearest, linear].map(|material| MeshDraw::with_material(square, &material, &mut shaders));
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
            draws.push_draw(place(column, row, 0.0), square);
        }
    }
    uploads.take(&mut meshes, &mut shaders, &mut textures);
    let data = RenderData {
        clear_color: CLEAR,
        viewport: Viewport {
            origin: [0, 0],
            size: [WIDTH, HEIGHT],
        },
        camera,
        uploads: &mut uploads,
        draws: &draws,
    };
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // A unit a pixel, around the origin.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: HEIGHT as f32,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    let leveled = shaders
        .add::<[f32; 5], Level>("level", &shader("level.slang"), Blend::Alpha)
        .expect("a shader");
    let corners = [
        [-32.0, -32.0, 0.0, 0.0, 1.0],
        [32.0, -32.0, 0.0, 1.0, 1.0],
        [32.0, 32.0, 0.0, 1.0, 0.0],
        [-32.0, 32.0, 0.0, 0.0, 0.0],
    ];
    let square = meshes
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
    let checkers = textures
        .add("checkers", [64, 64], &checkers, Mipmaps::AlphaWeighted)
        .expect("a texture");
    // A color for each texel.
    let odd: Vec<u8> = (0..15u32)
        .flat_map(|i| [i * 17, 255 - i * 13, i * 71 % 256, 255].map(|c| c as u8))
        .collect();
    let odd = textures
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
    uploads.take(&mut meshes, &mut shaders, &mut textures);
    let data = RenderData {
        clear_color: CLEAR,
        viewport: Viewport {
            origin: [0, 0],
            size: [WIDTH, HEIGHT],
        },
        camera,
        uploads: &mut uploads,
        draws: &draws,
    };
    render_without_ui(&mut renderer, data);
    check(renderer, "mipmaps", WIDTH, HEIGHT);
}

/// Draws a view where the UI paints a `ViewCallback`, among the UI's shapes rather than under them
/// all, and compares the frame with its reference image: a red rect under the view, which shows
/// where the callback's clip rect cuts the view's edges, and a green one over it. The view is a
/// blue square that fills it, under a triangle in red, green and blue. The callback is off
/// `data.viewport`'s origin, the frame's corner, where the frame shows the view without one, so
/// that the view shown there too, unclipped, or over the green rect, changes thousands of pixels. A
/// frame whose view has no area draws the UI alone.
fn a_view_in_the_ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    // 4 units from the view's bottom to its top, which is 160 pixels: 40 pixels a unit.
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 4.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
    };
    let flat = shaders
        .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
        .expect("a shader");
    let colored = shaders
        .add::<[f32; 7], ()>("vertex color", &shader("vertex_color.slang"), Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let corner = |[x, y, z]: [f32; 3], [r, g, b, a]: [f32; 4]| [x, y, z, r, g, b, a];
    let vertices = [
        corner([0.0, 0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
        corner([0.5, -0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
        corner([-0.5, -0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let triangle = meshes
        .add("triangle", &vertices, &[0, 1, 2])
        .expect("a mesh");
    let mut draws = Draws::default();
    let blue = Vec4::new(0.2, 0.3, 0.8, 1.0);
    draws.push(square, flat, Mat4::from_scale(Vec3::splat(10.0)), blue);
    draws.push(triangle, colored, Mat4::from_scale(Vec3::splat(3.0)), ());

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
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let ui = Ui {
            primitives: &primitives,
            textures: &mut output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        };
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size,
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
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
    let validation = renderer.validation();
    drop(renderer);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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

/// What [`replaced_shaders_draw_from_the_next_frame`] writes as its shader, in the color `COLOR`
/// stands for.
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

[shader("fragment")]
float4 fragment_main() : SV_Target {
    return COLOR;
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
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
    let mut uploads = Uploads::default();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replaced.slang");
    let write = |color: &str| fs::write(&path, REPLACED.replace("COLOR", color));
    write("float4(1.0, 0.0, 0.0, 1.0)").expect("the shader's file");
    let shader = shaders
        .add::<[f32; 3], ()>("replaced", &path, Blend::Opaque)
        .expect("a shader");
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let square = meshes
        .add("square", &corners, &[0, 1, 2, 2, 3, 0])
        .expect("a mesh");
    let mut draws = Draws::default();
    draws.push(square, shader, Mat4::IDENTITY, ());
    let camera = Camera {
        projection: Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        },
        ..Camera::default()
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
            let compiled = shaders.sources()[0].compile();
            shaders.replace(compiled.expect("the shader compiles again"));
        }
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [SIZE, SIZE],
            },
            camera,
            uploads: &mut uploads,
            draws: &draws,
        };
        render_without_ui(&mut renderer, data);
        // Reading back waits for the GPU, so not between the first frame and the replacement.
        match frame {
            0 => assert_eq!(center(&mut renderer), [255, 0, 0, 255], "red"),
            3 | 5 => assert_eq!(center(&mut renderer), [0, 255, 0, 255], "green"),
            _ => {}
        }
    }
    let validation = renderer.validation();
    drop(renderer);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let mut textures = Textures::default();
        let mut uploads = Uploads::default();
        let flat = shaders
            .add::<[f32; 3], Vec4>("flat color", &shader("flat_color.slang"), Blend::Opaque)
            .expect("a shader");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = meshes
            .add("triangle", &corners, &[0, 1, 2])
            .expect("a mesh");
        let mut draws = Draws::default();
        let orange = Vec4::new(0.9, 0.4, 0.1, 1.0);
        draws.push(triangle, flat, Mat4::IDENTITY, orange);
        uploads.take(&mut meshes, &mut shaders, &mut textures);
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [SIZE, SIZE],
            },
            camera: Camera::default(),
            uploads: &mut uploads,
            draws: &draws,
        };
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
    let validation = [first.validation(), second.validation()];
    drop((first, second));
    for validation in validation {
        assert_eq!(
            validation.messages(),
            0,
            "validation reported problems; the log above says what"
        );
    }
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

/// Reads back the last frame `renderer` rendered, shuts it down, checks that validation reported
/// nothing, and compares the frame with reference image `name`.
fn check(mut renderer: Renderer, name: &str, width: u32, height: u32) {
    let validation = renderer.validation();
    let pixels = renderer
        .read_back()
        .expect("the frame's pixels")
        .expect("a headless renderer's frame can be read back");
    drop(renderer);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
    compare(name, width, height, &pixels);
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
