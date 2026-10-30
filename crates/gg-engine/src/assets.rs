//! Assets named by their paths: what scenes, snapshots, recordings and the tool API name a texture
//! by, where its handle is the renderer's own, so that a restart in another process, or another
//! machine, finds it again.

use std::collections::HashMap;
use std::path::{self, Path};
use std::sync::Arc;

use gg_renderer::Texture;

/// The textures a context added, each by its index, named by the file it was loaded from: its
/// path relative to the app's [root](crate::App::root), with `/` between its parts, or as it was
/// given for a file outside it; a model's images by the model's path and their order in it, such
/// as `assets/truck.glb#0`. The engine's white texture has no name. A context's worlds and their
/// copies share it, and what a world loads from text finds its textures here, as an entity is found
/// by its ID: a path no texture has names none.
#[derive(Clone, Debug)]
pub(crate) struct Assets {
    /// Each texture's name, by its index.
    textures: Vec<Option<Arc<str>>>,
    /// Each texture added from a file of its own, by its name: the first added, if a file was
    /// added twice.
    by_path: HashMap<Arc<str>, Texture>,
    /// What the layers added, in order, each with how many meshes, shaders and textures there
    /// were after it, as `texture 1 1 2 assets/logo.png`: what names the meshes, shaders and
    /// textures by their indices, so that what's opaque in a snapshot, which holds indices, comes
    /// back only into a run whose layers added the same. A mesh's, a shader's and a model's give
    /// the types their draws hold the bytes of too, by their names and sizes, and a shader's its
    /// blend, as `shader 1 2 1 shaders/colored.slang game::Colored 28 () 0 Opaque`, so that a draw
    /// comes back only where its bytes mean what they did.
    added: Vec<Arc<str>>,
}

impl Default for Assets {
    /// The engine's white texture alone, which every renderer's textures start with.
    fn default() -> Self {
        Self {
            textures: vec![None],
            by_path: HashMap::new(),
            added: Vec::new(),
        }
    }
}

impl Assets {
    /// What the layers added, in order, as the struct says.
    pub(crate) fn added(&self) -> &[Arc<str>] {
        &self.added
    }

    /// Records that the layers added `what`, as the struct says.
    pub(crate) fn note(&mut self, what: String) {
        self.added.push(what.into());
    }

    /// The name of `texture`, if it has one.
    pub(crate) fn path(&self, texture: Texture) -> Option<&str> {
        let name = self.textures.get(texture.index() as usize)?;
        name.as_deref()
    }

    /// The texture loaded from the file `path` names, if one was.
    pub(crate) fn texture(&self, path: &str) -> Option<Texture> {
        self.by_path.get(path).copied()
    }

    /// Names `texture`, the next texture added, loaded from the file `path` names.
    pub(crate) fn add_texture(&mut self, texture: Texture, path: Arc<str>) {
        assert_eq!(
            texture.index() as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        self.by_path.entry(path.clone()).or_insert(texture);
        self.textures.push(Some(path));
    }

    /// Names the textures added since there were `before`, a model's images, from the model's path,
    /// `path`, and their order in it.
    pub(crate) fn add_images(&mut self, path: &str, before: u32, count: u32) {
        assert_eq!(
            before as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        let images = (0..count - before).map(|image| Some(format!("{path}#{image}").into()));
        self.textures.extend(images);
    }
}

/// What names the file at `path` among assets: its path relative to `root`, with `/` between its
/// parts, if it's in `root`, and otherwise as it's given, with `/` too.
pub(crate) fn name_of(root: Option<&Path>, path: &Path) -> Arc<str> {
    let relative = root.and_then(|root| path.strip_prefix(root).ok());
    let text = relative.unwrap_or(path).to_string_lossy();
    if path::MAIN_SEPARATOR == '/' {
        text.into()
    } else {
        text.replace(path::MAIN_SEPARATOR, "/").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path inside the root is named relative to it, with `/` between its parts on every
    /// platform; one outside it as it's given.
    #[test]
    fn files_are_named_by_their_paths_from_the_root() {
        let root = Path::new("game");
        let inside = root.join("assets").join("logo.png");
        assert_eq!(&*name_of(Some(root), &inside), "assets/logo.png");
        let outside = Path::new("elsewhere").join("logo.png");
        assert_eq!(&*name_of(Some(root), &outside), "elsewhere/logo.png");
        assert_eq!(&*name_of(None, &inside), "game/assets/logo.png");
    }
}
