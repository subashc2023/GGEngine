//! The Scenes panel: the scene files in the project's scenes folders, a row each, a click on which
//! has the game open it in place of the open scene, as the File menu's Open does, the Entities
//! panel then showing its entities: the open scene's row lit, and marked while it has unsaved
//! edits, and the start scene's tagged; with New, as the File menu's.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gg_engine::egui::{self, Align, Atom, Button, Layout, ScrollArea, Shape, Stroke, Vec2, vec2};
use tracing::warn;

/// How long a listing of the scenes lasts, after which they're listed again, so that what else
/// adds, removes or renames there shows, such as an agent's scene.
const RELIST: Duration = Duration::from_secs(1);
/// The side of a row's icon, and the height of a row, in points.
const ICON: f32 = 14.0;
const ROW: f32 = 24.0;

/// What a click in the panel chose.
#[derive(Debug, PartialEq)]
pub enum Chosen {
    /// Open the scene at this path in place of the open one.
    Open(PathBuf),
    /// Empty the open scene, as the File menu's New does.
    New,
}

/// The Scenes panel, as the module says: the scenes folders, the asset root, which names a scene as
/// the game does, the start scene, and the scenes as they were last listed.
pub struct Scenes {
    root: PathBuf,
    folders: Vec<PathBuf>,
    /// The start scene, as the asset root names it.
    start: Option<String>,
    listed: Option<Listed>,
}

/// The scenes, as they were last listed: each folder's in the manifest's order, and why each
/// folder that couldn't be listed couldn't.
struct Listed {
    at: Instant,
    scenes: Vec<Scene>,
    unlisted: Vec<String>,
}

/// A scene file.
struct Scene {
    path: PathBuf,
    /// As the asset root names it, with `/` between its parts, as the game names the open scene,
    /// such as `scenes/arena.ron`.
    name: String,
    /// What its row says: its file's name without `.ron`, after the folders it's in inside its
    /// scenes folder, such as `levels/one`.
    label: String,
}

impl Scenes {
    /// The panel of the scenes in `folders`, inside the asset root `root`, which start in `start`.
    pub fn new(root: PathBuf, folders: Vec<PathBuf>, start: Option<&Path>) -> Self {
        let start = start.map(|start| name_under(&root, start));
        Self {
            root,
            folders,
            start,
            listed: None,
        }
    }

    /// Draws the panel into `ui`: how many scenes there are and New, then a row for each scene,
    /// `open` naming the open scene, as the asset root names it, which has unsaved edits if
    /// `unsaved`; and returns what a click chose, which only the open scene's row and none while
    /// not `enabled` does. Lists the scenes first, unless that was less than a second ago.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        open: Option<&str>,
        unsaved: bool,
        enabled: bool,
    ) -> Option<Chosen> {
        let now = Instant::now();
        if !(self.listed.as_ref()).is_some_and(|listed| now.duration_since(listed.at) < RELIST) {
            self.list(now);
        }
        let listed = self.listed.as_ref()?;
        let mut chosen = None;
        ui.horizontal(|ui| {
            match listed.scenes.len() {
                1 => ui.weak("1 scene"),
                n => ui.weak(format!("{n} scenes")),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = "Empty the open scene, which is untitled until it's saved";
                let new = ui
                    .add_enabled(enabled, Button::new("New"))
                    .on_hover_text(tip);
                if new.clicked() {
                    chosen = Some(Chosen::New);
                }
            });
        });
        ui.separator();
        for why in &listed.unlisted {
            ui.colored_label(ui.visuals().error_fg_color, why.as_str());
        }
        if listed.scenes.is_empty() && listed.unlisted.is_empty() {
            ui.weak("No scenes yet: Save As saves one here");
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for scene in &listed.scenes {
                let is_open = open == Some(scene.name.as_str());
                let label = if is_open && unsaved {
                    format!("{} *", scene.label)
                } else {
                    scene.label.clone()
                };
                let width = ui.available_width();
                let mut row =
                    Button::selectable(is_open, (icon(), label)).min_size(vec2(width, ROW));
                if self.start.as_deref() == Some(scene.name.as_str()) {
                    row = row.shortcut_text("Start");
                }
                let tip = if is_open {
                    format!("{}, the open scene", scene.name)
                } else {
                    format!("Open {} in place of the open scene", scene.name)
                };
                let row = ui.add_enabled(enabled || is_open, row);
                let row = (row.on_hover_text(tip))
                    .on_disabled_hover_text("No game serves to open it in, or the last ask waits");
                if row.clicked() && !is_open {
                    chosen = Some(Chosen::Open(scene.path.clone()));
                }
            }
        });
        chosen
    }

    /// Lists the scenes, as of `now`, each once, as [`scene_files`] finds them, and warns of a
    /// folder it can't list, unless it couldn't the last time either, for the same reason.
    fn list(&mut self, now: Instant) {
        let mut scenes: Vec<Scene> = Vec::new();
        let mut unlisted = Vec::new();
        for folder in &self.folders {
            let files = match scene_files(folder) {
                Ok(files) => files,
                Err(error) => {
                    unlisted.push(format!("Can't list {}: {error}", folder.display()));
                    continue;
                }
            };
            let mut found: Vec<Scene> = (files.into_iter())
                .filter(|path| !scenes.iter().any(|scene| scene.path == *path))
                .map(|path| Scene {
                    name: name_under(&self.root, &path),
                    label: label(folder, &path),
                    path,
                })
                .collect();
            found.sort_by_cached_key(|scene| (scene.label.to_lowercase(), scene.label.clone()));
            scenes.extend(found);
        }
        let before = self.listed.as_ref().map(|listed| &listed.unlisted);
        for why in &unlisted {
            if !before.is_some_and(|before| before.contains(why)) {
                warn!(code = "editor.unlisted_folder", "{why}");
            }
        }
        self.listed = Some(Listed {
            at: now,
            scenes,
            unlisted,
        });
    }
}

/// The scene files in `folder` and the folders inside it: the regular files whose names end in
/// `.ron`, in any case, links not followed, as the game's scan takes no asset through one.
///
/// They're listed on the main thread, which a scenes folder of many thousands of files would hold
/// up for some milliseconds each second the panel shows; a thread watching it wouldn't.
fn scene_files(folder: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let (path, kind) = (entry.path(), entry.file_type()?);
            let scene = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ron"));
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() && scene {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// `path` as directory `root` names what's in it: the parts after it, with `/` between them, or
/// `path` whole if it isn't inside.
fn name_under(root: &Path, path: &Path) -> String {
    let under = path.strip_prefix(root).unwrap_or(path);
    let parts = under.iter().map(|part| part.to_string_lossy());
    parts.collect::<Vec<_>>().join("/")
}

/// What the row of the scene at `path`, in scenes folder `folder`, says, as [`Scene`]'s `label`.
fn label(folder: &Path, path: &Path) -> String {
    name_under(folder, &path.with_extension(""))
}

/// A row's icon: a frame round a hill and a sun, a scene's, painted as the toolbar's icons are.
fn icon() -> Atom<'static> {
    Atom::paint(Vec2::splat(ICON), |ui, args| {
        let (rect, color) = (args.rect, args.fallback_text_color);
        let scale = rect.width() / 16.0;
        let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
        let painter = ui.painter();
        let line = Stroke::new(1.25 * scale, color);
        let frame = egui::Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
        painter.rect_stroke(frame, 1.5 * scale, line, egui::StrokeKind::Inside);
        let hill = vec![
            at(2.5, 12.5),
            at(6.5, 7.5),
            at(9.5, 10.5),
            at(11.0, 9.0),
            at(13.5, 12.5),
        ];
        painter.add(Shape::convex_polygon(hill, color, Stroke::NONE));
        painter.circle_filled(at(11.0, 5.5), 1.5 * scale, color);
    })
}

#[cfg(test)]
mod tests {
    use gg_engine::egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

    use super::*;
    use crate::assets::tests::{Scratch, find, texts};

    /// A project's root holding scenes folders `scenes` and `menus`: `scenes` holds `b.ron`,
    /// `A.RON`, a note and `levels/one.ron`; `menus` holds `title.ron`; and another folder a scene
    /// too.
    fn root(scratch: &Scratch) -> Vec<PathBuf> {
        let root = &scratch.0;
        for folder in ["scenes/levels", "menus", "other"] {
            fs::create_dir_all(root.join(folder)).expect("made");
        }
        for file in [
            "scenes/b.ron",
            "scenes/A.RON",
            "scenes/notes.txt",
            "scenes/levels/one.ron",
            "menus/title.ron",
            "other/x.ron",
        ] {
            fs::write(root.join(file), "(\n    entities: {},\n)\n").expect("written");
        }
        vec![root.join("scenes"), root.join("menus")]
    }

    /// The scenes folders list their scenes, each folder's by name regardless of case, the folders
    /// inside them among them, each named as the asset root names it; and one that isn't there
    /// says why.
    #[test]
    fn the_scenes_folders_list_their_scenes() {
        let scratch = Scratch::new("scenes-listed");
        let mut folders = root(&scratch);
        folders.push(scratch.0.join("gone"));
        let mut panel = Scenes::new(scratch.0.clone(), folders, None);
        panel.list(Instant::now());
        let listed = panel.listed.as_ref().expect("listed");
        let shown: Vec<(&str, &str)> = (listed.scenes.iter())
            .map(|scene| (scene.label.as_str(), scene.name.as_str()))
            .collect();
        let expected = [
            ("A", "scenes/A.RON"),
            ("b", "scenes/b.ron"),
            ("levels/one", "scenes/levels/one.ron"),
            ("title", "menus/title.ron"),
        ];
        assert_eq!(shown, expected);
        assert_eq!(listed.unlisted.len(), 1);
        assert!(
            listed.unlisted[0].starts_with("Can't list"),
            "{:?}",
            listed.unlisted
        );
    }

    /// A frame of `panel` filling a window 300 points across, `b.ron` open, while `enabled`, with
    /// `events`; what it chose, and the text it drew.
    fn frame(
        ctx: &egui::Context,
        panel: &mut Scenes,
        enabled: bool,
        events: Vec<Event>,
    ) -> (Option<Chosen>, Vec<(String, Rect)>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0))),
            events,
            ..RawInput::default()
        };
        let mut chosen = None;
        let mut output = ctx.run_ui(input, |ui| {
            chosen = panel.show(ui, Some("scenes/b.ron"), true, enabled);
        });
        output.textures_delta.clear();
        (chosen, texts(&output.shapes))
    }

    /// A click on a scene's row opens it, but on the open scene's, marked unsaved, and none while
    /// the panel isn't enabled; New chooses New; and the start scene's row is tagged.
    #[test]
    fn a_click_on_a_scene_opens_it() {
        let scratch = Scratch::new("scenes-clicked");
        let folders = root(&scratch);
        let start = scratch.0.join("scenes/levels/one.ron");
        let mut panel = Scenes::new(scratch.0.clone(), folders, Some(&start));
        let ctx = egui::Context::default();
        let click = |panel: &mut Scenes, enabled, at: Pos2| {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            frame(&ctx, panel, enabled, vec![Event::PointerMoved(at)]);
            frame(&ctx, panel, enabled, vec![button(true)]);
            frame(&ctx, panel, enabled, vec![button(false)]).0
        };
        let (_, shown) = frame(&ctx, &mut panel, true, Vec::new());
        find(&shown, "Start");
        let title = find(&shown, "title").center();
        assert_eq!(click(&mut panel, false, title), None, "not enabled");
        let opened = click(&mut panel, true, title);
        assert_eq!(
            opened,
            Some(Chosen::Open(scratch.0.join("menus/title.ron")))
        );
        assert_eq!(click(&mut panel, true, find(&shown, "b *").center()), None);
        assert_eq!(
            click(&mut panel, true, find(&shown, "New").center()),
            Some(Chosen::New)
        );
    }
}
