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

use gg_renderer::{Meshes, RenderData, Renderer, Settings, Ui, Vertex, Viewport};

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
    run(
        "triangle_matches_its_reference",
        triangle_matches_its_reference,
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

/// Draws a window of egui widgets offscreen, and compares it with its reference image.
fn ui_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();

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
            meshes: &mut meshes,
            draws: &[],
        };
        assert!(
            renderer.render(data, ui, &mut []).expect("a frame"),
            "an offscreen image can always be rendered into"
        );
    }
    check(renderer, "ui", WIDTH, HEIGHT);
}

/// Draws a triangle in red, green and blue into part of an offscreen image, over more frames
/// than are in flight, so that the later frames draw from what the first copied to the GPU, and
/// compares it with its reference image.
fn triangle_matches_its_reference() {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let mut renderer = renderer(WIDTH, HEIGHT);
    let mut meshes = Meshes::default();
    let corner = |position, color| Vertex { position, color };
    let vertices = [
        corner([0.0, -0.5, 0.0], [1.0, 0.0, 0.0, 1.0]),
        corner([0.5, 0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
        corner([-0.5, 0.5, 0.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let triangle = meshes
        .add("triangle", &vertices, &[0, 1, 2])
        .expect("a mesh");
    for _ in 0..3 {
        let data = RenderData {
            clear_color: CLEAR,
            // Off the image's corner, so that the image shows where the view is.
            viewport: Viewport {
                origin: [40, 20],
                size: [240, 160],
            },
            meshes: &mut meshes,
            draws: &[triangle],
        };
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
    check(renderer, "triangle", WIDTH, HEIGHT);
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
