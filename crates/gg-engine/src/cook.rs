//! What `tools/cook` takes from the engine to write a game's package, so that what it cooks is what
//! the engine makes of each file as a run loads it: the package's [`Writer`], and a font baked, a
//! shader compiled and the files a model reads, each as a run's add makes them. Not for games.

use std::path::{Path, PathBuf};

use gg_core::Result;

use crate::package::Files;
pub use crate::package::{Kind, Writer};

/// Font `name`'s file at `path`, baked as [`Context::add_font`](crate::Context::add_font) bakes it,
/// on the job system's threads, which [`start_jobs`](crate::start_jobs) starts, as a game's package
/// holds it.
///
/// Fails as `add_font` does.
pub fn bake_font(name: &str, path: &Path) -> Result<Vec<u8>> {
    Ok(crate::font::bake(name, path)?.bytes())
}

/// Game shader `name`'s file at `path`, compiled to SPIR-V as
/// [`Context::add_shader`](crate::Context::add_shader) compiles it, as a game's package holds it,
/// with the files it read, its own first, then those it includes. A run checks it against the
/// contract for its types as it adds it.
///
/// Fails as `add_shader` does but for the contract.
pub fn compile_shader(name: &str, path: &Path) -> Result<(Vec<u8>, Vec<PathBuf>)> {
    gg_renderer::compile_shader(name, path)
}

/// The files model `name`'s file at `path` reads, its own first, then the buffers and images it
/// names, each once, which a game's package holds for it, as
/// [`Context::add_model`](crate::Context::add_model) reads them.
///
/// Fails as `add_model` does for a model that doesn't read.
pub fn model_files(name: &str, path: &Path) -> Result<Vec<PathBuf>> {
    let (_, files) = crate::gltf::read_listing(name, path, &Files::default())?;
    Ok(files)
}
