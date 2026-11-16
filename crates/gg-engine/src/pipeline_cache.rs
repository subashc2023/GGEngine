//! The pipeline cache's file, in the OS's per-user cache directory. Windowed and embedded runs
//! (the editor's game restarts at each change `gg run` can't patch in) load it at start and save
//! it at end, so earlier pipelines are rebuilt in a fraction of the time where the driver keeps
//! no cache. Other headless runs and replays, as most tests, keep none, so startup times compare
//! across runs. One file per app: a run on another GPU starts empty and saves its own.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use gg_core::Error;
use gg_renderer::Renderer;
use tracing::{debug, warn};

use crate::dirs::{self, Kind};

/// The most a cache file may hold; it's read whole before the renderer checks it. Far above the
/// size where the cache, which keeps every pipeline, would need pruning.
const MAX_FILE: u64 = 1 << 30;

/// App `app`'s cache file, in [`dirs::dir`]'s cache directory. `os` is [`std::env::consts::OS`];
/// `var` reads the environment. `None` without the variables.
pub(crate) fn path(app: &str, os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    Some(dirs::dir(app, Kind::Cache, os, var)?.join("pipelines.bin"))
}

/// The file's bytes, which the renderer checks before any driver sees them. Empty if there's no
/// file yet; empty with an `engine.pipeline_cache` warning if it can't be read, isn't a regular
/// file or exceeds [`MAX_FILE`].
pub(crate) fn load(path: &Path) -> Vec<u8> {
    let _zone = gg_core::zone!();
    // One byte past the limit, so a longer file shows.
    let problem = match dirs::read_file(path, MAX_FILE + 1) {
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

/// Saves `renderer`'s pipeline cache to `path` with [`dirs::write`], so a crash mid-write leaves
/// the last file whole. A failure warns; a renderer without a cache saves nothing.
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
        // What isn't a regular file holds nothing.
        assert!(load(&dir.0).is_empty());
    }
}
