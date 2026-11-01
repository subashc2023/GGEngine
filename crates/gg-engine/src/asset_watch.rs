//! The asset database kept as the asset root changes, in development builds' runs that serve the
//! tool API: a thread of its own, `gg-assets`, scans the root every [`LOOK`], or at once when the
//! API's `assets` asks, and once what it finds holds from one scan to the next, takes it into the
//! database as a run's start does, as [`assets::scanned`] says: in a run that writes ID files,
//! writing those missing, and moving one a rename left behind beside its renamed asset, by the
//! prints of what the assets hold, which it keeps in its clone's cache for the next run's start.
//! The main thread takes the database it sends as a frame starts.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use gg_api::asset::{self as asset_files, Scan};
use gg_core::{Error, Result};

use crate::assets::{self, Catalog, Prints};

/// How long the thread waits between scans while nothing changes: a rename is taken about this
/// long after it, at most, and [`SETTLE`] more.
///
/// Each scan walks the whole root and reads every ID file, about 70 ms for 1,000 assets on
/// Windows, on this thread: a ceiling for asset roots of tens of thousands of files, which the OS's
/// notifications of changes would lift.
const LOOK: Duration = Duration::from_secs(1);

/// How long after a scan that found a change the next looks, for what it found to hold, so that a
/// copy under way, or a rename's two steps, settle first.
const SETTLE: Duration = Duration::from_millis(100);

/// How many scans in a row a change waits to hold before it's taken all the same, as while
/// something under the root changes all the time.
const MOST_SETTLING: u32 = 10;

/// How long the run's end waits for a scan under way, so that it doesn't end as one writes an ID
/// file, which would leave the file it writes first beside it.
const STOP: Duration = Duration::from_secs(1);

/// The thread, from the main thread's side, which ends it as it drops.
pub(crate) struct AssetWatch {
    /// How many looks the main thread has asked for, which a scan reads as it begins.
    asked: Arc<AtomicU64>,
    /// What wakes the thread for a look asked for; `None` once it's dropping.
    wake: Option<Sender<()>>,
    looked: Receiver<Looked>,
    /// The looks asked for that the thread has answered: those up to it.
    answered: u64,
}

/// What a look the thread took sends: the looks asked for before its scan began, which it answers,
/// and the database, if it changed.
struct Looked {
    asked: u64,
    catalog: Option<Catalog>,
}

impl AssetWatch {
    /// Starts the thread, watching the asset root `root` from `catalog`, the database as the run's
    /// start left it, with `prints`, as [`assets::scanned`] takes them, writing ID files and moving
    /// those a rename left behind if the run `writes`, and keeping the prints in the cache at
    /// `cache`, if there's one.
    ///
    /// Fails with `engine.asset_watch` if it can't start.
    pub(crate) fn start(
        root: PathBuf,
        catalog: Catalog,
        prints: Prints,
        writes: bool,
        cache: Option<PathBuf>,
    ) -> Result<Self> {
        let asked = Arc::new(AtomicU64::new(0));
        let (wake, woken) = mpsc::channel();
        let (back, looked) = mpsc::channel();
        let asking = Arc::clone(&asked);
        let mut watched = Watched {
            root,
            catalog,
            prints,
            writes,
            cache,
        };
        thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-assets".to_owned())
            .spawn(move || watch(&mut watched, &asking, &woken, &back))
            .map_err(|error| {
                let message = "can't start the thread that watches the asset root";
                Error::new("engine.asset_watch", message).with_source(error)
            })?;
        Ok(Self {
            asked,
            wake: Some(wake),
            looked,
            answered: 0,
        })
    }

    /// Asks the thread to look now, rather than at its next look; returns the look's number, which
    /// [`answered`](Self::answered) reaches once a scan begun after it has held.
    pub(crate) fn ask(&self) -> u64 {
        let look = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(wake) = &self.wake {
            // The thread runs until this side drops, or until it panics, which the log has.
            let _ = wake.send(());
        }
        look
    }

    /// The looks asked for that the thread has answered, as [`take`](Self::take) last found:
    /// every one, once it has ended, as when it panicked.
    pub(crate) fn answered(&self) -> u64 {
        self.answered
    }

    /// Takes what the thread sent since: the database, if it changed.
    pub(crate) fn take(&mut self) -> Option<Catalog> {
        let mut latest = None;
        loop {
            match self.looked.try_recv() {
                Ok(looked) => {
                    self.answered = looked.asked;
                    latest = looked.catalog.or(latest);
                }
                Err(TryRecvError::Empty) => return latest,
                Err(TryRecvError::Disconnected) => {
                    self.answered = u64::MAX;
                    return latest;
                }
            }
        }
    }
}

impl Drop for AssetWatch {
    /// Ends the thread, waiting up to [`STOP`] for a scan under way.
    fn drop(&mut self) {
        drop(self.wake.take());
        let until = Instant::now() + STOP;
        // Until the thread ends, letting go of its side.
        while (self.looked)
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .is_ok()
        {}
    }
}

/// What the thread keeps: the root it watches, the database and the prints, whether it writes ID
/// files, and the cache it keeps the prints in, if any.
struct Watched {
    root: PathBuf,
    catalog: Catalog,
    prints: Prints,
    writes: bool,
    cache: Option<PathBuf>,
}

/// The thread: scans `watched`'s root every [`LOOK`], or as it's woken, until the main thread's
/// side drops, and takes what it finds once it holds, as [`settled`] says, sending `back` the
/// database whenever it changes, and whenever looks were `asked` for since it last did, before it
/// reads what changed of the assets' files for their prints, as [`printed`] says.
fn watch(watched: &mut Watched, asked: &AtomicU64, woken: &Receiver<()>, back: &Sender<Looked>) {
    // What the last scan found, and how many in a row have found a change.
    let mut last: Option<Scan> = None;
    let mut settling = 0;
    let mut answered = 0;
    loop {
        // A look asked for wakes the thread, but no sooner than a change found has had its time
        // to settle.
        let until = Instant::now() + if settling > 0 { SETTLE } else { LOOK };
        loop {
            match woken.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(()) if settling > 0 => {}
                Ok(()) | Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        // Each look asked for before the scan begins is answered by it, those whose wakes wait
        // among them.
        while woken.try_recv().is_ok() {}
        let asking = asked.load(Ordering::Acquire);
        let found = {
            let _zone = gg_core::zone!("scan the asset root");
            asset_files::scan(&watched.root)
        };
        if last.as_ref() != Some(&found) && settling < MOST_SETTLING {
            settling += 1;
            last = Some(found);
            continue;
        }
        settling = 0;
        let catalog = settled(watched, &found);
        if catalog.is_some() || asking > answered {
            answered = asking;
            let looked = Looked {
                asked: asking,
                catalog,
            };
            if back.send(looked).is_err() {
                return;
            }
        }
        printed(watched, &found);
        last = Some(found);
    }
}

/// Takes `found`, what a scan found that held from the scan before, into `watched`'s database, as
/// [`assets::scanned`] says; returns the database if it changed.
fn settled(watched: &mut Watched, found: &Scan) -> Option<Catalog> {
    let _zone = gg_core::zone!();
    let next = assets::scanned(&watched.catalog, found, &mut watched.prints, watched.writes);
    if let Some(next) = &next {
        watched.catalog.clone_from(next);
    }
    next
}

/// In a run that writes ID files, which alone pairs renames, takes the prints of the assets
/// `found`, reading each whose file changed since, as [`Prints::update`] says, which a clone's
/// first run reads all of, and keeps them in the cache if they changed.
fn printed(watched: &mut Watched, found: &Scan) {
    let _zone = gg_core::zone!();
    let Watched {
        root,
        catalog,
        prints,
        writes,
        cache,
    } = watched;
    if *writes
        && prints.update(catalog, found)
        && let Some(cache) = cache
    {
        prints.save(cache, root);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gg_api::asset::Meta;

    use super::*;
    use crate::dirs::tests::Scratch;

    /// Takes what `watch` sends into `catalog` until it has answered look `look`.
    fn answered(watch: &mut AssetWatch, look: u64, catalog: &mut Catalog) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(next) = watch.take() {
                *catalog = next;
            }
            if watch.answered() >= look {
                return;
            }
            assert!(Instant::now() < until, "look {look} answered in time");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// The thread takes an asset that came, writing its ID file, and one renamed without its ID
    /// file, moving it beside it, each by the look asked for after; keeps the prints in the cache;
    /// and ends as its side drops, at once between scans.
    #[test]
    fn the_watcher_takes_what_changed_by_the_look_asked_for_after_it() {
        let scratch = Scratch::new("asset-watch");
        let root = scratch.0.join("game");
        fs::create_dir_all(&root).expect("a directory");
        fs::write(root.join("a.png"), "alpha").expect("an asset");
        let cache = scratch.0.join("cache").join("prints.json");
        let mut catalog = Catalog::default();
        let started = AssetWatch::start(
            root.clone(),
            catalog.clone(),
            Prints::default(),
            true,
            Some(cache.clone()),
        );
        let mut watch = started.expect("started");
        let look = watch.ask();
        answered(&mut watch, look, &mut catalog);
        let a = catalog.id("a.png").expect("an ID written");
        let held = |name: &str| Meta::read(&root.join(name)).expect("an ID file").id;
        assert_eq!(held("a.png.meta"), a);

        fs::rename(root.join("a.png"), root.join("b.png")).expect("renamed");
        let look = watch.ask();
        answered(&mut watch, look, &mut catalog);
        assert_eq!(catalog.id("b.png"), Some(a));
        assert_eq!(
            (held("b.png.meta"), root.join("a.png.meta").exists()),
            (a, false)
        );
        let kept = fs::read_to_string(&cache).expect("the cache");
        assert!(kept.contains(&a.to_string()), "{kept}");

        let stopping = Instant::now();
        drop(watch);
        assert!(stopping.elapsed() < STOP, "{:?}", stopping.elapsed());
    }
}
