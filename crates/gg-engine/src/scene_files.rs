//! The scene files a game loaded, through [`Context::load_scene_file`], which a development build
//! watches as it runs: one saved applies to the world as the operations that take its entities from
//! what the file held to what it holds, as [`World::scene_edit`] finds them, an edit of the file's
//! own, its author the file's path as the app's root names it, which the tool API undoes. The file
//! as last applied is what the next save is compared with, and snapshots keep it, so that a run
//! restored from one watches the files its run had loaded, and applies what changed meanwhile.
//!
//! [`Context::load_scene_file`]: crate::Context::load_scene_file
//! [`World::scene_edit`]: crate::World

use std::path::PathBuf;
#[cfg(feature = "dev")]
use std::time::{Duration, Instant, SystemTime};

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

/// The scene files a game loaded, and when a development build last looked at them.
#[derive(Default)]
pub(crate) struct SceneFiles {
    pub(crate) files: Vec<SceneFile>,
    #[cfg(feature = "dev")]
    looked: Option<Instant>,
}

impl SceneFiles {
    /// Keeps `file`, in place of one of the same path.
    pub(crate) fn loaded(&mut self, file: SceneFile) {
        self.files.retain(|kept| kept.path != file.path);
        self.files.push(file);
    }
}

/// How long a development build waits between looks at the scene files: a save applies on the next
/// look, a frame's start, at most this long after.
#[cfg(feature = "dev")]
const LOOK: Duration = Duration::from_millis(200);

#[cfg(feature = "dev")]
mod watch {
    use std::fs;
    use std::mem;
    use std::time::Instant;

    use tracing::{info, warn};

    use gg_core::{Error, Result};

    use super::{LOOK, SceneFile};
    use crate::Context;
    use crate::ecs::SceneEdit;
    use crate::edits::{Done, Edit};
    use crate::layer::Stack;

    /// As a frame starts, between ticks, applies the scene files saved since the last look, at
    /// most one a [`LOOK`]: each as the edit that takes the world's entities from the file's text as
    /// last applied to its text now, through `stack`, which records it, logging `scene.applied`.
    /// One that doesn't load, or whose edit the world refuses, warns with the error's code and
    /// changes nothing, and the next save is compared with the file as last applied. In a lockstep
    /// session, the host offers each save for a tick both peers apply it before, as
    /// [`Stack::offer_save`] says, and a joiner doesn't look.
    pub(crate) fn look(cx: &mut Context, stack: &mut Stack) {
        let now = Instant::now();
        if !stack.watches_scenes()
            || cx.scenes.files.is_empty()
            || cx
                .scenes
                .looked
                .is_some_and(|looked| now.duration_since(looked) < LOOK)
        {
            return;
        }
        cx.scenes.looked = Some(now);
        let mut saves = Vec::new();
        for file in &mut cx.scenes.files {
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
                saves.push((file.author.clone(), text));
            }
        }
        // With the files in place, which a save finds its own among as it applies, now or at a
        // tick to come.
        for (author, text) in saves {
            stack.offer_save(cx, author, text);
        }
    }

    /// Applies the save of the scene file whose author is `author`, its text now `text`, through
    /// `edit`, which applies an edit to `cx`'s world and records it, as [`look`] says.
    ///
    /// Fails with `scene.not_watched` for an author no scene file has, and as [`apply`] does.
    pub(crate) fn apply_save(
        cx: &mut Context,
        edit: &mut dyn FnMut(&mut Context, Edit) -> Result<Done>,
        author: &str,
        text: String,
    ) -> Result<Done> {
        let mut files = mem::take(&mut cx.scenes.files);
        let applied = match files.iter_mut().find(|file| file.author == author) {
            Some(file) => apply(cx, edit, file, text),
            None => Err(Error::new(
                "scene.not_watched",
                format!("no scene file the layers loaded is {author}"),
            )),
        };
        files.append(&mut cx.scenes.files);
        cx.scenes.files = files;
        applied.map(|()| Done::Applied(Vec::new()))
    }

    /// Applies the edit that takes `cx`'s world from `file`'s text to `text`, its text now, through
    /// `edit`, as [`look`] says.
    ///
    /// Fails, having warned, as the text's load or the edit does.
    fn apply(
        cx: &mut Context,
        edit: &mut dyn FnMut(&mut Context, Edit) -> Result<Done>,
        file: &mut SceneFile,
        text: String,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
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
        let mut operations = 0;
        let applied = cx
            .world()
            .scene_edit(old.as_deref(), &text)
            .and_then(|found| match found {
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
                    let rest = match cx.world().scene_edit(old.as_deref(), &text) {
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

#[cfg(all(test, feature = "dev"))]
mod tests {
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    use gg_core::math::Vec3;

    use super::*;
    use crate::args::Net;
    use crate::dirs::tests::Scratch;
    use crate::edits::Edit;
    use crate::layer::Stack;
    use crate::net::{DELAY, Lockstep};
    use crate::{Context, Tick, Transform};

    /// Writes `text` at `path`, stamped `later` seconds past now, so that a look sees it changed
    /// whatever the clock's grain.
    fn save(path: &std::path::Path, text: &str, later: u64) {
        fs::write(path, text).expect("written");
        let at = SystemTime::now() + Duration::from_secs(later);
        let file = File::options().write(true).open(path).expect("opened");
        file.set_modified(at).expect("stamped");
    }

    /// A project's start scene loads as a scene file, watched, its edits' author its path from the
    /// project's asset root; a game that isn't a project's has none.
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
            translation: Vec3::new(1.0, 2.0, 3.0),
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
        let net = Net::Host("127.0.0.1:0".to_owned());
        stack.lockstep(Lockstep::start(&net, "Tested", &[]).expect("hosting"));
        let look = |cx: &mut Context, stack: &mut Stack| {
            cx.scenes.looked = None;
            look(cx, stack);
        };
        look(&mut cx, &mut stack);
        let moved = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
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
}
