//! The pipeline cache's file, which windowed runs give the renderer as it starts and save as it
//! ends, in the OS's per-user directory for caches: pipelines the GPU's driver made in an earlier
//! run are made again in a fraction of the time. Headless runs, replays and tests keep none, as
//! they keep no layout, so that their startup times compare from run to run. One file per app,
//! so a run on another GPU starts empty and saves its own.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use gg_core::Error;
use gg_renderer::Renderer;
use tracing::{debug, warn};

use crate::dirs::{self, Kind};
use crate::image;

/// The most a cache file may hold, which is read whole before the renderer checks it: far past
/// the size at which the cache, which keeps every pipeline it's given, would need pruning.
const MAX_FILE: u64 = 1 << 30;

/// The file app `app`'s pipeline cache is saved in, in the directory for caches that
/// [`dirs::dir`] says: `os` is [`std::env::consts::OS`], and `var` reads the environment. `None`
/// without the variables.
pub(crate) fn path(app: &str, os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    Some(dirs::dir(app, Kind::Cache, os, var)?.join("pipelines.bin"))
}

/// What the file at `path` holds, which the renderer checks before any driver sees it: nothing if
/// there's no file yet, as in a first run, nor if it can't be read, isn't a regular file or holds
/// more than [`MAX_FILE`], which warns `engine.pipeline_cache`.
pub(crate) fn load(path: &Path) -> Vec<u8> {
    let _zone = gg_core::zone!();
    // A byte past the most it may hold, so that a longer one shows.
    let problem = match image::read_file(path, MAX_FILE + 1) {
        Ok(data) if data.len() as u64 <= MAX_FILE => return data,
        Ok(_) => format!("it holds more than {} MiB", MAX_FILE >> 20),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => error.to_string(),
    };
    warn!(
        code = "engine.pipeline_cache",
        path = %path.display(),
        "can't load the pipeline cache from {}: {problem}; pipelines are made anew",
        path.display()
    );
    Vec::new()
}

/// Saves what `renderer`'s pipeline cache holds to `path`, as [`dirs::write`] writes, so that a
/// crash while writing leaves the last one whole. One that can't be saved warns; a renderer
/// without one saves nothing.
pub(crate) fn save(path: &Path, renderer: &Renderer) {
    let _zone = gg_core::zone!();
    let saved = renderer.pipeline_cache().and_then(|data| {
        let Some(data) = data else {
            return Ok(());
        };
        dirs::write(path, &data).map_err(|e| {
            let message = format!("can't save the pipeline cache to {}", path.display());
            Error::new("engine.pipeline_cache", message).with_source(e)
        })?;
        debug!(path = %path.display(), bytes = data.len(), "Pipeline cache saved");
        Ok(())
    });
    if let Err(error) = saved {
        warn!(
            code = error.code(),
            path = %path.display(),
            "{error:#}; the next run loads what was saved before"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::tests::Scratch;

    #[test]
    fn a_saved_cache_loads_as_it_was_and_none_is_empty() {
        let home = |name: &str| (name == "HOME").then(|| OsString::from("/home/ada"));
        assert_eq!(
            path("Sandbox", "linux", home),
            Some(Path::new("/home/ada/.cache/ggengine/Sandbox/pipelines.bin").to_owned())
        );
        let dir = Scratch::new("pipelines");
        let file = dir.0.join("Sandbox/pipelines.bin");
        assert!(load(&file).is_empty(), "there's no file yet");
        dirs::write(&file, b"cached").expect("the cache is written");
        assert_eq!(load(&file), b"cached");
        assert!(!file.with_extension("bin.new").exists());
        // What isn't a regular file holds nothing.
        assert!(load(&dir.0).is_empty());
    }
}
