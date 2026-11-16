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
        let mut folders =
            (cx.dev.folders.iter()).filter_map(|folder| fs::canonicalize(folder).ok());
        let inside = folders.any(|folder| found.starts_with(folder));
        let root_found = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let rest = found.strip_prefix(&root_found).ok().filter(|_| inside);
        let Some(rest) = rest else {
            let folders = cx.dev.folders.iter();
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
        match &cx.dev.edited {
            Some(edited) => (&edited.world, edited.scenes.as_slice(), &edited.open),
            None => (cx.world(), cx.scenes.files.as_slice(), &cx.scenes.open),
        }
    }

    /// The texts kept for scene changes of the world as edited, as [`as_edited`] says: those held
    /// aside while a copy plays, and else the context's own.
    fn cached_as_edited(cx: &Context) -> &[SceneFile] {
        match &cx.dev.edited {
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
        match &mut cx.dev.edited {
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
        let ticks = cx.dev.edited.as_ref().map_or(ticks, |edited| edited.ticks);
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
pub(crate) mod tests;
