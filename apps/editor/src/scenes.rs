//! The Scenes panel: a row per scene file in the project's scenes folders.
//!
//! - A click opens the scene in place of the open one, as File > Open does.
//! - The open scene's row is lit, and marked while it has unsaved edits. The start scene's is
//!   tagged.
//! - New, as the File menu's.
//! - A world's cells' files aren't listed as scenes: its file names their folder, or the tool
//!   API's `cells` does while it's open.
//! - An open world of cells, as `cells` reads it, is tagged. Its cells are listed under it,
//!   folded, each marked while it's in and while a save would write its file. A click on one
//!   frames it in the editor's camera.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gg_api::WorldCells;
use gg_engine::ecs::{cells_folders, read_scene};
use gg_engine::egui::{
    self, Align, Atom, Button, CollapsingHeader, Layout, ScrollArea, Shape, Stroke, Vec2, vec2,
};
use tracing::warn;

/// How long a listing lasts before the scenes are listed again, so others' changes there show, such
/// as an agent's new scene.
const RELIST: Duration = Duration::from_secs(1);
/// The side of a row's icon, and the height of a row, in points.
const ICON: f32 = 14.0;
const ROW: f32 = 24.0;

/// What a click in the panel chose.
#[derive(Debug, PartialEq)]
pub enum Chosen {
    /// Open this scene in place of the open one.
    Open(PathBuf),
    /// Empty the open scene, as File > New.
    New,
    /// Frame a cell's bounding sphere, center and radius, in the editor's camera.
    Frame([f64; 3], f32),
}

/// The Scenes panel. Its asset root names scenes as the game does.
pub struct Scenes {
    root: PathBuf,
    folders: Vec<PathBuf>,
    /// The start scene, as the asset root names it.
    start: Option<String>,
    listed: Option<Listed>,
    /// Each scene file's [`cells_folders`] as last read, and when it was modified then.
    worlds: HashMap<PathBuf, (SystemTime, Vec<String>)>,
}

/// The scenes as last listed: each folder's in the manifest's order, their worlds' cells' folders,
/// and why each folder that couldn't be listed couldn't.
struct Listed {
    at: Instant,
    scenes: Vec<Scene>,
    /// As the asset root names them, each with a `/` after.
    cells: Vec<String>,
    unlisted: Vec<String>,
}

struct Scene {
    path: PathBuf,
    /// As the asset root names it, `/`-separated, as the game names the open scene:
    /// `scenes/arena.ron`.
    name: String,
    /// Its row's text: the file's name without `.ron`, after its folders inside its scenes folder,
    /// such as `levels/one`.
    label: String,
}

impl Scenes {
    /// The scenes in `folders`, inside asset root `root`. The game starts in `start`.
    pub fn new(root: PathBuf, folders: Vec<PathBuf>, start: Option<&Path>) -> Self {
        let start = start.map(|start| name_under(&root, start));
        Self {
            root,
            folders,
            start,
            listed: None,
            worlds: HashMap::new(),
        }
    }

    /// Draws the panel: the scene count and New, then a row per scene. Lists the scenes first if
    /// the listing is older than [`RELIST`].
    ///
    /// - `open`: the open scene, as the asset root names it. `unsaved` marks it.
    /// - `cells`: the open world's cells, as the game last read them. Its folder's scenes go under
    ///   its row ([`show_cells`]).
    ///
    /// Returns what a click chose. The open scene's row chooses nothing, and while not `enabled`
    /// only a cell does.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        open: Option<&str>,
        unsaved: bool,
        enabled: bool,
        cells: Option<&WorldCells>,
    ) -> Option<Chosen> {
        let now = Instant::now();
        if !(self.listed.as_ref()).is_some_and(|listed| now.duration_since(listed.at) < RELIST) {
            self.list(now);
        }
        let listed = self.listed.as_ref()?;
        let mut chosen = None;
        // A world's cells' files are its cells, listed under it while it's open.
        let world = cells.map(|cells| format!("{}/", cells.folder));
        let in_folder = |scene: &Scene| {
            let mut folders = listed.cells.iter().chain(&world);
            folders.any(|folder| scene.name.starts_with(folder.as_str()))
        };
        let scenes = || listed.scenes.iter().filter(|scene| !in_folder(scene));
        ui.horizontal(|ui| {
            match scenes().count() {
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
            for scene in scenes() {
                let is_open = open == Some(scene.name.as_str());
                let label = if is_open && unsaved {
                    format!("{} *", scene.label)
                } else {
                    scene.label.clone()
                };
                let width = ui.available_width();
                let mut row =
                    Button::selectable(is_open, (icon(), label)).min_size(vec2(width, ROW));
                let start = self.start.as_deref() == Some(scene.name.as_str());
                let world = is_open && cells.is_some();
                match (start, world) {
                    (true, true) => row = row.shortcut_text("Start, world"),
                    (true, false) => row = row.shortcut_text("Start"),
                    (false, true) => row = row.shortcut_text("World"),
                    (false, false) => {}
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
                if let Some(cells) = cells.filter(|_| is_open)
                    && let Some(framed) = show_cells(ui, cells)
                {
                    chosen = Some(framed);
                }
            }
        });
        chosen
    }

    /// Lists the scenes as of `now`, each once, as [`scene_files`] finds them. Warns of a folder it
    /// can't list, unless it couldn't last time for the same reason.
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
            cells: self.cells(&scenes),
            scenes,
            unlisted,
        });
    }

    /// The folders `scenes`' worlds keep their cells' files in, each with a `/` after, as their
    /// files say ([`cells_folders`]). It reads a file only if it changed since it last did, and
    /// isn't a cell. One that doesn't read as a scene has no cells; opening it says why.
    fn cells(&mut self, scenes: &[Scene]) -> Vec<String> {
        let mut by_depth: Vec<&Scene> = scenes.iter().collect();
        // A world's cells, in a folder under it, are known as cells before they'd be read.
        by_depth.sort_by_key(|scene| scene.name.matches('/').count());
        let mut folders: Vec<String> = Vec::new();
        let mut worlds = HashMap::new();
        for scene in by_depth {
            if folders
                .iter()
                .any(|folder| scene.name.starts_with(folder.as_str()))
            {
                continue;
            }
            let Ok(modified) = fs::metadata(&scene.path).and_then(|file| file.modified()) else {
                continue;
            };
            let known = self.worlds.remove(&scene.path);
            let found = match known.filter(|(read, _)| *read == modified) {
                Some((_, found)) => found,
                // Only a text that says `Cells` is parsed: a debug build parsed the showcase's 44
                // scenes in 219 ms, its 2 worlds in 21.
                None => read_scene(&scene.path)
                    .and_then(|text| {
                        if text.contains("Cells") {
                            cells_folders(&text)
                        } else {
                            Ok(Vec::new())
                        }
                    })
                    .unwrap_or_default(),
            };
            folders.extend(found.iter().map(|folder| format!("{folder}/")));
            worlds.insert(scene.path.clone(), (modified, found));
        }
        self.worlds = worlds;
        folders
    }
}

/// Draws a world's `cells`, folded under a header of how many are in, of how many. A row each,
/// named as its file, marked `in` while in and `*` while a save would write its file. Returns a
/// click on one, which frames it.
fn show_cells(ui: &mut egui::Ui, cells: &WorldCells) -> Option<Chosen> {
    let cells_in = cells.cells.iter().filter(|cell| cell.is_in).count();
    let header = format!("{cells_in} of {} cells in", cells.cells.len());
    let shown = CollapsingHeader::new(header)
        .id_salt("cells")
        .show(ui, |ui| {
            let mut chosen = None;
            for cell in &cells.cells {
                let [x, y, z] = cell.cell;
                let unsaved = if cell.unsaved { " *" } else { "" };
                let width = ui.available_width();
                let mut row =
                    Button::new(format!("{x}_{y}_{z}{unsaved}")).min_size(vec2(width, ROW));
                if cell.is_in {
                    row = row.shortcut_text("in");
                }
                let tip = format!("Frame cell {x}_{y}_{z} of {} in the camera", cells.folder);
                if ui.add(row).on_hover_text(tip).clicked() {
                    let size = cells.size;
                    let center = cell.cell.map(|n| (f64::from(n) + 0.5) * f64::from(size));
                    chosen = Some(Chosen::Frame(center, size * 3f32.sqrt() / 2.0));
                }
            }
            chosen
        });
    shown.body_returned.flatten()
}

/// The scene files in `folder` and its folders: regular files ending in `.ron`, in any case. Links
/// aren't followed, as the game's scan takes no asset through one.
///
/// They're listed on the main thread: a folder of many thousands of files would hold it up some
/// milliseconds each second the panel shows. A watching thread wouldn't.
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

/// `path` as named under directory `root`: its parts after `root`, `/`-separated, or `path` whole
/// if outside.
fn name_under(root: &Path, path: &Path) -> String {
    let under = path.strip_prefix(root).unwrap_or(path);
    let parts = under.iter().map(|part| part.to_string_lossy());
    parts.collect::<Vec<_>>().join("/")
}

/// The [`Scene::label`] of `path` in scenes folder `folder`.
fn label(folder: &Path, path: &Path) -> String {
    name_under(folder, &path.with_extension(""))
}

/// A scene's icon, a hill and a sun in a frame, painted as the toolbar's icons are.
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

    /// A project root with scenes folders `scenes` (holding `b.ron`, `A.RON`, a note and
    /// `levels/one.ron`) and `menus` (`title.ron`), and a scene in another folder.
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

    /// Each scenes folder lists its scenes by name, regardless of case, its folders' among them,
    /// named as the asset root names them. A missing folder says why.
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

    /// A frame of `panel` 300 points wide, `b.ron` open, with `cells` if given, while `enabled`,
    /// with `events`: what it chose, and the text it drew.
    fn frame(
        ctx: &egui::Context,
        panel: &mut Scenes,
        (enabled, cells): (bool, Option<&WorldCells>),
        events: Vec<Event>,
    ) -> (Option<Chosen>, Vec<(String, Rect)>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0))),
            events,
            ..RawInput::default()
        };
        let mut chosen = None;
        let mut output = ctx.run_ui(input, |ui| {
            chosen = panel.show(ui, Some("scenes/b.ron"), true, enabled, cells);
        });
        output.textures_delta.clear();
        (chosen, texts(&output.shapes))
    }

    /// What a click at `at` chooses, in [`frame`]s with `given`.
    fn click(
        ctx: &egui::Context,
        panel: &mut Scenes,
        given: (bool, Option<&WorldCells>),
        at: Pos2,
    ) -> Option<Chosen> {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(ctx, panel, given, vec![Event::PointerMoved(at)]);
        frame(ctx, panel, given, vec![button(true)]);
        frame(ctx, panel, given, vec![button(false)]).0
    }

    /// A click on a row opens it, except the open scene's, marked unsaved, and none while the panel
    /// is disabled. New chooses New. The start scene's row is tagged.
    #[test]
    fn a_click_on_a_scene_opens_it() {
        let scratch = Scratch::new("scenes-clicked");
        let folders = root(&scratch);
        let start = scratch.0.join("scenes/levels/one.ron");
        let mut panel = Scenes::new(scratch.0.clone(), folders, Some(&start));
        let ctx = egui::Context::default();
        let click = |panel: &mut Scenes, enabled, at: Pos2| click(&ctx, panel, (enabled, None), at);
        let (_, shown) = frame(&ctx, &mut panel, (true, None), Vec::new());
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

    /// A world's cells' files aren't listed as scenes while its file names their folder, though no
    /// world is open. Once it no longer does, they are.
    #[test]
    fn a_worlds_cells_files_are_not_scenes() {
        let scratch = Scratch::new("scenes-world");
        let folders = root(&scratch);
        fs::create_dir_all(scratch.0.join("scenes/world")).expect("made");
        for cell in ["0_0_0", "1_0_0"] {
            let path = scratch.0.join(format!("scenes/world/{cell}.ron"));
            fs::write(path, "(\n    entities: {},\n)\n").expect("written");
        }
        let world = scratch.0.join("scenes/world.ron");
        let cells = r#"(
    entities: {
        "00000000000000a0": (
            Cells: (
                folder: "scenes/world",
                size: 10.0,
            ),
        ),
    },
)
"#;
        fs::write(&world, cells).expect("written");
        let mut panel = Scenes::new(scratch.0.clone(), folders, None);
        let ctx = egui::Context::default();
        let shown = |panel: &mut Scenes| {
            panel.list(Instant::now());
            frame(&ctx, panel, (true, None), Vec::new()).1
        };
        let listed = shown(&mut panel);
        find(&listed, "5 scenes");
        find(&listed, "world");
        assert!(
            !listed.iter().any(|(text, _)| text.contains("world/")),
            "{listed:?}"
        );
        // Its time moved on, as a rewrite within a tick of the file system's clock wouldn't.
        let modified = fs::metadata(&world).and_then(|file| file.modified());
        fs::write(&world, "(\n    entities: {},\n)\n").expect("written");
        let file = fs::File::options()
            .write(true)
            .open(&world)
            .expect("opened");
        let later = modified.expect("modified") + Duration::from_secs(2);
        file.set_modified(later).expect("set");
        let listed = shown(&mut panel);
        find(&listed, "7 scenes");
        find(&listed, "world/1_0_0");
    }

    /// The open world's row is tagged, its cells' files listed only under it, folded until its
    /// header's clicked, each marked while in and while unsaved. A click on one frames it, even
    /// while the panel is disabled.
    #[test]
    fn a_worlds_cells_are_listed_under_it() {
        let scratch = Scratch::new("scenes-cells");
        let folders = root(&scratch);
        fs::create_dir_all(scratch.0.join("scenes/cells")).expect("made");
        for cell in ["0_0_0", "1_0_0"] {
            let path = scratch.0.join(format!("scenes/cells/{cell}.ron"));
            fs::write(path, "(\n    entities: {},\n)\n").expect("written");
        }
        let mut panel = Scenes::new(scratch.0.clone(), folders, None);
        let cell = |x: i32, is_in: bool| gg_api::CellAt {
            cell: [x, 0, 0],
            file: true,
            made: false,
            is_in,
            coming: false,
            refused: false,
            kept: 0,
            roots: 1,
            unsaved: is_in,
        };
        let cells = WorldCells {
            entity: "00000000000000a0".to_owned(),
            folder: "scenes/cells".to_owned(),
            size: 10.0,
            viewers: Vec::new(),
            cells: vec![cell(0, true), cell(1, false)],
        };
        let ctx = egui::Context::default();
        let given = (false, Some(&cells));
        let (_, shown) = frame(&ctx, &mut panel, given, Vec::new());
        find(&shown, "World");
        let header = find(&shown, "1 of 2 cells in").center();
        assert!(
            !shown.iter().any(|(text, _)| text.contains("0_0_0")),
            "folded, and not at the top: {shown:?}"
        );
        assert_eq!(click(&ctx, &mut panel, given, header), None);
        // As it unfolds, over frames.
        let unfolded = (0..60).map(|_| frame(&ctx, &mut panel, given, Vec::new()).1);
        let mut unfolded =
            unfolded.skip_while(|shown| !shown.iter().any(|(text, _)| text == "1_0_0"));
        let shown = unfolded.next().expect("unfolded");
        find(&shown, "4 scenes");
        find(&shown, "0_0_0 *");
        find(&shown, "in");
        let one = find(&shown, "1_0_0").center();
        let framed = click(&ctx, &mut panel, given, one);
        assert_eq!(
            framed,
            Some(Chosen::Frame([15.0, 5.0, 5.0], 5.0 * 3f32.sqrt()))
        );
    }
}
