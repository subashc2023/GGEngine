//! Threads writing captures' PNGs and snapshots, and pruning their directories.

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

/// How many newest PNGs the `captures` directory beside the executable keeps. Captures without a
/// path go there; older ones go, earlier runs' too.
const KEPT_CAPTURES: usize = 32;

/// How many newest snapshots the `snapshots` directory beside the executable keeps. Snapshots
/// without a path go there. `gg run` removes its own, in a subdirectory of its own.
const KEPT_SNAPSHOTS: usize = 8;

/// A snapshot for the writing thread. `ticks` is when it was taken.
pub(super) struct Taken {
    pub(super) world: World,
    /// The scene files the layers loaded, which the restored run watches.
    pub(super) scenes: Vec<SceneFile>,
    /// The scene files' texts the run keeps for its scene changes.
    pub(super) cached: Vec<SceneFile>,
    pub(super) open: Open,
    /// The scripts the layers loaded, each a path and its text as last applied.
    pub(super) scripts: Vec<(String, String)>,
    pub(super) mods: Vec<crate::luau::ModText>,
    /// What the run was doing, with any world as edited it held; the restored run goes on.
    pub(super) run: Run,
    /// Whether the simulation was paused; the restored run then is.
    pub(super) paused: bool,
    pub(super) ticks: u64,
    pub(super) path: PathBuf,
    /// Whether the engine named the path (none given); then the directory is pruned.
    pub(super) named: bool,
    pub(super) reply: Reply,
    /// Told after the answer whether the snapshot was written, for a snapshot the run waits for.
    pub(super) written: Option<Sender<bool>>,
}

/// A capture for the PNG thread. `ticks` and `streaming` are its frame's.
pub(super) struct Png {
    pub(super) shot: Arc<Shot>,
    /// `None` for a pixel-only capture, which writes nothing.
    pub(super) path: Option<PathBuf>,
    /// The pixel whose bytes the answer gives.
    pub(super) at: Option<[f64; 2]>,
    /// Whether the engine named the path (none given); then the directory is pruned.
    pub(super) named: bool,
    pub(super) ticks: u64,
    pub(super) streaming: bool,
    pub(super) reply: Reply,
}

/// A capture, or a `frames`' frames into its directory, numbered from 1 and answered once all
/// are written.
pub(super) enum Writing {
    Capture(Png),
    Frames {
        dir: PathBuf,
        shots: Vec<FilmShot>,
        reply: Reply,
    },
}

/// Writes `pngs`' captures and frames as PNGs, frames numbered from 1 (`frame-0001.png` on).
/// Answers each once written, or at its first failure, until the API drops.
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

/// The PNG's RGBA bytes of the pixel `at` falls in (`[x, y]` from the view's top left); `None`
/// outside.
fn pixel_at(shot: &Shot, at: [f64; 2]) -> Option<[u8; 4]> {
    let [width, height] = shot.size;
    let [x, y] = at.map(f64::floor);
    // Negative and non-finite positions were refused with the request; this keeps the index valid.
    if !(0.0..f64::from(width)).contains(&x) || !(0.0..f64::from(height)).contains(&y) {
        return None;
    }
    let index = y as usize * width as usize + x as usize;
    shot.pixels.as_chunks::<4>().0.get(index).copied()
}

/// Writes `taken`'s snapshots of app `app` with `layers` ([`snapshot::write`]), whole or not at
/// all, making the `snapshots` directory if needed. Answers each, until the API drops.
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

/// Writes `shot` as an 8-bit RGB PNG at `path`, quickly compressed, whole or not at all, making the
/// `captures` directory if needed. The pixels are sRGB, as a PNG without a chunk saying otherwise
/// is taken to be. Alpha is dropped: the view shows opaque.
///
/// Fails with `api.capture_file`.
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
    // A 1280 by 720 view in 0.05 s rather than 0.17, its file some 30% larger: agents ask often.
    encoder.set_compression(png::Compression::Fast);
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

/// Removes `dir`'s engine-named files ([`engine_named`]) with `extension`, but the newest `keep` by
/// last write. Client-named files and subdirectories stay. A file that can't be removed, such as
/// one a client reads on Windows, waits for the next prune.
pub(super) fn prune(dir: &Path, extension: &str, keep: usize) {
    let entries = fs::read_dir(dir).into_iter().flatten().flatten();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| engine_named(&entry.file_name().to_string_lossy(), extension))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    // Newest first; names break ties.
    files.sort_by(|a, b| b.cmp(a));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

/// Whether `name`, with `extension`, is one the engine gives a capture or snapshot without a path:
/// the session file's stem (app and process), then the number, as in `sandbox-4242-7.png`.
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
