//! A game's view as it renders, shared with a client: memory the client makes and names in a `view`
//! request, which the game writes each frame's view into and the client reads them from, without
//! either waiting for the other. The memory holds three slots of pixels, a triple buffer: the one
//! the game writes, the one the client reads, and the latest the game published, which the game
//! swaps its own for as it publishes and the client swaps its own for as it takes it, each swap a
//! single atomic exchange, so that neither ever reads or writes a slot the other has. A header
//! before them holds what both read: the size the client asks frames to be, how many the game has
//! published and the client taken, and each slot's frame.
//!
//! The memory is the OS's: on Windows, a named file mapping backed by the paging file, which goes
//! with its last handle; elsewhere, a POSIX shared memory object, whose name the client removes
//! once the game has opened it, so that the memory goes with the last of the two to end.

use std::io;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The bytes of a pixel: RGBA8.
const PIXEL: usize = 4;

/// What the memory begins with: its layout's name and version, in the last byte.
const MAGIC: u64 = u64::from_le_bytes(*b"ggview\0\x01");

/// Where the slots begin, a page in, past the header.
const SLOTS_AT: usize = 4096;

/// The slots: the one the game writes, the one the client reads, and the latest between them.
const SLOTS: u64 = 3;

/// Set in [`Header::latest`] from the game's publishing a slot until the client takes it.
const NEW: u64 = 1 << 32;

/// The longest side a frame may have, the renderer's textures'.
pub const MAX_SIDE: u32 = 8192;

/// The memory's header, at its start. Each field is atomic, as both processes read it while the
/// other may write it, and each has one writer: the client, which made the memory, or the game.
#[repr(C)]
struct Header {
    magic: AtomicU64,
    /// The bytes of each slot, a whole number of pages: the client's, as it made the memory.
    capacity: AtomicU64,
    /// The size the client asks frames to be, as [`pack`] packs it: the client's.
    asked: AtomicU64,
    /// The slot the game published last, with [`NEW`] set until the client takes it: both swap it.
    latest: AtomicU64,
    /// The slot the client reads: the client's, for a game that opens the memory again.
    front: AtomicU64,
    /// How many frames the game has published: the game's.
    published: AtomicU64,
    /// The number of the frame the client took last: the client's.
    taken: AtomicU64,
    /// Each slot's frame, as [`Frame`] gives it: the game's, written before it publishes the slot.
    slots: [Slot; SLOTS as usize],
}

/// A slot's frame, in the header.
#[repr(C)]
struct Slot {
    size: AtomicU64,
    number: AtomicU64,
    ticks: AtomicU64,
    started: AtomicU64,
    published: AtomicU64,
}

const _: () = assert!(size_of::<Header>() <= SLOTS_AT);

/// A frame of the view in the memory: what the game published of it, beside its pixels, rows of
/// RGBA8, sRGB-encoded and tightly packed, the top row first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Its width and height in pixels.
    pub size: [u32; 2],
    /// Its number among the frames the game has published to the memory, from 1.
    pub number: u64,
    /// The ticks the simulation had run in the frame that rendered it.
    pub ticks: u64,
    /// When the frame that rendered it started, and when the game published it, in nanoseconds
    /// since the Unix epoch, by the machine's clock, which both processes read alike.
    pub started: u64,
    pub published: u64,
}

impl Frame {
    /// The bytes of its pixels.
    pub fn bytes(&self) -> usize {
        bytes(self.size)
    }
}

/// The client's end: the memory, which it makes, and the frames it takes from it.
pub struct Reader {
    memory: Memory,
    /// The slot it reads.
    front: u64,
    /// The frame it took last, in `front`.
    frame: Option<Frame>,
}

impl Reader {
    /// Makes memory for frames of up to `width` by `height` pixels, asking for frames that size,
    /// and names it, as [`name`](Self::name) gives, for a `view` request.
    ///
    /// Fails with `InvalidInput` for a side of 0 or past [`MAX_SIDE`], and as the OS does if it
    /// can't make the memory.
    pub fn create(width: u32, height: u32) -> io::Result<Self> {
        let size = [width, height];
        if !fits_sides(size) {
            let message = format!("a {width}x{height} view; each side is 1 to {MAX_SIDE} pixels");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let capacity = bytes(size).next_multiple_of(SLOTS_AT);
        let mut memory = Memory::create(SLOTS_AT + SLOTS as usize * capacity)?;
        memory.capacity = capacity;
        let header = memory.header();
        header.capacity.store(capacity as u64, Ordering::Relaxed);
        header.asked.store(pack(size), Ordering::Relaxed);
        header.latest.store(1, Ordering::Relaxed);
        header.front.store(2, Ordering::Relaxed);
        // Last, so that a game that finds it finds the rest.
        header.magic.store(MAGIC, Ordering::Release);
        Ok(Self {
            memory,
            front: 2,
            frame: None,
        })
    }

    /// The memory's name, which a `view` request gives the game.
    pub fn name(&self) -> &str {
        &self.memory.name
    }

    /// Removes the memory's name, once the game has opened it, so that the memory goes with the
    /// last of the two to end, even if the client never drops this: elsewhere than on Windows,
    /// where it goes with its last handle anyway. It's removed as this drops, if not before.
    pub fn unlink(&mut self) {
        self.memory.unlink();
    }

    /// Asks the game for frames of `size`, from its next; returns whether they fit in the memory,
    /// as they must, or it asks for nothing new.
    pub fn ask(&self, size: [u32; 2]) -> bool {
        let fits = fits_sides(size) && bytes(size) <= self.memory.capacity;
        if fits {
            let asked = &self.memory.header().asked;
            asked.store(pack(size), Ordering::Relaxed);
        }
        fits
    }

    /// The size it asks frames to be.
    pub fn asked(&self) -> [u32; 2] {
        unpack(self.memory.header().asked.load(Ordering::Relaxed))
    }

    /// Takes the latest frame the game has published, if it's published one since the last taken,
    /// in place of that one, which [`read`](Self::read) then reads: `None` if it hasn't, or if what
    /// it published doesn't hold together, as a game that wrote over the header wouldn't.
    pub fn take(&mut self) -> Option<Frame> {
        let header = self.memory.header();
        if header.latest.load(Ordering::Acquire) & NEW == 0 {
            return None;
        }
        // The game wrote the slot, and its frame in the header, before it published it.
        let slot = header.latest.swap(self.front, Ordering::AcqRel) & !NEW;
        if slot >= SLOTS {
            return None;
        }
        self.front = slot;
        header.front.store(slot, Ordering::Relaxed);
        let published = &header.slots[slot as usize];
        let frame = Frame {
            size: unpack(published.size.load(Ordering::Relaxed)),
            number: published.number.load(Ordering::Relaxed),
            ticks: published.ticks.load(Ordering::Relaxed),
            started: published.started.load(Ordering::Relaxed),
            published: published.published.load(Ordering::Relaxed),
        };
        header.taken.store(frame.number, Ordering::Release);
        let fits = fits_sides(frame.size) && frame.bytes() <= self.memory.capacity;
        self.frame = fits.then_some(frame);
        self.frame
    }

    /// The frame taken last, which [`read`](Self::read) reads, if it held together.
    pub fn frame(&self) -> Option<Frame> {
        self.frame
    }

    /// Copies the pixels of the frame taken last into `into`, which holds exactly as many bytes
    /// as they do ([`Frame::bytes`]); does nothing before a frame's taken.
    ///
    /// # Panics
    ///
    /// If `into` doesn't hold exactly the frame's bytes.
    pub fn read(&self, into: &mut [u8]) {
        let Some(frame) = self.frame else {
            return;
        };
        assert_eq!(
            into.len(),
            frame.bytes(),
            "a frame's pixels fill what they're read into"
        );
        let from = self.memory.slot(self.front);
        // SAFETY: the slot lies within the memory, which lives as long as `self`, and holds at
        // least the frame's bytes, as `take` checked; the game writes another slot, as the triple
        // buffer has it, and one that breaks that can only tear the pixels copied, plain bytes.
        unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), into.as_mut_ptr(), into.len()) };
    }

    /// How many frames the game has published to the memory.
    pub fn published(&self) -> u64 {
        self.memory.header().published.load(Ordering::Acquire)
    }
}

/// The game's end: the memory a client made, opened by its name, and the frames it publishes to it.
pub struct Writer {
    /// Shared with the [`Pacer`]s it gives.
    memory: Arc<Memory>,
    /// The slot it writes.
    back: u64,
}

impl Writer {
    /// Opens the memory a client made and named `name`, as [`Reader::name`] gives it.
    ///
    /// Fails with `InvalidInput` for a name that isn't a view's, with `InvalidData` for memory that
    /// isn't laid out as a view's, and as the OS does if it can't open it, as when it isn't there.
    pub fn open(name: &str) -> io::Result<Self> {
        if !is_name(name) {
            let message = format!("{name:?} isn't the name of a view's memory, as gg-view-12-3 is");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let mut memory = Memory::open(name)?;
        let header = memory.header();
        let capacity = header.capacity.load(Ordering::Relaxed);
        let laid_out = header.magic.load(Ordering::Acquire) == MAGIC
            && capacity.is_multiple_of(SLOTS_AT as u64)
            && (capacity.checked_mul(SLOTS))
                .and_then(|slots| slots.checked_add(SLOTS_AT as u64))
                .is_some_and(|len| len <= memory.len as u64);
        let slots = || {
            let latest = header.latest.load(Ordering::Acquire) & !NEW;
            (latest, header.front.load(Ordering::Acquire))
        };
        // A client taking a frame swaps the latest for the slot it reads, then says which it reads
        // now: for that moment, the two are the same.
        let mut read = slots();
        for _ in 0..1000 {
            if read.0 != read.1 {
                break;
            }
            std::thread::yield_now();
            read = slots();
        }
        let (latest, front) = read;
        if !laid_out || latest >= SLOTS || front >= SLOTS || latest == front {
            let message = format!("the memory {name:?} isn't laid out as a view's");
            return Err(io::Error::new(io::ErrorKind::InvalidData, message));
        }
        // Checked against its length, and never read from the header again, which the client
        // could write over.
        memory.capacity = capacity as usize;
        // The slot neither the latest nor the client's: the slots are 0, 1 and 2.
        let back = SLOTS - latest - front;
        Ok(Self {
            memory: Arc::new(memory),
            back,
        })
    }

    /// The size the client asks frames to be: `None` for one that doesn't fit in the memory, which
    /// [`Reader::ask`] asks for none of, but a client that wrote over the header could.
    pub fn asked(&self) -> Option<[u32; 2]> {
        let size = unpack(self.memory.header().asked.load(Ordering::Relaxed));
        (fits_sides(size) && bytes(size) <= self.memory.capacity).then_some(size)
    }

    /// What another thread reads to pace frames by the client's taking them, while this
    /// publishes them.
    pub fn pacer(&self) -> Pacer {
        Pacer {
            memory: self.memory.clone(),
        }
    }

    /// Publishes a frame of `size`, which `fill` fills in, given its pixels' bytes to fill, as rows
    /// of RGBA8, sRGB-encoded and tightly packed, the top row first, from the frame that started
    /// at `started` with the simulation having run `ticks` ticks; returns it.
    ///
    /// Fails with `InvalidInput` if it doesn't fit in the memory, and with `InvalidData` if the
    /// slots don't hold together, as when a client writes over the header.
    pub fn publish(
        &mut self,
        size: [u32; 2],
        ticks: u64,
        started: SystemTime,
        fill: impl FnOnce(&mut [u8]),
    ) -> io::Result<Frame> {
        let header = self.memory.header();
        let capacity = self.memory.capacity;
        if !fits_sides(size) || bytes(size) > capacity {
            let [width, height] = size;
            let message = format!("a {width}x{height} frame doesn't fit in {capacity} bytes");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let slot = self.memory.slot(self.back);
        // SAFETY: the slot lies within the memory, which lives as long as `self` and the slice,
        // and holds the frame's bytes, as checked; the client reads another slot, as the triple
        // buffer has it, and one that breaks that can only tear the pixels it reads.
        let pixels = unsafe { std::slice::from_raw_parts_mut(slot.as_ptr(), bytes(size)) };
        fill(pixels);
        // Wrapping, as a client that wrote over the header could have it at the most.
        let number = header.published.load(Ordering::Relaxed).wrapping_add(1);
        let frame = Frame {
            size,
            number,
            ticks,
            started: nanos(started),
            published: nanos(SystemTime::now()),
        };
        let written = &header.slots[self.back as usize];
        written.size.store(pack(size), Ordering::Relaxed);
        written.number.store(number, Ordering::Relaxed);
        written.ticks.store(ticks, Ordering::Relaxed);
        written.started.store(frame.started, Ordering::Relaxed);
        written.published.store(frame.published, Ordering::Relaxed);
        let back = header.latest.swap(self.back | NEW, Ordering::AcqRel) & !NEW;
        header.published.store(number, Ordering::Release);
        if back >= SLOTS || back == self.back {
            let message = "the view's slots don't hold together, as a client that wrote over them";
            return Err(io::Error::new(io::ErrorKind::InvalidData, message));
        }
        self.back = back;
        Ok(frame)
    }
}

/// What a game reads of the memory to pace its frames by the client's taking them, from a thread
/// other than its [`Writer`]'s, which publishes them: a frame runs once the client has taken the
/// last, rather than rendering frames it never takes.
#[derive(Clone)]
pub struct Pacer {
    memory: Arc<Memory>,
}

impl Pacer {
    /// How many frames the game has published, which numbers the last.
    pub fn published(&self) -> u64 {
        self.memory.header().published.load(Ordering::Acquire)
    }

    /// The number of the frame the client took last; 0 before it's taken one.
    pub fn taken(&self) -> u64 {
        self.memory.header().taken.load(Ordering::Acquire)
    }
}

/// The bytes of the pixels of a frame of `size`.
fn bytes([width, height]: [u32; 2]) -> usize {
    width as usize * height as usize * PIXEL
}

/// Whether each side of `size` is 1 to [`MAX_SIDE`] pixels.
fn fits_sides(size: [u32; 2]) -> bool {
    size.iter().all(|&side| (1..=MAX_SIDE).contains(&side))
}

/// `size` in a word, width high.
fn pack([width, height]: [u32; 2]) -> u64 {
    u64::from(width) << 32 | u64::from(height)
}

fn unpack(packed: u64) -> [u32; 2] {
    [(packed >> 32) as u32, packed as u32]
}

/// `time` in nanoseconds since the Unix epoch; 0 before it.
fn nanos(time: SystemTime) -> u64 {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
}

/// Whether `name` is a view's memory's, as [`Memory::create`] names it: `gg-view-`, then the
/// process's ID and a number, each in decimal digits.
fn is_name(name: &str) -> bool {
    let digits = |part: &str| {
        !part.is_empty() && part.len() <= 10 && part.bytes().all(|b| b.is_ascii_digit())
    };
    name.strip_prefix("gg-view-")
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(pid, number)| digits(pid) && digits(number))
}

/// The memories this process has made, which number their names.
static MADE: AtomicU32 = AtomicU32::new(0);

/// Memory shared with another process, mapped into this one, by its name.
struct Memory {
    name: String,
    base: NonNull<u8>,
    len: usize,
    /// The bytes of each slot, as its end checked them: 0 until then.
    capacity: usize,
    /// The mapping, open while it's mapped: `Memory`'s drop unmaps it before its fields drop.
    #[cfg(windows)]
    _handle: windows::Handle,
    /// Whether this process made it and its name hasn't been removed.
    #[cfg(not(windows))]
    named: bool,
}

// SAFETY: the memory is reached only through atomics and copies, from whichever thread has the
// `Memory`, and the other process reaches it whatever thread this one does.
unsafe impl Send for Memory {}

// SAFETY: through a shared `Memory`, threads reach the header alone, whose fields are atomic; a
// slot's pixels are written only through a `&mut Writer`, and read only through the `Reader`, which
// owns its memory.
unsafe impl Sync for Memory {}

impl Memory {
    fn header(&self) -> &Header {
        // SAFETY: the memory begins with a header's bytes, page-aligned, which live as long as
        // `self`; each field is atomic, and so may change underneath the reference.
        unsafe { self.base.cast::<Header>().as_ref() }
    }

    /// Where slot `slot`'s pixels begin.
    fn slot(&self, slot: u64) -> NonNull<u8> {
        assert!(slot < SLOTS, "a view has {SLOTS} slots");
        // SAFETY: the slot, and its capacity's bytes, are within the memory, whose capacity its
        // end checked against its length.
        unsafe { self.base.add(SLOTS_AT + slot as usize * self.capacity) }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::ptr::{self, NonNull};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION,
        MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
        UnmapViewOfFile, VirtualQuery,
    };

    use super::{MADE, Memory};

    /// A file mapping's handle, closed as it drops.
    pub(super) struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is open, and nothing else closes it.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// `name` as Windows names the mapping: in the session's namespace, as UTF-16 ending in 0.
    fn wide(name: &str) -> Vec<u16> {
        format!("Local\\{name}").encode_utf16().chain([0]).collect()
    }

    impl Memory {
        /// Makes `len` bytes of memory, zeroed, named for this process.
        pub(super) fn create(len: usize) -> io::Result<Self> {
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("gg-view-{}-{made}", std::process::id());
            let wide = wide(&name);
            let len64 = len as u64;
            // SAFETY: the name is a string ending in 0, which outlives the call; the paging file
            // backs the mapping, as `INVALID_HANDLE_VALUE` asks.
            let handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE,
                    (len64 >> 32) as u32,
                    len64 as u32,
                    wide.as_ptr(),
                )
            };
            if handle == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: called right after the call whose error it reads.
            let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            let handle = Handle(handle);
            if existed {
                let message = format!("the memory {name:?} is there already");
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, message));
            }
            let (base, _) = map(&handle)?;
            Ok(Self {
                name,
                base,
                len,
                capacity: 0,
                _handle: handle,
            })
        }

        /// Opens the memory named `name`.
        pub(super) fn open(name: &str) -> io::Result<Self> {
            let wide = wide(name);
            // SAFETY: the name is a string ending in 0, which outlives the call.
            let handle =
                unsafe { OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, wide.as_ptr()) };
            if handle == 0 {
                return Err(io::Error::last_os_error());
            }
            let handle = Handle(handle);
            let (base, len) = map(&handle)?;
            Ok(Self {
                name: name.to_owned(),
                base,
                len,
                capacity: 0,
                _handle: handle,
            })
        }

        /// Nothing: the mapping goes with its last handle.
        pub(super) fn unlink(&mut self) {}
    }

    /// Maps all of `handle`'s mapping, and returns where, and its length in whole pages.
    fn map(handle: &Handle) -> io::Result<(NonNull<u8>, usize)> {
        // SAFETY: the handle is an open file mapping, which a length of 0 maps all of.
        let view = unsafe { MapViewOfFile(handle.0, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0) };
        let Some(base) = NonNull::new(view.Value.cast::<u8>()) else {
            return Err(io::Error::last_os_error());
        };
        // SAFETY: the struct is plain integers and pointers, for which zeros are valid.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let size = size_of::<MEMORY_BASIC_INFORMATION>();
        // SAFETY: `base` is the start of the view just mapped, and `info` is as large as told.
        let queried = unsafe { VirtualQuery(view.Value, &raw mut info, size) };
        if queried == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the view was mapped above and is unmapped once.
            unsafe { UnmapViewOfFile(view) };
            return Err(error);
        }
        Ok((base, info.RegionSize))
    }

    impl Drop for Memory {
        fn drop(&mut self) {
            let view = MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.base.as_ptr().cast(),
            };
            // SAFETY: the view was mapped as the memory was made or opened, nothing refers to it
            // past `self`, and it's unmapped once, before its handle closes.
            unsafe { UnmapViewOfFile(view) };
        }
    }
}

#[cfg(not(windows))]
mod posix {
    use std::io;
    use std::ptr::{self, NonNull};

    use rustix::fd::AsFd;
    use rustix::fs::{Mode, fstat, ftruncate};
    use rustix::io::Errno;
    use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
    use rustix::shm;

    use super::{MADE, Memory};

    /// `name` as POSIX names the object: after a slash.
    fn path(name: &str) -> String {
        format!("/{name}")
    }

    impl Memory {
        /// Makes `len` bytes of memory, zeroed, named for this process: in place of an object of
        /// that name another process left, which had this one's ID and has ended.
        pub(super) fn create(len: usize) -> io::Result<Self> {
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("gg-view-{}-{made}", std::process::id());
            let flags = shm::OFlags::CREATE | shm::OFlags::EXCL | shm::OFlags::RDWR;
            let mode = Mode::RUSR | Mode::WUSR;
            let fd = match shm::open(path(&name), flags, mode) {
                Err(Errno::EXIST) => {
                    shm::unlink(path(&name))?;
                    shm::open(path(&name), flags, mode)?
                }
                opened => opened?,
            };
            let made = ftruncate(&fd, len as u64)
                .and_then(|()| reserve(&fd, len))
                .and_then(|()| map(&fd, len));
            let base = match made {
                Ok(base) => base,
                Err(error) => {
                    let _ = shm::unlink(path(&name));
                    return Err(error.into());
                }
            };
            Ok(Self {
                name,
                base,
                len,
                capacity: 0,
                named: true,
            })
        }

        /// Opens the memory named `name`.
        pub(super) fn open(name: &str) -> io::Result<Self> {
            let fd = shm::open(path(name), shm::OFlags::RDWR, Mode::empty())?;
            let len = usize::try_from(fstat(&fd)?.st_size).unwrap_or(0);
            let base = map(&fd, len)?;
            Ok(Self {
                name: name.to_owned(),
                base,
                len,
                capacity: 0,
                named: false,
            })
        }

        /// Removes the memory's name, if this process made it and hasn't removed it.
        pub(super) fn unlink(&mut self) {
            if self.named {
                self.named = false;
                let _ = shm::unlink(path(&self.name));
            }
        }
    }

    /// Reserves the `len` bytes of the object `fd` opens, which Linux's `/dev/shm` would otherwise
    /// back only as they're first written, failing then, with `SIGBUS`, once it's full.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn reserve(fd: &impl AsFd, len: usize) -> Result<(), Errno> {
        rustix::fs::fallocate(fd, rustix::fs::FallocateFlags::empty(), 0, len as u64)
    }

    /// Nothing outside Linux, whose `/dev/shm` the reservation is for.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn reserve(_: &impl AsFd, _: usize) -> Result<(), Errno> {
        Ok(())
    }

    /// Maps `len` bytes of the object `fd` opens, shared.
    fn map(fd: &impl AsFd, len: usize) -> Result<NonNull<u8>, Errno> {
        if len == 0 {
            return Err(Errno::INVAL);
        }
        let flags = ProtFlags::READ | ProtFlags::WRITE;
        // SAFETY: a new mapping, where the OS picks, of an object the descriptor opens; the
        // mapping outlives the descriptor, and is unmapped once, as the memory drops.
        let base = unsafe { mmap(ptr::null_mut(), len, flags, MapFlags::SHARED, fd, 0) }?;
        NonNull::new(base.cast::<u8>()).ok_or(Errno::INVAL)
    }

    impl Drop for Memory {
        fn drop(&mut self) {
            self.unlink();
            // SAFETY: the mapping was made as the memory was made or opened, nothing refers to it
            // past `self`, and it's unmapped once.
            unsafe {
                let _ = munmap(self.base.as_ptr().cast(), self.len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(level: u8) -> impl FnOnce(&mut [u8]) {
        move |pixels| pixels.fill(level)
    }

    #[test]
    fn the_client_takes_the_latest_frame_the_game_published() {
        let mut reader = Reader::create(4, 2).expect("memory for a view");
        assert_eq!(reader.asked(), [4, 2]);
        let mut writer = Writer::open(reader.name()).expect("the memory, by its name");
        assert_eq!(writer.asked(), Some([4, 2]));
        let pacer = writer.pacer();
        assert_eq!((pacer.published(), pacer.taken()), (0, 0));
        assert_eq!(reader.take(), None);
        let started = UNIX_EPOCH + std::time::Duration::from_secs(1);
        let first = (writer.publish([4, 2], 7, started, gray(10))).expect("a frame that fits");
        assert_eq!(
            (first.number, first.ticks, first.started),
            (1, 7, 1_000_000_000)
        );
        assert_eq!((pacer.published(), pacer.taken()), (1, 0));
        let second = (writer.publish([2, 2], 8, started, gray(20))).expect("a smaller frame");
        assert_eq!(
            reader.take(),
            Some(second),
            "the latest, past the one it skipped"
        );
        assert_eq!((pacer.published(), pacer.taken()), (2, 2));
        let mut pixels = vec![0; second.bytes()];
        reader.read(&mut pixels);
        assert_eq!(pixels, [20; 16]);
        assert_eq!(reader.take(), None, "nothing new");
        // The game writes into neither the slot read nor the latest, however often it publishes.
        for level in 30..40 {
            writer
                .publish([4, 2], 9, started, gray(level))
                .expect("a frame");
        }
        reader.read(&mut pixels);
        assert_eq!(pixels, [20; 16], "the frame taken stays as it was");
        let last = reader.take().expect("the latest");
        assert_eq!(last.number, 12);
        let mut pixels = vec![0; last.bytes()];
        reader.read(&mut pixels);
        assert_eq!(pixels, [39; 32]);
        assert_eq!(reader.published(), 12);
    }

    #[test]
    fn a_game_that_opens_the_memory_again_writes_the_slot_neither_end_has() {
        let mut reader = Reader::create(1, 1).expect("memory for a view");
        let mut first = Writer::open(reader.name()).expect("the memory");
        first
            .publish([1, 1], 0, SystemTime::now(), gray(1))
            .expect("a frame");
        reader.take().expect("the frame");
        first
            .publish([1, 1], 0, SystemTime::now(), gray(2))
            .expect("a frame");
        drop(first);
        let mut again = Writer::open(reader.name()).expect("the memory again");
        again
            .publish([1, 1], 0, SystemTime::now(), gray(3))
            .expect("a frame");
        let mut pixel = [0; 4];
        reader.read(&mut pixel);
        assert_eq!(pixel, [1; 4], "the frame taken stays as it was");
        assert_eq!(reader.take().map(|frame| frame.number), Some(3));
        reader.read(&mut pixel);
        assert_eq!(pixel, [3; 4]);
    }

    #[test]
    fn frames_fit_in_the_memory_or_are_refused() {
        assert!(Reader::create(0, 1).is_err());
        assert!(Reader::create(1, MAX_SIDE + 1).is_err());
        let reader = Reader::create(64, 64).expect("memory for a view");
        assert!(reader.ask([32, 128]), "as many pixels fit");
        assert_eq!(reader.asked(), [32, 128]);
        assert!(!reader.ask([65, 64]));
        assert!(!reader.ask([0, 64]));
        assert_eq!(
            reader.asked(),
            [32, 128],
            "a size that doesn't fit isn't asked"
        );
        let mut writer = Writer::open(reader.name()).expect("the memory");
        assert_eq!(writer.asked(), Some([32, 128]));
        let refused = writer.publish([65, 64], 0, SystemTime::now(), gray(0));
        assert_eq!(
            refused.map_err(|error| error.kind()),
            Err(io::ErrorKind::InvalidInput)
        );
        // As a client that writes over the header could ask.
        let asked = &reader.memory.header().asked;
        for wrong in [[0, 64], [65, 64]] {
            asked.store(pack(wrong), Ordering::Relaxed);
            assert_eq!(writer.asked(), None, "{wrong:?}");
        }
    }

    #[test]
    fn only_a_views_memory_opens() {
        for name in [
            "",
            "gg-view-",
            "gg-view-1",
            "gg-view-1-",
            "gg-view-a-1",
            "other-1-2",
            "gg-view-1-2-3",
        ] {
            let opened = Writer::open(name).map(|_| ());
            assert_eq!(
                opened.map_err(|error| error.kind()),
                Err(io::ErrorKind::InvalidInput),
                "{name:?}"
            );
        }
        assert!(Writer::open("gg-view-0-4294967295").is_err(), "not there");
    }
}
