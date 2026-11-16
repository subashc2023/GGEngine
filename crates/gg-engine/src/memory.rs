//! The process's resident memory as the OS counts it, now and at its peak. A run logs it once
//! loaded and as it ends.

/// The process's resident memory now and at its peak, in bytes. Each is `None` where the OS
/// doesn't say: its call fails, or this OS isn't read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resident {
    pub(crate) now: Option<u64>,
    pub(crate) peak: Option<u64>,
}

/// The process's resident memory: on Windows the working set and peak
/// (`K32GetProcessMemoryInfo`); on Linux `/proc/self/status`'s `VmRSS` and `VmHWM`; on macOS
/// `proc_pid_rusage`'s resident size and `getrusage`'s maximum. It asks the OS: read it once
/// loaded and at the end, never each frame.
pub(crate) fn resident() -> Resident {
    #[cfg(target_os = "windows")]
    {
        working_set()
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/self/status");
        text.as_deref().map(status).unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        rusage()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Resident::default()
    }
}

/// The working set's size and peak.
#[cfg(target_os = "windows")]
fn working_set() -> Resident {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: the counters are integers; zeros are valid.
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the current process's pseudo handle needs no closing; the call writes at most `size`
    // bytes, the counters' own, into them.
    let read = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    if read == 0 {
        return Resident::default();
    }
    Resident {
        now: Some(counters.WorkingSetSize as u64),
        peak: Some(counters.PeakWorkingSetSize as u64),
    }
}

/// The resident memory and peak in bytes from `/proc/self/status`'s `text`: its `VmRSS` and
/// `VmHWM` lines, each kibibytes then `kB`.
#[cfg(any(target_os = "linux", test))]
fn status(text: &str) -> Resident {
    let field = |name: &str| {
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?;
        let kib: u64 = line.trim().strip_suffix("kB")?.trim_end().parse().ok()?;
        kib.checked_mul(1024)
    };
    Resident {
        now: field("VmRSS"),
        peak: field("VmHWM"),
    }
}

/// The resident size from `proc_pid_rusage` and its peak from `getrusage`, which counts bytes on
/// macOS (kibibytes on Linux).
#[cfg(target_os = "macos")]
fn rusage() -> Resident {
    // SAFETY: the usage is integers; zeros are valid.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: for this process and `RUSAGE_INFO_V2`, the call writes a `rusage_info_v2` where its
    // buffer points; the C API's `rusage_info_t *` is cast from it.
    let now = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast())
    };
    // SAFETY: the usage is integers and times (integers too); zeros are valid.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: the call writes this process's usage into `usage`, a `rusage`.
    let peak = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    Resident {
        now: (now == 0).then_some(info.ri_resident_size),
        peak: u64::try_from(usage.ru_maxrss).ok().filter(|_| peak == 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux's status gives kibibytes, which come out as bytes; a missing or unreadable line gives
    /// none.
    #[test]
    fn linux_status_gives_the_resident_size_and_its_peak() {
        let text = "Name:\tgg-player\nVmPeak:\t  912340 kB\nVmHWM:\t  204800 kB\n\
                    VmRSS:\t  102400 kB\nThreads:\t12\n";
        let read = status(text);
        assert_eq!(read.now, Some(102_400 * 1024));
        assert_eq!(read.peak, Some(204_800 * 1024));
        let partial = status("VmRSS:\t  12 kB\nVmHWM:\t lots\n");
        assert_eq!(
            partial,
            Resident {
                now: Some(12 * 1024),
                peak: None
            }
        );
        assert_eq!(status(""), Resident::default());
    }

    /// The peak is at least the current size.
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_os_says_how_much_is_resident() {
        let Resident { now, peak } = resident();
        let (now, peak) = (now.expect("now"), peak.expect("its peak"));
        assert!(now > 0 && peak >= now, "{now} now, {peak} at its most");
    }
}
