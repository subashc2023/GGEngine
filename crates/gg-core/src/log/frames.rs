//! A panic's frames, each as the module its return address is in and the offset there, which the
//! build's kept symbols turn into functions and lines offline, as ARCHITECTURE.md's Diagnostics
//! says: by each OS's own calls, `RtlCaptureStackBackTrace` and `GetModuleHandleExW` on Windows,
//! and `backtrace` and `dladdr`, glibc's on Linux and libSystem's on macOS.

use std::fmt::Write as _;

/// The most frames a panic logs.
const MOST: usize = 64;

/// The frames of the calls the caller is inside, innermost first, this function's own and the
/// panic hook's among them, space-separated, each as `<module>+0x<offset>`: the file name of the
/// module whose code holds its return address, rather than its path, which may name the player;
/// and the offset from where the module is loaded of the return address less one, which falls
/// inside the call, as symbolizers take it; `?+0x<address>` where no module holds it. Empty on an
/// OS whose calls it doesn't make.
pub(super) fn capture() -> String {
    let mut text = String::new();
    for (module, offset) in frames() {
        if !text.is_empty() {
            text.push(' ');
        }
        let _ = write!(text, "{}+{offset:#x}", module.as_deref().unwrap_or("?"));
    }
    text
}

/// The file name a module's path, `/` or `\` between its parts, ends with.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(windows)]
fn frames() -> Vec<(Option<String>, usize)> {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };

    let mut addresses = [std::ptr::null_mut::<c_void>(); MOST];
    // SAFETY: the buffer holds `MOST` pointers, as many as it's asked for, and the hash it may
    // write is asked for none.
    let count = unsafe {
        RtlCaptureStackBackTrace(0, MOST as u32, addresses.as_mut_ptr(), std::ptr::null_mut())
    };
    let addresses = &addresses[..usize::from(count).min(MOST)];
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    (addresses.iter())
        .map(|&address| {
            let call = (address as usize).wrapping_sub(1);
            let mut module: HMODULE = 0;
            // SAFETY: with FROM_ADDRESS the name is an address, which needn't point at text, and
            // with UNCHANGED_REFCOUNT the module's count isn't raised, so that nothing is freed;
            // `module` is written.
            let found = unsafe { GetModuleHandleExW(flags, call as *const u16, &mut module) };
            if found == 0 {
                return (None, call);
            }
            let mut path = [0u16; 1024];
            // SAFETY: the buffer holds as many units as it's said to, and `module` is a loaded
            // module's handle, which stays loaded while its code is on the stack.
            let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) };
            let path = String::from_utf16_lossy(&path[..(len as usize).min(path.len())]);
            let name = (!path.is_empty()).then(|| file_name(&path).to_owned());
            // A module's handle is the address it's loaded at.
            (name, call.wrapping_sub(module as usize))
        })
        .collect()
}

#[cfg(any(all(target_os = "linux", target_env = "gnu"), target_vendor = "apple"))]
fn frames() -> Vec<(Option<String>, usize)> {
    use std::ffi::{CStr, c_void};

    let mut addresses = [std::ptr::null_mut::<c_void>(); MOST];
    // SAFETY: the buffer holds `MOST` pointers, as many as it's asked for.
    let count = unsafe { libc::backtrace(addresses.as_mut_ptr(), MOST as libc::c_int) };
    let addresses = &addresses[..usize::try_from(count).unwrap_or(0).min(MOST)];
    (addresses.iter())
        .map(|&address| {
            let call = (address as usize).wrapping_sub(1);
            // SAFETY: an all-zero `Dl_info` is its pointers null, which dladdr overwrites.
            let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
            // SAFETY: dladdr takes any address, writing `info` if a loaded object holds it.
            let found = unsafe { libc::dladdr(call as *const c_void, &mut info) };
            if found == 0 || info.dli_fname.is_null() {
                return (None, call);
            }
            // SAFETY: the loader's name of a loaded object, a C string that outlives the call.
            let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
            let name = file_name(&path).to_owned();
            // Where the object is loaded: on Linux, the executable being position-independent,
            // the offset is the address addr2line takes; on macOS, it's from the image's header.
            (Some(name), call.wrapping_sub(info.dli_fbase as usize))
        })
        .collect()
}

#[cfg(not(any(
    windows,
    all(target_os = "linux", target_env = "gnu"),
    target_vendor = "apple"
)))]
fn frames() -> Vec<(Option<String>, usize)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modules_file_name_is_its_paths_last_part() {
        assert_eq!(
            file_name(r"C:\Users\ada\Games\Foxball\Foxball.exe"),
            "Foxball.exe"
        );
        assert_eq!(file_name("/home/ada/foxball/Foxball"), "Foxball");
        assert_eq!(file_name("Foxball"), "Foxball");
    }
}
