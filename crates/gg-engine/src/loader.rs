//! The loader: decodes the images of the textures layers add, models' among them, off the main
//! thread, on a thread of its own, `gg-load`, and makes their mip levels there too, so that an add
//! hands out a texture's handle once its file's header has read, which gives its size, and the
//! texture shows its stand-in until its pixels have come and frames have streamed them to the GPU;
//! or, in a test's or a replay's context, which draws nothing, on the calling thread as each is
//! added. Either way, an image whose header read but whose pixels don't decode warns and keeps its
//! texture's stand-in, its add having succeeded, so that a replay's adds go as its run's did.

use std::collections::HashSet;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use gg_core::{Error, Result};
use gg_renderer::{Texels, Texture, Textures, add_levels};
use tracing::warn;

use crate::image::{self, Format, Image};

/// Decodes textures' images, as the module says, in the order their textures were added, one at a
/// time, as the renderer streams them.
pub(crate) struct Loader {
    kind: Kind,
    /// The textures whose images it's decoding, by their indices, which take their pixels as they
    /// come; one forgotten, as one removed or given another image meanwhile, takes none. Handles
    /// are never given out again, so an index names one texture for good.
    pending: HashSet<u32>,
}

enum Kind {
    /// Decodes each image on the calling thread as its texture is added, after its handle, leaving
    /// its levels to the frames that stream it: a test's or a replay's context's, which renders
    /// none, and a run's whose thread couldn't start.
    Inline,
    /// Decodes each image on the thread, which the first starts, `None` until then.
    Thread(Option<Worker>),
}

/// The thread, from the main thread's side. Dropped, it lets the thread go, unjoined: the thread
/// ends once the decode it's on is done, as it finds no one to send it to, and holds no file or
/// GPU object meanwhile, so that a run's end doesn't wait for a long decode.
struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Until it's joined, as it panicked.
    thread: Option<JoinHandle<()>>,
}

/// An image to decode: `texture`'s, `name`'s from `path`, read as far as its header, with the mip
/// levels made from it that `levels` says, as [`add_levels`] makes them, if it's given, as its
/// texture was reserved for.
struct Job {
    texture: Texture,
    name: String,
    path: PathBuf,
    image: Image,
    levels: Option<Texels>,
}

/// An image decoded, or why it didn't decode: `texture`'s pixels, its levels among them.
struct Done {
    texture: Texture,
    name: String,
    path: PathBuf,
    pixels: Result<Vec<u8>>,
}

impl Loader {
    /// A loader that decodes each image on the calling thread, as [`load`](Self::load) says.
    pub(crate) fn inline() -> Self {
        Self {
            kind: Kind::Inline,
            pending: HashSet::new(),
        }
    }

    /// A loader that decodes images on its thread, which the first image starts.
    pub(crate) fn threaded() -> Self {
        Self {
            kind: Kind::Thread(None),
            pending: HashSet::new(),
        }
    }

    /// Hands out a texture of `textures` for `image`, texture `name`'s from `path`, read as far as
    /// its header as [`image::load_image`] reads it, whose pixels it decodes, as
    /// [`image::decode_texture`] does, as `texels`, a [`Texels::Color`] or a [`Texels::Map`], say,
    /// with their mip levels, as [`Image::texels`] has them: on its thread, the texture showing its
    /// stand-in until [`take`](Self::take) fills it, its levels made there too; or, inline, at once,
    /// its levels left to the frames that stream it. Pixels that don't decode warn
    /// `engine.bad_texture`, as they're taken or at once, and the texture keeps its stand-in: the
    /// handle is the texture's whether they decode or not.
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
        if let Kind::Thread(None) = self.kind {
            self.kind = match Worker::start() {
                Ok(worker) => Kind::Thread(Some(worker)),
                Err(error) => {
                    warn!(
                        code = error.code(),
                        "{error:#}; images decode on the main thread as they're added"
                    );
                    Kind::Inline
                }
            };
        }
        // On the thread, colors and maps get their levels made with their pixels, as what's
        // cooked comes with its own; inline, the frames that stream them make them, as no frame
        // waits for this thread.
        let threaded = matches!(self.kind, Kind::Thread(_));
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
            Kind::Thread(Some(worker)) => {
                self.pending.insert(texture.index());
                // A thread that's gone panicked, which `take` resumes.
                let _ = worker.jobs.send(job);
            }
            Kind::Inline => fill(textures, decode(job)),
            Kind::Thread(None) => unreachable!("started above, or inline"),
        }
        Ok(texture)
    }

    /// Fills each texture whose image the thread has decoded since, in the order they were added,
    /// warning `engine.bad_texture` of each that didn't decode, which keeps its stand-in: as a
    /// frame is handed off, so that the frame takes them. One that finds none allocates nothing.
    ///
    /// # Panics
    ///
    /// If the thread panicked, which this resumes.
    pub(crate) fn take(&mut self, textures: &mut Textures) {
        let Self { kind, pending } = self;
        let Kind::Thread(Some(worker)) = kind else {
            return;
        };
        loop {
            match worker.done.try_recv() {
                Ok(done) => {
                    if pending.remove(&done.texture.index()) {
                        fill(textures, done);
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    let thread = worker.thread.take().expect("the thread is joined once");
                    match thread.join() {
                        Err(panic) => panic::resume_unwind(panic),
                        Ok(()) => unreachable!("the thread runs until the loader lets go"),
                    }
                }
            }
        }
    }

    /// Has `texture`, removed or given another image, take nothing of what its image decodes to,
    /// if it's decoding it.
    pub(crate) fn forget(&mut self, texture: Texture) {
        self.pending.remove(&texture.index());
    }

    /// How many textures' images it's decoding.
    pub(crate) fn pending(&self) -> u32 {
        self.pending.len() as u32
    }
}

impl Worker {
    /// Starts the thread, which decodes each job it's sent and sends it back, until the loader
    /// lets go. Fails with `engine.loader` if it can't start.
    fn start() -> Result<Self> {
        let (jobs, to_decode) = mpsc::channel::<Job>();
        let (back, done) = mpsc::channel();
        let thread = thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-load".to_owned())
            .spawn(move || {
                for job in to_decode {
                    if back.send(decode(job)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|error| {
                let message = "can't start the thread that decodes images";
                Error::new("engine.loader", message).with_source(error)
            })?;
        Ok(Self {
            jobs,
            done,
            thread: Some(thread),
        })
    }
}

/// Decodes `job`'s image, and makes its mip levels, as its texture was reserved for: a PNG's or a
/// JPEG's, but not what's cooked, which comes with its own.
fn decode(job: Job) -> Done {
    let _zone = gg_core::zone!("decode an image");
    let Job {
        texture,
        name,
        path,
        image,
        levels,
    } = job;
    let pixels = image::decode_texture(&name, &path, image).map(|image| {
        let mut pixels = image.pixels;
        if let Some(texels) = levels
            && image.format == Format::Rgba8
        {
            add_levels(&mut pixels, image.size, texels);
        }
        pixels
    });
    Done {
        texture,
        name,
        path,
        pixels,
    }
}

/// Fills `done`'s texture with its pixels, or warns that they didn't decode, as their error says,
/// the texture keeping its stand-in.
fn fill(textures: &mut Textures, done: Done) {
    let Done {
        texture,
        name,
        path,
        pixels,
    } = done;
    let filled = pixels.and_then(|pixels| textures.fill(texture, pixels));
    if let Err(error) = filled {
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
    use std::time::{Duration, Instant};

    use gg_renderer::Mipmaps;

    use super::*;

    /// A PNG of `width` pixels in a row, cut short of its last `cut` bytes, read as far as its
    /// header.
    fn read(width: u32, cut: usize) -> Image {
        let pixels = vec![9; width as usize * 4];
        let rgba = (png::ColorType::Rgba, png::BitDepth::Eight);
        let mut file = image::tests::png(rgba.0, rgba.1, width, &pixels, None);
        file.truncate(file.len() - cut);
        image::read(file).expect("its header")
    }

    /// Takes what `loader` decodes, as frames do, until it's decoding nothing.
    fn settle(loader: &mut Loader, textures: &mut Textures) {
        let until = Instant::now() + Duration::from_secs(30);
        while loader.pending() > 0 {
            assert!(Instant::now() < until, "the images decode");
            thread::sleep(Duration::from_millis(1));
            loader.take(textures);
        }
    }

    /// Whether `texture` has its pixels, as a fill of it again, a bug, panics for; one reserved
    /// still refuses pixels of no bytes, which no texture has.
    fn filled(textures: &mut Textures, texture: Texture) -> bool {
        let fill = AssertUnwindSafe(|| {
            let _ = textures.fill(texture, &[0u8; 0][..]);
        });
        panic::catch_unwind(fill).is_err()
    }

    /// Textures' handles come at once, as their images' headers read, before they decode, on the
    /// loader's thread, which fills each as it's taken, a model's image with its levels, of colors
    /// or a map; and inline, as a test's context decodes, each is filled as it's added.
    #[test]
    fn textures_decode_off_the_main_thread_and_fill_as_taken() {
        let mut textures = Textures::default();
        let mut loader = Loader::threaded();
        let path = Path::new("assets/a.png");
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

        let mut inline = Loader::inline();
        for mipmaps in [Mipmaps::None, Mipmaps::AlphaWeighted] {
            let texture = inline.load(&mut textures, "b", path, read(3, 0), mipmaps);
            let texture = texture.expect("a handle");
            assert_eq!(inline.pending(), 0);
            assert!(filled(&mut textures, texture), "filled as it's added");
        }
    }

    /// A texture whose image's header reads but whose rows don't decode loads all the same, its
    /// handle handed out, and warns, keeping its stand-in, unfilled: once its image comes to be
    /// taken from the loader's thread, or, inline, at once, so that a test's context or a
    /// replay's adds it as a run's does.
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

    /// A texture forgotten while its image decodes, as one removed or given another image is,
    /// takes nothing of it as it comes: here, its image decodes before the next's, which the
    /// thread decodes in order.
    #[test]
    fn a_texture_forgotten_while_decoding_takes_no_late_pixels() {
        let mut textures = Textures::default();
        let mut loader = Loader::threaded();
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
}
