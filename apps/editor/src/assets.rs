//! The Assets panel: the files in the project's asset root, a folder at a time, as a content
//! browser shows them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gg_engine::egui::{
    self, Atom, Button, Color32, Id, Painter, Rect, ScrollArea, Sense, Shape, Stroke, TextStyle,
    TextWrapMode, Vec2, WidgetText, pos2, vec2,
};
use tracing::warn;

/// The width of an entry's cell, in points.
const CELL: f32 = 88.0;
/// The side of an entry's icon, in points.
const ICON: f32 = 48.0;
/// The space around an entry's icon and its name, inside its cell, in points.
const PAD: f32 = 4.0;
/// The side of Back's icon, in points, as the toolbar's.
const BACK: f32 = 16.0;
/// How long a listing of the folder shown lasts, after which it's listed again, so that what else
/// adds, removes or renames there shows, such as an agent's scene.
const RELIST: Duration = Duration::from_secs(1);

/// The Assets panel: the files in a folder, its root, and in the folders under it, one folder at a
/// time, as a content browser shows them: folders first, then files, each by its name, in rows as
/// many across as fit. A double click on a folder shows it, and Back the folder above, as far up as
/// the root. Files show dimmed, since nothing opens or drags them yet.
pub struct Assets {
    root: PathBuf,
    /// The folder shown: the root, or a folder under it.
    folder: PathBuf,
    /// What the folder held as it was last listed: `None` until it's listed, as it is again once
    /// that's a second old, or another folder shows.
    listed: Option<Listed>,
}

/// A folder, as it was listed.
struct Listed {
    at: Instant,
    /// Its entries, as [`entries`] orders them; or why it couldn't be listed.
    entries: Result<Vec<Entry>, String>,
}

/// A file or folder in the folder shown.
struct Entry {
    path: PathBuf,
    /// Its name, as it shows: what of it isn't Unicode replaced.
    name: String,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Folder,
    File,
}

impl Assets {
    /// The panel, showing the folder `root`, which needn't be there: the panel says why it can't
    /// list one that isn't.
    pub fn new(root: PathBuf) -> Self {
        Self {
            folder: root.clone(),
            root,
            listed: None,
        }
    }

    /// Draws the panel into `ui`: Back, which shows the folder above, but at the root, and where
    /// the folder shown is; then its entries, or why it couldn't be listed. Lists the folder first,
    /// unless that was less than a second ago. What a click chooses shows from the next frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let fresh = |listed: &Listed| now.duration_since(listed.at) < RELIST;
        if !self.listed.as_ref().is_some_and(fresh) {
            self.list(now);
        }
        let mut chosen = None;
        ui.horizontal(|ui| {
            let above = self.folder.parent().filter(|_| self.folder != self.root);
            let back = ui.add_enabled(above.is_some(), Button::new(back()));
            if back.on_hover_text("Back to the folder above").clicked() {
                chosen = above.map(Path::to_path_buf);
            }
            ui.label(self.place());
        });
        ui.separator();
        if let Some(Listed { entries, .. }) = &self.listed {
            match entries {
                Err(why) => {
                    ui.colored_label(ui.visuals().error_fg_color, why.as_str());
                }
                Ok(entries) if entries.is_empty() => {
                    ui.weak("Empty");
                }
                Ok(entries) => {
                    // Drawn whether or not Back chose a folder.
                    let opened = cells(ui, &self.folder, entries);
                    chosen = chosen.or(opened);
                }
            }
        }
        if let Some(folder) = chosen {
            self.folder = folder;
            self.listed = None;
        }
    }

    /// Where the folder shown is: the root's name, then the folders under it down to the one
    /// shown, such as `assets/scenes`.
    fn place(&self) -> String {
        let name = |path: &Path| path.file_name().unwrap_or(path.as_os_str()).to_owned();
        let mut place = name(&self.root).to_string_lossy().into_owned();
        let under = self.folder.strip_prefix(&self.root);
        for part in under.unwrap_or(Path::new("")) {
            place.push('/');
            place.push_str(&part.to_string_lossy());
        }
        place
    }

    /// Lists the folder shown, as of `now`, and warns if it can't, unless it couldn't the last time
    /// either, for the same reason.
    fn list(&mut self, now: Instant) {
        let folder = &self.folder;
        let listed = entries(folder);
        let entries = listed.map_err(|error| format!("Can't list {}: {error}", folder.display()));
        if let Err(why) = &entries {
            let before = (self.listed.as_ref()).and_then(|listed| listed.entries.as_ref().err());
            if before != Some(why) {
                warn!(code = "editor.unlisted_folder", path = %folder.display(), "{why}");
            }
        }
        self.listed = Some(Listed { at: now, entries });
    }
}

/// The files and folders in `folder`, folders first, then files, each by its name regardless of
/// case, then as it's spelled. A link is what it leads to, and one that leads nowhere a file.
///
/// It's listed on the main thread, which a folder of many thousands of files would hold up for
/// some milliseconds each second it shows; a thread watching the folder wouldn't.
fn entries(folder: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        // Listing gives each entry's type without following links; only a link takes a call more.
        let file_type = entry.file_type()?;
        let is_folder = if file_type.is_symlink() {
            path.is_dir()
        } else {
            file_type.is_dir()
        };
        let kind = if is_folder { Kind::Folder } else { Kind::File };
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(Entry { path, name, kind });
    }
    entries.sort_by_cached_key(|entry| {
        let file = entry.kind != Kind::Folder;
        (file, entry.name.to_lowercase(), entry.name.clone())
    });
    Ok(entries)
}

/// Draws `entries`, those of `folder`, into `ui`, in rows as many across as fit, scrolling down,
/// and gives the folder a double click chose, if one did.
fn cells(ui: &mut egui::Ui, folder: &Path, entries: &[Entry]) -> Option<PathBuf> {
    let gap = ui.spacing().item_spacing.x;
    let across = (ui.available_width() + gap) / (CELL + gap);
    let across = across.floor().max(1.0) as usize;
    let rows = entries.len().div_ceil(across);
    let mut chosen = None;
    // A scroll of each folder's own, so that one shows from its top.
    let scroll = ScrollArea::vertical().id_salt(folder).auto_shrink(false);
    scroll.show_rows(ui, cell_height(ui), rows, |ui, rows| {
        for row in rows {
            let start = row * across;
            let end = (start + across).min(entries.len());
            ui.horizontal(|ui| {
                for entry in &entries[start..end] {
                    if let Some(folder) = cell(ui, entry) {
                        chosen = Some(folder);
                    }
                }
            });
        }
    });
    chosen
}

/// Draws `entry`'s cell into `ui`, and gives its folder if it's a folder double-clicked. A name cut
/// short to fit gives itself whole as the cell's tooltip.
fn cell(ui: &mut egui::Ui, entry: &Entry) -> Option<PathBuf> {
    // By its path, so that no two cells share one, however alike they look.
    let id = Id::unique(("gg editor asset", &entry.path));
    let (rect, cut) = draw(ui, entry);
    let response = ui.interact(rect, id, Sense::CLICK);
    let response = if cut {
        response.on_hover_text(entry.name.as_str())
    } else {
        response
    };
    (entry.kind == Kind::Folder && response.double_clicked()).then(|| entry.path.clone())
}

/// The height of a cell in `ui`: its icon, and a line of text under it.
fn cell_height(ui: &egui::Ui) -> f32 {
    3.0 * PAD + ICON + ui.text_style_height(&TextStyle::Body)
}

/// Places `entry`'s cell in `ui` and paints it: lit while the pointer's over it, its icon, and
/// under it its name, cut short with an ellipsis if it's wider than the cell. Gives the cell's
/// rect, and whether the name was cut short.
fn draw(ui: &mut egui::Ui, entry: &Entry) -> (Rect, bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(CELL, cell_height(ui)), Sense::hover());
    let width = CELL - 2.0 * PAD;
    let name = WidgetText::from(entry.name.as_str());
    let galley = name.into_galley(ui, Some(TextWrapMode::Truncate), width, TextStyle::Body);
    let cut = galley.elided;
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        if ui.rect_contains_pointer(rect) {
            let lit = &visuals.widgets.hovered;
            painter.rect_filled(rect, lit.corner_radius, lit.weak_bg_fill);
        }
        let color = match entry.kind {
            Kind::Folder => visuals.text_color(),
            Kind::File => visuals.weak_text_color(),
        };
        let icon = Rect::from_min_size(
            pos2(rect.center().x - ICON / 2.0, rect.min.y + PAD),
            Vec2::splat(ICON),
        );
        paint(painter, icon, entry.kind, color);
        let at = pos2(rect.center().x - galley.size().x / 2.0, icon.max.y + PAD);
        painter.galley(at, galley, color);
    }
    (rect, cut)
}

/// Paints the icon of an entry of `kind` in `rect`, a square, in `color`, as shapes, as the
/// toolbar's are: a folder, or a page with its corner folded.
fn paint(painter: &Painter, rect: Rect, kind: Kind, color: Color32) {
    let scale = rect.width() / 16.0;
    // From the top left of a square 16 across.
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
    match kind {
        Kind::Folder => {
            let tab = vec![at(1.5, 2.5), at(6.0, 2.5), at(7.5, 4.0), at(1.5, 4.0)];
            painter.add(Shape::convex_polygon(tab, color, Stroke::NONE));
            let body = Rect::from_min_max(at(1.5, 4.0), at(14.5, 13.0));
            painter.rect_filled(body, 0.75 * scale, color);
        }
        Kind::File => {
            let line = Stroke::new(0.75 * scale, color);
            let page = vec![
                at(3.5, 1.5),
                at(9.5, 1.5),
                at(12.5, 4.5),
                at(12.5, 14.5),
                at(3.5, 14.5),
            ];
            painter.add(Shape::closed_line(page, line));
            let fold = vec![at(9.5, 1.5), at(9.5, 4.5), at(12.5, 4.5)];
            painter.add(Shape::line(fold, line));
        }
    }
}

/// Back's icon: an arrow pointing left, painted as the toolbar's icons are.
fn back() -> Atom<'static> {
    Atom::paint(Vec2::splat(BACK), |ui, args| {
        let (rect, color) = (args.rect, args.fallback_text_color);
        let scale = rect.width() / 16.0;
        let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
        let painter = ui.painter();
        painter.line_segment([at(5.0, 8.0), at(14.0, 8.0)], Stroke::new(1.5, color));
        let head = vec![at(1.5, 8.0), at(6.5, 3.5), at(6.5, 12.5)];
        painter.add(Shape::convex_polygon(head, color, Stroke::NONE));
    })
}

#[cfg(test)]
mod tests {
    use gg_engine::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    use super::*;

    /// A directory of a test's own in the OS's temporary directory, made empty, and removed as the
    /// test ends, failed or not.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-editor-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).expect("made");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The text drawn in `shapes`, a frame's output, each with where it was drawn.
    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        fn find(shape: &egui::Shape, texts: &mut Vec<(String, Rect)>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        find(shape, texts);
                    }
                }
                egui::Shape::Text(text) => {
                    texts.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in shapes {
            find(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Where `text` was drawn among `texts`.
    fn find(texts: &[(String, Rect)], text: &str) -> Rect {
        let found = texts.iter().find(|(drawn, _)| drawn == text);
        found.unwrap_or_else(|| panic!("no {text}: {texts:?}")).1
    }

    /// Folders come first, then files, each by its name regardless of case, then as it's spelled,
    /// names that aren't Latin among them; a folder's name never makes it a file.
    #[test]
    fn a_folder_lists_its_folders_then_its_files_by_name() {
        let scratch = Scratch::new("assets-listed");
        for folder in ["b", "A", "maps.ron"] {
            fs::create_dir(scratch.0.join(folder)).expect("made");
        }
        for file in ["уровень.ron", "c.png", "B.ron", "a.RON", "Z.txt"] {
            fs::write(scratch.0.join(file), "").expect("written");
        }
        let listed = entries(&scratch.0).expect("listed");
        let listed: Vec<(&str, Kind)> = (listed.iter())
            .map(|entry| (entry.name.as_str(), entry.kind))
            .collect();
        let expected = [
            ("A", Kind::Folder),
            ("b", Kind::Folder),
            ("maps.ron", Kind::Folder),
            ("a.RON", Kind::File),
            ("B.ron", Kind::File),
            ("c.png", Kind::File),
            ("Z.txt", Kind::File),
            ("уровень.ron", Kind::File),
        ];
        assert_eq!(listed, expected);
    }

    /// A frame of `assets` filling a window 400 points across, with `events`; the text it drew.
    fn frame(ctx: &egui::Context, assets: &mut Assets, events: Vec<Event>) -> Vec<(String, Rect)> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
            events,
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| assets.show(ui));
        // As the renderer would have.
        output.textures_delta.clear();
        texts(&output.shapes)
    }

    /// Clicks at `at`, `times` over, a frame an event, as egui gets them.
    fn click(ctx: &egui::Context, assets: &mut Assets, at: Pos2, times: usize) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(ctx, assets, vec![Event::PointerMoved(at)]);
        for _ in 0..times {
            frame(ctx, assets, vec![button(true)]);
            frame(ctx, assets, vec![button(false)]);
        }
    }

    /// A double click on a folder shows it from the next frame, where the panel says where it is,
    /// and Back the folder above, as far up as the root, where Back is off.
    #[test]
    fn a_double_click_shows_a_folder_and_back_the_one_above() {
        let scratch = Scratch::new("assets-browsed");
        let scenes = scratch.0.join("scenes");
        fs::create_dir(&scenes).expect("made");
        fs::write(scenes.join("level.ron"), "").expect("written");
        let root = scratch.0.file_name().expect("named").to_string_lossy();
        let mut assets = Assets::new(scratch.0.clone());
        let ctx = egui::Context::default();
        frame(&ctx, &mut assets, Vec::new());
        let shown = frame(&ctx, &mut assets, Vec::new());
        // Left of where it is, inside Back's button.
        let place = find(&shown, &root);
        let back = Pos2::new(place.min.x - 16.0, place.center().y);
        click(&ctx, &mut assets, find(&shown, "scenes").center(), 1);
        assert_eq!(assets.folder, scratch.0, "a single click");
        click(&ctx, &mut assets, find(&shown, "scenes").center(), 2);
        assert_eq!(assets.folder, scenes);
        let shown = frame(&ctx, &mut assets, Vec::new());
        find(&shown, "level.ron");
        find(&shown, &format!("{root}/scenes"));
        for _ in 0..2 {
            click(&ctx, &mut assets, back, 1);
            assert_eq!(assets.folder, scratch.0);
        }
        find(&frame(&ctx, &mut assets, Vec::new()), "scenes");
    }

    /// Has `assets`' listing be a second old, as though it was listed a second earlier.
    fn age(assets: &mut Assets) {
        let listed = assets.listed.as_mut().expect("listed");
        listed.at = listed.at.checked_sub(RELIST).expect("a second earlier");
    }

    /// The folder shown is listed again once its listing is a second old, so that a file added
    /// shows then; and one that can't be listed, as once it's gone, says why.
    #[test]
    fn the_folder_shown_is_listed_again_a_second_on() {
        let scratch = Scratch::new("assets-relisted");
        let mut assets = Assets::new(scratch.0.clone());
        let ctx = egui::Context::default();
        let drew = |shown: &[(String, Rect)], text: &str| shown.iter().any(|(t, _)| t == text);
        assert!(drew(&frame(&ctx, &mut assets, Vec::new()), "Empty"));
        fs::write(scratch.0.join("level.ron"), "").expect("written");
        // As though it was listed just now, however long the first frame took.
        assets.listed.as_mut().expect("listed").at = Instant::now();
        let shown = frame(&ctx, &mut assets, Vec::new());
        assert!(!drew(&shown, "level.ron"), "not yet a second on");
        age(&mut assets);
        find(&frame(&ctx, &mut assets, Vec::new()), "level.ron");
        fs::remove_dir_all(&scratch.0).expect("removed");
        age(&mut assets);
        let shown = frame(&ctx, &mut assets, Vec::new());
        let why = |(text, _): &(String, Rect)| text.starts_with("Can't list");
        assert!(shown.iter().any(why), "{shown:?}");
    }
}
