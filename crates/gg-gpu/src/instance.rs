//! The Vulkan instance: the loader, validation, and the messenger that turns what Vulkan reports
//! into log events.

use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ash::{ext, khr, vk};
use gg_core::{Error, Result};
use raw_window_handle::HasDisplayHandle;
use tracing::{debug, info, trace, warn};

use crate::{Version, vk_error};

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// What an [`Instance`] is created with.
pub struct InstanceDesc<'a> {
    /// The app's name, which GPU tools such as RenderDoc show.
    pub app_name: &'a str,
    /// Run under the Khronos validation layer, with synchronization validation.
    pub validation: bool,
    /// The window the instance will present to, or `None` to render offscreen only.
    pub window: Option<&'a dyn HasDisplayHandle>,
}

/// The connection to the Vulkan loader, which devices and surfaces are made from.
pub struct Instance {
    entry: ash::Entry,
    raw: ash::Instance,
    surface_fn: Option<khr::surface::Instance>,
    debug_utils: Option<ext::debug_utils::Instance>,
    messenger: vk::DebugUtilsMessengerEXT,
    validation: Validation,
}

/// Counts the validation warnings and errors an instance reports. It outlives the instance, so
/// what validation reports while the instance is destroyed, such as objects left undestroyed,
/// counts too.
#[derive(Clone)]
pub struct Validation {
    messages: Arc<AtomicU32>,
    enabled: bool,
}

impl Validation {
    /// How many validation warnings and errors were reported.
    pub fn messages(&self) -> u32 {
        self.messages.load(Ordering::Relaxed)
    }

    /// Whether the instance ran under the validation layer. It doesn't when validation was
    /// asked for but the layer isn't installed.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl Instance {
    pub fn new(desc: &InstanceDesc<'_>) -> Result<Arc<Self>> {
        // SAFETY: the library ash loads, vulkan-1.dll or libvulkan, is the Khronos loader, whose
        // exports have the signatures ash expects.
        let entry = unsafe { ash::Entry::load() }.map_err(|error| {
            Error::new(
                "gpu.no_loader",
                "can't load the Vulkan loader; install a GPU driver that supports Vulkan 1.4",
            )
            .with_source(error)
        })?;
        // SAFETY: these queries take no handles, and the entry is loaded.
        let (loader, layers, extensions) = unsafe {
            (
                entry
                    .try_enumerate_instance_version()
                    .map_err(|e| vk_error("vkEnumerateInstanceVersion", e))?,
                entry
                    .enumerate_instance_layer_properties()
                    .map_err(|e| vk_error("vkEnumerateInstanceLayerProperties", e))?,
                entry
                    .enumerate_instance_extension_properties(None)
                    .map_err(|e| vk_error("vkEnumerateInstanceExtensionProperties", e))?,
            )
        };
        trace!(
            layers = %names(layers.iter().filter_map(|layer| layer.layer_name_as_c_str().ok())),
            "Vulkan layers installed"
        );

        let has_layer = layers
            .iter()
            .any(|layer| layer.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
        if desc.validation && !has_layer {
            warn!(
                code = "gpu.no_validation_layer",
                layer = "VK_LAYER_KHRONOS_validation",
                "Validation is on, but its layer isn't installed, so nothing is validated; install the Vulkan SDK"
            );
        }
        let validating = desc.validation && has_layer;
        let has = |name: &CStr| {
            extensions
                .iter()
                .any(|extension| extension.extension_name_as_c_str() == Ok(name))
        };
        let debug_utils = has(vk::EXT_DEBUG_UTILS_NAME);
        // SAFETY: as above, and the layer's name is a valid string.
        let layer_settings = validating
            && unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER)) }
                .is_ok_and(|extensions| {
                    extensions.iter().any(|extension| {
                        extension.extension_name_as_c_str() == Ok(vk::EXT_LAYER_SETTINGS_NAME)
                    })
                });

        // Not VK_KHR_portability_enumeration, though ash's examples enable it on macOS:
        // KosmicKrisp needs no portability extensions, and leaving it off keeps MoltenVK out of
        // the devices the loader lists.
        let mut enabled: Vec<&CStr> = Vec::new();
        if debug_utils {
            enabled.push(vk::EXT_DEBUG_UTILS_NAME);
        }
        if layer_settings {
            enabled.push(vk::EXT_LAYER_SETTINGS_NAME);
        }
        if let Some(window) = desc.window {
            let display = window.display_handle().map_err(|error| {
                Error::new("gpu.window", "can't get the window's display handle").with_source(error)
            })?;
            let required = ash_window::enumerate_required_extensions(display.as_raw())
                .map_err(|e| vk_error("listing the window system's Vulkan extensions", e))?;
            // SAFETY: ash-window returns pointers to its static, null-terminated extension names.
            enabled.extend(required.iter().map(|&name| unsafe { CStr::from_ptr(name) }));
        }
        let extension_pointers: Vec<*const c_char> =
            enabled.iter().map(|name| name.as_ptr()).collect();

        let app_name = CString::new(desc.app_name).unwrap_or_default();
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(c"GGEngine")
            .engine_version(engine_version())
            // ash 0.38 has Vulkan 1.3 headers, so the engine asks for 1.3 and reaches what 1.4
            // added through the extensions it was promoted from. Devices must still support 1.4.
            .api_version(vk::API_VERSION_1_3);
        let validation_layers = [VALIDATION_LAYER.as_ptr()];
        // ash's `LayerSettingEXT::values` sets the count in bytes rather than values, so the
        // setting is filled in by hand.
        let on = vk::TRUE;
        let settings = [vk::LayerSettingEXT {
            p_layer_name: VALIDATION_LAYER.as_ptr(),
            p_setting_name: c"validate_sync".as_ptr(),
            ty: vk::LayerSettingTypeEXT::BOOL32,
            value_count: 1,
            p_values: (&raw const on).cast(),
            ..Default::default()
        }];
        let mut settings_info = vk::LayerSettingsCreateInfoEXT::default().settings(&settings);
        let validation = Validation {
            messages: Arc::default(),
            enabled: validating,
        };
        // Reports what goes wrong while the instance is created and destroyed.
        let mut startup_messenger = messenger_info(&validation);
        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&extension_pointers);
        if validating {
            create_info = create_info.enabled_layer_names(&validation_layers);
            if debug_utils {
                create_info = create_info.push_next(&mut startup_messenger);
            }
            if layer_settings {
                create_info = create_info.push_next(&mut settings_info);
            }
        }
        // SAFETY: every pointer in the create info refers to locals that outlive the call.
        let raw = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|e| vk_error("vkCreateInstance", e))?;

        let mut instance = Self {
            surface_fn: desc
                .window
                .map(|_| khr::surface::Instance::new(&entry, &raw)),
            debug_utils: debug_utils.then(|| ext::debug_utils::Instance::new(&entry, &raw)),
            messenger: vk::DebugUtilsMessengerEXT::null(),
            validation,
            entry,
            raw,
        };
        if validating && let Some(debug_utils) = &instance.debug_utils {
            // SAFETY: the instance is live, and the messenger's user data, the instance's
            // message count, lives as long as the instance.
            instance.messenger = unsafe {
                debug_utils
                    .create_debug_utils_messenger(&messenger_info(&instance.validation), None)
            }
            .map_err(|e| vk_error("vkCreateDebugUtilsMessengerEXT", e))?;
        }
        info!(
            loader = %Version(loader.unwrap_or(vk::API_VERSION_1_0)),
            api = "1.3",
            validation = validating,
            sync_validation = layer_settings,
            "Vulkan instance created"
        );
        debug!(extensions = %names(enabled), "Instance extensions enabled");
        Ok(Arc::new(instance))
    }

    /// What validation reports; see [`Validation`].
    pub fn validation(&self) -> Validation {
        self.validation.clone()
    }

    pub(crate) fn entry(&self) -> &ash::Entry {
        &self.entry
    }

    pub(crate) fn raw(&self) -> &ash::Instance {
        &self.raw
    }

    /// The surface functions, which an instance made for a window has.
    pub(crate) fn surface_fn(&self) -> Option<&khr::surface::Instance> {
        self.surface_fn.as_ref()
    }

    /// Whether `VK_EXT_debug_utils` is on, for naming objects and labeling commands.
    pub(crate) fn has_debug_utils(&self) -> bool {
        self.debug_utils.is_some()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: devices and surfaces hold an Arc to their instance, so none is left. A null
        // messenger is allowed.
        unsafe {
            if let Some(debug_utils) = &self.debug_utils {
                debug_utils.destroy_debug_utils_messenger(self.messenger, None);
            }
            self.raw.destroy_instance(None);
        }
    }
}

fn engine_version() -> u32 {
    let part = |number: &str| number.parse().unwrap_or(0);
    vk::make_api_version(
        0,
        part(env!("CARGO_PKG_VERSION_MAJOR")),
        part(env!("CARGO_PKG_VERSION_MINOR")),
        part(env!("CARGO_PKG_VERSION_PATCH")),
    )
}

fn names<'a>(names: impl IntoIterator<Item = &'a CStr>) -> String {
    let names: Vec<Cow<'_, str>> = names.into_iter().map(CStr::to_string_lossy).collect();
    names.join(", ")
}

/// The messenger reports warnings and errors, and also the loader's and layers' informational
/// messages when trace logging is on.
fn messenger_info(validation: &Validation) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    use vk::DebugUtilsMessageSeverityFlagsEXT as Severity;
    use vk::DebugUtilsMessageTypeFlagsEXT as Type;

    let mut severity = Severity::WARNING | Severity::ERROR;
    if tracing::enabled!(target: "gg_gpu::vulkan", tracing::Level::TRACE) {
        severity |= Severity::INFO | Severity::VERBOSE;
    }
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(severity)
        .message_type(Type::GENERAL | Type::VALIDATION | Type::PERFORMANCE)
        .pfn_user_callback(Some(on_message))
        .user_data(Arc::as_ptr(&validation.messages).cast_mut().cast())
}

/// Logs a message from the loader, a layer or a driver, and counts validation's warnings and
/// errors.
unsafe extern "system" fn on_message(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    use vk::DebugUtilsMessageSeverityFlagsEXT as Severity;
    use vk::DebugUtilsMessageTypeFlagsEXT as Type;

    // SAFETY: the loader passes callback data that is valid during the call, or null.
    let Some(data) = (unsafe { data.as_ref() }) else {
        return vk::FALSE;
    };
    // Only validation's messages count against a run: the loader and third-party layers, such as
    // a broken overlay, report problems of the machine rather than of the engine.
    let validation = types.contains(Type::VALIDATION);
    if validation && severity.intersects(Severity::WARNING | Severity::ERROR) {
        // SAFETY: the user data is the instance's message count, which lives until
        // vkDestroyInstance returns.
        unsafe { &*user_data.cast::<AtomicU32>() }.fetch_add(1, Ordering::Relaxed);
    }
    // SAFETY: the callback data's strings and arrays are valid during the call.
    let (message, id, objects, labels) = unsafe {
        (
            data.message_as_c_str()
                .map(CStr::to_string_lossy)
                .unwrap_or_default(),
            data.message_id_name_as_c_str().map(CStr::to_string_lossy),
            Objects(slice(data.p_objects, data.object_count)),
            Labels(slice(data.p_cmd_buf_labels, data.cmd_buf_label_count)),
        )
    };
    let code = if validation {
        "gpu.validation"
    } else if types.contains(Type::PERFORMANCE) {
        "gpu.performance"
    } else {
        "gpu.general"
    };
    // Validation's message IDs are VUIDs; the loader calls all of its messages "Loader Message".
    let vuid = id.as_deref().filter(|_| validation);
    let objects = (!objects.0.is_empty()).then(|| tracing::field::display(&objects));
    let labels = (!labels.0.is_empty()).then(|| tracing::field::display(&labels));
    macro_rules! log {
        ($level:ident) => {
            tracing::$level!(target: "gg_gpu::vulkan", code, vuid, objects, labels, "{message}")
        };
    }
    if severity.contains(Severity::ERROR) {
        log!(error);
    } else if severity.contains(Severity::WARNING) {
        log!(warn);
    } else {
        log!(trace);
    }
    vk::FALSE
}

/// # Safety
///
/// `pointer` must be null or point to `len` values that live for `'a`.
unsafe fn slice<'a, T>(pointer: *const T, len: u32) -> &'a [T] {
    if pointer.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees it.
        unsafe { std::slice::from_raw_parts(pointer, len as usize) }
    }
}

/// The objects a message is about, as `IMAGE 0x2a0b "offscreen color"`. It only exists inside
/// the messenger's callback, where the names are valid.
struct Objects<'a>(&'a [vk::DebugUtilsObjectNameInfoEXT<'a>]);

impl fmt::Display for Objects<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, object) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{:?} {:#x}", object.object_type, object.object_handle)?;
            // SAFETY: inside the callback, a name is null or a valid string.
            if let Some(name) = unsafe { object.object_name_as_c_str() } {
                write!(f, " {:?}", name.to_string_lossy())?;
            }
        }
        Ok(())
    }
}

/// The debug labels open in the command buffer a message is about. Like [`Objects`], it only
/// exists inside the callback.
struct Labels<'a>(&'a [vk::DebugUtilsLabelEXT<'a>]);

impl fmt::Display for Labels<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, label) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            // SAFETY: inside the callback, a label's name is null or a valid string.
            if let Some(name) = unsafe { label.label_name_as_c_str() } {
                f.write_str(&name.to_string_lossy())?;
            }
        }
        Ok(())
    }
}
