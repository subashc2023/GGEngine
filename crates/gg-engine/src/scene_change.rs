//! A scene changed from a game's code as it plays, as from a menu to a level, from one level to the
//! next, or to a level again from its start: a layer's tick asks for it, through
//! [`World::change_scene`], and the engine puts the scene's entities in place of the world's as
//! that tick returns, as `Context::change_asked_scene` says, so that the change is the
//! simulation's own, made alike in a replay, on every lockstep peer and in a tick run again. What
//! the run keeps of it beside the world, the scene file it watches and its open scene, follows
//! once the tick is final, as `Context::confirm_scene` says, so that a tick rollback takes back
//! leaves none of it.

use gg_core::{Error, Result};

use crate::{Entity, Saved, World};

/// The scene a layer's tick asked for, by its path from the asset root: a resource, which holds
/// none between ticks, so that no snapshot holds one, and which no hash, recording or scene reads.
#[derive(Clone, Default)]
pub(crate) struct Asked(Option<String>);

/// The scene the world's entities came from, as the last change put it in, and how many changes
/// have: a resource, so that rollback takes it back with the world, and a saved one, so that a
/// snapshot, a lockstep joiner and a client have it, but which no hash, recording or scene reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneIn {
    pub(crate) changes: u64,
    pub(crate) path: String,
}

/// The scene changes the world's ticks asked for that were refused, and the last of them: its
/// code, its scene and its message, which a run logs once the tick is final, as a change that went
/// in is; a resource, so that rollback takes it back with the world, which no hash, recording,
/// scene or snapshot reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Refused {
    pub(crate) count: u64,
    pub(crate) last: Option<(String, String, String)>,
}

impl Saved for SceneIn {
    const NAME: &'static str = "scene";

    /// Its changes and its path, a space between.
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
    /// Asks for the scene file `scene` names, its path from the asset root with `/` between its
    /// parts, as `scenes/level2.ron`, to take the place of every entity of the world's, once the
    /// tick of the layer that asks returns, before the next layer's tick: as a menu's command
    /// starts a level, a level reached its end goes on to the next, or one starts again by naming
    /// itself.
    /// Each layer's tick changes the scene once at most, to the last it asked for; a layer above
    /// may ask again in the same tick. Asked outside a tick, as in a layer's `update`, `event` or
    /// `attach`, it's dropped, warning `scene.not_in_tick`: a layer queues a command for its tick.
    ///
    /// The engine reads the file once a run, from the game's package in a run that reads one, as a
    /// build without `dev` does, and else from the disk, keeping its text for the changes to come,
    /// and those of the project's scenes folders as the run starts, so that a tick run again, a
    /// replay and each lockstep peer read the same; a development build takes a save of the file
    /// for them as an edit, at the tick a lockstep session's peers apply it, as it takes a scene
    /// file's save. It loads each texture, model, font and prefab the scene names that no layer
    /// has added, as `Context::load_scene_file` does, a texture or a font that can't be loaded
    /// standing in, warning `asset.stand_in`; a model or a prefab that can't be refuses the change.
    /// A server's session, server and clients alike, loads none, and refuses a scene that names an
    /// asset no layer loaded (`net.no_scene_change`). A snapshot taken after holds the scene's
    /// entities and the assets it loaded, so that a run restored from one comes back in the scene.
    ///
    /// Every entity goes, the game's own as well as the scene's, as a despawn takes it, so that no
    /// handle from before names one, and the scene's are spawned under the IDs it gives them; the
    /// [resources](crate::Resource) stay, as what a game carries from one scene to the next, and
    /// the world stays the world, by its [serial](Self::serial). The authors' edits applied before
    /// the change, the tool API's clients' and the Inspector's, stay in their logs, but an undo or
    /// a redo of one is refused (`ops.scene_changed`), as the scene may have put another entity
    /// under an ID it names, as a level started again does, the entry staying, with the slots it
    /// keeps, so that no peer frees one another doesn't; those applied after undo as ever. A layer
    /// that keeps entities' handles finds them again at its next tick, as by a query. Once the tick
    /// is final, as it is at once in a run alone, the run logs `scene.replaced`, and in a
    /// development build the scene's file is the scene file watched as in the world, and a
    /// project's open scene.
    ///
    /// A scene that isn't found or doesn't load changes nothing: once the tick is final, the
    /// engine logs its error, with its code, such as `scene.not_found` naming it, `scene.bad_path`
    /// for a name that isn't a path inside the asset root, `scene.case` for one whose name isn't
    /// the file's exactly, or the scene's own, as `reflect.unknown_type`, and the world stays as
    /// it was.
    pub fn change_scene(&mut self, scene: &str) {
        match self.resource_mut::<Asked>() {
            Some(Asked(asked)) => *asked = Some(scene.to_owned()),
            None => {
                self.insert_resource(Asked(Some(scene.to_owned())));
            }
        }
    }

    /// The scene a tick asked for, as [`change_scene`](Self::change_scene) says, that hasn't gone
    /// in yet: for tests of a game's ticks, which call them directly, without the engine changing
    /// the scene.
    #[doc(hidden)]
    pub fn scene_asked(&self) -> Option<&str> {
        let asked = self.resource::<Asked>();
        asked.and_then(|Asked(asked)| asked.as_deref())
    }

    /// The scene the world's entities came from, by its path from the asset root, as the last
    /// [`change_scene`](Self::change_scene) put it in; `None` before the first, as for a scene the
    /// layers loaded as they attached. A run restored from a snapshot, a lockstep joiner and a
    /// server's client have it as the world they took has it.
    pub fn scene(&self) -> Option<&str> {
        self.resource::<SceneIn>().map(|scene| scene.path.as_str())
    }

    /// Takes the scene a tick asked for, if one did.
    pub(crate) fn take_asked_scene(&mut self) -> Option<String> {
        self.resource_mut::<Asked>()
            .and_then(|Asked(asked)| asked.take())
    }

    /// How many scene changes the world's ticks asked for were refused, as [`Refused`] says.
    pub(crate) fn scenes_refused(&self) -> u64 {
        self.resource::<Refused>()
            .map_or(0, |refused| refused.count)
    }

    /// Notes that the scene change to `scene` a tick asked for was refused, with `error`.
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

    /// How many scene changes the world's entities have been through, as [`SceneIn`] says.
    pub(crate) fn scene_changes(&self) -> u64 {
        self.resource::<SceneIn>().map_or(0, |scene| scene.changes)
    }

    /// Puts scene `text`'s entities, of the scene file `path` names, in place of every entity of
    /// the world's, as [`change_scene`](Self::change_scene) says, and returns the scene's roots:
    /// the world's despawned, roots first with what's under them, in the order a query finds them,
    /// and
    /// the scene loaded as [`load_scene`](Self::load_scene) loads it. All of it, or, if the scene
    /// is refused, none.
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
