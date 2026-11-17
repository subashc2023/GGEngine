//! A scene changed from a game's code as it plays: menu to level, level to level, or a level
//! again from its start. A layer's tick asks through [`World::change_scene`]; the engine swaps in
//! the scene's entities as that tick returns (`Context::change_asked_scene`), so the change is
//! the simulation's own, made alike in a replay, on every lockstep peer and in a tick run again.
//! What the run keeps beside the world (the watched scene file, the open scene) follows once the
//! tick is final (`Context::confirm_scene`), so a tick rollback takes back none of it.

use gg_core::{Error, Result};

use crate::ecs::Saved;
use crate::{Entity, World};

/// The scene a layer's tick asked for, by path from the asset root. A resource holding none
/// between ticks, so no snapshot holds one; no hash, recording or scene reads it.
#[derive(Clone, Default)]
pub(crate) struct Asked(Option<String>);

/// The scene the world's entities came from, as the last change put it in, and how many changes
/// have. A resource, so rollback takes it back with the world, and a saved one, so a snapshot,
/// lockstep joiner and client have it; no hash, recording or scene reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneIn {
    pub(crate) changes: u64,
    pub(crate) path: String,
}

/// The scene changes refused, and the last: code, scene and message, which a run logs once the
/// tick is final. A resource, so rollback takes it back with the world; no hash, recording, scene
/// or snapshot reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Refused {
    pub(crate) count: u64,
    pub(crate) last: Option<(String, String, String)>,
}

impl Saved for SceneIn {
    const NAME: &'static str = "scene";

    /// Its changes, a space, its path.
    fn save(&self, _: &World) -> Result<String> {
        Ok(format!("{} {}", self.changes, self.path))
    }

    fn load(text: &str, _: &World) -> Result<Self> {
        let read = text.split_once(' ').and_then(|(changes, path)| {
            let changes = changes.parse().ok()?;
            Some(Self {
                changes,
                path: path.to_owned(),
            })
        });
        read.ok_or_else(|| {
            let message = format!("`{text}` isn't a scene's changes and its path");
            Error::new("snapshot.bad", message)
        })
    }
}

impl World {
    /// Asks for the scene file `scene` (its `/`-separated path from the asset root, as
    /// `scenes/level2.ron`) to replace every entity once the asking layer's tick returns, before
    /// the next layer's tick. A menu starts a level, a level goes on to the next, or restarts by
    /// naming itself.
    ///
    /// Each layer's tick changes the scene at most once, to the last it asked for; a layer above
    /// may ask again in the same tick. Asked outside a tick (a layer's `update`, `event` or
    /// `attach`), it's dropped with warning `scene.not_in_tick`: queue a command for the tick.
    ///
    /// The engine reads the file once a run (with the project's scenes folders' files as the run
    /// starts) from the package if the run has one, else the disk, and keeps the text, so a tick
    /// run again, a replay and each lockstep peer read the same. A development build takes a save
    /// of the file as an edit, at the tick a lockstep session's peers apply it. It loads each
    /// texture, model, font and prefab the scene names that no layer added, as
    /// `Context::load_scene_file` does. A texture or font that can't load is stood in
    /// (`asset.stand_in`); a model or prefab that can't refuses the change. A server's session,
    /// server and clients alike, loads none and refuses a scene naming an asset no layer loaded
    /// (`net.no_scene_change`). A later snapshot holds the scene's entities and loaded assets.
    ///
    /// What changes:
    /// - Every entity goes, the game's own too, as a despawn takes it, so no earlier handle names
    ///   one. The scene's spawn under the IDs it gives them. A layer keeping handles finds them
    ///   again at its next tick, as by a query.
    /// - [Resources](crate::Resource) stay, carrying state between scenes. What the world keeps of
    ///   its [cells](crate::Cells) goes with its entities.
    /// - The world stays the world, by its [serial](Self::serial).
    /// - Authors' edits applied before (the tool API's clients', the Inspector's) stay in their
    ///   logs, but undoing or redoing one is refused (`ops.scene_changed`): the scene may have put
    ///   another entity under an ID it names, as a restarted level does. The entry stays, with the
    ///   slots it keeps, so no peer frees one another doesn't. Later edits undo as ever.
    /// - Once the tick is final (at once in a run alone) the run logs `scene.replaced`; in a
    ///   development build the scene's file becomes the watched scene file and the project's open
    ///   scene.
    ///
    /// A scene that isn't found or doesn't load changes nothing: once the tick is final, the
    /// engine logs the error and the world stays as it was. Codes: `scene.not_found` naming it,
    /// `scene.bad_path` for a name that isn't a path inside the asset root, `scene.case` for a
    /// name that isn't the file's exactly, or the scene's own, as `reflect.unknown_type`.
    pub fn change_scene(&mut self, scene: &str) {
        match self.resource_mut::<Asked>() {
            Some(Asked(asked)) => *asked = Some(scene.to_owned()),
            None => {
                self.insert_resource(Asked(Some(scene.to_owned())));
            }
        }
    }

    /// The scene a tick asked for, not yet gone in: for tests calling a game's ticks directly,
    /// where the engine doesn't change the scene.
    #[doc(hidden)]
    pub fn scene_asked(&self) -> Option<&str> {
        let asked = self.resource::<Asked>();
        asked.and_then(|Asked(asked)| asked.as_deref())
    }

    /// The scene the world's entities came from, by path from the asset root, as the last
    /// [`change_scene`](Self::change_scene) put it in; `None` before the first, as for a scene
    /// layers loaded as they attached. Snapshot restores, lockstep joiners and server clients
    /// have what the world they took has.
    pub fn scene(&self) -> Option<&str> {
        self.resource::<SceneIn>().map(|scene| scene.path.as_str())
    }

    pub(crate) fn take_asked_scene(&mut self) -> Option<String> {
        self.resource_mut::<Asked>()
            .and_then(|Asked(asked)| asked.take())
    }

    pub(crate) fn scenes_refused(&self) -> u64 {
        self.resource::<Refused>()
            .map_or(0, |refused| refused.count)
    }

    pub(crate) fn refuse_scene(&mut self, scene: &str, error: &Error) {
        let last = Some((
            error.code().to_owned(),
            scene.to_owned(),
            format!("{error:#}"),
        ));
        match self.resource_mut::<Refused>() {
            Some(refused) => {
                refused.count += 1;
                refused.last = last;
            }
            None => {
                self.insert_resource(Refused { count: 1, last });
            }
        }
    }

    pub(crate) fn scene_changes(&self) -> u64 {
        self.resource::<SceneIn>().map_or(0, |scene| scene.changes)
    }

    /// Puts the entities of scene `text`, from scene file `path`, in place of every entity of the
    /// world, as [`change_scene`](Self::change_scene) says, and returns the scene's roots. The
    /// world's entities are despawned, roots first with what's under them, in query order; what
    /// the world keeps of its cells is emptied with them, as [`Cells`](crate::Cells) says; the
    /// scene loads as [`load_scene`](Self::load_scene) loads it. All or, if refused, none.
    ///
    /// Fails, changing nothing, as `load_scene` does into the world without its entities.
    pub(crate) fn change_entities(&mut self, path: &str, text: &str) -> Result<Vec<Entity>> {
        let _zone = gg_core::zone!();
        self.scene_loads(text)?;
        let entities: Vec<Entity> = self.query::<Entity>().collect();
        let roots: Vec<Entity> = (entities.into_iter())
            .filter(|&entity| matches!(self.parent(entity), Ok(None)))
            .collect();
        for root in roots {
            self.despawn(root).expect("a root is alive");
        }
        self.empty_cells();
        let roots = self.load_scene(text).expect(
            "a scene that loads into the world without its entities loads once they've gone",
        );
        let changes = self.scene_changes() + 1;
        let path = path.to_owned();
        self.insert_resource(SceneIn { changes, path });
        Ok(roots)
    }
}

#[cfg(test)]
mod tests;
