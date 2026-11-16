//! A file only its owner can open, on Windows, as the tool API's session file (whose token unlocks
//! the game) must be. It's created with its own DACL: no ACEs inherited from its directory, all
//! access for its owner (the user the process runs as) and none for anyone else, so no other user
//! ever holds a handle, even one opened before a byte is written. std's `OpenOptions` inherits
//! its directory's ACEs, and a DACL set afterwards leaves earlier handles as they were, so this
//! calls `CreateFileW` itself.

use std::fs::{self, File};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

/// A private file's DACL, in SDDL: protected (`P`), inheriting no ACEs, with one ACE allowing
/// (`A`) all file access rights (`FA`) to its owner (`OW`, the Owner Rights SID), replacing the
/// read and change-DACL rights an owner has without one. The owner is the creator token's default:
/// the user the process runs as, or, for an elevated administrator where policy says so (Windows
/// Server's default), the Administrators group, whose members can take any file anyway.
const OWNER_ALONE: &str = "D:P(A;;FA;;;OW)";

/// Creates a new file at `path`, open for writing, with [`OWNER_ALONE`] from creation.
///
/// Fails with `AlreadyExists` if anything is there, a link too (not followed, as `O_EXCL` on
/// Unix), and with `InvalidInput` for a path with a NUL, as std does.
pub(crate) fn create_new(path: &Path) -> io::Result<File> {
    // Paths as long as std's threshold (MAX_PATH less room for an 8.3 name) work only in verbatim
    // form, `\\?\`, as std makes it for its own calls; `fs::canonicalize` gives the directory so.
    let verbatim;
    let path = match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if path.as_os_str().len() >= 248 => {
            verbatim = fs::canonicalize(dir)?.join(name);
            &verbatim
        }
        _ => path,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    if wide[..wide.len() - 1].contains(&0) {
        let message = "the path has a NUL in it";
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    }
    let sddl: Vec<u16> = OWNER_ALONE.encode_utf16().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the SDDL ends in 0 and outlives the call. `descriptor` takes the descriptor made,
    // freed below; its size isn't asked for.
    let made = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if made == 0 {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: the path ends in 0, and the attributes hold the descriptor just made; both outlive
    // the call, which copies the descriptor into the file. Shared as std shares files it opens.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    };
    // Before freeing the descriptor, which may set the thread's last error.
    let created = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    };
    // SAFETY: the conversion above allocated the descriptor, as `LocalFree` frees; nothing refers
    // to it now that `CreateFileW` has returned; it's freed once.
    unsafe { LocalFree(descriptor) };
    let handle = created?;
    // SAFETY: the handle is an open file's, just returned by `CreateFileW`; nothing else owns or
    // closes it.
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::core::PWSTR;

    use super::*;
    use crate::dirs::tests::Scratch;

    /// The DACL of the file at `path`, in SDDL, as Windows reads it back.
    fn dacl(path: &Path) -> String {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the path ends in 0 and outlives the call. `descriptor` takes the descriptor
        // allocated, freed below; the DACL pointer (into it), owner, group and SACL aren't asked
        // for.
        let read = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(read, 0, "the file's DACL wasn't read");
        let mut sddl: PWSTR = ptr::null_mut();
        let mut len = 0;
        // SAFETY: the descriptor is the one just read. `sddl` takes the string allocated, freed
        // below, and `len` its length, its 0 included.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut sddl,
                &mut len,
            )
        };
        assert_ne!(converted, 0, "the DACL wasn't written as SDDL");
        // SAFETY: the string is `len` UTF-16 units, alive until freed after this.
        let units = unsafe { std::slice::from_raw_parts(sddl, len as usize) };
        let text = String::from_utf16(units).expect("UTF-16");
        // SAFETY: each was allocated by its call, as `LocalFree` frees, and is freed once with
        // nothing referring to it after.
        unsafe {
            LocalFree(sddl.cast());
            LocalFree(descriptor);
        }
        text.trim_end_matches('\0').to_owned()
    }

    /// A private file's DACL is [`OWNER_ALONE`] (the owner can read it), even over a file there
    /// before, whose DACL it doesn't take, and past MAX_PATH.
    #[test]
    fn a_private_file_is_its_owner_s_alone_whatever_was_there() {
        let scratch = Scratch::new("private-dacl");
        let path = scratch.0.join("sessions").join("game-1.json");
        crate::dirs::write(&path, b"old").expect("written");
        let inherited = dacl(&path);
        assert!(inherited.contains("ID;"), "{inherited}");
        crate::dirs::write_private(&path, b"token").expect("written");
        assert_eq!(dacl(&path), OWNER_ALONE);
        assert_eq!(fs::read(&path).expect("its owner reads it"), b"token");
        let long = (scratch.0.join("d".repeat(150)).join("e".repeat(150))).join("game-1.json");
        crate::dirs::write_private(&long, b"token").expect("written");
        // `GetNamedSecurityInfoW` reaches it through its verbatim form.
        assert_eq!(dacl(&fs::canonicalize(&long).expect("there")), OWNER_ALONE);
    }
}
