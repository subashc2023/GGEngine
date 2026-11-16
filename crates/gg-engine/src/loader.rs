//! The loader decodes the images of the textures layers add (models' too) and makes their mip
//! levels, several at once on `gg-load` threads off the main thread. An add hands out the
//! texture's handle once its header has read (which gives its size); the texture shows its
//! stand-in until its pixels have come and frames have streamed them to the GPU. In a test's or a
//! replay's context, which draws nothing, each image decodes on the calling thread as it's added.
//!
//! Either way, an image whose header read but whose pixels don't decode warns and keeps its
//! texture's stand-in: the add succeeded, so a replay's adds go as its run's did. One that
//! decodes logs `asset.texture_decoded` with its time at the debug level, from its thread.

use std::collections::HashSet;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use gg_core::{Error, Result};
use gg_renderer::{Texels, Texture, Textures, add_levels};
use tracing::{debug, warn};

use crate::image::{self, Format, Image};
use crate::perf;

/// The most threads that decode images. Each holds its image whole, with its levels and the
/// floats they're made from: some 150 MiB for 4,096 pixels square. A scene's large images all come
/// at once, so they could hold over a GiB.
const MOST_THREADS: usize = 8;

/// Decodes textures' images (see the module) in the order added, as many at a time as it has
/// threads, and fills each texture as its image is done.
pub(crate) struct Loader {
    kind: Kind,
    /// The textures whose images it's decoding, by index; they take their pixels as they come.
    /// One forgotten (removed, or given another image meanwhile) takes none. Handles are never
    /// reused, so an index names one texture for good.
    pending: HashSet<u32>,
}

enum Kind {
    /// Decodes each image on the calling thread as its texture is added, after its handle, and
    /// leaves its levels to the streaming frames: a test's or a replay's context (which renders
    /// none), or a run whose threads couldn't start.
    Inline,
    /// Decodes on `count` threads, started by the first image (`None` until then).
    Threads {
        count: usize,
        workers: Option<Workers>,
    },
}

/// The threads, from the main thread's side. Dropped, it lets them go unjoined: each ends once its
/// current decode is done (it finds no one to send to) and holds no file or GPU object meanwhile,
/// so a run's end doesn't wait for a long decode.
struct Workers {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Each thread, until joined because it panicked.
    threads: Vec<JoinHandle<()>>,
}

/// An image to decode: `texture`'s, `name`'s from `path`, read as far as its header. If given,
/// `levels` says which mip levels to make ([`add_levels`]); the texture was reserved for them.
struct Job {
    texture: Texture,
    name: String,
    path: PathBuf,
    image: Image,
    levels: Option<Texels>,
}

/// `texture`'s pixels (levels included) decoded, or why they didn't.
struct Done {
    texture: Texture,
    name: String,
    path: PathBuf,
    pixels: Result<Vec<u8>>,
}

impl Loader {
    /// A loader that decodes each image on the calling thread.
    pub(crate) fn inline() -> Self {
        Self {
            kind: Kind::Inline,
            pending: HashSet::new(),
        }
    }

    /// A loader with threads the first image starts: half the hardware's (the rest are for the
    /// main, render and game threads), at least one, at most [`MOST_THREADS`]. If they can't
    /// start, decoding falls to the main thread, warning `engine.loader`.
    pub(crate) fn threaded() -> Self {
        let count = thread::available_parallelism().map_or(1, |count| count.get() / 2);
        Self::on_threads(count.clamp(1, MOST_THREADS))
    }

    /// A loader with `count` threads, started by the first image.
    fn on_threads(count: usize) -> Self {
        Self {
            kind: Kind::Threads {
                count,
                workers: None,
            },
            pending: HashSet::new(),
        }
    }

    /// Hands out a texture of `textures` for `image` (`name`'s from `path`, read as far as its
    /// header by [`image::load_image`]) and decodes its pixels as [`image::decode_texture`] does,
    /// as `texels` ([`Texels::Color`] or [`Texels::Map`]) with mip levels per [`Image::texels`].
    ///
    /// - On a thread: the texture shows its stand-in until [`take`](Self::take) fills it; levels
    ///   are made there too.
    /// - Inline: filled at once; levels are left to the streaming frames.
    ///
    /// Pixels that don't decode warn `engine.bad_texture`, when taken or at once; the texture
    /// keeps its stand-in and the handle stays its own.
    ///
    /// Fails as [`Textures::reserve`] does.
    pub(crate) fn load(
        &mut self,
        textures: &mut Textures,
        name: &str,
        path: &Path,
        image: Image,
        texels: impl Into<Texels>,
    ) -> Result<Texture> {
        if let Kind::Threads {
            count,
            workers: None,
        } = self.kind
        {
            self.kind = match Workers::start(count) {
                Ok(workers) => Kind::Threads {
                    count,
                    workers: Some(workers),
                },
                Err(error) => {
                    warn!(
                        code = error.code(),
                        "{error:#}; images decode on the main thread as they're added"
                    );
                    Kind::Inline
                }
            };
        }
        // On a thread, colors and maps get their levels with their pixels (cooked ones bring
        // their own); inline, the streaming frames make them, as no frame waits for these threads.
        let threaded = matches!(self.kind, Kind::Threads { .. });
        let texels = image.texels(texels.into());
        let (reserved, levels) = if threaded {
            (texels.with_levels(), Some(texels))
        } else {
            (texels, None)
        };
        let texture = textures.reserve(name, image.size, reserved)?;
        let job = Job {
            texture,
            name: name.to_owned(),
            path: path.to_owned(),
            image,
            levels,
        };
        match &self.kind {
            Kind::Threads {
                workers: Some(workers),
                ..
            } => {
                self.pending.insert(texture.index());
                // Fails only once every thread has panicked, whose panics `take` resumes.
                let _ = workers.jobs.send(job);
            }
            Kind::Inline => fill(textures, decode(job)),
            Kind::Threads { workers: None, .. } => unreachable!("started above, or inline"),
        }
        Ok(texture)
    }

    /// Fills each texture whose image the threads have decoded since the last call, warning
    /// `engine.bad_texture` of each that didn't (it keeps its stand-in). Called as a frame is
    /// handed off. A call that finds none allocates nothing.
    ///
    /// # Panics
    ///
    /// - If a thread panicked: the panic is resumed.
    pub(crate) fn take(&mut self, textures: &mut Textures) {
        let Self { kind, pending } = self;
        let Kind::Threads {
            workers: Some(workers),
            ..
        } = kind
        else {
            return;
        };
        while let Ok(done) = workers.done.try_recv() {
            if pending.remove(&done.texture.index()) {
                fill(textures, done);
            }
        }
        // A thread ends while the loader holds it only by panicking.
        if let Some(at) = workers.threads.iter().position(JoinHandle::is_finished) {
            match workers.threads.swap_remove(at).join() {
                Err(panic) => panic::resume_unwind(panic),
                Ok(()) => unreachable!("the threads run until the loader lets go"),
            }
        }
    }

    /// Has `texture` (removed, or given another image) take nothing of its decoding image.
    pub(crate) fn forget(&mut self, texture: Texture) {
        self.pending.remove(&texture.index());
    }

    /// How many images it's decoding.
    pub(crate) fn pending(&self) -> u32 {
        self.pending.len() as u32
    }
}

impl Workers {
    /// Starts `count` threads that decode jobs and send them back until the loader lets go. Fails
    /// with `engine.loader` if one can't start.
    fn start(count: usize) -> Result<Self> {
        let (jobs, to_decode) = mpsc::channel::<Job>();
        let to_decode = Arc::new(Mutex::new(to_decode));
        let (back, done) = mpsc::channel();
        let threads = (0..count)
            .map(|_| {
                let (to_decode, back) = (Arc::clone(&to_decode), back.clone());
                thread::Builder::new()
                    // Within the 15 bytes Linux keeps of a thread's name.
                    .name("gg-load".to_owned())
                    .spawn(move || {
                        loop {
                            // Held only while waiting for a job, so the others decode meanwhile.
                            let queue = to_decode.lock().unwrap_or_else(PoisonError::into_inner);
                            let Ok(job) = queue.recv() else {
                                return;
                            };
                            drop(queue);
                            if back.send(decode(job)).is_err() {
                                return;
                            }
                        }
                    })
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| {
                let message = "can't start the threads that decode images";
                Error::new("engine.loader", message).with_source(error)
            })?;
        Ok(Self {
            jobs,
            done,
            threads,
        })
    }
}

/// Decodes `job`'s image and makes the mip levels its texture was reserved for (PNG or JPEG;
/// cooked images bring their own). Logs `asset.texture_decoded` with how long it took, and how
/// much of that the levels took, if made.
fn decode(job: Job) -> Done {
    let _zone = gg_core::zone!("decode an image");
    let started = Instant::now();
    let Job {
        texture,
        name,
        path,
        image,
        levels,
    } = job;
    let size = image.size;
    let mut leveled = None;
    let pixels = image::decode_texture(&name, &path, image).map(|image| {
        let mut pixels = image.pixels;
        if let Some(texels) = levels
            && image.format == Format::Rgba8
        {
            let leveling = Instant::now();
            add_levels(&mut pixels, image.size, texels);
            leveled = Some(leveling.elapsed());
        }
        pixels
    });
    if pixels.is_ok() {
        debug!(
            code = "asset.texture_decoded",
            texture = &*name,
            path = %path.display(),
            ?size,
            ms = perf::millis(started.elapsed()),
            levels_ms = leveled.map(perf::millis),
            "Texture decoded"
        );
    }
    Done {
        texture,
        name,
        path,
        pixels,
    }
}

/// Fills `done`'s texture with its pixels, or warns with the decode's error; the texture keeps its
/// stand-in.
fn fill(textures: &mut Textures, done: Done) {
    let Done {
        texture,
        name,
        path,
        pixels,
    } = done;
    if let Err(error) = pixels.and_then(|pixels| textures.fill(texture, pixels)) {
        warn!(
            code = error.code(),
            texture = &*name,
            path = %path.display(),
            "{error:#}; it samples as its stand-in, a pixel of white, until its file decodes, as a \
             development run reloads it"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;
    use std::time::Duration;

    use gg_renderer::Mipmaps;

    use super::*;

    /// A one-row PNG of `width` pixels, its last `cut` bytes cut off, read as far as its header.
    fn read(width: u32, cut: usize) -> Image {
        let pixels = vec![9; width as usize * 4];
        let rgba = (png::ColorType::Rgba, png::BitDepth::Eight);
        let mut file = image::tests::png(rgba.0, rgba.1, width, &pixels, None);
        file.truncate(file.len() - cut);
        image::read(file).expect("its header")
    }

    /// Takes what `loader` decodes, as frames do, until it has nothing pending.
    fn settle(loader: &mut Loader, textures: &mut Textures) {
        let until = Instant::now() + Duration::from_secs(30);
        while loader.pending() > 0 {
            assert!(Instant::now() < until, "the images decode");
            thread::sleep(Duration::from_millis(1));
            loader.take(textures);
        }
    }

    /// Whether `texture` has its pixels: filling it again (a bug) panics, while a reserved one
    /// still refuses pixels of no bytes, which no texture has.
    fn filled(textures: &mut Textures, texture: Texture) -> bool {
        let fill = AssertUnwindSafe(|| {
            let _ = textures.fill(texture, &[0u8; 0][..]);
        });
        panic::catch_unwind(fill).is_err()
    }

    /// Handles come at once, before the images decode on one or several threads and are filled as
    /// taken (a model's image with its levels, colors or a map). Inline, each fills as added.
    #[test]
    fn textures_decode_off_the_main_thread_and_fill_as_taken() {
        let mut textures = Textures::default();
        let path = Path::new("assets/a.png");
        for mut loader in [Loader::on_threads(1), Loader::on_threads(3)] {
            let mut loaded = Vec::new();
            for (width, texels) in [
                (1, Texels::Color(Mipmaps::None)),
                (2, Texels::Color(Mipmaps::None)),
                (3, Texels::Color(Mipmaps::Averaged)),
                (3, Texels::Map(gg_renderer::Map::Normal)),
            ] {
                let texture = loader.load(&mut textures, "a", path, read(width, 0), texels);
                loaded.push(texture.expect("a handle"));
            }
            assert_eq!(loader.pending(), 4, "handed out before they decode");
            settle(&mut loader, &mut textures);
            for texture in loaded {
                assert!(
                    filled(&mut textures, texture),
                    "texture {}",
                    texture.index()
                );
            }
        }

        let mut inline = Loader::inline();
        for mipmaps in [Mipmaps::None, Mipmaps::AlphaWeighted] {
            let texture = inline.load(&mut textures, "b", path, read(3, 0), mipmaps);
            let texture = texture.expect("a handle");
            assert_eq!(inline.pending(), 0);
            assert!(filled(&mut textures, texture), "filled as it's added");
        }
    }

    /// A texture whose header reads but whose rows don't decode still loads: handle handed out, a
    /// warning (when taken, or at once inline), stand-in kept, unfilled.
    #[test]
    fn pixels_that_dont_decode_warn_and_keep_the_stand_in() {
        let logged = crate::testing::logged();
        let mut textures = Textures::default();
        let path = Path::new("assets/cut.png");
        for mut loader in [Loader::threaded(), Loader::inline()] {
            let cut = loader.load(&mut textures, "cut", path, read(2, 20), Mipmaps::None);
            let cut = cut.expect("a handle, as its header reads");
            settle(&mut loader, &mut textures);
            let codes: Vec<String> = logged.codes.try_iter().collect();
            assert_eq!(codes, ["engine.bad_texture"]);
            assert!(!filled(&mut textures, cut), "its stand-in kept");
        }
    }

    /// A texture forgotten while its image decodes takes nothing of it. One thread decodes in
    /// order, so its image comes before the next's.
    #[test]
    fn a_texture_forgotten_while_decoding_takes_no_late_pixels() {
        let mut textures = Textures::default();
        let mut loader = Loader::on_threads(1);
        let path = Path::new("assets/a.png");
        let forgotten = loader.load(&mut textures, "a", path, read(64, 0), Mipmaps::None);
        let forgotten = forgotten.expect("a handle");
        let next = loader.load(&mut textures, "b", path, read(1, 0), Mipmaps::None);
        let next = next.expect("a handle");
        loader.forget(forgotten);
        assert_eq!(loader.pending(), 1);
        settle(&mut loader, &mut textures);
        assert!(filled(&mut textures, next));
        assert!(!filled(&mut textures, forgotten), "no late pixels");
    }

    /// A decoding thread's panic resumes on the main thread when frames take, even with several
    /// threads, whose others keep the channel open. Here levels are made of one pixel said to be
    /// 4 pixels square: a broken invariant no file's image breaks.
    #[test]
    fn a_decode_that_panics_resumes_on_the_main_thread() {
        let mut textures = Textures::default();
        let mut loader = Loader::on_threads(2);
        let path = Path::new("assets/a.png");
        let image = Image {
            size: [4, 4],
            pixels: vec![9; 4],
            format: Format::Rgba8,
        };
        let texels = Texels::Color(Mipmaps::Averaged);
        let short = loader.load(&mut textures, "short", path, image, texels);
        short.expect("a handle");
        let until = Instant::now() + Duration::from_secs(30);
        let taken = panic::catch_unwind(AssertUnwindSafe(|| {
            while Instant::now() < until {
                thread::sleep(Duration::from_millis(1));
                loader.take(&mut textures);
            }
        }));
        assert!(taken.is_err(), "the panic resumed");
    }
}
