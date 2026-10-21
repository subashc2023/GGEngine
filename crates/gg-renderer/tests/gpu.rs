//! GPU tests of rendering, against reference images. They run on whatever GPU the loader offers,
//! which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another.
//! `GG_UPDATE_REFERENCES=1` writes each test's output as its new reference instead of comparing.

// For setting the environment; see the SAFETY comment.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use bytemuck::NoUninit;
use gg_core::math::{Mat4, Quat, Vec3, Vec4};
use gg_renderer::{
    Blend, Camera, Draws, Meshes, Projection, RenderData, Renderer, Sampler, Settings, Shaders,
    Texture, Textures, Ui, Viewport,
};

fn main() {
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
    run("meshes_match_their_reference", meshes_match_their_reference);
    run(
        "textures_match_their_reference",
        textures_match_their_reference,
    );
}

fn run(name: &str, test: impl FnOnce()) {
    test();
    println!("test {name} ... ok");
}

/// What the tests clear to, in linear RGBA.
const CLEAR: [f32; 4] = [0.02, 0.03, 0.05, 1.0];

/// A headless renderer of `width` by `height` pixels, under validation.
fn renderer(width: u32, height: u32) -> Renderer {
    let settings = Settings {
        app_name: "renderer gpu tests",
        validation: true,
        gpu: None,
        vsync: false,
    };
    let renderer = Renderer::headless(&settings, width, height).expect("a renderer");
    assert!(
        renderer.validation().enabled(),
        "GPU tests run under validation; install the Vulkan SDK's validation layer"
    );
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
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera: Camera::default(),
            meshes: &mut meshes,
            shaders: &mut shaders,
            textures: &mut textures,
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
/// clear of them, so that a draw given another's parameters changes a thousand. The triangle is
/// added after the first frame, and the frames outnumber those in flight, so that later frames
/// draw from what earlier ones copied to the GPU.
fn meshes_match_their_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
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
            Quat::from_rotation_z(turn.to_radians()),
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
    draws.push(square, flat, transform(0.8, 0.0, [3.2, 1.8]), orange);
    for frame in 0..3 {
        if frame == 1 {
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
        let data = RenderData {
            clear_color: CLEAR,
            // Off the image's corner, so that the image shows where the view is.
            viewport: Viewport {
                origin: [40, 20],
                size: [240, 160],
            },
            camera,
            meshes: &mut meshes,
            shaders: &mut shaders,
            textures: &mut textures,
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
/// color, so that an image flipped moves it. A texture added first and never drawn takes slot 0,
/// so that textures put in the wrong slots, or sampled through the wrong ones, show it or each
/// other; the one with alpha is added after the first frame. Its texels are opaque, half, a
/// quarter and wholly transparent, so that a shader blended as opaque, or as though its alpha were
/// multiplied in, changes thousands of pixels; and the textures are sRGB, so that colors sampled
/// without decoding come out brighter.
fn textures_match_their_reference() {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();
    let mut shaders = Shaders::default();
    let mut textures = Textures::default();
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

    textures
        .add("unused", [1, 1], &[255, 0, 255, 255])
        .expect("a texture");
    // 4 by 4 texels, sRGB-encoded: a checkerboard of red and blue, with yellow at the top left.
    let checkerboard: Vec<u8> = (0..16)
        .flat_map(|i| match (i % 4, i / 4) {
            (0, 0) => [250, 220, 60, 255],
            (x, y) if (x + y) % 2 == 0 => [200, 60, 40, 255],
            _ => [40, 90, 200, 255],
        })
        .collect();
    let checkerboard = textures
        .add("checkerboard", [4, 4], &checkerboard)
        .expect("a texture");
    let mut draws = Draws::default();
    // x from -2 to 0, pixels 40 to 120, each texel 10 pixels across.
    let sampling = Sampling {
        texture: checkerboard,
        sampler: Sampler::NearestRepeat.index(),
    };
    draws.push(tiled, textured, place(-1.0), sampling);
    for frame in 0..3 {
        if frame == 1 {
            // White, opaque and half transparent along the top; red a quarter opaque, then
            // nothing, along the bottom.
            let pixels = [
                [255, 255, 255, 255],
                [255, 255, 255, 128],
                [255, 0, 0, 64],
                [0, 0, 0, 0],
            ];
            let translucent = textures
                .add("translucent", [2, 2], pixels.as_flattened())
                .expect("a texture");
            // x from -1 to 1, over the checkerboard's right half and the clear color, each texel
            // 40 pixels across.
            let sampling = Sampling {
                texture: translucent,
                sampler: Sampler::NearestClamp.index(),
            };
            draws.push(plain, textured, place(0.0), sampling);
        }
        let data = RenderData {
            clear_color: CLEAR,
            viewport: Viewport {
                origin: [0, 0],
                size: [WIDTH, HEIGHT],
            },
            camera,
            meshes: &mut meshes,
            shaders: &mut shaders,
            textures: &mut textures,
            draws: &draws,
        };
        render_without_ui(&mut renderer, data);
    }
    check(renderer, "textures", WIDTH, HEIGHT);
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
