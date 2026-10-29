//! GPU tests. They run on whatever GPU the loader offers, which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another.

// For setting the environment, and for breaking a Vulkan rule on purpose; see the SAFETY
// comments.
#![allow(unsafe_code)]

use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

use ash::vk;
use gg_gpu::{
    ColorImage, DepthImage, Device, Frames, GpuBuffer, IdImage, Instance, InstanceDesc, MAX_READS,
    PLACED_TEXTURES, Target, Texture, TextureFormat, UploadPath,
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
    // add validation errors of their own. Apps keep implicit layers, since RenderDoc's capture
    // layer is one; the loader warns about each one it leaves out here.
    // SAFETY: no other thread exists yet to read the environment.
    unsafe { std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~") };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("logs");
    let log = gg_core::log::init("gpu-tests", &dir).expect("the tests read the JSON log");

    run("clear_reads_back", clear_reads_back);
    run(
        "sampled_images_keep_their_layout",
        sampled_images_keep_their_layout,
    );
    run(
        "dropped_images_free_their_slots",
        dropped_images_free_their_slots,
    );
    run("depth_clears_in_every_pass", depth_clears_in_every_pass);
    run(
        "ids_read_back_two_frames_later",
        ids_read_back_two_frames_later,
    );
    run(
        "filled_textures_can_be_written_over_on_every_path",
        filled_textures_can_be_written_over_on_every_path,
    );
    run(
        "forgotten_frames_and_passes_are_caught",
        forgotten_frames_and_passes_are_caught,
    );
    run(
        "another_devices_objects_are_refused",
        another_devices_objects_are_refused,
    );
    run(
        "refused_texture_slots_leak_nothing",
        refused_texture_slots_leak_nothing,
    );
    run(
        "passes_refuse_what_the_device_cant_draw",
        passes_refuse_what_the_device_cant_draw,
    );
    run(
        "pipeline_caches_load_what_this_gpu_saved_whole",
        pipeline_caches_load_what_this_gpu_saved_whole,
    );
    run("validation_errors_reach_the_json_log", || {
        validation_errors_reach_the_json_log(&log);
    });
}

fn run(name: &str, test: impl FnOnce()) {
    test();
    println!("test {name} ... ok");
}

/// Runs `broken`, which must panic, and gives its message.
fn panics(broken: impl FnOnce()) -> String {
    let payload = panic::catch_unwind(AssertUnwindSafe(broken)).expect_err("a panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "a panic whose payload isn't a string".to_owned())
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Makes and drops more images, one at a time, than the bindless set has slots to give out past
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Fills a texture and a buffer in a frame, then writes over a pixel of the texture in each of the
/// next two, more frames than are in flight, once for each way of uploading that the GPU has:
/// validation checks the layout each way leaves the texture in against the one the write starts
/// from, which it doesn't for what the bindless set samples. The frame fills a texture with mip
/// levels too, whose copies validation checks against each level's size and the staged bytes.
fn filled_textures_can_be_written_over_on_every_path() {
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
        let texture = Texture::new(&device, 8, 2, format, false, "filled test texture");
        let mut texture = texture.expect("a texture");
        // 5x3, 2x1 and 1x1.
        let levels = Texture::new(&device, 5, 3, format, true, "mipmapped test texture");
        let mut levels = levels.expect("a texture");
        let mut buffer = GpuBuffer::new(&device, 64, "filled test buffer").expect("a buffer");
        let mut frames = Frames::new(&device).expect("frames");
        for i in 0..3 {
            let mut frame = frames
                .begin([Target::Image(&mut image)])
                .expect("a frame")
                .expect("an offscreen image can always be rendered into");
            if i == 0 {
                let pixels = [255; 8 * 2 * 4];
                frame.fill_texture(&mut texture, &pixels).expect("a fill");
                let pixels = [127; (5 * 3 + 2 + 1) * 4];
                frame.fill_texture(&mut levels, &pixels).expect("a fill");
                frame.fill_buffer(&mut buffer, &[7; 64]).expect("a fill");
            } else {
                let write = frame.write_texture(&mut texture, [i, 0], [1, 1], &[0; 4]);
                write.expect("a write");
            }
            drop(frame.begin_pass(0, c"clear", [0.0, 0.0, 0.0, 1.0], None, None));
            frame.submit().expect("the frame's submission");
        }
        drop((frames, texture, levels, buffer, image, device));
    }

    drop(instance);
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Forgets a frame, which the next frame catches, and a pass, which the frame's next call catches,
/// its submission among them, after which no frame begins again. Nothing forgotten reaches the
/// GPU, so validation has nothing to say.
fn forgotten_frames_and_passes_are_caught() {
    let instance = instance();
    let validation = instance.validation();
    let black = [0.0, 0.0, 0.0, 1.0];
    // Each on a device of its own, since a device has one `Frames`, and a caught leak ends it.
    {
        let device =
            Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
        let mut image = ColorImage::new(&device, 4, 4, "forgotten frame target").expect("an image");
        let mut frames = Frames::new(&device).expect("frames");
        let frame = frames
            .begin([Target::Image(&mut image)])
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        mem::forget(frame);
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Opens two devices, and gives a frame of the one objects the other made: a target, textures, a
/// buffer, a depth image, an ID image to render into and to sample, and an upload's indices. Each
/// is refused before anything is recorded, so the frame goes on to submit.
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
    let mut frame = frames
        .begin([Target::Image(&mut image)])
        .expect("a frame")
        .expect("an offscreen image can always be rendered into");
    refused(panics(|| {
        let _ = frame.fill_texture(&mut others_texture, &[255; 4]);
    }));
    refused(panics(|| {
        let _ = frame.write_texture(&mut others_texture, [0, 0], [1, 1], &[255; 4]);
    }));
    refused(panics(|| {
        let _ = frame.fill_buffer(&mut others_buffer, &[7; 12]);
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
    drop((others_ids, others_texture, others_buffer));
    drop((device, other, instance));
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Makes a texture in a placed slot, then asks for its slot again, and for a slot that isn't
/// placed: both are refused before an image is made, so that dropping the device leaks nothing,
/// which VMA would abort on.
fn refused_texture_slots_leak_nothing() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None, None).expect("a GPU that supports Vulkan 1.4");
    let format = TextureFormat::Rgba8Srgb;
    let placed = Texture::placed(&device, 7, 1, 1, format, false, "placed test texture");
    let placed = placed.expect("a texture");
    let message = panics(|| {
        let _ = Texture::placed(&device, 7, 1, 1, format, false, "the same slot");
    });
    assert!(message.contains("texture slot 7 is in use"), "{message}");
    let message = panics(|| {
        let slot = PLACED_TEXTURES;
        let _ = Texture::placed(&device, slot, 1, 1, format, false, "a slot past the placed");
    });
    assert!(message.contains("isn't one of the"), "{message}");

    drop((placed, device, instance));
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
}

/// Asks a pass for viewports no device draws, one wider than its limit and one reaching past its
/// bounds, and to draw the indices of a buffer no frame has filled: each is refused before it's
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
    assert_eq!(
        validation.messages(),
        0,
        "validation reported problems; the log above says what"
    );
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
