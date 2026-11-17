//! The open scene: the File menu's requests of the game, the prompt to save first, and a close.

use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use gg_api::{Failure, Scene, SceneAction};
use gg_engine::Context;
use gg_engine::dialog::{FileDialog, Picks};
use tracing::warn;

use crate::file::{Choice, Item, Message};
use crate::look::Part;

use super::{Game, Level};
#[cfg(doc)]
use gg_engine::inspector::InspectorLink;
#[cfg(doc)]
use gg_engine::{Event, Layer};

/// A File menu `scene` request, and whether it's sent: once the Inspector has nothing waiting for
/// the game, as [`InspectorLink::idle`] says, so the game has its edits first.
pub(super) struct Request {
    pub(super) scene: Scene,
    pub(super) sent: bool,
}

/// What asks whether to save unsaved edits first: New and Open, which the game refuses while it
/// has some unless they're to be discarded, and a close of the editor.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Action {
    New,
    /// Opens the scene at this path, as the dialog picked it, which a prompt keeps so the dialog
    /// doesn't show again.
    Open(PathBuf),
    Close,
}

impl Action {
    /// The `scene` request that does it, discarding the unsaved edits if `discard`: none for a
    /// close.
    fn scene(&self, discard: bool) -> Option<Scene> {
        let (action, path) = match self {
            Self::New => (SceneAction::New, None),
            Self::Open(path) => (SceneAction::Open, Some(path.clone())),
            Self::Close => return None,
        };
        Some(Scene {
            action,
            path,
            discard,
        })
    }
}

/// How far a close of the editor, by Quit or the window's close button, has got, as
/// [`Game::close`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Closing {
    /// None asked for, or the last cancelled.
    No,
    /// Vetoed until the answer to a status asked after it says whether the scene has unsaved
    /// edits: `true` once that's asked.
    Checking(bool),
    /// Vetoed until the prompt is answered or the save its Save chose is done.
    Prompting,
    /// Granted: the UI asks the main window to close, and the close that makes goes ahead.
    Granted,
}

impl Game {
    /// What the title bar says of the open scene once a game has served: its file's name, or
    /// Untitled, then a `*` while it has unsaved edits.
    pub(super) fn scene_label(&self) -> Option<String> {
        let name = self.scene_name().unwrap_or("Untitled");
        let mark = if self.unsaved { "*" } else { "" };
        self.served.then(|| format!("{name}{mark}"))
    }

    /// The name of the open scene's file, such as `arena.ron`; `None` while it's untitled.
    fn scene_name(&self) -> Option<&str> {
        let scene = self.scene.as_deref()?;
        scene.rsplit('/').next()
    }

    /// Whether the File menu's scene items, their shortcuts and the Scenes panel do anything: while
    /// a game serves (which needs a project), the hub doesn't show, and no prompt, request of
    /// theirs or close waits.
    pub(super) fn files(&self) -> bool {
        let waits = self.prompt.is_some() || self.request.is_some() || self.due.is_some();
        self.talker.is_some() && !waits && self.closing == Closing::No && !self.hub_shows()
    }

    /// Does what the File menu's `item` does, from [`Layer::ui`], as the menu, a shortcut or a
    /// refusal chooses it.
    ///
    /// - Open and Save As show the OS's dialog, which blocks the frame until it closes, then do as
    ///   [`open`](Self::open) and [`save_as`](Self::save_as) do with the file it picked.
    /// - New prompts or asks the game, as [`act`](Self::act) says.
    /// - Save has the game save the open scene to its file, or, without one, does as Save As does.
    ///
    /// A Save As cancelled cancels what was to follow it.
    pub(super) fn choose(&mut self, cx: &mut Context, item: Item) {
        match item {
            Item::New => self.act(Action::New),
            Item::Open => {
                if let Some(path) = self.pick(cx, false) {
                    self.open(path);
                }
            }
            Item::Save if self.scene.is_some() => self.save(),
            Item::Save | Item::SaveAs => match self.pick(cx, true) {
                Some(path) => self.save_as(path),
                None => self.cancel_then(),
            },
        }
    }

    /// Shows the dialog that picks a scene to open, or, if `save`, a file to save the open scene
    /// as, and returns its absolute path, as the game takes it. It shows scenes, in the open
    /// scene's folder or, without its file, the project's first scene folder, and suggests the
    /// file's name to save as.
    fn pick(&self, cx: &mut Context, save: bool) -> Option<PathBuf> {
        let folders = self.folders.as_ref()?;
        let file = (self.scene.as_deref()).map(|scene| under(&folders.root, scene));
        let directory = file.as_deref().and_then(Path::parent);
        let file_name = file.as_deref().filter(|_| save).and_then(Path::file_name);
        cx.pick_file(&FileDialog {
            title: if save {
                "Save the scene as"
            } else {
                "Open a scene"
            },
            picks: if save { Picks::Save } else { Picks::Open },
            filter: Some(("Scenes", &["ron"])),
            directory: directory.or(folders.scenes.first().map(PathBuf::as_path)),
            file_name: file_name.and_then(OsStr::to_str),
        })
    }

    /// Has the game open the scene at `path` in place of the open one, as [`act`](Self::act) says.
    pub(super) fn open(&mut self, path: PathBuf) {
        self.act(Action::Open(path));
    }

    /// Has the game save the open scene to `picked`, the file Save As's dialog picked, with `.ron`
    /// added unless it ends in it, in any case, as the XDG portal's dialog may leave it. A file
    /// there at the name with `.ron` added is one the dialog didn't ask about replacing, so it
    /// isn't replaced: the editor says so, `editor.scene_exists`, and what was to follow the save
    /// is cancelled.
    pub(super) fn save_as(&mut self, mut picked: PathBuf) {
        let extension = picked.extension();
        if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("ron")) {
            picked.as_mut_os_string().push(".ron");
            if picked.exists() {
                let code = "editor.scene_exists";
                let shown = picked.display();
                let message =
                    format!("{shown} is there already: pick it in the dialog to replace it");
                warn!(code, path = %shown, "{message}");
                self.log(Level::Warn, format!("[{code}] {message}"));
                let name = picked.file_name().map(OsStr::to_string_lossy);
                self.message = Some(Message {
                    title: format!("Can't save {}", name.unwrap_or_default()),
                    text: format!("[{code}] {message}"),
                });
                self.cancel_then();
                return;
            }
        }
        self.request(Scene {
            action: SceneAction::Save,
            path: Some(picked),
            discard: false,
        });
    }

    /// Has the game save the open scene to its file, or, without one as the last status said,
    /// has the next UI choose Save As.
    fn save(&mut self) {
        if self.scene.is_none() {
            self.due = Some(Item::SaveAs);
            return;
        }
        self.request(Scene {
            action: SceneAction::Save,
            path: None,
            discard: false,
        });
    }

    /// Does `action`, a New or Open: prompts first if the last status said the scene has unsaved
    /// edits; otherwise asks the game, which refuses if it has some by then.
    pub(super) fn act(&mut self, action: Action) {
        if self.unsaved {
            self.prompt = Some(action);
        } else if let Some(scene) = action.scene(false) {
            self.request(scene);
        }
    }

    /// Has the File menu's request, `scene`, sent to the game, as [`Request`] says.
    fn request(&mut self, scene: Scene) {
        self.request = Some(Request { scene, sent: false });
    }

    /// Does what `choice` answers the prompt with, for what it asks about.
    ///
    /// - Save saves the open scene, or saves it as, then does the action once the game has saved:
    ///   a New or Open again, or a close going ahead. It does nothing while no game serves.
    /// - Don't Save does the action, discarding the unsaved edits, as a close going ahead does.
    /// - Cancel does nothing; a close, or a switch to another project, goes back to none.
    pub(super) fn answer(&mut self, choice: Choice) {
        let Some(action) = self.prompt.take() else {
            return;
        };
        match choice {
            Choice::Save if self.talker.is_none() => self.prompt = Some(action),
            Choice::Save => {
                self.then = Some(action);
                self.save();
            }
            Choice::DontSave => match action.scene(true) {
                Some(scene) => self.request(scene),
                None => self.closing = Closing::Granted,
            },
            Choice::Cancel if action == Action::Close => self.cancel_close(),
            Choice::Cancel => {}
        }
    }

    /// Cancels what was to follow a save that won't be made: a close going back to none.
    fn cancel_then(&mut self) {
        if self.then.take() == Some(Action::Close) {
            self.cancel_close();
        }
    }

    /// Has a close under way, or a switch to another project, go back to none.
    fn cancel_close(&mut self) {
        self.closing = Closing::No;
        self.switching = None;
    }

    /// Goes on from the File menu's request, `scene`, that the game did.
    ///
    /// - From a New or Open, the view goes through the game's camera, as the editor starts, until
    ///   the editor's adopts where it sees from, so it frames the scene the game has now, not the
    ///   last. The grid goes too, as the user's turn would. Not while a Play or Simulate clicked
    ///   since waits for its answer, which turned the view itself.
    /// - From a save, it goes on to what was to follow it.
    pub(super) fn scene_done(&mut self, scene: &Scene) {
        if scene.action != SceneAction::Save {
            let toolbar = &mut self.view.toolbar;
            if !toolbar.world_camera && self.playing == 0 {
                toolbar.world_camera = true;
                self.adopting = true;
                // `changes`, which runs before the answers are taken, doesn't see this turn.
                self.looking.change(Part::Viewpoint);
                self.looking.follow(Part::Grid);
            }
            return;
        }
        match self.then.take() {
            Some(Action::Close) => self.closing = Closing::Granted,
            Some(action) => self.act(action),
            None => {}
        }
    }

    /// Goes on from the File menu's request, `scene`, that the game refused with `failure`.
    ///
    /// - A New or Open refused for unsaved edits the last status didn't know of prompts, except
    ///   while a close goes on.
    /// - A Save refused for an untitled scene has the next UI choose Save As. While a close goes
    ///   on, only the logged refusal remains, unless it's the close prompt's own Save.
    /// - Any other refusal cancels what was to follow and shows in a message.
    pub(super) fn scene_refused(&mut self, scene: Scene, failure: Failure) {
        let action = match (scene.action, scene.path) {
            (SceneAction::New, _) => Action::New,
            (SceneAction::Open, Some(path)) => Action::Open(path),
            (SceneAction::Save, path) => {
                if failure.code == "scene.untitled" && path.is_none() {
                    // A close clears what was to follow, so a Save followed by one is its prompt's.
                    if self.closing == Closing::No || self.then == Some(Action::Close) {
                        self.due = Some(Item::SaveAs);
                    }
                    return;
                }
                let name = path.as_deref().and_then(Path::file_name);
                let name = name.and_then(OsStr::to_str).or(self.scene_name());
                self.refused(
                    format!("Can't save {}", name.unwrap_or("the scene")),
                    failure,
                );
                return;
            }
            (SceneAction::Open, None) => unreachable!("the File menu opens a file"),
        };
        if failure.code == "scene.unsaved" {
            if self.closing == Closing::No {
                self.prompt = Some(action);
            }
            return;
        }
        let title = match &action {
            Action::Open(path) => {
                let name = path.file_name().map(OsStr::to_string_lossy);
                format!("Can't open {}", name.unwrap_or_default())
            }
            _ => "Can't make a new scene".to_owned(),
        };
        self.refused(title, failure);
    }

    /// Shows `failure`, a refusal of the File menu's request, in a message titled `title`,
    /// cancelling what was to follow it.
    fn refused(&mut self, title: String, failure: Failure) {
        let Failure { code, message } = failure;
        self.message = Some(Message {
            title,
            text: format!("[{code}] {message}"),
        });
        self.cancel_then();
    }

    /// The prompt's title and text for `action`: whether to save the open scene's unsaved edits
    /// first.
    pub(super) fn asks(&self, action: &Action) -> (String, String) {
        let title = match self.scene_name() {
            Some(name) => format!("Save the edits to {name}?"),
            None => "Save the untitled scene?".to_owned(),
        };
        let then = match action {
            Action::New => "a new scene".to_owned(),
            Action::Open(path) => {
                let name = path.file_name().map(OsStr::to_string_lossy);
                format!("{} opens", name.unwrap_or_default())
            }
            Action::Close => match &self.switching {
                Some(dir) => {
                    let name = dir.file_name().map(OsStr::to_string_lossy);
                    format!("the project {} opens", name.unwrap_or_default())
                }
                None => "the editor closes".to_owned(),
            },
        };
        let text = format!("Its unsaved edits are lost unless they're saved before {then}.");
        (title, text)
    }

    /// Whether a close of the editor, by Quit or the window's close button, goes ahead, as
    /// [`Event::CloseRequested`] asks.
    ///
    /// - At once outside a project, before any game has served, and once it's granted.
    /// - Otherwise it's vetoed, `Break`, while the editor checks whether the open scene has unsaved
    ///   edits, asking the game for a status once the Inspector has nothing waiting for it. It's
    ///   granted if none, prompted for if some; a prompt for a New or Open gives way to it.
    /// - While no game serves after one has (`gg run` restarts it, waits for a rebuild, or the game
    ///   crashed), only the last status answers: granted if it said none, otherwise prompted for
    ///   without Save, which no game would do. A close being checked or prompted for as the game
    ///   goes goes the same way.
    /// - A close while the window is minimized waits until it's restored: no frames run meanwhile.
    /// - A switch to another project goes the same way, as [`switch_to`](Self::switch_to) says.
    pub(super) fn close(&mut self) -> ControlFlow<()> {
        match self.closing {
            Closing::Granted => return ControlFlow::Continue(()),
            Closing::Checking(_) | Closing::Prompting => return ControlFlow::Break(()),
            Closing::No if !self.served => return ControlFlow::Continue(()),
            Closing::No => {}
        }
        self.prompt = None;
        self.then = None;
        self.due = None;
        self.closing = Closing::Checking(false);
        if self.talker.is_none() {
            self.settle_close();
        }
        if self.closing == Closing::Granted {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }

    /// Settles a close by whether the open scene has unsaved edits, as the last status said:
    /// prompted for if so, granted otherwise.
    pub(super) fn settle_close(&mut self) {
        if self.unsaved {
            self.prompt = Some(Action::Close);
            self.closing = Closing::Prompting;
        } else {
            self.prompt = None;
            self.closing = Closing::Granted;
        }
    }
}

/// `path`, relative with `/` between its parts, as manifests and the API name files, joined to
/// `dir` a part at a time, so its separators are the OS's.
fn under(dir: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(dir.to_owned(), |under, part| under.join(part))
}
