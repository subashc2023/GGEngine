//! GPU tests, on whatever GPU the loader offers: lavapipe in Linux CI.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another. An argument
//! that isn't a flag picks those whose names contain it, as libtest does.

// For setting the environment, and for breaking a Vulkan rule on purpose: see the SAFETY comments.
#![allow(unsafe_code)]

use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;

use ash::vk;
use gg_core::label;
use gg_gpu::harness::{self, Tests, check};
use gg_gpu::{
    Arenas, Blend, COMPARISON_SAMPLER, ColorImage, ComputePipeline, Device, Format, Frame, Frames,
    GpuBuffer, Graph, GraphicsDesc, Instance, InstanceDesc, MAX_READS, PLACED_TEXTURES, Pass,
    PassId, Pipeline, Sampler, StorageImage, Target, Texture, TextureFormat, UploadPath, mip_sizes,
};

/// Runs each test through `tests` by its name, with the arguments written after it, if any.
macro_rules! run {
    ($tests:ident $(, $test:ident $(($($arg:expr),*))?)+ $(,)?) => {
        $($tests.run(stringify!($test), || $test($($($arg),*)?));)+
    };
}

fn main() -> ExitCode {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    // SAFETY: no other thread exists yet.
    let Some(log) = (unsafe { harness::begin("gpu-tests", &dir) }) else {
        return ExitCode::SUCCESS;
    };

    let mut tests = Tests::from_args();
    run!(
        tests,
        clear_reads_back,
        arenas_hold_more_than_a_frame_took,
        sampled_images_keep_their_layout,
        compute_passes_write_cubes_that_passes_sample,
        compute_passes_write_buffers_that_draws_take_commands_from,
        graph_buffers_hold_zeros_where_nothing_wrote,
        compute_queue_passes_run_beside_the_graphics_queues,
        passes_over_keep_what_their_target_holds,
        images_read_back_once_their_frame_has_finished,
        dropped_images_free_their_slots,
        depth_clears_in_every_pass,
        shadow_maps_draw_depth_alone_and_sample_by_comparison,
        ids_read_back_two_frames_later,
        the_graphs_own_images_share_memory_where_their_uses_dont_overlap,
        depth_and_colors_take_turns_in_memory,
        frames_time_their_passes_on_the_gpu,
        streamed_textures_can_be_written_over_on_every_path,
        bc7_textures_stream_and_decode_on_every_path,
        textures_refill_between_rgba8_and_bc7_on_every_path,
        textures_and_buffers_stream_by_parts_on_every_path,
        textures_come_to_show_images_before_the_frames_passes,
        stand_ins_live_while_slots_show_them,
        images_past_the_devices_most_are_refused,
        forgotten_frames_and_passes_are_caught(&log),
        another_devices_objects_are_refused,
        frames_that_began_none_leave_the_device_to_another,
        refused_texture_slots_leak_nothing,
        passes_refuse_what_the_device_cant_draw,
        pipeline_caches_load_what_this_gpu_saved_whole,
        captures_are_refused_without_renderdoc,
        validation_errors_reach_the_json_log(&log),
    );
    tests.finish()
}

/// What `tried` returns, or its panic's message. The panic hook is set aside meanwhile, so the
/// panic isn't logged as a failing test's is.
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
    harness::assert_validating(&instance.validation());
    instance
}

/// A device of the best GPU, as it draws best.
fn device(instance: &Arc<Instance>) -> Arc<Device> {
    Device::new(instance, None, None, None, None).expect("a GPU that supports Vulkan 1.4")
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
/// share one. Both endpoints are `color`, so every pixel is, whatever weight its index picks.
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

/// The BC7 pixels of a `size` texture, each block solid in a color of its own, and a probe per
/// block: at its middle, or its level's last row or column if that ends first. A block in the
/// wrong place or level shows.
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

/// The RGBA8 pixels of a `size` texture, each mip level a color of its own, and a probe per level.
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
    spirv(
        "sample",
        &[("vertex_main", "vertex"), ("fragment_main", "fragment")],
    )
}

/// The SPIR-V of `tests/shaders/<shader>.slang`'s `entries`, each an entry point and its stage,
/// compiled as [`sampling_spirv`] says.
fn spirv(shader: &str, entries: &[(&str, &str)]) -> Vec<u8> {
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
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/shaders/{shader}.slang"));
    let spirv = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{shader}.spv"));
    let mut command = Command::new(&slangc);
    command
        .arg(&source)
        .args(["-target", "spirv", "-fvk-use-entrypoint-name"])
        .args(["-warnings-as-errors", "38006"]);
    for (entry, stage) in entries {
        command.args(["-entry", entry, "-stage", stage]);
    }
    let output = command
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

fn sampling_pipeline(device: &Arc<Device>, spirv: &[u8]) -> Pipeline {
    let desc = GraphicsDesc {
        name: "sample probes",
        spirv,
        vertex: c"vertex_main",
        mesh: None,
        fragment: Some(c"fragment_main"),
        format: ColorImage::FORMAT,
        blend: Blend::Opaque,
        depth_write: false,
        ids_write: false,
        depth_bias: 0.0,
        depth_clamp: false,
    };
    Pipeline::graphics(device, &desc).expect("a pipeline")
}

/// Records `pass`, which clears its target, an image a pixel across per probe. The GPU samples
/// `texture` at each probe into its own pixel, by three vertices, or by the first three of
/// `indices`, a buffer frames have taken, if given.
fn draw_probes(
    frame: &mut Frame<'_>,
    pass: PassId,
    pipeline: &Pipeline,
    texture: &Texture,
    probes: &[Probe],
    indices: Option<&GpuBuffer>,
) {
    let mut pass = frame
        .render(pass)
        .expect("an offscreen image is always there");
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

/// Checks `image`'s pixels against `probes`, within the 1 that sRGB's encoding may round by. `what`
/// names the texture.
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

/// A graph of a pass for each of the frame's first `targets` targets, in order, that clears it to
/// `color` and draws whatever the frame draws there.
fn clearing(targets: usize, color: [f32; 4]) -> (Graph, Vec<PassId>) {
    let mut graph = Graph::default();
    let passes = (0..targets)
        .map(|target| {
            let image = graph.target(target);
            graph.render(label!("clear")).clear(image, color).add()
        })
        .collect();
    (graph, passes)
}

/// Clears two offscreen images in the same frames, each to a color of its own, over more frames
/// than are in flight, then reads the colors back.
fn clear_reads_back() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
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
    let mut graph = Graph::default();
    let passes: Vec<PassId> = (0..)
        .zip(srgb)
        .map(|(target, srgb)| {
            let color = [linear(srgb[0]), linear(srgb[1]), linear(srgb[2]), 1.0];
            let image = graph.target(target);
            graph.render(label!("clear")).clear(image, color).add()
        })
        .collect();
    for _ in 0..3 {
        let mut frame = frames
            .begin(images.iter_mut().map(Target::Image), &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        // In the first two frames, the arena has no buffer yet.
        frame.upload(&[]).expect("an upload of nothing");
        for &pass in &passes {
            // A pass that draws nothing clears its target.
            drop(frame.render(pass));
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

/// The arenas' peak is the most a frame's uploads took, each from a multiple of 16 bytes, and a
/// frame taking less leaves it. They hold at least that, in buffers of 1 MiB at least, kept.
fn arenas_hold_more_than_a_frame_took() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 8, 8, "arena test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    assert_eq!(frames.arenas(), Arenas::default());
    let (graph, passes) = clearing(1, [0.0; 4]);
    for uploads in [&[100, 7][..], &[20]] {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for &len in uploads {
            frame.upload(&vec![1; len]).expect("an upload");
        }
        drop(frame.render(passes[0]));
        frame.submit().expect("the frame's submission");
    }
    let arenas = frames.arenas();
    assert_eq!(arenas.peak, 112 + 16);
    assert!(arenas.held >= 2 << 20, "{arenas:?}");
    drop((frames, image, device, instance));
    check(&validation);
}

/// Clears an image, then samples it from a pass into another, in more frames than are in flight,
/// and reads it back. The read-back's barrier starts from the layout the frames recorded, so
/// validation reports sampling that didn't change the image's layout, or didn't record the change.
fn sampled_images_keep_their_layout() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 16, 16, "sampled test target").expect("an image");
    let mut drawn = ColorImage::new(&device, 16, 16, "sampling test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let (sampled, into) = (graph.target(0), graph.target(1));
    let red = [1.0, 0.0, 0.0, 1.0];
    let clear = graph.render(label!("clear")).clear(sampled, red).add();
    let sample = (graph.render(label!("sample")))
        .clear(into, [0.0; 4])
        .sample(sampled)
        .add();
    for _ in 0..3 {
        let targets = [Target::Image(&mut image), Target::Image(&mut drawn)];
        let mut frame = frames
            .begin(targets, &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.render(clear));
        let pass = frame
            .render(sample)
            .expect("an offscreen image is always there");
        // The same slot whichever frame samples it.
        assert_eq!(pass.sampled(sampled), pass.sampled(sampled));
        drop(pass);
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    for pixel in pixels.as_chunks::<4>().0 {
        assert_eq!(*pixel, [255, 0, 0, 255]);
    }

    drop((frames, image, drawn, device, instance));
    check(&validation);
}

/// A compute pass writes each face of a cube's first level, another halves it into the second, and
/// a render pass samples both levels of each face by direction. The next frame samples them again
/// without declaring the cube, as a frame whose last use sampled it may. Synchronization validation
/// sees each use wait for the write before it. A storage image's slot to sample through comes only
/// once a frame has left it sampled, and an offscreen image written as storage is refused.
fn compute_passes_write_cubes_that_passes_sample() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = spirv(
        "cube",
        &[
            ("fill_main", "compute"),
            ("halve_main", "compute"),
            ("vertex_main", "vertex"),
            ("fragment_main", "fragment"),
        ],
    );
    let fill = ComputePipeline::new(&device, "fill cube", &spirv, c"fill_main").expect("fill");
    let halve = ComputePipeline::new(&device, "halve cube", &spirv, c"halve_main").expect("halve");
    let draw = Pipeline::graphics(
        &device,
        &GraphicsDesc {
            name: "sample cube",
            spirv: &spirv,
            vertex: c"vertex_main",
            mesh: None,
            fragment: Some(c"fragment_main"),
            format: ColorImage::FORMAT,
            blend: Blend::Opaque,
            depth_write: false,
            ids_write: false,
            depth_bias: 0.0,
            depth_clamp: false,
        },
    )
    .expect("a pipeline");
    let mut cube = StorageImage::new(&device, [4, 4], 2, true, "test cube").expect("a cube");
    assert_eq!(cube.sampled(), None, "nothing has written it");
    // A pixel for each level of each face, in Vulkan's order of faces.
    let mut image = ColorImage::new(&device, 12, 1, "cube samples").expect("an image");
    let directions: [[f32; 3]; 6] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    // As `cube.slang` takes them: the cube's slot, the sampler's, the level and the direction.
    let sample = |pass: &mut Pass<'_, '_>, index: u32| {
        pass.set_pipeline(&draw);
        let probes = (0..2u32)
            .flat_map(|level| (directions.iter()).map(move |&direction| (level, direction)));
        for (x, (level, [dx, dy, dz])) in (0..).zip(probes) {
            pass.set_viewport([x, 0], [1, 1]);
            let level = (level as f32).to_bits();
            let words = [
                index,
                Sampler::NearestClamp.index(),
                0,
                level,
                dx.to_bits(),
                dy.to_bits(),
                dz.to_bits(),
                0,
            ];
            pass.push_constants(&words.map(u32::to_ne_bytes).concat());
            pass.draw(3, 1);
        }
    };
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let (color, written) = (graph.target(0), graph.target(1));
    let filled = graph.compute(label!("fill")).write(written).add();
    let halved = graph.compute(label!("halve")).write(written).add();
    let drawn = (graph.render(label!("draw")))
        .clear(color, [0.0; 4])
        .sample(written)
        .add();
    let targets = [Target::Image(&mut image), Target::Storage(&mut cube)];
    let mut frame = frames
        .begin(targets, &graph)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    // As `cube.slang` takes them: the level's slot, the next's when halving, and its side.
    let mut pass = frame.compute(filled);
    pass.set_pipeline(&fill);
    let words = [pass.storage(written, 0), 0, 4, 0, 0, 0, 0, 0];
    pass.push_constants(&words.map(u32::to_ne_bytes).concat());
    pass.dispatch([1, 1, 6]);
    drop(pass);
    let mut pass = frame.compute(halved);
    pass.set_pipeline(&halve);
    let (above, below) = (pass.storage(written, 0), pass.storage(written, 1));
    let words = [above, below, 2, 0, 0, 0, 0, 0];
    pass.push_constants(&words.map(u32::to_ne_bytes).concat());
    pass.dispatch([1, 1, 6]);
    drop(pass);
    let mut pass = frame
        .render(drawn)
        .expect("an offscreen image is always there");
    let index = pass.sampled(written);
    sample(&mut pass, index);
    drop(pass);
    frame.submit().expect("the frame's submission");
    assert_eq!(cube.sampled(), Some(index), "sampled last");
    // Each face's color in its first level, then halved in its second, sRGB-encoded.
    let expected: Vec<[u8; 4]> = (0..2)
        .flat_map(|level| {
            (0..6u32).map(move |face| {
                let on = if level == 0 { 255 } else { 188 };
                let bit = |shift: u32| if face >> shift & 1 == 1 { on } else { 0 };
                [bit(0), bit(1), bit(2), 255]
            })
        })
        .collect();
    let close = |pixels: &[u8]| {
        (pixels.as_chunks::<4>().0.iter().zip(&expected)).all(|(got, want)| {
            got.iter()
                .zip(want)
                .all(|(&got, &want)| got.abs_diff(want) <= 1)
        })
    };
    let pixels = image.read_back().expect("the image's pixels");
    assert!(close(&pixels), "{pixels:?}, not {expected:?}");

    let mut graph = Graph::default();
    let color = graph.target(0);
    let drawn = graph.render(label!("draw")).clear(color, [0.0; 4]).add();
    let mut frame = frames
        .begin([Target::Image(&mut image)], &graph)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let mut pass = frame
        .render(drawn)
        .expect("an offscreen image is always there");
    sample(&mut pass, index);
    drop(pass);
    frame.submit().expect("the frame's submission");
    let pixels = image.read_back().expect("the image's pixels");
    assert!(close(&pixels), "{pixels:?}, not {expected:?}");

    let mut graph = Graph::default();
    let color = graph.target(0);
    let _ = graph.compute(label!("misused")).write(color).add();
    let refused = panics(|| {
        let _ = frames.begin([Target::Image(&mut image)], &graph);
    });
    assert!(refused.contains("no pass writes as storage"), "{refused}");

    drop((frames, image, cube, fill, halve, draw, device, instance));
    check(&validation);
}

/// A compute pass writes two indirect draw commands into a graph buffer, the instances' colors into
/// another, and two counts into a third, the second by an atomic add. A render pass draws by the
/// commands, the first two instances from the second, the other none, each instance over its own
/// pixel in the color it reads. The counts and colors come back by their buffers' names as the
/// frame two later begins. Three frames, so synchronization validation sees each frame's writes
/// wait for the frame before's reads of the commands and the copies.
fn compute_passes_write_buffers_that_draws_take_commands_from() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = spirv(
        "indirect",
        &[
            ("write_main", "compute"),
            ("vertex_main", "vertex"),
            ("fragment_main", "fragment"),
        ],
    );
    let write = ComputePipeline::new(&device, "write commands", &spirv, c"write_main").expect("a");
    let draw = Pipeline::graphics(
        &device,
        &GraphicsDesc {
            name: "draw by commands",
            spirv: &spirv,
            vertex: c"vertex_main",
            mesh: None,
            fragment: Some(c"fragment_main"),
            format: ColorImage::FORMAT,
            blend: Blend::Opaque,
            depth_write: false,
            ids_write: false,
            depth_bias: 0.0,
            depth_clamp: false,
        },
    )
    .expect("a pipeline");
    let mut image = ColorImage::new(&device, 2, 1, "indirect draws").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let color = graph.target(0);
    let (commands, colors, counts) = (
        graph.buffer("test commands", 40),
        graph.buffer("test colors", 48),
        graph.buffer("test counts", 8),
    );
    let written = (graph.compute(label!("write")))
        .write_buffer(commands)
        .write_buffer(counts)
        .write_buffer(colors)
        .add();
    let read = graph.read_back(label!("read counts"), counts);
    let read_colors = graph.read_back(label!("read colors"), colors);
    let drawn = (graph.render(label!("draw")))
        .clear(color, [0.0; 4])
        .indirect(commands)
        .read_buffer(colors)
        .add();
    let mut reads = Vec::new();
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        let (number, counted) = frame.buffer_read("test counts");
        let colored = frame.buffer_read("test colors").1.to_vec();
        assert!(
            frame.buffer_read("test commands").1.is_empty(),
            "never read back"
        );
        reads.push((number, counted.to_vec(), colored));
        let indices = frame
            .upload(&[0u32, 1, 2].map(u32::to_ne_bytes).concat())
            .expect("indices");
        // Red, then green, the colors of the instances drawn.
        let shown = [1.0f32, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let source = frame
            .upload(&shown.map(f32::to_ne_bytes).concat())
            .expect("colors")
            .address();
        let mut pass = frame.compute(written);
        pass.set_pipeline(&write);
        let [to_commands, to_counts, to_colors] =
            [commands, counts, colors].map(|b| pass.address(b));
        let addresses = [to_commands, to_counts, to_colors, source];
        pass.push_constants(&addresses.map(u64::to_ne_bytes).concat());
        pass.dispatch([1, 1, 1]);
        drop(pass);
        frame.read_back(read).expect("a read back");
        frame.read_back(read_colors).expect("a read back");
        let mut pass = frame
            .render(drawn)
            .expect("an offscreen image is always there");
        pass.set_pipeline(&draw);
        let addresses = [0, 0, pass.address(colors)];
        pass.push_constants(&addresses.map(u64::to_ne_bytes).concat());
        pass.draw_indexed_indirect(indices, commands, 0);
        pass.draw_indexed_indirect(indices, commands, 20);
        drop(pass);
        frame.submit().expect("the frame's submission");
    }
    let counted = [2u32, 7].map(u32::to_ne_bytes).concat();
    // Blue, which no command draws, then red and green.
    let colored = [
        0.0f32, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
    ]
    .map(f32::to_ne_bytes)
    .concat();
    let none = || (0, Vec::new(), Vec::new());
    assert_eq!(reads, [none(), none(), (1, counted, colored)]);
    let pixels = image.read_back().expect("the image's pixels");
    assert_eq!(pixels, [255, 0, 0, 255, 0, 255, 0, 255], "red, then green");

    // What no pass wrote isn't read, nor read back, and a command past its buffer isn't drawn.
    let refused = panics(|| {
        let mut graph = Graph::default();
        let unwritten = graph.buffer("unwritten", 4);
        let _ = graph.compute(label!("read")).read_buffer(unwritten).add();
    });
    assert!(
        refused.contains("before any pass of the frame writes it"),
        "{refused}"
    );
    let refused = panics(|| {
        let mut graph = Graph::default();
        let large = graph.buffer("large", gg_gpu::MAX_BUFFER_READ + 4);
        let _ = graph.compute(label!("write")).write_buffer(large).add();
        let _ = graph.read_back(label!("read"), large);
    });
    assert!(refused.contains("reads back"), "{refused}");
    let refused = panics(|| {
        let mut graph = Graph::default();
        let small = graph.buffer("small", 4);
        let _ = graph.compute(label!("write")).write_buffer(small).add();
        for _ in 0..=gg_gpu::MAX_BUFFER_READS {
            let _ = graph.read_back(label!("read"), small);
        }
    });
    assert!(refused.contains("at most"), "{refused}");
    let refused = tries(|| {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        let indices = frame
            .upload(&[0u32, 1, 2].map(u32::to_ne_bytes).concat())
            .expect("indices");
        drop(frame.compute(written));
        frame.read_back(read).expect("a read back");
        frame.read_back(read_colors).expect("a read back");
        let mut pass = frame
            .render(drawn)
            .expect("an offscreen image is always there");
        pass.set_pipeline(&draw);
        pass.draw_indexed_indirect(indices, commands, 24);
    });
    let refused = refused.expect_err("a command past its buffer's end");
    assert!(refused.contains("within its buffer"), "{refused}");

    drop((frames, image, write, draw, device, instance));
    check(&validation);
}

/// A graph buffer holds zeros where no frame has written, even in memory that buffers written over
/// and dropped held: the frames fill each they make before its first use.
fn graph_buffers_hold_zeros_where_nothing_wrote() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = spirv("indirect", &[("write_main", "compute")]);
    let write = ComputePipeline::new(&device, "write commands", &spirv, c"write_main").expect("a");
    let mut image = ColorImage::new(&device, 1, 1, "zeros").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    // Each the size of the buffer read back.
    let mut dirtied = Graph::default();
    let dirt = ["test dirt 0", "test dirt 1", "test dirt 2"].map(|name| dirtied.buffer(name, 48));
    let dirty = (dirtied.compute(label!("dirty")))
        .write_buffer(dirt[0])
        .write_buffer(dirt[1])
        .write_buffer(dirt[2])
        .add();
    let (idle, cleared) = clearing(1, [0.0; 4]);
    let mut fresh = Graph::default();
    let zeros = fresh.buffer("test zeros", 48);
    let unwritten = (fresh.compute(label!("write nothing")))
        .write_buffer(zeros)
        .add();
    let read = fresh.read_back(label!("read zeros"), zeros);
    let mut read_last = Vec::new();
    // Dirt, then frames enough for its buffers to be freed, then three for the read to come back.
    for i in 0..8 {
        let graph = match i {
            0 => &dirtied,
            1..5 => &idle,
            _ => &fresh,
        };
        let mut frame = frames
            .begin([Target::Image(&mut image)], graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        read_last = frame.buffer_read("test zeros").1.to_vec();
        match i {
            0 => {
                let source = frame
                    .upload(&[1.0f32; 8].map(f32::to_ne_bytes).concat())
                    .expect("colors")
                    .address();
                let mut pass = frame.compute(dirty);
                pass.set_pipeline(&write);
                let [commands, counts, colors] = dirt.map(|buffer| pass.address(buffer));
                let addresses = [commands, counts, colors, source];
                pass.push_constants(&addresses.map(u64::to_ne_bytes).concat());
                pass.dispatch([1, 1, 1]);
            }
            1..5 => drop(frame.render(cleared[0])),
            _ => {
                drop(frame.compute(unwritten));
                frame.read_back(read).expect("a read back");
            }
        }
        frame.submit().expect("the frame's submission");
    }
    assert_eq!(read_last, [0; 48]);

    drop((frames, image, write, device, instance));
    check(&validation);
}

/// The compute pass of [`compute_passes_write_buffers_that_draws_take_commands_from`] on the
/// compute queue: apart from graphics on the RTX 4090 and AMD's GPUs, the graphics queue on
/// lavapipe.
/// Beside a graphics pass that clears the image, it writes the commands, and the colors it reads
/// from a buffer each frame writes before its passes, two colors that swap each frame. Read-backs
/// and draws wait for it, and the next frame's pass waits for them before writing over it, as
/// synchronization validation sees across the queues. Four frames, so two begin after the first
/// with what another frame left in each graph buffer.
fn compute_queue_passes_run_beside_the_graphics_queues() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = spirv(
        "indirect",
        &[
            ("write_main", "compute"),
            ("vertex_main", "vertex"),
            ("fragment_main", "fragment"),
        ],
    );
    let write = ComputePipeline::new(&device, "write commands", &spirv, c"write_main").expect("a");
    let draw = Pipeline::graphics(
        &device,
        &GraphicsDesc {
            name: "draw by commands",
            spirv: &spirv,
            vertex: c"vertex_main",
            mesh: None,
            fragment: Some(c"fragment_main"),
            format: ColorImage::FORMAT,
            blend: Blend::Opaque,
            depth_write: false,
            ids_write: false,
            depth_bias: 0.0,
            depth_clamp: false,
        },
    )
    .expect("a pipeline");
    let mut source = GpuBuffer::new(&device, 32, "test source").expect("a buffer");
    let mut image = ColorImage::new(&device, 2, 1, "compute queue draws").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let color = graph.target(0);
    let (commands, colors, counts) = (
        graph.buffer("test commands", 40),
        graph.buffer("test colors", 48),
        graph.buffer("test counts", 8),
    );
    let cleared = graph.render(label!("clear")).clear(color, [0.0; 4]).add();
    let written = (graph.compute(label!("write")).on_compute_queue())
        .write_buffer(commands)
        .write_buffer(counts)
        .write_buffer(colors)
        .add();
    let read = graph.read_back(label!("read counts"), counts);
    let drawn = (graph.render(label!("draw")).keep(color))
        .indirect(commands)
        .read_buffer(colors)
        .add();
    let (red, green) = ([1.0f32, 0.0, 0.0, 1.0], [0.0f32, 1.0, 0.0, 1.0]);
    let mut reads = Vec::new();
    for number in 0..4 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        reads.push(frame.buffer_read("test counts").0);
        let shown = if number % 2 == 0 {
            [red, green]
        } else {
            [green, red]
        };
        let bytes: Vec<u8> = (shown.as_flattened().iter())
            .flat_map(|channel| channel.to_ne_bytes())
            .collect();
        frame
            .write_buffer(&mut source, 0, &bytes)
            .expect("a buffer written");
        let indices = frame
            .upload(&[0u32, 1, 2].map(u32::to_ne_bytes).concat())
            .expect("indices");
        drop(
            frame
                .render(cleared)
                .expect("an offscreen image is always there"),
        );
        let mut pass = frame.compute(written);
        pass.set_pipeline(&write);
        let [to_commands, to_counts, to_colors] =
            [commands, counts, colors].map(|b| pass.address(b));
        let addresses = [to_commands, to_counts, to_colors, source.address()];
        pass.push_constants(&addresses.map(u64::to_ne_bytes).concat());
        pass.dispatch([1, 1, 1]);
        drop(pass);
        frame.read_back(read).expect("a read back");
        let mut pass = frame
            .render(drawn)
            .expect("an offscreen image is always there");
        pass.set_pipeline(&draw);
        let addresses = [0, 0, pass.address(colors)];
        pass.push_constants(&addresses.map(u64::to_ne_bytes).concat());
        pass.draw_indexed_indirect(indices, commands, 0);
        pass.draw_indexed_indirect(indices, commands, 20);
        drop(pass);
        frame.submit().expect("the frame's submission");
    }
    assert_eq!(reads, [0, 0, 1, 2], "the counts of the frame two before");
    let pixels = image.read_back().expect("the image's pixels");
    assert_eq!(pixels, [0, 255, 0, 255, 255, 0, 0, 255], "green, then red");

    // A pass on the compute queue uses no images, and what a frame writes comes before its passes.
    let refused = panics(|| {
        let mut graph = Graph::default();
        let target = graph.target(0);
        let pass = graph.compute(label!("sample")).on_compute_queue();
        let _ = pass.sample(target).add();
    });
    assert!(refused.contains("use buffers alone"), "{refused}");
    let refused = panics(|| {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(
            frame
                .render(cleared)
                .expect("an offscreen image is always there"),
        );
        let _ = frame.write_buffer(&mut source, 0, &[0; 16]);
    });
    assert!(refused.contains("before its passes"), "{refused}");

    drop((frames, image, source, write, draw, device, instance));
    check(&validation);
}

/// Clears an image, passes over it drawing nothing, then samples it into another, over more frames
/// than are in flight; it reads back as the clear left it. Synchronization validation sees the
/// second pass's load wait for the first's writes, and the next frame's clear wait for the
/// sampling. A pass over the image before anything drew into it is refused before it's recorded.
fn passes_over_keep_what_their_target_holds() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 16, 16, "kept test target").expect("an image");
    let mut drawn = ColorImage::new(&device, 16, 16, "kept test sampler").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut too_soon = Graph::default();
    let kept = too_soon.target(0);
    let over = too_soon.render(label!("too soon")).keep(kept).add();
    let mut frame = frames
        .begin([Target::Image(&mut image)], &too_soon)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let message = panics(|| drop(frame.render(over)));
    assert!(message.contains("no pass has drawn into it"), "{message}");
    drop(frame);
    let mut graph = Graph::default();
    let (kept, into) = (graph.target(0), graph.target(1));
    let blue = [0.0, 0.0, 1.0, 1.0];
    let clear = graph.render(label!("clear")).clear(kept, blue).add();
    let over = graph.render(label!("kept")).keep(kept).add();
    let sample = (graph.render(label!("sample")))
        .clear(into, [0.0; 4])
        .sample(kept)
        .add();
    for _ in 0..3 {
        let targets = [Target::Image(&mut image), Target::Image(&mut drawn)];
        let mut frame = frames
            .begin(targets, &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for pass in [clear, over, sample] {
            drop(frame.render(pass));
        }
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    for pixel in pixels.as_chunks::<4>().0 {
        assert_eq!(*pixel, [0, 0, 255, 255]);
    }

    drop((frames, image, drawn, device, instance));
    check(&validation);
}

/// Clears images a color per frame and reads them back, finding each frame's pixels once it has
/// finished, until the frame two later reads into its slot's buffer. A frame reads a larger image
/// than its slot read before, whole, and samples it after. Synchronization validation checks the
/// copies' barriers and the sampling after.
fn images_read_back_once_their_frame_has_finished() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut images = [
        ColorImage::new(&device, 8, 4, "small read target").expect("an image"),
        ColorImage::new(&device, 16, 16, "large read target").expect("an image"),
    ];
    let mut drawn = ColorImage::new(&device, 4, 4, "read sampler").expect("an image");
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
    // A graph for each color, whose frames read back what it clears, and sample it after.
    let reading = |color| {
        let mut graph = Graph::default();
        let (read, into) = (graph.target(0), graph.target(1));
        let clear = graph.render(label!("clear")).clear(read, color).add();
        let read_back = graph.read_image(label!("read"), read);
        let sample = (graph.render(label!("sample")))
            .clear(into, [0.0; 4])
            .sample(read)
            .add();
        (graph, vec![clear, read_back, sample])
    };
    let graphs = [
        reading(red),
        reading(green),
        reading(blue),
        clearing(1, red),
    ];
    // The image, small or large, of each graph's frame, which reads it back or doesn't.
    for (image, (graph, passes)) in [0, 0, 1, 0].into_iter().zip(&graphs) {
        let targets = [Target::Image(&mut images[image]), Target::Image(&mut drawn)];
        let mut frame = frames
            .begin(targets, graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.render(passes[0]));
        if let &[_, read, sample] = &passes[..] {
            frame.read_image(read).expect("a read");
            drop(frame.render(sample));
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

    drop((frames, images, drawn, device, instance));
    check(&validation);
}

/// Makes and drops, one at a time, more images than the bindless sets' 4,096 slots past the placed
/// ones, which works only if each gives its slot back.
fn dropped_images_free_their_slots() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    for i in 0..5_000 {
        let image = ColorImage::new(&device, 4, 4, "dropped test image");
        drop(image.unwrap_or_else(|error| panic!("image {i}: {error:#}")));
    }

    drop((device, instance));
    check(&validation);
}

/// Clears an image and the graph's own depth in two passes a frame, over more frames than are in
/// flight, so every pass but the first follows another's use of the depth, in its submission or the
/// last. Synchronization validation reports a barrier that doesn't wait for that use, and core
/// validation a pass that leaves the depth in the wrong layout.
fn depth_clears_in_every_pass() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 16, 16, "depth test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let (target, depth) = (graph.target(0), graph.depth("depth test depth", [16, 16]));
    let black = [0.0, 0.0, 0.0, 1.0];
    let passes: Vec<PassId> = (0..2)
        .map(|_| {
            graph
                .render(label!("clear"))
                .clear(target, black)
                .depth(depth)
                .add()
        })
        .collect();
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for &pass in &passes {
            drop(frame.render(pass));
        }
        frame.submit().expect("the frame's submission");
    }

    drop((frames, image, device, instance));
    check(&validation);
}

/// Draws three triangles' depth alone into a 16x16 shadow map, each over its own viewport, and
/// samples the map by comparison in a later pass, over three frames. The left half is flat at 0.5;
/// the top right is at 1.5, past the near plane, which depth clamp holds at 1; the bottom right
/// slopes 0.0375 a texel, which a slope-scaled depth bias of 2 takes back by twice that.
///
/// A depth as near as the map's or nearer reads 1, a farther one 0. So half the bias nearer than
/// the slope reads 1, as it wouldn't without the bias or with its sign wrong. Synchronization
/// validation sees the sampling wait for the depth, and the next frame's depth wait for it. A
/// pipeline without a fragment shader draws depth alone; one clamps or biases depth only where its
/// targets have it, and draws IDs only beside a color target.
fn shadow_maps_draw_depth_alone_and_sample_by_comparison() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = spirv(
        "shadow",
        &[
            ("depth_main", "vertex"),
            ("vertex_main", "vertex"),
            ("fragment_main", "fragment"),
        ],
    );
    let desc = |format, fragment, depth_bias, depth_clamp| GraphicsDesc {
        name: "shadow",
        spirv: &spirv,
        vertex: c"depth_main",
        mesh: None,
        fragment,
        format,
        blend: Blend::Opaque,
        depth_write: true,
        ids_write: false,
        depth_bias,
        depth_clamp,
    };
    let depth = Pipeline::graphics(&device, &desc(Format::DEPTH, None, 2.0, true))
        .expect("a pipeline of depth alone");
    let compare = Pipeline::graphics(
        &device,
        &GraphicsDesc {
            vertex: c"vertex_main",
            mesh: None,
            depth_write: false,
            ..desc(ColorImage::FORMAT, Some(c"fragment_main"), 0.0, false)
        },
    )
    .expect("a pipeline");
    let colorless = panics(|| {
        let undrawn = GraphicsDesc {
            depth_write: false,
            ..desc(ColorImage::FORMAT, None, 0.0, false)
        };
        drop(Pipeline::graphics(&device, &undrawn));
    });
    assert!(
        colorless.contains("without a fragment shader"),
        "{colorless}"
    );
    let flat = panics(|| {
        let clamped = GraphicsDesc {
            depth_write: false,
            ..desc(ColorImage::FORMAT, Some(c"fragment_main"), 0.0, true)
        };
        drop(Pipeline::graphics(&device, &clamped));
    });
    assert!(flat.contains("only for targets that have it"), "{flat}");
    let ids_alone = panics(|| {
        let ids = Format::DEPTH.with_ids();
        drop(Pipeline::graphics(&device, &desc(ids, None, 0.0, false)));
    });
    assert!(ids_alone.contains("IDs only beside a color"), "{ids_alone}");

    // Each of the triangles over its viewport, its corners' depths, as one that covers it, and a
    // point of it to compare there: a texel's center, which the filter takes alone.
    let corners = |z: [f32; 2]| {
        [
            [-1.0, -1.0, z[0], 1.0],
            [3.0, -1.0, z[1], 1.0],
            [-1.0, 3.0, z[0], 1.0f32],
        ]
    };
    let triangles = [
        ([0, 0], [8, 16], corners([0.5; 2])),
        ([8, 0], [8, 8], corners([1.5; 2])),
        // 0.15 across a unit of clip space, 4 texels, from 0.2 at the left edge.
        ([8, 8], [8, 8], corners([0.2, 0.8])),
    ];
    let sloped = 0.2 + 0.15 * 3.5 / 4.0;
    let probes = [
        ([4, 8], 0.55, 255),
        ([4, 8], 0.45, 0),
        ([12, 4], 0.99, 0),
        ([11, 12], sloped - 0.0375, 255),
    ];
    let mut image =
        ColorImage::new(&device, probes.len() as u32, 1, "shadow probes").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let (target, shadow) = (graph.target(0), graph.depth("shadow map", [16, 16]));
    let drawn = graph.render(label!("shadow")).depth(shadow).add();
    let black = [0.0, 0.0, 0.0, 1.0];
    let compared = (graph.render(label!("compare")).clear(target, black))
        .sample(shadow)
        .add();
    for _ in 0..3 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        let mut pass = frame
            .render(drawn)
            .expect("the graph's own images are always there");
        assert_eq!(pass.format(), Format::DEPTH);
        pass.set_pipeline(&depth);
        // As `shadow.slang` takes them, the corners first.
        let push = |pass: &mut Pass<'_, '_>, words: [u32; 17]| {
            pass.push_constants(&words.map(u32::to_ne_bytes).concat());
        };
        for (origin, size, corners) in triangles {
            pass.set_viewport(origin, size);
            let mut words = [0; 17];
            for (word, value) in words.iter_mut().zip(corners.as_flattened()) {
                *word = value.to_bits();
            }
            push(&mut pass, words);
            pass.draw(3, 1);
        }
        drop(pass);
        let mut pass = frame
            .render(compared)
            .expect("an offscreen image is always there");
        let map = pass.sampled(shadow);
        pass.set_pipeline(&compare);
        for (x, ([u, v], depth, _)) in (0..).zip(probes) {
            pass.set_viewport([x, 0], [1, 1]);
            let mut words = [0; 17];
            words[12..].copy_from_slice(&[
                map,
                COMPARISON_SAMPLER,
                ((u as f32 + 0.5) / 16.0).to_bits(),
                ((v as f32 + 0.5) / 16.0).to_bits(),
                f32::to_bits(depth),
            ]);
            push(&mut pass, words);
            pass.draw(3, 1);
        }
        drop(pass);
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    for (pixel, (at, depth, lit)) in pixels.as_chunks::<4>().0.iter().zip(probes) {
        assert_eq!(*pixel, [lit, lit, lit, 255], "{depth} compared at {at:?}");
    }

    drop((depth, compare, frames, image, device, instance));
    check(&validation);
}

/// Renders the graph's own IDs beside an image over eight frames, each pass clearing them to 0,
/// and reads pixels of them back:
///
/// - frames 0 to 4: one, none, three, none and two;
/// - frame 5: sixty, then, after another pass, the four of ten more that the most a frame reads
///   leaves, then none;
/// - frames 6 and 7: none.
///
/// Each frame finds what the one two before read, by its number, and none after a frame that read
/// none, as the first two find of frame 0. Each then samples the IDs from the layout its reads
/// left, frame 5 between its reads too. Synchronization validation sees each copy wait for the
/// pass before it, a pass for the copies and sampling before it, and the host for the copies. A
/// pixel outside the IDs is refused before anything is recorded, and a read of IDs no pass renders
/// as the graph is built.
fn ids_read_back_two_frames_later() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 8, 4, "read test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let black = [0.0, 0.0, 0.0, 1.0];
    let pixels: Vec<[u32; 2]> = (0..70).map(|i| [i % 8, i / 8 % 4]).collect();
    let message = panics(|| {
        let mut unread = Graph::default();
        let unrendered = unread.ids("unrendered", [8, 4]);
        unread.read_ids(label!("too soon"), unrendered);
    });
    assert!(message.contains("before any pass writes it"), "{message}");
    // A pass that renders IDs, one that reads them, and one that samples them, as often as given,
    // then two more reads after the last, if `twice`.
    let graph_of = |times: usize, twice: bool| {
        let mut graph = Graph::default();
        let (target, ids) = (graph.target(0), graph.ids("read test IDs", [8, 4]));
        let mut passes = Vec::new();
        for _ in 0..times {
            passes.push(
                graph
                    .render(label!("ids"))
                    .clear(target, black)
                    .ids(ids)
                    .add(),
            );
            passes.push(graph.read_ids(label!("read ids"), ids));
            passes.push(
                (graph.render(label!("sample ids")))
                    .keep(target)
                    .sample(ids)
                    .add(),
            );
        }
        if twice {
            passes.push(graph.read_ids(label!("read ids"), ids));
            passes.push(graph.read_ids(label!("read ids"), ids));
        }
        (graph, ids, passes)
    };
    let once = graph_of(1, false);
    let sixth = graph_of(2, true);
    // How many IDs each frame reads.
    let counts = [1, 0, 3, 0, 2, MAX_READS, 0, 0];
    for (number, count) in (1..).zip(counts) {
        let (graph, ids, passes) = if count == MAX_READS { &sixth } else { &once };
        let mut frame = frames
            .begin([Target::Image(&mut image)], graph)
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
        let mut passes = passes.iter().copied();
        let mut pass = || passes.next().expect("a pass left");
        let sample = |frame: &mut Frame<'_>, pass| {
            let drawn = frame
                .render(pass)
                .expect("an offscreen image is always there");
            let _ = drawn.sampled(*ids);
        };
        drop(frame.render(pass()));
        let read = pass();
        if number == 3 {
            let message = panics(|| {
                let _ = frame.read_ids(read, &[[8, 0]]);
            });
            assert!(message.contains("outside the 8x4 IDs"), "{message}");
        }
        if count < MAX_READS {
            let read = frame.read_ids(read, &pixels[..count]);
            assert_eq!(read.expect("a read"), count, "frame {number}");
            sample(&mut frame, pass());
        } else {
            let read_ids = |frame: &mut Frame<'_>, pass: PassId, pixels: &[[u32; 2]]| {
                frame.read_ids(pass, pixels).expect("a read")
            };
            assert_eq!(read_ids(&mut frame, read, &pixels[..60]), 60);
            sample(&mut frame, pass());
            drop(frame.render(pass()));
            assert_eq!(read_ids(&mut frame, pass(), &pixels[60..]), 4);
            sample(&mut frame, pass());
            assert_eq!(read_ids(&mut frame, pass(), &pixels[60..]), 0);
            assert_eq!(read_ids(&mut frame, pass(), &pixels[..1]), 0);
        }
        frame.submit().expect("the frame's submission");
    }

    drop((frames, image, device, instance));
    check(&validation);
}

/// A graph's own images whose uses in a frame don't overlap share memory, and keep what each pass
/// left for the next, over more frames than are in flight. A color image cleared red and read
/// back, then another cleared blue and sampled into the target after the first's last use, take
/// no more memory than the larger; used together, they take more. Synchronization validation,
/// which tracks the shared memory, sees each first use wait for the memory's last use before it,
/// in its frame or the one before.
fn the_graphs_own_images_share_memory_where_their_uses_dont_overlap() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = sampling_spirv();
    let pipeline = sampling_pipeline(&device, &spirv);
    let mut image = ColorImage::new(&device, 2, 1, "shared test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let (red, blue) = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
    // The red image read back, then the blue sampled; with `together`, the red sampled with it.
    let graph_of = |together: bool| {
        let mut graph = Graph::default();
        let target = graph.target(0);
        let first = graph.color("first", [2, 1]);
        let second = graph.color("second", [2, 1]);
        let clear_red = graph.render(label!("red")).clear(first, red).add();
        let read_red = graph.read_image(label!("read red"), first);
        let clear_blue = graph.render(label!("blue")).clear(second, blue).add();
        let mut sampling = (graph.render(label!("sample")))
            .clear(target, [0.0; 4])
            .sample(second);
        if together {
            sampling = sampling.sample(first);
        }
        let sample = sampling.add();
        (graph, second, [clear_red, read_red, clear_blue, sample])
    };
    let mut bytes = Vec::new();
    for together in [false, true] {
        let (graph, second, [clear_red, read_red, clear_blue, sample]) = graph_of(together);
        for _ in 0..4 {
            let mut frame = frames
                .begin([Target::Image(&mut image)], &graph)
                .expect("a frame")
                .expect("an offscreen image can always be rendered into");
            let number = frame.number();
            drop(frame.render(clear_red));
            frame.read_image(read_red).expect("a read");
            drop(frame.render(clear_blue));
            let mut pass = (frame.render(sample)).expect("an offscreen image is always there");
            pass.set_pipeline(&pipeline);
            for x in 0..2 {
                pass.set_viewport([x, 0], [1, 1]);
                // As `sample.slang` takes them: the image, a sampler, its level and a point of it.
                let words = [
                    pass.sampled(second),
                    Sampler::NearestClamp.index(),
                    0.0f32.to_bits(),
                    (x as f32 * 0.5 + 0.25).to_bits(),
                    0.5f32.to_bits(),
                ];
                pass.push_constants(&words.map(u32::to_ne_bytes).concat());
                pass.draw(3, 1);
            }
            drop(pass);
            frame.submit().expect("the frame's submission");
            let read = frames.image_read(number).expect("a wait for the frame");
            let read = read.expect("the frame read the red image back");
            let mut pixels = [0; 8];
            read.read_into(&mut pixels).expect("the pixels");
            assert_eq!(pixels, [255, 0, 0, 255].repeat(2)[..], "frame {number}");
        }
        let pixels = image.read_back().expect("the image's pixels");
        assert_eq!(pixels, [0, 0, 255, 255].repeat(2), "the blue image sampled");
        bytes.push(frames.transient_bytes());
    }
    let [apart, together] = bytes[..] else {
        unreachable!("one for each graph");
    };
    assert!(
        0 < apart && apart < together,
        "images used apart hold {apart} bytes, and used together {together}"
    );

    drop((frames, image, pipeline, device, instance));
    check(&validation);
}

/// Depth a pass tests and writes, then colors a later pass clears blue and the last samples into
/// the target, in the depth's memory, over more frames than are in flight. Synchronization
/// validation sees each first use wait for what the other image left in the memory, and for what
/// its own uses left a frame before, of other stages and accesses than the other's.
fn depth_and_colors_take_turns_in_memory() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let spirv = sampling_spirv();
    let pipeline = sampling_pipeline(&device, &spirv);
    let mut image = ColorImage::new(&device, 2, 1, "turns test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let mut graph = Graph::default();
    let target = graph.target(0);
    let depth = graph.depth("depth", [2, 1]);
    let colors = graph.color("colors", [2, 1]);
    let tested = (graph.render(label!("depth")))
        .clear(target, [0.0; 4])
        .depth(depth)
        .add();
    let blue = (graph.render(label!("blue")))
        .clear(colors, [0.0, 0.0, 1.0, 1.0])
        .add();
    let sample = graph
        .render(label!("sample"))
        .keep(target)
        .sample(colors)
        .add();
    for _ in 0..4 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.render(tested));
        drop(frame.render(blue));
        let mut pass = (frame.render(sample)).expect("an offscreen image is always there");
        pass.set_pipeline(&pipeline);
        // As `sample.slang` takes them: the image, a sampler, its level and a point of it.
        let words = [
            pass.sampled(colors),
            Sampler::NearestClamp.index(),
            0.0f32.to_bits(),
            0.5f32.to_bits(),
            0.5f32.to_bits(),
        ];
        pass.push_constants(&words.map(u32::to_ne_bytes).concat());
        pass.draw(3, 1);
        drop(pass);
        frame.submit().expect("the frame's submission");
    }
    let pixels = image.read_back().expect("the image's pixels");
    assert_eq!(pixels, [0, 0, 255, 255].repeat(2), "the blue image sampled");

    drop((frames, image, pipeline, device, instance));
    check(&validation);
}

/// The frame two later, which reuses a frame's resources, finds how long its passes took on the
/// GPU, from the first's beginning to the last's end; none before.
fn frames_time_their_passes_on_the_gpu() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 64, 64, "timed test target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let once = clearing(1, [0.0, 1.0, 0.0, 1.0]);
    let mut twice = Graph::default();
    let target = twice.target(0);
    let passes: Vec<PassId> = (0..2)
        .map(|_| twice.render(label!("timed")).clear(target, [0.0; 4]).add())
        .collect();
    let twice = (twice, passes);
    for number in 1..=4 {
        let (graph, passes) = if number % 2 == 0 { &twice } else { &once };
        let mut frame = frames
            .begin([Target::Image(&mut image)], graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for &pass in passes {
            drop(frame.render(pass));
        }
        frame.submit().expect("the frame's submission");
        let timed = frames.gpu_time();
        // As frame 3 began, it read frame 1's timestamps.
        assert_eq!(
            timed.is_some(),
            number >= 3,
            "after frame {number}: {timed:?}"
        );
    }

    drop((frames, image, device, instance));
    check(&validation);
}

/// For each upload path the GPU has, streams a texture, one with mip levels and a buffer in a
/// frame, each whole in one part. It then writes over a pixel of the texture in each of the two
/// frames after the one that shows it, more frames than are in flight. Validation checks the layout
/// each path leaves the texture in against the one the write starts from, which it doesn't for
/// what the bindless sets sample, and the levels' copies against each level's size and the staged
/// bytes. A write to a texture whose slot shows a stand-in is refused before anything is recorded.
fn streamed_textures_can_be_written_over_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path), None) {
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
        let (graph, passes) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
        let mut writes = 0;
        for i in 0..20 {
            let mut frame = frames
                .begin([Target::Image(&mut image)], &graph)
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
            drop(frame.render(passes[0]));
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

/// For each upload path the GPU has, streams BC7 textures in a frame, each whole in one part: one
/// of 13x5 pixels, whose 4 by 2 blocks reach past its sides; one of 13x5 with its mip levels,
/// 6x2, 3x1 and 1x1, which copies and validation check against each level's blocks and the staged
/// bytes; and one of a pixel. Each block is solid in a color of its own. The GPU samples each in
/// the frame that shows them and the two after, finding each block decoded in its level and place.
/// A write to a BC7 texture is refused before anything is recorded.
fn bc7_textures_stream_and_decode_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path), None) {
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
        let (graph, passes) = clearing(images.len(), [0.0; 4]);
        let mut shown_at = None;
        for i in 0..20 {
            let mut frame = frames
                .begin(images.iter_mut().map(Target::Image), &graph)
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
                assert!(
                    message.contains("compressed texture is streamed"),
                    "{message}"
                );
            }
            for (&pass, (texture, (_, probes))) in passes.iter().zip(textures.iter().zip(&fills)) {
                draw_probes(&mut frame, pass, &pipeline, texture, probes, None);
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

/// For each upload path the GPU has, refills one texture in turn, each streamed whole in a frame:
/// RGBA8 of 2x2, BC7 of 13x5 with mip levels, RGBA8 of 5x3 with its, then BC7 of 4x4 and of a
/// pixel. Each refill makes the image anew in its format, which validation checks the copies
/// against. The GPU samples the image before until the frame that shows the new one, and the new
/// one from then. Pixels of another format's size are refused before anything changes.
fn textures_refill_between_rgba8_and_bc7_on_every_path() {
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path), None) {
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
        let (graph, passes) = clearing(1, [0.0; 4]);
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
                    .begin([Target::Image(&mut image)], &graph)
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
                draw_probes(&mut frame, passes[0], &pipeline, &texture, expected, None);
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

/// For each upload path the GPU has, streams a 64x64 texture with mip levels, 21,844 bytes, and a
/// buffer of 4,096 bytes of indices, a part a frame, within budgets of 4 KiB and 1 KiB.
///
/// - Each part keeps within its budget, in whole rows. The texture takes six: four of 16 rows of
///   its first level, its second whole, and its five smaller levels together, which validation
///   checks against the levels and the staged bytes.
/// - The GPU samples the slot's white stand-in until the frame that shows the texture, then its
///   levels. That frame is the last part's on the graphics queue or the host, and a later one on a
///   transfer queue, whose copies no frame waits for.
/// - The buffer draws the probes by its indices from the frame that takes it, at the earliest with
///   its last part, as the texture's.
/// - Then the texture is hidden behind the stand-in, which the next frames sample. An image is
///   streamed into it again and hidden after its first part, whose copies may still run. Another
///   is streamed whole, handed over on the transfer queue, and discarded in the next frame before
///   it's shown. The frames go on, validation silent.
fn textures_and_buffers_stream_by_parts_on_every_path() {
    const BUDGET: u64 = 4096;
    let instance = instance();
    let validation = instance.validation();
    let spirv = sampling_spirv();
    for path in UploadPath::ALL {
        let device = match Device::new(&instance, None, None, Some(path), None) {
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
        let (graph, passes) = clearing(1, [0.0; 4]);
        let (mut texture_copied, mut buffer_copied) = (0, 0);
        let (mut texture_parts, mut buffer_parts) = (0, 0);
        let (mut shown_at, mut taken_at) = (None, None);
        let mut hidden_at = None;
        for i in 0..40 {
            let mut frame = frames
                .begin([Target::Image(&mut image)], &graph)
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
                passes[0],
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

/// Streams an image into a texture, then in each frame after, begins a pass before finishing it.
/// Once its copies are done, the frame refuses to show the image after the pass, which would
/// sample it before it's ready, before anything changes. The frame still submits, and the next
/// shows it before its passes.
fn textures_come_to_show_images_before_the_frames_passes() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 4, 4, "refill test target").expect("an image");
    let format = TextureFormat::Rgba8Srgb;
    let mut stand_in = stand_in(&device, "refill test stand-in");
    let texture = Texture::placed(&device, 4, &stand_in, "refilled test texture");
    let mut texture = texture.expect("a texture");
    let mut frames = Frames::new(&device).expect("frames");
    let (graph, passes) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
    let mut refused = false;
    for i in 0..20 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
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
            drop(frame.render(passes[0]));
            frame.submit().expect("the frame's submission");
            break;
        }
        drop(frame.render(passes[0]));
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

/// Drops a placed texture's stand-in a frame after writing it, and a hidden texture's in the frame
/// that hides it. Each slot holds its stand-in's image, drawing white in every frame after, with
/// validation silent, until it shows an image of its own and the stand-in's goes. Blue textures of
/// the stand-ins' size are made once each stand-in's frames finish, so memory freed too soon is
/// taken and written over rather than left reading white.
fn stand_ins_live_while_slots_show_them() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let pipeline = sampling_pipeline(&device, &sampling_spirv());
    let probe = |expected| {
        [Probe {
            level: 0,
            uv: [0.5, 0.5],
            expected,
        }]
    };
    let mut image = ColorImage::new(&device, 1, 1, "stand-in probes").expect("an image");
    let mut dropped = Some(stand_in(&device, "dropped stand-in"));
    let placed = Texture::placed(&device, 8, dropped.as_ref().expect("made"), "stood in for");
    let mut placed = placed.expect("a texture");
    let mut frames = Frames::new(&device).expect("frames");
    let (graph, passes) = clearing(1, [0.0; 4]);
    let red = [255, 0, 0, 255];
    let (mut shown, mut hidden_at) = (false, None);
    let mut reusing = Vec::new();
    for i in 0..24 {
        let mut frame = frames
            .begin([Target::Image(&mut image)], &graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        match i {
            0 => write_stand_in(&mut frame, dropped.as_mut().expect("made")),
            1 => drop(dropped.take()),
            6 => {
                placed
                    .refill(1, 1, TextureFormat::Rgba8Srgb, false)
                    .expect("an image");
                stream_whole(&mut frame, &mut placed, &red);
            }
            _ => {}
        }
        if i >= 6 && hidden_at.is_none() {
            shown = frame.finish_texture(&mut placed).expect("a finish");
        }
        // Two frames after it shows its own image, hidden behind a stand-in dropped at once.
        if shown && hidden_at.is_none() && i >= 8 {
            let mut again = stand_in(&device, "hiding stand-in");
            write_stand_in(&mut frame, &mut again);
            frame.hide_texture(&mut placed, &again);
            drop(again);
            hidden_at = Some(i);
        }
        let after_hidden = hidden_at.is_some_and(|at| (at + 3..at + 6).contains(&i));
        if (3..6).contains(&i) || after_hidden {
            let format = TextureFormat::Rgba8Srgb;
            let texture = Texture::new(&device, 1, 1, format, false, "reusing stand-ins' memory");
            let mut texture = texture.expect("a texture");
            let blue = [0, 0, 255, 255];
            frame
                .write_texture(&mut texture, [0, 0], [1, 1], &blue)
                .expect("a write");
            reusing.push(texture);
        }
        let expected = if shown && hidden_at.is_none() {
            red
        } else {
            [255; 4]
        };
        draw_probes(
            &mut frame,
            passes[0],
            &pipeline,
            &placed,
            &probe(expected),
            None,
        );
        frame.submit().expect("the frame's submission");
        check_probes(&mut image, &probe(expected), &format!("frame {i}"));
        if hidden_at.is_some_and(|at| i >= at + 6) {
            break;
        }
    }
    assert!(
        hidden_at.is_some(),
        "the texture never showed its own image"
    );
    drop((frames, placed, reusing, image, pipeline, device));

    drop(instance);
    check(&validation);
}

/// An offscreen image and a graph's own image with a side past any device's limit are refused with
/// `gpu.image_too_large` before Vulkan is asked, and the frames go on with a graph that fits.
fn images_past_the_devices_most_are_refused() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let past = 1 << 20;
    let error = ColorImage::new(&device, past, 1, "too wide")
        .err()
        .expect("refused");
    assert_eq!(error.code(), "gpu.image_too_large", "{error:#}");
    assert!(error.to_string().contains("\"too wide\""), "{error}");
    let mut image = ColorImage::new(&device, 1, 1, "too large target").expect("an image");
    let mut frames = Frames::new(&device).expect("frames");
    let (mut graph, _) = clearing(1, [0.0; 4]);
    let tall = graph.color("too tall", [1, past]);
    graph.render(label!("too tall")).clear(tall, [0.0; 4]).add();
    let error = frames
        .begin([Target::Image(&mut image)], &graph)
        .err()
        .expect("refused");
    assert_eq!(error.code(), "gpu.image_too_large", "{error:#}");
    let (graph, passes) = clearing(1, [0.0; 4]);
    let frame = frames.begin([Target::Image(&mut image)], &graph);
    let mut frame = frame.expect("a frame").expect("an offscreen image");
    drop(frame.render(passes[0]));
    frame.submit().expect("the frame's submission");
    drop((frames, image, device));

    drop(instance);
    check(&validation);
}

/// Forgets a frame, which the next frame catches, and a pass, which the frame's next call catches,
/// its submission among them, after which no frame begins again. Each reaches the JSON log as an
/// error with its code: the frame's as the frames drop, the pass's as the frame does. Nothing
/// forgotten reaches the GPU, so validation is silent.
fn forgotten_frames_and_passes_are_caught(log: &Path) {
    let instance = instance();
    let validation = instance.validation();
    let (graph, _) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
    // Each on a device of its own, since a device has one `Frames`, and a caught leak ends it.
    {
        let device = device(&instance);
        let mut image = ColorImage::new(&device, 4, 4, "forgotten frame target").expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let mut read_graph = Graph::default();
        let target = read_graph.target(0);
        let cleared = (read_graph.render(label!("cleared")))
            .clear(target, [0.0; 4])
            .add();
        let read = read_graph.read_image(label!("read"), target);
        let mut frame = frames
            .begin([Target::Image(&mut image)], &read_graph)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        drop(frame.render(cleared));
        frame.read_image(read).expect("a read");
        mem::forget(frame);
        // What it read is never waited for, as the GPU never gets the frame.
        let read = frames.image_read(1).expect("no wait");
        assert!(read.is_none(), "a frame never submitted read nothing");
        let message = panics(|| {
            let _ = frames.begin([Target::Image(&mut image)], &graph);
        });
        assert!(message.contains("frame 1 was forgotten"), "{message}");
        drop((frames, image, device));
    }
    {
        let device = device(&instance);
        let mut image = ColorImage::new(&device, 4, 4, "forgotten pass target").expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let (mut two, passes_of_two) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
        let target = two.target(0);
        let next = two.render(label!("next")).keep(target).add();
        let mut frame = frames
            .begin([Target::Image(&mut image)], &two)
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        mem::forget(frame.render(passes_of_two[0]));
        let message = panics(|| drop(frame.render(next)));
        assert!(message.contains("a pass was forgotten"), "{message}");
        let message = panics(|| {
            let _ = frame.submit();
        });
        assert!(message.contains("a pass was forgotten"), "{message}");
        // The frame's drop, as the submission unwound, failed it instead of submitting it.
        let error = frames
            .begin([Target::Image(&mut image)], &graph)
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

/// Gives one device's frame objects another device made: a target, textures to write, stream,
/// finish and hide, a stand-in, a buffer to stream and finish, and an upload's indices. Each is
/// refused before anything is recorded, so the frame still submits. The graph's own images are the
/// frame's device's.
fn another_devices_objects_are_refused() {
    let instance = instance();
    let validation = instance.validation();
    let other = device(&instance);
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 4, 4, "identity test target").expect("an image");
    let mut others_image = ColorImage::new(&other, 4, 4, "the other's image").expect("an image");
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
    let (graph, passes) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
    refused(panics(|| {
        let targets = [Target::Image(&mut image), Target::Image(&mut others_image)];
        let _ = frames.begin(targets, &graph);
    }));
    refused(panics(|| {
        let _ = Texture::placed(&device, 7, &others_texture, "stood in by the other's");
    }));
    let mut frame = frames
        .begin([Target::Image(&mut image)], &graph)
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
    let mut others_frame = others_frames
        .begin([Target::Image(&mut others_image)], &graph)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let others_indices = others_frame.upload(&[0; 12]).expect("an upload");
    let mut pass = frame
        .render(passes[0])
        .expect("an offscreen image is always there");
    refused(panics(|| pass.draw_indexed(others_indices, 0, 3, 0, 1)));
    drop(pass);
    frame.submit().expect("the frame's submission");
    drop(others_frame.render(passes[0]));
    others_frame.submit().expect("the other frame's submission");

    drop((frames, others_frames, image, others_image));
    drop((others_texture, others_buffer, placed, own_stand_in));
    drop((device, other, instance));
    check(&validation);
}

/// A device has one `Frames` at a time, and once one has begun a frame, no other, whose frame
/// numbers would repeat. One dropped before beginning any, as a failed one is, leaves the device to
/// another.
fn frames_that_began_none_leave_the_device_to_another() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 4, 4, "claim test target").expect("an image");
    let refused = |message: String| assert!(message.contains("one Frames"), "{message}");
    let first = Frames::new(&device).expect("frames");
    refused(panics(|| drop(Frames::new(&device))));
    drop(first);
    let mut frames = Frames::new(&device).expect("frames again, as the first began none");
    let (graph, passes) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
    let mut frame = frames
        .begin([Target::Image(&mut image)], &graph)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    drop(frame.render(passes[0]));
    frame.submit().expect("the frame's submission");
    drop(frames);
    refused(panics(|| drop(Frames::new(&device))));

    drop((image, device, instance));
    check(&validation);
}

/// Makes a texture in a placed slot, then asks for that slot again and for one that isn't placed.
/// Both are refused before anything is made, so dropping the device leaks nothing, which VMA would
/// abort on. A stand-in without an image of its own is refused too.
fn refused_texture_slots_leak_nothing() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
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

/// Asks a pass for a viewport wider than the limit, one past the bounds, and indices from a buffer
/// no frame has taken. Each is refused before it's recorded, so the pass goes on.
fn passes_refuse_what_the_device_cant_draw() {
    let instance = instance();
    let validation = instance.validation();
    let device = device(&instance);
    let mut image = ColorImage::new(&device, 4, 4, "limits test target").expect("an image");
    let buffer = GpuBuffer::new(&device, 12, "unfilled test buffer").expect("a buffer");
    let mut frames = Frames::new(&device).expect("frames");
    let (graph, passes) = clearing(1, [0.0, 0.0, 0.0, 1.0]);
    let mut frame = frames
        .begin([Target::Image(&mut image)], &graph)
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    let mut pass = frame
        .render(passes[0])
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

/// Saves a device's pipeline cache and loads it into other devices of the same GPU. Whole, it
/// loads; changed or cut short, it doesn't, and no driver is given it, so validation is silent. A
/// device loads one cache. The header's unit tests cover another GPU's data, and the renderer's GPU
/// tests drivers filling caches as pipelines are made.
fn pipeline_caches_load_what_this_gpu_saved_whole() {
    let instance = instance();
    let validation = instance.validation();
    let device = || device(&instance);
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
    let device = device(&instance);
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
    let device = device(&instance);
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
