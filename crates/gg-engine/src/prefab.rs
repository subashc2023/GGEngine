//! Prefabs: `.prefab` files under the asset root, each a scene of one subtree, which a context adds
//! once, and whose entities a world spawns under each entity whose [`PrefabRoot`] names the prefab,
//! as many times as it likes, each such entity an instance that keeps what of it differs from the
//! prefab; and, in development builds, read again as their files change, every instance taking
//! what changed.

#[cfg(feature = "dev")]
use std::ffi::OsStr;
use std::path::Path;
#[cfg(feature = "dev")]
use std::sync::Arc;

use gg_core::{Error, Result};

#[cfg(feature = "dev")]
use crate::EntityId;
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

/// The folder under the asset root that a prefab saved without a path goes in.
#[cfg(feature = "dev")]
const FOLDER: &str = "prefabs";

/// The most characters the stem of a prefab's file named after its entity has: so that its name
/// and its ID file's, with a number, and the files they're written through first, with this
/// process's ID and a count after, stay well within the 255 every OS takes.
#[cfg(feature = "dev")]
const STEM: usize = 64;

/// A prefab [`Context::save_prefab`](crate::Context::save_prefab) saved: its file's path from the
/// asset root, with `/` between its parts, and the prefab, added, named by its asset ID.
#[cfg(feature = "dev")]
pub(crate) struct Saved {
    pub(crate) path: String,
    pub(crate) prefab: Prefab,
}

#[cfg(feature = "dev")]
impl crate::Context {
    /// Saves the entity of ID `id` and its descendants as a new prefab, as [`World::save_prefab`]
    /// writes it, so that linking the entity to it, as [`Operation::Link`](crate::Operation::Link)
    /// does, makes it an instance of it under the IDs its subtree holds: in a new file at `path`,
    /// absolute or from the asset root, in a folder there that the asset scan reaches, ending in
    /// `.prefab`, or else in `prefabs/` under the root, which it makes, named after the entity's
    /// [`Name`](crate::Name), in at most [`STEM`] lowercase letters, digits and `_`, or `prefab`
    /// without one, and `-2`, `-3` and so on after it while a file of that name, or its ID file, is
    /// there; whole or not at all, writing over nothing. Writes its ID file beside it, as a scan
    /// would, takes it into the asset database, and adds the prefab, of the text written, named by
    /// its ID, a new one whatever a file there gave before; or, if the ID file can't be written,
    /// warning `asset.unwritten`, by its path.
    ///
    /// Fails, writing nothing, with `engine.no_root` in a game that names no asset root;
    /// `ecs.no_entity` for an ID no living entity has; as `World::save_prefab` does, as for a
    /// subtree that wouldn't link to the text; with `engine.bad_prefab` for a text of more than 16
    /// MiB, which no load reads; with `asset.outside_root` for a path outside the root, or whose
    /// folder isn't there, as a save makes none there, or that the asset scan doesn't reach, as
    /// [`reached`](gg_api::asset::reached) says, such as one in a folder whose name starts with
    /// `.`, in a `target` or in a top-level `tests`; with `asset.exists` for a path where a file is,
    /// or the ID file an asset deleted from there left, whose ID scenes may name; and with
    /// `asset.unwritten` if the file can't be written.
    pub(crate) fn save_prefab(&mut self, id: EntityId, path: Option<&Path>) -> Result<Saved> {
        use gg_api::asset::{AssetId, AssetKind, Meta};

        let _zone = gg_core::zone!();
        let Some(root) = self.root.clone() else {
            let message = "the game names no asset root, which App::root or App::project gives it, \
                           to save a prefab under";
            return Err(Error::new("engine.no_root", message));
        };
        let world = self.world();
        let entity = world
            .entity(id)
            .ok_or_else(|| Error::new("ecs.no_entity", format!("no living entity has ID {id}")))?;
        let text = world.save_prefab(entity)?;
        if text.len() as u64 > MOST {
            let message = format!(
                "entity {id} and its descendants make a prefab's text of {} bytes, and a prefab's \
                 file is at most 16 MiB, as a load reads it whole",
                text.len()
            );
            return Err(Error::new("engine.bad_prefab", message));
        }
        let stem = match world.get::<crate::Name>(entity) {
            Some(name) => file_stem(&name.0),
            None => "prefab".to_owned(),
        };
        let outside = |given: &Path, why: &str| {
            let message = format!(
                "can't save a prefab at {}, {why}: a prefab goes in a folder under the asset root, \
                 {}",
                given.display(),
                root.display()
            );
            Error::new("asset.outside_root", message)
        };
        let unwritten = |file: &Path, error: std::io::Error| {
            let message = format!("can't write the prefab {}", file.display());
            Error::new("asset.unwritten", message).with_source(error)
        };
        let base = std::fs::canonicalize(&root).map_err(|e| unwritten(&root, e))?;
        // The folder it goes in, as the disk resolves it, which is under the root.
        let folder = |given: &Path| -> Result<std::path::PathBuf> {
            let dir = given.parent().unwrap_or(&root);
            let dir = std::fs::canonicalize(dir)
                .map_err(|_| outside(given, "whose folder isn't there, as a save makes none"))?;
            if !dir.starts_with(&base) {
                return Err(outside(given, "which is outside it"));
            }
            Ok(dir)
        };
        // The path from the root of the file `name` in `dir`, as `folder` gives it for `given`,
        // which the asset scan reaches, so that the next run finds the prefab by its ID, and its
        // package holds it.
        let scanned = |dir: &Path, name: &OsStr, given: &Path| -> Result<Arc<str>> {
            let within = dir.strip_prefix(&base).expect("checked under the root");
            let path = crate::assets::name_of(None, &within.join(name));
            if !gg_api::asset::reached(&path) {
                let why = "which the asset scan skips, as hidden or built, so that no run would \
                           find it nor a package hold it";
                return Err(outside(given, why));
            }
            Ok(path)
        };
        // Whether `file`'s ID file is there, as one an asset deleted from there left, whose ID
        // scenes may name still.
        let id_file = |file: &Path| std::fs::symlink_metadata(Meta::path(file)).is_ok();
        let (file, path) = match path {
            Some(given) => {
                let given = root.join(given);
                let name = given
                    .file_name()
                    .ok_or_else(|| outside(&given, "naming no file"))?;
                let dir = folder(&given)?;
                let path = scanned(&dir, name, &given)?;
                let file = dir.join(name);
                if id_file(&file) {
                    let message = format!(
                        "{}.meta, the ID file an asset deleted from there left, is there, and \
                         scenes may name its ID: delete it first, or save the prefab elsewhere",
                        given.display()
                    );
                    return Err(Error::new("asset.exists", message));
                }
                // Named as given, as the folder's canonical path may be one Windows spells
                // `\\?\`.
                match crate::dirs::create(&file, text.as_bytes()) {
                    Ok(()) => (file, path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let message = format!(
                            "{} is there already, and a prefab is saved as a new file, writing \
                             over none",
                            given.display()
                        );
                        return Err(Error::new("asset.exists", message));
                    }
                    Err(error) => return Err(unwritten(&given, error)),
                }
            }
            None => {
                let dir = root.join(FOLDER);
                std::fs::create_dir_all(&dir).map_err(|e| unwritten(&dir, e))?;
                let dir = folder(&dir.join(&stem))?;
                let mut count = 1;
                loop {
                    let name = match count {
                        1 => format!("{stem}.prefab"),
                        n => format!("{stem}-{n}.prefab"),
                    };
                    let given = root.join(FOLDER).join(&name);
                    let path = scanned(&dir, OsStr::new(&name), &given)?;
                    let file = dir.join(&name);
                    if id_file(&file) {
                        count += 1;
                        continue;
                    }
                    match crate::dirs::create(&file, text.as_bytes()) {
                        Ok(()) => break (file, path),
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            count += 1;
                        }
                        Err(error) => return Err(unwritten(&given, error)),
                    }
                }
            }
        };
        let catalog = self.catalog();
        let drawn = loop {
            let drawn = AssetId::draw();
            if catalog.path(drawn).is_none() {
                break drawn;
            }
        };
        let meta = Meta::path(&file);
        let written = match crate::dirs::create(&meta, Meta { id: drawn }.text().as_bytes()) {
            Ok(()) => {
                tracing::info!(
                    code = "asset.id_written",
                    path = &*path,
                    id = %drawn,
                    "Gave {path} the ID {drawn}, in its ID file {path}.meta, to commit with it"
                );
                Ok(drawn)
            }
            // Written first by the scan, which took the file for an asset without one.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Meta::read(&meta)
                .map(|meta| meta.id)
                .map_err(|f| f.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let name = match written {
            Ok(id) => {
                self.bind_asset(Arc::clone(&path), AssetKind::Prefab, id);
                id.to_string()
            }
            Err(error) => {
                tracing::warn!(
                    code = "asset.unwritten",
                    path = &*path,
                    error,
                    "Can't write {path}.meta: {error}; the prefab is named by its path"
                );
                path.to_string()
            }
        };
        let prefab = self.add_saved_prefab(&path, &name, text);
        tracing::info!(path = &*path, entity = %id, "Saved {path} as a prefab");
        Ok(Saved {
            path: path.to_string(),
            prefab,
        })
    }
}

/// A file's stem made of `name`, an entity's: its ASCII letters, lowered, and digits, each run of
/// anything else one `_`, none at either end, so that every OS takes it, cut to its first [`STEM`]
/// characters; `prefab` for none, and with `_` after a name Windows keeps for a device, as `con`.
#[cfg(feature = "dev")]
fn file_stem(name: &str) -> String {
    let mut stem = String::new();
    for c in name.chars() {
        // ASCII alone, a byte a character.
        if stem.len() == STEM {
            break;
        }
        if c.is_ascii_alphanumeric() {
            stem.push(c.to_ascii_lowercase());
        } else if !stem.is_empty() && !stem.ends_with('_') {
            stem.push('_');
        }
    }
    let stem = stem.trim_end_matches('_');
    let device = |stem: &str| {
        let numbered = |prefix: &str| {
            let digit = stem.strip_prefix(prefix);
            digit.is_some_and(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit())
        };
        ["con", "prn", "aux", "nul"].contains(&stem) || numbered("com") || numbered("lpt")
    };
    match stem {
        "" => "prefab".to_owned(),
        _ if device(stem) => format!("{stem}_"),
        _ => stem.to_owned(),
    }
}

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

#[cfg(all(test, feature = "dev"))]
mod tests {
    use std::fs;

    use gg_api::asset::Meta;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::scene_files::tests as scenes;
    use crate::{Context, Log, Name, Operation};

    /// A prefab saved without a path is named after its entity in what every OS takes, and short
    /// enough that every OS takes the files it's written through.
    #[test]
    fn a_saved_prefabs_file_is_named_after_its_entity() {
        for (name, stem) in [
            ("Big Lamp", "big_lamp"),
            ("  ..Lamp--Post 2!", "lamp_post_2"),
            ("Ünï", "n"),
            ("", "prefab"),
            ("***", "prefab"),
            ("CON", "con_"),
            ("com1", "com1_"),
            ("com10", "com10"),
        ] {
            assert_eq!(file_stem(name), stem, "{name:?}");
        }
        let cut = format!("{}lamp", "lamp_".repeat(12));
        assert_eq!(file_stem(&"Lamp ".repeat(60)), cut);
        let long = format!("{} b", "a".repeat(STEM - 1));
        assert_eq!(file_stem(&long), "a".repeat(STEM - 1), "no `_` at its end");
    }

    /// The entity of ID `n`.
    fn id(n: u64) -> EntityId {
        EntityId::new(n).expect("not zero")
    }

    /// The game of a project in `dir`, its asset root, whose start scene is a lamp, `Lamp`, of ID
    /// a01, with its bulb, a02, under it.
    fn lamp(dir: &Path) -> Context {
        scenes::project(dir, 0);
        let main = "(entities: {\"0000000000000a01\": (Name: (\"Lamp\")), \"0000000000000a02\": \
                    (Name: (\"bulb\"), Parent: (\"0000000000000a01\"))})\n";
        scenes::save(&dir.join("scenes").join("main.ron"), main, 0);
        scenes::started(dir)
    }

    /// The code `saved` was refused with.
    fn refused(saved: Result<Saved>) -> String {
        saved.err().expect("refused").code().to_owned()
    }

    /// A prefab saved where one was saved before in the run, whose file and ID file are deleted
    /// since, is a new prefab, of its new ID and the text written, which its path names from then
    /// on, and which its entity links to.
    #[test]
    fn a_prefab_saved_where_one_was_deleted_is_a_new_one() {
        let dir = Scratch::new("prefab-saved-again");
        let mut cx = lamp(&dir.0);
        let root = id(0xa01);
        let first = cx.save_prefab(root, None).expect("saved");
        let file = dir.0.join("prefabs").join("lamp.prefab");
        let meta = Meta::path(&file);
        let was = Meta::read(&meta).expect("its ID").id;
        fs::remove_file(&file).expect("deleted");
        fs::remove_file(&meta).expect("deleted");
        let world = cx.world_mut();
        let bulb = world.entity(id(0xa02)).expect("alive");
        world.despawn(bulb).expect("code may");
        let again = cx.save_prefab(root, None).expect("saved again");
        assert_eq!(again.path, "prefabs/lamp.prefab");
        assert_ne!(again.prefab, first.prefab, "a new prefab");
        let now = Meta::read(&meta).expect("its ID").id;
        assert_ne!(now, was);
        let assets = cx.world().assets();
        assert_eq!(assets.prefab_name(again.prefab), now.to_string());
        let text = fs::read_to_string(&file).expect("written");
        let held = assets.prefab_text(again.prefab).map(|text| &**text);
        assert_eq!(held, Some(&*text), "as written");
        assert_eq!(assets.prefab(&again.path), Some(again.prefab));
        let link = Operation::Link {
            entity: root,
            prefab: again.prefab,
        };
        Log::default().apply(cx.world_mut(), link).expect("linked");
        let world = cx.world();
        let saved = world.save_scene(&[world.entity(root).expect("alive")]);
        let saved = saved.expect("saved");
        assert!(saved.contains(&now.to_string()), "by its ID: {saved}");
    }

    /// A prefab is saved only where the asset scan finds it, so that the next run finds it by its
    /// ID, and a package holds it: nothing is written in a folder it skips, or hidden.
    #[test]
    fn a_prefab_is_saved_only_where_the_asset_scan_reaches() {
        let dir = Scratch::new("prefab-unscanned");
        let mut cx = lamp(&dir.0);
        let root = id(0xa01);
        for folder in ["tests", ".hidden", "sub/target"] {
            fs::create_dir_all(dir.0.join(folder)).expect("a folder");
        }
        for path in [
            "tests/x0.prefab",
            ".hidden/x1.prefab",
            "sub/target/x2.prefab",
            ".x3.prefab",
        ] {
            let saved = cx.save_prefab(root, Some(Path::new(path)));
            assert_eq!(refused(saved), "asset.outside_root", "{path}");
            assert!(!dir.0.join(path).exists(), "{path}");
        }
        let saved = cx.save_prefab(root, Some(Path::new("sub/x.prefab")));
        let path = saved.expect("saved").path;
        let scan = gg_api::asset::scan(&dir.0);
        let listed = scan.assets.iter().map(|asset| &*asset.path);
        assert_eq!(listed.collect::<Vec<_>>(), [&*path]);
    }

    /// A prefab whose text no load would read is refused before anything is written.
    #[test]
    fn a_prefab_too_big_to_load_is_refused() {
        let dir = Scratch::new("prefab-too-big");
        let mut cx = lamp(&dir.0);
        let world = cx.world_mut();
        let bulb = world.entity(id(0xa02)).expect("alive");
        let name = Name("b".repeat(MOST as usize));
        world.insert(bulb, name).expect("code may");
        let saved = cx.save_prefab(id(0xa01), None);
        assert_eq!(refused(saved), "engine.bad_prefab");
        assert!(!dir.0.join("prefabs").exists(), "nothing written");
    }

    /// The name of a prefab whose file was deleted, its ID file left, is taken, so that scenes
    /// naming its ID don't find another prefab by it: a save without a path is numbered past it,
    /// and one at its path refused.
    #[test]
    fn a_left_id_file_keeps_its_name_taken() {
        let dir = Scratch::new("prefab-left-id");
        let mut cx = lamp(&dir.0);
        let root = id(0xa01);
        cx.save_prefab(root, None).expect("saved");
        let file = dir.0.join("prefabs").join("lamp.prefab");
        let meta = Meta::path(&file);
        let was = Meta::read(&meta).expect("its ID").id;
        fs::remove_file(&file).expect("deleted, its ID file left");
        let at = Some(Path::new("prefabs/lamp.prefab"));
        assert_eq!(refused(cx.save_prefab(root, at)), "asset.exists");
        assert!(!file.exists(), "nothing written");
        let again = cx.save_prefab(root, None).expect("saved");
        assert_eq!(again.path, "prefabs/lamp-2.prefab");
        assert_eq!(Meta::read(&meta).expect("left").id, was);
    }
}
