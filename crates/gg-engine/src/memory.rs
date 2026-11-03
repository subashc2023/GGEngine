//! The process's memory as the OS counts it, which a run logs as it has loaded and as it ends: how
//! much of it is resident, and the most that has been.

/// The process's resident memory, now and at its most so far, in bytes: each `None` where the OS
/// doesn't say, as when its call fails or on an OS this reads neither on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resident {
    pub(crate) now: Option<u64>,
    pub(crate) peak: Option<u64>,
}

/// The process's resident memory, as [`Resident`] says: on Windows its working set and the most it
/// has been, through `K32GetProcessMemoryInfo`; on Linux `/proc/self/status`'s `VmRSS` and
/// `VmHWM`; on macOS `proc_pid_rusage`'s resident size and `getrusage`'s most resident. It asks the
/// OS, so a run reads it as it has loaded and as it ends, never each frame.
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

/// The working set's size and its peak, as `K32GetProcessMemoryInfo` gives them.
#[cfg(target_os = "windows")]
fn working_set() -> Resident {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: the counters are integers, of which zeros are valid values.
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the current process's pseudo handle needs no closing, and the call writes no more
    // than `size` bytes, the counters' own, into them.
    let read = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    if read == 0 {
        return Resident::default();
    }
    Resident {
        now: Some(counters.WorkingSetSize as u64),
        peak: Some(counters.PeakWorkingSetSize as u64),
    }
}

/// The resident memory and its peak that `text`, of `/proc/self/status`, gives, in bytes: its
/// `VmRSS` and `VmHWM` lines, each a number of kibibytes and `kB`.
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

/// The resident size, as `proc_pid_rusage` gives it, and its peak, as `getrusage` does, which on
/// macOS counts bytes, where Linux's counts kibibytes.
#[cfg(target_os = "macos")]
fn rusage() -> Resident {
    // SAFETY: the usage is integers, of which zeros are valid values.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: for this process, by its ID, and `RUSAGE_INFO_V2`, the call writes a
    // `rusage_info_v2` where its buffer points, which the C API's `rusage_info_t *` is cast from.
    let now = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast())
    };
    // SAFETY: the usage is integers and times, which are integers too, of which zeros are valid
    // values.
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

    /// Linux's status gives the resident size and its peak in kibibytes, which come out in bytes;
    /// a line missing, or one that doesn't read, gives none.
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

    /// The OS this runs on says how much of the process is resident, the peak at least as much.
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_os_says_how_much_is_resident() {
        let Resident { now, peak } = resident();
        let (now, peak) = (now.expect("now"), peak.expect("its peak"));
        assert!(now > 0 && peak >= now, "{now} now, {peak} at its most");
    }
}
