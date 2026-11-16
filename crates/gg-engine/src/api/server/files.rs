//! The threads that write captures' PNGs and snapshots, and prune the directories they're named in.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use gg_api::{Captured, FrameShot, Framed, Snapshotted};
use gg_core::{Error, Result};

use super::connection::Reply;
use super::failure;
use super::view::{FilmShot, api_viewpoint};
use crate::World;
use crate::render_thread::Shot;
use crate::scene_files::{Open, SceneFile};
use crate::snapshot::{self, Run};

/// How many PNGs the `captures` directory beside the executable keeps, the newest, as each capture
/// without a path is written there: older ones, a long session's and those of runs before, go.
const KEPT_CAPTURES: usize = 32;

/// How many snapshots the `snapshots` directory beside the executable keeps, the newest, as each
/// snapshot without a path is written there; those `gg run` takes, in a directory of its own
/// inside it, are its to remove.
const KEPT_SNAPSHOTS: usize = 8;

/// A snapshot for the thread that writes them: the copy of the world, the ticks the simulation had
/// run as it was taken, where it goes, and where the answer goes.
pub(super) struct Taken {
    pub(super) world: World,
    /// The scene files the layers loaded, which the restored run watches, and its open scene.
    pub(super) scenes: Vec<SceneFile>,
    /// The scene files' texts the run keeps for its scene changes.
    pub(super) cached: Vec<SceneFile>,
    pub(super) open: Open,
    /// The scripts the layers loaded, each a path and its text as last applied, and the mods.
    pub(super) scripts: Vec<(String, String)>,
    pub(super) mods: Vec<crate::luau::ModText>,
    /// What the run was doing, with a copy of the world as edited it held, if any, which the
    /// restored run goes on doing.
    pub(super) run: Run,
    /// Whether the simulation was paused, as the restored run is then.
    pub(super) paused: bool,
    pub(super) ticks: u64,
    pub(super) path: PathBuf,
    /// Whether the engine named the path, the request giving none, which prunes the directory.
    pub(super) named: bool,
    pub(super) reply: Reply,
    /// Told whether the snapshot was written once it's answered, for a snapshot the run waits for.
    pub(super) written: Option<Sender<bool>>,
}

/// A capture's PNG for the thread that writes them: the view, where it goes, the ticks the frame
/// that shows it had run, whether textures or meshes were still decoding or streaming in it, and
/// where the answer goes.
pub(super) struct Png {
    pub(super) shot: Arc<Shot>,
    /// Where it goes; `None` for a capture of a pixel alone, which writes nothing.
    pub(super) path: Option<PathBuf>,
    /// The pixel whose bytes the answer gives.
    pub(super) at: Option<[f64; 2]>,
    /// Whether the engine named the path, the request giving none, which prunes the directory.
    pub(super) named: bool,
    pub(super) ticks: u64,
    pub(super) streaming: bool,
    pub(super) reply: Reply,
}

/// What the thread that writes PNGs writes: a capture's, or a `frames`' every frame, into its
/// directory, numbered from 1, answered once all are written.
pub(super) enum Writing {
    Capture(Png),
    Frames {
        dir: PathBuf,
        shots: Vec<FilmShot>,
        reply: Reply,
    },
}

/// Writes the captures `pngs` brings, each as a PNG, and each `frames`' frames, as PNGs numbered
/// from 1, `frame-0001.png` and on, answering each once its PNGs are written, or as the first
/// fails, until the API drops.
pub(super) fn write_pngs(pngs: &Receiver<Writing>) {
    for writing in pngs {
        let png = match writing {
            Writing::Capture(png) => png,
            Writing::Frames { dir, shots, reply } => {
                let framed = (shots.into_iter().enumerate())
                    .map(|(at, back)| {
                        let path = dir.join(format!("frame-{:04}.png", at + 1));
                        write_png(&path, &back.shot, false)?;
                        let [width, height] = back.shot.size;
                        Ok(FrameShot {
                            path,
                            width,
                            height,
                            ticks: back.ticks,
                            streaming: back.streaming,
                            viewpoint: api_viewpoint(&back.viewpoint),
                        })
                    })
                    .collect::<Result<Vec<_>>>();
                match framed {
                    Ok(frames) => reply.result(&Framed { frames }),
                    Err(error) => reply.send(Err(failure(&error))),
                }
                continue;
            }
        };
        let [width, height] = png.shot.size;
        let written = (png.path.as_deref())
            .map_or(Ok(()), |path| write_png(path, &png.shot, png.named))
            .map(|()| Captured {
                path: png.path,
                width,
                height,
                pixel: png.at.and_then(|at| pixel_at(&png.shot, at)),
                ticks: png.ticks,
                streaming: png.streaming,
            });
        match written {
            Ok(captured) => png.reply.result(&captured),
            Err(error) => png.reply.send(Err(failure(&error))),
        }
    }
}

/// The RGBA bytes of the pixel `at` falls in, `[x, y]` from the top left of `shot`'s view, as the
/// PNG has them; `None` outside the view.
fn pixel_at(shot: &Shot, at: [f64; 2]) -> Option<[u8; 4]> {
    let [width, height] = shot.size;
    let [x, y] = at.map(f64::floor);
    // Negative and not finite positions were refused with the request; this keeps an index in range.
    if !(0.0..f64::from(width)).contains(&x) || !(0.0..f64::from(height)).contains(&y) {
        return None;
    }
    let index = y as usize * width as usize + x as usize;
    shot.pixels.as_chunks::<4>().0.get(index).copied()
}

/// Writes the snapshots `taken` brings, each a file of app `app`'s, whose layers are `layers`, as
/// [`snapshot::write`] writes one, whole or not at all, making the `snapshots` directory beside the
/// executable if it goes there; and answers each, until the API drops.
pub(super) fn write_snapshots(taken: &Receiver<Taken>, app: &str, layers: &[String]) {
    let layers: Vec<&str> = layers.iter().map(String::as_str).collect();
    for Taken {
        world,
        scenes,
        cached,
        open,
        scripts,
        mods,
        run,
        paused,
        ticks,
        path,
        named,
        reply,
        written: told,
    } in taken
    {
        let of = snapshot::Of {
            world: &world,
            scenes: &scenes,
            cached: &cached,
            open: &open,
            scripts: &scripts,
            mods: &mods,
            seen: None,
        };
        let written = snapshot::write_paused(&of, &run, paused, app, &layers, ticks);
        let written = written.and_then(|text| {
            if path.parent() == Some(crate::beside_exe("snapshots").as_path()) {
                fs::create_dir_all(crate::beside_exe("snapshots")).map_err(|error| {
                    let message = format!("can't make the directory of {}", path.display());
                    Error::new("snapshot.file", message).with_source(error)
                })?;
            }
            snapshot::save(&path, &text)?;
            if named && let Some(dir) = path.parent() {
                prune(dir, "snapshot", KEPT_SNAPSHOTS);
            }
            Ok(text.len() as u64)
        });
        let whole = written.is_ok();
        match written {
            Ok(bytes) => reply.result(&Snapshotted {
                path,
                ticks,
                entities: world.entities(None).map_or(0, |entities| entities.len()),
                bytes,
            }),
            Err(error) => reply.send(Err(failure(&error))),
        }
        if let Some(told) = told {
            let _ = told.send(whole);
        }
    }
}

/// Writes `shot` as an 8-bit RGB PNG at `path`, whole or not at all, making the `captures`
/// directory beside the executable if it's there: the view's pixels are sRGB-encoded, as a PNG's
/// are taken to be without a chunk that says otherwise, and its alpha goes, since the view shows
/// opaque.
///
/// Fails with `api.capture_file` if it can't.
fn write_png(path: &Path, shot: &Shot, named: bool) -> Result<()> {
    let failed = |error: Box<dyn std::error::Error + Send + Sync>| {
        let message = format!("can't write the capture {}", path.display());
        Error::new("api.capture_file", message).with_source(error)
    };
    let [width, height] = shot.size;
    let (pixels, _) = shot.pixels.as_chunks::<4>();
    let rgb: Vec<u8> = (pixels.iter())
        .flat_map(|&[r, g, b, _]| [r, g, b])
        .collect();
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let encoded = encoder.write_header().and_then(|mut writer| {
        writer.write_image_data(&rgb)?;
        writer.finish()
    });
    encoded.map_err(|error| failed(error.into()))?;
    let captures = crate::beside_exe("captures");
    if path.parent() == Some(captures.as_path()) {
        fs::create_dir_all(&captures).map_err(|error| failed(error.into()))?;
    }
    crate::dirs::write_whole(path, &data).map_err(|error| failed(error.into()))?;
    if named && let Some(dir) = path.parent() {
        prune(dir, "png", KEPT_CAPTURES);
    }
    Ok(())
}

/// Removes the files of directory `dir` that the engine named, as [`engine_named`] tells, whose
/// extension is `extension`, but the newest `keep`, by when they were last written: those a client
/// named, and the directories in it, are left alone, and a file that can't be removed, as one a
/// client reads on Windows, stays, for the next to try again.
pub(super) fn prune(dir: &Path, extension: &str, keep: usize) {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| engine_named(&entry.file_name().to_string_lossy(), extension))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    // The newest first, two written at once by their names.
    files.sort_by(|a, b| b.cmp(a));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

/// Whether a file named `name` is one the engine named for a capture or a snapshot given no path,
/// whose extension is `extension`: a run's session file's stem, its app and its process, then the
/// capture's or snapshot's number, as `sandbox-4242-7.png`.
pub(super) fn engine_named(name: &str, extension: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(extension)
        .and_then(|stem| stem.strip_suffix('.'))
    else {
        return false;
    };
    let mut parts = stem.rsplitn(3, '-');
    let numbered = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    numbered(parts.next())
        && numbered(parts.next())
        && parts.next().is_some_and(|app| !app.is_empty())
}
