//! Projects: the hub over the panels, a switch to another project, and a project's folders.

use std::mem;
use std::path::{Path, PathBuf};

use gg_engine::{Context, FileDialog, Picks};
use tracing::info;

use crate::hub::{self};

use super::{Closing, Game, Level};
#[cfg(doc)]
use gg_engine::Layer;

/// The project's folders, as its manifest names them: its asset root, which the open scene's file
/// is named from, and its scene folders, where the dialogs start without an open scene's file; and
/// its start scene.
pub(super) struct Folders {
    pub(super) root: PathBuf,
    pub(super) scenes: Vec<PathBuf>,
    pub(super) start: Option<PathBuf>,
}

impl Game {
    /// Whether the hub shows over the panels: while no project is open, and from the File menu's
    /// Open Project until it's gone back from, or another project opens.
    pub(super) fn hub_shows(&self) -> bool {
        self.dir.is_none() || self.hub.asked
    }

    /// Opens the project in `dir`, as the hub picked it, in place of the one open: at once while
    /// no game has served; otherwise as a close of the editor goes, as [`close`](Self::close)
    /// says, the prompt asking whether to save the open scene's unsaved edits before the project
    /// opens, and the switch going, as [`switch`](Self::switch) does, once the close is granted.
    /// The project open itself opens nothing, the hub giving way to it; and none opens while a
    /// close is under way.
    pub(super) fn switch_to(&mut self, dir: PathBuf) {
        if self
            .dir
            .as_deref()
            .is_some_and(|open| hub::same(open, &dir))
        {
            self.hub.asked = false;
            return;
        }
        if self.closing != Closing::No {
            return;
        }
        self.switching = Some(dir);
        if self.close().is_continue() {
            self.closing = Closing::Granted;
        }
    }

    /// Opens the project in `dir` in place of the one open, if one is: lets go of its game, closing
    /// its run's stdin, which ends it, the run going on ending meanwhile; logs what was measured of
    /// its view, and empties the view, which would show its game's last frame until the new game's
    /// first, as a restart's does, through the new project's build; and makes the layer the new
    /// project's, as it starts but for the hub, the Logs, the runs ending and where the toolbar is,
    /// noting the project opened, and starting its run, as [`start`](Self::start) does.
    pub(super) fn switch(&mut self, cx: &mut Context, dir: PathBuf) {
        self.disconnect();
        if let Some((child, stdin)) = self.run.take() {
            drop(stdin);
            let ran = self.dir.clone().unwrap_or_default();
            self.ending.push((ran, child));
        }
        self.measures.report();
        cx.hide_pixels();
        let mut opened = Game::new(Some(dir.clone()), self.view.link.clone());
        opened.release = self.release;
        opened.hub = mem::take(&mut self.hub);
        opened.ending = mem::take(&mut self.ending);
        opened.logs = mem::take(&mut self.logs);
        opened.view.toolbar.placement = mem::take(&mut self.view.toolbar.placement);
        *self = opened;
        info!(code = "editor.project", dir = %dir.display(), "Opened a project");
        self.log(
            Level::Info,
            format!("Opened the project in {}", dir.display()),
        );
        self.hub.opened(&dir);
        self.hub.save(cx);
        self.start();
    }

    /// Picks a project's manifest with the OS's dialog, from [`Layer::ui`], and opens its project,
    /// as [`switch_to`](Self::switch_to) does; the hub says why if the folder it's in isn't a
    /// project's, or its manifest doesn't read.
    pub(super) fn browse(&mut self, cx: &mut Context) {
        let games = self.hub_games();
        let picked = cx.pick_file(&FileDialog {
            title: "Open a project's manifest",
            picks: Picks::Open,
            filter: Some(("Project manifests", &["ron"])),
            directory: games.as_deref(),
            file_name: Some(gg_api::Project::FILE),
        });
        let Some(picked) = picked else {
            return;
        };
        let dir = picked.parent().map(Path::to_owned).unwrap_or_default();
        let found = gg_api::Project::find(&dir).is_some_and(|found| found == dir);
        let read = gg_api::Project::read(&dir);
        match read {
            Ok(_) if found => self.switch_to(dir),
            Ok(_) => self.hub.refuse(format!(
                "{} isn't a game's project: its folder holds no Cargo.toml beside its manifest",
                dir.display()
            )),
            Err(failure) => self
                .hub
                .refuse(format!("[{}] {}", failure.code, failure.message)),
        }
    }

    /// Picks the folder the hub's New Project makes its game in with the OS's dialog, from
    /// [`Layer::ui`], starting in `start`, and has the form make it there.
    pub(super) fn pick_folder(&mut self, cx: &mut Context, start: Option<&Path>) {
        let picked = cx.pick_file(&FileDialog {
            title: "Choose the folder to make the project in",
            picks: Picks::Folder,
            filter: None,
            directory: start,
            file_name: None,
        });
        if let Some(folder) = picked {
            self.hub.make_in(folder);
        }
    }

    /// The folder Open's dialog starts in: the open project's parent, or else the workspace's games.
    fn hub_games(&self) -> Option<PathBuf> {
        let parent = self.dir.as_deref().and_then(Path::parent);
        parent.map(Path::to_owned).or_else(|| self.hub.games())
    }
}

/// The folders of the project in directory `dir`, as its manifest names them; or if the manifest
/// doesn't read, which `gg run` says why of, `dir` as the asset root, and no scene folders.
pub(super) fn folders(dir: &Path) -> Folders {
    match gg_api::Project::read(dir) {
        Ok(project) => Folders {
            root: project.assets_dir(dir),
            scenes: project.scene_dirs(dir),
            start: Some(project.start_path(dir)),
        },
        Err(_) => Folders {
            root: dir.to_owned(),
            scenes: Vec::new(),
            start: None,
        },
    }
}
