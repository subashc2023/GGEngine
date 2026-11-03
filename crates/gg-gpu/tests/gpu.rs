//! GPU tests. They run on whatever GPU the loader offers, which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another: those whose
//! names contain an argument that isn't a flag, as libtest picks them, or all of them without one.

// For setting the environment, and for breaking a Vulkan rule on purpose; see the SAFETY
// comments.
#![allow(unsafe_code)]

use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;

use ash::vk;
use gg_gpu::{
    Blend, ColorImage, DepthImage, Device, Frame, Frames, GpuBuffer, GraphicsDesc, IdImage,
    Instance, InstanceDesc, MAX_READS, PLACED_TEXTURES, Pipeline, Sampler, Target, Texture,
    TextureFormat, UploadPath, Validation, mip_sizes,
};

/// Runs each test through `tests`, by the name it's written with, given the arguments written
/// after it, if any.
macro_rules! run {
    ($tests:ident $(, $test:ident $(($($arg:expr),*))?)+ $(,)?) => {
        $($tests.run(stringify!($test), || $test($($($arg),*)?));)+
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
    // add validation errors of their own. Apps keep implicit layers, since RenderDoc's capture
    // layer is one; the loader warns about each one it leaves out here. Unless `RUST_LOG` is set,
    // only warnings and errors are logged, and of the loader's and other layers' messages only
    // errors, so that a passing run's output is its tests' results rather than what each
    // instance, device and pipeline logs as it's made.
    // SAFETY: no other thread exists yet to read the environment.
    unsafe {
        std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        if std::env::var_os("RUST_LOG").is_none() {
            std::env::set_var("RUST_LOG", "warn,gg_gpu::vulkan::general=error");
        }
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    let log = gg_core::log::init("gpu-tests", &dir, true).expect("the tests read the JSON log");

    let mut tests = Tests::new();
    run!(
        tests,
        clear_reads_back,
        sampled_images_keep_their_layout,
        passes_over_keep_what_their_target_holds,
        images_read_back_once_their_frame_has_finished,
        dropped_images_free_their_slots,
        depth_clears_in_every_pass,
        ids_read_back_two_frames_later,
        streamed_textures_can_be_written_over_on_every_path,
        bc7_textures_stream_and_decode_on_every_path,
        textures_refill_between_rgba8_and_bc7_on_every_path,
        textures_and_buffers_stream_by_parts_on_every_path,
        textures_come_to_show_images_before_the_frames_passes,
        forgotten_frames_and_passes_are_caught(&log),
        another_devices_objects_are_refused,
        refused_texture_slots_leak_nothing,
        passes_refuse_what_the_device_cant_draw,
        pipeline_caches_load_what_this_gpu_saved_whole,
        captures_are_refused_without_renderdoc,
        validation_errors_reach_the_json_log(&log),
    );
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

/// Runs `tried`, and gives what it returned, or the message of the panic that ended it. The panic
/// isn't logged, as a failing test's is: the panic hook is set aside while it runs.
fn tries<T>(tried: impl FnOnce() -> T) -> Result<T, String> {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let caught = panic::catch_unwind(AssertUnwindSafe(tried));
    panic::set_hook(hook);
    caught.map_err(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
            })
            .unwrap_or_else(|| "a panic whose payload isn't a string".to_owned())
    })
}

/// Runs `broken`, which must panic, and gives its message.
fn panics(broken: impl FnOnce()) -> String {
    tries(broken).expect_err("a panic")
}

fn instance() -> Arc<Instance> {
    let instance = Instance::new(&InstanceDesc {
        app_name: "gpu tests",
        validation: true,
        window: None,
    })
    .expect("a Vulkan instance");
    assert!(
        instance.validation().enabled(),
        "GPU tests run under validation; install the Vulkan SDK's validation layer"
    );
    assert!(
        instance.validation().synchronization(),
        "GPU tests run under synchronization validation; install a newer Vulkan SDK, whose \
         validation layer takes VK_EXT_layer_settings"
    );
    instance
}

/// Checks that validation reported nothing, once what it validated has been dropped.
fn check(validation: &Validation) {
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// A point of a texture, and the color the GPU should read there.
struct Probe {
    level: u32,
    uv: [f32; 2],
    expected: [u8; 4],
}

/// White, which a stand-in made with [`stand_in`] reads as, at each of `probes`.
fn white_probes(probes: &[Probe]) -> Vec<Probe> {
    probes
        .iter()
        .map(|probe| Probe {
            level: probe.level,
            uv: probe.uv,
            expected: [255; 4],
        })
        .collect()
}

/// A color for block or level `n` of a texture, distinct among 128 of them. Its channels share a
/// low bit, `n`'s, which BC7's mode 6 needs to hold it exactly.
fn color_of(n: u32) -> [u8; 4] {
    let low = (n % 2) as u8;
    // Each step is odd, so that its channel takes all 128 values in turn.
    let channel = |step: u32, start: u32| ((((n * step + start) % 128) as u8) << 1) | low;
    [channel(53, 0), channel(91, 17), channel(29, 101), 254 | low]
}

/// A BC7 block of mode 6 that holds `color` in all 16 pixels. The mode gives each endpoint's
/// channels 7 bits and the endpoint one more, the low bit of all four, so `color`'s channels must
/// share one. Both endpoints are `color`, so that every pixel is, whatever weight its index picks.
fn bc7_solid(color: [u8; 4]) -> [u8; 16] {
    let low = color[0] & 1;
    assert!(color.iter().all(|&channel| channel & 1 == low), "{color:?}");
    // The 128 bits, least significant first: the mode, six zeros then a one; each channel's two
    // endpoints; the endpoints' low bits, P0 and P1; and the indices.
    let mut bits = 1u128 << 6;
    let mut at = 7;
    for channel in color {
        for _endpoint in 0..2 {
            bits |= u128::from(channel >> 1) << at;
            at += 7;
        }
    }
    bits |= (u128::from(low) << at) | (u128::from(low) << (at + 1));
    bits.to_le_bytes()
}

/// The BC7 pixels of a `size` texture, a solid block of a color of its own to each, and a probe
/// for each block, at the middle of its pixels or the last row or column of its level if that ends
/// first, so that a block put in the wrong place, or the wrong level, shows.
fn bc7_blocks(size: [u32; 2], mipmapped: bool) -> (Vec<u8>, Vec<Probe>) {
    let mut pixels = Vec::new();
    let mut probes = Vec::new();
    let levels = if mipmapped { usize::MAX } else { 1 };
    for (level, [width, height]) in mip_sizes(size[0], size[1]).take(levels).enumerate() {
        for by in 0..height.div_ceil(4) {
            for bx in 0..width.div_ceil(4) {
                let color = color_of(probes.len() as u32);
                pixels.extend_from_slice(&bc7_solid(color));
                let x = (bx * 4 + 2).min(width - 1);
                let y = (by * 4 + 2).min(height - 1);
                probes.push(Probe {
                    level: level as u32,
                    uv: [
                        (x as f32 + 0.5) / width as f32,
                        (y as f32 + 0.5) / height as f32,
                    ],
                    expected: color,
                });
            }
        }
    }
    (pixels, probes)
}

/// The RGBA8 pixels of a `size` texture, a color of its own to each mip level, and a probe for
/// each level.
fn rgba8_levels(size: [u32; 2], mipmapped: bool) -> (Vec<u8>, Vec<Probe>) {
    let mut pixels = Vec::new();
    let mut probes = Vec::new();
    let levels = if mipmapped { usize::MAX } else { 1 };
    for (level, [width, height]) in mip_sizes(size[0], size[1]).take(levels).enumerate() {
        let color = color_of(level as u32);
        pixels.extend(color.repeat((width * height) as usize));
        probes.push(Probe {
            level: level as u32,
            uv: [0.5, 0.5],
            expected: color,
        });
    }
    (pixels, probes)
}

/// The SPIR-V of `tests/shaders/sample.slang`, compiled as the renderer's build compiles its
/// shaders, with the slangc that `SLANGC` names, else the Vulkan SDK's, under `VULKAN_SDK`, else
/// the first on the path.
fn sampling_spirv() -> Vec<u8> {
    let exe = if cfg!(windows) {
        "slangc.exe"
    } else {
        "slangc"
    };
    // The SDK's tools are in `Bin` on Windows and `bin` elsewhere.
    let in_sdk = |dir| {
        let sdk = std::env::var_os("VULKAN_SDK")?;
        Some(PathBuf::from(sdk).join(dir).join(exe)).filter(|path| path.exists())
    };
    let slangc = std::env::var_os("SLANGC")
        .map(PathBuf::from)
        .or_else(|| in_sdk("Bin"))
        .or_else(|| in_sdk("bin"))
        .unwrap_or_else(|| exe.into());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/sample.slang");
    let spirv = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sample.spv");
    let output = Command::new(&slangc)
        .arg(&source)
        .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
        .args(["-warnings-as-errors", "38006"])
        .args(["-entry", "vertex_main", "-stage", "vertex"])
        .args(["-entry", "fragment_main", "-stage", "fragment"])
        .arg("-o")
        .arg(&spirv)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "can't run {}: {error}; install the Vulkan SDK, or set SLANGC to slangc's path",
                slangc.display()
            )
        });
    assert!(
        output.status.success(),
        "slangc failed on {}:\n{}{}",
        source.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(&spirv).expect("the SPIR-V slangc wrote")
}

/// The pipeline that draws a probe of a texture, from `spirv`.
fn sampling_pipeline(device: &Arc<Device>, spirv: &[u8]) -> Pipeline {
    let desc = GraphicsDesc {
        name: "sample probes",
        spirv,
        vertex: c"vertex_main",
        fragment: c"fragment_main",
        format: ColorImage::FORMAT,
        blend: Blend::Opaque,
        depth_write: false,
    };
    Pipeline::graphics(device, &desc).expect("a pipeline")
}

/// A pass over target `target` of `frame`, an image a pixel across for each of `probes`, in which
/// the GPU samples `texture` at each probe, into a pixel of its own: by three vertices, or by the
/// first three of `indices`, a buffer of indices frames have taken, if given.
fn draw_probes(
    frame: &mut Frame<'_>,
    target: usize,
    pipeline: &Pipeline,
    texture: &Texture,
    probes: &[Probe],
    indices: Option<&GpuBuffer>,
) {
    let pass = frame.begin_pass(target, c"probes", [0.0; 4], None, None);
    let mut pass = pass.expect("an offscreen image is always there");
    pass.set_pipeline(pipeline);
    for (x, probe) in (0..).zip(probes) {
        pass.set_viewport([x, 0], [1, 1]);
        // As `sample.slang` takes them.
        let words = [
            texture.index(),
            Sampler::NearestClamp.index(),
            (probe.level as f32).to_bits(),
            probe.uv[0].to_bits(),
            probe.uv[1].to_bits(),
        ];
        pass.push_constants(&words.map(u32::to_ne_bytes).concat());
        match indices {
            Some(indices) => pass.draw_indexed(indices, 0, 3, 0, 1),
            None => pass.draw(3, 1),
        }
    }
}

/// Reads `image` back, and checks that its pixels are what `probes` expect, to within the 1 that
/// sRGB's encoding may round by; `what` says which texture.
fn check_probes(image: &mut ColorImage, probes: &[Probe], what: &str) {
    let pixels = image.read_back().expect("the image's pixels");
    for (pixel, probe) in pixels.as_chunks::<4>().0.iter().zip(probes) {
        let close = (pixel.iter().zip(probe.expected)).all(|(&got, want)| got.abs_diff(want) <= 1);
        let Probe {
            level,
            uv,
            expected,
        } = probe;
        assert!(
            close,
            "{what}: level {level} at {uv:?} reads {pixel:?}, expected {expected:?}"
        );
    }
}

/// A texture a pixel across, in a free slot, for placed textures' slots to show until they have
/// images of their own: white, once the first frame has written it ([`write_stand_in`]).
fn stand_in(device: &Arc<Device>, name: &str) -> Texture {
    let format = TextureFormat::Rgba8Srgb;
    Texture::new(device, 1, 1, format, false, name).expect("a texture")
}

/// Writes the stand-in white, in the first frame, before any pass samples a slot that shows it.
fn write_stand_in(frame: &mut Frame<'_>, stand_in: &mut Texture) {
    let written = frame.write_texture(stand_in, [0, 0], [1, 1], &[255; 4]);
    written.expect("a write");
}

/// Streams all of `pixels` into the image made for `texture` in one part, which a budget past
/// them takes.
fn stream_whole(frame: &mut Frame<'_>, texture: &mut Texture, pixels: &[u8]) {
    let copied = frame
        .stream_texture(texture, pixels, u64::MAX)
        .expect("a copy");
    assert_eq!(
        copied,
        pixels.len() as u64,
        "a budget past the pixels copies them at once"
    );
}

/// Clears two offscreen images in the same frames, each to a color of its own, over more frames
/// than are in flight, then reads the colors back.
fn clear_reads_back() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut images = [
        ColorImage::new(&device, 64, 32, "clear test target 0").expect("an image"),
        ColorImage::new(&device, 64, 32, "clear test target 1").expect("an image"),
    ];
    let mut frames = Frames::new(&device).expect("frames");

    let srgb = [[64, 128, 192, 255], [200, 30, 90, 255]];
    // The sRGB encoding's inverse; alpha isn't encoded.
    let linear = |byte: u8| {
        let c = f32::from(byte) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            gg_core::math::libm::powf((c + 0.055) / 1.055, 2.4)
        }
    };
    for _ in 0..3 {
        let mut frame = frames
            .begin(images.iter_mut().map(Target::Image))
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        // In the first two frames, the arena has no buffer yet.
        frame.upload(&[]).expect("an upload of nothing");
        for (target, srgb) in srgb.iter().enumerate() {
            let color = [linear(srgb[0]), linear(srgb[1]), linear(srgb[2]), 1.0];
            // A pass that draws nothing clears its target.
            drop(frame.begin_pass(target, c"clear", color, None, None));
        }
        frame.submit().expect("the frame's submission");
    }
    for (image, srgb) in images.iter_mut().zip(srgb) {
        let pixels = image.read_back().expect("the image's pixels");
        assert_eq!(pixels.len(), 64 * 32 * 4);
        for pixel in pixels.as_chunks::<4>().0 {
            let close = pixel
                .iter()
                .zip(srgb)
                .all(|(&got, want)| got.abs_diff(want) <= 1);
            assert!(close, "read back {pixel:?}, expected {srgb:?}");
        }
    }

    drop((frames, images, device, instance));
    check(&validation);
}

/// Clears an image, then lets the frame's later passes sample it, in more frames than are in
/// flight, and reads it back. The read-back's barrier starts from the layout the frames recorded,
/// so validation reports sampling that didn't change the image's layout, or didn't record the
/// change.
fn sampled_images_keep_their_layout() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 16, 16, "sampled test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.begin_pass(0, c"clear", [1.0, 0.0, 0.0, 1.0], None, None));
        frame.begin_sampling(0);
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    for pixel in pixels.as_chunks::<4>().0 {
        assert_eq!(*pixel, [255, 0, 0, 255]);
    }

    drop((frames, image, device, instance));
    check(&validation);
}

/// Clears an image, then starts a pass over it that draws nothing, and samples it, in more frames
/// than are in flight, and reads it back as the clear left it: synchronization validation sees the
/// second pass's load wait for the first's writes. A pass over the image before the frame's clear,
/// which it was made or sampled before, is refused before anything is recorded.
fn passes_over_keep_what_their_target_holds() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 16, 16, "kept test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        let message = panics(|| drop(frame.begin_pass_over(0, c"too soon")));
        assert!(message.contains("none has since"), "{message}");
        drop(frame.begin_pass(0, c"clear", [0.0, 0.0, 1.0, 1.0], None, None));
        drop(frame.begin_pass_over(0, c"kept"));
        frame.begin_sampling(0);
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    for pixel in pixels.as_chunks::<4>().0 {
        assert_eq!(*pixel, [0, 0, 255, 255]);
    }

    drop((frames, image, device, instance));
    check(&validation);
}

/// Clears images, a color for each frame, reads them back as frames render them, and finds each
/// frame's pixels once it has finished, until the frame two later reads back into its slot's
/// buffer; a frame reads a larger image than its slot read before whole, and samples it after.
/// Synchronization validation checks the copies' barriers, and the sampling after them.
fn images_read_back_once_their_frame_has_finished() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut images = [
        ColorImage::new(&device, 8, 4, "small read target").expect("an image"),
        ColorImage::new(&device, 16, 16, "large read target").expect("an image"),
    ];
    let mut frames = Frames::new(&device).expect("frames");
    let pixels_of = |frames: &Frames, frame: u64| {
        let read = frames.image_read(frame).expect("a wait for the frame")?;
        let [width, height] = read.size();
        let mut pixels = vec![0; (width * height * 4) as usize];
        read.read_into(&mut pixels).expect("the pixels");
        Some((read.size(), pixels))
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    // The image, small or large, its color, and whether the frame reads it back.
    for (image, color, read) in [
        (0, red, true),
        (0, green, true),
        (1, blue, true),
        (0, red, false),
    ] {
        let mut frame = frames
            .begin([Target::Image(&mut images[image])])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.begin_pass(0, c"clear", color, None, None));
        if read {
            let index = frame.read_image(0).expect("a read");
            assert_eq!(frame.begin_sampling(0), index, "sampled as it was read");
        }
        frame.submit().expect("the frame's submission");
    }
    let solid =
        |size: [u32; 2], pixel: [u8; 4]| Some((size, pixel.repeat((size[0] * size[1]) as usize)));
    assert_eq!(pixels_of(&frames, 2), solid([8, 4], [0, 255, 0, 255]));
    assert_eq!(pixels_of(&frames, 3), solid([16, 16], [0, 0, 255, 255]));
    assert_eq!(
        pixels_of(&frames, 1),
        None,
        "frame 3 read into its slot's buffer since"
    );
    assert_eq!(pixels_of(&frames, 4), None, "it read nothing back");
    assert_eq!(pixels_of(&frames, 0), None);

    drop((frames, images, device, instance));
    check(&validation);
}

/// Makes and drops more images, one at a time, than the bindless sets have slots to give out past
/// the placed ones, 4,096, which works only if each gives its slot back.
fn dropped_images_free_their_slots() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    for i in 0..5_000 {
        let image = ColorImage::new(&device, 4, 4, "dropped test image");
        drop(image.unwrap_or_else(|error| panic!("image {i}: {error:#}")));
    }

    drop((device, instance));
    check(&validation);
}

/// Clears an image and a depth image in two passes each frame, over more frames than are in
/// flight, so that every pass but the first follows another's use of the depth image, in its own
/// submission or the last: synchronization validation reports a pass whose barrier doesn't wait
/// for that use, and core validation one that leaves the depth image in the wrong layout.
fn depth_clears_in_every_pass() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 16, 16, "depth test target").expect("an image");
    let depth = DepthImage::new(&device, 16, 16, "depth test depth").expect("a depth image");
    let mut frames = Frames::new(&device).expect("frames");
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for _ in 0..2 {
            drop(frame.begin_pass(0, c"clear", [0.0, 0.0, 0.0, 1.0], Some(&depth), None));
        }
        frame.submit().expect("the frame's submission");
    }

    drop((frames, image, depth, device, instance));
    check(&validation);
}

/// Renders IDs beside an image in eight frames, each pass clearing them to 0, and reads pixels of
/// them back: one in the first frame, none in the second, three in the third, none in the fourth,
/// two in the fifth; in the sixth, sixty, then after another pass the four of ten more that the
/// most a frame reads leaves, then none; and none in the last two. Each frame finds what the one two
/// before it read, by its number, and none after a frame that read none, as the first two find of
/// frame 0. Each frame then begins sampling them, from the layout its reads left them in, or its
/// pass where it read none, and the sixth between its reads too, before its second pass.
/// Synchronization validation sees each copy wait for the pass before it, a pass for the copies
/// and the sampling before it, and the host for the copies. A pixel outside the IDs, IDs no pass of
/// the frame rendered, and a read once the frame samples them, are refused before anything is
/// recorded.
fn ids_read_back_two_frames_later() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 8, 4, "read test target").expect("an image");
    let mut ids = IdImage::new(&device, 8, 4, "read test IDs").expect("an ID image");
    let mut frames = Frames::new(&device).expect("frames");
    let black = [0.0, 0.0, 0.0, 1.0];
    let pixels: Vec<[u32; 2]> = (0..70).map(|i| [i % 8, i / 8 % 4]).collect();
    // How many IDs each frame reads.
    let counts = [1, 0, 3, 0, 2, MAX_READS, 0, 0];
    for (number, count) in (1..).zip(counts) {
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        assert_eq!(frame.number(), number);
        let (before, read) = frame.ids_read();
        let expected = match number {
            1 | 2 => (0, 0),
            _ => (number - 2, counts[number as usize - 3]),
        };
        assert_eq!((before, read.len()), expected, "frame {number}");
        assert!(read.iter().all(|&id| id == [0; 2]), "IDs read {read:?}");
        if number == 2 {
            // Frame 1's pass rendered them.
            let message = panics(|| {
                let _ = frame.read_ids(&mut ids, &pixels[..1]);
            });
            assert!(message.contains("no pass of frame 2"), "{message}");
            let message = panics(|| {
                let _ = frame.begin_sampling_ids(&mut ids);
            });
            assert!(message.contains("no pass of frame 2"), "{message}");
        }
        drop(frame.begin_pass(0, c"ids", black, None, Some(&mut ids)));
        if number == 3 {
            let message = panics(|| {
                let _ = frame.read_ids(&mut ids, &[[8, 0]]);
            });
            assert!(message.contains("outside the 8x4 IDs"), "{message}");
        }
        if count < MAX_READS {
            let read = frame.read_ids(&mut ids, &pixels[..count]);
            assert_eq!(read.expect("a read"), count, "frame {number}");
        } else {
            let read = |frame: &mut gg_gpu::Frame<'_>, ids: &mut IdImage, pixels| {
                frame.read_ids(ids, pixels).expect("a read")
            };
            assert_eq!(read(&mut frame, &mut ids, &pixels[..60]), 60);
            frame.begin_sampling_ids(&mut ids);
            drop(frame.begin_pass(0, c"ids again", black, None, Some(&mut ids)));
            assert_eq!(read(&mut frame, &mut ids, &pixels[60..]), 4);
            assert_eq!(read(&mut frame, &mut ids, &pixels[..1]), 0);
        }
        let index = frame.begin_sampling_ids(&mut ids);
        assert_eq!(frame.begin_sampling_ids(&mut ids), index, "sampled already");
        if number == 3 {
            let message = panics(|| {
                let _ = frame.read_ids(&mut ids, &pixels[..1]);
            });
            assert!(
                message.contains("before the frame samples them"),
                "{message}"
            );
        }
        frame.submit().expect("the frame's submission");
    }

    drop((frames, image, ids, device, instance));
    check(&validation);
}

/// Streams a texture, one with mip levels and a buffer in a frame, each whole in one part, then
/// writes over a pixel of the texture in each of the two frames after the one that shows it, more
/// frames than are in flight, once for each way of uploading that the GPU has: validation checks
/// the layout each way leaves the texture in against the one the write starts from, which it
/// doesn't for what the bindless sets sample. The copies of the texture with levels validation
/// checks against each level's size and the staged bytes. A texture whose slot shows a stand-in
/// can't be written, which is refused before anything is recorded.
fn streamed_textures_can_be_written_over_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path)) {
            Ok(device) => device,
            Err(error) => {
                assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
                println!("uploading by {path} skipped: {error}");
                continue;
            }
        };
        let mut image = ColorImage::new(&device, 4, 4, "fill test target").expect("an image");
        let format = TextureFormat::Rgba8Srgb;
        let mut stand_in = stand_in(&device, "fill test stand-in");
        let texture = Texture::placed(&device, 1, &stand_in, "streamed test texture");
        let mut texture = texture.expect("a texture");
        // 5x3, 2x1 and 1x1.
        let levels = Texture::placed(&device, 2, &stand_in, "mipmapped test texture");
        let mut levels = levels.expect("a texture");
        let mut buffer = GpuBuffer::new(&device, 64, "streamed test buffer").expect("a buffer");
        let mut frames = Frames::new(&device).expect("frames");
        let mut writes = 0;
        for i in 0..20 {
            let mut frame = frames
                .begin([Target::Image(&mut image)])
                .expect("a frame")
                .expect("an offscreen image can always be rendered into");
            if i == 0 {
                write_stand_in(&mut frame, &mut stand_in);
                let message = panics(|| {
                    let _ = frame.write_texture(&mut texture, [0, 0], [1, 1], &[0; 4]);
                });
                assert!(message.contains("shows a stand-in"), "{message}");
                texture.refill(8, 2, format, false).expect("an image");
                stream_whole(&mut frame, &mut texture, &[255; 8 * 2 * 4]);
                levels.refill(5, 3, format, true).expect("an image");
                stream_whole(&mut frame, &mut levels, &[127; (5 * 3 + 2 + 1) * 4]);
                let copied = frame.stream_buffer(&mut buffer, &[7; 64], u64::MAX);
                assert_eq!(copied.expect("a copy"), 64);
            }
            let shown = frame.finish_texture(&mut texture).expect("a finish");
            frame.finish_texture(&mut levels).expect("a finish");
            frame.finish_buffer(&mut buffer).expect("a finish");
            if shown && writes < 2 {
                let write = frame.write_texture(&mut texture, [writes, 0], [1, 1], &[0; 4]);
                write.expect("a write");
                writes += 1;
            }
            drop(frame.begin_pass(0, c"clear", [0.0, 0.0, 0.0, 1.0], None, None));
            frame.submit().expect("the frame's submission");
            if writes == 2 && i >= 2 {
                break;
            }
        }
        assert_eq!(
            writes, 2,
            "uploading by {path}: the texture was never shown"
        );
        assert_eq!(texture.size(), [8, 2]);
        drop((frames, texture, levels, buffer, stand_in, image, device));
    }

    drop(instance);
    check(&validation);
}

/// Streams BC7 textures in a frame, each whole in one part, once for each way of uploading that
/// the GPU has: one of 13x5 pixels, whose 4 by 2 blocks reach past its sides; another of 13x5 with
/// its full chain of mip levels, 6x2, 3x1 and 1x1 below it, which copies and validation check
/// against each level's blocks and the staged bytes; and one of a pixel. Each block is solid and
/// of a color of its own, and the GPU samples each in the frame that shows them, and again in the
/// two after it, which finds each block decoded, in the level and at the place it was streamed. A
/// BC7 texture can't be written, which is refused before anything is recorded.
fn bc7_textures_stream_and_decode_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path)) {
            Ok(device) => device,
            Err(error) => {
                assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
                println!("uploading by {path} skipped: {error}");
                continue;
            }
        };
        let pipeline = sampling_pipeline(&device, &spirv);
        let format = TextureFormat::Bc7Srgb;
        let cases = [
            ("bc7 single level", [13, 5], false),
            ("bc7 chain", [13, 5], true),
            ("bc7 pixel", [1, 1], true),
        ];
        let mut stand_in = stand_in(&device, "bc7 stand-in");
        let mut textures = Vec::new();
        let mut images = Vec::new();
        let mut fills = Vec::new();
        for (slot, (name, size, mipmapped)) in (0..).zip(cases) {
            let texture = Texture::placed(&device, slot, &stand_in, name);
            let mut texture = texture.expect("a texture");
            texture
                .refill(size[0], size[1], format, mipmapped)
                .expect("an image");
            textures.push(texture);
            let fill = bc7_blocks(size, mipmapped);
            assert_eq!(fill.0.len() as u64, format.bytes(size, mipmapped), "{name}");
            let image = ColorImage::new(&device, fill.1.len() as u32, 1, "bc7 probes");
            images.push(image.expect("an image"));
            fills.push(fill);
        }
        let mut frames = Frames::new(&device).expect("frames");
        let mut shown_at = None;
        for i in 0..20 {
            let mut frame = frames
                .begin(images.iter_mut().map(Target::Image))
                .expect("a frame")
                .expect("an offscreen image can always be rendered into");
            if i == 0 {
                write_stand_in(&mut frame, &mut stand_in);
                for (texture, (pixels, _)) in textures.iter_mut().zip(&fills) {
                    stream_whole(&mut frame, texture, pixels);
                }
            }
            let mut shown = true;
            for texture in &mut textures {
                shown &= frame.finish_texture(texture).expect("a finish");
            }
            if shown && shown_at.is_none() {
                shown_at = Some(i);
                let message = panics(|| {
                    let _ = frame.write_texture(&mut textures[0], [0, 0], [1, 1], &[0; 4]);
                });
                assert!(message.contains("BC7 texture is streamed"), "{message}");
            }
            for (target, (texture, (_, probes))) in textures.iter().zip(&fills).enumerate() {
                draw_probes(&mut frame, target, &pipeline, texture, probes, None);
            }
            frame.submit().expect("the frame's submission");
            for ((image, (_, probes)), (name, ..)) in images.iter_mut().zip(&fills).zip(cases) {
                let what = format!("{name}, uploading by {path}, frame {i}");
                match shown_at {
                    Some(_) => check_probes(image, probes, &what),
                    None => check_probes(image, &white_probes(probes), &what),
                }
            }
            if shown_at.is_some_and(|at| i >= at + 2) {
                break;
            }
        }
        assert!(shown_at.is_some(), "uploading by {path}: never shown");
        drop((frames, textures, images, stand_in, pipeline, device));
    }

    drop(instance);
    check(&validation);
}

/// Refills one texture in turn, as RGBA8 of 2x2, as BC7 of 13x5 with its mip levels, as RGBA8 of
/// 5x3 with its, as BC7 of 4x4 and of a pixel, each streamed whole in a frame, once for each way
/// of uploading that the GPU has: each refill makes the image anew in its format, which validation
/// checks the copies into against, and the texture's slot shows it from the frame that shows it,
/// found by the GPU sampling it there, as it samples the image before until then. Pixels of
/// another format's size are refused before anything changes.
fn textures_refill_between_rgba8_and_bc7_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path)) {
            Ok(device) => device,
            Err(error) => {
                assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
                println!("uploading by {path} skipped: {error}");
                continue;
            }
        };
        let pipeline = sampling_pipeline(&device, &spirv);
        let (rgba8, bc7) = (TextureFormat::Rgba8Srgb, TextureFormat::Bc7Srgb);
        let steps = [
            (rgba8, [2, 2], false),
            (bc7, [13, 5], true),
            (rgba8, [5, 3], true),
            (bc7, [4, 4], false),
            (bc7, [1, 1], false),
        ];
        // Wide enough for the most probes a step has, the 12 of 13x5's blocks in four levels.
        let mut image = ColorImage::new(&device, 12, 1, "refill probes").expect("an image");
        let mut stand_in = stand_in(&device, "refill stand-in");
        let texture = Texture::placed(&device, 5, &stand_in, "refilled format test texture");
        let mut texture = texture.expect("a texture");
        let mut frames = Frames::new(&device).expect("frames");
        // What the slot showed before the step, to sample until the step's image is shown.
        let mut before: Option<([u32; 2], Vec<Probe>)> = None;
        for (i, (format, size, mipmapped)) in steps.into_iter().enumerate() {
            let (pixels, probes) = if format == bc7 {
                bc7_blocks(size, mipmapped)
            } else {
                rgba8_levels(size, mipmapped)
            };
            texture
                .refill(size[0], size[1], format, mipmapped)
                .expect("an image");
            let mut shown = false;
            for frame_in_step in 0..20 {
                let mut frame = frames
                    .begin([Target::Image(&mut image)])
                    .expect("a frame")
                    .expect("an offscreen image can always be rendered into");
                if i == 0 && frame_in_step == 0 {
                    write_stand_in(&mut frame, &mut stand_in);
                }
                if frame_in_step == 0 {
                    // Pixels of RGBA8's size for BC7's, and of BC7's for RGBA8's; a 2x2 texture
                    // takes 16 bytes in both.
                    let other = if format == bc7 { rgba8 } else { bc7 };
                    let wrong = vec![0; other.bytes(size, mipmapped) as usize];
                    if wrong.len() != pixels.len() {
                        let message = panics(|| {
                            let _ = frame.stream_texture(&mut texture, &wrong, u64::MAX);
                        });
                        assert!(message.contains("don't fill"), "{message}");
                    }
                    stream_whole(&mut frame, &mut texture, &pixels);
                }
                shown = frame.finish_texture(&mut texture).expect("a finish");
                let (expected, what) = match (&before, shown) {
                    (_, true) => (&probes, "shown"),
                    (Some((_, probes)), false) => (probes, "before"),
                    (None, false) => (&white_probes(&probes), "the stand-in"),
                };
                draw_probes(&mut frame, 0, &pipeline, &texture, expected, None);
                frame.submit().expect("the frame's submission");
                let what = format!("{format:?} {size:?}, uploading by {path}, step {i}, {what}");
                check_probes(&mut image, expected, &what);
                if shown {
                    break;
                }
            }
            assert!(shown, "uploading by {path}: step {i} never shown");
            assert_eq!(texture.size(), size);
            before = Some((size, probes));
        }
        drop((frames, texture, stand_in, image, pipeline, device));
    }

    drop(instance);
    check(&validation);
}

/// Streams a texture of 64 by 64 texels with its mip levels, 21,844 bytes, and a buffer of 4,096
/// bytes of indices, a part a frame within a budget of 4 KiB for the texture and 1 KiB for the
/// buffer, once for each way of uploading that the GPU has. Each frame's part keeps within the
/// budget, in whole rows, and the texture takes six: four of 16 rows of its first level, its
/// second whole, and the tail of its five smaller levels together, which validation checks against
/// the levels and the staged bytes. Until the frame that shows the texture, which is the last
/// part's frame through the graphics queue or the host, and a later one through a transfer queue,
/// whose copies no frame waits for, the GPU samples its slot's stand-in, white; from that frame
/// on, its levels. The buffer draws the probes by its indices from the frame that takes it, which
/// comes at the earliest with its last part, as the texture's does. Then the texture is hidden
/// behind the stand-in, which the next frames sample; an image is streamed into it again and
/// hidden after its first part, whose copies may still run; another is streamed whole, handed
/// over on the transfer queue, and discarded in the next frame before it's shown; and the frames
/// go on, validation silent.
fn textures_and_buffers_stream_by_parts_on_every_path() {
    const BUDGET: u64 = 4096;
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path)) {
            Ok(device) => device,
            Err(error) => {
                assert_eq!(error.code(), "gpu.no_upload_path", "{error:#}");
                println!("uploading by {path} skipped: {error}");
                continue;
            }
        };
        let pipeline = sampling_pipeline(&device, &spirv);
        let format = TextureFormat::Rgba8Srgb;
        let mut stand_in = stand_in(&device, "parts stand-in");
        let texture = Texture::placed(&device, 3, &stand_in, "streamed by parts");
        let mut texture = texture.expect("a texture");
        let (pixels, probes) = rgba8_levels([64, 64], true);
        let total = pixels.len() as u64;
        let white = white_probes(&probes);
        let image = ColorImage::new(&device, probes.len() as u32, 1, "parts probes");
        let mut image = image.expect("an image");
        let mut buffer = GpuBuffer::new(&device, 4096, "streamed indices").expect("a buffer");
        let indices: Vec<u8> = (0..1024u32).flat_map(|i| (i % 3).to_ne_bytes()).collect();
        texture.refill(64, 64, format, true).expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let (mut texture_copied, mut buffer_copied) = (0, 0);
        let (mut texture_parts, mut buffer_parts) = (0, 0);
        let (mut shown_at, mut taken_at) = (None, None);
        let mut hidden_at = None;
        for i in 0..40 {
            let mut frame = frames
                .begin([Target::Image(&mut image)])
                .expect("a frame")
                .expect("an offscreen image can always be rendered into");
            if i == 0 {
                write_stand_in(&mut frame, &mut stand_in);
            }
            // The image streamed whole in the frame before, and handed over on the transfer
            // queue, is discarded before the frame could show it.
            let discarded = hidden_at.is_some_and(|at| i == at + 4);
            if discarded {
                assert_eq!(texture.streamed(), total);
                texture.discard();
                assert_eq!((texture.streamed(), texture.size()), (0, [0, 0]));
            }
            if texture_copied < total {
                let copied = frame.stream_texture(&mut texture, &pixels, BUDGET);
                let copied = copied.expect("a copy");
                assert!(
                    copied <= BUDGET,
                    "{copied} bytes, past the budget, by {path}"
                );
                texture_copied += copied;
                texture_parts += 1;
            }
            if buffer_copied < 4096 {
                let copied = frame.stream_buffer(&mut buffer, &indices, BUDGET / 4);
                let copied = copied.expect("a copy");
                assert_eq!(copied, 1024, "by {path}");
                buffer_copied += copied;
                buffer_parts += 1;
            }
            let shown = frame.finish_texture(&mut texture).expect("a finish");
            assert!(!(discarded && shown), "nothing left to show");
            if shown && shown_at.is_none() {
                shown_at = Some(i);
                assert_eq!(
                    texture_copied, total,
                    "shown before its last part, by {path}"
                );
            }
            let taken = frame.finish_buffer(&mut buffer).expect("a finish");
            if taken && taken_at.is_none() {
                taken_at = Some(i);
                assert_eq!(buffer_copied, 4096, "taken before its last part, by {path}");
            }
            // Once shown and sampled, hidden; then streamed into again and hidden mid-stream.
            let hide = shown_at.is_some_and(|at| i == at + 2);
            if hide {
                frame.hide_texture(&mut texture, &stand_in);
                hidden_at = Some(i);
                assert_eq!(texture.size(), [0, 0]);
            }
            if hidden_at.is_some_and(|at| i == at + 2) {
                texture.refill(64, 64, format, true).expect("an image");
                let copied = frame.stream_texture(&mut texture, &pixels, BUDGET);
                assert_eq!(copied.expect("a copy"), BUDGET);
                assert_eq!(texture.streamed(), BUDGET);
                frame.hide_texture(&mut texture, &stand_in);
                assert_eq!(texture.streamed(), 0);
            }
            // Streamed whole, and handed over on the transfer queue, for the next frame to
            // discard before it could be shown.
            if hidden_at.is_some_and(|at| i == at + 3) {
                texture.refill(64, 64, format, true).expect("an image");
                stream_whole(&mut frame, &mut texture, &pixels);
            }
            let expected = if shown && !hide && hidden_at.is_none() {
                &probes
            } else {
                &white
            };
            draw_probes(
                &mut frame,
                0,
                &pipeline,
                &texture,
                expected,
                taken.then_some(&buffer),
            );
            frame.submit().expect("the frame's submission");
            let what = format!("uploading by {path}, frame {i}");
            check_probes(&mut image, expected, &what);
            if hidden_at.is_some_and(|at| i >= at + 6) {
                break;
            }
        }
        let shown_at = shown_at.unwrap_or_else(|| panic!("by {path}: never shown"));
        let taken_at = taken_at.unwrap_or_else(|| panic!("by {path}: never taken"));
        assert_eq!((texture_parts, buffer_parts), (6, 4), "by {path}");
        match path {
            UploadPath::TransferQueue => {
                assert!(
                    shown_at > 5 && taken_at > 3,
                    "by {path}: {shown_at}, {taken_at}"
                );
            }
            UploadPath::HostImageCopy | UploadPath::GraphicsQueue => {
                assert_eq!((shown_at, taken_at), (5, 3), "by {path}");
            }
        }
        drop((frames, texture, buffer, stand_in, image, pipeline, device));
    }

    drop(instance);
    check(&validation);
}

/// Streams an image into a texture, and in each frame after, begins a pass before finishing it:
/// once its copies are done, the frame refuses to show the image after the pass, which would
/// sample it before it's ready, before anything changes, and the frame still submits; the next
/// frame shows it before its passes.
fn textures_come_to_show_images_before_the_frames_passes() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 4, 4, "refill test target").expect("an image");
    let format = TextureFormat::Rgba8Srgb;
    let mut stand_in = stand_in(&device, "refill test stand-in");
    let texture = Texture::placed(&device, 4, &stand_in, "refilled test texture");
    let mut texture = texture.expect("a texture");
    let mut frames = Frames::new(&device).expect("frames");
    let black = [0.0, 0.0, 0.0, 1.0];
    let mut refused = false;
    for i in 0..20 {
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        if i == 0 {
            write_stand_in(&mut frame, &mut stand_in);
            texture.refill(3, 1, format, false).expect("an image");
            stream_whole(&mut frame, &mut texture, &[127; 3 * 4]);
        }
        if refused {
            let shown = frame.finish_texture(&mut texture).expect("a finish");
            assert!(shown, "shown before the next frame's passes");
            assert_eq!(texture.size(), [3, 1]);
            frame.submit().expect("the frame's submission");
            break;
        }
        drop(frame.begin_pass(0, c"clear", black, None, None));
        match tries(|| frame.finish_texture(&mut texture)) {
            Ok(Ok(false)) => {}
            Ok(Ok(true)) => panic!("shown after the frame's pass"),
            Ok(Err(error)) => panic!("{error:#}"),
            Err(message) => {
                assert!(message.contains("before the frame's passes"), "{message}");
                assert_eq!(texture.size(), [0, 0], "a refused finish changes nothing");
                refused = true;
            }
        }
        frame.submit().expect("the frame's submission");
    }
    assert!(refused, "the copies never finished");
    drop((frames, texture, stand_in, image, device));

    drop(instance);
    check(&validation);
}

/// Forgets a frame, which the next frame catches, and a pass, which the frame's next call catches,
/// its submission among them, after which no frame begins again. Each reaches the JSON log as an
/// error with its code: the frame's as the frames drop, the pass's as the frame does. Nothing
/// forgotten reaches the GPU, so validation has nothing to say.
fn forgotten_frames_and_passes_are_caught(log: &Path) {
    let instance = instance();
    let validation = instance.validation();
    let black = [0.0, 0.0, 0.0, 1.0];
    // Each on a device of its own, since a device has one `Frames`, and a caught leak ends it.
    {
        let device =
            Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
        let mut image = ColorImage::new(&device, 4, 4, "forgotten frame target").expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.begin_pass(0, c"cleared", black, None, None));
        frame.read_image(0).expect("a read");
        mem::forget(frame);
        // What it read is never waited for, as the GPU never gets the frame.
        let read = frames.image_read(1).expect("no wait");
        assert!(read.is_none(), "a frame never submitted read nothing");
        let message = panics(|| {
            let _ = frames.begin([Target::Image(&mut image)]);
        });
        assert!(message.contains("frame 1 was forgotten"), "{message}");
        drop((frames, image, device));
    }
    {
        let device =
            Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
        let mut image = ColorImage::new(&device, 4, 4, "forgotten pass target").expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let mut frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        mem::forget(frame.begin_pass(0, c"forgotten", black, None, None));
        let message = panics(|| drop(frame.begin_pass(0, c"next", black, None, None)));
        assert!(message.contains("a pass was forgotten"), "{message}");
        let message = panics(|| {
            let _ = frame.submit();
        });
        assert!(message.contains("a pass was forgotten"), "{message}");
        // The frame's drop, as the submission unwound, failed it instead of submitting it.
        let error = frames
            .begin([Target::Image(&mut image)])
            .err()
            .expect("no frame begins after a failed one");
        assert_eq!(error.code(), "gpu.frame_failed");
        drop((frames, image, device));
    }

    drop(instance);
    check(&validation);
    let log = std::fs::read_to_string(log).expect("the JSON log");
    for code in ["gpu.frame_forgotten", "gpu.pass_forgotten"] {
        let logged = log
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|event| event["code"] == code && event["level"] == "ERROR");
        assert!(logged, "the JSON log has no {code} error");
    }
}

/// Opens two devices, and gives a frame of the one objects the other made: a target, textures to
/// write, stream, finish and hide, a stand-in, a buffer to stream and finish, a depth image, an
/// ID image to render into and to sample, and an upload's indices. Each is refused before anything
/// is recorded, so the frame goes on to submit.
fn another_devices_objects_are_refused() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let other = Device::new(&instance, None, None, None).expect("a second device");
    let mut image = ColorImage::new(&device, 4, 4, "identity test target").expect("an image");
    let mut others_image = ColorImage::new(&other, 4, 4, "the other's image").expect("an image");
    let others_depth = DepthImage::new(&other, 4, 4, "the other's depth").expect("a depth image");
    let mut others_ids = IdImage::new(&other, 4, 4, "the other's IDs").expect("an ID image");
    let format = TextureFormat::Rgba8Srgb;
    let others_texture = Texture::new(&other, 1, 1, format, false, "the other's texture");
    let mut others_texture = others_texture.expect("a texture");
    let mut others_buffer = GpuBuffer::new(&other, 12, "the other's buffer").expect("a buffer");
    let mut own_stand_in = stand_in(&device, "own stand-in");
    let placed = Texture::placed(&device, 6, &own_stand_in, "own placed texture");
    let mut placed = placed.expect("a texture");
    let mut frames = Frames::new(&device).expect("frames");
    let mut others_frames = Frames::new(&other).expect("frames");
    let refused = |message: String| {
        assert!(message.contains("made by another device"), "{message}");
    };
    let black = [0.0, 0.0, 0.0, 1.0];
    refused(panics(|| {
        let targets = [Target::Image(&mut image), Target::Image(&mut others_image)];
        let _ = frames.begin(targets);
    }));
    refused(panics(|| {
        let _ = Texture::placed(&device, 7, &others_texture, "stood in by the other's");
    }));
    let mut frame = frames
        .begin([Target::Image(&mut image)])
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    write_stand_in(&mut frame, &mut own_stand_in);
    refused(panics(|| {
        let _ = frame.stream_texture(&mut others_texture, &[255; 4], u64::MAX);
    }));
    refused(panics(|| {
        let _ = frame.write_texture(&mut others_texture, [0, 0], [1, 1], &[255; 4]);
    }));
    refused(panics(|| {
        let _ = frame.finish_texture(&mut others_texture);
    }));
    refused(panics(|| {
        frame.hide_texture(&mut others_texture, &own_stand_in)
    }));
    refused(panics(|| frame.hide_texture(&mut placed, &others_texture)));
    refused(panics(|| {
        let _ = frame.stream_buffer(&mut others_buffer, &[7; 12], u64::MAX);
    }));
    refused(panics(|| {
        let _ = frame.finish_buffer(&mut others_buffer);
    }));
    refused(panics(|| {
        drop(frame.begin_pass(0, c"depth", black, Some(&others_depth), None));
    }));
    refused(panics(|| {
        drop(frame.begin_pass(0, c"ids", black, None, Some(&mut others_ids)));
    }));
    refused(panics(|| {
        let _ = frame.begin_sampling_ids(&mut others_ids);
    }));
    let mut others_frame = others_frames
        .begin([Target::Image(&mut others_image)])
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let others_indices = others_frame.upload(&[0; 12]).expect("an upload");
    let mut pass = frame
        .begin_pass(0, c"identity", black, None, None)
        .expect("an offscreen image is always there");
    refused(panics(|| pass.draw_indexed(others_indices, 0, 3, 0, 1)));
    drop(pass);
    frame.submit().expect("the frame's submission");
    others_frame.submit().expect("the other frame's submission");

    drop((frames, others_frames, image, others_image, others_depth));
    drop((
        others_ids,
        others_texture,
        others_buffer,
        placed,
        own_stand_in,
    ));
    drop((device, other, instance));
    check(&validation);
}

/// Makes a texture in a placed slot, then asks for its slot again, and for a slot that isn't
/// placed: both are refused before anything is made, so that dropping the device leaks nothing,
/// which VMA would abort on. A stand-in without an image of its own is refused too.
fn refused_texture_slots_leak_nothing() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let stand_in = stand_in(&device, "slot test stand-in");
    let placed = Texture::placed(&device, 7, &stand_in, "placed test texture");
    let placed = placed.expect("a texture");
    let message = panics(|| {
        let _ = Texture::placed(&device, 7, &stand_in, "the same slot");
    });
    assert!(message.contains("texture slot 7 is in use"), "{message}");
    let message = panics(|| {
        let slot = PLACED_TEXTURES;
        let _ = Texture::placed(&device, slot, &stand_in, "a slot past the placed");
    });
    assert!(message.contains("isn't one of the"), "{message}");
    let message = panics(|| {
        let _ = Texture::placed(
            &device,
            8,
            &placed,
            "stood in by a slot that shows a stand-in",
        );
    });
    assert!(message.contains("shows a stand-in"), "{message}");
    // Dropped without an image, its slot comes free.
    drop(placed);
    let again = Texture::placed(&device, 7, &stand_in, "placed again");
    drop(again.expect("the slot came free"));

    drop((stand_in, device, instance));
    check(&validation);
}

/// Asks a pass for viewports no device draws, one wider than its limit and one reaching past its
/// bounds, and to draw the indices of a buffer no frame has taken: each is refused before it's
/// recorded, so the pass goes on.
fn passes_refuse_what_the_device_cant_draw() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let mut image = ColorImage::new(&device, 4, 4, "limits test target").expect("an image");
    let buffer = GpuBuffer::new(&device, 12, "unfilled test buffer").expect("a buffer");
    let mut frames = Frames::new(&device).expect("frames");
    let mut frame = frames
        .begin([Target::Image(&mut image)])
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let mut pass = frame
        .begin_pass(0, c"limits", [0.0, 0.0, 0.0, 1.0], None, None)
        .expect("an offscreen image is always there");
    let message = panics(|| pass.set_viewport([0, 0], [u32::MAX, 1]));
    assert!(message.contains("viewports of up to"), "{message}");
    let message = panics(|| pass.set_viewport([u32::MAX - 1, 0], [1, 1]));
    assert!(message.contains("reach at most"), "{message}");
    let message = panics(|| pass.draw_indexed(&buffer, 0, 3, 0, 1));
    assert!(message.contains("once a frame has filled it"), "{message}");
    drop(pass);
    frame.submit().expect("the frame's submission");

    drop((frames, buffer, image, device, instance));
    check(&validation);
}

/// Saves a device's pipeline cache, and loads it into other devices of the same GPU: whole, it
/// loads; changed or cut short, it doesn't, and no driver is given it, so validation has nothing
/// to say. A device loads one cache. Data of another GPU is refused as the header's unit tests
/// show; drivers fill caches as pipelines are made, which the renderer's GPU tests check.
fn pipeline_caches_load_what_this_gpu_saved_whole() {
    let instance = instance();
    let validation = instance.validation();
    let device =
        || Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let first = device();
    assert_eq!(first.pipeline_cache().expect("no cache"), None);
    assert!(!first.load_pipeline_cache(&[]).expect("an empty cache"));
    let saved = first.pipeline_cache().expect("the cache's data");
    let saved = saved.expect("a cache");
    let message = panics(|| {
        let _ = first.load_pipeline_cache(&saved);
    });
    assert!(message.contains("cache once"), "{message}");
    drop(first);

    let loads = |data: &[u8]| device().load_pipeline_cache(data).expect("a cache");
    assert!(loads(&saved), "whole");
    let mut changed = saved.clone();
    *changed.last_mut().expect("data") ^= 1;
    assert!(!loads(&changed), "changed");
    assert!(!loads(&saved[..saved.len() - 1]), "cut short");

    drop(instance);
    check(&validation);
}

/// Asks a device to capture its work, which only RenderDoc's layer does: the tests turn it off
/// with the other implicit layers, so the device refuses.
fn captures_are_refused_without_renderdoc() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let error = device
        .start_capture()
        .err()
        .expect("no capture without RenderDoc");
    assert_eq!(error.code(), "gpu.no_renderdoc", "{error:#}");

    drop((device, instance));
    check(&validation);
}

/// Breaks a Vulkan rule on purpose, and checks that validation's error reaches the JSON log
/// with its VUID and the debug name of the object involved.
fn validation_errors_reach_the_json_log(log: &Path) {
    const NAME: &str = "deliberately misviewed image";
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let image = ColorImage::new(&device, 4, 4, NAME).expect("an image");
    let info = vk::ImageViewCreateInfo::default()
        .image(image.raw())
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R32_SFLOAT)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    // SAFETY: deliberately invalid: the view's format differs from the image's, which wasn't
    // made with a mutable format. Validation reports it before the driver sees it; both formats
    // are 32 bits a texel, so the driver makes the view regardless, and it's destroyed unused.
    unsafe {
        let view = device
            .raw()
            .create_image_view(&info, None)
            .expect("an image view");
        device.raw().destroy_image_view(view, None);
    }
    drop((image, device, instance));
    assert!(validation.messages() >= 1, "validation counted nothing");

    let log = std::fs::read_to_string(log).expect("the JSON log");
    let event = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event["code"] == "gpu.validation"
                && event["objects"]
                    .as_str()
                    .is_some_and(|objects| objects.contains(NAME))
        })
        .unwrap_or_else(|| panic!("the JSON log has no validation error naming {NAME:?}"));
    assert_eq!(event["level"], "ERROR");
    let vuid = event["vuid"].as_str().unwrap_or_default();
    assert!(
        vuid.starts_with("VUID-VkImageViewCreateInfo-"),
        "the VUID is {vuid:?}"
    );
}
