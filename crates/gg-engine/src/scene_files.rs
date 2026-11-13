//! The scene files a game loaded, through [`Context::load_scene_file`], which a development build
//! watches as it runs: one saved applies to the world as the operations that take its entities from
//! what the file held to what it holds, as [`World::scene_edit`] finds them, an edit of the file's
//! own, its author the file's path as the app's root names it, which the tool API undoes. The file
//! as last applied is what the next save is compared with, and snapshots keep it, so that a run
//! restored from one watches the files its run had loaded, and applies what changed meanwhile.
//!
//! One of them is the open scene, a project's start scene as the game loads it, which the tool
//! API's `scene` saves, empties, and opens another file in place of. It owns the entities its file
//! held as last applied, those clients' edits make into it while it's open, and those its file's
//! saves delete, as an undo may bring them back, never those the game's code makes, so that a save
//! writes the scene alone. Snapshots keep it too.
//!
//! [`Context::load_scene_file`]: crate::Context::load_scene_file
//! [`World::scene_edit`]: crate::World

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "dev")]
use std::time::{Duration, Instant, SystemTime};

use gg_core::Result;

use crate::edits::{Done, Edit};
use crate::{Context, EntityId};

/// A scene file a game loaded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SceneFile {
    /// As the game named it.
    pub(crate) path: PathBuf,
    /// Its path as the app's root names it, as assets are named: the author of its edits.
    pub(crate) author: String,
    /// Its text as it was loaded, or as last applied since.
    pub(crate) text: String,
    /// When it last changed and its size, as last looked at; `None` before the first look.
    #[cfg(feature = "dev")]
    stamp: Option<(SystemTime, u64)>,
    /// Its text as last read, which the next save is compared with: in a lockstep session a save
    /// applies at a tick to come, after which `text` is what was read.
    #[cfg(feature = "dev")]
    read: String,
}

impl SceneFile {
    pub(crate) fn new(path: PathBuf, author: String, text: String) -> Self {
        Self {
            path,
            author,
            #[cfg(feature = "dev")]
            read: text.clone(),
            text,
            #[cfg(feature = "dev")]
            stamp: None,
        }
    }
}

/// The scene files a game loaded, the open scene among them, and when a development build last
/// looked at them.
#[derive(Default)]
pub(crate) struct SceneFiles {
    pub(crate) files: Vec<SceneFile>,
    /// The scene files' texts the run keeps for the scene changes to come, each its author's, as
    /// [`Context::scene_text`] reads them: those it read, as it started or as a change first named
    /// one, and those the layers loaded, as last saved since, which a development build watches as
    /// it watches `files`, and which snapshots keep.
    pub(crate) cached: Vec<SceneFile>,
    /// The open scene of the world in the context's place: of the copy, while one plays.
    pub(crate) open: Open,
    #[cfg(feature = "dev")]
    looked: Option<Instant>,
    /// Whether the open scene of the world as edited differs from its file, as last found, and the
    /// ticks that world was at; `None` once that world or its open scene may have changed since.
    #[cfg(feature = "dev")]
    unsaved: Option<(u64, bool)>,
    /// The cells of the world as edited whose files a save would write, as last found with
    /// `unsaved`.
    #[cfg(feature = "dev")]
    unsaved_cells: Vec<crate::cells::Cell>,
    /// A text the open scene's file held as last applied, and that text in canonical form, which
    /// the scene as it would be saved is compared with; `None` for a text that no longer loads.
    #[cfg(feature = "dev")]
    canonical: Option<(String, Option<String>)>,
    /// How many times the world as edited, or its open scene, may have changed, as [`changed`]
    /// counts them.
    ///
    /// [`changed`]: Self::changed
    #[cfg(feature = "dev")]
    pub(crate) changes: u64,
}

impl SceneFiles {
    /// Keeps `file`, in place of one of the same path.
    pub(crate) fn loaded(&mut self, file: SceneFile) {
        self.files.retain(|kept| kept.path != file.path);
        self.files.push(file);
    }

    /// Keeps `file`'s text for the scene changes to come, in place of one of the same author's, as
    /// last read for the watch's next look, which the file's next save is compared with.
    pub(crate) fn cache(&mut self, file: SceneFile) {
        cache(&mut self.cached, file);
    }

    /// Forgets whether the open scene of the world as edited differs from its file, as
    /// [`unsaved`] last found, since that world, or its open scene, may have changed: as an edit
    /// applies, a scene file's save applies, or the world is restored.
    #[cfg(feature = "dev")]
    pub(crate) fn changed(&mut self) {
        self.unsaved = None;
        self.changes += 1;
    }
}

/// Keeps `file` among `cached`, texts kept for scene changes, as [`SceneFiles::cache`] says.
fn cache(cached: &mut Vec<SceneFile>, file: SceneFile) {
    match cached.iter_mut().find(|kept| kept.author == file.author) {
        #[cfg(feature = "dev")]
        Some(kept) => {
            kept.read.clone_from(&file.text);
            kept.text = file.text;
        }
        #[cfg(not(feature = "dev"))]
        Some(kept) => kept.text = file.text,
        None => cached.push(file),
    }
}

/// The open scene: which of the scene files is its file, if it's titled, and the entities it owns
/// beyond those its file's text holds as last applied, which it owns too: a save writes those of
/// either that are alive.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Open {
    /// Its file's author, one of the scene files'; `None` while it's untitled, as after the tool
    /// API's `scene new`, or in a game that loaded no start scene.
    pub(crate) author: Option<String>,
    /// The entities it owns that its file's text doesn't hold: those clients' edits made into it,
    /// those a save left out as they were gone, and those its file's saves took out of the text;
    /// each once, in ID order, those gone kept, as an undo may bring them back.
    pub(crate) made: Vec<EntityId>,
    /// Whether an author's edit has applied to its world since it was last saved, opened or made
    /// new, or its file's save applied: what the tool API's status calls `edited`. Ticks never set
    /// it, nor a script's reload; an undo back to the saved scene leaves it set. A snapshot keeps
    /// it, as the world as edited that a copy plays in place of keeps its own.
    pub(crate) edited: bool,
}

/// The open scene of a world whose snapshot holds none, as one written before the open scene was:
/// the start scene's file, `start`, if `files` holds it, and else untitled.
pub(crate) fn opened(start: Option<&Path>, files: &[SceneFile]) -> Open {
    let start = start.and_then(|start| files.iter().find(|file| same_file(&file.path, start)));
    Open {
        author: start.map(|file| file.author.clone()),
        made: Vec::new(),
        edited: false,
    }
}

/// Checks that each part of `path`, a path from `root` with `/` between its parts, is named on the
/// disk exactly as it's given, byte for byte, so that a disk that ignores case or normalizes
/// Unicode, as macOS's and Windows' do, finds no file that a package, or Linux's disk, wouldn't.
///
/// Fails with the path as the disk names it, where each part that isn't named exactly is named
/// alike in Unicode's lower case, or with `None` for a part that isn't there even so.
pub(crate) fn named_exactly(root: &Path, path: &str) -> Result<(), Option<String>> {
    let mut dir = root.to_owned();
    let mut found = Vec::new();
    let mut exact = true;
    for part in path.split('/') {
        let names = fs::read_dir(&dir).map_err(|_| None)?;
        let names: Vec<String> = (names.flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let name = match names.iter().find(|name| *name == part) {
            Some(name) => name,
            None => {
                exact = false;
                let part = part.to_lowercase();
                let other = names.iter().find(|name| name.to_lowercase() == part);
                other.ok_or(None)?
            }
        };
        dir.push(name);
        found.push(name.clone());
    }
    if exact {
        Ok(())
    } else {
        Err(Some(found.join("/")))
    }
}

/// Whether paths `a` and `b` name the same file, compared canonical, as [`canonical`] makes them,
/// as one may be named in another form, such as Windows' `\\?\` one, or through a link.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

/// `path` canonical: the file's own canonical path; or, for a file that isn't there, as one to be
/// written, its directory's, joined with its name; or else as it's given.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let parent = path.parent().and_then(|dir| fs::canonicalize(dir).ok());
        match (parent, path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path.to_owned(),
        }
    })
}

/// Applies `edit`, a client's, through `apply`, and has the entities an apply created and
/// duplicated go into `cx`'s open scene, as only a client's edit makes into it, never the game's
/// code, nor a scene file's save, whose entities its text holds. What an undo brings back makes
/// nothing: the scene owns it already if it owned it as it went, those its file's save took out of
/// the text among them, which it owns from the save on. In a game that isn't a project's, which
/// has no open scene, nothing goes in.
///
/// Fails as `apply` does.
pub(crate) fn client_edit(
    cx: &mut Context,
    edit: Edit,
    apply: impl FnOnce(&mut Context, Edit) -> Result<Done>,
) -> Result<Done> {
    let done = apply(cx, edit);
    if cx.start.is_some()
        && let Ok(Done::Applied(created)) = &done
        && !created.is_empty()
    {
        let made = &mut cx.scenes.open.made;
        made.extend(created);
        made.sort_unstable();
        made.dedup();
    }
    done
}

/// How long a development build waits between looks at the scene files: a save applies on the next
/// look, a frame's start, at most this long after.
#[cfg(feature = "dev")]
const LOOK: Duration = Duration::from_millis(200);

#[cfg(feature = "dev")]
mod watch {
    use std::collections::HashSet;
    use std::fs;
    use std::mem;
    use std::time::Instant;

    use tracing::{info, warn};

    use gg_core::{Error, Result};

    use super::{LOOK, SceneFile};
    use crate::ecs::SceneEdit;
    use crate::edits::{Done, Edit};
    use crate::layer::Stack;
    use crate::{Context, EntityId, World};

    /// As a frame starts, between ticks, applies the scene files saved since the last look, at
    /// most one a [`LOOK`]: each as the edit that takes the world's entities from the file's text as
    /// last applied to its text now, through `stack`, which records it, logging `scene.applied`.
    /// One that doesn't load, or whose edit the world refuses, warns with the error's code and
    /// changes nothing, and the next save is compared with the file as last applied. In a lockstep
    /// session, the host offers each save for a tick both peers apply it before, as
    /// [`Stack::offer_save`] says, and a joiner doesn't look, nor does a run that reads its game's
    /// package, whose scenes aren't files.
    pub(crate) fn look(cx: &mut Context, stack: &mut Stack) {
        let now = Instant::now();
        if !stack.watches_files()
            || cx.reads_package()
            || (cx.scenes.files.is_empty() && cx.scenes.cached.is_empty())
            || cx
                .scenes
                .looked
                .is_some_and(|looked| now.duration_since(looked) < LOOK)
        {
            return;
        }
        cx.scenes.looked = Some(now);
        let mut saves: Vec<(String, String)> = Vec::new();
        // The files in the world first, and then the texts kept for scene changes, which a save
        // of one of the former takes too.
        let scenes = &mut cx.scenes;
        for file in scenes.files.iter_mut().chain(&mut scenes.cached) {
            let stamp = fs::metadata(&file.path)
                .and_then(|meta| Ok((meta.modified()?, meta.len())))
                .ok();
            if stamp.is_none() || stamp == file.stamp {
                continue;
            }
            // Being written, or held by its writer: a later look reads it, its stamp unchanged.
            let Ok(text) = crate::read_scene(&file.path) else {
                continue;
            };
            file.stamp = stamp;
            if text != file.read {
                file.read.clone_from(&text);
                if !saves.iter().any(|(author, _)| *author == file.author) {
                    saves.push((file.author.clone(), text));
                }
            }
        }
        // With the files in place, which a save finds its own among as it applies, now or at a
        // tick to come.
        for (author, text) in saves {
            stack.offer_save(cx, author, text);
        }
        // A cell's file made or taken away since: its cell comes in from it, or no longer does.
        // Not in a run that can't stop, whose recording or session lists it once.
        if stack.refuses_stop().is_none() {
            cx.list_cells_again();
        }
    }

    /// Applies the save of the scene file whose author is `author`, its text now `text`, through
    /// `edit`, which applies an edit to `cx`'s world and records it, as [`look`] says; and keeps
    /// the text for the scene changes to come, through an edit too, [`Edit::Scene`], so that a
    /// replay and each lockstep peer keep it at the same tick: of a file in the world once it has
    /// applied, and of one kept for them alone, as it's saved. A kept file that's a cell's, whose
    /// cell is in and whose file came in, as [`World::filed_cell_in`] says, applies to the world as
    /// a file in it does, its roots in both texts given `Streamed`, as the cell's load gives them.
    ///
    /// Fails with `scene.not_watched` for an author no scene file has, and as [`apply`] does.
    pub(crate) fn apply_save(
        cx: &mut Context,
        edit: &mut dyn FnMut(&mut Context, Edit) -> Result<Done>,
        author: &str,
        text: String,
    ) -> Result<Done> {
        // An edit of the file's author, which isn't the open scene's authors' unless it's the open
        // scene's file, whose save leaves the scene as saved.
        let edited = cx.scenes.open.edited && cx.scenes.open.author.as_deref() != Some(author);
        let mut files = mem::take(&mut cx.scenes.files);
        let kept = Edit::Scene {
            path: author.to_owned(),
            text: text.clone(),
        };
        let cached = cx.scenes.cached.iter().find(|file| file.author == author);
        let cell = cached.filter(|_| cx.world().filed_cell_in(author).is_some());
        let mut cell = cell.cloned();
        let cached = cached.is_some();
        let applied = match files.iter_mut().find(|file| file.author == author) {
            Some(file) => apply(cx, edit, file, text, false),
            None if let Some(file) = &mut cell => apply(cx, edit, file, text, true),
            None if cached => Ok(()),
            None => Err(Error::new(
                "scene.not_watched",
                format!("no scene file the layers loaded is {author}"),
            )),
        };
        let applied = match applied {
            Ok(()) if cached => edit(cx, kept).map(drop),
            applied => applied,
        };
        files.append(&mut cx.scenes.files);
        cx.scenes.files = files;
        // Its text as last applied, which the open scene is compared with, may be another.
        cx.scenes.changed();
        cx.scenes.open.edited = edited;
        applied.map(|()| Done::Applied(Vec::new()))
    }

    /// Applies the edit that takes `cx`'s world from `file`'s text to `text`, its text now, through
    /// `edit`, as [`look`] says, each text's roots given `Streamed` first for a cell's file,
    /// `cell`; of the open scene's file, the scene owning, from then on, the entities it takes out
    /// of the text, deleted or gone already.
    ///
    /// Fails, having warned, as the text's load or the edit does.
    fn apply(
        cx: &mut Context,
        edit: &mut dyn FnMut(&mut Context, Edit) -> Result<Done>,
        file: &mut SceneFile,
        text: String,
        cell: bool,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
        // What it names that isn't loaded is loaded first, as a scene's load loads it; a text that
        // doesn't load is refused below.
        let _ = cx.load_scene_assets(&text);
        let path = file.path.display();
        let author = || file.author.clone();
        // The file as last applied, unless it no longer loads, as after a rebuild took a field away
        // that it holds: the world's entities stand for it then.
        let old = match cx.world().scene_loads(&file.text) {
            Ok(()) => Some(file.text.clone()),
            Err(error) => {
                warn!(
                    code = "scene.old_text",
                    path = %path,
                    "{path}: the file as last applied no longer loads, so its save applies as it \
                     is, each field of its entities that the world holds otherwise set, and \
                     nothing removed or deleted: {error:#}"
                );
                None
            }
        };
        // The open scene's entities that its file's save takes out of the text, which it owns from
        // then on, as an undo brings them back: each the text as last applied holds and the text
        // now doesn't, though the edit deletes only the top-most of them, the rest with it; and
        // those already gone, as a client's undo of its delete brings one back. A child the game
        // made under one, which no text holds, it never owns.
        let mut deleted = Vec::new();
        if cx.start.is_some()
            && cx.scenes.open.author.as_deref() == Some(file.author.as_str())
            && let Some(old) = &old
            && let Ok(now) = World::scene_ids(&text)
        {
            let now: HashSet<EntityId> = now.into_iter().collect();
            // A text as last applied loaded then, so it reads as a scene's RON.
            let held = World::scene_ids(old).unwrap_or_default().into_iter();
            deleted.extend(held.filter(|id| !now.contains(id)));
        }
        // A cell's roots stream in the world, as its load gave them `Streamed`.
        let (old, new) = match cell {
            true => {
                let world = cx.world();
                let old = old.and_then(|old| world.streamed_text(&old).ok());
                (old, world.streamed_text(&text))
            }
            false => (old, Ok(text.clone())),
        };
        let mut operations = 0;
        let applied = new.and_then(|new| {
            let found = cx.world().scene_edit(old.as_deref(), &new)?;
            match found {
                SceneEdit::Changes(changes) => {
                    operations = changes.len();
                    if changes.is_empty() {
                        return Ok(());
                    }
                    let apply = Edit::Apply {
                        author: author(),
                        operations: changes,
                    };
                    edit(cx, apply).map(drop)
                }
                SceneEdit::Creates(creates) => {
                    operations = creates.len();
                    let apply = Edit::Apply {
                        author: author(),
                        operations: creates,
                    };
                    edit(cx, apply)?;
                    // With the entities made, the rest, which may hold them; refused, the append
                    // undoes the creates too.
                    let rest = match cx.world().scene_edit(old.as_deref(), &new) {
                        Ok(SceneEdit::Changes(rest)) => rest,
                        Ok(SceneEdit::Creates(_)) => unreachable!("every entity is made"),
                        Err(error) => {
                            let _ = edit(cx, Edit::Undo { author: author() });
                            return Err(error);
                        }
                    };
                    operations += rest.len();
                    let append = Edit::Append {
                        author: author(),
                        operations: rest,
                    };
                    edit(cx, append).map(drop)
                }
            }
        });
        match &applied {
            Ok(()) => {
                info!(
                    code = "scene.applied",
                    path = %path,
                    operations,
                    "Applied the scene file's changes to the world"
                );
                file.text = text;
                let made = &mut cx.scenes.open.made;
                made.extend(deleted);
                made.sort_unstable();
                made.dedup();
            }
            Err(error) => {
                warn!(
                    code = error.code(),
                    path = %path,
                    "{path}: {error:#}; the world stays as it was, and the file's next save is \
                     compared with it as last applied"
                );
            }
        }
        applied
    }
}

#[cfg(feature = "dev")]
pub(crate) use watch::{apply_save, look};

#[cfg(feature = "dev")]
mod open_scene {
    use std::borrow::Cow;
    use std::collections::HashSet;
    use std::fs;
    use std::mem;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use tracing::info;

    use gg_core::{Error, Result};

    use super::{Open, SceneFile, same_file};
    use crate::cells::{Cell, CellState, MadeCells};
    use crate::ecs::{SceneEdit, text_digest};
    use crate::layer::Stack;
    use crate::{Context, Entity, EntityId, Operation, World};
    use crate::{package, perf};

    /// The scene file that `path`, absolute or relative to the asset root, names for the tool
    /// API's `scene`: its path rebuilt from the asset root, and its author, its path from the asset
    /// root with `/` between its parts, such as `scenes/level2.ron`, so that it's named as the
    /// game names its scene files, in whatever form it's given. A file to open, `existing`, must be
    /// there; a file to save as, its directory, since a save makes none. Each is compared canonical
    /// with the project's scenes folders.
    ///
    /// Fails with `engine.no_project` in a game that isn't a project's; with `scene.file`, naming
    /// the path, for a file to open that isn't there, or a directory to save in that isn't; and
    /// with `scene.bad_path` for a path outside the scenes folders.
    pub(crate) fn resolve(cx: &Context, path: &Path, existing: bool) -> Result<(PathBuf, String)> {
        let _zone = gg_core::zone!();
        let (Some(root), Some(_)) = (&cx.root, &cx.start) else {
            return Err(no_project());
        };
        let given = root.join(path);
        let found = if existing {
            fs::canonicalize(&given).map_err(|error| {
                let message = format!("can't read the scene from {}", given.display());
                Error::new("scene.file", message).with_source(error)
            })
        } else {
            let (dir, name) = (given.parent(), given.file_name());
            let dir = dir.map(fs::canonicalize).transpose();
            match (dir, name) {
                (Ok(Some(dir)), Some(name)) => Ok(dir.join(name)),
                (dir, _) => {
                    let message = format!(
                        "can't save the scene to {}, whose directory isn't there: a save makes \
                         none",
                        given.display()
                    );
                    let error = Error::new("scene.file", message);
                    Err(match dir {
                        Err(source) => error.with_source(source),
                        Ok(_) => error,
                    })
                }
            }
        }?;
        let mut folders = (cx.folders.iter()).filter_map(|folder| fs::canonicalize(folder).ok());
        let inside = folders.any(|folder| found.starts_with(folder));
        let root_found = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let rest = found.strip_prefix(&root_found).ok().filter(|_| inside);
        let Some(rest) = rest else {
            let folders = cx.folders.iter();
            let named = folders.map(|folder| crate::assets::name_of(Some(root), folder));
            let message = format!(
                "{} isn't in one of the project's scenes folders, {}, where the game's scenes are",
                given.display(),
                crate::reflect::list(named)
            );
            return Err(Error::new("scene.bad_path", message));
        };
        let parts = rest.iter().map(|part| part.to_string_lossy());
        let author = parts.collect::<Vec<_>>().join("/");
        Ok((root.join(rest), author))
    }

    /// The error that a game isn't a project's, which has no open scene.
    pub(crate) fn no_project() -> Error {
        let message = "the game isn't a project's, which has scenes to open and save: \
                       `App::project` makes it one";
        Error::new("engine.no_project", message)
    }

    /// The author of the open scene's file, of the world as edited: the file the tool API's
    /// `scene` saves, as its status names it; `None` while it's untitled.
    pub(crate) fn title(cx: &Context) -> Option<&str> {
        as_edited(cx).2.author.as_deref()
    }

    /// The world as edited, its scene files and its open scene: those held aside while a copy
    /// plays, as [`Stack::play`] holds them, and else the context's own.
    fn as_edited(cx: &Context) -> (&World, &[SceneFile], &Open) {
        match &cx.edited {
            Some(edited) => (&edited.world, edited.scenes.as_slice(), &edited.open),
            None => (cx.world(), cx.scenes.files.as_slice(), &cx.scenes.open),
        }
    }

    /// The texts kept for scene changes of the world as edited, as [`as_edited`] says: those held
    /// aside while a copy plays, and else the context's own.
    fn cached_as_edited(cx: &Context) -> &[SceneFile] {
        match &cx.edited {
            Some(edited) => &edited.cached,
            None => &cx.scenes.cached,
        }
    }

    /// The file of open scene `open` among `files`, if it's titled.
    fn file_of<'f>(files: &'f [SceneFile], open: &Open) -> Option<&'f SceneFile> {
        let author = open.author.as_deref()?;
        files.iter().find(|file| file.author == author)
    }

    /// The IDs open scene `open` owns, alive or not: those its file among `files` held as last
    /// applied, read from the text's syntax alone, so that a text that no longer loads, as after a
    /// rebuild took away a field it holds, still says what it holds; and those it made; each once,
    /// in ID order.
    fn claimed(files: &[SceneFile], open: &Open) -> Vec<EntityId> {
        let text = file_of(files, open).map(|file| file.text.as_str());
        // A text as last applied loaded then, so it reads as a scene's RON.
        let mut ids = text.map_or_else(Vec::new, |text| World::scene_ids(text).unwrap_or_default());
        ids.extend(&open.made);
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The entities of `world` that open scene `open`, of scene files `files`, owns: those of
    /// [`claimed`]'s IDs that are alive, in ID order.
    pub(super) fn owned(world: &World, files: &[SceneFile], open: &Open) -> Vec<Entity> {
        let ids = claimed(files, open).into_iter();
        ids.filter_map(|id| world.entity(id)).collect()
    }

    /// A world's streamed roots, each with its cell and ID, in order, as
    /// [`World::streamed_roots`] gives them.
    type Roots = Vec<(Cell, EntityId, Entity)>;

    /// Of `world`, if its cells stream, as [`Cells`](crate::Cells) says: their folder, and the
    /// streamed roots. `None` for a world without, or whose cells' size isn't finite and positive,
    /// whose streamed roots stream nowhere, and are saved as the rest are.
    fn streaming(world: &World) -> Option<(&str, Roots)> {
        let (_, cells, _) = world.world_cells()?;
        let size = cells.size;
        if !(size.is_finite() && size > 0.0) {
            return None;
        }
        let mut roots = Vec::new();
        world.streamed_roots(size, &mut roots);
        Some((cells.folder.as_str(), roots))
    }

    /// `roots`, streamed roots of `world`, and everything under them.
    fn under(world: &World, roots: &[(Cell, EntityId, Entity)]) -> HashSet<Entity> {
        let mut walk: Vec<Entity> = roots.iter().map(|&(.., root)| root).collect();
        let mut under = HashSet::new();
        while let Some(entity) = walk.pop() {
            under.insert(entity);
            walk.extend(world.children(entity).expect("alive"));
        }
        under
    }

    /// The entities of `world` that open scene `open`, of scene files `files`, and its world's
    /// cells hold, so that a save writes them and `scene new` and `open` delete them: those it owns,
    /// as [`owned`] says; and, in a world whose cells stream, as [`streaming`] says, the streamed
    /// roots and everything under them, which the cells' files hold, and the entities alive that
    /// the files of the cells in held as last applied, of `cached`, the texts kept for scene
    /// changes, as one whose `Streamed` was taken away, which the world's file holds from its next
    /// save on. Returns them, in ID order, and those the cells' files hold.
    fn held(
        world: &World,
        files: &[SceneFile],
        (cached, made): (&[SceneFile], Option<&MadeCells>),
        open: &Open,
    ) -> (Vec<Entity>, HashSet<Entity>) {
        let mut held = owned(world, files, open);
        let Some((folder, roots)) = streaming(world) else {
            return (held, HashSet::new());
        };
        let empty = CellState::default();
        let state = world.resource::<CellState>().unwrap_or(&empty);
        for &cell in state
            .cells_in
            .iter()
            .filter(|cell| state.files.contains_key(cell))
        {
            let Some(text) = applied(folder, cell, cached, made) else {
                continue;
            };
            // A text as last applied loaded then, so it reads as a scene's RON.
            let ids = World::scene_ids(&text).unwrap_or_default();
            held.extend(ids.into_iter().filter_map(|id| world.entity(id)));
        }
        let under = under(world, &roots);
        held.extend(&under);
        held.sort_unstable_by_key(|&entity| world.id(entity).expect("alive"));
        held.dedup();
        (held, under)
    }

    /// The text cell `cell`'s file held as last applied, of folder `folder`, as it came in: its text
    /// among `cached`, the texts kept for scene changes, or, for a cell without, what the game
    /// makes of it, `made`, if it makes it.
    fn applied<'a>(
        folder: &str,
        cell: Cell,
        cached: &'a [SceneFile],
        made: Option<&MadeCells>,
    ) -> Option<Cow<'a, str>> {
        let author = format!("{folder}/{cell}.ron");
        match cached.iter().find(|file| file.author == author) {
            Some(file) => Some(Cow::Borrowed(file.text.as_str())),
            None => (made.filter(|made| made.makes(cell))).map(|made| (made.text)(cell).into()),
        }
    }

    /// What the game makes of the cells of `world`, `cx`'s or the one it holds as edited, if it
    /// makes some.
    fn made_for<'a>(cx: &'a Context, world: &World) -> Option<&'a MadeCells> {
        let (_, cells, _) = world.world_cells()?;
        cx.cells.made(&cells.folder)
    }

    /// What a save of the world as edited writes to its cells' files, in a world whose cells
    /// stream, as [`streaming`] says, and else nothing: of each cell in, and each that a streamed
    /// root is in, whose file would change, as [`World::cell_written`] finds it, the cell, and its
    /// file, under the asset root `root`, as written, or why the save is refused it. A cell's file
    /// as last applied is as [`applied`] gives it: so a cell the game made gets a file once it
    /// changes, which stands for the game's making of it from then on.
    ///
    /// Refuses a cell whose file would change with `scene.changed`, naming the file, while it holds
    /// another text than the one last applied, as the open scene's file is refused; and with
    /// `scene.cell_out`, naming the file, for a file that's there but hasn't come in, as a cell
    /// refused, or not yet in, that a root was moved into, whose entities a save would lose.
    ///
    /// Fails with `scene.no_root` in a run without an asset root, and as `cell_written` fails.
    fn cell_saves(
        world: &World,
        (cached, made): (&[SceneFile], Option<&MadeCells>),
        root: Option<&Path>,
    ) -> Result<Vec<(Cell, Result<SceneFile>)>> {
        let Some((folder, roots)) = streaming(world) else {
            return Ok(Vec::new());
        };
        let Some(root) = root else {
            let message = "there's no asset root to save the world's cells' files under";
            return Err(Error::new("scene.no_root", message));
        };
        let empty = CellState::default();
        let state = world.resource::<CellState>().unwrap_or(&empty);
        let rooted = roots.iter().map(|&(cell, ..)| cell);
        let mut cells: Vec<Cell> = state.cells_in.iter().copied().chain(rooted).collect();
        cells.sort_unstable();
        cells.dedup();
        let mut saves = Vec::new();
        for cell in cells {
            let author = format!("{folder}/{cell}.ron");
            let path = package::under(root, &author);
            let ours: Vec<Entity> = (roots.iter())
                .filter(|&&(at, ..)| at == cell)
                .map(|&(.., root)| root)
                .collect();
            let came = state.files.contains_key(&cell);
            let file = came.then(|| applied(folder, cell, cached, made)).flatten();
            let Some(text) = world.cell_written(file.as_deref(), &ours)? else {
                continue;
            };
            // What the game made of a cell has no file to compare.
            let file = file.filter(|_| cached.iter().any(|file| file.author == author));
            let refused = match file.as_deref() {
                Some(file) => crate::read_scene(&path)
                    .ok()
                    .filter(|now| now != file)
                    .map(|_| {
                        let message = format!(
                            "{} holds another text than the one the game last applied, as after an \
                         edit the game hasn't applied yet, or refused; saving would lose it",
                            path.display()
                        );
                        Error::new("scene.changed", message)
                    }),
                None if fs::metadata(&path).is_ok() => {
                    let message = format!(
                        "{} is the file of cell {cell}, which a streamed root is in, but it hasn't \
                         come in, as when it was refused, or a root was just moved into it; \
                         saving would lose what it holds: save again once it's in",
                        path.display()
                    );
                    Some(Error::new("scene.cell_out", message))
                }
                None if !came && made.is_some_and(|made| made.makes(cell)) => {
                    let message = format!(
                        "the game makes cell {cell}, which a streamed root is in, but it hasn't \
                         come in, as when it was refused, or a root was just moved into it; \
                         saving would lose what the game makes of it: save again once it's in"
                    );
                    Some(Error::new("scene.cell_out", message))
                }
                None => None,
            };
            let save = match refused {
                Some(refused) => Err(refused),
                None => Ok(SceneFile::new(path, author, text)),
            };
            saves.push((cell, save));
        }
        Ok(saves)
    }

    /// Saves the open scene of the world as edited, as the tool API's `scene save` asks: to `to`,
    /// a path and its author as [`resolve`] gives them, which becomes its file in place of the one
    /// it had, no longer watched; or to its own file. It writes exactly the entities the scene
    /// owns, as [`owned`] says, whole or not at all, and keeps the text written as its file's as
    /// last applied and as read, so that the watch's next look finds the file as read and applies
    /// nothing; and those it owns that are gone it keeps owning, as an undo may bring them back.
    /// In a world whose cells stream, the streamed roots and what's under them go to their cells'
    /// files in place of its own, and those the cells in held as last applied that no longer
    /// stream go to its own, as [`held`] says; and each cell's file that would change is written,
    /// as [`cell_saves`] says, kept as the open scene's is, the cell's file noted as come in. While
    /// a copy plays, it saves the world as edited, held aside, the copy's scene files following,
    /// and neither world applies the save. It changes no world but the cells' files' digests, and
    /// records nothing. Logs `scene.saved`, with how many cells' files it wrote.
    ///
    /// Fails, changing nothing, with `scene.untitled` for a save without `to` of an untitled scene;
    /// with `scene.watched` for a path another scene file the game loaded has; with
    /// `scene.changed`, naming the file, for a save to the open scene's file while the file holds
    /// another text than the one last applied, as after an edit the watch hasn't applied yet, or
    /// refused, so that the edit isn't lost; as `cell_saves` fails; and as
    /// [`World::save_entities`] fails; and having written the files before, as
    /// [`write_scene`](crate::write_scene) fails.
    pub(crate) fn save(cx: &mut Context, to: Option<(PathBuf, String)>) -> Result<()> {
        let _zone = gg_core::zone!();
        let cached = cached_as_edited(cx);
        let (world, files, open) = as_edited(cx);
        let file = file_of(files, open);
        let (path, author, own) = match (to, file) {
            (None, None) => {
                let message = "the open scene is untitled, as after `scene new`: save it to a path";
                return Err(Error::new("scene.untitled", message));
            }
            (None, Some(file)) => (file.path.clone(), file.author.clone(), true),
            (Some((path, _)), Some(file)) if same_file(&path, &file.path) => {
                (file.path.clone(), file.author.clone(), true)
            }
            (Some((path, author)), _) => {
                if let Some(other) = files.iter().find(|other| same_file(&path, &other.path)) {
                    return Err(watched(other));
                }
                (path, author, false)
            }
        };
        if let Some(file) = file.filter(|_| own)
            && let Ok(text) = crate::read_scene(&path)
            && text != file.text
        {
            let message = format!(
                "{} holds another text than the one the game last applied, as after an edit the \
                 game hasn't applied yet, or refused; saving would lose it",
                path.display()
            );
            return Err(Error::new("scene.changed", message));
        }
        let made = made_for(cx, world);
        let (mut owned, under) = held(world, files, (cached, made), open);
        owned.retain(|entity| !under.contains(entity));
        let cells = cell_saves(world, (cached, made), cx.root.as_deref())?.into_iter();
        let cells = cells.map(|(cell, save)| save.map(|file| (cell, file)));
        let cells: Vec<(Cell, SceneFile)> = cells.collect::<Result<_>>()?;
        let text = world.save_entities(&owned)?;
        // The cells' files first, so that a write that fails leaves an entity moved between the
        // world's file and a cell's in both, which a load refuses, rather than in neither.
        for (_, file) in &cells {
            crate::write_scene(&file.path, &file.text)?;
        }
        crate::write_scene(&path, &text)?;
        let gone = claimed(files, open).into_iter();
        let made: Vec<EntityId> = gone.filter(|&id| world.entity(id).is_none()).collect();
        let (entities, old) = (owned.len(), open.author.clone());
        let (saved, old) = (SceneFile::new(path, author, text), old.as_deref());
        let scenes = &mut cx.scenes;
        // Its text for the scene changes to come, as read, which the watch applies nothing of;
        // and its cells' files'.
        let written = || std::iter::once(&saved).chain(cells.iter().map(|(_, file)| file));
        match &mut cx.edited {
            Some(edited) => {
                follow(&mut edited.scenes, &mut edited.open, old, &saved);
                edited.open.made = made;
                edited.open.edited = false;
                follow(&mut scenes.files, &mut scenes.open, old, &saved);
                for file in written() {
                    super::cache(&mut edited.cached, file.clone());
                }
            }
            None => {
                follow(&mut scenes.files, &mut scenes.open, old, &saved);
                scenes.open.made = made;
                scenes.open.edited = false;
            }
        }
        for file in written() {
            scenes.cache(file.clone());
        }
        scenes.changed();
        cx.note_filed(
            cells
                .iter()
                .map(|(cell, file)| (*cell, text_digest(&file.text))),
        );
        info!(
            code = "scene.saved",
            path = %saved.path.display(),
            entities,
            cells = cells.len(),
            "Saved the open scene to its file"
        );
        Ok(())
    }

    /// Has `files` and `open`, a world's scene files and open scene, hold `saved`, the open
    /// scene's file as just saved, in place of the file whose author is `old`, or after the others
    /// if they hold none.
    fn follow(files: &mut Vec<SceneFile>, open: &mut Open, old: Option<&str>, saved: &SceneFile) {
        match files
            .iter()
            .position(|file| Some(file.author.as_str()) == old)
        {
            Some(at) => files[at].clone_from(saved),
            None => files.push(saved.clone()),
        }
        open.author = Some(saved.author.clone());
    }

    /// The error that `other`, a scene file the game loaded but not the open scene's, has the path
    /// asked for.
    fn watched(other: &SceneFile) -> Error {
        let message = format!(
            "{} is another scene file the game loaded, {}, which a save or an open in place of \
             the open scene would part from what the game made of it",
            other.path.display(),
            other.author
        );
        Error::new("scene.watched", message)
    }

    /// Refuses to put another scene in place of the open scene of the world as edited, as the
    /// tool API's `scene new` and `scene open` do, while it has unsaved edits, as [`unsaved`] finds
    /// them, the world as edited being at `ticks` unless a copy plays; unless `discard`.
    ///
    /// Fails with `scene.unsaved`.
    pub(crate) fn check_unsaved(cx: &mut Context, ticks: u64, discard: bool) -> Result<()> {
        if discard || !unsaved(cx, ticks) {
            return Ok(());
        }
        let message = "the open scene has unsaved edits, which this would lose: save it first, or \
                       discard them";
        Err(Error::new("scene.unsaved", message))
    }

    /// Empties the open scene, as the tool API's `scene new` asks, once a copy that played has
    /// stopped: deletes the entities it and its world's cells hold, as [`held`] says, keeping the
    /// game's own, a child of theirs made a root first, but what's under a model's node, which goes
    /// with the model's root, as [`emptying`] says; empties the world's cells, as
    /// [`Context::empty_cells`] says; starts every author's edits afresh, as [`Stack::rewrite`]
    /// says, since none of them would undo it; and leaves it untitled, its file no longer watched,
    /// and on disk as it was. Logs `scene.new`.
    ///
    /// Fails, changing nothing, as the deletes do.
    pub(crate) fn new(cx: &mut Context, stack: &mut Stack) -> Result<()> {
        let _zone = gg_core::zone!();
        let (scenes, world) = (&cx.scenes, cx.world());
        let cached = (scenes.cached.as_slice(), made_for(cx, world));
        let (emptying, entities) = emptying(world, &scenes.files, cached, &scenes.open);
        stack.rewrite(cx, |world, log| log.apply_all(world, emptying))?;
        cx.empty_cells();
        let closed = close(cx);
        info!(
            code = "scene.new",
            path = closed.as_ref().map(|path| path.display().to_string()),
            entities,
            "Emptied the open scene, which is untitled now"
        );
        Ok(())
    }

    /// A scene file to open in place of the open scene, as [`check_open`] read it: its path, its
    /// author and its text, and when the check started.
    pub(crate) struct Opening {
        path: PathBuf,
        author: String,
        text: String,
        started: Instant,
    }

    /// Checks that the scene file `file`, a path and its author as [`resolve`] gives them, can
    /// open in place of the open scene of the world as edited, as the tool API's `scene open`
    /// asks, before anything changes, and reads it; the world as edited being at `ticks` unless a
    /// copy plays.
    ///
    /// Fails with `scene.unsaved` as [`check_unsaved`] does, unless `discard`; with
    /// `scene.watched` for a file that another scene file the game loaded has, but the open one;
    /// as [`read_scene`](crate::read_scene) fails; as the text's load would, the path beginning its
    /// message; and with `ecs.id_taken` for an ID it gives that an entity neither the open scene
    /// nor its world's cells hold has, as [`held`] says, a model's node going with its root.
    pub(crate) fn check_open(
        cx: &mut Context,
        file: (PathBuf, String),
        ticks: u64,
        discard: bool,
    ) -> Result<Opening> {
        let _zone = gg_core::zone!();
        let started = Instant::now();
        let (path, author) = file;
        check_unsaved(cx, ticks, discard)?;
        let (_, files, open) = as_edited(cx);
        let mut others = files
            .iter()
            .filter(|other| open.author.as_ref() != Some(&other.author));
        if let Some(other) = others.find(|other| same_file(&path, &other.path)) {
            return Err(watched(other));
        }
        let text = crate::read_scene(&path)?;
        // What it names that isn't loaded is loaded first, as a scene's load loads it.
        let _ = cx.load_scene_assets(&text);
        let cached = cached_as_edited(cx);
        let (world, files, open) = as_edited(cx);
        world.scene_loads(&text).map_err(|error| {
            let message = format!("{}:{error}", path.display());
            error.with_message(message)
        })?;
        let cached = (cached, made_for(cx, world));
        let owned: HashSet<Entity> = held(world, files, cached, open).0.into_iter().collect();
        let theirs =
            |id: &EntityId| (world.entity(*id)).is_some_and(|e| !owned.contains(&world.owner(e)));
        if let Some(id) = World::scene_ids(&text)?.into_iter().find(theirs) {
            let message = format!(
                "{}: entity {id} is in the world already, and the open scene doesn't own it, so \
                 the game's own entity would give way to the file's",
                path.display()
            );
            return Err(Error::new("ecs.id_taken", message));
        }
        Ok(Opening {
            path,
            author,
            text,
            started,
        })
    }

    /// Opens `opening` in place of the open scene, as the tool API's `scene open` asks, once a copy
    /// that played has stopped: in one change of no author's, deletes what [`new`] deletes and
    /// makes the file's entities, as [`World::scene_edit`] finds them once those are gone, all of
    /// it or none; empties the world's cells as `new` does; starts every author's edits afresh, as
    /// [`Stack::rewrite`] says; and has the file be the open scene's, watched from the watch's next
    /// look, in place of the file it had, no longer watched. The open scene's own file opens again
    /// as it is on disk, the scene reverting to it. A world's cells come in from their files as
    /// [`Context::stream_stopped`] brings them in. Logs `scene.opened`, with how long it took from
    /// the check, the loads of what the file names among it.
    ///
    /// Fails, changing nothing, as the change does.
    pub(crate) fn open(cx: &mut Context, stack: &mut Stack, opening: Opening) -> Result<()> {
        let _zone = gg_core::zone!();
        let Opening {
            path,
            author,
            text,
            started,
        } = opening;
        let (scenes, world) = (&cx.scenes, cx.world());
        let cached = (scenes.cached.as_slice(), made_for(cx, world));
        let (emptying, _) = emptying(world, &scenes.files, cached, &scenes.open);
        stack.rewrite(cx, |world, log| {
            log.apply_all(world, emptying)?;
            // The entities first, then the rest, which may hold them, as a save applies.
            let rest = match world.scene_edit(None, &text)? {
                SceneEdit::Creates(creates) => {
                    log.append(world, creates)?;
                    world.scene_edit(None, &text)?
                }
                changes => changes,
            };
            let SceneEdit::Changes(rest) = rest else {
                unreachable!("every entity is made");
            };
            log.append(world, rest)
        })?;
        cx.empty_cells();
        close(cx);
        let entities = World::scene_ids(&text).map_or(0, |ids| ids.len());
        let file = SceneFile::new(path.clone(), author.clone(), text);
        cx.scenes.cache(file.clone());
        cx.scenes.loaded(file);
        cx.scenes.open.author = Some(author);
        info!(
            code = "scene.opened",
            path = %path.display(),
            entities,
            ms = perf::millis(started.elapsed()),
            "Opened the scene file in place of the open scene"
        );
        Ok(())
    }

    /// The operations that delete the entities of `world` that open scene `open`, of scene files
    /// `files`, and its world's cells hold, as [`held`] says of them and `cached`, and how many
    /// they hold: each child of theirs they don't hold made a root first, so that it stays, but a
    /// model's node or an entity an instance's prefab gives, which goes with its root, with
    /// whatever is under it, as when its model or its prefab changes; and then each deleted whose
    /// parent they don't hold, nor the root that a parent goes with, with its descendants, which
    /// they hold once those children are roots, but models' nodes, the entities prefabs give, and
    /// what's under them.
    fn emptying(
        world: &World,
        files: &[SceneFile],
        cached: (&[SceneFile], Option<&MadeCells>),
        open: &Open,
    ) -> (Vec<Operation>, usize) {
        let (owned, _) = held(world, files, cached, open);
        let owns: HashSet<Entity> = owned.iter().copied().collect();
        let id = |entity| world.id(entity).expect("alive");
        let mut operations = Vec::new();
        for &entity in &owned {
            // Nothing is taken out from under a node, nor an instance's entity: its undo, as an
            // open refused after the deletes has, would put it back under the node, which
            // operations refuse, and an instance takes none of its prefab's entities away.
            let theirs = |child: &Entity| !owns.contains(child) && world.owner(*child) == *child;
            let children = world.children(entity).expect("alive");
            for child in children.filter(theirs) {
                let entity = id(child);
                operations.push(Operation::Reparent {
                    entity,
                    parent: None,
                });
            }
        }
        for &entity in &owned {
            if world.owner(entity) != entity {
                continue;
            }
            let parent = world.parent(entity).expect("alive");
            let parent = parent.map(|parent| world.owner(parent));
            if parent.is_none_or(|parent| !owns.contains(&parent)) {
                let entity = id(entity);
                operations.push(Operation::Delete { entity });
            }
        }
        (operations, owned.len())
    }

    /// Has the open scene's file, if it has one, leave the scene files, unwatched, and the open
    /// scene be untitled, owning nothing; returns the file's path.
    fn close(cx: &mut Context) -> Option<PathBuf> {
        let author = mem::take(&mut cx.scenes.open).author?;
        let files = &mut cx.scenes.files;
        let at = files.iter().position(|file| file.author == author)?;
        Some(files.remove(at).path)
    }

    /// Whether the open scene of the world as edited differs from its file, as the tool API's
    /// status says, the world as edited being at `ticks` unless a copy plays: in a game that isn't
    /// a project's, never; untitled, while it owns an entity; with a file whose text as last
    /// applied no longer loads, always; and else while what [`save`] would write differs from that
    /// text, compared in canonical form, as [`World::scene_canonical`] gives it, so that a file
    /// written by hand isn't unsaved as loaded, or while the save would be refused; and while a
    /// save would write a cell's file, or be refused one, as [`cell_saves`] says. Kept until the
    /// world as edited, at its ticks, or its open scene changes, as [`SceneFiles::changed`] says,
    /// since clients ask often, and a large scene saves slowly.
    ///
    /// [`SceneFiles::changed`]: super::SceneFiles::changed
    pub(crate) fn unsaved(cx: &mut Context, ticks: u64) -> bool {
        if cx.start.is_none() {
            return false;
        }
        let ticks = cx.edited.as_ref().map_or(ticks, |edited| edited.ticks);
        if let Some((at, unsaved)) = cx.scenes.unsaved
            && at == ticks
        {
            return unsaved;
        }
        let _zone = gg_core::zone!();
        let mut canonical = mem::take(&mut cx.scenes.canonical);
        let cached = cached_as_edited(cx);
        let (world, files, open) = as_edited(cx);
        let cached = (cached, made_for(cx, world));
        let (mut owned, under) = held(world, files, cached, open);
        owned.retain(|entity| !under.contains(entity));
        let cells = cell_saves(world, cached, cx.root.as_deref());
        let cells_unsaved = cells.as_ref().map_or(true, |saves| !saves.is_empty());
        // Those it would write, or is refused; none where it fails, as a root it can't save.
        let cells = cells.map(|saves| saves.into_iter().map(|(cell, _)| cell).collect());
        let cells: Vec<Cell> = cells.unwrap_or_default();
        let unsaved = cells_unsaved
            || match file_of(files, open) {
                None => !owned.is_empty(),
                Some(file) => {
                    // Made again only as the text changes.
                    if (canonical.as_ref()).is_none_or(|(text, _)| *text != file.text) {
                        let loaded = world.scene_canonical(&file.text).ok();
                        canonical = Some((file.text.clone(), loaded));
                    }
                    let loaded = canonical.as_ref().and_then(|(_, loaded)| loaded.as_ref());
                    let saved = world.save_entities(&owned).ok();
                    loaded.is_none() || saved.as_ref() != loaded
                }
            };
        cx.scenes.canonical = canonical;
        cx.scenes.unsaved = Some((ticks, unsaved));
        cx.scenes.unsaved_cells = cells;
        unsaved
    }

    /// The cells of the world as edited whose files a save would write, or is refused, in order,
    /// as [`unsaved`] finds them, and keeps them; none where the save fails for all, as for a root
    /// it can't save, which `unsaved` is true for.
    pub(crate) fn unsaved_cells(cx: &mut Context, ticks: u64) -> &[Cell] {
        unsaved(cx, ticks);
        &cx.scenes.unsaved_cells
    }
}

#[cfg(feature = "dev")]
pub(crate) use open_scene::{
    check_open, check_unsaved, new, no_project, open, resolve, save, title, unsaved, unsaved_cells,
};

#[cfg(all(test, feature = "dev"))]
pub(crate) mod tests {
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    use gg_core::math::{DVec3, Vec3};

    use super::*;
    use crate::args::Net;
    use crate::cells::CellState;
    use crate::cells::tests::{id_of, look_at, scene as cells_scene, xs};
    use crate::dirs::tests::Scratch;
    use crate::edits::Edit;
    use crate::layer::{Offered, Stack};
    use crate::net::{DELAY, Lockstep};
    use crate::reflect::Value;
    use crate::snapshot::{self, Run, Snapshot};
    use crate::{
        Context, Entity, ModelNode, ModelRoot, Operation, Reflect, Tick, Transform, World,
    };

    /// Writes `text` at `path`, stamped `later` seconds past now, so that a look sees it changed
    /// whatever the clock's grain.
    pub(crate) fn save(path: &Path, text: &str, later: u64) {
        fs::write(path, text).expect("written");
        let at = SystemTime::now() + Duration::from_secs(later);
        let file = File::options().write(true).open(path).expect("opened");
        file.set_modified(at).expect("stamped");
    }

    /// Looks at `cx`'s scene files at once, as a frame's start does once a [`LOOK`] has gone by.
    pub(crate) fn look_now(cx: &mut Context, stack: &mut Stack) {
        cx.scenes.looked = None;
        look(cx, stack);
    }

    /// Writes a project's manifest in `dir`, a game's whose scenes are in its `scenes` folder, its
    /// start scene `scenes/main.ron`, which holds `count` entities with transforms, saved from a
    /// world of their own, which is returned with their IDs.
    pub(crate) fn project(dir: &Path, count: usize) -> (World, Vec<EntityId>) {
        fs::create_dir_all(dir.join("scenes")).expect("a directory");
        let project = gg_api::Project {
            game: "tested".to_owned(),
            assets: ".".to_owned(),
            scenes: vec!["scenes".to_owned()],
            start: "scenes/main.ron".to_owned(),
        };
        fs::write(dir.join(gg_api::Project::FILE), project.text()).expect("written");
        let mut file = World::seeded(1);
        let roots: Vec<Entity> = (0..count)
            .map(|_| file.spawn(Transform::IDENTITY))
            .collect();
        let text = file.save_scene(&roots).expect("saved");
        save(&dir.join("scenes").join("main.ron"), &text, 0);
        let ids = roots.iter().map(|&root| file.id(root).expect("alive"));
        let ids = ids.collect();
        (file, ids)
    }

    /// A context of the project in `dir`, its start scene loaded, as a layer's attach loads it.
    pub(crate) fn started(dir: &Path) -> Context {
        let mut cx = Context::seeded(2).project(dir).expect("the project reads");
        cx.load_start_scene().expect("loaded");
        cx
    }

    /// A create of an entity with a transform, its ID drawn.
    fn create() -> Operation {
        Operation::Create {
            entity: None,
            parent: None,
            components: vec![("Transform".into(), Transform::IDENTITY.to_value())],
        }
    }

    /// A set of entity `id`'s translation's x to `x`.
    fn set_x(id: EntityId, x: f32) -> Operation {
        Operation::Set {
            entity: id,
            component: "Transform".into(),
            path: "translation.x".to_owned(),
            value: Value::F64(f64::from(x)),
        }
    }

    /// Has `edit`, a client's, apply now, through `stack`, as the tool API's does, and returns what
    /// it did.
    fn client(cx: &mut Context, stack: &mut Stack, edit: Edit) -> Done {
        match stack.offer(cx, edit) {
            Offered::Now(done) => done.unwrap_or_else(|error| panic!("{error:#}")),
            Offered::Later(_) => panic!("applied now, outside a lockstep session"),
        }
    }

    /// Applies `operations` as client `author`'s edit, as [`client`] does, and returns the IDs of
    /// the entities it made.
    fn apply(
        cx: &mut Context,
        stack: &mut Stack,
        author: &str,
        operations: Vec<Operation>,
    ) -> Vec<EntityId> {
        let author = author.to_owned();
        match client(cx, stack, Edit::Apply { author, operations }) {
            Done::Applied(made) => made,
            Done::Stepped(_) => panic!("an apply applies"),
        }
    }

    /// The IDs of the entities `cx`'s open scene owns.
    fn owned_ids(cx: &Context) -> Vec<EntityId> {
        let owned = open_scene::owned(cx.world(), &cx.scenes.files, &cx.scenes.open);
        owned
            .iter()
            .map(|&entity| cx.world().id(entity).expect("alive"))
            .collect()
    }

    /// The x of entity `id`'s translation in `world`.
    fn x(world: &World, id: EntityId) -> f64 {
        let entity = world.entity(id).expect("alive");
        world
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .x
    }

    /// The authors whose edits `cx`'s authors keep.
    fn authors(cx: &Context) -> Vec<String> {
        let logs = cx.authors.logs(cx.world());
        logs.map(|(author, _)| author.to_owned()).collect()
    }

    /// A sorted copy of `ids`.
    fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
        ids.sort_unstable();
        ids
    }

    /// A project's start scene loads as a scene file, watched, its edits' author its path from the
    /// project's asset root, and is the open scene; a game that isn't a project's has none.
    #[test]
    fn the_start_scene_loads_as_a_scene_file_of_its_project() {
        let dir = Scratch::new("start-scene");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        let project = gg_api::Project {
            game: "tested".to_owned(),
            assets: ".".to_owned(),
            scenes: vec!["scenes".to_owned()],
            start: "scenes/main.ron".to_owned(),
        };
        fs::write(dir.0.join(gg_api::Project::FILE), project.text()).expect("written");
        let mut file = crate::World::seeded(1);
        let a = file.spawn(Transform::IDENTITY);
        let path = dir.0.join("scenes").join("main.ron");
        fs::write(&path, file.save_scene(&[a]).expect("saved")).expect("written");
        let mut cx = Context::seeded(2)
            .project(&dir.0)
            .expect("the project reads");
        assert_eq!(cx.root.as_deref(), Some(dir.0.as_path()));
        assert_eq!(cx.load_start_scene().expect("loaded").len(), 1);
        let files = &cx.scenes.files;
        assert_eq!(files.len(), 1);
        assert_eq!(
            (&files[0].path, files[0].author.as_str()),
            (&path, "scenes/main.ron")
        );
        let open = cx.scenes.open.author.as_deref();
        assert_eq!(open, Some("scenes/main.ron"));
        let none = Context::seeded(3).load_start_scene().err();
        assert_eq!(none.map(|error| error.code()), Some("engine.no_project"));
    }

    /// A scene file a layer loaded applies as it's saved, as an edit of its own, which an undo of
    /// its author takes back; one saved that doesn't load changes nothing, and the next save is
    /// compared with the file as last applied.
    #[test]
    fn a_scene_file_saved_applies_as_an_edit_of_its_own() {
        let dir = Scratch::new("scene-files");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        let path = dir.0.join("scenes").join("level.ron");
        let mut file = crate::World::seeded(1);
        let a = file.spawn(Transform::IDENTITY);
        let old = file.save_scene(&[a]).expect("saved");
        save(&path, &old, 0);
        let mut cx = Context::seeded(2).root(&dir.0);
        let roots = cx.load_scene_file(&path).expect("loaded");
        assert_eq!(roots.len(), 1);
        let mut stack = Stack::default();
        let look = |cx: &mut Context, stack: &mut Stack| {
            cx.scenes.looked = None;
            look(cx, stack);
        };
        // A first look sees it as loaded.
        look(&mut cx, &mut stack);
        assert_eq!(cx.authors.logs(cx.world()).count(), 0);
        let moved = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        };
        file.insert(a, moved).expect("alive");
        let new = file.save_scene(&[a]).expect("saved");
        save(&path, &new, 2);
        look(&mut cx, &mut stack);
        let at = |cx: &Context| *cx.world().get::<Transform>(roots[0]).expect("one");
        assert_eq!(at(&cx), moved);
        let logs: Vec<&str> = cx
            .authors
            .logs(cx.world())
            .map(|(author, _)| author)
            .collect();
        assert_eq!(logs, ["scenes/level.ron"]);
        assert_eq!(cx.scenes.files[0].text, new);
        let undo = Edit::Undo {
            author: "scenes/level.ron".to_owned(),
        };
        stack.edit(&mut cx, undo).expect("undone");
        assert_eq!(at(&cx), Transform::IDENTITY);
        // Broken, it changes nothing; fixed, it's compared with the file as last applied, which
        // the undo didn't change, so the world stays as it is but for what the save changes.
        save(&path, "(entities: {", 4);
        look(&mut cx, &mut stack);
        assert_eq!(at(&cx), Transform::IDENTITY);
        assert_eq!(cx.scenes.files[0].text, new);
        let turned = Transform {
            scale: Vec3::splat(2.0),
            ..moved
        };
        file.insert(a, turned).expect("alive");
        save(&path, &file.save_scene(&[a]).expect("saved"), 6);
        look(&mut cx, &mut stack);
        assert_eq!(
            at(&cx),
            Transform {
                scale: Vec3::splat(2.0),
                ..Transform::IDENTITY
            }
        );
    }

    /// A scene file saved while a copy of the world as edited plays applies to the copy, and once
    /// `stop` brings back the world as edited, which holds the file as it was last applied there,
    /// to that world too, as the file's own edit.
    #[test]
    fn a_scene_file_saved_while_a_copy_plays_applies_to_both_worlds() {
        let dir = Scratch::new("scene-files-play");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        let path = dir.0.join("scenes").join("level.ron");
        let mut file = crate::World::seeded(1);
        let a = file.spawn(Transform::IDENTITY);
        save(&path, &file.save_scene(&[a]).expect("saved"), 0);
        let mut cx = Context::seeded(2).root(&dir.0);
        let roots = cx.load_scene_file(&path).expect("loaded");
        let mut stack = Stack::default();
        let look = |cx: &mut Context, stack: &mut Stack| {
            cx.scenes.looked = None;
            look(cx, stack);
        };
        look(&mut cx, &mut stack);
        stack.play(&mut cx, 0);
        let moved = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        };
        file.insert(a, moved).expect("alive");
        save(&path, &file.save_scene(&[a]).expect("saved"), 2);
        look(&mut cx, &mut stack);
        let at = |cx: &Context| *cx.world().get::<Transform>(roots[0]).expect("one");
        assert_eq!(at(&cx), moved, "the copy");
        assert_eq!(stack.stop(&mut cx), Some(0));
        assert_eq!(
            at(&cx),
            Transform::IDENTITY,
            "the world as edited, as it was"
        );
        look(&mut cx, &mut stack);
        assert_eq!(at(&cx), moved, "the world as edited");
        let logs = cx.authors.logs(cx.world());
        let authors: Vec<&str> = logs.map(|(author, _)| author).collect();
        assert_eq!(authors, ["scenes/level.ron"]);
    }

    /// In a lockstep session, whose saves apply at a tick to come, a file put back before its
    /// save has applied is a save too, and the world ends as the file does.
    #[test]
    fn a_file_put_back_before_its_save_applies_applies_as_put_back() {
        let dir = Scratch::new("scene-files-lockstep");
        fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
        let path = dir.0.join("scenes").join("level.ron");
        let mut file = crate::World::seeded(1);
        let a = file.spawn(Transform::IDENTITY);
        let old = file.save_scene(&[a]).expect("saved");
        save(&path, &old, 0);
        let mut cx = Context::seeded(2).root(&dir.0);
        let roots = cx.load_scene_file(&path).expect("loaded");
        let mut stack = Stack::default();
        let net = Net::Host(crate::args::Listen::at("127.0.0.1:0"));
        let code = crate::net::TESTED;
        stack.lockstep(Lockstep::start(&net, code, "Tested", &[]).expect("hosting"));
        let look = |cx: &mut Context, stack: &mut Stack| {
            cx.scenes.looked = None;
            look(cx, stack);
        };
        look(&mut cx, &mut stack);
        let moved = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        };
        file.insert(a, moved).expect("alive");
        save(&path, &file.save_scene(&[a]).expect("saved"), 2);
        look(&mut cx, &mut stack);
        save(&path, &old, 4);
        look(&mut cx, &mut stack);
        for number in 0..=DELAY {
            assert!(stack.tick(&mut cx, Tick { number }));
        }
        assert_eq!(
            *cx.world().get::<Transform>(roots[0]).expect("one"),
            Transform::IDENTITY
        );
        assert_eq!(cx.scenes.files[0].text, old);
    }

    /// The open scene owns the entities its file's text holds and those clients' edits make into
    /// it, but not the game's own, nor another scene file's, whose save's creates aren't a
    /// client's; one its file's save deleted it owns still, which an undo of the file's author
    /// brings back.
    #[test]
    fn the_open_scene_owns_its_files_entities_and_what_clients_make_into_it() {
        let dir = Scratch::new("open-scene-owns");
        let (mut file, ids) = project(&dir.0, 2);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let other = dir.0.join("scenes").join("other.ron");
        let mut theirs = World::seeded(3);
        let c = theirs.spawn(Transform::IDENTITY);
        save(&other, &theirs.save_scene(&[c]).expect("saved"), 0);
        cx.load_scene_file(&other).expect("loaded");
        cx.world_mut().spawn(Transform::IDENTITY);
        let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
        let mut owns = sorted([&ids[..], &made[..]].concat());
        assert_eq!(owned_ids(&cx), owns);
        assert_eq!(cx.scenes.open.made, made);
        look_now(&mut cx, &mut stack);
        theirs.spawn(Transform::IDENTITY);
        let all: Vec<Entity> = theirs.query::<Entity>().collect();
        save(&other, &theirs.save_scene(&all).expect("saved"), 2);
        look_now(&mut cx, &mut stack);
        assert_eq!(
            cx.world().query::<&Transform>().count(),
            6,
            "the other file's save applied"
        );
        assert_eq!(owned_ids(&cx), owns, "nor its create");
        let [a, b] = [ids[0], ids[1]].map(|id| file.entity(id).expect("alive"));
        file.despawn(b).expect("alive");
        save(
            &dir.0.join("scenes").join("main.ron"),
            &file.save_scene(&[a]).expect("saved"),
            4,
        );
        look_now(&mut cx, &mut stack);
        owns.retain(|&id| id != ids[1]);
        assert_eq!(owned_ids(&cx), owns, "b deleted by its file's save");
        let undo = Edit::Undo {
            author: "scenes/main.ron".to_owned(),
        };
        assert!(matches!(
            client(&mut cx, &mut stack, undo),
            Done::Stepped(Some(_))
        ));
        assert_eq!(owned_ids(&cx), sorted([&ids[..], &made[..]].concat()));
    }

    /// A save of its file that deletes an entity with a child deletes the child with it, and a
    /// child the game made under it; an undo of the file's author brings all three back, the open
    /// scene owning the two its file held, but not the game's, and a save writing them as they
    /// were, without it.
    #[test]
    fn an_undo_of_its_files_save_brings_back_what_it_deleted_owned() {
        let dir = Scratch::new("open-scene-undo-save");
        let (mut file, ids) = project(&dir.0, 1);
        let b = file.entity(ids[0]).expect("alive");
        let [a, a1] = [0, 1].map(|_| file.spawn(Transform::IDENTITY));
        file.set_parent(a1, Some(a)).expect("no cycle");
        let main = dir.0.join("scenes").join("main.ron");
        save(&main, &file.save_scene(&[a, b]).expect("saved"), 0);
        let [a_id, a1_id] = [a, a1].map(|entity| file.id(entity).expect("alive"));
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        look_now(&mut cx, &mut stack);
        let world = cx.world_mut();
        let game = world.spawn(Transform::IDENTITY);
        let a_in = world.entity(a_id).expect("loaded");
        world.set_parent(game, Some(a_in)).expect("no cycle");
        let game = world.id(game).expect("alive");
        save(&main, &file.save_scene(&[b]).expect("saved"), 2);
        look_now(&mut cx, &mut stack);
        for id in [a_id, a1_id, game] {
            assert!(cx.world().entity(id).is_none(), "{id} deleted");
        }
        assert_eq!(owned_ids(&cx), [ids[0]]);
        let undo = Edit::Undo {
            author: "scenes/main.ron".to_owned(),
        };
        client(&mut cx, &mut stack, undo);
        for id in [a_id, a1_id, game] {
            assert!(cx.world().entity(id).is_some(), "{id} back");
        }
        assert_eq!(owned_ids(&cx), sorted(vec![ids[0], a_id, a1_id]));
        open_scene::save(&mut cx, None).expect("saved");
        let mut loaded = World::seeded(5);
        loaded.register::<Transform>().expect("registered");
        loaded
            .load_scene(&crate::read_scene(&main).expect("read"))
            .expect("loaded");
        let [a_at, a1_at] = [a_id, a1_id].map(|id| loaded.entity(id).expect("saved"));
        assert_eq!(loaded.parent(a1_at).expect("alive"), Some(a_at));
        assert!(loaded.entity(game).is_none(), "the game's");
    }

    /// An entity of the open scene that a client deleted, and its file's save then took out of the
    /// text, comes back owned as the client undoes its delete, so a save doesn't leave it out.
    #[test]
    fn what_its_files_save_takes_out_while_gone_comes_back_owned() {
        let dir = Scratch::new("open-scene-undo-gone");
        let (file, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        look_now(&mut cx, &mut stack);
        let delete = Operation::Delete { entity: ids[0] };
        apply(&mut cx, &mut stack, "agent", vec![delete]);
        let main = dir.0.join("scenes").join("main.ron");
        save(&main, &file.save_scene(&[]).expect("saved"), 2);
        look_now(&mut cx, &mut stack);
        let undo = Edit::Undo {
            author: "agent".to_owned(),
        };
        client(&mut cx, &mut stack, undo);
        assert!(cx.world().entity(ids[0]).is_some(), "back");
        assert_eq!(owned_ids(&cx), [ids[0]]);
    }

    /// A save writes exactly the entities the open scene owns, as the world as edited has them, to
    /// its file, a child the game made under one of them left out, and changes no world; the
    /// watch's next look applies nothing of it, and nothing is unsaved after it. One it owned
    /// that's gone it owns still, which an undo brings back.
    #[test]
    fn a_save_writes_what_the_open_scene_owns_which_the_watch_applies_nothing_of() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("open-scene-save");
        let (_, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let a = cx.world().entity(ids[0]).expect("loaded");
        let world = cx.world_mut();
        let game = world.spawn(Transform::IDENTITY);
        let kid = world.spawn(Transform::IDENTITY);
        world.set_parent(kid, Some(a)).expect("no cycle");
        let made = apply(
            &mut cx,
            &mut stack,
            "agent",
            vec![create(), set_x(ids[0], 5.0)],
        );
        assert!(open_scene::unsaved(&mut cx, 0));
        let hash = cx.world().hash();
        open_scene::save(&mut cx, None).expect("saved");
        assert_eq!(cx.world().hash(), hash, "no world changed");
        let path = dir.0.join("scenes").join("main.ron");
        let text = crate::read_scene(&path).expect("read");
        let d = cx.world().entity(made[0]).expect("alive");
        assert_eq!(text, cx.world().save_entities(&[a, d]).expect("saved"));
        assert_eq!(cx.scenes.files[0].text, text);
        let mut loaded = World::seeded(5);
        loaded.register::<Transform>().expect("registered");
        loaded.load_scene(&text).expect("loaded");
        assert_eq!(
            (loaded.query::<Entity>().count(), x(&loaded, ids[0])),
            (2, 5.0)
        );
        for left in [game, kid] {
            let id = cx.world().id(left).expect("alive");
            assert!(loaded.entity(id).is_none(), "{id}, the game's");
        }
        assert!(!open_scene::unsaved(&mut cx, 0));
        look_now(&mut cx, &mut stack);
        assert_eq!(authors(&cx), ["agent"], "no edit of the file's");
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.iter().any(|code| code == "scene.saved"), "{codes:?}");
        assert!(
            !codes.iter().any(|code| code == "scene.applied"),
            "{codes:?}"
        );
        // d gone, its file's save leaves it out but owns it still, and an undo brings it back.
        apply(
            &mut cx,
            &mut stack,
            "agent",
            vec![Operation::Delete { entity: made[0] }],
        );
        open_scene::save(&mut cx, None).expect("saved");
        assert_eq!(cx.scenes.open.made, made);
        assert!(!open_scene::unsaved(&mut cx, 0));
        let undo = Edit::Undo {
            author: "agent".to_owned(),
        };
        client(&mut cx, &mut stack, undo);
        assert_eq!(owned_ids(&cx), sorted([ids[0], made[0]].to_vec()));
        assert!(open_scene::unsaved(&mut cx, 0));
    }

    /// A save is refused for a file the watch hasn't applied as it holds it, for an untitled scene
    /// without a path, and for another scene file's path, saving nothing; to a path, it saves the
    /// scene there, the file it's open in from then on, the one before no longer watched.
    #[test]
    fn a_save_is_refused_what_would_lose_an_edit_and_saves_as_another_file() {
        let dir = Scratch::new("open-scene-save-as");
        let (mut file, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let main = dir.0.join("scenes").join("main.ron");
        let other = dir.0.join("scenes").join("other.ron");
        save(&other, "(entities: {})\n", 0);
        cx.load_scene_file(&other).expect("loaded");
        look_now(&mut cx, &mut stack);
        // An agent's edit to the file, which the watch hasn't looked at.
        let a = file.entity(ids[0]).expect("alive");
        file.insert(
            a,
            Transform {
                translation: DVec3::X,
                ..Transform::IDENTITY
            },
        )
        .expect("alive");
        let edited = file.save_scene(&[a]).expect("saved");
        save(&main, &edited, 2);
        let refused = open_scene::save(&mut cx, None).expect_err("an edit unapplied");
        assert_eq!(refused.code(), "scene.changed", "{refused}");
        assert!(
            refused.to_string().contains(&main.display().to_string()),
            "{refused}"
        );
        assert_eq!(crate::read_scene(&main).expect("read"), edited, "kept");
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), ids[0]), 1.0);
        let resolve = |cx: &Context, path: &str| open_scene::resolve(cx, Path::new(path), false);
        let to = resolve(&cx, "scenes/other.ron").expect("in the scenes folder");
        let watched = open_scene::save(&mut cx, Some(to)).expect_err("another's");
        assert_eq!(watched.code(), "scene.watched", "{watched}");
        let to = resolve(&cx, "scenes/copy.ron").expect("in the scenes folder");
        open_scene::save(&mut cx, Some(to)).expect("saved as");
        let copy = dir.0.join("scenes").join("copy.ron");
        assert_eq!(crate::read_scene(&copy).expect("read"), edited);
        assert_eq!(open_scene::title(&cx), Some("scenes/copy.ron"));
        let watching: Vec<&str> = cx
            .scenes
            .files
            .iter()
            .map(|file| file.author.as_str())
            .collect();
        assert_eq!(watching, ["scenes/copy.ron", "scenes/other.ron"]);
        assert!(!open_scene::unsaved(&mut cx, 0));
        // An untitled scene saves to a path alone.
        open_scene::new(&mut cx, &mut stack).expect("emptied");
        let untitled = open_scene::save(&mut cx, None).expect_err("untitled");
        assert_eq!(untitled.code(), "scene.untitled", "{untitled}");
    }

    /// While a copy of the world as edited plays, a save writes the world as edited, held aside,
    /// and the copy's scene files follow: neither world applies it, then or once `stop` brings the
    /// world as edited back.
    #[test]
    fn a_save_while_a_copy_plays_writes_the_world_as_edited() {
        let dir = Scratch::new("open-scene-save-playing");
        let (_, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        look_now(&mut cx, &mut stack);
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 3.0)]);
        stack.play(&mut cx, 0);
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 7.0)]);
        open_scene::save(&mut cx, None).expect("saved");
        let text = crate::read_scene(&dir.0.join("scenes").join("main.ron")).expect("read");
        let mut loaded = World::seeded(5);
        loaded.register::<Transform>().expect("registered");
        loaded.load_scene(&text).expect("loaded");
        assert_eq!(x(&loaded, ids[0]), 3.0, "the world as edited's");
        assert_eq!(cx.scenes.files[0].text, text, "the copy's file follows");
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), ids[0]), 7.0, "the copy, as it was");
        assert_eq!(stack.stop(&mut cx), Some(0));
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), ids[0]), 3.0);
        assert_eq!(authors(&cx), ["agent"], "no edit of the file's");
    }

    /// New is refused while the open scene has unsaved edits, unless they're discarded; it deletes
    /// the entities the scene owns, keeping the game's own, a child of theirs a root, and leaves
    /// the scene untitled, its file unwatched and as it was; every author's history starts afresh,
    /// in another world, as the layers are told. Untitled, the scene is unsaved once it owns an
    /// entity.
    #[test]
    fn new_empties_the_open_scene_and_starts_history_afresh() {
        let dir = Scratch::new("open-scene-new");
        let (_, ids) = project(&dir.0, 2);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let restored = std::rc::Rc::new(std::cell::Cell::new(0));
        stack.push_layer(Counted(restored.clone()));
        let a = cx.world().entity(ids[0]).expect("loaded");
        let world = cx.world_mut();
        let game = world.spawn(Transform::IDENTITY);
        let kid = world.spawn(Transform::IDENTITY);
        world.set_parent(kid, Some(a)).expect("no cycle");
        let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
        let refused = open_scene::check_unsaved(&mut cx, 0, false).expect_err("unsaved");
        assert_eq!(refused.code(), "scene.unsaved", "{refused}");
        open_scene::check_unsaved(&mut cx, 0, true).expect("discarded");
        let main = dir.0.join("scenes").join("main.ron");
        let text = crate::read_scene(&main).expect("read");
        let serial = cx.world().serial();
        open_scene::new(&mut cx, &mut stack).expect("emptied");
        let world = cx.world();
        for id in [ids[0], ids[1], made[0]] {
            assert!(world.entity(id).is_none(), "{id}, the scene's");
        }
        assert!(world.contains(game) && world.contains(kid), "the game's");
        assert_eq!(world.parent(kid).expect("alive"), None);
        assert_ne!(world.serial(), serial);
        assert!(authors(&cx).is_empty());
        assert_eq!(restored.get(), 1);
        let undo = Edit::Undo {
            author: "agent".to_owned(),
        };
        assert_eq!(client(&mut cx, &mut stack, undo), Done::Stepped(None));
        assert_eq!((open_scene::title(&cx), cx.scenes.files.len()), (None, 0));
        assert_eq!(
            crate::read_scene(&main).expect("read"),
            text,
            "on disk as it was"
        );
        assert!(!open_scene::unsaved(&mut cx, 0));
        apply(&mut cx, &mut stack, "agent", vec![create()]);
        assert!(open_scene::unsaved(&mut cx, 0));
    }

    /// Counts the calls of its `restored`.
    struct Counted(std::rc::Rc<std::cell::Cell<u32>>);

    impl crate::Layer for Counted {
        fn restored(&mut self, _: &mut Context) {
            self.0.set(self.0.get() + 1);
        }
    }

    /// Open is refused, changing nothing, for unsaved edits not discarded, another scene file's
    /// path, a file that doesn't load, and one that gives a game's entity's ID; else it puts the
    /// file's entities in place of the open scene's, keeping the game's own, the file watched from
    /// then on in place of the one before, and history starting afresh; and opening the open
    /// scene's file again reverts it.
    #[test]
    fn open_puts_a_files_entities_in_place_of_the_open_scenes() {
        let dir = Scratch::new("open-scene-open");
        let (_, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let scenes = dir.0.join("scenes");
        let mut level = World::seeded(4);
        let [p, q] = [0, 1].map(|_| level.spawn(Transform::IDENTITY));
        level.set_parent(q, Some(p)).expect("no cycle");
        save(
            &scenes.join("level2.ron"),
            &level.save_scene(&[p]).expect("saved"),
            0,
        );
        let [p, q] = [p, q].map(|entity| level.id(entity).expect("alive"));
        let other = scenes.join("other.ron");
        save(&other, "(entities: {})\n", 0);
        cx.load_scene_file(&other).expect("loaded");
        let game = cx.world_mut().spawn(Transform::IDENTITY);
        let game_id = cx.world().id(game).expect("alive");
        save(&scenes.join("broken.ron"), "(entities: {", 0);
        let taken = format!("(entities: {{\"{game_id}\": ()}})\n");
        save(&scenes.join("taken.ron"), &taken, 0);
        apply(&mut cx, &mut stack, "agent", vec![create()]);
        let resolve = |cx: &Context, path: &str| open_scene::resolve(cx, Path::new(path), true);
        let (hash, edits) = (cx.world().hash(), cx.authors.logs(cx.world()).count());
        for (path, discard, code) in [
            ("scenes/level2.ron", false, "scene.unsaved"),
            ("scenes/other.ron", true, "scene.watched"),
            ("scenes/broken.ron", true, "scene.syntax"),
            ("scenes/taken.ron", true, "ecs.id_taken"),
        ] {
            let file = resolve(&cx, path).expect("in the scenes folder");
            let refused = open_scene::check_open(&mut cx, file, 0, discard).err();
            assert_eq!(refused.map(|error| error.code()), Some(code), "{path}");
            assert_eq!(cx.world().hash(), hash, "{path}");
            assert_eq!(cx.authors.logs(cx.world()).count(), edits, "{path}");
        }
        let file = resolve(&cx, "scenes/level2.ron").expect("in the scenes folder");
        let opening = open_scene::check_open(&mut cx, file, 0, true).expect("opens");
        open_scene::open(&mut cx, &mut stack, opening).expect("opened");
        let world = cx.world();
        assert!(world.entity(ids[0]).is_none() && world.contains(game));
        let q_in = world.entity(q).expect("made");
        assert_eq!(world.parent(q_in).expect("alive"), world.entity(p));
        assert_eq!(open_scene::title(&cx), Some("scenes/level2.ron"));
        let watching: Vec<&str> = cx
            .scenes
            .files
            .iter()
            .map(|file| file.author.as_str())
            .collect();
        assert_eq!(watching, ["scenes/other.ron", "scenes/level2.ron"]);
        assert!(authors(&cx).is_empty());
        assert!(!open_scene::unsaved(&mut cx, 0));
        // Watched: its save applies, as its own edit.
        look_now(&mut cx, &mut stack);
        let moved = Transform {
            translation: DVec3::X,
            ..Transform::IDENTITY
        };
        level
            .insert(level.entity(p).expect("alive"), moved)
            .expect("alive");
        let roots = [level.entity(p).expect("alive")];
        save(
            &scenes.join("level2.ron"),
            &level.save_scene(&roots).expect("saved"),
            2,
        );
        look_now(&mut cx, &mut stack);
        assert_eq!(
            (x(cx.world(), p), authors(&cx)),
            (1.0, vec!["scenes/level2.ron".to_owned()])
        );
        // Opened again, it reverts.
        apply(&mut cx, &mut stack, "agent", vec![set_x(q, 9.0)]);
        let file = resolve(&cx, "scenes/level2.ron").expect("in the scenes folder");
        let opening = open_scene::check_open(&mut cx, file, 0, true).expect("opens");
        open_scene::open(&mut cx, &mut stack, opening).expect("opened");
        assert_eq!((x(cx.world(), p), x(cx.world(), q)), (1.0, 0.0));
    }

    /// Open and new delete a model's root the open scene owns with its nodes, which scenes don't
    /// save, and whatever code put under them, the scene's or the game's, as a change of its model
    /// does; its file, opened again, spawns the nodes again, under the same IDs; and a file may give
    /// an entity a node's ID, which the node frees as it goes.
    #[test]
    fn open_and_new_take_a_models_nodes_with_its_root() {
        let dir = Scratch::new("open-scene-model");
        let (_, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let (model, _) = crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
        // First in ID order, so that its delete comes before those of what the scene owns.
        let root_id = EntityId::new(1).expect("not zero");
        let create = Operation::Create {
            entity: Some(root_id),
            parent: None,
            components: vec![("ModelRoot".into(), ModelRoot(model).to_value())],
        };
        apply(&mut cx, &mut stack, "agent", vec![create]);
        open_scene::save(&mut cx, None).expect("saved");
        let nodes = |world: &World| {
            let nodes = world.query::<(Entity, &ModelNode)>();
            let ids = nodes.map(|(node, _)| world.id(node).expect("alive"));
            sorted(ids.collect())
        };
        let spawned = nodes(cx.world());
        assert_eq!(spawned.len(), 4);
        let world = cx.world_mut();
        let node = world.entity(spawned[0]).expect("alive");
        let a = world.entity(ids[0]).expect("loaded");
        world.set_parent(a, Some(node)).expect("code may");
        let held = world.spawn(Transform::IDENTITY);
        world.set_parent(held, Some(node)).expect("code may");

        let open = |cx: &mut Context, stack: &mut Stack, path: &str| {
            let file = open_scene::resolve(cx, Path::new(path), true);
            let file = file.expect("in the scenes folder");
            let opening = open_scene::check_open(cx, file, 0, true).expect("opens");
            open_scene::open(cx, stack, opening).expect("opened");
        };
        open(&mut cx, &mut stack, "scenes/main.ron");
        let world = cx.world();
        assert_eq!(nodes(world), spawned, "spawned again, under the same IDs");
        let a = world.entity(ids[0]).expect("the file's");
        assert_eq!(world.parent(a).expect("alive"), None, "as its file has it");
        assert!(!world.contains(held), "gone with the node it was under");
        assert!(!open_scene::unsaved(&mut cx, 0));

        let node = format!("(entities: {{\"{}\": ()}})\n", spawned[0]);
        save(&dir.0.join("scenes").join("node.ron"), &node, 0);
        open(&mut cx, &mut stack, "scenes/node.ron");
        let world = cx.world();
        let entity = world.entity(spawned[0]).expect("the file's");
        assert!(world.get::<ModelNode>(entity).is_none());
        assert_eq!((world.entity(root_id), nodes(world)), (None, vec![]));
        open(&mut cx, &mut stack, "scenes/main.ron");

        open_scene::new(&mut cx, &mut stack).expect("emptied");
        let world = cx.world();
        assert_eq!((world.entity(root_id), world.entity(ids[0])), (None, None));
        assert_eq!(nodes(world), []);
    }

    /// A path names a scene file from the asset root, given relative or absolute, in its folder's
    /// own form; one outside the scenes folders is refused, as is a file to open that isn't there,
    /// a directory to save in that isn't, and any path in a game that isn't a project's.
    #[test]
    fn a_path_names_a_scene_file_in_the_projects_scenes_folders() {
        let dir = Scratch::new("open-scene-paths");
        project(&dir.0, 1);
        let cx = started(&dir.0);
        let main = dir.0.join("scenes").join("main.ron");
        let resolve = |path: &Path, existing| open_scene::resolve(&cx, path, existing);
        let found = (main.clone(), "scenes/main.ron".to_owned());
        assert_eq!(
            resolve(Path::new("scenes/main.ron"), true).expect("found"),
            found
        );
        assert_eq!(resolve(&main, true).expect("found"), found);
        let dotted = dir
            .0
            .join("scenes")
            .join("..")
            .join("scenes")
            .join("new.ron");
        let new = (
            dir.0.join("scenes").join("new.ron"),
            "scenes/new.ron".to_owned(),
        );
        assert_eq!(resolve(&dotted, false).expect("to save as"), new);
        fs::write(dir.0.join("outside.ron"), "(entities: {})\n").expect("written");
        for (path, existing, code) in [
            ("outside.ron", true, "scene.bad_path"),
            ("scenes/../outside.ron", false, "scene.bad_path"),
            ("scenes/none.ron", true, "scene.file"),
            ("scenes/none/new.ron", false, "scene.file"),
        ] {
            let refused = resolve(Path::new(path), existing).err();
            assert_eq!(refused.map(|error| error.code()), Some(code), "{path}");
        }
        let none = open_scene::resolve(&Context::seeded(3), &main, true).err();
        assert_eq!(none.map(|error| error.code()), Some("engine.no_project"));
    }

    /// A copy of the world as edited plays with a copy of its open scene, what clients make into
    /// the copy going with it at `stop`; a snapshot keeps the open scene of the run's world and of
    /// the world as edited it holds, and one written before the open scene was opens the start
    /// scene's file.
    #[test]
    fn the_open_scene_lasts_across_play_stop_and_a_snapshots_restore() {
        let dir = Scratch::new("open-scene-kept");
        project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
        let edited = cx.scenes.open.clone();
        stack.play(&mut cx, 0);
        assert_eq!(cx.scenes.open, edited);
        let copied = apply(&mut cx, &mut stack, "agent", vec![create()]);
        let played = cx.scenes.open.clone();
        assert_eq!(played.made, sorted([&made[..], &copied[..]].concat()));
        let run = Run::Held {
            world: Box::new(cx.edited.as_ref().expect("held").world.clone()),
            scenes: cx.scenes.files.clone(),
            open: edited.clone(),
            ticks: 0,
        };
        let of = snapshot::Of {
            world: cx.world(),
            scenes: &cx.scenes.files,
            cached: &cx.scenes.cached,
            open: &played,
            scripts: &[],
            mods: &[],
            seen: None,
        };
        let text = snapshot::write(&of, &run, "Tested", &[], 3).expect("written");
        assert_eq!(stack.stop(&mut cx), Some(0));
        assert_eq!(cx.scenes.open, edited);
        let restore = |text: &str| {
            let mut restored = Context::seeded(6)
                .project(&dir.0)
                .expect("the project reads");
            let snapshot = Snapshot::parse(text.to_owned(), "kept".to_owned(), "Tested");
            let snapshot = snapshot.expect("read");
            let restoring = Stack::default().restore(&mut restored, &snapshot, true);
            restoring.expect("restored");
            restored
        };
        let restored = restore(&text);
        assert_eq!(restored.scenes.open, played);
        assert_eq!(
            restored.edited.as_ref().map(|held| &held.open),
            Some(&edited)
        );
        let older: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("open "))
            .collect();
        let restored = restore(&format!("{}\n", older.join("\n")));
        let start = Open {
            author: Some("scenes/main.ron".to_owned()),
            made: Vec::new(),
            edited: false,
        };
        assert_eq!(restored.scenes.open, start);
        assert_eq!(
            restored.edited.as_ref().map(|held| &held.open),
            Some(&start)
        );
    }

    /// Whether the open scene is unsaved is compared in canonical form, so that a file written by
    /// hand isn't as loaded; is found again after an edit, or another tick, and kept otherwise; and
    /// a file whose text as last applied no longer loads is.
    #[test]
    fn unsaved_compares_canonically_and_is_kept_until_the_world_as_edited_changes() {
        let dir = Scratch::new("open-scene-unsaved");
        let (_, ids) = project(&dir.0, 1);
        let main = dir.0.join("scenes").join("main.ron");
        let id = ids[0];
        let transform =
            "Transform: (scale: (1, 1, 1), rotation: (0, 0, 0, 1), translation: (0, 0, 0))";
        let loose = format!("// Written by hand.\n(entities: {{\"{id}\": ({transform})}})");
        save(&main, &loose, 0);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        assert!(!open_scene::unsaved(&mut cx, 0), "as loaded");
        apply(&mut cx, &mut stack, "agent", vec![set_x(id, 1.0)]);
        assert!(open_scene::unsaved(&mut cx, 0), "found again after an edit");
        let entity = cx.world().entity(id).expect("alive");
        cx.world_mut()
            .insert(entity, Transform::IDENTITY)
            .expect("alive");
        assert!(open_scene::unsaved(&mut cx, 0), "kept");
        assert!(
            !open_scene::unsaved(&mut cx, 1),
            "found again at another tick"
        );
        cx.scenes.files[0].text = format!("(entities: {{\"{id}\": (Gone: ())}})\n");
        cx.scenes.changed();
        assert!(
            open_scene::unsaved(&mut cx, 1),
            "a text that no longer loads"
        );
    }
    /// A tick's change to the world makes the open scene unsaved but not edited; an author's apply
    /// makes it edited, which a reload of a script, a renewal, another scene file's save, a play
    /// and its stop leave as they found it, an edit of the copy that played going with it; and a
    /// save, the open scene's file's applying and New clear it.
    #[test]
    fn edited_is_set_by_authors_edits_alone_and_cleared_by_saves() {
        let dir = Scratch::new("open-scene-edited");
        let (_, ids) = project(&dir.0, 1);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        assert!(!cx.scenes.open.edited, "as loaded");
        // What a tick's physics does: the world changes, with no edit.
        let entity = cx.world().entity(ids[0]).expect("alive");
        cx.world_mut()
            .insert(
                entity,
                Transform {
                    translation: [3.0, 0.0, 0.0].into(),
                    ..Transform::IDENTITY
                },
            )
            .expect("alive");
        cx.scenes.changed();
        assert!(open_scene::unsaved(&mut cx, 1), "unsaved");
        assert!(!cx.scenes.open.edited, "but no author's edit");
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 5.0)]);
        assert!(cx.scenes.open.edited, "an author's apply");
        // Edits of no author of the open scene's.
        let script = Edit::Script {
            path: "scripts/none.luau".to_owned(),
            text: "return 1".to_owned(),
        };
        let _ = cx.apply_edit(script);
        cx.apply_edit(Edit::Renew).expect("renewed");
        let other = "(entities: {\n})\n".to_owned();
        let path = dir.0.join("scenes/other.ron");
        cx.scenes.loaded(SceneFile::new(
            path,
            "scenes/other.ron".to_owned(),
            other.clone(),
        ));
        let saved = super::apply_save(
            &mut cx,
            &mut |cx, edit| cx.apply_edit(edit),
            "scenes/other.ron",
            other,
        );
        saved.expect("applied");
        assert!(cx.scenes.open.edited, "kept by others' edits");
        // A play and its stop bring back the world as edited's, whatever the copy's edits did.
        stack.play(&mut cx, 1);
        stack.stop(&mut cx);
        assert!(cx.scenes.open.edited, "kept across a play");
        open_scene::save(&mut cx, None).expect("saved");
        assert!(!cx.scenes.open.edited, "a save clears it");
        stack.play(&mut cx, 1);
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 7.0)]);
        assert!(cx.edited.as_ref().is_some_and(|edited| !edited.open.edited));
        stack.stop(&mut cx);
        assert!(!cx.scenes.open.edited, "the copy's edit went with it");
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 6.0)]);
        assert!(cx.scenes.open.edited);
        let text = cx.scenes.files[0].text.clone();
        let applied = super::apply_save(
            &mut cx,
            &mut |cx, edit| cx.apply_edit(edit),
            "scenes/main.ron",
            text,
        );
        applied.expect("applied");
        assert!(!cx.scenes.open.edited, "its file applied");
        apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 8.0)]);
        open_scene::new(&mut cx, &mut stack).expect("emptied");
        assert!(!cx.scenes.open.edited, "New clears it");
    }

    /// The entity of the world's own in [`world_project`]'s start scene, beside its `Cells`.
    const OWN: u64 = 0xb0;

    /// Writes a project's manifest in `dir`, as [`project`] does, whose start scene is a world of
    /// cells, `scenes/cells`, 10 units on a side, of an entity of its own, [`OWN`], at x 100 beside
    /// its `Cells`; its cells 0 to 2 along x each hold an entity at its middle, and cell 100 one at
    /// x 1005, as [`id_of`] names them. Returns the scenes folder.
    fn world_project(dir: &Path) -> PathBuf {
        project(dir, 0);
        let scenes = dir.join("scenes");
        fs::create_dir_all(scenes.join("cells")).expect("a directory");
        let main = format!(
            "(\n    entities: {{\n        \"00000000000000a0\": (\n            Cells: (\n                \
             folder: \"scenes/cells\",\n                size: 10.0,\n            ),\n        ),\n{}",
            &cells_scene(&[(OWN, 100.0)])["(\n    entities: {\n".len()..]
        );
        save(&scenes.join("main.ron"), &main, 0);
        for cell in [0, 1, 2, 100] {
            let x = match cell {
                100 => 1005.0,
                cell => cell as f32 * 10.0 + 5.0,
            };
            let text = cells_scene(&[(id_of(cell).get(), x)]);
            save(&scenes.join(format!("cells/{cell}_0_0.ron")), &text, 0);
        }
        scenes
    }

    /// `cx`'s world's cells near x 15, as the look's viewpoint brings them in while the run is
    /// stopped: cells 0 to 2.
    fn bring_cells_in(cx: &mut Context) {
        look_at(cx, 15.0);
        cx.stream_stopped();
        cx.stream_stopped();
        assert_eq!(xs(cx.world()), [0, 1, 2]);
    }

    /// The IDs scene file `path` holds.
    fn file_ids(path: &Path) -> Vec<u64> {
        let text = crate::read_scene(path).expect("read");
        let ids = World::scene_ids(&text).expect("a scene");
        ids.into_iter().map(EntityId::get).collect()
    }

    /// A world's save writes each cell's file whose entities changed, as they stand, each streamed
    /// root in the file of the cell it's in, without `Streamed`, a duplicate of one too, and the
    /// world's file without them, but for one whose `Streamed` was taken away; a cell's file that
    /// didn't change isn't written. Unsaved follows.
    #[test]
    fn a_world_s_save_writes_each_cell_s_file_as_its_entities_stand() {
        let dir = Scratch::new("open-scene-world-save");
        let scenes = world_project(&dir.0);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        // Attached, as a run's is, whose world changes between ticks through the engine alone.
        stack.attach(&mut cx);
        bring_cells_in(&mut cx);
        assert!(!open_scene::unsaved(&mut cx, 0), "as loaded");
        let cell = |x: i32| scenes.join(format!("cells/{x}_0_0.ron"));
        let far = crate::read_scene(&cell(100)).expect("read");
        let operations = vec![set_x(id_of(1), 16.0), set_x(id_of(0), 25.0)];
        apply(&mut cx, &mut stack, "agent", operations);
        assert!(open_scene::unsaved(&mut cx, 0));
        open_scene::save(&mut cx, None).expect("saved");
        assert!(!open_scene::unsaved(&mut cx, 0), "saved");
        let [zero, one, two] = [0, 1, 2].map(|x| id_of(x).get());
        assert_eq!(file_ids(&cell(0)), [0; 0], "emptied");
        assert_eq!(file_ids(&cell(1)), [one]);
        assert_eq!(file_ids(&cell(2)), [zero, two], "moved in");
        assert_eq!(file_ids(&scenes.join("main.ron")), [0xa0, OWN]);
        let text = crate::read_scene(&cell(1)).expect("read");
        assert!(
            text.contains("16.0") && !text.contains("Streamed"),
            "{text}"
        );
        assert_eq!(crate::read_scene(&cell(100)).expect("read"), far, "out");
        let duplicate = Operation::Duplicate {
            entity: id_of(1),
            copies: Vec::new(),
        };
        let copy = apply(&mut cx, &mut stack, "agent", vec![duplicate])[0].get();
        let unstreamed = Operation::Remove {
            entity: id_of(2),
            component: "Streamed".into(),
        };
        apply(&mut cx, &mut stack, "agent", vec![unstreamed]);
        open_scene::save(&mut cx, None).expect("saved");
        let mut held = vec![one, copy];
        held.sort_unstable();
        assert_eq!(file_ids(&cell(1)), held, "the duplicate in its cell's");
        assert_eq!(file_ids(&cell(2)), [zero]);
        assert_eq!(file_ids(&scenes.join("main.ron")), [two, 0xa0, OWN]);
        assert!(!open_scene::unsaved(&mut cx, 0));
        // Into cell 3, which has no file: it comes in, and the save makes its file, which holds
        // what the cell does from then on.
        apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(1), 35.0)]);
        cx.stream_stopped();
        cx.stream_stopped();
        assert_eq!(xs(cx.world()), [0, 1, 2, 3]);
        open_scene::save(&mut cx, None).expect("saved");
        assert_eq!(file_ids(&cell(3)), [one]);
        assert!(!open_scene::unsaved(&mut cx, 0), "its file's");
        open_scene::save(&mut cx, None).expect("saved again");
    }

    /// The cells the game makes come in around the look's viewpoint while the run is stopped and go
    /// out as they came; a world's save writes no file of a made cell but of one that changed,
    /// whose file stands for it from then on, and is refused a root moved into a made cell that
    /// hasn't come in, until it has.
    #[test]
    fn a_world_s_save_writes_a_made_cell_s_file_once_it_changes() {
        use crate::cells::Cell;
        use gg_core::math::IVec3;

        let dir = Scratch::new("open-scene-world-made");
        let scenes = world_project(&dir.0);
        let mut cx = started(&dir.0);
        let [from, to] = [3, 100].map(|x| Cell(IVec3::new(x, 0, 0)));
        cx.make_cells("scenes/cells", from, to, |cell| {
            let x = cell.0.x;
            cells_scene(&[(0x100 + x as u64, x as f32 * 10.0 + 5.0)])
        });
        let mut stack = Stack::default();
        stack.attach(&mut cx);
        let look = |cx: &mut Context, x: f32| {
            look_at(cx, x);
            cx.stream_stopped();
            cx.stream_stopped();
        };
        look(&mut cx, 15.0);
        // Within the look's radius of 512.
        assert_eq!(xs(cx.world()), (0..=52).collect::<Vec<_>>());
        assert!(!open_scene::unsaved(&mut cx, 0), "as made");
        let cell = |x: i32| scenes.join(format!("cells/{x}_0_0.ron"));
        let four = EntityId::new(0x104).expect("an ID");
        apply(&mut cx, &mut stack, "agent", vec![set_x(four, 46.0)]);
        assert!(open_scene::unsaved(&mut cx, 0));
        open_scene::save(&mut cx, None).expect("saved");
        assert!(!open_scene::unsaved(&mut cx, 0), "saved");
        assert_eq!(file_ids(&cell(4)), [0x104]);
        assert!(!cell(3).exists() && !cell(5).exists(), "as made");
        let state = cx.world().resource::<CellState>().expect("streamed");
        let saved = Cell(IVec3::new(4, 0, 0));
        assert!(
            !state.came_as.contains_key(&saved),
            "it came as the file no longer has it"
        );
        assert!(state.came_as.contains_key(&Cell(IVec3::new(5, 0, 0))));
        let list = cx.cells_read(true, 0).world.expect("a world of cells");
        let five = list.cells.iter().find(|at| at.cell == [5, 0, 0]);
        let five = five.expect("listed, as it's in");
        assert!(five.made && !five.file, "{five:?}");

        look(&mut cx, 5000.0);
        assert_eq!(xs(cx.world()), [0; 0], "each as it came, or as saved");
        look(&mut cx, 15.0);
        let at = |cx: &Context| {
            let world = cx.world();
            let four = world.entity(four).expect("in");
            world.get::<Transform>(four).expect("placed").translation.x
        };
        assert_eq!(at(&cx), 46.0, "as its file has it");

        // Into cell 80, made, which hasn't come in.
        apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(0), 805.0)]);
        let refused = open_scene::save(&mut cx, None).expect_err("refused");
        assert_eq!(refused.code(), "scene.cell_out", "{refused:#}");
        assert!(!cell(80).exists());
        cx.stream_stopped();
        cx.stream_stopped();
        open_scene::save(&mut cx, None).expect("saved once it's in");
        assert_eq!(file_ids(&cell(80)), [id_of(0).get(), 0x150]);
    }

    /// A world's save is refused, writing nothing, for a cell's file saved since it last applied,
    /// and for a root moved into a cell whose file hasn't come in, until it has; unsaved meanwhile.
    #[test]
    fn a_world_s_save_is_refused_what_would_lose_a_cell_s_file() {
        let dir = Scratch::new("open-scene-world-refused");
        let scenes = world_project(&dir.0);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        bring_cells_in(&mut cx);
        let main = scenes.join("main.ron");
        let text = crate::read_scene(&main).expect("read");
        let one = scenes.join("cells/1_0_0.ron");
        let theirs = cells_scene(&[(id_of(1).get(), 17.0)]);
        save(&one, &theirs, 2);
        apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(1), 16.0)]);
        let refused = open_scene::save(&mut cx, None).expect_err("refused");
        assert_eq!(refused.code(), "scene.changed", "{refused:#}");
        assert_eq!(crate::read_scene(&one).expect("read"), theirs);
        assert_eq!(
            crate::read_scene(&main).expect("read"),
            text,
            "nothing written"
        );
        save(&one, &cells_scene(&[(id_of(1).get(), 15.0)]), 4);
        apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(0), 1004.0)]);
        let refused = open_scene::save(&mut cx, None).expect_err("refused");
        assert_eq!(refused.code(), "scene.cell_out", "{refused:#}");
        assert!(open_scene::unsaved(&mut cx, 0));
        // One frame notes it coming, the next brings it in.
        cx.stream_stopped();
        cx.stream_stopped();
        assert_eq!(xs(cx.world()), [0, 1, 2, 100], "the root's cell came in");
        open_scene::save(&mut cx, None).expect("saved");
        let held = [id_of(0).get(), id_of(100).get()];
        assert_eq!(file_ids(&scenes.join("cells/100_0_0.ron")), held);
    }

    /// New and open from a world leave none of its cells' entities, and its cells' state empty.
    #[test]
    fn new_and_open_from_a_world_leave_none_of_its_cells() {
        let dir = Scratch::new("open-scene-world-new");
        world_project(&dir.0);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        bring_cells_in(&mut cx);
        let streamed = |cx: &Context| cx.world().query::<&crate::Streamed>().count();
        assert_eq!(streamed(&cx), 3);
        open_scene::new(&mut cx, &mut stack).expect("emptied");
        assert_eq!(streamed(&cx), 0);
        let state = cx.world().resource::<CellState>();
        assert!(state.is_none_or(CellState::is_empty));
        let main = open_scene::resolve(&cx, Path::new("scenes/main.ron"), true).expect("found");
        let opening = open_scene::check_open(&mut cx, main, 0, true).expect("opens");
        open_scene::open(&mut cx, &mut stack, opening).expect("opened");
        bring_cells_in(&mut cx);
        let main = open_scene::resolve(&cx, Path::new("scenes/main.ron"), true).expect("found");
        let opening = open_scene::check_open(&mut cx, main, 0, true).expect("opens again");
        open_scene::open(&mut cx, &mut stack, opening).expect("opened again");
        assert_eq!(streamed(&cx), 0, "until they come in");
        assert!(xs(cx.world()).is_empty());
    }

    /// A cell's file saved on disk applies to its cell in as the open scene's file does, an edit
    /// of the file's own, which an undo takes back, a root it adds streaming; a cell's file made
    /// since brings its cell in; and one saved while a copy plays applies to the world as edited
    /// after it stops.
    #[test]
    fn a_cell_s_file_saved_applies_to_its_cell_in() {
        let dir = Scratch::new("open-scene-world-watch");
        let scenes = world_project(&dir.0);
        let mut cx = started(&dir.0);
        let mut stack = Stack::default();
        bring_cells_in(&mut cx);
        look_now(&mut cx, &mut stack);
        let one = scenes.join("cells/1_0_0.ron");
        let added = 0x20;
        save(
            &one,
            &cells_scene(&[(id_of(1).get(), 17.0), (added, 12.0)]),
            2,
        );
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), id_of(1)), 17.0);
        let added = EntityId::new(added).expect("an ID");
        let entity = cx.world().entity(added).expect("added");
        assert!(cx.world().get::<crate::Streamed>(entity).is_some());
        assert!(!open_scene::unsaved(&mut cx, 0), "as its file has it");
        let undo = Edit::Undo {
            author: "scenes/cells/1_0_0.ron".to_owned(),
        };
        client(&mut cx, &mut stack, undo);
        assert_eq!(x(cx.world(), id_of(1)), 15.0, "taken back");
        assert!(cx.world().entity(added).is_none());
        let three = scenes.join("cells/3_0_0.ron");
        save(&three, &cells_scene(&[(id_of(3).get(), 35.0)]), 2);
        look_now(&mut cx, &mut stack);
        cx.stream_stopped();
        cx.stream_stopped();
        assert_eq!(xs(cx.world()), [0, 1, 2, 3], "its file made since");
        stack.play(&mut cx, 0);
        save(&one, &cells_scene(&[(id_of(1).get(), 18.0)]), 4);
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), id_of(1)), 18.0, "the copy");
        assert_eq!(stack.stop(&mut cx), Some(0));
        assert_eq!(
            x(cx.world(), id_of(1)),
            15.0,
            "the world as edited, as it was"
        );
        look_now(&mut cx, &mut stack);
        assert_eq!(x(cx.world(), id_of(1)), 18.0, "the world as edited");
    }
}
