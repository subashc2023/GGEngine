//! Frame captures through RenderDoc's in-application API, described by `renderdoc_app.h`,
//! installed beside RenderDoc. RenderDoc's library exports it once it's in the process: injected
//! into an app run through `renderdoccmd capture` or qrenderdoc, or loaded as RenderDoc's Vulkan
//! layer as an instance is made, with `ENABLE_VULKAN_RENDERDOC_CAPTURE=1`. The engine only finds
//! the library loaded, never loads it, and finds none where RenderDoc doesn't run, as on macOS.
//!
//! A capture shows every Vulkan object by its debug name, except the images of a torn-off
//! window's swapchain that the frame doesn't render into: RenderDoc records only the captured
//! window's images and those the frame uses, so these keep RenderDoc's names.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::{offset_of, size_of};
use std::path::PathBuf;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk::{self, Handle};
use gg_core::{Error, Result};

use crate::Device;

/// The API version the engine asks for, 1.4.0: the first with `DiscardFrameCapture`, the newest
/// function it calls. RenderDoc may give a later version, whose functions follow these.
const VERSION: c_int = 10400;

/// A RenderDoc function the engine doesn't call; it keeps the table's layout.
type Unused = Option<unsafe extern "C" fn()>;

/// The table `RENDERDOC_GetAPI` gives, laid out as `renderdoc_app.h`'s `RENDERDOC_API_1_4_0`, up
/// to the last function the engine calls.
#[repr(C)]
struct Api {
    /// GetAPIVersion to GetCaptureFilePathTemplate.
    _options: [Unused; 13],
    get_num_captures: Option<unsafe extern "C" fn() -> u32>,
    get_capture: Option<GetCapture>,
    /// TriggerCapture to SetActiveWindow.
    _triggers: [Unused; 4],
    start_frame_capture: Option<unsafe extern "C" fn(device: *mut c_void, window: *mut c_void)>,
    is_frame_capturing: Option<unsafe extern "C" fn() -> u32>,
    end_frame_capture: Option<EndCapture>,
    /// TriggerMultiFrameCapture and SetCaptureFileComments.
    _comments: [Unused; 2],
    discard_frame_capture: Option<EndCapture>,
}

type GetCapture = unsafe extern "C" fn(
    index: u32,
    path: *mut c_char,
    length: *mut u32,
    timestamp: *mut u64,
) -> u32;

/// `EndFrameCapture` and `DiscardFrameCapture`, which return 1 for success.
type EndCapture = unsafe extern "C" fn(device: *mut c_void, window: *mut c_void) -> u32;

// Where `renderdoc_app.h` puts the functions the engine calls, in pointers. CI never calls
// RenderDoc, so a layout slip shows here.
const _: () = {
    const SLOT: usize = size_of::<*const c_void>();
    assert!(size_of::<Unused>() == SLOT);
    assert!(offset_of!(Api, get_num_captures) == 13 * SLOT);
    assert!(offset_of!(Api, get_capture) == 14 * SLOT);
    assert!(offset_of!(Api, start_frame_capture) == 19 * SLOT);
    assert!(offset_of!(Api, is_frame_capturing) == 20 * SLOT);
    assert!(offset_of!(Api, end_frame_capture) == 21 * SLOT);
    assert!(offset_of!(Api, discard_frame_capture) == 24 * SLOT);
    assert!(size_of::<Api>() == 25 * SLOT);
};

/// The RenderDoc functions the engine calls, while its library is in the process.
pub(crate) struct RenderDoc {
    get_num_captures: unsafe extern "C" fn() -> u32,
    get_capture: GetCapture,
    start_frame_capture: unsafe extern "C" fn(device: *mut c_void, window: *mut c_void),
    is_frame_capturing: unsafe extern "C" fn() -> u32,
    end_frame_capture: EndCapture,
    discard_frame_capture: EndCapture,
    /// Whether one of the engine's captures is under way: two that overlap are undefined, even of
    /// different devices.
    capturing: AtomicBool,
    /// Keeps the library, and so its functions, loaded.
    _library: libloading::Library,
}

/// RenderDoc, if its library is in the process. Looked up on the first call, which must come
/// after an instance is made, as the loader may load it then.
pub(crate) fn renderdoc() -> Option<&'static RenderDoc> {
    // Once a process, as `RENDERDOC_GetAPI` must be called on one thread at a time.
    static RENDERDOC: OnceLock<Option<RenderDoc>> = OnceLock::new();
    RENDERDOC.get_or_init(RenderDoc::find).as_ref()
}

impl RenderDoc {
    fn find() -> Option<Self> {
        let library = library()?;
        let table = {
            // SAFETY: RenderDoc's library exports `RENDERDOC_GetAPI` with this signature
            // (`pRENDERDOC_GetAPI`).
            let get_api = unsafe {
                library.get::<unsafe extern "C" fn(c_int, *mut *mut c_void) -> c_int>(
                    b"RENDERDOC_GetAPI\0",
                )
            }
            .ok()?;
            let mut table = ptr::null_mut();
            // SAFETY: the function is RenderDoc's, called once a process by `renderdoc`.
            let found = unsafe { (*get_api)(VERSION, &raw mut table) };
            (found == 1 && !table.is_null()).then_some(table)?
        };
        // SAFETY: RenderDoc gave a table for version 1.4.0 or a later one, whose functions follow
        // 1.4.0's, so it starts as `Api` is laid out, aligned for pointers; the functions are valid
        // while `_library` keeps the library loaded.
        let api = unsafe { table.cast::<Api>().read() };
        Some(Self {
            get_num_captures: api.get_num_captures?,
            get_capture: api.get_capture?,
            start_frame_capture: api.start_frame_capture?,
            is_frame_capturing: api.is_frame_capturing?,
            end_frame_capture: api.end_frame_capture?,
            discard_frame_capture: api.discard_frame_capture?,
            capturing: AtomicBool::new(false),
            _library: library,
        })
    }

    /// Starts capturing what `device` does, until the capture ends or drops. Fails with
    /// `gpu.capture_busy` while another capture is under way: captures mustn't overlap.
    pub(crate) fn start<'a>(&'static self, device: &'a Device) -> Result<Capture<'a>> {
        let _zone = gg_core::zone!("start capture");
        let busy = || {
            let message = format!(
                "can't capture device {:?}: another capture is under way, which one mustn't \
                 overlap",
                device.name()
            );
            Error::new("gpu.capture_busy", message)
        };
        if self.capturing.swap(true, Ordering::Acquire) {
            return Err(busy());
        }
        // From here on, dropping `capture` clears the flag and discards what it started.
        let mut capture = Capture {
            renderdoc: self,
            device,
            instance: dispatch_table(device.instance().raw().handle()),
            before: 0,
            ended: true,
        };
        // SAFETY: neither takes an argument. Captures that F12 or target control started count
        // too.
        unsafe {
            if (self.is_frame_capturing)() != 0 {
                return Err(busy());
            }
            capture.before = (self.get_num_captures)();
        }
        // SAFETY: the instance lives as long as `device`, which the capture borrows. A null window
        // matches any of the instance's windows, or none, as headless runs have.
        unsafe { (self.start_frame_capture)(capture.instance, ptr::null_mut()) };
        capture.ended = false;
        Ok(capture)
    }
}

/// RenderDoc's library, if it's loaded.
#[cfg(target_os = "windows")]
fn library() -> Option<libloading::Library> {
    // Takes a reference, which keeps the library loaded while the handle lives.
    libloading::os::windows::Library::open_already_loaded("renderdoc.dll")
        .ok()
        .map(Into::into)
}

/// RenderDoc's library, if it's loaded.
#[cfg(target_os = "linux")]
fn library() -> Option<libloading::Library> {
    use libloading::os::unix;

    /// `dlfcn.h`'s, the same in glibc, musl and bionic; libloading doesn't export it.
    const RTLD_NOLOAD: c_int = 4;
    // SAFETY: RTLD_NOLOAD loads nothing, so no library's initializers run: it finds the library
    // only if something loaded it, and takes a reference to it.
    unsafe { unix::Library::open(Some("librenderdoc.so"), unix::RTLD_NOW | RTLD_NOLOAD) }
        .ok()
        .map(Into::into)
}

/// RenderDoc doesn't run here.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn library() -> Option<libloading::Library> {
    None
}

/// What RenderDoc tells instances apart by: the address of the dispatch table at the start of the
/// object a `VkInstance` points to (`RENDERDOC_DEVICEPOINTER_FROM_VKINSTANCE`).
fn dispatch_table(instance: vk::Instance) -> *mut c_void {
    let object = ptr::with_exposed_provenance::<*mut c_void>(instance.as_raw() as usize);
    // SAFETY: a VkInstance is a dispatchable handle: a pointer to an object whose first word is
    // its dispatch table's address, as the loader's interface with layers requires.
    unsafe { object.read() }
}

/// A RenderDoc capture of what a device does, from [`Device::start_capture`] until it ends.
/// Dropped without ending, it's discarded.
#[must_use = "a capture that isn't ended is discarded"]
pub struct Capture<'a> {
    renderdoc: &'static RenderDoc,
    device: &'a Device,
    instance: *mut c_void,
    /// How many captures RenderDoc had made when this one started.
    before: u32,
    /// Whether it ended, or never started, so that dropping it discards nothing.
    ended: bool,
}

impl Capture<'_> {
    /// Ends the capture, which RenderDoc writes to a file, and returns the file's absolute path.
    /// Fails with `gpu.capture_failed` if RenderDoc made no capture.
    pub fn end(mut self) -> Result<PathBuf> {
        let _zone = gg_core::zone!("end capture");
        self.ended = true;
        let renderdoc = self.renderdoc;
        // SAFETY: the capture is under way, of the instance it started with, which `device` keeps;
        // the count takes nothing.
        let made = unsafe {
            (renderdoc.end_frame_capture)(self.instance, ptr::null_mut()) == 1
                && (renderdoc.get_num_captures)() == self.before + 1
        };
        let failed = || {
            let message = format!(
                "RenderDoc made no capture of device {:?}'s work",
                self.device.name()
            );
            Error::new("gpu.capture_failed", message)
        };
        if !made {
            return Err(failed());
        }
        // The path is written with no size limit, so its length comes first.
        let mut length = 0;
        // SAFETY: given no buffer, GetCapture writes the path's length alone.
        let found = unsafe {
            (renderdoc.get_capture)(
                self.before,
                ptr::null_mut(),
                &raw mut length,
                ptr::null_mut(),
            )
        };
        if found != 1 {
            return Err(failed());
        }
        // One zeroed byte more, so the path ends in a nul whether or not the length counts one.
        let mut path = vec![0_u8; length as usize + 1];
        // SAFETY: the buffer holds as many bytes as the path's length, and one more.
        unsafe {
            (renderdoc.get_capture)(
                self.before,
                path.as_mut_ptr().cast(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
        let path = CStr::from_bytes_until_nul(&path).map_err(|_| failed())?;
        let path = PathBuf::from(path.to_string_lossy().into_owned());
        // As its template gives it, possibly relative to the app's working directory:
        // renderdoccmd's `-d`, or the executable's directory without it.
        Ok(std::path::absolute(&path).unwrap_or(path))
    }
}

impl Drop for Capture<'_> {
    fn drop(&mut self) {
        if !self.ended {
            // SAFETY: as in `end`.
            unsafe { (self.renderdoc.discard_frame_capture)(self.instance, ptr::null_mut()) };
        }
        self.renderdoc.capturing.store(false, Ordering::Release);
    }
}
