//! The hub, the editor's start screen, which it shows over its panels while it has no project open,
//! and from its File menu's Open Project, which goes back to the project open: the projects it
//! knows, those opened lately, the latest first, then the rest of those in the engine's workspace,
//! each a card that a click opens in the editor; a search through them; Open, whose dialog picks a
//! project elsewhere by its manifest; New Project, which `gg new` makes in the workspace; and
//! whether the editor opens the latest project as it starts, skipping the hub. The editor keeps the
//! projects opened, and that choice, for the user from run to run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gg_api::{Failure, Project};
use gg_engine::egui::{
    self, Align, Button, Color32, CornerRadius, CursorIcon, FontFamily, Key, Layout, Rect,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, TextStyle, TextWrapMode, UiBuilder,
    WidgetText, pos2, vec2,
};
use gg_engine::{BOLD, Context};
use serde_json::{Value, json};
use tracing::warn;

/// The file the editor keeps the projects in, as [`Context::read_state`] names it.
const FILE: &str = "projects.json";
/// The most projects it keeps, past which the one opened longest ago goes.
const KEPT: usize = 16;
/// How long a listing of the projects lasts, after which they're listed again, so that a project
/// made or removed elsewhere shows.
const RELIST: Duration = Duration::from_secs(1);
/// The widest the hub's column gets, and the least room around it, in points.
const WIDTH: f32 = 760.0;
const MARGIN: f32 = 32.0;
/// A project's card: how tall it is, how far its avatar and text are from its edges, and the side
/// of its avatar, in points.
const CARD: f32 = 64.0;
const INSET: f32 = 12.0;
const AVATAR: f32 = 40.0;
/// The avatars' colors, a project's chosen by its name.
const AVATARS: [Color32; 8] = [
    Color32::from_rgb(0x4c, 0x8d, 0xf6),
    Color32::from_rgb(0x8b, 0x5c, 0xf6),
    Color32::from_rgb(0xec, 0x48, 0x99),
    Color32::from_rgb(0xf5, 0x9e, 0x0b),
    Color32::from_rgb(0x10, 0xb9, 0x81),
    Color32::from_rgb(0x06, 0xb6, 0xd4),
    Color32::from_rgb(0xef, 0x44, 0x44),
    Color32::from_rgb(0x84, 0xcc, 0x16),
];

/// What the user chose on the hub.
#[derive(Debug, PartialEq)]
pub enum Chosen {
    /// Open the project in this folder.
    Open(PathBuf),
    /// Pick a project's manifest with the OS's dialog, whose folder opens.
    Browse,
    /// Go back to the project open, hiding the hub.
    Back,
}

/// The hub, as the module says: what the editor keeps of the projects, the workspace's folder, the
/// projects as they were last listed, and what's typed in its fields.
#[derive(Default)]
pub struct Hub {
    kept: Kept,
    /// Whether `kept` changed since it was last saved.
    changed: bool,
    /// The engine's workspace, whose `games` folder holds projects and gets those `gg new` makes,
    /// if the editor runs in one.
    workspace: Option<PathBuf>,
    listed: Option<Listed>,
    search: String,
    /// The New Project form, while it shows.
    making: Option<Making>,
    /// Why the last project picked couldn't open, until another opens, or it's dismissed.
    refused: Option<String>,
    /// Whether the File menu's Open Project shows it over the project open.
    pub asked: bool,
}

/// What the editor keeps of the projects for the user: those opened, the latest first, and whether
/// it opens the latest as it starts.
#[derive(Clone, Debug, Default, PartialEq)]
struct Kept {
    opened: Vec<Opened>,
    skip: bool,
}

/// A project opened, by its folder, and when, in seconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq)]
struct Opened {
    dir: PathBuf,
    at: u64,
}

/// The projects as they were last listed: those opened, the latest first, then the rest of the
/// workspace's, by their folders' names.
struct Listed {
    at: Instant,
    recent: Vec<Shown>,
    workspace: Vec<Shown>,
}

/// A project the hub shows.
struct Shown {
    /// Its folder, and that as the file system finds it, as [`found`] says, which tells the
    /// project open's card without asking the file system each frame.
    dir: PathBuf,
    found: PathBuf,
    /// Its game's name, as its manifest gives it, or its folder's, if that doesn't read.
    name: String,
    /// Its manifest, or why it doesn't read.
    project: Result<Project, Failure>,
    /// When it was last opened, if the editor keeps it.
    opened: Option<u64>,
}

/// The New Project form: the name typed, and while `gg new` makes it, what its run will send, the
/// folder it made or why it made none; and why the last made none.
#[derive(Default)]
struct Making {
    name: String,
    run: Option<Receiver<Result<PathBuf, String>>>,
    failed: Option<String>,
}

impl Kept {
    /// What `text`, the file's, holds: what of it reads, a project without a folder or a time left
    /// out.
    fn parse(text: &str) -> serde_json::Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let opened = value["opened"].as_array().into_iter().flatten();
        let opened = opened.filter_map(|opened| {
            let dir = PathBuf::from(opened["dir"].as_str()?);
            Some(Opened {
                dir,
                at: opened["at"].as_u64()?,
            })
        });
        Ok(Self {
            opened: opened.collect(),
            skip: value["skip"].as_bool().unwrap_or(false),
        })
    }

    /// Its file's text, which [`parse`](Self::parse) reads back: but a folder whose path isn't
    /// Unicode, which JSON can't hold.
    fn text(&self) -> String {
        let opened = self
            .opened
            .iter()
            .filter_map(|opened| Some(json!({"dir": opened.dir.to_str()?, "at": opened.at})));
        let value = json!({"opened": opened.collect::<Vec<_>>(), "skip": self.skip});
        format!("{value:#}\n")
    }

    /// Notes that the project in `dir` was opened `at`: first, in place of where it was, the one
    /// opened longest ago going past [`KEPT`].
    fn open(&mut self, dir: &Path, at: u64) {
        self.opened.retain(|opened| !same(&opened.dir, dir));
        self.opened.insert(
            0,
            Opened {
                dir: dir.to_owned(),
                at,
            },
        );
        self.opened.truncate(KEPT);
    }
}

impl Hub {
    /// The hub of the editor running in `workspace`, if it runs in one, keeping no projects until
    /// it [`read`](Self::read)s those it kept.
    pub fn new(workspace: Option<PathBuf>) -> Self {
        Self {
            workspace,
            ..Self::default()
        }
    }

    /// Reads what the editor kept of the projects, as `cx` keeps it for the user, warning
    /// (`editor.projects`) if it can't, the hub then keeping none.
    pub fn read(&mut self, cx: &Context) {
        let read = match cx.read_state(FILE) {
            Ok(None) => return,
            Ok(Some(bytes)) => String::from_utf8(bytes).map_err(|error| error.to_string()),
            Err(error) => Err(format!("{error:#}")),
        };
        let kept = read.and_then(|text| Kept::parse(&text).map_err(|error| error.to_string()));
        match kept {
            Ok(kept) => self.kept = kept,
            Err(why) => warn!(
                code = "editor.projects",
                file = FILE,
                "can't read the projects the editor kept: {why}; it keeps them afresh"
            ),
        }
        self.listed = None;
    }

    /// Saves what the editor keeps of the projects, as `cx` keeps it for the user, if it changed
    /// since, warning (`editor.projects`) if it can't.
    pub fn save(&mut self, cx: &Context) {
        if !self.changed {
            return;
        }
        self.changed = false;
        if let Err(error) = cx.save_state(FILE, self.kept.text().as_bytes()) {
            warn!(code = "editor.projects", file = FILE, "{error:#}");
        }
    }

    /// Notes that the project in `dir` opened now, as the latest, which the hub goes back from.
    pub fn opened(&mut self, dir: &Path) {
        self.kept.open(dir, now());
        self.changed = true;
        self.listed = None;
        self.refused = None;
        self.asked = false;
    }

    /// The project the editor opens as it starts, skipping the hub: the latest opened, if it's
    /// chosen to and that's still a project whose manifest reads.
    pub fn skip_to(&self) -> Option<PathBuf> {
        let latest = self.kept.opened.first().filter(|_| self.kept.skip)?;
        let found = Project::find(&latest.dir).is_some_and(|dir| dir == latest.dir);
        (found && Project::read(&latest.dir).is_ok()).then(|| latest.dir.clone())
    }

    /// Shows why the project picked can't open, until another opens.
    pub fn refuse(&mut self, why: String) {
        self.refused = Some(why);
    }

    /// The workspace's `games` folder, if the editor runs in one.
    pub fn games(&self) -> Option<PathBuf> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.join("games"))
    }

    /// Draws the hub into `ui`, the editor's cover, over the project in `current` if one's open,
    /// which its Back goes back to, as Escape does; and returns what the user chose this frame, if
    /// anything. Lists the projects first, unless that was less than a second ago.
    pub fn show(&mut self, ui: &mut egui::Ui, current: Option<&Path>) -> Option<Chosen> {
        let now = Instant::now();
        if !(self.listed.as_ref()).is_some_and(|listed| now.duration_since(listed.at) < RELIST) {
            self.listed = Some(list(&self.kept, self.workspace.as_deref(), now));
        }
        let mut chosen = self.made();
        let full = ui.max_rect();
        let width = (full.width() - 2.0 * MARGIN).clamp(0.0, WIDTH);
        let column = Rect::from_min_max(
            pos2(full.center().x - width / 2.0, full.min.y + MARGIN),
            pos2(full.center().x + width / 2.0, full.max.y - MARGIN / 2.0),
        );
        // As the file system finds it, which the cards' folders are listed as.
        let current = current.map(found);
        ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
            if let Some(dir) = &current {
                let name = self.name_of(dir, dir);
                let back = ui.add(Button::new(format!("← Back to {name}")).frame(false));
                let escaped =
                    !ui.ctx().any_popup_open() && ui.input(|i| i.key_pressed(Key::Escape));
                if back.on_hover_text("Close the hub").clicked() || escaped {
                    chosen = Some(Chosen::Back);
                }
                ui.add_space(8.0);
            }
            chosen = self.header(ui).or(chosen.take());
            ui.add_space(16.0);
            self.forms(ui);
            let field = TextEdit::singleline(&mut self.search)
                .hint_text("Search projects")
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 8.0));
            ui.add(field);
            ui.add_space(12.0);
            let footer = ui.spacing().interact_size.y + 2.0 * ui.spacing().item_spacing.y + 8.0;
            let lists = ui.available_height() - footer;
            ScrollArea::vertical()
                .max_height(lists.max(0.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    chosen = self.lists(ui, current.as_deref()).or(chosen.take());
                });
            ui.separator();
            let skip = ui.checkbox(
                &mut self.kept.skip,
                "Open the latest project as the editor starts, skipping this screen",
            );
            if skip.changed() {
                self.changed = true;
            }
        });
        chosen
    }

    /// What `gg new` made, once it's done: the folder of the project it made, to open; or why it
    /// made none, which the form shows.
    fn made(&mut self) -> Option<Chosen> {
        let making = self.making.as_mut()?;
        let made = match making.run.as_ref()?.try_recv() {
            Ok(made) => made,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("`cargo gg new` ended unheard".to_owned()),
        };
        making.run = None;
        match made {
            Ok(dir) => {
                self.making = None;
                Some(Chosen::Open(dir))
            }
            Err(why) => {
                making.failed = Some(why);
                None
            }
        }
    }

    /// The name the hub shows for the project in `dir`, which the file system finds at `found`:
    /// its game's, as listed, or its folder's.
    fn name_of(&self, dir: &Path, found: &Path) -> String {
        let listed = self.listed.iter().flat_map(|listed| {
            let all = listed.recent.iter().chain(&listed.workspace);
            all.filter(|shown| shown.found == found)
        });
        match listed.map(|shown| shown.name.clone()).next() {
            Some(name) => name,
            None => folder_name(dir),
        }
    }

    /// Draws the hub's header: the engine's mark and name, and Open and New Project, which shows
    /// the form, and returns Browse for Open's click.
    fn header(&mut self, ui: &mut egui::Ui) -> Option<Chosen> {
        let mut chosen = None;
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            let accent = ui.visuals().selection.bg_fill;
            ui.painter()
                .rect_filled(mark, CornerRadius::same(10), accent);
            let gg = RichText::new("GG")
                .size(18.0)
                .family(bold())
                .color(Color32::WHITE);
            paint_centered(ui, mark, gg);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("GGEngine").size(24.0).family(bold()).strong());
                ui.weak("Pick a project to edit, or make one");
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let new = Button::new(RichText::new("New Project").color(Color32::WHITE))
                    .fill(accent)
                    .min_size(vec2(0.0, 32.0));
                let new = ui.add_enabled(self.workspace.is_some() && self.making.is_none(), new);
                let tip = "Make a game of its own in the workspace's games folder, with `gg new`";
                let new = new.on_hover_text(tip).on_disabled_hover_text(
                    "Start the editor in GGEngine's workspace to make a project there",
                );
                if new.clicked() {
                    self.making = Some(Making::default());
                }
                let open = ui.add(Button::new("Open...").min_size(vec2(0.0, 32.0)));
                if open
                    .on_hover_text("Open a project by its manifest, project.ron")
                    .clicked()
                {
                    chosen = Some(Chosen::Browse);
                }
            });
        });
        chosen
    }

    /// Draws why the last project picked couldn't open, while it can't, and the New Project form,
    /// while it shows: the name, why it isn't a game's, if it isn't, and Create, which has `gg new`
    /// make the game, and Cancel; or while `gg new` makes it, a spinner.
    fn forms(&mut self, ui: &mut egui::Ui) {
        let error = ui.visuals().error_fg_color;
        if let Some(why) = &self.refused {
            let mut dismissed = false;
            card_frame(ui).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(error, why.as_str());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        dismissed = ui.small_button("Dismiss").clicked();
                    });
                });
            });
            if dismissed {
                self.refused = None;
            }
            ui.add_space(12.0);
        }
        let (Some(making), Some(workspace)) = (&mut self.making, &self.workspace) else {
            return;
        };
        let mut cancelled = false;
        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("New Project")
                    .size(16.0)
                    .family(bold())
                    .strong(),
            );
            if making.run.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Making {}...", making.name));
                });
                return;
            }
            let fine = new_project(&making.name);
            ui.horizontal(|ui| {
                let field = TextEdit::singleline(&mut making.name)
                    .hint_text("Name, such as my_game")
                    .desired_width(240.0);
                let typed = ui.add(field);
                let entered = typed.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let create = ui.add_enabled(fine.is_ok(), Button::new("Create"));
                if (create.clicked() || entered) && fine.is_ok() {
                    making.failed = None;
                    making.run = Some(make(workspace, &making.name));
                }
                cancelled = ui.button("Cancel").clicked();
            });
            let games = workspace.join("games");
            match (&fine, &making.failed) {
                (_, Some(failed)) => {
                    ui.colored_label(error, failed.as_str());
                }
                (Err(why), None) if !making.name.is_empty() => {
                    ui.colored_label(error, why.as_str());
                }
                _ => {
                    ui.weak(format!("Made in {} by `cargo gg new`", games.display()));
                }
            }
        });
        if cancelled {
            self.making = None;
        }
        ui.add_space(12.0);
    }

    /// Draws the projects whose names or folders' names hold the search, whatever their case,
    /// those opened then the workspace's, each a card, as [`card`] draws it, the one open, which
    /// the file system finds at `current`, outlined; and returns the one a click chose, or Back for
    /// a click on the one open. A card's menu has Open, and for one opened, Remove From List.
    fn lists(&mut self, ui: &mut egui::Ui, current: Option<&Path>) -> Option<Chosen> {
        let listed = self.listed.as_ref()?;
        let search = self.search.trim().to_lowercase();
        let matches = |shown: &&Shown| {
            search.is_empty()
                || shown.name.to_lowercase().contains(&search)
                || folder_name(&shown.dir).to_lowercase().contains(&search)
        };
        let now = now();
        let mut chosen = None;
        let mut removed = None;
        let sections = [
            ("RECENT", &listed.recent),
            ("IN THIS WORKSPACE", &listed.workspace),
        ];
        let mut any = false;
        for (title, projects) in sections {
            let shown: Vec<&Shown> = projects.iter().filter(matches).collect();
            if shown.is_empty() {
                continue;
            }
            any = true;
            ui.label(RichText::new(title).size(11.0).family(bold()).weak());
            ui.add_space(4.0);
            for shown in shown {
                let open = current == Some(shown.found.as_path());
                let response = card(ui, shown, open, now);
                if response.clicked() {
                    chosen = Some(if open {
                        Chosen::Back
                    } else {
                        Chosen::Open(shown.dir.clone())
                    });
                }
                response.context_menu(|ui| {
                    if ui.button("Open").clicked() {
                        chosen = Some(Chosen::Open(shown.dir.clone()));
                    }
                    let kept = shown.opened.is_some();
                    let remove = ui.add_enabled(kept, Button::new("Remove From List"));
                    if remove.clicked() {
                        removed = Some(shown.dir.clone());
                    }
                });
                ui.add_space(6.0);
            }
            ui.add_space(10.0);
        }
        if !any {
            let none = if search.is_empty() {
                "No projects yet: open one, or make one"
            } else {
                "No project matches the search"
            };
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.weak(none));
        }
        if let Some(dir) = removed {
            self.kept.opened.retain(|opened| !same(&opened.dir, &dir));
            self.changed = true;
            self.listed = None;
        }
        chosen
    }
}

/// The engine's workspace that `from`, the working directory, or else `exe`, the editor's
/// executable, is in: the nearest of their ancestors with a `games` folder and the `Cargo.toml` of
/// a workspace, where `gg new` makes games.
pub fn workspace(from: Option<&Path>, exe: Option<&Path>) -> Option<PathBuf> {
    let mut ancestors = from.into_iter().chain(exe).flat_map(Path::ancestors);
    let found = ancestors.find(|dir| {
        let manifest = fs::read_to_string(dir.join("Cargo.toml"));
        dir.join("games").is_dir() && manifest.is_ok_and(|text| text.contains("[workspace]"))
    });
    found.map(Path::to_owned)
}

/// The projects to show, as of `now`, as [`Listed`] orders them: those `kept` has opened, which
/// show whether their manifests read or not, then the rest of those in `workspace`'s `games`
/// folder, each a project's folder, with its manifest beside a `Cargo.toml`.
fn list(kept: &Kept, workspace: Option<&Path>, now: Instant) -> Listed {
    let recent: Vec<Shown> = (kept.opened.iter())
        .map(|opened| shown(&opened.dir, Some(opened.at)))
        .collect();
    let games = workspace.map(|workspace| workspace.join("games"));
    let entries = games.and_then(|games| fs::read_dir(games).ok());
    let dirs = entries.into_iter().flatten().filter_map(|entry| {
        let dir = entry.ok()?.path();
        (Project::find(&dir).as_deref() == Some(dir.as_path())).then_some(dir)
    });
    let all = dirs.map(|dir| shown(&dir, None));
    let fresh = all.filter(|shown| !recent.iter().any(|opened| opened.found == shown.found));
    let mut workspace: Vec<Shown> = fresh.collect();
    workspace.sort_by_cached_key(|shown| (shown.name.to_lowercase(), shown.dir.clone()));
    Listed {
        at: now,
        recent,
        workspace,
    }
}

/// The project in `dir` as the hub shows it, opened `opened`, if it was.
fn shown(dir: &Path, opened: Option<u64>) -> Shown {
    let project = Project::read(dir);
    let name = match &project {
        Ok(project) => project.game.clone(),
        Err(_) => folder_name(dir),
    };
    Shown {
        dir: dir.to_owned(),
        found: found(dir),
        name,
        project,
        opened,
    }
}

/// Folder `dir` as the file system finds it, links and `..` resolved, and in its case on a file
/// system that ignores case; as it's written if it isn't there.
fn found(dir: &Path) -> PathBuf {
    fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned())
}

/// Draws `shown`'s card into `ui`, as wide as it, lit while the pointer's over it, and outlined
/// in the accent while it's the project `open`: its avatar, its name, and its folder, or why its
/// manifest doesn't read; and at its right, when it was opened, `now` being the time, and the scene
/// it starts in.
fn card(ui: &mut egui::Ui, shown: &Shown, open: bool, now: u64) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, CARD), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let (fill, stroke) = if response.hovered() {
        let lit = &visuals.widgets.hovered;
        (lit.weak_bg_fill, lit.bg_stroke)
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    let stroke = if open {
        Stroke::new(1.5, visuals.selection.bg_fill)
    } else {
        stroke
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    let avatar = Rect::from_min_size(
        pos2(rect.min.x + INSET, rect.center().y - AVATAR / 2.0),
        vec2(AVATAR, AVATAR),
    );
    painter.rect_filled(avatar, CornerRadius::same(8), avatar_color(&shown.name));
    let initial: String = shown
        .name
        .chars()
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    let initial = RichText::new(initial)
        .size(18.0)
        .family(bold())
        .color(Color32::WHITE);
    paint_centered(ui, avatar, initial);
    let right = 200.0;
    let text_left = avatar.max.x + INSET;
    let text_width = (rect.max.x - INSET - right - text_left).max(0.0);
    let name = RichText::new(shown.name.as_str()).size(15.0).family(bold());
    let name_at = pos2(text_left, rect.min.y + 13.0);
    paint_left(ui, name_at, name, text_width, visuals.strong_text_color());
    let (detail, color) = match &shown.project {
        Ok(_) => (shown.dir.display().to_string(), visuals.weak_text_color()),
        Err(failure) => (failure.message.clone(), visuals.error_fg_color),
    };
    let detail = RichText::new(detail).size(12.0);
    paint_left(
        ui,
        pos2(text_left, rect.min.y + 36.0),
        detail,
        text_width,
        color,
    );
    let weak = visuals.weak_text_color();
    if let Some(at) = shown.opened {
        let ago = RichText::new(ago(at, now)).size(12.0);
        paint_right(
            ui,
            pos2(rect.max.x - INSET, rect.min.y + 15.0),
            ago,
            right,
            weak,
        );
    }
    if let Ok(project) = &shown.project {
        let start = RichText::new(format!("Starts in {}", project.start)).size(12.0);
        paint_right(
            ui,
            pos2(rect.max.x - INSET, rect.min.y + 36.0),
            start,
            right,
            weak,
        );
    }
    if open {
        let tip = "The project open: a click goes back to it";
        return response.on_hover_text(tip);
    }
    response
}

/// Paints `text` in `ui` from `at`, its top left, cut short with an ellipsis past `width`.
fn paint_left(ui: &egui::Ui, at: egui::Pos2, text: RichText, width: f32, color: Color32) {
    let galley = galley(ui, text, width);
    ui.painter().galley(at, galley, color);
}

/// Paints `text` in `ui` back from `at`, its top right, cut short with an ellipsis past `width`.
fn paint_right(ui: &egui::Ui, at: egui::Pos2, text: RichText, width: f32, color: Color32) {
    let galley = galley(ui, text, width);
    let at = pos2(at.x - galley.size().x, at.y);
    ui.painter().galley(at, galley, color);
}

/// Paints `text` in `ui` in the middle of `rect`, in its own color.
fn paint_centered(ui: &egui::Ui, rect: Rect, text: RichText) {
    let galley = galley(ui, text, rect.width());
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, Color32::WHITE);
}

/// `text` laid out on a line in `ui`, cut short with an ellipsis past `width`.
fn galley(ui: &egui::Ui, text: RichText, width: f32) -> std::sync::Arc<egui::Galley> {
    let text = WidgetText::from(text);
    text.into_galley(ui, Some(TextWrapMode::Truncate), width, TextStyle::Body)
}

/// The frame of a card that holds a form or a message, in `ui`.
fn card_frame(ui: &egui::Ui) -> egui::Frame {
    let visuals = ui.visuals();
    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(INSET)
}

/// The UI's bold.
fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

/// The color of the avatar of a project named `name`: one of [`AVATARS`], by the top three bits
/// of its name's FNV-1a hash, which tell short names apart better than its low bits, so that a
/// project keeps its color.
fn avatar_color(name: &str) -> Color32 {
    let hash = (name.bytes()).fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    AVATARS[(hash >> 61) as usize % AVATARS.len()]
}

/// When a project was opened, `at`, as its card says, `now` being the time, in seconds since the
/// Unix epoch.
fn ago(at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(at);
    match seconds {
        0..60 => "Opened just now".to_owned(),
        60..3_600 => format!("Opened {} min ago", seconds / 60),
        3_600..86_400 => format!("Opened {} h ago", seconds / 3_600),
        86_400..172_800 => "Opened yesterday".to_owned(),
        _ => format!("Opened {} days ago", seconds / 86_400),
    }
}

/// Why `name` can't be a new game's, as `gg new` checks it: its project's, which a name that isn't
/// a package's fails.
fn new_project(name: &str) -> Result<(), String> {
    let project = Project {
        game: name.to_owned(),
        assets: ".".to_owned(),
        scenes: vec!["scenes".to_owned()],
        start: "scenes/main.ron".to_owned(),
    };
    project.check().map_err(|failure| failure.message)
}

/// Runs `cargo gg new <name>` in `workspace` on a thread of its own, whose result comes back as
/// [`made`] reads it.
fn make(workspace: &Path, name: &str) -> Receiver<Result<PathBuf, String>> {
    let (send, made_it) = mpsc::channel();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(["gg", "new", name])
        .current_dir(workspace)
        .stdin(Stdio::null());
    thread::spawn(move || {
        let output = command.output();
        let result = output
            .map_err(|error| format!("can't run `cargo gg new`: {error}"))
            .and_then(|output| made(&output));
        let _ = send.send(result);
    });
    made_it
}

/// What `gg new`'s `output` says: the folder of the game it made, as the line of JSON it prints
/// names it; or why it made none: its error, as it prints one, or else what it printed.
fn made(output: &Output) -> Result<PathBuf, String> {
    let lines = |bytes: &[u8]| -> Vec<Value> {
        let text = String::from_utf8_lossy(bytes);
        let lines = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok());
        lines.collect()
    };
    let dir =
        (lines(&output.stdout).iter()).find_map(|line| line["dir"].as_str().map(PathBuf::from));
    if let Some(dir) = dir.filter(|_| output.status.success()) {
        return Ok(dir);
    }
    let error = lines(&output.stderr).into_iter().find_map(|line| {
        let error = &line["error"];
        Some(format!(
            "[{}] {}",
            error["code"].as_str()?,
            error["message"].as_str()?
        ))
    });
    let printed = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(error.unwrap_or(if printed.is_empty() {
        format!("`cargo gg new` ended, {}", output.status)
    } else {
        printed
    }))
}

/// Whether `a` and `b` are the same folder: as they're written, or as the file system finds them.
pub fn same(a: &Path, b: &Path) -> bool {
    a == b || matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// The name of folder `dir`, as it shows.
fn folder_name(dir: &Path) -> String {
    let name = dir.file_name().unwrap_or(dir.as_os_str());
    name.to_string_lossy().into_owned()
}

/// The time, in seconds since the Unix epoch.
fn now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use gg_engine::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    use super::*;
    use crate::assets::tests::{Scratch, find, texts};

    /// A project `game` in folder `dir`: its manifest beside a `Cargo.toml`.
    fn project(dir: &Path, game: &str) {
        fs::create_dir_all(dir).expect("made");
        let manifest = Project {
            game: game.to_owned(),
            assets: ".".to_owned(),
            scenes: vec!["scenes".to_owned()],
            start: "scenes/main.ron".to_owned(),
        };
        fs::write(dir.join(Project::FILE), manifest.text()).expect("written");
        fs::write(dir.join("Cargo.toml"), "").expect("written");
    }

    /// A workspace in `dir`, its `games` holding projects `a` and `b`, and a folder `c` without a
    /// `Cargo.toml` beside its manifest, which isn't a project.
    fn workspace_in(dir: &Path) {
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").expect("written");
        project(&dir.join("games/b"), "b");
        project(&dir.join("games/a"), "a");
        project(&dir.join("games/c"), "c");
        fs::remove_file(dir.join("games/c/Cargo.toml")).expect("removed");
    }

    /// Each project opened goes first, once, the oldest going past sixteen; and what's kept reads
    /// back as it was, but for what of a file doesn't read, which is left out.
    #[test]
    fn the_projects_opened_read_back_the_latest_first() {
        let mut kept = Kept::default();
        for (n, at) in [(1, 10), (2, 20), (1, 30)] {
            kept.open(Path::new(&format!("/games/{n}")), at);
        }
        let dirs: Vec<_> = kept
            .opened
            .iter()
            .map(|opened| opened.dir.clone())
            .collect();
        assert_eq!(dirs, [PathBuf::from("/games/1"), PathBuf::from("/games/2")]);
        assert_eq!(kept.opened[0].at, 30);
        kept.skip = true;
        assert_eq!(Kept::parse(&kept.text()).expect("read"), kept);
        for n in 0..20 {
            kept.open(Path::new(&format!("/more/{n}")), 40 + n);
        }
        assert_eq!(kept.opened.len(), KEPT);
        assert_eq!(kept.opened[0].dir, Path::new("/more/19"));
        let partly = r#"{"opened": [{"dir": "/a", "at": 1}, {"dir": 7}, {"at": 2}], "skip": "no"}"#;
        let read = Kept::parse(partly).expect("read");
        let only = Opened {
            dir: PathBuf::from("/a"),
            at: 1,
        };
        assert_eq!(
            read,
            Kept {
                opened: vec![only],
                skip: false
            }
        );
        assert!(Kept::parse("not json").is_err());
    }

    /// The workspace is the nearest ancestor of where the editor runs, or else of its executable,
    /// with a `games` folder and a workspace's `Cargo.toml`; its projects show after those opened,
    /// by name, each once, a folder that isn't a project left out, and one opened that's gone
    /// showing why. The latest opened is skipped to only while that's chosen and it's a project.
    #[test]
    fn the_hub_lists_those_opened_then_the_workspaces_projects() {
        let scratch = Scratch::new("hub-listed");
        workspace_in(&scratch.0);
        let inside = scratch.0.join("games/a/src");
        fs::create_dir_all(&inside).expect("made");
        assert_eq!(workspace(Some(&inside), None), Some(scratch.0.clone()));
        let exe = scratch.0.join("target/debug/gg-editor.exe");
        assert_eq!(
            workspace(Some(Path::new("/")), Some(&exe)),
            Some(scratch.0.clone())
        );
        let mut kept = Kept::default();
        kept.open(&scratch.0.join("gone"), 1);
        kept.open(&scratch.0.join("games/b"), 2);
        let listed = list(&kept, Some(&scratch.0), Instant::now());
        let names = |shown: &[Shown]| -> Vec<String> {
            shown.iter().map(|shown| shown.name.clone()).collect()
        };
        assert_eq!(names(&listed.recent), ["b", "gone"]);
        assert_eq!(names(&listed.workspace), ["a"]);
        let gone = &listed.recent[1].project;
        assert_eq!(
            gone.as_ref().err().map(|f| f.code.as_str()),
            Some("project.read")
        );
        let mut hub = Hub::new(Some(scratch.0.clone()));
        hub.kept = kept;
        assert_eq!(hub.skip_to(), None, "not chosen");
        hub.kept.skip = true;
        assert_eq!(hub.skip_to(), Some(scratch.0.join("games/b")));
        hub.kept.open(&scratch.0.join("gone"), 3);
        assert_eq!(hub.skip_to(), None, "gone");
    }

    /// A frame of `hub` over the project in `current`, if one's open, filling a window 800 points
    /// across, with `events`; what it chose, and the text it drew.
    fn frame(
        ctx: &egui::Context,
        hub: &mut Hub,
        current: Option<&Path>,
        events: Vec<Event>,
    ) -> (Option<Chosen>, Vec<(String, Rect)>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..RawInput::default()
        };
        let mut chosen = None;
        let mut output = ctx.run_ui(input, |ui| chosen = hub.show(ui, current));
        output.textures_delta.clear();
        (chosen, texts(&output.shapes))
    }

    /// A click on a project's card opens it, and on the open project's goes back to it, as Back
    /// does, and Escape; Open asks for the dialog; and the search shows only the projects that
    /// match it.
    #[test]
    fn a_click_on_a_card_opens_its_project() {
        let scratch = Scratch::new("hub-clicked");
        workspace_in(&scratch.0);
        let mut hub = Hub::new(Some(scratch.0.clone()));
        let ctx = egui::Context::default();
        let a = scratch.0.join("games/a");
        let b = scratch.0.join("games/b");
        let click = |hub: &mut Hub, current: Option<&Path>, at: Pos2| {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            frame(&ctx, hub, current, vec![Event::PointerMoved(at)]);
            frame(&ctx, hub, current, vec![button(true)]);
            frame(&ctx, hub, current, vec![button(false)]).0
        };
        let (_, shown) = frame(&ctx, &mut hub, None, Vec::new());
        assert_eq!(
            click(&mut hub, None, find(&shown, "b").center()),
            Some(Chosen::Open(b))
        );
        assert_eq!(
            click(&mut hub, None, find(&shown, "Open...").center()),
            Some(Chosen::Browse)
        );
        let (_, shown) = frame(&ctx, &mut hub, Some(&a), Vec::new());
        let open = click(&mut hub, Some(&a), find(&shown, "a").center());
        assert_eq!(open, Some(Chosen::Back));
        let back = click(&mut hub, Some(&a), find(&shown, "← Back to a").center());
        assert_eq!(back, Some(Chosen::Back));
        let escape = Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(
            frame(&ctx, &mut hub, Some(&a), vec![escape]).0,
            Some(Chosen::Back)
        );
        hub.search = "B".to_owned();
        let (_, shown) = frame(&ctx, &mut hub, None, Vec::new());
        assert!(shown.iter().any(|(text, _)| text == "b"));
        assert!(!shown.iter().any(|(text, _)| text == "a"), "{shown:?}");
    }

    /// A new game's name is checked as `gg new` checks it; and what `gg new` printed says the
    /// folder it made, or why it made none.
    #[test]
    fn gg_news_output_says_what_it_made() {
        assert_eq!(new_project("my_game"), Ok(()));
        for refused in ["2fast", "a b", "-fast", "nul"] {
            assert!(new_project(refused).is_err(), "{refused}");
        }
        let output = |code: i32, stdout: &str, stderr: &str| Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        let printed = r#"{"new":"probe","dir":"/w/games/probe","files":[]}"#;
        assert_eq!(
            made(&output(0, printed, "")),
            Ok(PathBuf::from("/w/games/probe"))
        );
        let refused = r#"{"error":{"code":"gg.new","message":"/w/games/probe is there already"}}"#;
        let refused = made(&output(1, "", refused));
        assert_eq!(
            refused,
            Err("[gg.new] /w/games/probe is there already".to_owned())
        );
        assert_eq!(
            made(&output(101, "", "error: no such command")),
            Err("error: no such command".to_owned())
        );
    }

    /// An exit status of `code`, as a process that ended so gives.
    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        let status = std::os::unix::process::ExitStatusExt::from_raw(code << 8);
        #[cfg(windows)]
        let status = std::os::windows::process::ExitStatusExt::from_raw(code as u32);
        status
    }
}
