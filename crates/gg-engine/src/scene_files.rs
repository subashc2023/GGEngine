//! The scene files a game loaded through [`Context::load_scene_file`]. A development build watches
//! them: a saved file applies as the operations [`World::scene_edit`] finds between its text as
//! last applied and its text now, an edit by the file's path from the app's root, which the tool
//! API undoes. Snapshots keep the text as last applied, so a restored run applies what changed.
//!
//! One is the open scene, a project's start scene, which the tool API's `scene` saves, empties or
//! replaces. It owns its file's entities as last applied, those clients' edits make into it, and
//! those its file's saves delete (an undo may restore them); never the game code's, so a save
//! writes the scene alone. Snapshots keep it.
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SceneFile {
    /// As the game named it.
    pub(crate) path: PathBuf,
    /// Its path from the app's root, as assets are named: the author of its edits.
    pub(crate) author: String,
    /// Its text as loaded, or as last applied since.
    pub(crate) text: String,
    /// Modified time and size at the last look; `None` before the first.
    #[cfg(feature = "dev")]
    stamp: Option<(SystemTime, u64)>,
    /// Its text as last read, which the next save is compared with. In a lockstep session a save
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
    /// Texts kept for scene changes, each its author's, as [`Context::scene_text`] reads them:
    /// files read at start or first named by a change, and files the layers loaded, as last
    /// saved. Watched like `files`; snapshots keep them.
    pub(crate) cached: Vec<SceneFile>,
    /// The open scene of the world in the context's place: the copy's, while one plays.
    pub(crate) open: Open,
    #[cfg(feature = "dev")]
    looked: Option<Instant>,
    /// When the last look that read every file saved before it started, by the wall clock.
    #[cfg(feature = "dev")]
    pub(crate) looked_at: Option<SystemTime>,
    /// Whether the edited world's open scene differs from its file, as last found, and that
    /// world's ticks then; `None` once either may have changed.
    #[cfg(feature = "dev")]
    unsaved: Option<(u64, bool)>,
    /// The cells whose files a save would write, as last found with `unsaved`.
    #[cfg(feature = "dev")]
    unsaved_cells: Vec<crate::cells::Cell>,
    /// The open scene's file text as last applied, and its canonical form (`None` if the text no
    /// longer loads), which the scene as it would be saved is compared with.
    #[cfg(feature = "dev")]
    canonical: Option<(String, Option<String>)>,
    /// How many times the edited world or its open scene may have changed, as [`changed`] counts.
    ///
    /// [`changed`]: Self::changed
    #[cfg(feature = "dev")]
    pub(crate) changes: u64,
}

impl SceneFiles {
    /// Keeps `file`, replacing one of the same path.
    pub(crate) fn loaded(&mut self, file: SceneFile) {
        self.files.retain(|kept| kept.path != file.path);
        self.files.push(file);
    }

    /// Keeps `file`'s text for scene changes, replacing one of the same author's.
    pub(crate) fn cache(&mut self, file: SceneFile) {
        cache(&mut self.cached, file);
    }

    /// Forgets what [`unsaved`] last found: an edit or a scene file's save applied, or the world
    /// was restored.
    #[cfg(feature = "dev")]
    pub(crate) fn changed(&mut self) {
        self.unsaved = None;
        self.changes += 1;
    }
}

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

/// The open scene: its file, if titled, and the entities it owns beyond those in the file's text
/// as last applied. A save writes those of either that are alive.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Open {
    /// Its file's author, one of the scene files'; `None` while untitled (after the tool API's
    /// `scene new`, or in a game with no start scene).
    pub(crate) author: Option<String>,
    /// Owned entities its file's text doesn't hold: those clients' edits made into it, those a
    /// save left out as gone, and those its file's saves took out of the text. Each once, in ID
    /// order; gone ones are kept, for an undo.
    pub(crate) made: Vec<EntityId>,
    /// Whether an author's edit has applied since it was last saved, opened or made new, or its
    /// file's save applied: the tool API status's `edited`. Ticks and a script's reload never set
    /// it; an undo back to the saved scene leaves it set. Snapshots keep it, as does the edited
    /// world a playing copy replaces.
    pub(crate) edited: bool,
}

/// The open scene for a snapshot written before open scenes existed: the start scene's file
/// `start` if `files` holds it, else untitled.
pub(crate) fn opened(start: Option<&Path>, files: &[SceneFile]) -> Open {
    let start = start.and_then(|start| files.iter().find(|file| same_file(&file.path, start)));
    Open {
        author: start.map(|file| file.author.clone()),
        made: Vec::new(),
        edited: false,
    }
}

/// Checks that each part of `path`, from `root` with `/` between parts, is named on the disk
/// exactly as given, byte for byte. A disk that ignores case or normalizes Unicode, as macOS's and
/// Windows' do, finds files that a package, or Linux's disk, wouldn't.
///
/// Fails with the path as the disk names it, each inexact part matched by Unicode lower case, or
/// with `None` for a part that isn't there even so.
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

/// Whether paths `a` and `b` name the same file, compared [`canonical`]: one may be in another
/// form, such as Windows' `\\?\` one, or through a link.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

/// `path` canonical: the file's canonical path; for a file not there yet, its directory's joined
/// with its name; else as given.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let parent = path.parent().and_then(|dir| fs::canonicalize(dir).ok());
        match (parent, path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path.to_owned(),
        }
    })
}

/// Applies a client's `edit` through `apply`, and puts the entities it created or duplicated into
/// `cx`'s open scene. Game code and scene file saves (whose text holds their entities) never do.
/// An undo adds nothing: the scene owns what it brings back already. A game that isn't a
/// project's has no open scene.
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

/// How long a development build waits between looks at the scene files: a save applies at most
/// this long after.
#[cfg(feature = "dev")]
const LOOK: Duration = Duration::from_millis(200);

#[cfg(feature = "dev")]
mod watch {
    use std::collections::HashSet;
    use std::fs;
    use std::mem;
    use std::time::{Instant, SystemTime};

    use tracing::{info, warn};

    use gg_core::{Error, Result};

    use super::{LOOK, SceneFile};
    use crate::ecs::SceneEdit;
    use crate::edits::{Done, Edit};
    use crate::layer::Stack;
    use crate::{Context, EntityId, World};

    /// At a frame's start, between ticks, applies scene files saved since the last look, at most
    /// one look per [`LOOK`]: the edit from the file's text as last applied to its text now,
    /// through `stack`, which records it. Logs `scene.applied`, naming the entities whose changes
    /// it left out as the world has deleted them since ([`World::scene_left_out`]). A file that
    /// doesn't load, or whose edit the world refuses, warns with the error's code and changes
    /// nothing; the next save is compared with the file as last applied. In lockstep the host
    /// offers each save for a tick
    /// both peers apply it before ([`Stack::offer_save`]). A joiner doesn't look, nor does a run
    /// reading a package, whose scenes aren't files.
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
        let started = SystemTime::now();
        let mut read_all = true;
        let mut saves: Vec<(String, String)> = Vec::new();
        // Files in the world first: a save of one takes its kept text too.
        let scenes = &mut cx.scenes;
        for file in scenes.files.iter_mut().chain(&mut scenes.cached) {
            let stamp = fs::metadata(&file.path)
                .and_then(|meta| Ok((meta.modified()?, meta.len())))
                .ok();
            if stamp.is_none() || stamp == file.stamp {
                continue;
            }
            // Being written, or held by its writer: a later look reads it, its stamp unchanged.
            let Ok(text) = crate::ecs::read_scene(&file.path) else {
                read_all = false;
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
        // After the loop, so a save finds its own file in place, applying now or later.
        for (author, text) in saves {
            stack.offer_save(cx, author, text);
        }
        if read_all {
            cx.scenes.looked_at = Some(started);
        }
        // A cell's file made or taken away since: its cell comes in from it, or no longer does.
        // Not in a run that can't stop, whose recording or session lists it once.
        if stack.refuses_stop().is_none() {
            cx.list_cells_again();
        }
    }

    /// Applies the save of the scene file with `author`, its text now `text`, through `edit`, as
    /// [`look`] says. It also keeps the text through an edit, [`Edit::Scene`], so a replay and
    /// each lockstep peer keep it at the same tick: once applied for a file in the world, as saved
    /// for a kept-only one. A kept cell file whose cell is in and whose file came in
    /// ([`World::filed_cell_in`]) applies like a file in the world, its roots given `Streamed` in
    /// both texts as the cell's load gives them.
    ///
    /// Fails with `scene.not_watched` for an author no scene file has, and as [`apply`] does.
    pub(crate) fn apply_save(
        cx: &mut Context,
        edit: &mut dyn FnMut(&mut Context, Edit) -> Result<Done>,
        author: &str,
        text: String,
    ) -> Result<Done> {
        // `edited` keeps its value, or clears for the open scene's own file: its save leaves the
        // scene as saved.
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
        // The open scene's file text as last applied may have changed.
        cx.scenes.changed();
        cx.scenes.open.edited = edited;
        applied.map(|()| Done::Applied(Vec::new()))
    }

    /// Applies the edit from `file`'s text to `text`, its text now, through `edit`, as [`look`]
    /// says. For a cell's file, `cell`, each text's roots are given `Streamed` first. For the open
    /// scene's file, the scene owns from then on the entities the edit takes out of the text,
    /// deleted or gone already.
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
        // Load what it names first, as a scene's load does; a text that doesn't load is refused
        // below.
        let _ = cx.load_scene_assets(&text);
        let path = file.path.display();
        let author = || file.author.clone();
        // The file as last applied, unless it no longer loads (a rebuild took a field away): the
        // world's entities stand for it then.
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
        // Entities the save takes out of the text, owned from now on so an undo restores them:
        // each in the old text and not the new, though the edit deletes only the top-most, and
        // those already gone (a client's undo of its delete restores one). A child the game made
        // under one, which no text holds, is never owned.
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
        let mut left_out = Vec::new();
        let applied = new.and_then(|new| {
            let found = cx.world().scene_edit(old.as_deref(), &new)?;
            match found {
                SceneEdit::Changes(changes) => {
                    operations = changes.len();
                    if !changes.is_empty() {
                        let apply = Edit::Apply {
                            author: author(),
                            operations: changes,
                        };
                        edit(cx, apply)?;
                    }
                }
                SceneEdit::Creates(creates) => {
                    operations = creates.len();
                    let apply = Edit::Apply {
                        author: author(),
                        operations: creates,
                    };
                    edit(cx, apply)?;
                    // With the entities made, the rest, which may hold them; if refused, the
                    // append undoes the creates too.
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
                    edit(cx, append)?;
                }
            }
            // Asked once applied, when what the file adds is made. It's for the log alone, so a
            // failure, which the edit's loads make unlikely, leaves the log without it.
            if let Some(old) = &old {
                left_out = cx.world().scene_left_out(old, &new).unwrap_or_default();
            }
            Ok(())
        });
        match &applied {
            Ok(()) => {
                info!(
                    code = "scene.applied",
                    path = %path,
                    operations,
                    left_out = left_out.len(),
                    "Applied the scene file's changes to the world{}",
                    left_out_told(&left_out)
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

    /// How many entities left out a `scene.applied` names; it counts the rest.
    const NAMED: usize = 5;

    /// What a `scene.applied` message adds for the entities its edit left out, as
    /// [`World::scene_left_out`] gives them: nothing for none.
    pub(super) fn left_out_told(left_out: &[(EntityId, Option<String>)]) -> String {
        if left_out.is_empty() {
            return String::new();
        }
        let named = left_out.iter().take(NAMED).map(|(id, name)| match name {
            Some(name) => format!("{name} ({id})"),
            None => id.to_string(),
        });
        let mut told = format!(
            ", leaving out its changes to entities the world has deleted since: {}",
            named.collect::<Vec<_>>().join(", ")
        );
        if left_out.len() > NAMED {
            told.push_str(&format!(" and {} more", left_out.len() - NAMED));
        }
        told.push_str("; reloading the scene brings them back");
        told
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
    use crate::ecs::Operation;
    use crate::ecs::{SceneEdit, text_digest};
    use crate::layer::Stack;
    use crate::{Context, Entity, EntityId, World};
    use crate::{package, perf};

    /// The scene file `path`, absolute or relative to the asset root, names for the tool API's
    /// `scene`: its path from the root, and its author, that path with `/` between parts, such as
    /// `scenes/level2.ron`. A file to open (`existing`) must be there; one to save as needs its
    /// directory, since a save makes none. Each is compared canonical with the project's scenes
    /// folders.
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

    /// The error for a game that isn't a project's, which has no open scene.
    pub(crate) fn no_project() -> Error {
        let message = "the game isn't a project's, which has scenes to open and save: \
                       `App::project` makes it one";
        Error::new("engine.no_project", message)
    }

    /// The author of the edited world's open scene file; `None` while untitled.
    pub(crate) fn title(cx: &Context) -> Option<&str> {
        as_edited(cx).2.author.as_deref()
    }

    /// The edited world, its scene files and open scene: held aside while a copy plays
    /// ([`Stack::play`]), else the context's own.
    fn as_edited(cx: &Context) -> (&World, &[SceneFile], &Open) {
        match &cx.dev.edited {
            Some(edited) => (&edited.world, edited.scenes.as_slice(), &edited.open),
            None => (cx.world(), cx.scenes.files.as_slice(), &cx.scenes.open),
        }
    }

    fn cached_as_edited(cx: &Context) -> &[SceneFile] {
        match &cx.dev.edited {
            Some(edited) => &edited.cached,
            None => &cx.scenes.cached,
        }
    }

    fn file_of<'f>(files: &'f [SceneFile], open: &Open) -> Option<&'f SceneFile> {
        let author = open.author.as_deref()?;
        files.iter().find(|file| file.author == author)
    }

    /// The IDs open scene `open` owns, alive or not: those its file held as last applied, read
    /// from the text's syntax alone so a text that no longer loads still says what it holds, and
    /// those it made. Each once, in ID order.
    fn claimed(files: &[SceneFile], open: &Open) -> Vec<EntityId> {
        let text = file_of(files, open).map(|file| file.text.as_str());
        // A text as last applied loaded then, so it reads as a scene's RON.
        let mut ids = text.map_or_else(Vec::new, |text| World::scene_ids(text).unwrap_or_default());
        ids.extend(&open.made);
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The live entities of [`claimed`]'s IDs, in ID order.
    pub(super) fn owned(world: &World, files: &[SceneFile], open: &Open) -> Vec<Entity> {
        let ids = claimed(files, open).into_iter();
        ids.filter_map(|id| world.entity(id)).collect()
    }

    /// Streamed roots with their cell and ID, as [`World::streamed_roots`] gives them.
    type Roots = Vec<(Cell, EntityId, Entity)>;

    /// If `world`'s cells stream ([`Cells`](crate::Cells)): their folder and the streamed roots.
    /// `None` for a world without cells, or with a cell size that isn't finite and positive; its
    /// streamed roots stream nowhere, and save like the rest.
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

    /// `roots` and everything under them.
    fn under(world: &World, roots: &[(Cell, EntityId, Entity)]) -> HashSet<Entity> {
        let mut walk: Vec<Entity> = roots.iter().map(|&(.., root)| root).collect();
        let mut under = HashSet::new();
        while let Some(entity) = walk.pop() {
            under.insert(entity);
            walk.extend(world.children(entity).expect("alive"));
        }
        under
    }

    /// The entities that open scene `open` and its world's cells hold, which a save writes and
    /// `scene new` and `open` delete: those it owns ([`owned`]); and, where cells stream
    /// ([`streaming`]), the streamed roots with everything under them, which the cells' files
    /// hold, plus live entities in the as-last-applied files of cells in (from `cached`), such as
    /// one whose `Streamed` was taken away, which the world's file holds from its next save.
    /// Returns them in ID order, and those the cells' files hold.
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

    /// The text of cell `cell`'s file in `folder` as last applied: its text in `cached`, or, for a
    /// cell without, what the game makes of it (`made`), if it does.
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

    fn made_for<'a>(cx: &'a Context, world: &World) -> Option<&'a MadeCells> {
        let (_, cells, _) = world.world_cells()?;
        cx.cells.made(&cells.folder)
    }

    /// What a save writes to the edited world's cells' files, where cells stream ([`streaming`]),
    /// else nothing. For each cell in, or with a streamed root in it, whose file would change
    /// ([`World::cell_written`]): the cell and its file under asset root `root`, or why the save
    /// is refused it. The last-applied file is [`applied`]'s, so a cell the game made gets a file
    /// once it changes, which stands for the game's making of it from then on.
    ///
    /// Refuses with `scene.changed`, naming the file, a cell whose file holds another text than the
    /// last applied, like the open scene's file; and with `scene.cell_out`, naming the file, one
    /// that's there but hasn't come in (a cell refused or not yet in that a root was moved into),
    /// whose entities a save would lose.
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
                Some(file) => crate::ecs::read_scene(&path)
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

    /// Saves the edited world's open scene (the tool API's `scene save`): to `to`, a path and
    /// author from [`resolve`], which replaces its file, now unwatched; or to its own file. Writes
    /// exactly the entities the scene owns ([`owned`]), whole or not at all, and keeps the text as
    /// its file's last applied and read, so the watch's next look applies nothing; gone entities
    /// stay owned, for an undo. Where cells stream, streamed roots and what's under them go to
    /// their cells' files, and entities from cells in that no longer stream go to its own
    /// ([`held`]); each cell file that would change is written ([`cell_saves`]), kept like the
    /// open scene's, the cell noted as come in. While a copy plays it saves the world held aside,
    /// the copy's scene files following; neither world applies the save. Changes no world but the
    /// cells' files' digests, records nothing, and logs `scene.saved` with how many cell files it
    /// wrote.
    ///
    /// Fails, changing nothing, with `scene.untitled` for a save without `to` of an untitled
    /// scene; with `scene.watched` for a path another scene file the game loaded has; with
    /// `scene.changed`, naming the file, for a save to the open scene's file while the file holds
    /// another text than the one last applied, as after an edit the watch hasn't applied yet, or
    /// refused, so the edit isn't lost; as `cell_saves` fails; and as [`World::save_entities`]
    /// fails. After writing some files, fails as [`write_scene`](crate::ecs::write_scene) does.
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
            && let Ok(text) = crate::ecs::read_scene(&path)
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
        // The cells' files first, so a write that fails leaves an entity moved between the
        // world's file and a cell's in both, which a load refuses, rather than in neither.
        for (_, file) in &cells {
            crate::ecs::write_scene(&file.path, &file.text)?;
        }
        crate::ecs::write_scene(&path, &text)?;
        let gone = claimed(files, open).into_iter();
        let made: Vec<EntityId> = gone.filter(|&id| world.entity(id).is_none()).collect();
        let (entities, old) = (owned.len(), open.author.clone());
        let (saved, old) = (SceneFile::new(path, author, text), old.as_deref());
        let scenes = &mut cx.scenes;
        // Its text and its cells' files', as read, for scene changes to come: the watch applies
        // nothing of them.
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

    /// Has `files` and `open` hold `saved`, the open scene's file as just saved, replacing the
    /// file whose author is `old`, or after the others if none.
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

    /// The error for `other`, a scene file the game loaded that isn't the open scene's, having the
    /// path asked for.
    fn watched(other: &SceneFile) -> Error {
        let message = format!(
            "{} is another scene file the game loaded, {}, which a save or an open in place of \
             the open scene would part from what the game made of it",
            other.path.display(),
            other.author
        );
        Error::new("scene.watched", message)
    }

    /// Refuses `scene new` and `scene open` while the open scene has unsaved edits ([`unsaved`],
    /// at `ticks` unless a copy plays), unless `discard`.
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

    /// Empties the open scene (`scene new`) once a copy that played has stopped: deletes the
    /// entities it and its cells hold ([`held`]) but the game's own ([`emptying`]); empties the
    /// cells ([`Context::empty_cells`]); starts every author's edits afresh ([`Stack::rewrite`]),
    /// since none would undo it; and leaves the scene untitled, its file unwatched and unchanged
    /// on disk. Logs `scene.new`.
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

    /// A scene file to open, as [`check_open`] read it, and when the check started.
    pub(crate) struct Opening {
        path: PathBuf,
        author: String,
        text: String,
        started: Instant,
    }

    /// Before anything changes, checks that `file`, a path and author from [`resolve`], can open
    /// in place of the open scene (`scene open`), and reads it. The edited world is at `ticks`
    /// unless a copy plays.
    ///
    /// Fails with `scene.unsaved` as [`check_unsaved`] does, unless `discard`; with
    /// `scene.watched` for a file that another scene file the game loaded has, but the open one;
    /// as [`read_scene`](crate::ecs::read_scene) fails; as the text's load would, the path
    /// beginning its message; and with `ecs.id_taken` for an ID it gives that an entity neither
    /// the open scene nor its world's cells hold has ([`held`]), a model's node going with its
    /// root.
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
        let text = crate::ecs::read_scene(&path)?;
        // Load what it names first, as a scene's load does.
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

    /// Opens `opening` in place of the open scene (`scene open`) once a copy that played has
    /// stopped. One change of no author's, all or none, deletes what [`new`] deletes and makes the
    /// file's entities ([`World::scene_edit`], once those are gone). Like `new`, it empties the
    /// cells and starts every author's edits afresh ([`Stack::rewrite`]). The file becomes the
    /// open scene's, watched from the next look; the old file is unwatched. The open scene's own
    /// file opens again as on disk, reverting the scene. Cells come in from their files as
    /// [`Context::stream_stopped`] does. Logs `scene.opened`, with the time since the check, asset
    /// loads included.
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

    /// The operations that delete the entities [`held`] gives, and how many. A child of theirs
    /// that isn't held is made a root first, so it stays; except a model's node or an instance's
    /// prefab entity, which goes with its root, and what's under it, as when its model or prefab
    /// changes. Then each held entity whose parent isn't held, nor the root a parent goes with, is
    /// deleted with its descendants, which are held once those children are roots, except models'
    /// nodes, prefabs' entities and what's under them.
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
            // Nothing leaves a node or an instance's entity: an undo would put it back under the
            // node, which operations refuse, and an instance loses none of its prefab's entities.
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

    /// Unwatches the open scene's file and leaves the scene untitled, owning nothing. Returns the
    /// file's path.
    fn close(cx: &mut Context) -> Option<PathBuf> {
        let author = mem::take(&mut cx.scenes.open).author?;
        let files = &mut cx.scenes.files;
        let at = files.iter().position(|file| file.author == author)?;
        Some(files.remove(at).path)
    }

    /// Whether the edited world's open scene differs from its file, as the tool API's status says,
    /// at `ticks` unless a copy plays. Never, in a game that isn't a project's. Untitled: while it
    /// owns an entity. A file whose text as last applied no longer loads: always. Else while what
    /// [`save`] would write differs from that text in canonical form ([`World::scene_canonical`]),
    /// so a hand-written file isn't unsaved as loaded, or the save would be refused; and while a
    /// save would write or be refused a cell's file ([`cell_saves`]). Kept until the world at its
    /// ticks or its open scene changes ([`SceneFiles::changed`]): clients ask often and a large
    /// scene saves slowly.
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

    /// The cells whose files a save would write, or is refused, in order, as [`unsaved`] finds
    /// them. None where the save fails for all (a root it can't save), for which `unsaved` is true.
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
