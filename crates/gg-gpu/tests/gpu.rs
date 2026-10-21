//! GPU tests. They run on whatever GPU the loader offers, which in Linux CI is lavapipe.
//!
//! They run in one process with a main of their own, which turns off the Vulkan loader's
//! implicit layers before any thread exists, then runs the tests one after another.

// For setting the environment, and for breaking a Vulkan rule on purpose; see the SAFETY
// comments.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use ash::vk;
use gg_gpu::{ColorImage, Device, Frames, Instance, InstanceDesc, Target};

fn main() {
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
    run("validation_errors_reach_the_json_log", || {
        validation_errors_reach_the_json_log(&log);
    });
}

fn run(name: &str, test: impl FnOnce()) {
    test();
    println!("test {name} ... ok");
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
    instance
}

/// Clears two offscreen images in the same frames, each to a color of its own, over more frames
/// than are in flight, then reads the colors back.
fn clear_reads_back() {
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None).expect("a GPU that supports Vulkan 1.4");
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
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    for _ in 0..3 {
        let mut frame = frames
            .begin(images.iter_mut().map(Target::Image))
            .expect("a frame")
            .expect("an offscreen image can always be rendered into");
        for (target, srgb) in srgb.iter().enumerate() {
            frame.clear(
                target,
                [linear(srgb[0]), linear(srgb[1]), linear(srgb[2]), 1.0],
            );
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

/// Breaks a Vulkan rule on purpose, and checks that validation's error reaches the JSON log
/// with its VUID and the debug name of the object involved.
fn validation_errors_reach_the_json_log(log: &Path) {
    const NAME: &str = "deliberately misviewed image";
    let instance = instance();
    let validation = instance.validation();
    let device = Device::new(&instance, None, None).expect("a GPU that supports Vulkan 1.4");
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
