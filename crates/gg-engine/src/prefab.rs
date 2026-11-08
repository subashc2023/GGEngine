//! Prefabs: `.prefab` files under the asset root, each a scene of one subtree, which a context adds
//! once, and whose entities a world spawns under each entity whose [`PrefabRoot`] names the prefab,
//! as many times as it likes, each such entity an instance that keeps what of it differs from the
//! prefab; and, in development builds, read again as their files change, every instance taking
//! what changed.

use std::path::Path;

use gg_core::{Error, Result};

use crate::package::Files;
use crate::{Component, Entity, Reflect, Transform, World};

/// The most bytes a prefab's file may have, as a scene's text is read whole.
const MOST: u64 = 16 << 20;

/// A prefab that [`Context::add_prefab`](crate::Context::add_prefab) added, or that text named and
/// the engine loaded: its index among its context's, which a [`PrefabRoot`] holds, and which
/// scenes, recordings, snapshots and the tool API name as they name a texture, by its asset ID, or
/// else its path. [`spawn`](Self::spawn) places an instance of it in a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Prefab(u32);

impl Prefab {
    /// The prefab that's `index`th among its context's.
    pub(crate) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("fewer than 2^32 prefabs"))
    }

    /// Its index among its context's prefabs.
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    /// Spawns an instance of the prefab at `at`, as [`PrefabRoot`] says, and returns its root: an
    /// entity with a `PrefabRoot` of the prefab, `at` and the prefab's root's other components, and
    /// under it the prefab's other entities. Its ID is drawn from the world's stream, and the
    /// others' are mixed from it, so that a world seeded alike spawns the prefab alike. It reads
    /// the prefab's text, which allocates, so that a game spawns a prefab as it loads a level
    /// rather than each tick.
    ///
    /// Fails, spawning nothing, as a scene that names the prefab fails to load, such as with
    /// `reflect.unknown_type` for a component type the world hasn't registered,
    /// `scene.prefab_cycle` for a prefab that links itself, or `scene.prefab_variant` for one
    /// whose root links another.
    ///
    /// # Panics
    ///
    /// If `world` doesn't share the assets of the context that added the prefab, as a world a
    /// layer makes doesn't until it's the context's.
    pub fn spawn(self, world: &mut World, at: Transform) -> Result<Entity> {
        world.spawn_prefab(self, at)
    }
}

/// The root of an instance of a prefab: an entity whose instance the world spawns as it comes with
/// one, as [`Prefab::spawn`], a scene loaded or a tool's create or add makes it, and again as it's
/// given another prefab; and takes away with it. The root takes the prefab's root's components but
/// its [`Transform`], which is its own, placing the instance; each of the prefab's other entities
/// is an entity under it, its ID mixed from the root's and the prefab's own for it, so that the
/// root's ID alone gives them wherever it's loaded, made or restored, and its references to the
/// prefab's entities are to the instance's. An entity of a prefab may itself link another, whose
/// entities' IDs mix again from its own.
///
/// An instance keeps what differs from its prefab, which a scene saves, field by field, and
/// nothing else: a field it leaves as the prefab has it takes what the prefab has, as the prefab's
/// file changes, in a development run that serves the tool API. Tools change its entities' fields
/// and give them components and children, but take none of the prefab's entities or components
/// away (`ecs.prefab_entity`); they change the root's prefab, take it away, its entities with it,
/// unlink the instance into entities of their own, and revert a field to the prefab's.
///
/// The world keeps it, as it keeps [`Parent`](crate::Parent): a scene, the tool API's operations
/// and [`Prefab::spawn`] give it, and code reads it but can't change it, since the entities under
/// it would then be another prefab's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct PrefabRoot(pub Prefab);

// And not `Writable`, so that only the world changes it, spawning its instance as it does.
impl Component for PrefabRoot {}

/// The text of prefab `name`'s file at `path`, from `files`, the disk or the game's package, whole.
///
/// Fails with `engine.bad_prefab` if it can't be read, isn't a regular file, is more than 16 MiB,
/// isn't UTF-8, or isn't a scene's RON, as far as its syntax and its entities' IDs go.
pub(crate) fn read(name: &str, path: &Path, files: &Files) -> Result<String> {
    let _zone = gg_core::zone!();
    let error = |why: String| {
        let message = format!("can't read prefab {name:?} from {}: {why}", path.display());
        Error::new("engine.bad_prefab", message)
    };
    let bytes = files
        .read(path, MOST + 1)
        .map_err(|e| error(e.to_string()))?;
    if bytes.len() as u64 > MOST {
        return Err(error("it's more than 16 MiB".to_owned()));
    }
    let text = String::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
    World::scene_ids(&text).map_err(|e| error(format!("{e:#}")))?;
    Ok(text)
}
